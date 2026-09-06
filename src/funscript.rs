//! Funscript data model, parser, serializer, and Script Doctor linter.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

/// Canonical `.funscript` file schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Funscript {
    pub version: String,
    pub actions: Vec<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Action {
    /// Timestamp in milliseconds
    pub at: i64,
    /// Position in [0, 100]
    pub pos: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
}

impl Funscript {
    pub fn new(actions: Vec<Action>) -> Self {
        Self {
            version: "1.0".to_string(),
            actions,
            metadata: None,
        }
    }

    /// Load a funscript from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path.as_ref())
            .with_context(|| format!("Failed to open funscript file: {:?}", path.as_ref()))?;
        let reader = BufReader::new(file);
        let script: Self = serde_json::from_reader(reader)
            .with_context(|| format!("Failed to parse JSON funscript: {:?}", path.as_ref()))?;
        Ok(script)
    }

    /// Save the funscript to a formatted JSON file.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path.as_ref())
            .with_context(|| format!("Failed to create output file: {:?}", path.as_ref()))?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self)
            .with_context(|| format!("Failed to write funscript JSON: {:?}", path.as_ref()))?;
        Ok(())
    }

    /// Deduplicate consecutive identical timestamps or redundant positions.
    pub fn sanitize(&mut self) {
        if self.actions.is_empty() {
            return;
        }

        // 1. Sort strictly by timestamp
        self.actions.sort_by_key(|a| a.at);

        // 2. Deduplicate identical timestamps (keep latest position)
        let mut clean: Vec<Action> = Vec::with_capacity(self.actions.len());
        for action in &self.actions {
            let clamped_pos = action.pos.clamp(0, 100);
            if let Some(last) = clean.last_mut() {
                if last.at == action.at {
                    last.pos = clamped_pos;
                    continue;
                }
            }
            clean.push(Action {
                at: action.at,
                pos: clamped_pos,
            });
        }
        self.actions = clean;
    }

    /// Fix transitions that exceed safe hardware motor speed limits by clamping position deltas.
    pub fn fix_speed_violations(&mut self, max_safe_speed: f64) -> usize {
        if self.actions.len() < 2 {
            return 0;
        }

        self.sanitize();
        let mut fixed_count = 0;

        for i in 1..self.actions.len() {
            let dt = (self.actions[i].at - self.actions[i - 1].at).max(1) as f64 / 1000.0;
            let max_allowed_delta = (max_safe_speed * dt).round() as i32;

            let prev_pos = self.actions[i - 1].pos;
            let curr_pos = self.actions[i].pos;
            let delta = curr_pos - prev_pos;

            if delta.abs() > max_allowed_delta {
                let clamped_pos = if delta > 0 {
                    (prev_pos + max_allowed_delta).min(100)
                } else {
                    (prev_pos - max_allowed_delta).max(0)
                };
                self.actions[i].pos = clamped_pos;
                fixed_count += 1;
            }
        }

        fixed_count
    }

    /// Remove micro-jitter actions with negligible position change within a small time window.
    pub fn remove_micro_jitters(&mut self, min_dt_ms: i64, min_dpos: i32) -> usize {
        if self.actions.len() < 3 {
            return 0;
        }

        self.sanitize();
        let initial_len = self.actions.len();
        let mut filtered = Vec::with_capacity(initial_len);

        // Always keep first point
        filtered.push(self.actions[0]);

        for i in 1..(self.actions.len() - 1) {
            let prev = filtered.last().unwrap();
            let curr = &self.actions[i];
            let dt = curr.at - prev.at;
            let dpos = (curr.pos - prev.pos).abs();

            if dt < min_dt_ms && dpos <= min_dpos {
                // Skip micro-jitter point
                continue;
            }
            filtered.push(*curr);
        }

        // Always keep last point
        if let Some(last) = self.actions.last() {
            if filtered.last().map(|p| p.at) != Some(last.at) {
                filtered.push(*last);
            }
        }

        let removed = initial_len - filtered.len();
        self.actions = filtered;
        removed
    }

    /// Linearly interpolate position [0.0, 100.0] at a given timestamp.
    pub fn interpolate_position(&self, time_ms: i64) -> f32 {
        if self.actions.is_empty() {
            return 0.0;
        }
        if time_ms <= self.actions[0].at {
            return self.actions[0].pos as f32;
        }
        if time_ms >= self.actions.last().unwrap().at {
            return self.actions.last().unwrap().pos as f32;
        }

        let idx = match self.actions.binary_search_by_key(&time_ms, |a| a.at) {
            Ok(i) => return self.actions[i].pos as f32,
            Err(i) => i,
        };

        let prev = &self.actions[idx - 1];
        let next = &self.actions[idx];
        let dt = (next.at - prev.at).max(1) as f32;
        let frac = ((time_ms - prev.at) as f32 / dt).clamp(0.0, 1.0);
        prev.pos as f32 + frac * (next.pos - prev.pos) as f32
    }

    /// Infill a periodic pattern between two timestamps
    pub fn infill_pattern(
        &mut self,
        start_at: i64,
        end_at: i64,
        min_pos: i32,
        max_pos: i32,
        freq_hz: f32,
        pattern: InfillPattern,
    ) {
        if start_at >= end_at || freq_hz <= 0.0 {
            return;
        }

        // Remove existing actions strictly between start_at and end_at
        self.actions.retain(|a| a.at < start_at || a.at > end_at);

        let duration_ms = (end_at - start_at) as f32;
        let period_ms = (1000.0 / freq_hz).max(20.0);
        let num_cycles = duration_ms / period_ms;
        let points_per_cycle = match pattern {
            InfillPattern::Sine => 8,
            InfillPattern::Triangle => 4,
            InfillPattern::Sawtooth => 3,
        };
        let total_points = (num_cycles * points_per_cycle as f32).round() as usize;
        let step_ms = duration_ms / total_points.max(1) as f32;

        for i in 0..=total_points {
            let t = start_at + (i as f32 * step_ms).round() as i64;
            let phase = ((t - start_at) as f32 / period_ms) * 2.0 * std::f32::consts::PI;

            let norm_val = match pattern {
                InfillPattern::Sine => (phase.sin() + 1.0) / 2.0,
                InfillPattern::Triangle => {
                    let p = ((t - start_at) as f32 / period_ms).fract();
                    if p < 0.5 {
                        p * 2.0
                    } else {
                        (1.0 - p) * 2.0
                    }
                }
                InfillPattern::Sawtooth => ((t - start_at) as f32 / period_ms).fract(),
            };

            let pos = (min_pos as f32 + norm_val * (max_pos - min_pos) as f32).round() as i32;
            self.actions.push(Action {
                at: t,
                pos: pos.clamp(0, 100),
            });
        }

        self.sanitize();
    }

    /// Scale amplitude of all actions relative to a pivot position
    pub fn scale_amplitude(&mut self, factor: f32, pivot_pos: i32) {
        for a in &mut self.actions {
            let diff = (a.pos - pivot_pos) as f32;
            let new_pos = (pivot_pos as f32 + diff * factor).round() as i32;
            a.pos = new_pos.clamp(0, 100);
        }
    }

    /// Invert all action positions (pos -> 100 - pos)
    pub fn invert_positions(&mut self) {
        for a in &mut self.actions {
            a.pos = 100 - a.pos.clamp(0, 100);
        }
    }
}

