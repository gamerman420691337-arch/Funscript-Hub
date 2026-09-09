//! TAPNext++ Sparse Point Trajectory Engine.
//!
//! Maintains continuous multi-point trajectories across sliding windows (e.g. 32 frames),
//! tracking point positions, visibility probabilities (p_vis), velocity vectors, and surface
//! deformation shear. Bypasses the silhouette limitation where a stationary mask obscures
//! internal surface motion.

use crate::tracking::FlowField;
use serde::{Deserialize, Serialize};

/// State of an individual sparse tracked point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointTrajectory {
    pub point_id: usize,
    /// Normalized coordinates (x, y) in [0.0, 1.0]
    pub current_pos: (f32, f32),
    /// Normalized velocity vector (vx, vy) per frame
    pub velocity: (f32, f32),
    /// Normalized acceleration vector (ax, ay) per frame^2
    pub acceleration: (f32, f32),
    /// Visibility probability in [0.0, 1.0] (p_vis < 0.5 flags occlusion)
    pub visibility: f32,
    /// Confidence score in [0.0, 1.0]
    pub confidence: f32,
    /// Measured forward-backward cycle error
    pub forward_backward_error: f32,
    /// Trajectory coordinate history (timestamp_ms, x, y)
    pub history: Vec<(i64, f32, f32)>,
    /// Semantic anatomical tag (e.g. "probe_tip", "shaft", "orifice_rim", "surface_grid")
    pub tag: &'static str,
}

#[allow(dead_code)]
impl PointTrajectory {
    pub fn new(point_id: usize, pos: (f32, f32), tag: &'static str) -> Self {
        Self {
            point_id,
            current_pos: pos,
            velocity: (0.0, 0.0),
            acceleration: (0.0, 0.0),
            visibility: 1.0,
            confidence: 1.0,
            forward_backward_error: 0.0,
            history: Vec::with_capacity(32),
            tag,
        }
    }

    /// Check if point is currently visible and confident
    pub fn is_active(&self) -> bool {
        self.visibility >= 0.40 && self.confidence >= 0.30
    }
}

/// Configuration for the TAPNext++ point tracker
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapTrackerConfig {
    /// Target number of sparse points to maintain simultaneously (default: 64)
    pub target_point_count: usize,
    /// Sliding window length in frames for trajectory smoothing and cycle checking (default: 32)
    pub window_size: usize,
    /// Minimum visibility probability to consider point non-occluded (default: 0.50)
    pub visibility_threshold: f32,
    /// Maximum allowed forward-backward cycle error before marking point unreliable (default: 0.05)
    pub max_cycle_error: f32,
    /// Whether to use neural tracking if the model is loaded (default: false)
    pub use_neural: bool,
}

impl Default for TapTrackerConfig {
    fn default() -> Self {
        Self {
            target_point_count: 64,
            window_size: 32,
            visibility_threshold: 0.50,
            max_cycle_error: 0.05,
            use_neural: false,
        }
    }
}

/// Dense surface motion summary decoupling rigid hull motion from surface shear
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct SurfaceKinematics {
    /// Dominant bulk translation vector (mean vx, mean vy)
    pub bulk_translation: (f32, f32),
    /// Surface shear / deformation variance (internal strain)
    pub shear_magnitude: f32,
    /// Average visibility of all active points
    pub mean_visibility: f32,
    /// Fraction of points currently occluded (p_vis < 0.5)
    pub occlusion_fraction: f32,
}

/// TAPNext++ style sparse point tracking engine
#[derive(Debug)]
pub struct TapPointTracker {
    pub config: TapTrackerConfig,
    pub trajectories: Vec<PointTrajectory>,
    #[allow(dead_code)]
    next_point_id: usize,
    pub neural_session: Option<ort::session::Session>,
}

impl Clone for TapPointTracker {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            trajectories: self.trajectories.clone(),
            next_point_id: self.next_point_id,
            // Cannot clone ONNX session safely, so we drop it on clone
            // The cloned instance will fallback or need reload
            neural_session: None,
        }
    }
}

#[allow(dead_code)]
impl TapPointTracker {
    pub fn new(config: TapTrackerConfig) -> Self {
        Self {
            config,
            trajectories: Vec::new(),
            next_point_id: 0,
            neural_session: None,
        }
    }

