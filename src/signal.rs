//! Signal processing: numerical integration, polynomial detrending, rolling normalization, and keyframe extraction.

use crate::funscript::Action;

/// Trapezoidal / midpoint integration over radial flow velocities.
/// Resets to 0.0 upon scene cuts.
/// Input: slice of (radial_velocity, is_cut, timestamp_ms)
/// Returns: (integrated_displacement, timestamps_ms)
pub fn integrate_flow(samples: &[(f32, bool, i64)]) -> (Vec<f32>, Vec<i64>) {
    if samples.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let n = samples.len();
    let mut cum_flow = Vec::with_capacity(n);
    let mut timestamps = Vec::with_capacity(n);

    cum_flow.push(0.0f32);
    timestamps.push(samples[0].2);

    for i in 1..n {
        let (flow_prev, _, _) = samples[i - 1];
        let (flow_curr, is_cut, t_curr) = samples[i];

        if is_cut {
            cum_flow.push(0.0f32);
        } else {
            let mid = (flow_prev + flow_curr) * 0.5;
            let last = *cum_flow.last().unwrap();
            cum_flow.push(last + mid);
        }
        timestamps.push(t_curr);
    }

    // Phase adjustment: half-step blending, respecting cut resets
    let mut blended = cum_flow.clone();
    for i in 1..n {
        if !samples[i].1 {
            blended[i] = (cum_flow[i] + cum_flow[i - 1]) * 0.5;
        } else {
            blended[i] = 0.0;
        }
    }

    (blended, timestamps)
}

/// Computes Hanning window of length N
fn hanning_window(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }
    let factor = 2.0 * std::f32::consts::PI / ((n - 1) as f32);
    (0..n)
        .map(|i| 0.5 * (1.0 - (factor * (i as f32)).cos()))
        .collect()
}

/// Linear least squares regression (y = mx + b)
fn linear_fit(y: &[f32]) -> (f32, f32) {
    let n = y.len() as f32;
    if n < 2.0 {
        return (0.0, y.first().copied().unwrap_or(0.0));
    }

    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut sum_xx = 0.0f32;
    let mut sum_xy = 0.0f32;

    for (i, &val) in y.iter().enumerate() {
        let x = i as f32;
        sum_x += x;
        sum_y += val;
        sum_xx += x * x;
        sum_xy += x * val;
    }

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < 1e-6 {
        return (0.0, sum_y / n);
    }

    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;
    (slope, intercept)
}

/// Detrend signal using overlapping Hanning-windowed linear regression,
/// followed by 5-tap Gaussian smoothing and rolling min-max normalization into [0, 100].
pub fn detrend_and_normalize(
    signal: &[f32],
    effective_fps: f64,
    detrend_window_sec: f64,
    norm_window_sec: f64,
    discontinuity_threshold: f32,
) -> Vec<f32> {
    let n = signal.len();
    if n == 0 {
        return Vec::new();
    }

    let detrend_win = (detrend_window_sec * effective_fps).round().max(5.0) as usize;
    let mut detrended = vec![0.0f32; n];
    let mut weight_sum = vec![0.0f32; n];

    // Find discontinuity boundaries
    let mut boundaries = vec![0usize];
    for i in 1..n {
        if (signal[i] - signal[i - 1]).abs() > discontinuity_threshold {
            boundaries.push(i);
        }
    }
    boundaries.push(n);

    let overlap = (detrend_win / 2).max(1);

    for b in 0..(boundaries.len() - 1) {
        let seg_start = boundaries[b];
        let seg_end = boundaries[b + 1];
        let seg_len = seg_end - seg_start;

        if seg_len < 5 {
            let mean: f32 = signal[seg_start..seg_end].iter().sum::<f32>() / (seg_len as f32);
            for i in seg_start..seg_end {
                detrended[i] = signal[i] - mean;
                weight_sum[i] = 1.0;
            }
            continue;
        }

        if seg_len <= detrend_win {
            let slice = &signal[seg_start..seg_end];
            let (m, c) = linear_fit(slice);
            let weights = hanning_window(seg_len);

            for (i, &w) in weights.iter().enumerate() {
                let idx = seg_start + i;
                let trend = m * (i as f32) + c;
                detrended[idx] += (signal[idx] - trend) * w;
                weight_sum[idx] += w;
            }
        } else {
            let mut start = seg_start;
            while start + overlap < seg_end {
                let end = (start + detrend_win).min(seg_end);
                let cur_len = end - start;
                if cur_len < 2 {
                    break;
                }

                let slice = &signal[start..end];
                let (m, c) = linear_fit(slice);
                let weights = hanning_window(cur_len);

                for (i, &w) in weights.iter().enumerate() {
                    let idx = start + i;
                    let trend = m * (i as f32) + c;
                    detrended[idx] += (signal[idx] - trend) * w;
                    weight_sum[idx] += w;
                }

                if end == seg_end {
                    break;
                }
                start += overlap;
            }
        }
    }

    // Blend overlapping windows
    for i in 0..n {
        detrended[i] /= weight_sum[i].max(1e-6);
    }

    // 5-tap Gaussian smoothing filter: [1/16, 4/16, 6/16, 4/16, 1/16]
    let kernel = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut smoothed = vec![0.0f32; n];
    for i in 0..n {
        let mut sum = 0.0f32;
        let mut w_total = 0.0f32;
        for (k_idx, &k_val) in kernel.iter().enumerate() {
            let offset = (k_idx as isize) - 2;
            let sample_idx = (i as isize) + offset;
            if sample_idx >= 0 && sample_idx < (n as isize) {
                sum += detrended[sample_idx as usize] * k_val;
                w_total += k_val;
            }
        }
        smoothed[i] = if w_total > 0.0 { sum / w_total } else { detrended[i] };
    }

    // Rolling min-max normalization
    let norm_win = (norm_window_sec * effective_fps).round().max(3.0) as usize;
    let half_norm = norm_win / 2;
    let mut normalized = vec![50.0f32; n];

    for i in 0..n {
        let start = i.saturating_sub(half_norm);
        let end = (i + half_norm + 1).min(n);

        let mut local_min = f32::INFINITY;
        let mut local_max = f32::NEG_INFINITY;
        for j in start..end {
            local_min = local_min.min(smoothed[j]);
            local_max = local_max.max(smoothed[j]);
        }

        let span = local_max - local_min;
        if span > 1e-4 {
            let val = (smoothed[i] - local_min) / span * 100.0;
            normalized[i] = val.clamp(0.0, 100.0);
        } else {
            normalized[i] = 50.0;
        }
    }

    normalized
}

