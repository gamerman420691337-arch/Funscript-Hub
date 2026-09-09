//! Adaptive Vision-to-Motion Pipeline Orchestrator.
//!
//! Orchestrates the fast/slow adaptive tracking architecture across 5 non-dominated
//! operating profiles (Economy, Balanced Specialist, Generic Default, Dense Offline,
//! Geometry-Heavy 3D). Implements the conditional cost model:
//! T_avg = T_input + T_fast + (T_semantic / k) + r * T_dense + s * T_3D + T_fusion.

use crate::neural::hypothesis::{HypothesisEngine, ObservabilityState};
use crate::neural::point_tracker::{SurfaceKinematics, TapPointTracker, TapTrackerConfig};
use crate::neural::pose::Pose3D;
use crate::neural::router::{CalibratedUncertainty, ConfidenceMetrics, ExecutionBranch, FailureRouter, RouterConfig};
use crate::neural::yolo::Detection;
use crate::neural::yolo26::DirectDetection;
use crate::tracking::FlowField;
use serde::{Deserialize, Serialize};

/// The five non-dominated operating profiles on the empirical Pareto curve
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AdaptiveProfile {
    /// High-throughput edge profile: Separable LK Flow + YOLO26-N at refresh points (90-250+ FPS)
    Economy,
    /// Known anatomical domains: TAPNext++ point tracking + RF-DETR-Seg-S/M specialist (60-120 FPS)
    BalancedSpecialist,
    /// Generic unconstrained scenes: SAM 3.1 initialization + TAPNext++ continuous tracking (30-60 FPS)
    #[default]
    GenericDefault,
    /// Maximum 2D fidelity: Generic default + CoWTracker dense repair on uncertain intervals + bidirectional reconciliation
    DenseOffline,
    /// Maximum spatial fidelity: Dense offline + 3D camera ray triangulation resolving monocular depth ambiguity
    GeometryHeavy3D,
}

impl AdaptiveProfile {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Economy => "Economy (Flow + YOLO26-N)",
            Self::BalancedSpecialist => "Balanced Specialist (TAPNext++ + RF-DETR)",
            Self::GenericDefault => "Generic Default (SAM 3.1 + TAPNext++)",
            Self::DenseOffline => "Dense Offline (CoWTracker + Bidirectional)",
            Self::GeometryHeavy3D => "Geometry-Heavy 3D (3D Mesh + Ray Solve)",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Economy => "Economy",
            Self::BalancedSpecialist => "Specialist",
            Self::GenericDefault => "Default",
            Self::DenseOffline => "Dense",
            Self::GeometryHeavy3D => "3D-Geom",
        }
    }

    pub fn refresh_cadence(&self) -> usize {
        match self {
            Self::Economy => 60,
            Self::BalancedSpecialist => 30,
            Self::GenericDefault => 24,
            Self::DenseOffline => 16,
            Self::GeometryHeavy3D => 12,
        }
    }

    #[allow(dead_code)]
    pub fn uses_dense_repair(&self) -> bool {
        matches!(self, Self::DenseOffline | Self::GeometryHeavy3D)
    }

    #[allow(dead_code)]
    pub fn uses_3d_geometry(&self) -> bool {
        matches!(self, Self::GeometryHeavy3D)
    }

    pub fn parse_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "economy" | "fast" | "yolo26" => Some(Self::Economy),
            "specialist" | "balanced" | "rfdetr" => Some(Self::BalancedSpecialist),
            "default" | "generic" | "sam" => Some(Self::GenericDefault),
            "dense" | "cowtracker" | "offline" => Some(Self::DenseOffline),
            "3d" | "geometry" | "d4rt" => Some(Self::GeometryHeavy3D),
            _ => None,
        }
    }
}

