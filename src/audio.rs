//! Audio extraction and multi-band waveform analysis for timeline visualization.

use anyhow::{Context, Result};
use std::io::Read;
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
    let mut raw_bytes = Vec::new();
    stdout.read_to_end(&mut raw_bytes)?;
    let status = child.wait()?;

    if !status.success() {
        anyhow::bail!("FFmpeg failed to extract audio from media");
    }

    if raw_bytes.is_empty() {
        return Ok(AudioWaveform::default());
    }

    let num_samples = raw_bytes.len() / 2;
    let mut samples = Vec::with_capacity(num_samples);
    for chunk in raw_bytes.chunks_exact(2) {
        let val = i16::from_le_bytes([chunk[0], chunk[1]]);
        samples.push(val);
    }

    let mut peaks = Vec::with_capacity(num_samples / samples_per_bin + 1);
    let mut max_observed_rms = 0.001f32;

    for chunk in samples.chunks(samples_per_bin) {
        let mut sum_sq = 0.0f64;
        for &s in chunk {
            let normalized = s as f64 / 32768.0;
            sum_sq += normalized * normalized;
        }
        let rms = (sum_sq / chunk.len() as f64).sqrt() as f32;
        if rms > max_observed_rms {
            max_observed_rms = rms;
        }
        peaks.push(rms);
    }

    // Normalize peaks to [0.0, 1.0]
    if max_observed_rms > 0.0001 {
        for p in &mut peaks {
            *p = (*p / max_observed_rms).clamp(0.0, 1.0);
        }
    }

    Ok(AudioWaveform::new(peaks, bins_per_sec))
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
}
