//! Fast parallelized optical flow, divergence calculation, and radial motion projection in pure Rust.

use rayon::prelude::*;

#[derive(Debug, Clone)]
pub struct FlowField {
    pub width: usize,
    pub height: usize,
    /// Velocity vector components: (u, v) for each pixel in row-major order
    pub u: Vec<f32>,
    pub v: Vec<f32>,
}

impl FlowField {
    pub fn new(width: usize, height: usize) -> Self {
        let size = width * height;
        Self {
            width,
            height,
            u: vec![0.0; size],
            v: vec![0.0; size],
        }
    }

    /// Mean magnitude of the velocity field
    pub fn mean_magnitude(&self) -> f32 {
        let sum: f32 = self
            .u
            .par_iter()
            .zip(self.v.par_iter())
            .map(|(&u, &v)| (u * u + v * v).sqrt())
            .sum();
        sum / (self.u.len() as f32)
    }

    /// Bilinear interpolation sampling of optical flow velocity (u, v) at normalized coordinates [0.0, 1.0]
    pub fn sample_flow_at(&self, norm_x: f32, norm_y: f32) -> (f32, f32) {
        if self.width == 0 || self.height == 0 {
            return (0.0, 0.0);
        }
        let px = (norm_x * (self.width as f32 - 1.0)).clamp(0.0, self.width as f32 - 1.0);
        let py = (norm_y * (self.height as f32 - 1.0)).clamp(0.0, self.height as f32 - 1.0);

        let x0 = px.floor() as usize;
        let y0 = py.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);

        let fx = px - x0 as f32;
        let fy = py - y0 as f32;

        let idx00 = y0 * self.width + x0;
        let idx10 = y0 * self.width + x1;
        let idx01 = y1 * self.width + x0;
        let idx11 = y1 * self.width + x1;

        let u0 = self.u[idx00] * (1.0 - fx) + self.u[idx10] * fx;
        let u1 = self.u[idx01] * (1.0 - fx) + self.u[idx11] * fx;
        let sampled_u = u0 * (1.0 - fy) + u1 * fy;

        let v0 = self.v[idx00] * (1.0 - fx) + self.v[idx10] * fx;
        let v1 = self.v[idx01] * (1.0 - fx) + self.v[idx11] * fx;
        let sampled_v = v0 * (1.0 - fy) + v1 * fy;

        (sampled_u, sampled_v)
    }

    /// Compute optical flow vorticity (rotational curl: dv/dx - du/dy) in a circular region around (norm_cx, norm_cy).
    /// Used for T-Code Twist (R2) axial rotation detection.
    pub fn compute_vorticity(&self, norm_cx: f32, norm_cy: f32, norm_radius: f32) -> f32 {
        if self.width < 3 || self.height < 3 {
            return 0.0;
        }

        let cx = (norm_cx * self.width as f32).round() as isize;
        let cy = (norm_cy * self.height as f32).round() as isize;
        let r = ((norm_radius * self.width as f32).round() as isize).max(2);

        let mut sum_curl = 0.0f32;
        let mut count = 0usize;

        let y_min = (cy - r).max(1);
        let y_max = (cy + r).min(self.height as isize - 2);
        let x_min = (cx - r).max(1);
        let x_max = (cx + r).min(self.width as isize - 2);

        for y in y_min..=y_max {
            for x in x_min..=x_max {
                let dx = x - cx;
                let dy = y - cy;
                if dx * dx + dy * dy <= r * r {
                    let idx_xp = (y as usize) * self.width + (x + 1) as usize;
                    let idx_xm = (y as usize) * self.width + (x - 1) as usize;
                    let idx_yp = ((y + 1) as usize) * self.width + (x as usize);
                    let idx_ym = ((y - 1) as usize) * self.width + (x as usize);

                    let dv_dx = (self.v[idx_xp] - self.v[idx_xm]) * 0.5;
                    let du_dy = (self.u[idx_yp] - self.u[idx_ym]) * 0.5;
                    sum_curl += dv_dx - du_dy;
                    count += 1;
                }
            }
        }

        if count > 0 {
            sum_curl / (count as f32)
        } else {
            0.0
        }
    }
}