/// Supported periodic waveform infill patterns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfillPattern {
    Sine,
    Triangle,
    Sawtooth,
}

/// Standard multi-axis haptic channels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AxisChannel {
    Stroke,  // L0 - Primary Up/Down
    Surge,   // L1 - Forward/Backward
    Sway,    // L2 - Left/Right
    Pitch,   // R1 - Tilt Up/Down
    Roll,    // R0 - Tilt Left/Right
    Twist,   // R2 - Axial Rotation
    Suction, // V0 - Vacuum / Pressure
}

impl AxisChannel {
    pub const ALL: [AxisChannel; 7] = [
        AxisChannel::Stroke,
        AxisChannel::Surge,
        AxisChannel::Sway,
        AxisChannel::Pitch,
        AxisChannel::Roll,
        AxisChannel::Twist,
        AxisChannel::Suction,
    ];

    pub fn tcode_axis(&self) -> &'static str {
        match self {
            AxisChannel::Stroke => "L0",
            AxisChannel::Surge => "L1",
            AxisChannel::Sway => "L2",
            AxisChannel::Pitch => "R1",
            AxisChannel::Roll => "R0",
            AxisChannel::Twist => "R2",
            AxisChannel::Suction => "V0",
        }
    }

    pub fn file_suffix(&self) -> &'static str {
        match self {
            AxisChannel::Stroke => "",
            AxisChannel::Surge => ".surge",
            AxisChannel::Sway => ".sway",
            AxisChannel::Pitch => ".pitch",
            AxisChannel::Roll => ".roll",
            AxisChannel::Twist => ".twist",
            AxisChannel::Suction => ".suction",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            AxisChannel::Stroke => "Stroke (L0)",
            AxisChannel::Surge => "Surge (L1)",
            AxisChannel::Sway => "Sway (L2)",
            AxisChannel::Pitch => "Pitch (R1)",
            AxisChannel::Roll => "Roll (R0)",
            AxisChannel::Twist => "Twist (R2)",
            AxisChannel::Suction => "Suction (V0)",
        }
    }

    pub fn color_rgb(&self) -> (u8, u8, u8) {
        match self {
            AxisChannel::Stroke => (0, 200, 255),   // Bright Cyan
            AxisChannel::Surge => (255, 127, 80),   // Coral
            AxisChannel::Sway => (186, 85, 211),    // Medium Orchid / Purple
            AxisChannel::Pitch => (50, 205, 50),    // Lime Green
            AxisChannel::Roll => (255, 215, 0),     // Gold
            AxisChannel::Twist => (32, 178, 170),   // Light Sea Green
            AxisChannel::Suction => (255, 20, 147), // Deep Pink
        }
    }
}

