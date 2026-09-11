//! Jerk-limited S-curve filter and quintic Hermite polynomial smoothing.
//!
//! Transforms piecewise-linear funscripts with infinite jerk corners
//! into C² continuous-acceleration trajectories that protect physical servos.

use crate::funscript::{Action, Funscript};

/// S-Curve smoothing presets
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SCurvePreset {
    Off,
    Balanced,
    Protective,
}

impl SCurvePreset {
    pub const ALL: [SCurvePreset; 3] = [
        SCurvePreset::Off,
        SCurvePreset::Balanced,
        SCurvePreset::Protective,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            SCurvePreset::Off => "Off (Raw Linear)",
            SCurvePreset::Balanced => "Balanced (±35ms Blend)",
            SCurvePreset::Protective => "Protective (±75ms Blend)",
        }
    }

    pub fn blend_radius_ms(&self) -> i64 {
        match self {
            SCurvePreset::Off => 0,
            SCurvePreset::Balanced => 35,
            SCurvePreset::Protective => 75,
        }
    }
}

/// Quintic smoothstep polynomial S(u) = 10u³ - 15u⁴ + 6u⁵
/// First and second derivatives are zero at u = 0 and u = 1 (C² continuity).
#[inline]
pub fn quintic_smoothstep(u: f32) -> f32 {
    let u_clamped = u.clamp(0.0, 1.0);
    u_clamped * u_clamped * u_clamped * (u_clamped * (u_clamped * 6.0 - 15.0) + 10.0)
}

/// Evaluate continuous S-curve smoothed position at any arbitrary timecode
pub fn eval_scurve(actions: &[Action], target_ms: i64, blend_radius_ms: i64) -> f32 {
    if actions.is_empty() {
        return 50.0;
    }
    if actions.len() == 1 {
        return actions[0].pos as f32;
    }
    if target_ms <= actions[0].at {
        return actions[0].pos as f32;
    }
    if target_ms >= actions[actions.len() - 1].at {
        return actions[actions.len() - 1].pos as f32;
    }

    // Binary search for segment enclosing target_ms
    let idx = match actions.binary_search_by_key(&target_ms, |a| a.at) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };

    if idx >= actions.len() - 1 {
        return actions[actions.len() - 1].pos as f32;
    }

    if blend_radius_ms <= 0 {
        // Raw linear interpolation
        let a0 = &actions[idx];
        let a1 = &actions[idx + 1];
        let span = (a1.at - a0.at).max(1) as f32;
        let frac = ((target_ms - a0.at) as f32 / span).clamp(0.0, 1.0);
        return a0.pos as f32 + frac * (a1.pos - a0.pos) as f32;
    }

    // Check if target_ms falls within blend zone of vertex at idx or idx+1
    // Let vertex V_i be at actions[idx].
    // If target is near actions[idx]:
    if idx > 0 && idx < actions.len() - 1 {
        let v_prev = &actions[idx - 1];
        let v_curr = &actions[idx];
        let v_next = &actions[idx + 1];

        let max_delta = ((v_curr.at - v_prev.at) / 2).min((v_next.at - v_curr.at) / 2);
        let delta = blend_radius_ms.min(max_delta).max(1);

        let blend_start = v_curr.at - delta;
        let blend_end = v_curr.at + delta;

        if target_ms >= blend_start && target_ms <= blend_end {
            let u = (target_ms - blend_start) as f32 / (2 * delta) as f32;
            let s = quintic_smoothstep(u);

            let vel_in = (v_curr.pos - v_prev.pos) as f32 / (v_curr.at - v_prev.at) as f32;
            let vel_out = (v_next.pos - v_curr.pos) as f32 / (v_next.at - v_curr.at) as f32;

            let line_in = v_curr.pos as f32 + vel_in * (target_ms - v_curr.at) as f32;
            let line_out = v_curr.pos as f32 + vel_out * (target_ms - v_curr.at) as f32;

            return ((1.0 - s) * line_in + s * line_out).clamp(0.0, 100.0);
        }
    }

    // Check vertex at idx + 1
    if idx + 1 < actions.len() - 1 {
        let v_curr = &actions[idx];
        let v_next = &actions[idx + 1];
        let v_after = &actions[idx + 2];

        let max_delta = ((v_next.at - v_curr.at) / 2).min((v_after.at - v_next.at) / 2);
        let delta = blend_radius_ms.min(max_delta).max(1);

        let blend_start = v_next.at - delta;
        let blend_end = v_next.at + delta;

        if target_ms >= blend_start && target_ms <= blend_end {
            let u = (target_ms - blend_start) as f32 / (2 * delta) as f32;
            let s = quintic_smoothstep(u);

            let vel_in = (v_next.pos - v_curr.pos) as f32 / (v_next.at - v_curr.at) as f32;
            let vel_out = (v_after.pos - v_next.pos) as f32 / (v_after.at - v_next.at) as f32;

            let line_in = v_next.pos as f32 + vel_in * (target_ms - v_next.at) as f32;
            let line_out = v_next.pos as f32 + vel_out * (target_ms - v_next.at) as f32;

            return ((1.0 - s) * line_in + s * line_out).clamp(0.0, 100.0);
        }
    }

    // Outside blend zones, exact linear segment
    let a0 = &actions[idx];
    let a1 = &actions[idx + 1];
    let span = (a1.at - a0.at).max(1) as f32;
    let frac = ((target_ms - a0.at) as f32 / span).clamp(0.0, 1.0);
    a0.pos as f32 + frac * (a1.pos - a0.pos) as f32
}

