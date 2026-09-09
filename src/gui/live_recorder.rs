//! Live Motion Recording & Gestural Puppeteering Subsystem.
//!
//! Enables real-time funscript recording by tracking mouse movements over the video
//! canvas or an interactive vertical slider while the video plays at variable speeds
//! (0.25x - 2.0x). Automatically detects directional inflections (peaks & valleys)
//! and applies Ramer-Douglas-Peucker (RDP) curve simplification.

use crate::funscript::{Action, Funscript};

/// State of the live gestural funscript recorder
#[derive(Debug, Clone)]
pub struct LiveRecorderState {
    /// Whether recording is armed (will record when playback is active)
    pub is_armed: bool,
    /// Whether a recording take is actively capturing samples
    pub is_recording_active: bool,
    /// Current live stroke position in [0.0, 100.0]
    pub live_pos: f32,
    /// Raw gestural samples collected during the current take: `(time_ms, pos)`
    pub raw_samples: Vec<(i64, f32)>,
    /// Last sampled video timestamp in milliseconds
    pub last_sample_ms: i64,
    /// Minimum interval between captured samples in milliseconds (default: 25ms / 40Hz)
    pub sample_interval_ms: i64,
    /// If true, only records while primary mouse button is held/dragged
    pub require_mouse_drag: bool,
    /// If true, inverts Y mapping (top = 0, bottom = 100)
    pub invert_y: bool,
    /// Deadband hysteresis threshold to filter out hand micro-tremors (default: 2.0 units)
    pub extrema_tolerance: f32,
    /// Ramer-Douglas-Peucker curve simplification tolerance (default: 1.5)
    pub rdp_epsilon: f32,
    /// Direction of motion: -1 = moving down, 0 = stationary, +1 = moving up
    last_direction: i8,
    /// Last committed turning point position
    last_extrema_pos: f32,
}

impl Default for LiveRecorderState {
    fn default() -> Self {
        Self {
            is_armed: false,
            is_recording_active: false,
            live_pos: 50.0,
            raw_samples: Vec::with_capacity(1024),
            last_sample_ms: -1,
            sample_interval_ms: 25,
            require_mouse_drag: false,
            invert_y: false,
            extrema_tolerance: 2.0,
            rdp_epsilon: 1.5,
            last_direction: 0,
            last_extrema_pos: 50.0,
        }
    }
}

