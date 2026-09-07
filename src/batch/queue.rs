//! Headless batch processing queue manager and directory scanner.

use anyhow::{anyhow, Result};
use crate::funscript::{AxisChannel, Funscript, MultiAxisScript};
use crate::signal;
use crate::tracking::{self, FlowScratchContext};
use crate::video::{self, FrameStreamReader, StreamConfig};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BatchJobStatus {
    Queued,
    Processing { progress: f32, current_frame: u64 },
    Completed { actions_count: usize, elapsed_secs: f64 },
    Failed { error: String },
    Skipped { reason: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchJobConfig {
    pub model_path: Option<PathBuf>,
    pub conf: f32,
    pub fps: f64,
    pub multi_axis: bool,
    pub vr_mode: bool,
    pub pov_mode: bool,
    pub overwrite: bool,
}

impl Default for BatchJobConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            conf: 0.35,
            fps: 30.0,
            multi_axis: true,
            vr_mode: false,
            pov_mode: false,
            overwrite: false,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchJob {
    pub id: usize,
    pub video_path: PathBuf,
    pub output_path: PathBuf,
    pub status: BatchJobStatus,
}

#[derive(Debug, Clone)]
pub enum BatchProgressUpdate {
    JobStarted { id: usize },
    JobProgress { id: usize, progress: f32, frame: u64 },
    JobCompleted { id: usize, actions_count: usize, elapsed_secs: f64 },
    JobFailed { id: usize, error: String },
    AllJobsCompleted,
}

/// Batch queue manager handling directory scanning and execution tracking
#[derive(Debug, Clone, Default)]
pub struct BatchQueue {
    pub jobs: Vec<BatchJob>,
    pub next_job_id: usize,
}

impl BatchQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all completed or queued jobs
    pub fn clear(&mut self) {
        self.jobs.clear();
        self.next_job_id = 0;
    }

    /// Recursively scan a folder for videos and enqueue jobs for missing scripts
    pub fn scan_directory(&mut self, root_dir: &Path, recursive: bool, config: &BatchJobConfig) -> usize {
        let mut added = 0;
        let mut dirs_to_visit = vec![root_dir.to_path_buf()];

        let video_exts = ["mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv"];

        while let Some(dir) = dirs_to_visit.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && recursive {
                    dirs_to_visit.push(path);
                } else if path.is_file() {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();

                    if video_exts.contains(&ext.as_str()) {
                        let script_path = path.with_extension("funscript");
                        let exists = script_path.exists();

                        if !exists || config.overwrite {
                            // Check if not already in queue
                            if !self.jobs.iter().any(|j| j.video_path == path) {
                                self.jobs.push(BatchJob {
                                    id: self.next_job_id,
                                    video_path: path,
                                    output_path: script_path,
                                    status: BatchJobStatus::Queued,
                                });
                                self.next_job_id += 1;
                                added += 1;
                            }
                        }
                    }
                }
            }
        }

        added
    }

    /// Return statistics: (total, queued, processing, completed, failed, skipped)
    pub fn stats(&self) -> (usize, usize, usize, usize, usize, usize) {
        let mut queued = 0;
        let mut processing = 0;
        let mut completed = 0;
        let mut failed = 0;
        let mut skipped = 0;

        for job in &self.jobs {
            match job.status {
                BatchJobStatus::Queued => queued += 1,
                BatchJobStatus::Processing { .. } => processing += 1,
                BatchJobStatus::Completed { .. } => completed += 1,
                BatchJobStatus::Failed { .. } => failed += 1,
                BatchJobStatus::Skipped { .. } => skipped += 1,
            }
        }

        (self.jobs.len(), queued, processing, completed, failed, skipped)
    }
}

/// Multi-threaded batch worker thread handle
pub struct BatchWorker {
    pub is_running: Arc<AtomicBool>,
    pub rx: Receiver<BatchProgressUpdate>,
    worker_handle: Option<JoinHandle<()>>,
}

impl BatchWorker {
    pub fn new() -> Self {
        let (_tx, rx) = channel();
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            rx,
            worker_handle: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    pub fn stop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
        if let Some(h) = self.worker_handle.take() {
            let _ = h.join();
        }
    }

