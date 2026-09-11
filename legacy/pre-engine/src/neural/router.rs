//! Adaptive Confidence and Failure Router for Fast/Slow Vision-to-Motion Tracking.
//!
//! Evaluates temporal reliability metrics on every frame to decide whether the fast
//! tracking path is reliable or whether to trigger a specialist refresh, dense repair,
//! or mark an unresolved interval.

use serde::{Deserialize, Serialize};

/// The execution branch selected by the failure router based on confidence metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExecutionBranch {
    /// Inexpensive continuous propagation (e.g. TAPNext++ / Lucas-Kanade flow).
    #[default]
    FastTracking,
    /// Fast tracking was uncertain or refresh cadence elapsed; run specialist detector/segmenter.
    SpecialistRefresh,
    /// Severe deformation or occlusion detected; invoke dense correspondence (e.g. CoWTracker).
    DenseRepair,
    /// Complete tracking loss or scene cut; perform open-vocabulary semantic reacquisition (SAM 3.1).
    SemanticReacquisition,
    /// Kinematically unobservable interval; motion is explicitly marked unresolved with uncertainty bounds.
    UnresolvedInterval,
}

#[allow(dead_code)]
impl ExecutionBranch {
    pub fn badge_label(&self) -> &'static str {
        match self {
            Self::FastTracking => "⚡ FAST (TAPNext++/Flow)",
            Self::SpecialistRefresh => "🧠 REFRESH (RF-DETR/YOLO)",
            Self::DenseRepair => "🔍 DENSE REPAIR (CoWTracker)",
            Self::SemanticReacquisition => "🌟 REACQUISITION (SAM 3.1)",
            Self::UnresolvedInterval => "⚠️ UNRESOLVED INTERVAL",
        }
    }

    pub fn is_fast(&self) -> bool {
        matches!(self, Self::FastTracking)
    }
}

/// Calibrated spatial uncertainty in normalized coordinates [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalibratedUncertainty {
    /// Standard deviation in horizontal axis sigma_x
    pub sigma_x: f32,
    /// Standard deviation in vertical axis sigma_y
    pub sigma_y: f32,
    /// Standard deviation in depth/scale axis sigma_z
    pub sigma_z: f32,
}

impl Default for CalibratedUncertainty {
    fn default() -> Self {
        Self {
            sigma_x: 0.02,
            sigma_y: 0.02,
            sigma_z: 0.05,
        }
    }
}

impl CalibratedUncertainty {
    pub fn new(sigma_x: f32, sigma_y: f32, sigma_z: f32) -> Self {
        Self {
            sigma_x: sigma_x.max(1e-4),
            sigma_y: sigma_y.max(1e-4),
            sigma_z: sigma_z.max(1e-4),
        }
    }

    #[allow(dead_code)]
    /// Combined scalar uncertainty radius (Frobenius / Euclidean norm of variances)
    pub fn norm(&self) -> f32 {
        (self.sigma_x * self.sigma_x + self.sigma_y * self.sigma_y + self.sigma_z * self.sigma_z).sqrt()
    }
}

/// Real-time tracking metrics evaluated on each frame
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceMetrics {
    /// Forward-backward cycle consistency error: || x - track_back(track_fwd(x)) ||
    pub forward_backward_error: f32,
    /// Second derivative of position (acceleration magnitude in normalized units / frame^2)
    pub acceleration_jerk: f32,
    /// Detector or point visibility confidence score in [0.0, 1.0]
    pub landmark_confidence: f32,
    /// Whether tracked points remain within the semantic organ bounding hull
    pub inside_hull: bool,
    /// Cross-modal audio impact flag (e.g. from spectral biquad transients)
    pub audio_transient_detected: bool,
    /// Number of consecutive frames since the last full semantic detector execution
    pub frames_since_refresh: usize,
    /// Computed calibrated spatial uncertainty
    pub uncertainty: CalibratedUncertainty,
}

impl Default for ConfidenceMetrics {
    fn default() -> Self {
        Self {
            forward_backward_error: 0.0,
            acceleration_jerk: 0.0,
            landmark_confidence: 1.0,
            inside_hull: true,
            audio_transient_detected: false,
            frames_since_refresh: 0,
            uncertainty: CalibratedUncertainty::default(),
        }
    }
}