/// Container managing all multi-axis funscripts for a scene
#[derive(Debug, Clone, Default)]
pub struct MultiAxisScript {
    pub channels: HashMap<AxisChannel, Funscript>,
}

impl MultiAxisScript {
    pub fn new() -> Self {
        let mut channels = HashMap::new();
        channels.insert(AxisChannel::Stroke, Funscript::new(vec![]));
        Self { channels }
    }

    /// Retrieve or initialize a script channel
    pub fn get_or_create(&mut self, axis: AxisChannel) -> &mut Funscript {
        self.channels.entry(axis).or_insert_with(|| Funscript::new(vec![]))
    }

    /// Retrieve an immutable reference to a channel if present
    pub fn get(&self, axis: AxisChannel) -> Option<&Funscript> {
        self.channels.get(&axis)
    }

    /// Load all companion multi-axis files sharing the base stem
    pub fn load_bundle<P: AsRef<Path>>(path: P) -> Result<Self> {
        let (parent, base_stem) = extract_base_stem(&path);
        let mut channels = HashMap::new();

        for axis in &AxisChannel::ALL {
            let filename = if *axis == AxisChannel::Stroke {
                format!("{}.funscript", base_stem)
            } else {
                format!("{}{}.funscript", base_stem, axis.file_suffix())
            };
            let candidate = parent.join(filename);
            if candidate.exists() {
                if let Ok(script) = Funscript::load(&candidate) {
                    channels.insert(*axis, script);
                }
            }
        }

        channels
            .entry(AxisChannel::Stroke)
            .or_insert_with(|| Funscript::new(vec![]));

        Ok(Self { channels })
    }

    /// Save all non-empty channels into their canonical companion files
    pub fn save_bundle<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let (parent, base_stem) = extract_base_stem(&path);
        for (axis, script) in &self.channels {
            if *axis == AxisChannel::Stroke || !script.actions.is_empty() {
                let filename = if *axis == AxisChannel::Stroke {
                    format!("{}.funscript", base_stem)
                } else {
                    format!("{}{}.funscript", base_stem, axis.file_suffix())
                };
                let target_path = parent.join(filename);
                script.save(&target_path)?;
            }
        }
        Ok(())
    }
}

