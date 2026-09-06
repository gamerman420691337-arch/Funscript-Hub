//! Headless batch processing queue manager and directory scanner.

use anyhow::{anyhow, Result};
use crate::funscript::{AxisChannel, Funscript, MultiAxisScript};
use crate::signal;
use crate::tracking;
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
                    let mut neural_poses: Vec<(crate::neural::Pose3D, i64)> = Vec::new();

                    let first = stream.next_frame()?.ok_or_else(|| anyhow!("Zero video frames decoded"))?;
                    let mut last_raw = first.0;

                    let mut detector = if let Some(ref m) = config.model_path {
                        Some(crate::neural::NeuralDetector::new(m, config.conf)?)
                    } else {
                        None
                    };
                    let mut tracker = if config.model_path.is_some() {
                        Some(crate::neural::tracker::AnatomicalTracker::new())
                    } else {
                        None
                    };

                    let mut frame_count = 0u64;
                    while let Some((curr_raw, ts)) = stream.next_frame()? {
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

                        let (last_gray, curr_gray) = if stream_cfg.is_rgb {
                            (
                                video::rgb_to_gray(&last_raw),
                                video::rgb_to_gray(&curr_raw),
                            )
                        } else {
                            (last_raw.clone(), curr_raw.clone())
                        };

                        let is_cut_diff = tracking::detect_cut_photometric(&last_gray, &curr_gray, 30.0);
                        let flow = tracking::compute_dense_flow(&last_gray, &curr_gray, width as usize, height as usize);
                        let is_cut = is_cut_diff || (flow.mean_magnitude() > 8.0);

                        let center = if config.pov_mode {
                            ((width / 2) as f32, (height - 1) as f32)
                        } else {
                            let (cx, cy, _) = tracking::find_divergence_center(&flow);
                            (cx, cy)
                        };

                        let radial_dot = tracking::project_radial_motion(&flow, center, is_cut, config.pov_mode, true);
                        all_samples.push((radial_dot, is_cut, ts));

                        if let (Some(ref mut det), Some(ref mut trk)) = (&mut detector, &mut tracker) {
                            let detections = det.detect(&curr_raw, width as usize, height as usize)?;
                            let (pose, _) = trk.update_pose(&detections, Some(&flow));
                            neural_poses.push((pose, ts));
                        }

                        last_raw = curr_raw;
                    }

                    let mut bundle = MultiAxisScript::new();

                    if config.model_path.is_some() && !neural_poses.is_empty() {
                        let timestamps: Vec<i64> = neural_poses.iter().map(|(_, ts)| *ts).collect();
                        let stroke_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.stroke).collect();
                        let mut stroke_script = Funscript::new(signal::extract_actions(&stroke_values, &timestamps, true));
                        stroke_script.sanitize();
                        bundle.channels.insert(AxisChannel::Stroke, stroke_script);

                        if config.multi_axis {
                            let surge_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.surge).collect();
                            let mut surge_script = Funscript::new(signal::extract_actions(&surge_values, &timestamps, true));
                            surge_script.sanitize();
                            bundle.channels.insert(AxisChannel::Surge, surge_script);

                            let sway_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.sway).collect();
                            let mut sway_script = Funscript::new(signal::extract_actions(&sway_values, &timestamps, true));
                            sway_script.sanitize();
                            bundle.channels.insert(AxisChannel::Sway, sway_script);

                            let pitch_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.pitch).collect();
                            let mut pitch_script = Funscript::new(signal::extract_actions(&pitch_values, &timestamps, true));
                            pitch_script.sanitize();
                            bundle.channels.insert(AxisChannel::Pitch, pitch_script);

                            let roll_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.roll).collect();
                            let mut roll_script = Funscript::new(signal::extract_actions(&roll_values, &timestamps, true));
                            roll_script.sanitize();
                            bundle.channels.insert(AxisChannel::Roll, roll_script);

                            let twist_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.twist).collect();
                            let mut twist_script = Funscript::new(signal::extract_actions(&twist_values, &timestamps, true));
                            twist_script.sanitize();
                            bundle.channels.insert(AxisChannel::Twist, twist_script);

                            let suction_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.suction).collect();
                            let mut suction_script = Funscript::new(signal::extract_actions(&suction_values, &timestamps, true));
                            suction_script.sanitize();
                            bundle.channels.insert(AxisChannel::Suction, suction_script);
                        }
                    } else {
                        let (cum, ts) = signal::integrate_flow(&all_samples);
                        let norm = signal::detrend_and_normalize(&cum, config.fps, 1.5, 3.0, 1000.0);
                        let mut stroke_script = Funscript::new(signal::extract_actions(&norm, &ts, true));
                        stroke_script.sanitize();
                        bundle.channels.insert(AxisChannel::Stroke, stroke_script);
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
}
