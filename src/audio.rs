//! Audio extraction and multi-band waveform analysis for timeline visualization.

use anyhow::{Context, Result};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

/// Normalized audio waveform representation
#[derive(Debug, Clone, Default)]
pub struct AudioWaveform {
    /// Bins per second (default: 100 bins/sec = 10ms per bin)
    pub bins_per_sec: f32,
    /// Total duration in seconds
    #[allow(dead_code)]
    pub duration_secs: f64,
    /// Normalized peak amplitudes [0.0, 1.0] per bin
    pub peaks: Vec<f32>,
}

impl AudioWaveform {
    pub fn new(peaks: Vec<f32>, bins_per_sec: f32) -> Self {
        let duration_secs = if bins_per_sec > 0.0 {
            peaks.len() as f64 / bins_per_sec as f64
        } else {
            0.0
        };
        Self {
            bins_per_sec,
            duration_secs,
            peaks,
        }
    }

    /// Query normalized peak amplitude at a specific millisecond timestamp
    pub fn get_peak_at(&self, time_ms: i64) -> f32 {
        if self.peaks.is_empty() || time_ms < 0 {
            return 0.0;
        }
        let bin_idx = ((time_ms as f64 / 1000.0) * self.bins_per_sec as f64).round() as usize;
        if bin_idx < self.peaks.len() {
            self.peaks[bin_idx]
        } else {
            0.0
        }
    }

    /// Check if waveform has data
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }

    /// Find the nearest acoustic transient/peak within a search radius (for magnetic snapping)
    pub fn find_nearest_transient(&self, time_ms: i64, max_dist_ms: i64) -> Option<i64> {
        if self.peaks.is_empty() || time_ms < 0 || max_dist_ms <= 0 {
            return None;
        }

        let center_bin = ((time_ms as f64 / 1000.0) * self.bins_per_sec as f64).round() as isize;
        let radius_bins = ((max_dist_ms as f64 / 1000.0) * self.bins_per_sec as f64).ceil() as isize;

        let start_bin = (center_bin - radius_bins).max(1) as usize;
        let end_bin = ((center_bin + radius_bins) as usize).min(self.peaks.len().saturating_sub(2));

        let mut best_peak_val = 0.15f32; // Minimum threshold to qualify as transient
        let mut best_time_ms = None;
        let mut min_time_dist = i64::MAX;

        for b in start_bin..=end_bin {
            let val = self.peaks[b];
            let prev = self.peaks[b - 1];
            let next = self.peaks[b + 1];

            // Local maximum with significant amplitude
            if val > prev && val >= next && val >= best_peak_val {
                let bin_time = ((b as f64 / self.bins_per_sec as f64) * 1000.0).round() as i64;
                let dist = (bin_time - time_ms).abs();
                if dist <= max_dist_ms && dist < min_time_dist {
                    best_peak_val = val;
                    best_time_ms = Some(bin_time);
                    min_time_dist = dist;
                }
            }
        }

        best_time_ms
    }
}