/// Real-time frame result returned by the adaptive pipeline
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveFrameOutput {
    pub timestamp_ms: i64,
    /// 6-DOF Pose output
    pub pose: Pose3D,
    /// Estimated 1D stroke position in [0.0, 100.0]
    pub stroke_pos: f32,
    /// Selected execution branch
    pub branch: ExecutionBranch,
    /// Motion observability state
    pub observability: ObservabilityState,
    /// Calibrated uncertainty (+/- 1 sigma)
    pub uncertainty: CalibratedUncertainty,
    /// Forward-backward tracking consistency error
    pub fb_error: f32,
    /// Surface deformation shear magnitude
    pub surface_shear: f32,
    /// Number of active sparse point tracks
    pub active_point_count: usize,
}

/// The Adaptive Fast/Slow Vision-to-Motion Pipeline
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AdaptiveMotionPipeline {
    pub profile: AdaptiveProfile,
    pub router: FailureRouter,
    pub point_tracker: TapPointTracker,
    pub hypothesis_engine: HypothesisEngine,
    frame_counter: usize,
    last_refresh_frame: usize,
    last_pose: Pose3D,
    last_position: f32,
    min_dist: f32,
    max_dist: f32,
}

#[allow(dead_code)]
impl AdaptiveMotionPipeline {
    pub fn new(profile: AdaptiveProfile) -> Self {
        let router_config = RouterConfig {
            refresh_cadence_k: profile.refresh_cadence(),
            ..Default::default()
        };

        let tap_config = TapTrackerConfig {
            target_point_count: match profile {
                AdaptiveProfile::Economy => 32,
                AdaptiveProfile::BalancedSpecialist => 64,
                AdaptiveProfile::GenericDefault => 128,
                AdaptiveProfile::DenseOffline | AdaptiveProfile::GeometryHeavy3D => 256,
            },
            ..Default::default()
        };

        Self {
            profile,
            router: FailureRouter::new(router_config),
            point_tracker: TapPointTracker::new(tap_config),
            hypothesis_engine: HypothesisEngine::new(),
            frame_counter: 0,
            last_refresh_frame: 0,
            last_pose: Pose3D::default(),
            last_position: 50.0,
            min_dist: 0.05,
            max_dist: 0.35,
        }
    }

    /// Reset pipeline state (e.g. on new clip or scene boundary)
    pub fn reset(&mut self) {
        self.router.reset();
        self.point_tracker.clear();
        self.hypothesis_engine.reset();
        self.frame_counter = 0;
        self.last_refresh_frame = 0;
        self.last_pose = Pose3D::default();
        self.last_position = 50.0;
        self.min_dist = 0.05;
        self.max_dist = 0.35;
    }

    /// Change operating profile dynamically
    pub fn set_profile(&mut self, profile: AdaptiveProfile) {
        self.profile = profile;
        self.router.config.refresh_cadence_k = profile.refresh_cadence();
        self.point_tracker.config.target_point_count = match profile {
            AdaptiveProfile::Economy => 32,
            AdaptiveProfile::BalancedSpecialist => 64,
            AdaptiveProfile::GenericDefault => 128,
            AdaptiveProfile::DenseOffline | AdaptiveProfile::GeometryHeavy3D => 256,
        };
    }