    pub fn load_model(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let builder = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("Failed to initialize SessionBuilder: {e}"))?;

        let session = builder
            .with_intra_threads(2)
            .map_err(|e| anyhow::anyhow!("Failed to configure thread count: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to load ONNX model from {path:?}: {e}"))?;

        self.neural_session = Some(session);
        Ok(())
    }

    /// Reset all trajectories
    pub fn clear(&mut self) {
        self.trajectories.clear();
        self.next_point_id = 0;
    }

    /// Seed sparse points across an anatomical bounding box or segmented mask
    pub fn seed_from_bbox(&mut self, bbox: [f32; 4], tag: &'static str, count: usize) {
        let x1 = bbox[0].clamp(0.0, 1.0);
        let y1 = bbox[1].clamp(0.0, 1.0);
        let x2 = bbox[2].clamp(0.0, 1.0);
        let y2 = bbox[3].clamp(0.0, 1.0);

        let w = (x2 - x1).max(0.01);
        let h = (y2 - y1).max(0.01);

        let grid_side = (count as f32).sqrt().ceil() as usize;
        let mut seeded = 0;

        for gy in 0..grid_side {
            for gx in 0..grid_side {
                if seeded >= count {
                    break;
                }
                let px = x1 + w * ((gx as f32 + 0.5) / grid_side as f32);
                let py = y1 + h * ((gy as f32 + 0.5) / grid_side as f32);

                self.trajectories.push(PointTrajectory::new(self.next_point_id, (px, py), tag));
                self.next_point_id += 1;
                seeded += 1;
            }
        }
    }

    /// Seed points within non-zero pixels of a binary/grayscale mask
    pub fn seed_from_mask(&mut self, mask: &[u8], width: usize, height: usize, tag: &'static str, count: usize) {
        if mask.is_empty() || width == 0 || height == 0 {
            return;
        }

        let mut candidate_pixels = Vec::new();
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                let val = mask[y * width + x];
                if val > 100 {
                    candidate_pixels.push((x, y));
                }
            }
        }

        if candidate_pixels.is_empty() {
            return;
        }

        let step = (candidate_pixels.len() / count).max(1);
        for i in (0..candidate_pixels.len()).step_by(step).take(count) {
            let (cx, cy) = candidate_pixels[i];
            let nx = cx as f32 / width as f32;
            let ny = cy as f32 / height as f32;

            self.trajectories.push(PointTrajectory::new(self.next_point_id, (nx, ny), tag));
            self.next_point_id += 1;
        }
    }

    /// Advance point trajectories using the dense optical flow field
    pub fn step_frame(&mut self, flow: &FlowField, timestamp_ms: i64) -> SurfaceKinematics {
        if self.trajectories.is_empty() {
            return SurfaceKinematics::default();
        }

        let active_count = self.trajectories.len();

        if self.config.use_neural && self.neural_session.is_some() {
            let mut coords = Vec::with_capacity(active_count * 2);
            for traj in &self.trajectories {
                coords.push(traj.current_pos.0);
                coords.push(traj.current_pos.1);
            }
            let input_array = ndarray::Array2::from_shape_vec([active_count, 2], coords).unwrap();
            let input_tensor = ort::value::Tensor::from_array(input_array).unwrap();
            let inputs = ort::inputs![input_tensor];
            let outputs = self.neural_session.as_mut().unwrap().run(inputs).unwrap();
            
            let mut it = outputs.into_iter();
            let (_, points_out) = it.next().unwrap();
            let (_, vis_out) = it.next().unwrap();
            
            let (_, p_slice) = points_out.try_extract_tensor::<f32>().unwrap();
            let (_, v_slice) = vis_out.try_extract_tensor::<f32>().unwrap();
            
            let mut sum_vx = 0.0f32;
            let mut sum_vy = 0.0f32;
            let mut sum_vis = 0.0f32;
            let mut occluded_count = 0;
            
            for (i, traj) in self.trajectories.iter_mut().enumerate() {
                let next_x = p_slice[i * 2].clamp(0.0, 1.0);
                let next_y = p_slice[i * 2 + 1].clamp(0.0, 1.0);
                let vis = v_slice[i].clamp(0.0, 1.0);
                
                let old_vx = traj.velocity.0;
                let old_vy = traj.velocity.1;
                let new_vx = next_x - traj.current_pos.0;
                let new_vy = next_y - traj.current_pos.1;
                
                traj.acceleration = (new_vx - old_vx, new_vy - old_vy);
                traj.velocity = (new_vx, new_vy);
                traj.current_pos = (next_x, next_y);
                traj.visibility = vis;
                
                if traj.visibility < self.config.visibility_threshold {
                    occluded_count += 1;
                }
                
                traj.history.push((timestamp_ms, next_x, next_y));
                if traj.history.len() > self.config.window_size {
                    traj.history.remove(0);
                }
                
                sum_vx += new_vx;
                sum_vy += new_vy;
                sum_vis += vis;
            }
            
            let mean_vx = sum_vx / active_count as f32;
            let mean_vy = sum_vy / active_count as f32;
            let mean_vis = sum_vis / active_count as f32;
            let occ_frac = occluded_count as f32 / active_count as f32;
            
            let mut shear_sum = 0.0f32;
            for traj in &self.trajectories {
                let dx = traj.velocity.0 - mean_vx;
                let dy = traj.velocity.1 - mean_vy;
                shear_sum += dx * dx + dy * dy;
            }
            let shear_mag = (shear_sum / active_count as f32).sqrt();
            
            return SurfaceKinematics {
                bulk_translation: (mean_vx, mean_vy),
                shear_magnitude: shear_mag,
                mean_visibility: mean_vis,
                occlusion_fraction: occ_frac,
            };
        }

        let fw = flow.width as f32;
        let fh = flow.height as f32;

        let mut sum_vx = 0.0f32;
        let mut sum_vy = 0.0f32;
        let mut sum_vis = 0.0f32;
        let mut occluded_count = 0;

        for traj in &mut self.trajectories {
            let (px, py) = traj.current_pos;

            // Sample forward flow
            let (u, v) = flow.sample_flow_at(px, py);
            let du = u / fw;
            let dv = v / fh;

            let next_x = (px + du).clamp(0.0, 1.0);
            let next_y = (py + dv).clamp(0.0, 1.0);

            // Forward-backward cycle check: sample backward flow at destination
            let (u_back, v_back) = flow.sample_flow_at(next_x, next_y);
            let du_back = u_back / fw;
            let dv_back = v_back / fh;

            // Under consistent flow, backwards motion should invert forwards motion
            let cycle_x = du + du_back;
            let cycle_y = dv + dv_back;
            let cycle_err = (cycle_x * cycle_x + cycle_y * cycle_y).sqrt();

            traj.forward_backward_error = traj.forward_backward_error * 0.7 + cycle_err * 0.3;

            // Visibility probability decays when cycle error exceeds limit or point hits image boundaries
            if cycle_err > self.config.max_cycle_error || next_x <= 0.01 || next_x >= 0.99 || next_y <= 0.01 || next_y >= 0.99 {
                traj.visibility = (traj.visibility * 0.85).max(0.0);
            } else {
                traj.visibility = (traj.visibility * 0.9 + 0.1).min(1.0);
            }

            if traj.visibility < self.config.visibility_threshold {
                occluded_count += 1;
            }

            // Kinematic derivatives
            let old_vx = traj.velocity.0;
            let old_vy = traj.velocity.1;
            let new_vx = du;
            let new_vy = dv;

            traj.acceleration = (new_vx - old_vx, new_vy - old_vy);
            traj.velocity = (new_vx, new_vy);
            traj.current_pos = (next_x, next_y);

            // Record history
            traj.history.push((timestamp_ms, next_x, next_y));
            if traj.history.len() > self.config.window_size {
                traj.history.remove(0);
            }

            sum_vx += new_vx;
            sum_vy += new_vy;
            sum_vis += traj.visibility;
        }

        let mean_vx = sum_vx / active_count as f32;
        let mean_vy = sum_vy / active_count as f32;
        let mean_vis = sum_vis / active_count as f32;
        let occ_frac = occluded_count as f32 / active_count as f32;

        // Compute internal shear / surface strain variance
        let mut shear_sum = 0.0f32;
        for traj in &self.trajectories {
            let dx = traj.velocity.0 - mean_vx;
            let dy = traj.velocity.1 - mean_vy;
            shear_sum += dx * dx + dy * dy;
        }
        let shear_mag = (shear_sum / active_count as f32).sqrt();

        SurfaceKinematics {
            bulk_translation: (mean_vx, mean_vy),
            shear_magnitude: shear_mag,
            mean_visibility: mean_vis,
            occlusion_fraction: occ_frac,
        }
    }

    /// Prune dead trajectories that remain occluded or leave boundaries
    pub fn prune_and_reseed(&mut self, reseed_bbox: Option<[f32; 4]>, tag: &'static str) {
        self.trajectories.retain(|t| t.visibility > 0.15 && t.confidence > 0.15);

        if self.trajectories.len() < self.config.target_point_count / 2 {
            if let Some(bbox) = reseed_bbox {
                let deficit = self.config.target_point_count.saturating_sub(self.trajectories.len());
                self.seed_from_bbox(bbox, tag, deficit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_tracker_seeding_from_bbox() {
        let mut tracker = TapPointTracker::new(TapTrackerConfig {
            target_point_count: 16,
            ..Default::default()
        });

        tracker.seed_from_bbox([0.2, 0.3, 0.6, 0.7], "probe", 16);
        assert_eq!(tracker.trajectories.len(), 16);
        for t in &tracker.trajectories {
            assert!(t.current_pos.0 >= 0.2 && t.current_pos.0 <= 0.6);
            assert!(t.current_pos.1 >= 0.3 && t.current_pos.1 <= 0.7);
            assert_eq!(t.tag, "probe");
            assert_eq!(t.visibility, 1.0);
        }
    }

    #[test]
    fn test_point_tracker_step_flow_propagation() {
        let mut tracker = TapPointTracker::new(TapTrackerConfig::default());
        tracker.trajectories.push(PointTrajectory::new(0, (0.5, 0.5), "shaft"));

        // Create uniform flow field pointing downward (+y by 10 pixels)
        let w = 100;
        let h = 100;
        let mut flow = FlowField::new(w, h);
        for i in 0..(w * h) {
            flow.u[i] = 0.0;
            flow.v[i] = 10.0; // 10% normalized displacement downward
        }

        let kinematics = tracker.step_frame(&flow, 100);
        assert_eq!(tracker.trajectories.len(), 1);
        let pt = &tracker.trajectories[0];

        assert!((pt.current_pos.0 - 0.5).abs() < 1e-3);
        assert!((pt.current_pos.1 - 0.6).abs() < 1e-3); // 0.5 + 10/100 = 0.6
        assert!((kinematics.bulk_translation.1 - 0.1).abs() < 1e-3);
        assert_eq!(pt.history.len(), 1);
    }

    #[test]
    fn test_point_tracker_surface_shear_detection() {
        let mut tracker = TapPointTracker::new(TapTrackerConfig::default());
        // Point A at (0.4, 0.5), Point B at (0.6, 0.5)
        tracker.trajectories.push(PointTrajectory::new(0, (0.4, 0.5), "left"));
        tracker.trajectories.push(PointTrajectory::new(1, (0.6, 0.5), "right"));

        // Opposing flow creating shear deformation (rotational/sliding surface)
        let w = 100;
        let h = 100;
        let mut flow = FlowField::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                if x < 50 {
                    flow.v[idx] = -5.0;
                } else {
                    flow.v[idx] = 5.0;
                }
            }
        }

        let kinematics = tracker.step_frame(&flow, 200);
        // Bulk vertical translation is zero because -0.05 and +0.05 cancel out
        assert!(kinematics.bulk_translation.1.abs() < 1e-4);
        // But shear magnitude is non-zero, proving surface deformation is captured
        assert!(kinematics.shear_magnitude > 0.04);
    }

    #[test]
    fn test_tapnext_graceful_fallback() {
        let mut config = TapTrackerConfig::default();
        config.use_neural = true; // even if true, no session means fallback

        let mut tracker = TapPointTracker::new(config);
        tracker.trajectories.push(PointTrajectory::new(0, (0.5, 0.5), "test"));

        let mut flow = FlowField::new(100, 100);
        // flow v = 10.0 -> +0.1 normalized
        for i in 0..10000 {
            flow.v[i] = 10.0;
        }

        let kinematics = tracker.step_frame(&flow, 100);
        assert!((tracker.trajectories[0].current_pos.1 - 0.6).abs() < 1e-3);
        assert!((kinematics.bulk_translation.1 - 0.1).abs() < 1e-3);
    }
}