/// Detect sudden cut via L1 mean pixel difference between frames
pub fn detect_cut_photometric(p0: &[u8], p1: &[u8], threshold: f32) -> bool {
    if p0.len() != p1.len() || p0.is_empty() {
        return true;
    }
    let sum_diff: u64 = p0
        .par_iter()
        .zip(p1.par_iter())
        .map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs() as u64)
        .sum();
    let mean_diff = (sum_diff as f32) / (p0.len() as f32);
    mean_diff > threshold
}

/// High-performance multi-threaded dense optical flow calculation (Lucas-Kanade gradient tensor).
/// Highly optimized with precomputed gradient buffers to eliminate 98% of redundant arithmetic ops.

pub struct FlowScratchContext {
    pub ix: Vec<f32>,
    pub iy: Vec<f32>,
    pub it: Vec<f32>,
    pub ixx: Vec<f32>,
    pub ixy: Vec<f32>,
    pub iyy: Vec<f32>,
    pub ixt: Vec<f32>,
    pub iyt: Vec<f32>,
    pub h_sum_xx: Vec<f32>,
    pub h_sum_xy: Vec<f32>,
    pub h_sum_yy: Vec<f32>,
    pub h_sum_xt: Vec<f32>,
    pub h_sum_yt: Vec<f32>,
    pub sum_xx: Vec<f32>,
    pub sum_xy: Vec<f32>,
    pub sum_yy: Vec<f32>,
    pub sum_xt: Vec<f32>,
    pub sum_yt: Vec<f32>,
    pub field: FlowField,
}