/// Extract PCM audio stream and compute normalized RMS peak bins.
pub fn extract_audio_waveform<P: AsRef<Path>>(video_path: P) -> Result<AudioWaveform> {
    let sample_rate = 8000;
    let bins_per_sec = 100.0f32;
    let samples_per_bin = (sample_rate as f32 / bins_per_sec) as usize; // 80 samples per bin

    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-i"])
        .arg(video_path.as_ref())
        .args([
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            "8000",
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| "Failed to spawn ffmpeg for audio extraction")?;

    let stdout = child
        .stdout
        .as_mut()
        .context("Failed to open ffmpeg audio stdout")?;
    let mut reader = BufReader::new(stdout);

    // Streaming RMS computation: process chunks of `samples_per_bin * 2` bytes (160 bytes) directly from stdout,
    // avoiding large intermediate memory buffers for raw audio bytes and sample vectors.
    let chunk_size = samples_per_bin * 2;
    let mut chunk_buf = vec![0u8; chunk_size];
    let mut peaks = Vec::new();
    let mut max_observed_rms = 0.001f32;

    loop {
        let mut bytes_read = 0;
        while bytes_read < chunk_size {
            match reader.read(&mut chunk_buf[bytes_read..chunk_size]) {
                Ok(0) => break, // EOF reached
                Ok(n) => bytes_read += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }

        if bytes_read == 0 {
            break;
        }

        let num_samples = bytes_read / 2;
        if num_samples > 0 {
            let mut sum_sq = 0.0f64;
            for chunk in chunk_buf[..num_samples * 2].chunks_exact(2) {
                let val = i16::from_le_bytes([chunk[0], chunk[1]]);
                let normalized = val as f64 / 32768.0;
                sum_sq += normalized * normalized;
            }
            let rms = (sum_sq / num_samples as f64).sqrt() as f32;
            if rms > max_observed_rms {
                max_observed_rms = rms;
            }
            peaks.push(rms);
        }

        if bytes_read < chunk_size {
            break;
        }
    }

    drop(reader);
    let status = child.wait()?;

    if !status.success() {
        anyhow::bail!("FFmpeg failed to extract audio from media");
    }

    if peaks.is_empty() {
        return Ok(AudioWaveform::default());
    }

    // Normalize peaks to [0.0, 1.0]
    if max_observed_rms > 0.0001 {
        for p in &mut peaks {
            *p = (*p / max_observed_rms).clamp(0.0, 1.0);
        }
    }

    Ok(AudioWaveform::new(peaks, bins_per_sec))
}

use std::path::PathBuf;

/// Synchronized audio player backed by ffplay subprocess
#[derive(Debug)]
pub struct AudioPlayer {
    current_process: Option<std::process::Child>,
    current_path: Option<PathBuf>,
    pub volume: f32, // 0.0 to 1.0 (default 0.8)
    pub is_muted: bool,
    pub is_playing: bool,
    current_time_ms: i64,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self {
            current_process: None,
            current_path: None,
            volume: 0.8,
            is_muted: false,
            is_playing: false,
            current_time_ms: 0,
        }
    }
}

impl AudioPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if ffplay is available on the host system
    #[allow(dead_code)]
    pub fn is_backend_available() -> bool {
        Command::new("ffplay")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Start or resume audio playback at the specified timestamp in milliseconds
    pub fn play_at<P: AsRef<Path>>(&mut self, video_path: P, start_time_ms: i64, playback_speed: f64) {
        self.stop();

        let path = video_path.as_ref().to_path_buf();
        self.current_path = Some(path.clone());
        self.current_time_ms = start_time_ms.max(0);
        self.is_playing = true;

        if self.is_muted || self.volume <= 0.001 {
            return;
        }

        let start_sec = (self.current_time_ms as f64 / 1000.0).max(0.0);
        let vol_int = (self.volume * 100.0).round().clamp(0.0, 100.0) as i32;

        let mut cmd = Command::new("ffplay");
        cmd.args(["-nodisp", "-autoexit", "-vn", "-sn", "-dn"]);
        cmd.arg("-ss").arg(format!("{:.3}", start_sec));
        cmd.arg("-volume").arg(format!("{}", vol_int));

        if (playback_speed - 1.0).abs() > 0.02 {
            let speed_clamped = playback_speed.clamp(0.25, 4.0);
            cmd.arg("-af").arg(format!("atempo={:.2}", speed_clamped));
        }

        cmd.arg("-i").arg(path);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());

        match cmd.spawn() {
            Ok(child) => {
                self.current_process = Some(child);
            }
            Err(e) => {
                eprintln!("[AudioPlayer] Failed to spawn ffplay: {e}");
            }
        }
    }

    /// Pause / stop current audio playback
    pub fn pause(&mut self) {
        self.stop();
        self.is_playing = false;
    }

    /// Seek to a new timestamp. If currently playing, re-spawns at the new position.
    pub fn seek_to(&mut self, time_ms: i64, playback_speed: f64) {
        self.current_time_ms = time_ms.max(0);
        if self.is_playing {
            if let Some(path) = self.current_path.clone() {
                self.play_at(path, self.current_time_ms, playback_speed);
            }
        } else {
            self.stop();
        }
    }

    /// Set playback volume (0.0 to 1.0)
    pub fn set_volume(&mut self, vol: f32, playback_speed: f64) {
        let clamped = vol.clamp(0.0, 1.0);
        if (self.volume - clamped).abs() > 0.01 {
            self.volume = clamped;
            if self.is_playing && !self.is_muted {
                if let Some(path) = self.current_path.clone() {
                    self.play_at(path, self.current_time_ms, playback_speed);
                }
            }
        }
    }

    /// Toggle mute
    pub fn set_muted(&mut self, muted: bool, playback_speed: f64) {
        if self.is_muted != muted {
            self.is_muted = muted;
            if self.is_playing {
                if muted {
                    self.stop();
                } else if let Some(path) = self.current_path.clone() {
                    self.play_at(path, self.current_time_ms, playback_speed);
                }
            }
        }
    }

    /// Terminate background playback process
    pub fn stop(&mut self) {
        if let Some(mut child) = self.current_process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_waveform_peak_lookup() {
        let peaks = vec![0.1, 0.5, 1.0, 0.2]; // 4 bins at 100 bins/sec = 40ms total
        let waveform = AudioWaveform::new(peaks, 100.0);

        assert_eq!(waveform.get_peak_at(0), 0.1);
        assert_eq!(waveform.get_peak_at(10), 0.5);
        assert_eq!(waveform.get_peak_at(20), 1.0);
        assert_eq!(waveform.get_peak_at(30), 0.2);
        assert_eq!(waveform.get_peak_at(100), 0.0); // Out of bounds
    }

    #[test]
    fn test_synthetic_audio_extraction() {
        // Test with empty or non-existent path returns error or empty waveform
        let wf = extract_audio_waveform(Path::new("/nonexistent_audio_file.mp4"));
        assert!(wf.is_err());
    }

    #[test]
    fn test_find_nearest_transient() {
        // Create 20 bins (200ms total, 10ms per bin at 100 bins/sec)
        let mut peaks = vec![0.0f32; 20];
        // Introduce a transient peak at bin 10 (100ms) with value 0.8
        peaks[9] = 0.2;
        peaks[10] = 0.8;
        peaks[11] = 0.3;

        let wf = AudioWaveform::new(peaks, 100.0);

        // Within 35ms: query at 90ms -> should snap to 100ms
        let snapped = wf.find_nearest_transient(90, 35);
        assert_eq!(snapped, Some(100));

        // Query at 150ms with 35ms window -> too far, returns None
        assert_eq!(wf.find_nearest_transient(150, 35), None);

        // Query at 120ms with 35ms window -> within 20ms, snaps to 100ms
        assert_eq!(wf.find_nearest_transient(120, 35), Some(100));
    }

    #[test]
    fn test_audio_player_state_transitions() {
        let mut player = AudioPlayer::new();
        assert!((player.volume - 0.8).abs() < 0.01);
        assert!(!player.is_muted);
        assert!(!player.is_playing);

        player.set_volume(0.4, 1.0);
        assert!((player.volume - 0.4).abs() < 0.01);

        player.set_muted(true, 1.0);
        assert!(player.is_muted);

        player.set_muted(false, 1.0);
        assert!(!player.is_muted);

        player.pause();
        assert!(!player.is_playing);
        assert!(AudioPlayer::is_backend_available());
    }
}
