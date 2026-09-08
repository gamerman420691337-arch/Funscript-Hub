//! Spectral audio analysis, Biquad filter banks, STFT, and haptic channel infill.

use crate::funscript::{Action, AxisChannel, Funscript, MultiAxisScript};
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

// ==============================================================================
// 1. Pure-Rust 2nd-Order Biquad Filter (Direct Form II Transposed)
// ==============================================================================

/// 2nd-order Biquad IIR filter using Direct Form II Transposed topology.
/// Guaranteed numerically stable, zero heap allocation, sub-microsecond execution.
#[derive(Debug, Clone)]
pub struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl BiquadFilter {
    /// 2nd-order Butterworth lowpass filter
    pub fn lowpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let omega = 2.0 * PI * (cutoff_hz / sample_rate).clamp(0.0001, 0.499);
        let alpha = omega.sin() / (2.0 * q.max(0.1));
        let cos_w = omega.cos();

        let b0 = (1.0 - cos_w) * 0.5;
        let b1 = 1.0 - cos_w;
        let b2 = (1.0 - cos_w) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// 2nd-order Butterworth bandpass filter
    pub fn bandpass(sample_rate: f32, center_hz: f32, q: f32) -> Self {
        let omega = 2.0 * PI * (center_hz / sample_rate).clamp(0.0001, 0.499);
        let alpha = omega.sin() / (2.0 * q.max(0.1));
        let cos_w = omega.cos();

        let b0 = alpha;
        let b1 = 0.0;
        let b2 = -alpha;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// 2nd-order Butterworth highpass filter
    pub fn highpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let omega = 2.0 * PI * (cutoff_hz / sample_rate).clamp(0.0001, 0.499);
        let alpha = omega.sin() / (2.0 * q.max(0.1));
        let cos_w = omega.cos();

        let b0 = (1.0 + cos_w) * 0.5;
        let b1 = -(1.0 + cos_w);
        let b2 = (1.0 + cos_w) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Process a single audio sample in-place
    #[inline(always)]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Reset filter state
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

// ==============================================================================
// 2. Pure-Rust 256-Point Radix-2 Cooley-Tukey FFT & STFT
// ==============================================================================

pub const FFT_SIZE: usize = 256;
pub const NUM_SPEC_BANDS: usize = 16;

/// Precomputed tables for 256-point Radix-2 Cooley-Tukey FFT
#[derive(Debug, Clone)]
pub struct CooleyTukeyFft {
    bit_rev: [usize; FFT_SIZE],
    twiddle_cos: [f32; FFT_SIZE / 2],
    twiddle_sin: [f32; FFT_SIZE / 2],
    hann_window: [f32; FFT_SIZE],
}

impl Default for CooleyTukeyFft {
    fn default() -> Self {
        Self::new()
    }
}

impl CooleyTukeyFft {
    pub fn new() -> Self {
        let mut bit_rev = [0usize; FFT_SIZE];
        for i in 0..FFT_SIZE {
            let mut rev = 0;
            let mut val = i;
            for _ in 0..8 {
                // log2(256) = 8
                rev = (rev << 1) | (val & 1);
                val >>= 1;
            }
            bit_rev[i] = rev;
        }

        let mut twiddle_cos = [0.0f32; FFT_SIZE / 2];
        let mut twiddle_sin = [0.0f32; FFT_SIZE / 2];
        for k in 0..(FFT_SIZE / 2) {
            let angle = -2.0 * PI * (k as f32) / (FFT_SIZE as f32);
            twiddle_cos[k] = angle.cos();
            twiddle_sin[k] = angle.sin();
        }

        let mut hann_window = [0.0f32; FFT_SIZE];
        for n in 0..FFT_SIZE {
            hann_window[n] = 0.5 * (1.0 - (2.0 * PI * n as f32 / (FFT_SIZE as f32 - 1.0)).cos());
        }

        Self {
            bit_rev,
            twiddle_cos,
            twiddle_sin,
            hann_window,
        }
    }

    /// Compute 256-point FFT on windowed real samples and output 16 perceptual frequency band magnitudes
    pub fn compute_16_bands(&self, samples: &[f32; FFT_SIZE]) -> [f32; NUM_SPEC_BANDS] {
        let mut real = [0.0f32; FFT_SIZE];
        let mut imag = [0.0f32; FFT_SIZE];

        // 1. Windowing and bit-reversal reordering
        for i in 0..FFT_SIZE {
            let rev_idx = self.bit_rev[i];
            real[i] = samples[rev_idx] * self.hann_window[rev_idx];
            imag[i] = 0.0;
        }

        // 2. In-place Cooley-Tukey butterflies
        let mut step = 2;
        while step <= FFT_SIZE {
            let half = step / 2;
            let twiddle_step = (FFT_SIZE / 2) / half;

            let mut group = 0;
            while group < FFT_SIZE {
                for k in 0..half {
                    let tw_idx = k * twiddle_step;
                    let c = self.twiddle_cos[tw_idx];
                    let s = self.twiddle_sin[tw_idx];

                    let u_r = real[group + k];
                    let u_i = imag[group + k];

                    let v_r = real[group + k + half];
                    let v_i = imag[group + k + half];

                    let t_r = v_r * c - v_i * s;
                    let t_i = v_r * s + v_i * c;

                    real[group + k] = u_r + t_r;
                    imag[group + k] = u_i + t_i;

                    real[group + k + half] = u_r - t_r;
                    imag[group + k + half] = u_i - t_i;
                }
                group += step;
            }
            step *= 2;
        }

        // 3. Compute magnitudes for the first 128 positive frequency bins (0 Hz to 8000 Hz at 16kHz)
        let mut magnitudes = [0.0f32; 128];
        for k in 0..128 {
            magnitudes[k] = (real[k] * real[k] + imag[k] * imag[k]).sqrt();
        }

        // 4. Perceptual logarithmic/mel band aggregation into 16 bands
        // Bin ranges mapped across 128 bins (62.5 Hz per bin at 16 kHz):
        let band_bounds: [(usize, usize); NUM_SPEC_BANDS] = [
            (1, 2),    // 0: 62.5 - 125 Hz (Sub-bass rumble)
            (2, 3),    // 1: 125 - 187.5 Hz (Kick fundamental)
            (3, 5),    // 2: 187.5 - 312.5 Hz (Bass warmth)
            (5, 7),    // 3: 312.5 - 437.5 Hz (Low mid punch)
            (7, 10),   // 4: 437.5 - 625 Hz (Body)
            (10, 14),  // 5: 625 - 875 Hz (Mid presence)
            (14, 19),  // 6: 875 - 1187.5 Hz (Mid vocal)
            (19, 25),  // 7: 1.18 - 1.56 kHz (Vocal clarity)
            (25, 32),  // 8: 1.56 - 2.0 kHz (Attack/slap)
            (32, 40),  // 9: 2.0 - 2.5 kHz (Upper mid)
            (40, 50),  // 10: 2.5 - 3.12 kHz (Presence)
            (50, 62),  // 11: 3.12 - 3.87 kHz (Crispness)
            (62, 76),  // 12: 3.87 - 4.75 kHz (High presence)
            (76, 92),  // 13: 4.75 - 5.75 kHz (Sizzle)
            (92, 110), // 14: 5.75 - 6.87 kHz (Air)
            (110, 128), // 15: 6.87 - 8.0 kHz (Brilliance)
        ];

        let mut bands = [0.0f32; NUM_SPEC_BANDS];
        for (i, &(start, end)) in band_bounds.iter().enumerate() {
            let mut sum = 0.0f32;
            let count = (end - start).max(1) as f32;
            for bin in start..end.min(128) {
                sum += magnitudes[bin];
            }
            bands[i] = sum / count;
        }

        bands
    }
}

// ==============================================================================
// 3. Spectral Frequency Bands & Multi-Band Analysis Data Structure
// ==============================================================================

/// User-selectable frequency band for haptic coupling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FrequencyBand {
    #[default]
    SubBass,      // 20 - 250 Hz (kicks, thumps, heavy impacts)
    Mid,          // 250 - 2500 Hz (vocal body, slaps, rhythm)
    High,         // 2500 - 8000 Hz (crisp transients, high accents)
    FullSpectrum, // Full RMS envelope
}

impl FrequencyBand {
    pub const ALL: [FrequencyBand; 4] = [
        FrequencyBand::SubBass,
        FrequencyBand::Mid,
        FrequencyBand::High,
        FrequencyBand::FullSpectrum,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            FrequencyBand::SubBass => "Sub-Bass (20 - 250 Hz)",
            FrequencyBand::Mid => "Mid-Range (250 - 2500 Hz)",
            FrequencyBand::High => "Highs / Transients (2.5 - 8 kHz)",
            FrequencyBand::FullSpectrum => "Full Audio RMS",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            FrequencyBand::SubBass => "Bass",
            FrequencyBand::Mid => "Mid",
            FrequencyBand::High => "High",
            FrequencyBand::FullSpectrum => "Full",
        }
    }
}