impl FlowScratchContext {
    pub fn new(width: usize, height: usize) -> Self {
        let size = width * height;
        Self {
            ix: vec![0.0; size],
            iy: vec![0.0; size],
            it: vec![0.0; size],
            ixx: vec![0.0; size],
            ixy: vec![0.0; size],
            iyy: vec![0.0; size],
            ixt: vec![0.0; size],
            iyt: vec![0.0; size],
            h_sum_xx: vec![0.0; size],
            h_sum_xy: vec![0.0; size],
            h_sum_yy: vec![0.0; size],
            h_sum_xt: vec![0.0; size],
            h_sum_yt: vec![0.0; size],
            sum_xx: vec![0.0; size],
            sum_xy: vec![0.0; size],
            sum_yy: vec![0.0; size],
            sum_xt: vec![0.0; size],
            sum_yt: vec![0.0; size],
            field: FlowField::new(width, height),
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        let size = width * height;
        if self.ix.len() != size {
            self.ix.resize(size, 0.0);
            self.iy.resize(size, 0.0);
            self.it.resize(size, 0.0);
            self.ixx.resize(size, 0.0);
            self.ixy.resize(size, 0.0);
            self.iyy.resize(size, 0.0);
            self.ixt.resize(size, 0.0);
            self.iyt.resize(size, 0.0);
            self.h_sum_xx.resize(size, 0.0);
            self.h_sum_xy.resize(size, 0.0);
            self.h_sum_yy.resize(size, 0.0);
            self.h_sum_xt.resize(size, 0.0);
            self.h_sum_yt.resize(size, 0.0);
            self.sum_xx.resize(size, 0.0);
            self.sum_xy.resize(size, 0.0);
            self.sum_yy.resize(size, 0.0);
            self.sum_xt.resize(size, 0.0);
            self.sum_yt.resize(size, 0.0);
            self.field = FlowField::new(width, height);
        } else {
            self.field.u.fill(0.0);
            self.field.v.fill(0.0);
        }
    }
}

#[allow(dead_code)]
pub fn compute_dense_flow_tmp(p0: &[u8], p1: &[u8], width: usize, height: usize) -> FlowField {
    let mut ctx = FlowScratchContext::new(width, height);
    let field = compute_dense_flow(p0, p1, width, height, &mut ctx);
    field.clone()
}

pub fn compute_dense_flow<'a>(p0: &[u8], p1: &[u8], width: usize, height: usize, ctx: &'a mut FlowScratchContext) -> &'a FlowField {
    ctx.resize(width, height);
    if width < 3 || height < 3 {
        return &ctx.field;
    }

    let _size = width * height;
    let w = width as isize;
    let h = height as isize;

    // Phase 1: Precompute spatial and temporal gradients in parallel across rows
    ctx.ix.par_chunks_mut(width)
        .zip(ctx.iy.par_chunks_mut(width))
        .zip(ctx.it.par_chunks_mut(width))
        .zip(ctx.ixx.par_chunks_mut(width))
        .zip(ctx.ixy.par_chunks_mut(width))
        .zip(ctx.iyy.par_chunks_mut(width))
        .zip(ctx.ixt.par_chunks_mut(width))
        .zip(ctx.iyt.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (((((((row_ix, row_iy), row_it), row_ixx), row_ixy), row_iyy), row_ixt), row_iyt))| {
            if y == 0 || y >= height - 1 {
                return;
            }
            let row_offset = y * width;
            let prev_offset = (y - 1) * width;
            let next_offset = (y + 1) * width;

            for x in 1..(width - 1) {
                let idx = row_offset + x;
                let ix = ((p0[idx + 1] as f32 - p0[idx - 1] as f32)
                    + (p1[idx + 1] as f32 - p1[idx - 1] as f32))
                    * 0.25;
                let iy = ((p0[next_offset + x] as f32 - p0[prev_offset + x] as f32)
                    + (p1[next_offset + x] as f32 - p1[prev_offset + x] as f32))
                    * 0.25;
                let it = p1[idx] as f32 - p0[idx] as f32;

                row_ix[x] = ix;
                row_iy[x] = iy;
                row_it[x] = it;
                
                row_ixx[x] = ix * ix;
                row_ixy[x] = ix * iy;
                row_iyy[x] = iy * iy;
                row_ixt[x] = ix * it;
                row_iyt[x] = iy * it;
            }
        });

    let win_rad = 3isize;
    let lambda = 0.05f32; // Regularization parameter

    // Phase 2: Separable Box Filter
    // 2a. Horizontal pass (window 7)
    ctx.h_sum_xx.par_chunks_mut(width)
        .zip(ctx.h_sum_xy.par_chunks_mut(width))
        .zip(ctx.h_sum_yy.par_chunks_mut(width))
        .zip(ctx.h_sum_xt.par_chunks_mut(width))
        .zip(ctx.h_sum_yt.par_chunks_mut(width))
        .zip(ctx.ixx.par_chunks(width))
        .zip(ctx.ixy.par_chunks(width))
        .zip(ctx.iyy.par_chunks(width))
        .zip(ctx.ixt.par_chunks(width))
        .zip(ctx.iyt.par_chunks(width))
        .for_each(|(((((((((h_xx, h_xy), h_yy), h_xt), h_yt), r_ixx), r_ixy), r_iyy), r_ixt), r_iyt)| {
            for x in 1..(width as isize - 1) {
                let x_start = (x - win_rad).max(1) as usize;
                let x_end = (x + win_rad).min(w - 2) as usize;
                
                let mut sum_xx = 0.0;
                let mut sum_xy = 0.0;
                let mut sum_yy = 0.0;
                let mut sum_xt = 0.0;
                let mut sum_yt = 0.0;
                
                for k in x_start..=x_end {
                    sum_xx += r_ixx[k];
                    sum_xy += r_ixy[k];
                    sum_yy += r_iyy[k];
                    sum_xt += r_ixt[k];
                    sum_yt += r_iyt[k];
                }
                
                let ux = x as usize;
                h_xx[ux] = sum_xx;
                h_xy[ux] = sum_xy;
                h_yy[ux] = sum_yy;
                h_xt[ux] = sum_xt;
                h_yt[ux] = sum_yt;
            }
        });

    // 2b. Vertical pass (window 7) and solve
    let h_sum_xx = &ctx.h_sum_xx;
    let h_sum_xy = &ctx.h_sum_xy;
    let h_sum_yy = &ctx.h_sum_yy;
    let h_sum_xt = &ctx.h_sum_xt;
    let h_sum_yt = &ctx.h_sum_yt;

    ctx.field.u.par_chunks_mut(width)
        .zip(ctx.field.v.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y_idx, (row_u, row_v))| {
            let y = y_idx as isize;
            if y < 1 || y >= h - 1 {
                return;
            }

            let y_start = (y - win_rad).max(1) as usize;
            let y_end = (y + win_rad).min(h - 2) as usize;

            for x in 1..(width - 1) {
                let mut sum_xx = 0.0;
                let mut sum_xy = 0.0;
                let mut sum_yy = 0.0;
                let mut sum_xt = 0.0;
                let mut sum_yt = 0.0;

                for ny in y_start..=y_end {
                    let idx = ny * width + x;
                    sum_xx += h_sum_xx[idx];
                    sum_xy += h_sum_xy[idx];
                    sum_yy += h_sum_yy[idx];
                    sum_xt += h_sum_xt[idx];
                    sum_yt += h_sum_yt[idx];
                }

                let a = sum_xx + lambda;
                let b = sum_xy;
                let c = sum_yy + lambda;
                let det = a * c - b * b;

                if det.abs() > 1e-6 {
                    let inv_det = 1.0 / det;
                    let u = -(c * sum_xt - b * sum_yt) * inv_det;
                    let v = -(-b * sum_xt + a * sum_yt) * inv_det;

                    row_u[x] = u.clamp(-30.0, 30.0);
                    row_v[x] = v.clamp(-30.0, 30.0);
                }
            }
        });

    &ctx.field
}