fn extract_base_stem<P: AsRef<Path>>(path: P) -> (PathBuf, String) {
    let p = path.as_ref();
    let parent = p.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let filename = p
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let clean_name = if filename.ends_with(".funscript") {
        &filename[..filename.len() - 10]
    } else {
        match filename.rfind('.') {
            Some(idx) => &filename[..idx],
            None => &filename,
        }
    };

    // Strip known axis suffixes if present (e.g. .surge, .pitch)
    for axis in &AxisChannel::ALL {
        let sfx = axis.file_suffix();
        if !sfx.is_empty() && clean_name.ends_with(sfx) {
            return (parent, clean_name[..clean_name.len() - sfx.len()].to_string());
        }
    }

    (parent, clean_name.to_string())
}

// ==============================================================================
// Undo / Redo History Engine
// ==============================================================================

/// Bounded undo/redo history manager for keyframe editing.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoHistory {
    undo_stack: Vec<Funscript>,
    redo_stack: Vec<Funscript>,
    max_depth: usize,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self::new(50)
    }
}

impl UndoHistory {
    pub fn new(max_depth: usize) -> Self {
        Self {
            undo_stack: Vec::with_capacity(max_depth),
            redo_stack: Vec::new(),
            max_depth: max_depth.max(5),
        }
    }