/// Multi-band spectral decomposition data container
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpectralAnalysis {
    /// Normalized sub-bass energy [0.0, 1.0] per time bin
    pub sub_bass: Vec<f32>,
    /// Normalized mid-frequency energy [0.0, 1.0] per time bin
    pub mid: Vec<f32>,
    /// Normalized high-frequency energy [0.0, 1.0] per time bin
    pub high: Vec<f32>,
    /// 16-band normalized spectrogram magnitude [0.0, 1.0] per time bin
    pub spectrogram: Vec<[f32; NUM_SPEC_BANDS]>,
}

impl SpectralAnalysis {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.sub_bass.is_empty()
    }

    /// Query normalized energy for a specific band at millisecond timestamp
    pub fn get_band_energy_at(&self, band: FrequencyBand, time_ms: i64, bins_per_sec: f32) -> f32 {
        if time_ms < 0 || bins_per_sec <= 0.0 {
            return 0.0;
        }
        let bin_idx = ((time_ms as f64 / 1000.0) * bins_per_sec as f64).round() as usize;

        let vec = match band {
            FrequencyBand::SubBass => &self.sub_bass,
            FrequencyBand::Mid => &self.mid,
            FrequencyBand::High => &self.high,
            FrequencyBand::FullSpectrum => return 0.0,
        };

        if bin_idx < vec.len() {
            vec[bin_idx]
        } else {
            0.0
        }
    }

    /// Query 16-band spectrogram values at millisecond timestamp
    pub fn get_spectrogram_at(&self, time_ms: i64, bins_per_sec: f32) -> Option<&[f32; NUM_SPEC_BANDS]> {
        if time_ms < 0 || bins_per_sec <= 0.0 || self.spectrogram.is_empty() {
            return None;
        }
        let bin_idx = ((time_ms as f64 / 1000.0) * bins_per_sec as f64).round() as usize;
        self.spectrogram.get(bin_idx)
    }

    /// Find nearest transient in a specific spectral frequency band
    #[allow(dead_code)]
    pub fn find_nearest_band_transient(
        &self,
        band: FrequencyBand,
        time_ms: i64,
        max_dist_ms: i64,
        bins_per_sec: f32,
    ) -> Option<i64> {
        if time_ms < 0 || max_dist_ms <= 0 || bins_per_sec <= 0.0 {
            return None;
        }

        let vec = match band {
            FrequencyBand::SubBass => &self.sub_bass,
            FrequencyBand::Mid => &self.mid,
            FrequencyBand::High => &self.high,
            FrequencyBand::FullSpectrum => return None,
        };

        if vec.is_empty() {
            return None;
        }

        let center_bin = ((time_ms as f64 / 1000.0) * bins_per_sec as f64).round() as isize;
        let radius_bins = ((max_dist_ms as f64 / 1000.0) * bins_per_sec as f64).ceil() as isize;

        let start_bin = (center_bin - radius_bins).max(1) as usize;
        let end_bin = ((center_bin + radius_bins) as usize).min(vec.len().saturating_sub(2));

        let mut best_peak = 0.15f32;
        let mut best_time = None;
        let mut min_dist = i64::MAX;

        for b in start_bin..=end_bin {
            let val = vec[b];
            let prev = vec[b - 1];
            let next = vec[b + 1];

            if val > prev && val >= next && val >= best_peak {
                let bin_time = ((b as f64 / bins_per_sec as f64) * 1000.0).round() as i64;
                let dist = (bin_time - time_ms).abs();
                if dist <= max_dist_ms && dist < min_dist {
                    best_peak = val;
                    best_time = Some(bin_time);
                    min_dist = dist;
                }
            }
        }

        best_time
    }
}