/// Thresholds and configuration governing failure router decisions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Maximum allowable forward-backward cycle error before flagging drift (default: 0.06 in norm coord)
    pub max_fb_error: f32,
    /// Maximum kinematically plausible acceleration before flagging a tracking jump (default: 0.15 norm/frame^2)
    pub max_acceleration_jerk: f32,
    /// Confidence floor below which fast tracking is considered degraded (default: 0.40)
    pub min_confidence_floor: f32,
    /// Maximum frames between mandatory specialist refreshes (cadence k, default: 30 frames)
    pub refresh_cadence_k: usize,
    /// Hard limit on consecutive occluded/lost frames before triggering semantic reacquisition (default: 45)
    pub max_lost_frames_reacquisition: usize,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            max_fb_error: 0.06,
            max_acceleration_jerk: 0.15,
            min_confidence_floor: 0.40,
            refresh_cadence_k: 30,
            max_lost_frames_reacquisition: 45,
        }
    }
}

/// The Confidence-Aware Failure Router that governs execution branch transitions.
#[derive(Debug, Clone)]
pub struct FailureRouter {
    pub config: RouterConfig,
    history_positions: Vec<(f32, f32)>,
    history_velocities: Vec<(f32, f32)>,
    consecutive_lost_frames: usize,
    total_frames_processed: usize,
    fast_branch_count: usize,
    refresh_branch_count: usize,
    dense_repair_count: usize,
    reacquisition_count: usize,
    unresolved_count: usize,
}

impl FailureRouter {
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config,
            history_positions: Vec::with_capacity(8),
            history_velocities: Vec::with_capacity(8),
            consecutive_lost_frames: 0,
            total_frames_processed: 0,
            fast_branch_count: 0,
            refresh_branch_count: 0,
            dense_repair_count: 0,
            reacquisition_count: 0,
            unresolved_count: 0,
        }
    }

    /// Reset router state on scene transitions
    pub fn reset(&mut self) {
        self.history_positions.clear();
        self.history_velocities.clear();
        self.consecutive_lost_frames = 0;
    }

    /// Calculate kinematics derivatives (velocity and acceleration jerk) given a new position
    pub fn update_kinematics(&mut self, current_pos: (f32, f32)) -> (f32, f32) {
        let n = self.history_positions.len();
        let (vx, vy) = if n > 0 {
            let last = self.history_positions[n - 1];
            (current_pos.0 - last.0, current_pos.1 - last.1)
        } else {
            (0.0, 0.0)
        };

        let accel = if !self.history_velocities.is_empty() {
            let last_v = self.history_velocities[self.history_velocities.len() - 1];
            let ax = vx - last_v.0;
            let ay = vy - last_v.1;
            (ax * ax + ay * ay).sqrt()
        } else {
            0.0
        };

        let speed = (vx * vx + vy * vy).sqrt();

        self.history_positions.push(current_pos);
        if self.history_positions.len() > 10 {
            self.history_positions.remove(0);
        }

        self.history_velocities.push((vx, vy));
        if self.history_velocities.len() > 10 {
            self.history_velocities.remove(0);
        }

        (speed, accel)
    }

    /// Route frame to the optimal execution branch according to confidence metrics
    pub fn route_frame(
        &mut self,
        metrics: &ConfidenceMetrics,
        is_scene_cut: bool,
    ) -> ExecutionBranch {
        self.total_frames_processed += 1;

        // 1. Scene cuts immediately require full semantic reacquisition
        if is_scene_cut {
            self.reset();
            self.reacquisition_count += 1;
            return ExecutionBranch::SemanticReacquisition;
        }

        // 2. Prolonged loss of landmarks triggers semantic reacquisition
        if self.consecutive_lost_frames >= self.config.max_lost_frames_reacquisition {
            self.consecutive_lost_frames = 0;
            self.reacquisition_count += 1;
            return ExecutionBranch::SemanticReacquisition;
        }

        // 3. Cadence refresh: run specialist detector every k frames to prevent error drift
        if metrics.frames_since_refresh >= self.config.refresh_cadence_k {
            self.refresh_branch_count += 1;
            return ExecutionBranch::SpecialistRefresh;
        }

        // 4. Kinematic anomaly: severe acceleration jerk indicates tracking swap or tracking divergence
        let is_jerk_divergent = metrics.acceleration_jerk > self.config.max_acceleration_jerk;
        // 5. Forward-backward inconsistency: cycle drift
        let is_fb_inconsistent = metrics.forward_backward_error > self.config.max_fb_error;
        // 6. Low confidence floor or outside anatomical hull
        let is_confidence_low = metrics.landmark_confidence < self.config.min_confidence_floor;

        if is_jerk_divergent || is_fb_inconsistent {
            // Unphysical jump or tracking drift: trigger dense repair or specialist refresh
            if metrics.inside_hull {
                self.dense_repair_count += 1;
                ExecutionBranch::DenseRepair
            } else {
                self.consecutive_lost_frames += 1;
                self.refresh_branch_count += 1;
                ExecutionBranch::SpecialistRefresh
            }
        } else if is_confidence_low || !metrics.inside_hull {
            self.consecutive_lost_frames += 1;
            // If an audio impact occurs during low visual confidence, we know turnaround occurred
            // even if optical points are occluded
            if metrics.audio_transient_detected {
                // Cross-modal clue resolves ambiguity: trigger specialist refresh with grounded timing
                self.refresh_branch_count += 1;
                ExecutionBranch::SpecialistRefresh
            } else if self.consecutive_lost_frames > 15 {
                self.unresolved_count += 1;
                ExecutionBranch::UnresolvedInterval
            } else {
                self.refresh_branch_count += 1;
                ExecutionBranch::SpecialistRefresh
            }
        } else {
            // All reliability indicators pass: fast propagation is trusted
            self.consecutive_lost_frames = 0;
            self.fast_branch_count += 1;
            ExecutionBranch::FastTracking
        }
    }

    /// Execution statistics summary
    pub fn execution_stats(&self) -> RouterStats {
        let total = self.total_frames_processed.max(1) as f32;
        RouterStats {
            total_frames: self.total_frames_processed,
            fast_percent: (self.fast_branch_count as f32 / total) * 100.0,
            refresh_percent: (self.refresh_branch_count as f32 / total) * 100.0,
            dense_repair_percent: (self.dense_repair_count as f32 / total) * 100.0,
            reacquisition_percent: (self.reacquisition_count as f32 / total) * 100.0,
            unresolved_percent: (self.unresolved_count as f32 / total) * 100.0,
        }
    }
}