/// Convert a Funscript into an S-curve smoothed funscript by densifying transition zones
pub fn smooth_funscript(script: &Funscript, blend_radius_ms: i64) -> Funscript {
    if script.actions.len() < 3 || blend_radius_ms <= 0 {
        return script.clone();
    }

    let mut smoothed_actions = Vec::new();
    let actions = &script.actions;

    for i in 0..actions.len() {
        if i == 0 || i == actions.len() - 1 {
            smoothed_actions.push(actions[i]);
            continue;
        }

        let prev = &actions[i - 1];
        let curr = &actions[i];
        let next = &actions[i + 1];

        let max_delta = ((curr.at - prev.at) / 2).min((next.at - curr.at) / 2);
        let delta = blend_radius_ms.min(max_delta).max(1);

        let t_start = curr.at - delta;
        let t_end = curr.at + delta;

        // Sample blend zone every 10ms
        let step = 10;
        let mut t = t_start;
        while t <= t_end {
            let pos = eval_scurve(actions, t, blend_radius_ms);
            smoothed_actions.push(Action {
                at: t,
                pos: pos.round().clamp(0.0, 100.0) as i32,
            });
            t += step;
        }
    }

    let mut res = Funscript::new(smoothed_actions);
    res.sanitize();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quintic_smoothstep_bounds() {
        assert_eq!(quintic_smoothstep(0.0), 0.0);
        assert_eq!(quintic_smoothstep(1.0), 1.0);
        assert_eq!(quintic_smoothstep(0.5), 0.5);

        // Check slope at ends is flat (zero derivative)
        let eps = 1e-4;
        let slope_start = (quintic_smoothstep(eps) - quintic_smoothstep(0.0)) / eps;
        let slope_end = (quintic_smoothstep(1.0) - quintic_smoothstep(1.0 - eps)) / eps;
        assert!(slope_start < 0.01);
        assert!(slope_end < 0.01);
    }

    #[test]
    fn test_scurve_apex_preservation() {
        // V-corner stroke down then up: 0ms -> 100, 200ms -> 0, 400ms -> 100
        let actions = vec![
            Action { at: 0, pos: 100 },
            Action { at: 200, pos: 0 },
            Action { at: 400, pos: 100 },
        ];

        // At vertex 200ms, the position must remain exactly at the apex (0.0)
        let pos_at_vertex = eval_scurve(&actions, 200, 40);
        assert_eq!(pos_at_vertex, 0.0);

        // Approaching vertex (190ms), smooth position decelerates into apex (0.0 <= pos <= 5.0)
        let pos_scurve = eval_scurve(&actions, 190, 40);
        assert!((0.0..=5.0).contains(&pos_scurve));
    }

    #[test]
    fn test_smooth_funscript_densification() {
        let actions = vec![
            Action { at: 0, pos: 10 },
            Action { at: 200, pos: 90 },
            Action { at: 400, pos: 10 },
        ];
        let original = Funscript::new(actions);
        let smoothed = smooth_funscript(&original, 35);

        assert!(smoothed.actions.len() > original.actions.len());
        // All positions clamped in [0, 100]
        for a in &smoothed.actions {
            assert!(a.pos >= 0 && a.pos <= 100);
        }
    }
}
