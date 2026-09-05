//! Funscript data model, parser, serializer, and Script Doctor linter.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

/// Canonical `.funscript` file schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Funscript {
    pub version: String,
    pub actions: Vec<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}