    /// Push current state to undo history before making modifications.
    pub fn push_snapshot(&mut self, script: &Funscript) {
        if self.undo_stack.len() >= self.max_depth {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(script.clone());
        self.redo_stack.clear();
    }

    /// Undo to previous state. Returns true if undo was performed.
    pub fn undo(&mut self, current: &mut Funscript) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(current.clone());
            *current = prev;
            true
        } else {
            false
        }
    }

    /// Redo to next state. Returns true if redo was performed.
    pub fn redo(&mut self, current: &mut Funscript) -> bool {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(current.clone());
            *current = next;
            true
        } else {
            false
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

// ==============================================================================
// Script Doctor Linter
// ==============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub total_actions: usize,
    pub duration_ms: i64,
    pub min_pos: i32,
    pub max_pos: i32,
    pub avg_speed_units_per_sec: f64,
    pub max_speed_units_per_sec: f64,
    pub speed_violations_count: usize,
    pub timing_inversions_count: usize,
    pub micro_jitter_count: usize,
    pub issues: Vec<DoctorIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorIssue {
    pub timestamp_ms: i64,
    pub severity: IssueSeverity,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
}

pub struct DoctorConfig {
    /// Maximum safe hardware speed in pos-units per second (e.g. 400 units/s)
    pub max_safe_speed: f64,
    /// Threshold in units to consider a micro-jitter (e.g. 2 units)
    pub micro_jitter_threshold: i32,
}

impl Default for DoctorConfig {
    fn default() -> Self {
        Self {
            max_safe_speed: 450.0,
            micro_jitter_threshold: 2,
        }
    }
}

/// Audit a funscript for hardware safety and smoothness.
pub fn diagnose_funscript(script: &Funscript, config: &DoctorConfig) -> DoctorReport {
    let mut issues = Vec::new();
    let actions = &script.actions;

    if actions.is_empty() {
        return DoctorReport {
            total_actions: 0,
            duration_ms: 0,
            min_pos: 0,
            max_pos: 0,
            avg_speed_units_per_sec: 0.0,
            max_speed_units_per_sec: 0.0,
            speed_violations_count: 0,
            timing_inversions_count: 0,
            micro_jitter_count: 0,
            issues: vec![DoctorIssue {
                timestamp_ms: 0,
                severity: IssueSeverity::Error,
                description: "Funscript contains zero actions.".to_string(),
            }],
        };
    }

    let mut min_pos = 100;
    let mut max_pos = 0;
    let mut max_speed = 0.0;
    let mut total_speed = 0.0;
    let mut speed_samples = 0;
    let mut speed_violations = 0;
    let mut timing_inversions = 0;
    let mut micro_jitters = 0;

    for i in 0..actions.len() {
        let cur = &actions[i];
        min_pos = min_pos.min(cur.pos);
        max_pos = max_pos.max(cur.pos);

        if cur.pos < 0 || cur.pos > 100 {
            issues.push(DoctorIssue {
                timestamp_ms: cur.at,
                severity: IssueSeverity::Error,
                description: format!("Position {} is outside valid range [0, 100]", cur.pos),
            });
        }

        if i > 0 {
            let prev = &actions[i - 1];
            let dt = cur.at - prev.at;

            if dt <= 0 {
                timing_inversions += 1;
                issues.push(DoctorIssue {
                    timestamp_ms: cur.at,
                    severity: IssueSeverity::Error,
                    description: format!(
                        "Non-increasing timestamp: prev {}ms, cur {}ms (delta {}ms)",
                        prev.at, cur.at, dt
                    ),
                });
            } else {
                let dpos = (cur.pos - prev.pos).abs();
                let speed = (dpos as f64) / (dt as f64 / 1000.0);
                total_speed += speed;
                speed_samples += 1;

                if speed > max_speed {
                    max_speed = speed;
                }

                if speed > config.max_safe_speed {
                    speed_violations += 1;
                    issues.push(DoctorIssue {
                        timestamp_ms: cur.at,
                        severity: IssueSeverity::Warning,
                        description: format!(
                            "Excessive hardware speed: {:.1} units/s (delta {} in {}ms)",
                            speed, dpos, dt
                        ),
                    });
                }

                if dpos > 0 && dpos <= config.micro_jitter_threshold && dt < 80 {
                    micro_jitters += 1;
                    issues.push(DoctorIssue {
                        timestamp_ms: cur.at,
                        severity: IssueSeverity::Info,
                        description: format!(
                            "Micro-jitter detected: tiny motion of {} units in {}ms",
                            dpos, dt
                        ),
                    });
                }
            }
        }
    }

    let duration_ms = actions.last().map(|a| a.at).unwrap_or(0);
    let avg_speed = if speed_samples > 0 {
        total_speed / (speed_samples as f64)
    } else {
        0.0
    };

    DoctorReport {
        total_actions: actions.len(),
        duration_ms,
        min_pos,
        max_pos,
        avg_speed_units_per_sec: avg_speed,
        max_speed_units_per_sec: max_speed,
        speed_violations_count: speed_violations,
        timing_inversions_count: timing_inversions,
        micro_jitter_count: micro_jitters,
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_funscript_serialization_roundtrip() {
        let script = Funscript::new(vec![
            Action { at: 0, pos: 50 },
            Action { at: 500, pos: 100 },
            Action { at: 1000, pos: 0 },
        ]);

        let json = serde_json::to_string(&script).unwrap();
        let parsed: Funscript = serde_json::from_str(&json).unwrap();
        assert_eq!(script, parsed);
    }

    #[test]
    fn test_doctor_diagnose_speed_and_inversions() {
        let script = Funscript::new(vec![
            Action { at: 0, pos: 0 },
            // Speed = 100 / 0.1s = 1000 units/s (exceeds default 450)
            Action { at: 100, pos: 100 },
            // Non-increasing timestamp
            Action { at: 100, pos: 50 },
        ]);

        let report = diagnose_funscript(&script, &DoctorConfig::default());
        assert_eq!(report.speed_violations_count, 1);
        assert_eq!(report.timing_inversions_count, 1);
        assert!(report.max_speed_units_per_sec >= 999.0);
    }

    #[test]
    fn test_sanitize_deduplicates_and_clamps() {
        let mut script = Funscript::new(vec![
            Action { at: 100, pos: 120 }, // Clamp to 100
            Action { at: 100, pos: 90 },  // Deduplicate
            Action { at: 50, pos: -10 },  // Clamp to 0 and sort
        ]);

        script.sanitize();
        assert_eq!(script.actions.len(), 2);
        assert_eq!(script.actions[0], Action { at: 50, pos: 0 });
        assert_eq!(script.actions[1], Action { at: 100, pos: 90 });
    }

    #[test]
    fn test_fix_speed_violations() {
        let mut script = Funscript::new(vec![
            Action { at: 0, pos: 0 },
            // dt = 100ms, max speed 400 u/s => max delta = 40. pos 100 should be clamped to 40.
            Action { at: 100, pos: 100 },
        ]);

        let fixed = script.fix_speed_violations(400.0);
        assert_eq!(fixed, 1);
        assert_eq!(script.actions[1].pos, 40);

        let report = diagnose_funscript(&script, &DoctorConfig { max_safe_speed: 400.0, ..Default::default() });
        assert_eq!(report.speed_violations_count, 0);
    }

    #[test]
    fn test_remove_micro_jitters() {
        let mut script = Funscript::new(vec![
            Action { at: 0, pos: 0 },
            Action { at: 10, pos: 1 }, // Micro jitter (<20ms, delta <= 2)
            Action { at: 500, pos: 100 },
        ]);

        let removed = script.remove_micro_jitters(20, 2);
        assert_eq!(removed, 1);
        assert_eq!(script.actions.len(), 2);
        assert_eq!(script.actions[0].at, 0);
        assert_eq!(script.actions[1].at, 500);
    }

    #[test]
    fn test_interpolate_position() {
        let script = Funscript::new(vec![
            Action { at: 0, pos: 0 },
            Action { at: 1000, pos: 100 },
        ]);

        assert_eq!(script.interpolate_position(-50), 0.0);
        assert_eq!(script.interpolate_position(0), 0.0);
        assert_eq!(script.interpolate_position(500), 50.0);
        assert_eq!(script.interpolate_position(1000), 100.0);
        assert_eq!(script.interpolate_position(1500), 100.0);
    }

    #[test]
    fn test_infill_pattern_sine() {
        let mut script = Funscript::new(vec![]);
        script.infill_pattern(0, 1000, 10, 90, 2.0, InfillPattern::Sine);

        assert!(!script.actions.is_empty());
        assert_eq!(script.actions.first().unwrap().at, 0);
        assert_eq!(script.actions.last().unwrap().at, 1000);
        for a in &script.actions {
            assert!(a.pos >= 10 && a.pos <= 90);
        }
    }

    #[test]
    fn test_transformations_invert_and_scale() {
        let mut script = Funscript::new(vec![
            Action { at: 0, pos: 20 },
            Action { at: 500, pos: 80 },
        ]);

        script.invert_positions();
        assert_eq!(script.actions[0].pos, 80);
        assert_eq!(script.actions[1].pos, 20);

        script.scale_amplitude(0.5, 50);
        assert_eq!(script.actions[0].pos, 65); // 50 + (80-50)*0.5 = 65
        assert_eq!(script.actions[1].pos, 35); // 50 + (20-50)*0.5 = 35
    }

    #[test]
    fn test_extract_base_stem() {
        let (parent, stem) = extract_base_stem("/home/user/video.surge.funscript");
        assert_eq!(stem, "video");
        assert_eq!(parent, PathBuf::from("/home/user"));

        let (_, stem2) = extract_base_stem("/tmp/clip.funscript");
        assert_eq!(stem2, "clip");

        let (_, stem3) = extract_base_stem("/tmp/movie.mp4");
        assert_eq!(stem3, "movie");
    }

    #[test]
    fn test_undo_history_lifecycle() {
        let mut history = UndoHistory::new(3);
        let mut script = Funscript::new(vec![Action { at: 100, pos: 10 }]);

        assert!(!history.can_undo());
        assert!(!history.can_redo());

        // Snapshot 1
        history.push_snapshot(&script);
        script.actions.push(Action { at: 200, pos: 50 });
        assert_eq!(script.actions.len(), 2);
        assert!(history.can_undo());

        // Snapshot 2
        history.push_snapshot(&script);
        script.actions.push(Action { at: 300, pos: 90 });
        assert_eq!(script.actions.len(), 3);

        // Undo 1 step
        assert!(history.undo(&mut script));
        assert_eq!(script.actions.len(), 2);
        assert_eq!(script.actions[1].pos, 50);
        assert!(history.can_redo());

        // Undo 2nd step
        assert!(history.undo(&mut script));
        assert_eq!(script.actions.len(), 1);
        assert!(!history.can_undo());

        // Redo 1 step
        assert!(history.redo(&mut script));
        assert_eq!(script.actions.len(), 2);

        // Redo 2nd step
        assert!(history.redo(&mut script));
        assert_eq!(script.actions.len(), 3);
        assert!(!history.can_redo());
    }
}