    /// Start processing batch queue jobs in background thread
    pub fn start(&mut self, jobs: Vec<BatchJob>, config: BatchJobConfig) {
        self.stop();

        let (tx, rx) = channel();
        self.rx = rx;
        self.is_running.store(true, Ordering::SeqCst);
        let is_running = self.is_running.clone();

        let handle = thread::spawn(move || {
            // Optimization: Initialize neural detector once outside the job loop
            // to reuse the ONNX session across all batch jobs instead of reloading the model per video.
            let mut detector = if let Some(ref m) = config.model_path {
                match crate::neural::NeuralDetector::new(m, config.conf) {
                    Ok(det) => Some(det),
                    Err(err) => {
                        for job in jobs {
                            if job.output_path.exists() && !config.overwrite {
                                continue;
                            }
                            let _ = tx.send(BatchProgressUpdate::JobStarted { id: job.id });
                            let _ = tx.send(BatchProgressUpdate::JobFailed {
                                id: job.id,
                                error: format!("Failed to initialize neural detector: {err}"),
                            });
                        }
                        let _ = tx.send(BatchProgressUpdate::AllJobsCompleted);
                        is_running.store(false, Ordering::SeqCst);
                        return;
                    }
                }
            } else {
                None
            };

            for job in jobs {
                if !is_running.load(Ordering::SeqCst) {
                    break;
                }

                if job.output_path.exists() && !config.overwrite {
                    continue;
                }

                let _ = tx.send(BatchProgressUpdate::JobStarted { id: job.id });
                let t0 = std::time::Instant::now();

                let result: Result<usize> = (|| -> Result<usize> {
                    let width = if config.model_path.is_some() { 640 } else { 256 };
                    let height = if config.model_path.is_some() { 640 } else { 256 };
                    let est_total = video::probe_video(&job.video_path)
                        .ok()
                        .map(|m| (m.duration_secs * config.fps).round().max(1.0) as u64);

                    let stream_cfg = StreamConfig {
                        target_width: width,
                        target_height: height,
                        target_fps: config.fps,
                        vr_mode: config.vr_mode,
                        is_rgb: config.model_path.is_some(),
                    };

                    let mut stream = FrameStreamReader::new(&job.video_path, &stream_cfg)?;
                    let mut all_samples = Vec::new();
                    let mut surge_samples = Vec::new();
                    let mut sway_samples = Vec::new();
                    let mut pitch_samples = Vec::new();
                    let mut roll_samples = Vec::new();
                    let mut neural_poses: Vec<(crate::neural::Pose3D, i64)> = Vec::new();

                    let first = stream.next_frame()?.ok_or_else(|| anyhow!("Zero video frames decoded"))?;
                    let mut last_raw = first.0;
                    // Optimization: Maintain a persistent grayscale buffer across iterations
                    // instead of recomputing grayscale on last_raw every frame.
                    let mut last_gray = if stream_cfg.is_rgb {
                        Some(video::rgb_to_gray(&last_raw))
                    } else {
                        None
                    };

                    let mut tracker = if config.model_path.is_some() {
                        Some(crate::neural::tracker::AnatomicalTracker::new())
                    } else {
                        None
                    };

                    let mut flow_ctx = FlowScratchContext::new(width as usize, height as usize);
                    let mut frame_count = 0u64;
                    while let Some((mut curr_raw, ts)) = stream.next_frame()? {
                        if !is_running.load(Ordering::SeqCst) {
                            break;
                        }
                        frame_count += 1;
                        if frame_count.is_multiple_of(30) {
                            let progress = est_total
                                .map(|tot| (frame_count as f32 / tot as f32).min(0.99))
                                .unwrap_or(0.5);
                            let _ = tx.send(BatchProgressUpdate::JobProgress {
                                id: job.id,
                                progress,
                                frame: frame_count,
                            });
                        }

                        // Optimization: When RGB, convert only curr_raw to gray and borrow slices.
                        // When already grayscale, borrow slices directly from last_raw and curr_raw without cloning.
                        let curr_gray = if stream_cfg.is_rgb {
                            Some(video::rgb_to_gray(&curr_raw))
                        } else {
                            None
                        };

                        let (prev_gray, curr_gray_ref) = if stream_cfg.is_rgb {
                            (last_gray.as_ref().unwrap().as_slice(), curr_gray.as_ref().unwrap().as_slice())
                        } else {
                            (last_raw.as_slice(), curr_raw.as_slice())
                        };

                        let is_cut_diff = tracking::detect_cut_photometric(prev_gray, curr_gray_ref, 30.0);
                        let flow = tracking::compute_dense_flow(prev_gray, curr_gray_ref, width as usize, height as usize, &mut flow_ctx);
                        let is_cut = is_cut_diff || (flow.mean_magnitude() > 8.0);

                        let center = if config.pov_mode {
                            ((width / 2) as f32, (height - 1) as f32)
                        } else {
                            let (cx, cy, _) = tracking::find_divergence_center(&flow);
                            (cx, cy)
                        };

                        let motion_dot = tracking::project_adaptive_motion(&flow, center, is_cut, config.pov_mode, true);
                        all_samples.push((motion_dot, is_cut, ts));

                        if config.multi_axis {
                            let (surge, sway, pitch, roll) = tracking::project_companion_motion(&flow, center, is_cut, config.pov_mode, true);
                            surge_samples.push((surge, is_cut, ts));
                            sway_samples.push((sway, is_cut, ts));
                            pitch_samples.push((pitch, is_cut, ts));
                            roll_samples.push((roll, is_cut, ts));
                        }

                        if let (Some(det), Some(ref mut trk)) = (detector.as_mut(), &mut tracker) {
                            let detections = det.detect(&curr_raw, width as usize, height as usize)?;
                            let (pose, _) = trk.update_pose(&detections, Some(&flow));
                            neural_poses.push((pose, ts));
                        }

                        // Advance state for next iteration: avoid redundant cloning and re-conversion
                        if stream_cfg.is_rgb {
                            last_gray = curr_gray;
                        } else {
                            std::mem::swap(&mut last_raw, &mut curr_raw);
                        }
                    }

                    let mut bundle = MultiAxisScript::new();

                    if config.model_path.is_some() && !neural_poses.is_empty() {
                        // Optimization: Single-pass multi-channel extraction to populate
                        // all axis vectors simultaneously rather than iterating over neural_poses 8 separate times.
                        let n = neural_poses.len();
                        let mut timestamps = Vec::with_capacity(n);
                        let mut stroke_values = Vec::with_capacity(n);
                        let mut surge_values = if config.multi_axis { Vec::with_capacity(n) } else { Vec::new() };
                        let mut sway_values = if config.multi_axis { Vec::with_capacity(n) } else { Vec::new() };
                        let mut pitch_values = if config.multi_axis { Vec::with_capacity(n) } else { Vec::new() };
                        let mut roll_values = if config.multi_axis { Vec::with_capacity(n) } else { Vec::new() };
                        let mut twist_values = if config.multi_axis { Vec::with_capacity(n) } else { Vec::new() };
                        let mut suction_values = if config.multi_axis { Vec::with_capacity(n) } else { Vec::new() };

                        if config.multi_axis {
                            for (p, ts) in &neural_poses {
                                timestamps.push(*ts);
                                stroke_values.push(p.stroke);
                                surge_values.push(p.surge);
                                sway_values.push(p.sway);
                                pitch_values.push(p.pitch);
                                roll_values.push(p.roll);
                                twist_values.push(p.twist);
                                suction_values.push(p.suction);
                            }
                        } else {
                            for (p, ts) in &neural_poses {
                                timestamps.push(*ts);
                                stroke_values.push(p.stroke);
                            }
                        }

                        let mut stroke_script = Funscript::new(signal::extract_actions_full_range(&stroke_values, &timestamps, 3.5, true));
                        stroke_script.sanitize();
                        bundle.channels.insert(AxisChannel::Stroke, stroke_script);

                        if config.multi_axis {
                            let mut surge_script = Funscript::new(signal::extract_actions(&surge_values, &timestamps, true));
                            surge_script.sanitize();
                            bundle.channels.insert(AxisChannel::Surge, surge_script);

                            let mut sway_script = Funscript::new(signal::extract_actions(&sway_values, &timestamps, true));
                            sway_script.sanitize();
                            bundle.channels.insert(AxisChannel::Sway, sway_script);

                            let mut pitch_script = Funscript::new(signal::extract_actions(&pitch_values, &timestamps, true));
                            pitch_script.sanitize();
                            bundle.channels.insert(AxisChannel::Pitch, pitch_script);

                            let mut roll_script = Funscript::new(signal::extract_actions(&roll_values, &timestamps, true));
                            roll_script.sanitize();
                            bundle.channels.insert(AxisChannel::Roll, roll_script);

                            let mut twist_script = Funscript::new(signal::extract_actions(&twist_values, &timestamps, true));
                            twist_script.sanitize();
                            bundle.channels.insert(AxisChannel::Twist, twist_script);

                            let mut suction_script = Funscript::new(signal::extract_actions(&suction_values, &timestamps, true));
                            suction_script.sanitize();
                            bundle.channels.insert(AxisChannel::Suction, suction_script);
                        }
                    } else {
                        let (cum, ts) = signal::integrate_flow(&all_samples);
                        let mut stroke_script = Funscript::new(signal::extract_actions_full_range(&cum, &ts, 0.8, true));
                        stroke_script.sanitize();
                        bundle.channels.insert(AxisChannel::Stroke, stroke_script);

                        if config.multi_axis {
                            let (surge_cum, _) = signal::integrate_flow(&surge_samples);
                            let surge_norm = signal::detrend_and_normalize(&surge_cum, config.fps, 2.0, 3.0, 50.0);
                            let mut surge_script = Funscript::new(signal::extract_actions(&surge_norm, &ts, true));
                            surge_script.sanitize();
                            bundle.channels.insert(AxisChannel::Surge, surge_script);

                            let (sway_cum, _) = signal::integrate_flow(&sway_samples);
                            let sway_norm = signal::detrend_and_normalize(&sway_cum, config.fps, 2.0, 3.0, 50.0);
                            let mut sway_script = Funscript::new(signal::extract_actions(&sway_norm, &ts, true));
                            sway_script.sanitize();
                            bundle.channels.insert(AxisChannel::Sway, sway_script);

                            let (pitch_cum, _) = signal::integrate_flow(&pitch_samples);
                            let pitch_norm = signal::detrend_and_normalize(&pitch_cum, config.fps, 2.0, 3.0, 50.0);
                            let mut pitch_script = Funscript::new(signal::extract_actions(&pitch_norm, &ts, true));
                            pitch_script.sanitize();
                            bundle.channels.insert(AxisChannel::Pitch, pitch_script);

                            let (roll_cum, _) = signal::integrate_flow(&roll_samples);
                            let roll_norm = signal::detrend_and_normalize(&roll_cum, config.fps, 2.0, 3.0, 50.0);
                            let mut roll_script = Funscript::new(signal::extract_actions(&roll_norm, &ts, true));
                            roll_script.sanitize();
                            bundle.channels.insert(AxisChannel::Roll, roll_script);
                        }
                    }

                    if config.multi_axis && bundle.channels.len() > 1 {
                        bundle.save_bundle(&job.output_path)?;
                    } else if let Some(stroke) = bundle.get(AxisChannel::Stroke) {
                        stroke.save(&job.output_path)?;
                    }

                    let action_count = bundle.get(AxisChannel::Stroke).map(|s| s.actions.len()).unwrap_or(0);
                    Ok(action_count)
                })();

                match result {
                    Ok(actions_count) => {
                        let _ = tx.send(BatchProgressUpdate::JobCompleted {
                            id: job.id,
                            actions_count,
                            elapsed_secs: t0.elapsed().as_secs_f64(),
                        });
                    }
                    Err(err) => {
                        let _ = tx.send(BatchProgressUpdate::JobFailed {
                            id: job.id,
                            error: err.to_string(),
                        });
                    }
                }
            }

            let _ = tx.send(BatchProgressUpdate::AllJobsCompleted);
            is_running.store(false, Ordering::SeqCst);
        });

        self.worker_handle = Some(handle);
    }
}

