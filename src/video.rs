//! Video decoding and frame streaming using high-performance FFmpeg pipes.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_secs: f64,
    pub total_frames: u64,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    nb_frames: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

/// Query video metadata via ffprobe
pub fn probe_video<P: AsRef<Path>>(video_path: P) -> Result<VideoMetadata> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,nb_frames,duration:format=duration",
            "-of",
            "json",
        ])
        .arg(video_path.as_ref())
        .output()
        .with_context(|| "Failed to execute ffprobe. Ensure ffmpeg/ffprobe is installed.")?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("ffprobe failed: {}", err);
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .with_context(|| "Failed to parse ffprobe JSON output")?;

    let stream = parsed
        .streams
        .first()
        .context("No video streams found in media file")?;

    let width = stream.width.unwrap_or(0);
    let height = stream.height.unwrap_or(0);

    let fps = if let Some(ref r_fps) = stream.r_frame_rate {
        parse_fps_fraction(r_fps).unwrap_or(30.0)
    } else {
        30.0
    };

    let duration_secs = stream
        .duration
        .as_deref()
        .or_else(|| parsed.format.as_ref().and_then(|f| f.duration.as_deref()))
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);

    let total_frames = stream
        .nb_frames
        .as_deref()
        .and_then(|f| f.parse::<u64>().ok())
        .unwrap_or_else(|| (duration_secs * fps).round() as u64);

    Ok(VideoMetadata {
        width,
        height,
        fps,
        duration_secs,
        total_frames,
    })
}

fn parse_fps_fraction(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() == 2 {
        let num = parts[0].parse::<f64>().ok()?;
        let den = parts[1].parse::<f64>().ok()?;
        if den > 0.0 {
            return Some(num / den);
        }
    } else if let Ok(val) = s.parse::<f64>() {
        return Some(val);
    }
    None
}

/// Configuration for frame streaming reader
#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub target_width: u32,
    pub target_height: u32,
    pub target_fps: f64,
    pub vr_mode: bool,
    pub is_rgb: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            target_width: 256,
            target_height: 256,
            target_fps: 30.0,
            vr_mode: false,
            is_rgb: false,
        }
    }
}

/// Convert contiguous RGB bytes into Grayscale bytes (Rec. 601 standard luma)
pub fn rgb_to_gray(rgb: &[u8]) -> Vec<u8> {
    let num_pixels = rgb.len() / 3;
    let mut gray = Vec::with_capacity(num_pixels);
    for chunk in rgb.chunks_exact(3) {
        let r = chunk[0] as u32;
        let g = chunk[1] as u32;
        let b = chunk[2] as u32;
        let y = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
        gray.push(y);
    }
    gray
}

/// High-throughput streaming frame reader
pub struct FrameStreamReader {
    child: Child,
    frame_bytes: usize,
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
    pub height: u32,
    pub fps: f64,
    #[allow(dead_code)]
    pub is_rgb: bool,
    current_frame: u64,
}

impl FrameStreamReader {
    pub fn new<P: AsRef<Path>>(video_path: P, config: &StreamConfig) -> Result<Self> {
        Self::new_at(video_path, config, 0.0)
    }