/// Extract Funscript action points from normalized values.
/// Inverts position (100 - norm) to follow standard funscript convention.
pub fn extract_actions(
    norm_values: &[f32],
    timestamps_ms: &[i64],
    keyframe_reduction: bool,
) -> Vec<Action> {
    let n = norm_values.len();
    if n == 0 {
        return Vec::new();
    }

    let key_indices = if keyframe_reduction && n > 2 {
        let mut indices = vec![0usize];
        for i in 1..(n - 1) {
            let d1 = norm_values[i] - norm_values[i - 1];
            let d2 = norm_values[i + 1] - norm_values[i];
            // Detect slope sign inversion (local maximum or minimum)
            if (d1 < 0.0) != (d2 < 0.0) {
                indices.push(i);
            }
        }
        indices.push(n - 1);
        indices
    } else {
        (0..n).collect()
    };

    let mut actions: Vec<Action> = Vec::with_capacity(key_indices.len());
    for ki in key_indices {
        let at = timestamps_ms[ki];
        let pos = (100.0 - norm_values[ki]).round().clamp(0.0, 100.0) as i32;

        if let Some(last) = actions.last_mut() {
            if (last as &Action).at == at {
                last.pos = pos;
                continue;
            }
        }
        actions.push(Action { at, pos });
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integration_and_cut_reset() {
        let samples = vec![
            (10.0, false, 0),
            (10.0, false, 33),
            (10.0, false, 66),
            (0.0, true, 100), // Cut
            (10.0, false, 133),
            (10.0, false, 166),
        ];

        let (cum, ts) = integrate_flow(&samples);
        assert_eq!(cum.len(), 6);
        assert_eq!(ts.len(), 6);
        assert_eq!(cum[3], 0.0); // Reset on cut
        assert!(cum[5] > cum[3]);
    }

    #[test]
    fn test_detrend_and_normalize_bounded() {
        let mut signal = Vec::new();
        for i in 0..300 {
            let t = i as f32 / 30.0;
            signal.push(t * 10.0 + (t * 2.0 * std::f32::consts::PI).sin() * 5.0);
        }

        let norm = detrend_and_normalize(&signal, 30.0, 2.0, 2.0, 1000.0);
        assert_eq!(norm.len(), 300);
        for &val in &norm {
            assert!(val >= 0.0 && val <= 100.0);
        }
    }

    #[test]
    fn test_extract_actions_inflections() {
        let norm = vec![0.0, 25.0, 50.0, 75.0, 100.0, 75.0, 50.0, 25.0, 0.0];
        let ts: Vec<i64> = (0..9).map(|i| i * 100).collect();

        let actions = extract_actions(&norm, &ts, true);
        assert!(actions.len() < norm.len());
        assert_eq!(actions[0].pos, 100); // 100 - 0
        assert_eq!(actions[1].pos, 0);   // 100 - 100 (peak)
        assert_eq!(actions[2].pos, 100); // 100 - 0 (valley)
    }
}
