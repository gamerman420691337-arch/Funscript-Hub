//! Video decoding and frame streaming using high-performance FFmpeg pipes.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};

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
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            target_width: 256,
            target_height: 256,
            target_fps: 30.0,
            vr_mode: false,
        }
    }
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
    current_frame: u64,
}

impl FrameStreamReader {
    pub fn new<P: AsRef<Path>>(video_path: P, config: &StreamConfig) -> Result<Self> {
        let w = config.target_width;
        let h = config.target_height;
        let fps = config.target_fps;

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

        let child = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hwaccel",
                "auto",
                "-i",
            ])
            .arg(video_path.as_ref())
            .args([
                "-vf",
                &filter,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| "Failed to spawn ffmpeg child process")?;

        let frame_bytes = (w * h) as usize;

        Ok(Self {
            child,
            frame_bytes,
            width: w,
            height: h,
            fps,
            current_frame: 0,
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