impl Default for BatchWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_batch_queue_lifecycle() {
        let mut queue = BatchQueue::new();
        assert_eq!(queue.jobs.len(), 0);

        let temp_dir = std::env::temp_dir().join("fs_hub_batch_test");
        let _ = fs::create_dir_all(&temp_dir);

        // Create dummy video files
        let vid1 = temp_dir.join("clip1.mp4");
        let vid2 = temp_dir.join("clip2.mkv");
        let _ = File::create(&vid1);
        let _ = File::create(&vid2);

        let cfg = BatchJobConfig::default();
        let added = queue.scan_directory(&temp_dir, false, &cfg);
        assert_eq!(added, 2);
        assert_eq!(queue.jobs.len(), 2);

        let (total, queued, _, _, _, _) = queue.stats();
        assert_eq!(total, 2);
        assert_eq!(queued, 2);

        // Clean up
        let _ = fs::remove_file(vid1);
        let _ = fs::remove_file(vid2);
        let _ = fs::remove_dir(temp_dir);
    }

    #[test]
    fn test_batch_worker_execution() {
        let mut worker = BatchWorker::new();
        assert!(!worker.is_active());

        let temp_dir = std::env::temp_dir().join("fs_hub_worker_test");
        let _ = fs::create_dir_all(&temp_dir);
        let vid = temp_dir.join("nonexistent_or_empty.mp4");
        let _ = File::create(&vid);

        let job = BatchJob {
            id: 1,
            video_path: vid.clone(),
            output_path: temp_dir.join("nonexistent_or_empty.funscript"),
            status: BatchJobStatus::Queued,
        };

        worker.start(vec![job], BatchJobConfig::default());
        assert!(worker.is_active());

        let mut received_completed_signal = false;
        let mut received_job_event = false;

        let start_time = std::time::Instant::now();
        while start_time.elapsed().as_secs() < 5 {
            if let Ok(msg) = worker.rx.recv_timeout(std::time::Duration::from_millis(200)) {
                match msg {
                    BatchProgressUpdate::JobStarted { id } => {
                        assert_eq!(id, 1);
                        received_job_event = true;
                    }
                    BatchProgressUpdate::JobFailed { id, error } => {
                        assert_eq!(id, 1);
                        assert!(!error.is_empty());
                    }
                    BatchProgressUpdate::AllJobsCompleted => {
                        received_completed_signal = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        assert!(received_job_event);
        assert!(received_completed_signal);
        worker.stop();
        assert!(!worker.is_active());

        let _ = fs::remove_file(vid);
        let _ = fs::remove_dir(temp_dir);
    }

    #[test]
    fn test_batch_worker_with_invalid_model_path() {
        let mut worker = BatchWorker::new();
        let job = BatchJob {
            id: 42,
            video_path: PathBuf::from("nonexistent.mp4"),
            output_path: PathBuf::from("nonexistent.funscript"),
            status: BatchJobStatus::Queued,
        };

        let mut cfg = BatchJobConfig::default();
        cfg.model_path = Some(PathBuf::from("definitely_nonexistent_model.onnx"));

        worker.start(vec![job], cfg);

        let mut got_failed = false;
        let mut got_all_completed = false;
        let start_time = std::time::Instant::now();
        while start_time.elapsed().as_secs() < 5 {
            if let Ok(msg) = worker.rx.recv_timeout(std::time::Duration::from_millis(200)) {
                match msg {
                    BatchProgressUpdate::JobFailed { id, error } => {
                        assert_eq!(id, 42);
                        assert!(error.contains("Failed to initialize neural detector"));
                        got_failed = true;
                    }
                    BatchProgressUpdate::AllJobsCompleted => {
                        got_all_completed = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        assert!(got_failed);
        assert!(got_all_completed);
        worker.stop();
    }
}