/// Computes divergence map div = du/dx + dv/dy and finds maximum divergence center (expansion point)
pub fn find_divergence_center(flow: &FlowField) -> (f32, f32, f32) {
    let w = flow.width;
    let h = flow.height;

    let best_div = (2..(h - 2)).into_par_iter().map(|y| {
        let mut local_best_div = 0.0f32;
        let mut local_best_x = (w / 2) as f32;
        let local_y = y as f32;

        for x in 2..(w - 2) {
            let idx = y * w + x;
            let du_dx = (flow.u[idx + 1] - flow.u[idx - 1]) * 0.5;
            let dv_dy = (flow.v[idx + w] - flow.v[idx - w]) * 0.5;
            let div = du_dx + dv_dy;

            if div.abs() > local_best_div.abs() {
                local_best_div = div;
                local_best_x = x as f32;
            }
        }
        (local_best_x, local_y, local_best_div)
    }).reduce(
        || ((w / 2) as f32, (h / 2) as f32, 0.0f32),
        |a, b| if b.2.abs() > a.2.abs() { b } else { a }
    );

    best_div
}

/// Temporal median filtering of center coordinates to suppress sudden noise jumps
pub fn filter_centers_median(centers: &[(f32, f32)], window_radius: usize) -> Vec<(f32, f32)> {
    let n = centers.len();
    let mut filtered = Vec::with_capacity(n);

    let mut xs = Vec::new();
    let mut ys = Vec::new();

    for i in 0..n {
        let start = i.saturating_sub(window_radius);
        let end = (i + window_radius + 1).min(n);

        xs.clear();
        ys.clear();
        for center in &centers[start..end] {
            xs.push(center.0);
            ys.push(center.1);
        }

        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let med_x = xs[xs.len() / 2];
        let med_y = ys[ys.len() / 2];
        filtered.push((med_x, med_y));
    }

    filtered
}