// ==============================================================================
// 4. Multi-Band Haptic Coupling & Spectral Infill Engine
// ==============================================================================

/// Coupling strategy for converting spectral energy into physical actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SpectralInfillMode {
    #[default]
    RhythmicBeats,        // Transient beat detection with turnaround points (peaks/valleys)
    EnvelopeFollower,     // Attack/decay smoothed continuous volume envelope
    ModulatedOscillation, // Continuous oscillatory wave (e.g. 8 Hz) amplitude-modulated by energy
}

impl SpectralInfillMode {
    pub const ALL: [SpectralInfillMode; 3] = [
        SpectralInfillMode::RhythmicBeats,
        SpectralInfillMode::EnvelopeFollower,
        SpectralInfillMode::ModulatedOscillation,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            SpectralInfillMode::RhythmicBeats => "Rhythmic Beats (Turnaround Pulses)",
            SpectralInfillMode::EnvelopeFollower => "Envelope Follower (Continuous Dynamics)",
            SpectralInfillMode::ModulatedOscillation => "Modulated Oscillation (Audio-Reactive Vibe)",
        }
    }
}

/// Configuration parameters for spectral haptic infill
#[derive(Debug, Clone, PartialEq)]
pub struct SpectralInfillConfig {
    pub band: FrequencyBand,
    pub target_axis: AxisChannel,
    pub mode: SpectralInfillMode,
    /// Minimum normalized threshold to trigger motion [0.05, 0.9]
    pub threshold: f32,
    /// Output position floor [0, 100]
    pub min_pos: i32,
    /// Output position ceiling [0, 100]
    pub max_pos: i32,
    /// Minimum interval between rhythmic beat pulses in ms (prevents motor stall)
    pub min_interval_ms: i64,
    /// Modulation frequency in Hz for ModulatedOscillation mode (e.g. 5.0 - 15.0 Hz)
    pub modulation_freq_hz: f32,
}