#[allow(dead_code)]
impl LiveRecorderState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle arm/disarm recording state
    pub fn toggle_armed(&mut self) -> bool {
        self.is_armed = !self.is_armed;
        if !self.is_armed {
            self.is_recording_active = false;
        }
        self.is_armed
    }

    /// Arm recording
    pub fn arm(&mut self) {
        self.is_armed = true;
    }

    /// Disarm recording
    pub fn disarm(&mut self) {
        self.is_armed = false;
        self.is_recording_active = false;
    }

    /// Begin a new recording take starting at timestamp `start_time_ms`
    pub fn start_take(&mut self, start_time_ms: i64) {
        self.raw_samples.clear();
        self.is_recording_active = true;
        self.last_sample_ms = start_time_ms;
        self.last_direction = 0;
        self.last_extrema_pos = self.live_pos;
        self.raw_samples.push((start_time_ms, self.live_pos));
    }

    /// Update live position and record sample if interval elapsed
    pub fn sample(&mut self, time_ms: i64, pos: f32) {
        self.live_pos = pos.clamp(0.0, 100.0);

        if !self.is_recording_active {
            return;
        }

        if self.last_sample_ms < 0 || (time_ms - self.last_sample_ms).abs() >= self.sample_interval_ms {
            self.raw_samples.push((time_ms, self.live_pos));
            self.last_sample_ms = time_ms;
        }
    }

    /// Finalize current take, extract inflections and RDP-simplified keyframes.
    ///
    /// Returns `Some((start_ms, end_ms, actions))` or `None` if no samples were taken.
    pub fn finish_take(&mut self) -> Option<(i64, i64, Vec<Action>)> {
        if self.raw_samples.is_empty() || !self.is_recording_active {
            self.is_recording_active = false;
            return None;
        }

        self.is_recording_active = false;

        // Ensure chronological order
        self.raw_samples.sort_by_key(|s| s.0);

        let start_ms = self.raw_samples.first()?.0;
        let end_ms = self.raw_samples.last()?.0;

        if start_ms >= end_ms && self.raw_samples.len() <= 1 {
            let p = self.raw_samples[0].1.round().clamp(0.0, 100.0) as i32;
            return Some((start_ms, end_ms, vec![Action { at: start_ms, pos: p }]));
        }

        let actions = self.simplify_samples(&self.raw_samples);
        Some((start_ms, end_ms, actions))
    }

    /// Simplify raw gestural samples into clean funscript actions
    pub fn simplify_samples(&self, samples: &[(i64, f32)]) -> Vec<Action> {
        if samples.is_empty() {
            return Vec::new();
        }
        if samples.len() == 1 {
            return vec![Action {
                at: samples[0].0,
                pos: samples[0].1.round().clamp(0.0, 100.0) as i32,
            }];
        }

        // 1. Identify Inflection / Turning Points (Extrema: Peaks & Valleys)
        let mut key_indices = vec![0usize];
        let mut cur_dir: i8 = 0;
        let mut last_extrema_idx = 0usize;

        for i in 1..samples.len() {
            let delta = samples[i].1 - samples[last_extrema_idx].1;
            if delta.abs() >= self.extrema_tolerance {
                let new_dir = if delta > 0.0 { 1 } else { -1 };
                if cur_dir != 0 && new_dir != cur_dir {
                    // Direction reversed! Commit previous peak or valley
                    key_indices.push(last_extrema_idx);
                }
                cur_dir = new_dir;
                last_extrema_idx = i;
            } else if cur_dir != 0 {
                // Tracking within same direction, update peak/valley candidate
                if (cur_dir == 1 && samples[i].1 > samples[last_extrema_idx].1)
                    || (cur_dir == -1 && samples[i].1 < samples[last_extrema_idx].1)
                {
                    last_extrema_idx = i;
                }
            }
        }

        if last_extrema_idx != 0 && last_extrema_idx != samples.len() - 1 {
            key_indices.push(last_extrema_idx);
        }
        if *key_indices.last().unwrap() != samples.len() - 1 {
            key_indices.push(samples.len() - 1);
        }

        // Deduplicate indices
        key_indices.sort_unstable();
        key_indices.dedup();

        // 2. Apply RDP on segments between extrema to retain smooth curve transitions
        let mut final_indices = Vec::new();
        for w in key_indices.windows(2) {
            let start = w[0];
            let end = w[1];
            if end > start + 1 {
                let segment_points: Vec<(f64, f64)> = (start..=end)
                    .map(|idx| (samples[idx].0 as f64 / 100.0, samples[idx].1 as f64))
                    .collect();
                let kept = rdp_simplify(&segment_points, self.rdp_epsilon as f64);
                for (t_scaled, _) in kept {
                    let t_ms = (t_scaled * 100.0).round() as i64;
                    // Find closest matching index in segment
                    if let Some(matching_idx) = (start..=end).min_by_key(|&idx| (samples[idx].0 - t_ms).abs()) {
                        final_indices.push(matching_idx);
                    }
                }
            } else {
                final_indices.push(start);
                final_indices.push(end);
            }
        }

        final_indices.sort_unstable();
        final_indices.dedup();

        // 3. Assemble and sanitize funscript actions
        let mut actions: Vec<Action> = Vec::with_capacity(final_indices.len());
        for idx in final_indices {
            let at = samples[idx].0;
            let pos = samples[idx].1.round().clamp(0.0, 100.0) as i32;
            if let Some(last) = actions.last_mut() {
                if last.at == at {
                    last.pos = pos;
                    continue;
                }
            }
            actions.push(Action { at, pos });
        }

        actions
    }

    /// Splice recorded take into an existing funscript, replacing any prior actions in [start_ms, end_ms]
    pub fn splice_take_into_script(
        script: &mut Funscript,
        start_ms: i64,
        end_ms: i64,
        take_actions: &[Action],
    ) {
        if take_actions.is_empty() {
            return;
        }

        // Remove actions in the overwrite range
        script.actions.retain(|a| a.at < start_ms || a.at > end_ms);

        // Insert new actions
        script.actions.extend_from_slice(take_actions);

        // Sanitize & sort
        script.sanitize();
    }
}