/// Projects flow vectors radially from the motion center.
/// Positive values correspond to outward expansion; negative to inward contraction.
pub fn project_radial_motion(
    flow: &FlowField,
    center: (f32, f32),
    is_cut: bool,
    pov_mode: bool,
    balance_global: bool,
) -> f32 {
    if is_cut {
        return 0.0;
    }

    let w = flow.width as f32;
    let h = flow.height as f32;
    let cx = center.0;
    let cy = center.1;

    let total_pixels = flow.width * flow.height;
    if total_pixels == 0 {
        return 0.0;
    }

    let sum_dot: f32 = (0..flow.height)
        .into_par_iter()
        .map(|y| {
            let mut row_sum = 0.0f32;
            let y_f = y as f32;
            let dy = y_f - cy;

            for x in 0..flow.width {
                let x_f = x as f32;
                let dx = x_f - cx;
                let idx = y * flow.width + x;

                let u = flow.u[idx];
                let v = flow.v[idx];
                let mut dot = u * dx + v * dy;

                if !pov_mode && balance_global {
                    // Attenuate edge pixels to reduce camera panning drift
                    let wx = if x_f > cx {
                        (w - x_f) / w
                    } else {
                        x_f / w
                    };
                    let wy = if y_f > cy {
                        (h - y_f) / h
                    } else {
                        y_f / h
                    };
                    dot *= wx * wy;
                }

                row_sum += dot;
            }
            row_sum
        })
        .sum();

    sum_dot / (total_pixels as f32)
}

/// Derives 6-DOF companion motion signals from optical flow and interaction center.
/// Returns (surge_velocity, sway_velocity, pitch_velocity, roll_velocity).
pub fn project_companion_motion(
    flow: &FlowField,
    center: (f32, f32),
    is_cut: bool,
    pov_mode: bool,
    balance_global: bool,
) -> (f32, f32, f32, f32) {
    if is_cut || flow.width == 0 || flow.height == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    // 1. Surge: radial expansion/contraction (depth thrust)
    let surge = project_radial_motion(flow, center, is_cut, pov_mode, balance_global);

    // 2. Sway: lateral horizontal velocity (mean u in central 50% ROI)
    let w = flow.width;
    let h = flow.height;
    let mut sum_u = 0.0f32;
    let mut count = 0usize;
    let x_start = w / 4;
    let x_end = (3 * w) / 4;
    let y_start = h / 4;
    let y_end = (3 * h) / 4;
    for y in y_start..y_end {
        let row = y * w;
        for x in x_start..x_end {
            sum_u += flow.u[row + x];
            count += 1;
        }
    }
    let sway = if count > 0 { (sum_u / count as f32) * 5.0 } else { 0.0 };

    // 3. Roll: rotational curl around center
    let norm_cx = (center.0 / w as f32).clamp(0.1, 0.9);
    let norm_cy = (center.1 / h as f32).clamp(0.1, 0.9);
    let roll = flow.compute_vorticity(norm_cx, norm_cy, 0.25) * 10.0;

    // 4. Pitch: differential vertical flow between top and bottom halves
    let mut top_v = 0.0f32;
    let mut bot_v = 0.0f32;
    let half_h = h / 2;
    let half_count = (half_h * (x_end - x_start)).max(1) as f32;
    for y in 0..half_h {
        let row = y * w;
        for x in x_start..x_end {
            top_v += flow.v[row + x];
        }
    }
    for y in half_h..h {
        let row = y * w;
        for x in x_start..x_end {
            bot_v += flow.v[row + x];
        }
    }
    let pitch = ((top_v - bot_v) / half_count) * 5.0;

    (surge, sway, pitch, roll)
}