/// Aggregated router execution statistics across a session
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RouterStats {
    pub total_frames: usize,
    pub fast_percent: f32,
    pub refresh_percent: f32,
    pub dense_repair_percent: f32,
    pub reacquisition_percent: f32,
    pub unresolved_percent: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_router_defaults_to_fast_when_confident() {
        let mut router = FailureRouter::new(RouterConfig::default());
        let mut metrics = ConfidenceMetrics::default();
        metrics.landmark_confidence = 0.95;
        metrics.forward_backward_error = 0.01;
        metrics.acceleration_jerk = 0.02;
        metrics.inside_hull = true;
        metrics.frames_since_refresh = 5;

        let branch = router.route_frame(&metrics, false);
        assert_eq!(branch, ExecutionBranch::FastTracking);
    }

    #[test]
    fn test_failure_router_detects_acceleration_jerk() {
        let mut router = FailureRouter::new(RouterConfig::default());
        let mut metrics = ConfidenceMetrics::default();
        metrics.landmark_confidence = 0.95;
        metrics.acceleration_jerk = 0.35; // exceeds max_acceleration_jerk 0.15
        metrics.inside_hull = true;

        let branch = router.route_frame(&metrics, false);
        assert_eq!(branch, ExecutionBranch::DenseRepair);
    }

    #[test]
    fn test_failure_router_detects_forward_backward_inconsistency() {
        let mut router = FailureRouter::new(RouterConfig::default());
        let mut metrics = ConfidenceMetrics::default();
        metrics.forward_backward_error = 0.12; // exceeds max_fb_error 0.06
        metrics.inside_hull = false;

        let branch = router.route_frame(&metrics, false);
        assert_eq!(branch, ExecutionBranch::SpecialistRefresh);
    }

    #[test]
    fn test_failure_router_scene_cut_triggers_semantic_reacquisition() {
        let mut router = FailureRouter::new(RouterConfig::default());
        let metrics = ConfidenceMetrics::default();

        let branch = router.route_frame(&metrics, true);
        assert_eq!(branch, ExecutionBranch::SemanticReacquisition);
    }

    #[test]
    fn test_failure_router_refresh_cadence() {
        let mut router = FailureRouter::new(RouterConfig {
            refresh_cadence_k: 10,
            ..Default::default()
        });
        let mut metrics = ConfidenceMetrics::default();
        metrics.frames_since_refresh = 10;

        let branch = router.route_frame(&metrics, false);
        assert_eq!(branch, ExecutionBranch::SpecialistRefresh);
    }

    #[test]
    fn test_kinematics_velocity_and_acceleration_update() {
        let mut router = FailureRouter::new(RouterConfig::default());
        let (v1, a1) = router.update_kinematics((0.5, 0.5));
        assert_eq!(v1, 0.0);
        assert_eq!(a1, 0.0);

        let (v2, a2) = router.update_kinematics((0.5, 0.6));
        assert!((v2 - 0.1).abs() < 1e-4);
        assert!((a2 - 0.1).abs() < 1e-4);

        // Constant velocity: acceleration should drop to zero
        let (v3, a3) = router.update_kinematics((0.5, 0.7));
        assert!((v3 - 0.1).abs() < 1e-4);
        assert!(a3 < 1e-4);
    }
}