impl Default for SpectralInfillConfig {
    fn default() -> Self {
        Self {
            band: FrequencyBand::SubBass,
            target_axis: AxisChannel::Stroke,
            mode: SpectralInfillMode::RhythmicBeats,
            threshold: 0.35,
            min_pos: 10,
            max_pos: 90,
            min_interval_ms: 180,
            modulation_freq_hz: 8.0,
        }
    }
}

/// Synthesize funscript actions from spectral audio data
pub fn synthesize_spectral_actions(
    spectral: &SpectralAnalysis,
    peaks: &[f32],
    bins_per_sec: f32,
    config: &SpectralInfillConfig,
    start_time_ms: i64,
    end_time_ms: i64,
) -> Vec<Action> {
    if start_time_ms >= end_time_ms || bins_per_sec <= 0.0 {
        return Vec::new();
    }

    // Select the energy signal source
    let energy_signal: &[f32] = match config.band {
        FrequencyBand::SubBass => {
            if !spectral.sub_bass.is_empty() {
                &spectral.sub_bass
            } else {
                peaks
            }
        }
        FrequencyBand::Mid => {
            if !spectral.mid.is_empty() {
                &spectral.mid
            } else {
                peaks
            }
        }
        FrequencyBand::High => {
            if !spectral.high.is_empty() {
                &spectral.high
            } else {
                peaks
            }
        }
        FrequencyBand::FullSpectrum => peaks,
    };

    if energy_signal.is_empty() {
        return Vec::new();
    }

    let start_bin = ((start_time_ms as f64 / 1000.0) * bins_per_sec as f64).round().max(0.0) as usize;
    let end_bin = ((end_time_ms as f64 / 1000.0) * bins_per_sec as f64).round().min(energy_signal.len() as f64) as usize;

    if start_bin >= end_bin {
        return Vec::new();
    }

    let mut actions = Vec::new();
    let min_p = config.min_pos.clamp(0, 100);
    let max_p = config.max_pos.clamp(0, 100);
    let mid_p = (min_p + max_p) / 2;

    match config.mode {
        SpectralInfillMode::RhythmicBeats => {
            let min_interval_bins = ((config.min_interval_ms as f64 / 1000.0) * bins_per_sec as f64).round().max(1.0) as usize;
            let mut last_beat_bin = 0usize;
            let mut is_high = false;

            for b in (start_bin + 1)..(end_bin.saturating_sub(1)) {
                let val = energy_signal[b];
                let prev = energy_signal[b - 1];
                let next = energy_signal[b + 1];

                // Local peak above threshold with refractory spacing
                if val > prev && val >= next && val >= config.threshold {
                    if last_beat_bin == 0 || (b - last_beat_bin) >= min_interval_bins {
                        let t_ms = ((b as f64 / bins_per_sec as f64) * 1000.0).round() as i64;

                        // Insert return point before new beat if significant gap
                        if last_beat_bin > 0 && (b - last_beat_bin) > min_interval_bins * 2 {
                            let mid_bin = (last_beat_bin + b) / 2;
                            let mid_t = ((mid_bin as f64 / bins_per_sec as f64) * 1000.0).round() as i64;
                            let return_pos = if is_high { min_p } else { max_p };
                            actions.push(Action { at: mid_t, pos: return_pos });
                        }

                        let target_pos = if is_high {
                            is_high = false;
                            min_p
                        } else {
                            is_high = true;
                            // Scale stroke depth dynamically with beat velocity
                            let depth = ((val - config.threshold) / (1.0 - config.threshold).max(0.01)).clamp(0.0, 1.0);
                            mid_p + ((max_p - mid_p) as f32 * depth).round() as i32
                        };

                        actions.push(Action { at: t_ms, pos: target_pos.clamp(0, 100) });
                        last_beat_bin = b;
                    }
                }
            }
        }

        SpectralInfillMode::EnvelopeFollower => {
            // Sample envelope at 50ms intervals with exponential attack/decay smoothing
            let step_bins = (0.050 * bins_per_sec).round().max(1.0) as usize;
            let mut smoothed_val = 0.0f32;
            let alpha = 0.35f32; // Smoothing factor

            let mut b = start_bin;
            while b < end_bin {
                let raw_val = energy_signal[b];
                smoothed_val = alpha * raw_val + (1.0 - alpha) * smoothed_val;

                let t_ms = ((b as f64 / bins_per_sec as f64) * 1000.0).round() as i64;
                let active_val = if smoothed_val >= config.threshold {
                    (smoothed_val - config.threshold) / (1.0 - config.threshold).max(0.01)
                } else {
                    0.0
                };

                let pos = (min_p as f32 + active_val * (max_p - min_p) as f32).round() as i32;
                actions.push(Action { at: t_ms, pos: pos.clamp(0, 100) });

                b += step_bins;
            }
        }

        SpectralInfillMode::ModulatedOscillation => {
            let freq = config.modulation_freq_hz.clamp(1.0, 25.0);
            let period_sec = 1.0 / freq;
            let step_ms = (period_sec * 1000.0 * 0.25).max(10.0); // 4 samples per cycle
            let step_bins = ((step_ms as f64 / 1000.0) * bins_per_sec as f64).round().max(1.0) as usize;

            let mut b = start_bin;
            while b < end_bin {
                let t_sec = b as f32 / bins_per_sec;
                let t_ms = (t_sec * 1000.0).round() as i64;

                let energy = energy_signal[b];
                let amp = if energy >= config.threshold {
                    ((energy - config.threshold) / (1.0 - config.threshold).max(0.01)).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                let phase = 2.0 * PI * freq * t_sec;
                let carrier = phase.sin(); // [-1.0, 1.0]
                let half_span = (max_p - min_p) as f32 * 0.5;
                let pos = (mid_p as f32 + carrier * half_span * amp).round() as i32;

                actions.push(Action { at: t_ms, pos: pos.clamp(0, 100) });
                b += step_bins;
            }
        }
    }

    let mut script = Funscript::new(actions);
    script.sanitize();
    script.actions
}

/// Automatically synthesize a complete multi-band companion bundle from spectral audio
pub fn generate_multiband_companion_bundle(
    spectral: &SpectralAnalysis,
    peaks: &[f32],
    bins_per_sec: f32,
    duration_ms: i64,
) -> MultiAxisScript {
    let mut bundle = MultiAxisScript::new();

    // 1. Primary Stroke (L0): Rhythmic beats driven by Sub-Bass transients
    let stroke_cfg = SpectralInfillConfig {
        band: FrequencyBand::SubBass,
        target_axis: AxisChannel::Stroke,
        mode: SpectralInfillMode::RhythmicBeats,
        threshold: 0.30,
        min_pos: 10,
        max_pos: 95,
        min_interval_ms: 220,
        modulation_freq_hz: 5.0,
    };
    let stroke_actions = synthesize_spectral_actions(spectral, peaks, bins_per_sec, &stroke_cfg, 0, duration_ms);
    let mut stroke_script = Funscript::new(stroke_actions);
    stroke_script.sanitize();
    bundle.channels.insert(AxisChannel::Stroke, stroke_script);

    // 2. Surge (L1): Low-frequency swell follower from Sub-Bass envelope
    let surge_cfg = SpectralInfillConfig {
        band: FrequencyBand::SubBass,
        target_axis: AxisChannel::Surge,
        mode: SpectralInfillMode::EnvelopeFollower,
        threshold: 0.20,
        min_pos: 30,
        max_pos: 75,
        min_interval_ms: 200,
        modulation_freq_hz: 5.0,
    };
    let surge_actions = synthesize_spectral_actions(spectral, peaks, bins_per_sec, &surge_cfg, 0, duration_ms);
    let mut surge_script = Funscript::new(surge_actions);
    surge_script.sanitize();
    bundle.channels.insert(AxisChannel::Surge, surge_script);

    // 3. Twist (R2): Modulated 8Hz oscillation driven by Mid-band vocal/slap textures
    let twist_cfg = SpectralInfillConfig {
        band: FrequencyBand::Mid,
        target_axis: AxisChannel::Twist,
        mode: SpectralInfillMode::ModulatedOscillation,
        threshold: 0.25,
        min_pos: 20,
        max_pos: 80,
        min_interval_ms: 150,
        modulation_freq_hz: 8.0,
    };
    let twist_actions = synthesize_spectral_actions(spectral, peaks, bins_per_sec, &twist_cfg, 0, duration_ms);
    let mut twist_script = Funscript::new(twist_actions);
    twist_script.sanitize();
    bundle.channels.insert(AxisChannel::Twist, twist_script);

    // 4. Suction / Vibration (V0): Envelope follower from Highs & transients
    let suction_cfg = SpectralInfillConfig {
        band: FrequencyBand::High,
        target_axis: AxisChannel::Suction,
        mode: SpectralInfillMode::EnvelopeFollower,
        threshold: 0.20,
        min_pos: 0,
        max_pos: 85,
        min_interval_ms: 100,
        modulation_freq_hz: 10.0,
    };
    let suction_actions = synthesize_spectral_actions(spectral, peaks, bins_per_sec, &suction_cfg, 0, duration_ms);
    let mut suction_script = Funscript::new(suction_actions);
    suction_script.sanitize();
    bundle.channels.insert(AxisChannel::Suction, suction_script);

    bundle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_biquad_lowpass_attenuates_high_frequencies() {
        let sample_rate = 16000.0;
        let mut lp = BiquadFilter::lowpass(sample_rate, 200.0, 0.707);

        // Low frequency tone: 50 Hz
        let mut low_amp_sum = 0.0f32;
        for i in 0..1000 {
            let t = i as f32 / sample_rate;
            let x = (2.0 * PI * 50.0 * t).sin();
            let y = lp.process_sample(x);
            if i > 200 {
                low_amp_sum += y.abs();
            }
        }
        let low_avg_amp = low_amp_sum / 800.0;

        lp.reset();
        // High frequency tone: 3000 Hz
        let mut high_amp_sum = 0.0f32;
        for i in 0..1000 {
            let t = i as f32 / sample_rate;
            let x = (2.0 * PI * 3000.0 * t).sin();
            let y = lp.process_sample(x);
            if i > 200 {
                high_amp_sum += y.abs();
            }
        }
        let high_avg_amp = high_amp_sum / 800.0;

        assert!(low_avg_amp > 0.4, "Low frequency 50Hz should pass: {low_avg_amp}");
        assert!(high_avg_amp < 0.05, "High frequency 3000Hz should be attenuated: {high_avg_amp}");
        assert!(low_avg_amp > high_avg_amp * 8.0);
    }

    #[test]
    fn test_cooley_tukey_fft_sine_peak() {
        let fft = CooleyTukeyFft::new();
        let sample_rate = 16000.0;
        let mut samples = [0.0f32; FFT_SIZE];

        // 125 Hz sine wave -> should peak in band 1
        for i in 0..FFT_SIZE {
            let t = i as f32 / sample_rate;
            samples[i] = (2.0 * PI * 125.0 * t).sin();
        }

        let bands = fft.compute_16_bands(&samples);
        let max_band = bands.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert!(max_band <= 2, "125Hz sine should concentrate in low bands (got band {max_band})");
    }

    #[test]
    fn test_synthesize_spectral_rhythmic_beats() {
        let mut sub_bass = vec![0.0f32; 100]; // 1 second at 100 bins/sec
        sub_bass[20] = 0.8; // Transient at 200ms
        sub_bass[60] = 0.9; // Transient at 600ms

        let spectral = SpectralAnalysis {
            sub_bass,
            mid: vec![0.0; 100],
            high: vec![0.0; 100],
            spectrogram: vec![[0.0; 16]; 100],
        };

        let cfg = SpectralInfillConfig {
            band: FrequencyBand::SubBass,
            target_axis: AxisChannel::Stroke,
            mode: SpectralInfillMode::RhythmicBeats,
            threshold: 0.4,
            min_pos: 10,
            max_pos: 90,
            min_interval_ms: 150,
            modulation_freq_hz: 5.0,
        };

        let actions = synthesize_spectral_actions(&spectral, &[], 100.0, &cfg, 0, 1000);
        assert!(!actions.is_empty());
        // Should detect both peaks
        assert!(actions.iter().any(|a| (a.at - 200).abs() <= 20));
        assert!(actions.iter().any(|a| (a.at - 600).abs() <= 20));
    }

    #[test]
    fn test_synthesize_modulated_oscillation_bounds() {
        let spectral = SpectralAnalysis {
            sub_bass: vec![0.8; 50],
            mid: vec![0.8; 50],
            high: vec![0.8; 50],
            spectrogram: vec![[0.5; 16]; 50],
        };

        let cfg = SpectralInfillConfig {
            band: FrequencyBand::Mid,
            target_axis: AxisChannel::Twist,
            mode: SpectralInfillMode::ModulatedOscillation,
            threshold: 0.1,
            min_pos: 20,
            max_pos: 80,
            min_interval_ms: 50,
            modulation_freq_hz: 8.0,
        };

        let actions = synthesize_spectral_actions(&spectral, &[], 100.0, &cfg, 0, 500);
        assert!(!actions.is_empty());
        for a in &actions {
            assert!(a.pos >= 20 && a.pos <= 80, "Position {} out of range", a.pos);
        }
    }

    #[test]
    fn test_generate_multiband_companion_bundle() {
        let mut sub_bass = vec![0.1f32; 100];
        sub_bass[25] = 0.9;
        sub_bass[75] = 0.95;

        let spectral = SpectralAnalysis {
            sub_bass,
            mid: vec![0.5; 100],
            high: vec![0.4; 100],
            spectrogram: vec![[0.3; 16]; 100],
        };

        let bundle = generate_multiband_companion_bundle(&spectral, &[], 100.0, 1000);
        assert!(bundle.channels.contains_key(&AxisChannel::Stroke));
        assert!(bundle.channels.contains_key(&AxisChannel::Surge));
        assert!(bundle.channels.contains_key(&AxisChannel::Twist));
        assert!(bundle.channels.contains_key(&AxisChannel::Suction));

        let stroke = bundle.channels.get(&AxisChannel::Stroke).unwrap();
        assert!(!stroke.actions.is_empty());
        let twist = bundle.channels.get(&AxisChannel::Twist).unwrap();
        assert!(!twist.actions.is_empty());
    }
}