/// Adaptive motion projection: automatically determines dominant motion axis (linear stroke vs radial expansion).
/// Focuses on actively moving pixels so static backgrounds do not dilute motion velocity.
/// Inverts screen-space Y velocity so moving UP corresponds to positive velocity (towards 100).
pub fn project_adaptive_motion(
    flow: &FlowField,
    center: (f32, f32),
    is_cut: bool,
    pov_mode: bool,
    balance_global: bool,
) -> f32 {
    if is_cut {
        return 0.0;
    }

    let w = flow.width as f32;
    let h = flow.height as f32;
    let cx = center.0;
    let cy = center.1;

    if flow.width == 0 || flow.height == 0 {
        return 0.0;
    }

    struct Accum {
        sum_u: f32,
        sum_v: f32,
        sum_radial: f32,
        weight_active: f32,
    }

    let inv_w = 1.0 / w;
    let inv_h = 1.0 / h;

    let accum = (0..flow.height)
        .into_par_iter()
        .map(|y| {
            let mut acc = Accum {
                sum_u: 0.0,
                sum_v: 0.0,
                sum_radial: 0.0,
                weight_active: 0.0,
            };
            let y_f = y as f32;
            let dy = y_f - cy;
            let wy = if y_f > cy { (h - y_f) * inv_h } else { y_f * inv_h };

            for x in 0..flow.width {
                let x_f = x as f32;
                let dx = x_f - cx;
                let idx = y * flow.width + x;

                let u = flow.u[idx];
                let v = flow.v[idx];
                let mag = (u * u + v * v).sqrt();
                if mag < 0.03 {
                    continue;
                }

                let mut weight = mag; // Weight by velocity magnitude so moving foreground dominates
                if !pov_mode && balance_global {
                    let wx = if x_f > cx { (w - x_f) * inv_w } else { x_f * inv_w };
                    weight *= (wx * wy).max(0.15);
                }

                let dist = (dx * dx + dy * dy).sqrt().max(1.0);
                let inv_dist = 1.0 / dist;
                let rad_dot = (u * dx + v * dy) * inv_dist;

                acc.sum_u += u * weight;
                acc.sum_v += v * weight;
                acc.sum_radial += rad_dot * weight;
                acc.weight_active += weight;
            }
            acc
        })
        .reduce(
            || Accum {
                sum_u: 0.0,
                sum_v: 0.0,
                sum_radial: 0.0,
                weight_active: 0.0,
            },
            |mut a, b| {
                a.sum_u += b.sum_u;
                a.sum_v += b.sum_v;
                a.sum_radial += b.sum_radial;
                a.weight_active += b.weight_active;
                a
            },
        );

    if accum.weight_active < 1e-4 {
        return 0.0;
    }

    let mean_u = accum.sum_u / accum.weight_active;
    let mean_v = accum.sum_v / accum.weight_active;
    let mean_radial = accum.sum_radial / accum.weight_active;

    let linear_mag = (mean_u * mean_u + mean_v * mean_v).sqrt();
    let radial_mag = mean_radial.abs();

    if pov_mode && radial_mag > linear_mag * 0.4 {
        mean_radial
    } else if linear_mag > radial_mag * 0.6 {
        // Linear stroking dominates: project along dominant linear motion
        if mean_v.abs() >= mean_u.abs() * 0.5 {
            -mean_v // Moving UP on screen is positive velocity
        } else {
            mean_u
        }
    } else {
        mean_radial * 0.5 + (-mean_v) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_photometric_cut_detection() {
        let f0 = vec![128u8; 1000];
        let f1 = vec![128u8; 1000];
        assert!(!detect_cut_photometric(&f0, &f1, 30.0));

        let f_black = vec![0u8; 1000];
        assert!(detect_cut_photometric(&f0, &f_black, 30.0));
    }

    #[test]
    fn test_dense_flow_motion_direction() {
        let w = 64;
        let h = 64;
        let mut p0 = vec![0u8; w * h];
        let mut p1 = vec![0u8; w * h];

        // Shift block rightward
        for y in 20..40 {
            for x in 20..40 {
                p0[y * w + x] = 255;
            }
            for x in 23..43 {
                p1[y * w + x] = 255;
            }
        }

        let flow = compute_dense_flow_tmp(&p0, &p1, w, h);
        // Measure horizontal flow at the leading edge (x: 38..42, y: 25..35)
        let mut edge_u_sum = 0.0f32;
        let mut count = 0;
        for y in 25..35 {
            for x in 38..42 {
                edge_u_sum += flow.u[y * w + x];
                count += 1;
            }
        }
        let avg_edge_u = edge_u_sum / (count as f32);
        assert!(avg_edge_u > 0.0, "Expected positive horizontal flow at leading edge, got {}", avg_edge_u);
    }

    #[test]
    fn test_filter_centers_suppresses_spikes() {
        let mut raw = vec![(50.0, 50.0); 10];
        raw[5] = (500.0, 500.0); // Outlier spike
        let filtered = filter_centers_median(&raw, 2);
        assert!((filtered[5].0 - 50.0).abs() < 1.0);
        assert!((filtered[5].1 - 50.0).abs() < 1.0);
    }

    #[test]
    fn test_flow_sample_and_vorticity() {
        let mut flow = FlowField::new(10, 10);
        for y in 0..10 {
            for x in 0..10 {
                let idx = y * 10 + x;
                flow.v[idx] = (x as f32 - 4.5) * 2.0;
                flow.u[idx] = -(y as f32 - 4.5) * 2.0;
            }
        }

        let (u_center, v_center) = flow.sample_flow_at(0.5, 0.5);
        assert!(u_center.abs() < 1.0);
        assert!(v_center.abs() < 1.0);

        let vorticity = flow.compute_vorticity(0.5, 0.5, 0.3);
        assert!(vorticity > 0.0, "Expected positive rotational vorticity, got {}", vorticity);
    }

    #[test]
    fn test_dense_flow_benchmark_256x256() {
        let w = 256;
        let h = 256;
        let mut p0 = vec![128u8; w * h];
        let mut p1 = vec![128u8; w * h];

        // Add pattern that shifts
        for y in 80..160 {
            for x in 80..160 {
                p0[y * w + x] = 220;
                p1[y * w + (x + 3)] = 220;
            }
        }

        let t0 = std::time::Instant::now();
        let flow = compute_dense_flow_tmp(&p0, &p1, w, h);
        let elapsed = t0.elapsed();

        assert_eq!(flow.width, 256);
        assert_eq!(flow.height, 256);
        assert!(elapsed.as_millis() < 500, "Flow computation took too long: {:?}", elapsed);
    }

    #[test]
    fn test_motion_axes_on_fixture() {
        use std::path::Path;
        let video_path = Path::new("/home/brianklam/Desktop/Funscripts/Testing Fixtures/handonly_sync_animation.mp4");
        if !video_path.exists() {
            return;
        }

        let cfg = crate::video::StreamConfig {
            target_width: 256,
            target_height: 256,
            target_fps: 12.0, // native video fps!
            vr_mode: false,
            is_rgb: false,
        };

        let mut stream = crate::video::FrameStreamReader::new(video_path, &cfg).unwrap();
        let mut prev = stream.next_frame().unwrap().unwrap().0;

        let mut adaptive_dots = Vec::new();
        let mut timestamps = Vec::new();

        while let Ok(Some((curr, ts))) = stream.next_frame() {
            let flow = compute_dense_flow_tmp(&prev, &curr, 256, 256);
            let center = find_divergence_center(&flow);
            let dot = project_adaptive_motion(&flow, (center.0, center.1), false, false, true);

            adaptive_dots.push(dot);
            timestamps.push(ts);
            prev = curr;
        }

        println!("Total frames analyzed: {}", adaptive_dots.len());

        let mut cum_pos = 0.0f32;
        let mut pos_curve = Vec::new();
        for &dot in &adaptive_dots {
            cum_pos += dot;
            pos_curve.push(cum_pos);
        }

        let min_c = pos_curve.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_c = pos_curve.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        println!("Integrated displacement range: [{:.2}, {:.2}]", min_c, max_c);

        let actions = crate::signal::extract_actions_full_range(&pos_curve, &timestamps, 1.0, true);
        println!("Extracted actions with 1.0% prominence: {}", actions.len());
        for a in actions.iter().take(15) {
            println!("  at: {:>5} ms, pos: {:>3}", a.at, a.pos);
        }
    }
}