/// 2D Ramer-Douglas-Peucker line simplification
fn rdp_simplify(points: &[(f64, f64)], epsilon: f64) -> Vec<(f64, f64)> {
    if points.len() <= 2 {
        return points.to_vec();
    }

    let mut dmax = 0.0f64;
    let mut index = 0usize;
    let end = points.len() - 1;

    for i in 1..end {
        let d = perpendicular_distance(points[i], points[0], points[end]);
        if d > dmax {
            index = i;
            dmax = d;
        }
    }

    if dmax > epsilon {
        let mut rec_results1 = rdp_simplify(&points[..=index], epsilon);
        let rec_results2 = rdp_simplify(&points[index..], epsilon);

        rec_results1.pop(); // remove duplicate middle point
        rec_results1.extend(rec_results2);
        rec_results1
    } else {
        vec![points[0], points[end]]
    }
}

/// Calculate perpendicular distance from point p to line segment (p1 -> p2)
fn perpendicular_distance(p: (f64, f64), p1: (f64, f64), p2: (f64, f64)) -> f64 {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;

    let mag = (dx * dx + dy * dy).sqrt();
    if mag < 1e-9 {
        let px = p.0 - p1.0;
        let py = p.1 - p1.1;
        return (px * px + py * py).sqrt();
    }

    let num = ((p2.1 - p1.1) * p.0 - (p2.0 - p1.0) * p.1 + p2.0 * p1.1 - p2.1 * p1.0).abs();
    num / mag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recorder_initial_state() {
        let rec = LiveRecorderState::new();
        assert!(!rec.is_armed);
        assert!(!rec.is_recording_active);
        assert_eq!(rec.live_pos, 50.0);
    }

    #[test]
    fn test_recorder_arm_toggle() {
        let mut rec = LiveRecorderState::new();
        assert!(rec.toggle_armed());
        assert!(rec.is_armed);
        assert!(!rec.toggle_armed());
        assert!(!rec.is_armed);
    }

    #[test]
    fn test_recorder_take_lifecycle_and_extrema() {
        let mut rec = LiveRecorderState::new();
        rec.arm();
        rec.start_take(0);

        // Simulate gestural stroke: 20 -> 80 -> 10 -> 90
        rec.sample(0, 20.0);
        rec.sample(250, 40.0);
        rec.sample(500, 80.0); // peak 1
        rec.sample(750, 50.0);
        rec.sample(1000, 10.0); // valley 1
        rec.sample(1250, 60.0);
        rec.sample(1500, 90.0); // peak 2

        let (start, end, actions) = rec.finish_take().expect("Take should produce actions");
        assert_eq!(start, 0);
        assert_eq!(end, 1500);
        assert!(!actions.is_empty());

        // Verify that peaks and valleys are captured
        let positions: Vec<i32> = actions.iter().map(|a| a.pos).collect();
        assert!(positions.iter().any(|&p| p >= 75), "Should capture upper stroke");
        assert!(positions.iter().any(|&p| p <= 25), "Should capture lower stroke");
    }

    #[test]
    fn test_splice_take_into_script() {
        let mut script = Funscript::new(vec![
            Action { at: 0, pos: 50 },
            Action { at: 1000, pos: 50 },
            Action { at: 2000, pos: 50 },
            Action { at: 3000, pos: 50 },
        ]);

        let take = vec![
            Action { at: 1000, pos: 90 },
            Action { at: 1500, pos: 10 },
            Action { at: 2000, pos: 95 },
        ];

        LiveRecorderState::splice_take_into_script(&mut script, 1000, 2000, &take);

        assert_eq!(script.actions[0], Action { at: 0, pos: 50 });
        assert_eq!(script.actions.last().unwrap(), &Action { at: 3000, pos: 50 });
        assert!(script.actions.iter().any(|a| a.at == 1500 && a.pos == 10));
    }

    #[test]
    fn test_rdp_straight_line_reduction() {
        let straight: Vec<(f64, f64)> = (0..=10).map(|i| (i as f64, i as f64 * 2.0)).collect();
        let simplified = rdp_simplify(&straight, 0.1);
        assert_eq!(simplified.len(), 2);
        assert_eq!(simplified[0], (0.0, 0.0));
        assert_eq!(simplified[1], (10.0, 20.0));
    }
}
