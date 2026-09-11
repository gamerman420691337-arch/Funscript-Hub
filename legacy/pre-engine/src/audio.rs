//! Audio extraction and multi-band waveform analysis for timeline visualization.

use anyhow::{Context, Result};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

pub mod spectral;
#[allow(unused_imports)]
pub use spectral::{
    generate_multiband_companion_bundle, synthesize_spectral_actions, BiquadFilter, CooleyTukeyFft,
    FrequencyBand, SpectralAnalysis, SpectralInfillConfig, SpectralInfillMode, FFT_SIZE,
    NUM_SPEC_BANDS,
};

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
    /// Multi-band spectral decomposition (Sub-Bass, Mid, High, Spectrogram)
    pub spectral: Option<SpectralAnalysis>,
}

impl AudioWaveform {
    #[allow(dead_code)]
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
            spectral: None,
        }
    }

    pub fn new_with_spectral(
        peaks: Vec<f32>,
        spectral: SpectralAnalysis,
        bins_per_sec: f32,
    ) -> Self {
        let duration_secs = if bins_per_sec > 0.0 {
            peaks.len() as f64 / bins_per_sec as f64
        } else {
            0.0
        };
        Self {
            bins_per_sec,
            duration_secs,
            peaks,
            spectral: Some(spectral),
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

    /// Query normalized Sub-Bass (20-250 Hz) energy at a specific millisecond timestamp
    pub fn get_sub_bass_at(&self, time_ms: i64) -> f32 {
        if let Some(ref s) = self.spectral {
            s.get_band_energy_at(FrequencyBand::SubBass, time_ms, self.bins_per_sec)
        } else {
            self.get_peak_at(time_ms)
        }
    }

    /// Query normalized Mid-Range (250-2500 Hz) energy at a specific millisecond timestamp
    pub fn get_mid_at(&self, time_ms: i64) -> f32 {
        if let Some(ref s) = self.spectral {
            s.get_band_energy_at(FrequencyBand::Mid, time_ms, self.bins_per_sec)
        } else {
            self.get_peak_at(time_ms)
        }
    }

    /// Query normalized Highs / Transients (2.5-8 kHz) energy at a specific millisecond timestamp
    pub fn get_high_at(&self, time_ms: i64) -> f32 {
        if let Some(ref s) = self.spectral {
            s.get_band_energy_at(FrequencyBand::High, time_ms, self.bins_per_sec)
        } else {
            self.get_peak_at(time_ms)
        }
    }

    /// Query 16-band spectrogram slice at a specific millisecond timestamp
    pub fn get_spectrogram_at(&self, time_ms: i64) -> Option<&[f32; NUM_SPEC_BANDS]> {
        self.spectral
            .as_ref()
            .and_then(|s| s.get_spectrogram_at(time_ms, self.bins_per_sec))
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

/// Extract PCM audio stream and compute normalized RMS and multi-band spectral decomposition.
pub fn extract_audio_waveform<P: AsRef<Path>>(video_path: P) -> Result<AudioWaveform> {
    let sample_rate = 16000;
    let bins_per_sec = 100.0f32;
    let samples_per_bin = (sample_rate as f32 / bins_per_sec) as usize; // 160 samples per bin

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
            "16000",
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

    // Multi-band DSP filters:
    // Sub-Bass: 2nd-order Butterworth lowpass at 220 Hz
    let mut lp_filter = BiquadFilter::lowpass(sample_rate as f32, 220.0, 0.707);
    // Mid: 2nd-order Butterworth bandpass at 1200 Hz
    let mut bp_filter = BiquadFilter::bandpass(sample_rate as f32, 1200.0, 1.0);
    // High: 2nd-order Butterworth highpass at 2500 Hz
    let mut hp_filter = BiquadFilter::highpass(sample_rate as f32, 2500.0, 0.707);

    // 256-point Cooley-Tukey STFT for 16-band waterfall spectrogram
    let fft = CooleyTukeyFft::new();
    let mut fft_window = [0.0f32; FFT_SIZE];

    let chunk_size = samples_per_bin * 2; // 320 bytes = 160 samples
    let mut chunk_buf = vec![0u8; chunk_size];

    let mut peaks = Vec::new();
    let mut sub_bass = Vec::new();
    let mut mid = Vec::new();
    let mut high = Vec::new();
    let mut spectrogram = Vec::new();

    let mut max_observed_rms = 0.001f32;
    let mut max_observed_sub = 0.001f32;
    let mut max_observed_mid = 0.001f32;
    let mut max_observed_high = 0.001f32;
    let mut max_observed_spec = 0.001f32;

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
            let mut sum_sq_raw = 0.0f64;
            let mut sum_sq_sub = 0.0f64;
            let mut sum_sq_mid = 0.0f64;
            let mut sum_sq_high = 0.0f64;

            // Shift FFT sliding window by 160 samples
            fft_window.copy_within(num_samples..FFT_SIZE, 0);

            for (idx, chunk) in chunk_buf[..num_samples * 2].chunks_exact(2).enumerate() {
                let val = i16::from_le_bytes([chunk[0], chunk[1]]);
                let x = val as f32 / 32768.0;

                sum_sq_raw += (x * x) as f64;

                let y_sub = lp_filter.process_sample(x);
                sum_sq_sub += (y_sub * y_sub) as f64;

                let y_mid = bp_filter.process_sample(x);
                sum_sq_mid += (y_mid * y_mid) as f64;

                let y_high = hp_filter.process_sample(x);
                sum_sq_high += (y_high * y_high) as f64;

                let fft_pos = (FFT_SIZE - num_samples) + idx;
                if fft_pos < FFT_SIZE {
                    fft_window[fft_pos] = x;
                }
            }

            let rms_raw = (sum_sq_raw / num_samples as f64).sqrt() as f32;
            let rms_sub = (sum_sq_sub / num_samples as f64).sqrt() as f32;
            let rms_mid = (sum_sq_mid / num_samples as f64).sqrt() as f32;
            let rms_high = (sum_sq_high / num_samples as f64).sqrt() as f32;

            if rms_raw > max_observed_rms {
                max_observed_rms = rms_raw;
            }
            if rms_sub > max_observed_sub {
                max_observed_sub = rms_sub;
            }
            if rms_mid > max_observed_mid {
                max_observed_mid = rms_mid;
            }
            if rms_high > max_observed_high {
                max_observed_high = rms_high;
            }

            peaks.push(rms_raw);
            sub_bass.push(rms_sub);
            mid.push(rms_mid);
            high.push(rms_high);

            // Compute 16-band STFT frame
            let bands = fft.compute_16_bands(&fft_window);
            for &val in &bands {
                if val > max_observed_spec {
                    max_observed_spec = val;
                }
            }
            spectrogram.push(bands);
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

    // Normalize peaks & spectral bands to [0.0, 1.0]
    if max_observed_rms > 0.0001 {
        for p in &mut peaks {
            *p = (*p / max_observed_rms).clamp(0.0, 1.0);
        }
    }
    if max_observed_sub > 0.0001 {
        for s in &mut sub_bass {
            *s = (*s / max_observed_sub).clamp(0.0, 1.0);
        }
    }
    if max_observed_mid > 0.0001 {
        for m in &mut mid {
            *m = (*m / max_observed_mid).clamp(0.0, 1.0);
        }
    }
    if max_observed_high > 0.0001 {
        for h in &mut high {
            *h = (*h / max_observed_high).clamp(0.0, 1.0);
        }
    }
    if max_observed_spec > 0.0001 {
        for spec_frame in &mut spectrogram {
            for b in spec_frame.iter_mut() {
                *b = (*b / max_observed_spec).clamp(0.0, 1.0);
            }
        }
    }

    let spectral = SpectralAnalysis {
        sub_bass,
        mid,
        high,
        spectrogram,
    };

    Ok(AudioWaveform::new_with_spectral(
        peaks,
        spectral,
        bins_per_sec,
    ))
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

/// Helper to construct chained ffmpeg atempo audio filters within supported [0.5, 2.0] bounds
pub fn format_atempo_filter(speed: f64) -> String {
    let mut s = speed.clamp(0.25, 4.0);
    let mut filters = Vec::new();
    while s < 0.5 {
        filters.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    while s > 2.0 {
        filters.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    filters.push(format!("atempo={:.3}", s));
    filters.join(",")
}

impl AudioPlayer {
    #[allow(dead_code)]
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

    /// Set target media file path for playback
    pub fn set_media_path<P: AsRef<Path>>(&mut self, path: P) {
        self.current_path = Some(path.as_ref().to_path_buf());
    }

    /// Update internal tracking of current timestamp
    pub fn update_current_time(&mut self, time_ms: i64) {
        self.current_time_ms = time_ms.max(0);
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
        // Note: ffplay supports -nodisp, -autoexit, -vn, -sn.
        // It does NOT support -dn (which is ffmpeg-only and causes ffplay to abort).
        cmd.args(["-nodisp", "-autoexit", "-vn", "-sn"]);
        cmd.arg("-ss").arg(format!("{:.3}", start_sec));
        cmd.arg("-volume").arg(format!("{}", vol_int));

        if (playback_speed - 1.0).abs() > 0.02 {
            let filter = format_atempo_filter(playback_speed);
            cmd.arg("-af").arg(filter);
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

    /// Set playback volume at a specific timestamp (0.0 to 1.0)
    pub fn set_volume_at(&mut self, vol: f32, current_time_ms: i64, playback_speed: f64) {
        self.current_time_ms = current_time_ms.max(0);
        self.set_volume(vol, playback_speed);
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

    /// Toggle mute at a specific timestamp
    pub fn set_muted_at(&mut self, muted: bool, current_time_ms: i64, playback_speed: f64) {
        self.current_time_ms = current_time_ms.max(0);
        self.set_muted(muted, playback_speed);
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

        // Test time tracking and volume_at
        player.update_current_time(5000);
        player.set_volume_at(0.6, 7500, 1.0);
        assert!((player.volume - 0.6).abs() < 0.01);

        player.set_muted_at(true, 8000, 1.0);
        assert!(player.is_muted);
    }

    #[test]
    fn test_format_atempo_filter_chaining() {
        assert_eq!(format_atempo_filter(0.5), "atempo=0.500");
        assert_eq!(format_atempo_filter(1.5), "atempo=1.500");
        assert_eq!(format_atempo_filter(2.0), "atempo=2.000");
        // 0.25x requires chaining two 0.5 filters
        assert_eq!(format_atempo_filter(0.25), "atempo=0.5,atempo=0.500");
        // 3.0x requires chaining 2.0 and 1.5
        assert_eq!(format_atempo_filter(3.0), "atempo=2.0,atempo=1.500");
    }
}