    pub fn new_at<P: AsRef<Path>>(video_path: P, config: &StreamConfig, start_time_secs: f64) -> Result<Self> {
        let w = config.target_width;
        let h = config.target_height;
        let fps = config.target_fps;
        let is_rgb = config.is_rgb;

        // Build hardware accelerated video filter graph
        let filter = if config.vr_mode {
            // For VR side-by-side or top-bottom, crop bottom-left quadrant first
            format!(
                "crop=in_w/2:in_h/2:0:in_h/2,fps={:.3},scale={}:{}",
                fps, w, h
            )
        } else {
            format!("fps={:.3},scale={}:{}", fps, w, h)
        };

        let pix_fmt = if is_rgb { "rgb24" } else { "gray" };

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-nostdin",
            "-hwaccel",
            "auto",
            // Fast probing: minimize startup latency
            "-probesize", "32768",
            "-analyzeduration", "0",
            "-fflags", "+nobuffer+fastseek",
        ]);
        if start_time_secs > 0.01 {
            cmd.args(["-ss", &format!("{:.3}", start_time_secs)]);
        }
        cmd.arg("-i").arg(video_path.as_ref());
        cmd.args([
            // Disable non-video streams
            "-an", "-sn", "-dn",
            "-vf",
            &filter,
            "-f",
            "rawvideo",
            "-pix_fmt",
            pix_fmt,
            "pipe:1",
        ]);

        let child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| "Failed to spawn ffmpeg child process")?;

        let channels = if is_rgb { 3 } else { 1 };
        let frame_bytes = (w * h * channels) as usize;
        let start_frame = (start_time_secs.max(0.0) * fps).round() as u64;

        Ok(Self {
            child,
            frame_bytes,
            width: w,
            height: h,
            fps,
            is_rgb,
            current_frame: start_frame,
        })
    }

    /// Read the next grayscale frame.
    /// Returns Ok(Some((frame_data, timestamp_ms))) or Ok(None) at EOF.
    pub fn next_frame(&mut self) -> Result<Option<(Vec<u8>, i64)>> {
        let stdout = self
            .child
            .stdout
            .as_mut()
            .context("FFmpeg stdout is closed")?;

        let mut buffer = vec![0u8; self.frame_bytes];
        match stdout.read_exact(&mut buffer) {
            Ok(()) => {
                let ts_ms = ((self.current_frame as f64 / self.fps) * 1000.0).round() as i64;
                self.current_frame += 1;
                Ok(Some((buffer, ts_ms)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e).context("Error reading frame from ffmpeg stream"),
        }
    }
}

impl Drop for FrameStreamReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// High-performance buffered playback streamer that decodes frames sequentially
/// in a background thread and keeps a bounded ring buffer for zero-latency UI display.
pub struct PlaybackStreamer {
    queue: Arc<Mutex<VecDeque<(i64, Arc<Vec<u8>>)>>>,
    seek_target: Arc<AtomicI64>,
    stop_flag: Arc<AtomicBool>,
    _worker: JoinHandle<()>,
}

impl PlaybackStreamer {
    pub fn new<P: AsRef<Path>>(
        video_path: P,
        width: u32,
        height: u32,
        fps: f64,
        start_time_ms: i64,
    ) -> Self {
        let path = video_path.as_ref().to_path_buf();
        let queue = Arc::new(Mutex::new(VecDeque::<(i64, Arc<Vec<u8>>)>::with_capacity(64)));
        let seek_target = Arc::new(AtomicI64::new(start_time_ms.max(0)));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let q_clone = Arc::clone(&queue);
        let seek_clone = Arc::clone(&seek_target);
        let stop_clone = Arc::clone(&stop_flag);

        let config = StreamConfig {
            target_width: width,
            target_height: height,
            target_fps: fps,
            vr_mode: false,
            is_rgb: true,
        };

        let worker = thread::spawn(move || {
            let mut current_reader: Option<FrameStreamReader> = None;

            while !stop_clone.load(Ordering::Relaxed) {
                // Check if a seek was requested
                let seek_req = seek_clone.swap(-1, Ordering::SeqCst);
                if seek_req >= 0 {
                    // Reset existing reader and empty queue
                    current_reader = None;
                    if let Ok(mut q) = q_clone.lock() {
                        q.clear();
                    }
                    let start_sec = (seek_req as f64 / 1000.0).max(0.0);
                    match FrameStreamReader::new_at(&path, &config, start_sec) {
                        Ok(reader) => current_reader = Some(reader),
                        Err(e) => {
                            eprintln!("PlaybackStreamer failed to seek to {:.2}s: {e}", start_sec);
                        }
                    }
                }

                if current_reader.is_none() {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                // Check queue size
                let q_len = q_clone.lock().map(|q| q.len()).unwrap_or(0);
                if q_len >= 45 {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                if let Some(reader) = current_reader.as_mut() {
                    match reader.next_frame() {
                        Ok(Some((bytes, ts))) => {
                            if let Ok(mut q) = q_clone.lock() {
                                q.push_back((ts, Arc::new(bytes)));
                            }
                        }
                        Ok(None) => {
                            // End of file
                            current_reader = None;
                            thread::sleep(Duration::from_millis(15));
                        }
                        Err(_) => {
                            current_reader = None;
                            thread::sleep(Duration::from_millis(15));
                        }
                    }
                }
            }
        });

        Self {
            queue,
            seek_target,
            stop_flag,
            _worker: worker,
        }
    }

    /// Request a seek to a specific millisecond timestamp
    pub fn seek_to(&self, time_ms: i64) {
        self.seek_target.store(time_ms.max(0), Ordering::SeqCst);
    }

    /// Retrieve the frame matching `target_time_ms`.
    /// Automatically advances the ring buffer and discards elapsed frames.
    /// Returns Arc-wrapped frame data for zero-copy sharing with the GUI.
    pub fn get_frame(&self, target_time_ms: i64) -> Option<(i64, Arc<Vec<u8>>)> {
        let mut q = self.queue.lock().ok()?;

        // If target is significantly behind what's queued, auto-seek backwards
        if let Some(&(front_ts, _)) = q.front() {
            if target_time_ms < front_ts - 250 {
                self.seek_to(target_time_ms);
                return None;
            }
        }
        // If target is far ahead (> 1200ms), auto-seek forwards
        if let Some(&(back_ts, _)) = q.back() {
            if target_time_ms > back_ts + 1200 {
                self.seek_to(target_time_ms);
                return None;
            }
        }

        // Discard frames that are strictly in the past when a newer frame is already available
        while q.len() >= 2 && q[1].0 <= target_time_ms {
            q.pop_front();
        }

        // Return current front frame if it is within reasonable temporal range (within 250ms)
        if let Some((ts, rgb)) = q.front() {
            if (target_time_ms - *ts).abs() <= 250 {
                return Some((*ts, Arc::clone(rgb)));
            }
        }

        None
    }
}

impl Drop for PlaybackStreamer {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

/// Extract a single frame at a specific timestamp in milliseconds.
/// Returns RGB24 bytes of dimensions `target_width x target_height`.
pub fn extract_frame_at<P: AsRef<Path>>(
    video_path: P,
    time_ms: i64,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    let time_secs = (time_ms.max(0) as f64 / 1000.0).max(0.0);
    let scale_filter = format!("scale={}:{}", target_width, target_height);

    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-ss",
            &format!("{:.3}", time_secs),
            "-i",
        ])
        .arg(video_path.as_ref())
        .args([
            "-vframes",
            "1",
            "-vf",
            &scale_filter,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .with_context(|| "Failed to execute ffmpeg for single-frame extraction")?;

    let expected_bytes = (target_width * target_height * 3) as usize;
    if output.stdout.len() >= expected_bytes {
        // Zero-copy: truncate the owned buffer instead of cloning
        let mut buf = output.stdout;
        buf.truncate(expected_bytes);
        Ok(buf)
    } else {
        bail!(
            "Insufficient bytes returned by ffmpeg for frame at {}ms (expected {}, got {})",
            time_ms,
            expected_bytes,
            output.stdout.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_frame_from_synth_video() {
        let path = Path::new("/tmp/test_synth_rust.mp4");
        if path.exists() {
            let res = extract_frame_at(path, 500, 64, 64);
            assert!(res.is_ok());
            let bytes = res.unwrap();
            assert_eq!(bytes.len(), 64 * 64 * 3);
        }
    }

    #[test]
    fn test_playback_streamer_lifecycle() {
        let streamer = PlaybackStreamer::new("/nonexistent_video_path.mp4", 160, 90, 30.0, 0);
        streamer.seek_to(1000);
        assert!(streamer.get_frame(1000).is_none());
        drop(streamer);
    }
}
