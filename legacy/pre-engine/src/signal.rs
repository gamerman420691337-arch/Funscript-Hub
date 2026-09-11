//! Signal processing: numerical integration, polynomial detrending, rolling normalization, and keyframe extraction.

use crate::funscript::Action;
use rayon::prelude::*;

/// Direct cumulative integration over motion velocities.
/// Resets to 0.0 upon scene cuts.
/// Input: slice of (motion_velocity, is_cut, timestamp_ms)
/// Returns: (integrated_displacement, timestamps_ms)
pub fn integrate_flow(samples: &[(f32, bool, i64)]) -> (Vec<f32>, Vec<i64>) {
    if samples.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let n = samples.len();
    let mut cum_flow = Vec::with_capacity(n);
    let mut timestamps = Vec::with_capacity(n);

    let mut current_pos = 0.0f32;
    for &(dot, is_cut, ts) in samples {
        if is_cut {
            current_pos = 0.0;
        } else {
            current_pos += dot;
        }
        cum_flow.push(current_pos);
        timestamps.push(ts);
    }

    (cum_flow, timestamps)
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
    // Pre-compute Hanning window once for the full detrend_win size; slice for shorter segments
    let full_hanning = hanning_window(detrend_win);

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
            // Reuse cached window, slicing if segment is shorter
            let weights = if seg_len == detrend_win { &full_hanning[..] } else { &hanning_window(seg_len)[..seg_len] };

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
                // Reuse full window when cur_len == detrend_win (99% of iterations)
                let weights: &[f32] = if cur_len == detrend_win {
                    &full_hanning
                } else {
                    // Edge case: partial window at segment boundary
                    &full_hanning[..cur_len]
                };

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
    // Parallelized across threads with Rayon for high-throughput batch execution
    let kernel = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let smoothed: Vec<f32> = (0..n)
        .into_par_iter()
        .map(|i| {
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
            if w_total > 0.0 { sum / w_total } else { detrended[i] }
        })
        .collect();

    // Rolling min-max normalization in linear O(N) time via monotonic sliding windows
    // Merged into single pass: compute normalized values directly from deque fronts
    let norm_win = (norm_window_sec * effective_fps).round().max(3.0) as usize;
    let half_norm = norm_win / 2;
    let mut normalized = vec![50.0f32; n];

    let mut min_dq: std::collections::VecDeque<usize> = std::collections::VecDeque::with_capacity(norm_win);
    let mut max_dq: std::collections::VecDeque<usize> = std::collections::VecDeque::with_capacity(norm_win);

    let mut r = 0;
    for i in 0..n {
        let win_end = (i + half_norm).min(n - 1);
        let win_start = i.saturating_sub(half_norm);

        while r <= win_end {
            let val = smoothed[r];
            while let Some(&back) = min_dq.back() {
                if smoothed[back] >= val {
                    min_dq.pop_back();
                } else {
                    break;
                }
            }
            min_dq.push_back(r);

            while let Some(&back) = max_dq.back() {
                if smoothed[back] <= val {
                    max_dq.pop_back();
                } else {
                    break;
                }
            }
            max_dq.push_back(r);

            r += 1;
        }

        while let Some(&front) = min_dq.front() {
            if front < win_start {
                min_dq.pop_front();
            } else {
                break;
            }
        }

        while let Some(&front) = max_dq.front() {
            if front < win_start {
                max_dq.pop_front();
            } else {
                break;
            }
        }

        // Compute normalized value directly — no intermediate mins/maxs buffers
        let local_min = smoothed[*min_dq.front().unwrap_or(&i)];
        let local_max = smoothed[*max_dq.front().unwrap_or(&i)];
        let span = local_max - local_min;
        if span > 1e-4 {
            let val = (smoothed[i] - local_min) / span * 100.0;
            normalized[i] = val.clamp(0.0, 100.0);
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

/// Extract full-range Funscript actions from a displacement/position signal.
/// Detects local extrema (peaks and valleys) with prominence and slope filtering.
/// Equalizes stroke amplitudes so full strokes reach [0, 100], while preserving relative amplitudes
/// for smaller/vibratory oscillations.
pub fn extract_actions_full_range(
    signal: &[f32],
    timestamps_ms: &[i64],
    min_prominence_pct: f32,
    full_range_equalize: bool,
) -> Vec<Action> {
    let n = signal.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![Action { at: timestamps_ms[0], pos: 50 }];
    }

    // 1. Light 3-tap smoothing to eliminate single-sample high frequency noise
    let mut smoothed = vec![0.0f32; n];
    smoothed[0] = signal[0];
    for i in 1..(n - 1) {
        smoothed[i] = signal[i - 1] * 0.25 + signal[i] * 0.5 + signal[i + 1] * 0.25;
    }
    smoothed[n - 1] = signal[n - 1];

    let min_val = smoothed.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let max_val = smoothed.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let global_span = (max_val - min_val).max(1e-4);
    let prominence_threshold = global_span * (min_prominence_pct / 100.0).clamp(0.005, 0.5);

    // 2. Detect local extrema candidates (slope sign changes)
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum ExtremaKind {
        Peak,
        Valley,
    }

    let mut candidates = Vec::new();
    for i in 1..(n - 1) {
        let d1 = smoothed[i] - smoothed[i - 1];
        let d2 = smoothed[i + 1] - smoothed[i];
        if d1 > 0.0 && d2 <= 0.0 {
            candidates.push((i, ExtremaKind::Peak, smoothed[i]));
        } else if d1 < 0.0 && d2 >= 0.0 {
            candidates.push((i, ExtremaKind::Valley, smoothed[i]));
        }
    }

    // 3. Filter candidates by prominence / alternating peak-valley reduction
    let mut filtered_extrema: Vec<(usize, ExtremaKind, f32)> = Vec::new();
    for cand in candidates {
        if let Some(last) = filtered_extrema.last_mut() {
            if last.1 == cand.1 {
                // Same type consecutive extrema: keep the more extreme one
                if (cand.1 == ExtremaKind::Peak && cand.2 > last.2)
                    || (cand.1 == ExtremaKind::Valley && cand.2 < last.2)
                {
                    *last = cand;
                }
            } else {
                // Alternating extrema: check if amplitude delta is significant
                let delta = (cand.2 - last.2).abs();
                if delta >= prominence_threshold {
                    filtered_extrema.push(cand);
                }
            }
        } else {
            filtered_extrema.push(cand);
        }
    }

    // Pass 2: Ensure strictly alternating extrema
    let mut final_extrema: Vec<(usize, ExtremaKind, f32)> = Vec::new();
    for item in filtered_extrema {
        if let Some(last) = final_extrema.last_mut() {
            if last.1 == item.1 {
                if (item.1 == ExtremaKind::Peak && item.2 > last.2)
                    || (item.1 == ExtremaKind::Valley && item.2 < last.2)
                {
                    *last = item;
                }
            } else {
                final_extrema.push(item);
            }
        } else {
            final_extrema.push(item);
        }
    }

    if final_extrema.is_empty() {
        return vec![
            Action { at: timestamps_ms[0], pos: 0 },
            Action { at: *timestamps_ms.last().unwrap(), pos: 100 },
        ];
    }

    // 4. Compute major stroke span for equalized amplitude scaling
    let mut stroke_spans = Vec::new();
    for i in 1..final_extrema.len() {
        stroke_spans.push((final_extrema[i].2 - final_extrema[i - 1].2).abs());
    }
    // O(N) partial sort to find 75th percentile — replaces O(N log N) full sort
    let median_stroke_span = if !stroke_spans.is_empty() {
        let idx_75 = stroke_spans.len() * 3 / 4;
        stroke_spans.select_nth_unstable_by(idx_75, |a, b| a.partial_cmp(b).unwrap());
        stroke_spans[idx_75].max(1e-4)
    } else {
        global_span
    };

    let mut actions = Vec::with_capacity(final_extrema.len() + 2);

    // Initial boundary
    let first_t = timestamps_ms[0];
    let first_ex = final_extrema[0];
    if timestamps_ms[first_ex.0] > first_t {
        let initial_pos = if first_ex.1 == ExtremaKind::Peak { 0 } else { 100 };
        actions.push(Action { at: first_t, pos: initial_pos });
    }

    for i in 0..final_extrema.len() {
        let (idx, kind, val) = final_extrema[i];
        let at = timestamps_ms[idx];

        let pos = if full_range_equalize {
            let neighbor_diff = if i > 0 {
                (val - final_extrema[i - 1].2).abs()
            } else if i + 1 < final_extrema.len() {
                (val - final_extrema[i + 1].2).abs()
            } else {
                median_stroke_span
            };

            let stroke_ratio = (neighbor_diff / median_stroke_span).clamp(0.0, 1.0);

            if stroke_ratio > 0.55 {
                // Full major stroke: hits physical limits
                if kind == ExtremaKind::Peak { 100 } else { 0 }
            } else {
                // Partial / vibratory stroke: scale proportionally around 50
                let half_amp = (stroke_ratio * 38.0).round() as i32;
                if kind == ExtremaKind::Peak {
                    (50 + half_amp).min(100)
                } else {
                    (50 - half_amp).max(0)
                }
            }
        } else {
            let norm = (val - min_val) / global_span * 100.0;
            norm.round().clamp(0.0, 100.0) as i32
        };

        if let Some(last) = actions.last_mut() {
            if last.at == at {
                last.pos = pos;
                continue;
            }
        }
        actions.push(Action { at, pos });
    }

    // Trailing boundary
    let last_t = *timestamps_ms.last().unwrap();
    if let Some(last_act) = actions.last() {
        if last_act.at < last_t {
            let last_pos = if last_act.pos > 50 { 0 } else { 100 };
            actions.push(Action { at: last_t, pos: last_pos });
        }
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
            assert!((0.0..=100.0).contains(&val));
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

    #[test]
    fn test_normalization_benchmark_50k_samples() {
        let n = 50_000;
        let mut signal = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / 30.0;
            signal.push((t * 0.5).sin() * 20.0 + (t * 3.0).cos() * 5.0);
        }

        let t0 = std::time::Instant::now();
        let norm = detrend_and_normalize(&signal, 30.0, 2.0, 3.0, 1000.0);
        let elapsed = t0.elapsed();

        assert_eq!(norm.len(), n);
        assert!(elapsed.as_millis() < 500, "50k sample normalization took too long: {:?}", elapsed);
    }

    #[test]
    fn test_extract_actions_full_range_equalization() {
        // Sine wave oscillating between -10 and 10
        let mut signal = Vec::new();
        let mut timestamps = Vec::new();
        for i in 0..100 {
            let t = i as f32 / 10.0;
            signal.push((t * std::f32::consts::PI).sin() * 10.0);
            timestamps.push(i as i64 * 100);
        }

        let actions = extract_actions_full_range(&signal, &timestamps, 5.0, true);
        assert!(!actions.is_empty());

        let min_p = actions.iter().map(|a| a.pos).min().unwrap();
        let max_p = actions.iter().map(|a| a.pos).max().unwrap();
        assert_eq!(min_p, 0, "Expected full range minimum of 0");
        assert_eq!(max_p, 100, "Expected full range maximum of 100");
    }
}