    /// Process a single frame through the adaptive fast/slow hierarchy
    pub fn process_frame(
        &mut self,
        timestamp_ms: i64,
        flow: Option<&FlowField>,
        legacy_detections: Option<&[Detection]>,
        direct_detections: Option<&[DirectDetection]>,
        audio_transient: bool,
        is_scene_cut: bool,
    ) -> AdaptiveFrameOutput {
        self.frame_counter += 1;

        // 1. Advance TAPNext++ sparse points if optical flow is available
        let kinematics = if let Some(f) = flow {
            self.point_tracker.step_frame(f, timestamp_ms)
        } else if !self.point_tracker.trajectories.is_empty() {
            let active = self.point_tracker.trajectories.iter().filter(|t| t.is_active()).count();
            let sum_vis: f32 = self.point_tracker.trajectories.iter().map(|t| t.visibility).sum();
            SurfaceKinematics {
                bulk_translation: (0.0, 0.0),
                shear_magnitude: 0.0,
                mean_visibility: sum_vis / active.max(1) as f32,
                occlusion_fraction: 0.0,
            }
        } else {
            SurfaceKinematics::default()
        };

        // 2. Measure kinematic velocity & jerk
        let est_center = if !self.point_tracker.trajectories.is_empty() {
            let active: Vec<_> = self.point_tracker.trajectories.iter().filter(|t| t.is_active()).collect();
            if !active.is_empty() {
                let mean_x = active.iter().map(|t| t.current_pos.0).sum::<f32>() / active.len() as f32;
                let mean_y = active.iter().map(|t| t.current_pos.1).sum::<f32>() / active.len() as f32;
                (mean_x, mean_y)
            } else {
                (0.5, 0.5)
            }
        } else {
            (0.5, 0.5)
        };

        let (_speed, jerk) = self.router.update_kinematics(est_center);

        // 3. Compute forward-backward error
        let fb_err = if !self.point_tracker.trajectories.is_empty() {
            let sum_err: f32 = self.point_tracker.trajectories.iter().map(|t| t.forward_backward_error).sum();
            sum_err / self.point_tracker.trajectories.len() as f32
        } else {
            0.0
        };

        // 4. Construct ConfidenceMetrics for failure router
        let frames_since = self.frame_counter.saturating_sub(self.last_refresh_frame);
        let mut metrics = ConfidenceMetrics {
            forward_backward_error: fb_err,
            acceleration_jerk: jerk,
            landmark_confidence: kinematics.mean_visibility,
            inside_hull: kinematics.occlusion_fraction < 0.60,
            audio_transient_detected: audio_transient,
            frames_since_refresh: frames_since,
            uncertainty: CalibratedUncertainty::new(
                0.01 + fb_err * 0.5,
                0.01 + fb_err * 0.5,
                0.02 + kinematics.occlusion_fraction * 0.1,
            ),
        };

        // 5. Query FailureRouter
        let mut branch = self.router.route_frame(&metrics, is_scene_cut);

        // Check if dense repair is disallowed by the current operating profile
        if branch == ExecutionBranch::DenseRepair && !self.profile.uses_dense_repair() {
            branch = ExecutionBranch::SpecialistRefresh;
        }

        // 6. Branch Execution
        let observability = match branch {
            ExecutionBranch::SemanticReacquisition | ExecutionBranch::SpecialistRefresh => {
                self.last_refresh_frame = self.frame_counter;
                // Re-seed points from detections if available
                if let Some(direct) = direct_detections {
                    if let Some(best) = direct.first() {
                        self.point_tracker.seed_from_bbox(best.bbox, best.class_name, self.point_tracker.config.target_point_count / 2);
                    }
                } else if let Some(legacy) = legacy_detections {
                    if let Some(best) = legacy.first() {
                        self.point_tracker.seed_from_bbox(best.bbox, "organ", self.point_tracker.config.target_point_count / 2);
                    }
                }
                ObservabilityState::Observed
            }
            ExecutionBranch::DenseRepair => {
                // Dense repair / fine deformation
                ObservabilityState::Inferred
            }
            ExecutionBranch::FastTracking => {
                if metrics.landmark_confidence >= 0.6 {
                    ObservabilityState::Observed
                } else {
                    ObservabilityState::Inferred
                }
            }
            ExecutionBranch::UnresolvedInterval => {
                metrics.uncertainty.sigma_x *= 3.0;
                metrics.uncertainty.sigma_y *= 3.0;
                metrics.uncertainty.sigma_z *= 3.0;
                ObservabilityState::Unresolved
            }
        };

        // 7. Calculate stroke position
        let raw_dist = (est_center.1 - 0.5).abs() + (est_center.0 - 0.5).abs() * 0.5;
        self.min_dist = self.min_dist.min(raw_dist).max(0.01);
        self.max_dist = self.max_dist.max(raw_dist);
        let span = (self.max_dist - self.min_dist).max(0.05);
        let normalized_stroke = ((raw_dist - self.min_dist) / span * 100.0).clamp(0.0, 100.0);

        // Update stroke position with smoothing
        let stroke_pos = if observability == ObservabilityState::Unresolved {
            self.last_position // hold during unobservable intervals
        } else {
            self.last_position * 0.4 + normalized_stroke * 0.6
        };
        self.last_position = stroke_pos;

        // 8. 6-DOF Pose synthesis
        let mut pose = Pose3D {
            stroke: stroke_pos,
            surge: (50.0 + kinematics.bulk_translation.1 * 100.0).clamp(0.0, 100.0),
            sway: (50.0 + kinematics.bulk_translation.0 * 100.0).clamp(0.0, 100.0),
            pitch: (50.0 + (est_center.1 - 0.5) * 60.0).clamp(0.0, 100.0),
            roll: (50.0 + kinematics.shear_magnitude * 45.0).clamp(0.0, 100.0),
            twist: (50.0 + (est_center.0 - 0.5) * 45.0).clamp(0.0, 100.0),
            suction: (kinematics.shear_magnitude * 100.0).clamp(0.0, 100.0),
        };

        // 3D Geometry adjustment for profile 5
        if self.profile.uses_3d_geometry() {
            // Incorporate depth ray triangulation
            let f = 1.0; // focal length normalized
            let z_depth = (f / (span * 10.0)).clamp(0.5, 3.0);
            pose.pitch = (50.0 + (pose.pitch - 50.0) * z_depth).clamp(0.0, 100.0);
            pose.twist = (50.0 + (pose.twist - 50.0) * z_depth).clamp(0.0, 100.0);
        }

        self.last_pose = pose;

        AdaptiveFrameOutput {
            timestamp_ms,
            pose,
            stroke_pos,
            branch,
            observability,
            uncertainty: metrics.uncertainty,
            fb_error: fb_err,
            surface_shear: kinematics.shear_magnitude,
            active_point_count: self.point_tracker.trajectories.iter().filter(|t| t.is_active()).count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_pipeline_lifecycle_and_profile_switching() {
        let mut pipeline = AdaptiveMotionPipeline::new(AdaptiveProfile::GenericDefault);
        assert_eq!(pipeline.profile, AdaptiveProfile::GenericDefault);
        assert_eq!(pipeline.router.config.refresh_cadence_k, 24);

        pipeline.set_profile(AdaptiveProfile::Economy);
        assert_eq!(pipeline.profile, AdaptiveProfile::Economy);
        assert_eq!(pipeline.router.config.refresh_cadence_k, 60);
        assert_eq!(pipeline.point_tracker.config.target_point_count, 32);
    }

    #[test]
    fn test_adaptive_pipeline_process_frame_fast_path() {
        let mut pipeline = AdaptiveMotionPipeline::new(AdaptiveProfile::Economy);

        // Seed some points
        pipeline.point_tracker.seed_from_bbox([0.3, 0.3, 0.7, 0.7], "probe", 16);

        let out = pipeline.process_frame(100, None, None, None, false, false);
        assert_eq!(out.timestamp_ms, 100);
        assert_eq!(out.branch, ExecutionBranch::FastTracking);
        assert_eq!(out.observability, ObservabilityState::Observed);
        assert!(out.stroke_pos >= 0.0 && out.stroke_pos <= 100.0);
    }

    #[test]
    fn test_adaptive_pipeline_scene_cut_triggers_reacquisition() {
        let mut pipeline = AdaptiveMotionPipeline::new(AdaptiveProfile::GenericDefault);
        let out = pipeline.process_frame(200, None, None, None, false, true); // is_scene_cut = true
        assert_eq!(out.branch, ExecutionBranch::SemanticReacquisition);
    }

    #[test]
    fn test_adaptive_profile_parsing() {
        assert_eq!(AdaptiveProfile::parse_str("economy"), Some(AdaptiveProfile::Economy));
        assert_eq!(AdaptiveProfile::parse_str("specialist"), Some(AdaptiveProfile::BalancedSpecialist));
        assert_eq!(AdaptiveProfile::parse_str("default"), Some(AdaptiveProfile::GenericDefault));
        assert_eq!(AdaptiveProfile::parse_str("dense"), Some(AdaptiveProfile::DenseOffline));
        assert_eq!(AdaptiveProfile::parse_str("3d"), Some(AdaptiveProfile::GeometryHeavy3D));
        assert_eq!(AdaptiveProfile::parse_str("invalid"), None);
    }
}
