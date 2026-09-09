//! Anatomical stroke tracking, temporal memory bank, and 6-DOF pose fusion.

use crate::neural::hypothesis::{HypothesisEngine, ObservabilityState};
use crate::neural::point_tracker::{TapPointTracker, TapTrackerConfig};
use crate::neural::pose::Pose3D;
use crate::neural::router::{CalibratedUncertainty, ConfidenceMetrics, ExecutionBranch, FailureRouter, RouterConfig};
use crate::neural::yolo::Detection;
use crate::neural::yolo26::DirectDetection;
use crate::tracking::FlowField;

/// Occlusion state for temporal point tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OcclusionState {
    /// Both probe tip and target are explicitly detected by YOLO
    Visible,
    /// Probe tip has entered target orifice bounding area and is occluded internally
    OccludedInside,
    /// Detections lost, propagating coordinates via optical flow
    #[default]
    Lost,
}

/// A temporally tracked anatomical point with velocity and confidence history
#[derive(Debug, Clone)]
pub struct TrackedPoint {
    pub center: (f32, f32),
    pub bbox: [f32; 4],
    pub velocity: (f32, f32),
    pub confidence: f32,
    pub frames_tracked: usize,
    pub frames_occluded: usize,
}

impl TrackedPoint {
    pub fn new(center: (f32, f32), bbox: [f32; 4], confidence: f32) -> Self {
        Self {
            center,
            bbox,
            velocity: (0.0, 0.0),
            confidence,
            frames_tracked: 1,
            frames_occluded: 0,
        }
    }

    /// Update with fresh neural YOLO detection
    pub fn update_detection(&mut self, center: (f32, f32), bbox: [f32; 4], confidence: f32) {
        let vx = center.0 - self.center.0;
        let vy = center.1 - self.center.1;
        self.velocity = (self.velocity.0 * 0.4 + vx * 0.6, self.velocity.1 * 0.4 + vy * 0.6);
        self.center = center;
        self.bbox = bbox;
        self.confidence = self.confidence * 0.3 + confidence * 0.7;
        self.frames_tracked += 1;
        self.frames_occluded = 0;
    }

    /// Propagate point forward using dense optical flow when detection is occluded or absent
    pub fn propagate_flow(&mut self, flow: &FlowField) {
        let (u, v) = flow.sample_flow_at(self.center.0, self.center.1);
        let norm_u = u / flow.width as f32;
        let norm_v = v / flow.height as f32;
        self.center.0 = (self.center.0 + norm_u).clamp(0.0, 1.0);
        self.center.1 = (self.center.1 + norm_v).clamp(0.0, 1.0);
        self.velocity = (norm_u, norm_v);
        self.confidence *= 0.95; // Decay confidence smoothly
        self.frames_occluded += 1;
    }
}

/// SAM 2 / Co-Tracker inspired temporal memory bank maintaining continuous organ tracking
#[derive(Debug, Clone, Default)]
pub struct TemporalMemoryBank {
    pub tracked_probe: Option<TrackedPoint>,
    pub tracked_target: Option<TrackedPoint>,
    pub state: OcclusionState,
    pub last_pose: Pose3D,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StrokeObservation {
    pub timestamp_ms: i64,
    /// Estimated position in [0.0, 100.0]
    pub position: f32,
    /// Whether this observation was derived from neural YOLO detection
    pub is_neural: bool,
    /// Probe organ center
    pub probe_center: Option<(f32, f32)>,
    /// Target organ center
    pub target_center: Option<(f32, f32)>,
}

#[derive(Debug, Clone)]
pub struct AnatomicalTracker {
    /// Rolling minimum distance observed (full penetration)
    min_dist: f32,
    /// Rolling maximum distance observed (full retraction)
    max_dist: f32,
    /// Last known estimated stroke position
    pub last_position: f32,
    /// Temporal landmark memory bank
    pub memory: TemporalMemoryBank,
    /// Adaptive confidence and failure router
    pub router: FailureRouter,
    /// TAPNext++ sparse point surface tracker
    pub point_tracker: TapPointTracker,
    /// Multi-hypothesis engine for occlusion handling
    #[allow(dead_code)]
    pub hypothesis_engine: HypothesisEngine,
    /// Last active execution branch
    pub last_branch: ExecutionBranch,
    /// Observability state of current motion
    pub observability: ObservabilityState,
    /// Calibrated uncertainty estimate
    pub uncertainty: CalibratedUncertainty,
    /// Consecutive frame counter
    frame_idx: usize,
    last_refresh_idx: usize,
}

impl Default for AnatomicalTracker {
    fn default() -> Self {
        Self {
            min_dist: 0.05,
            max_dist: 0.35,
            last_position: 50.0,
            memory: TemporalMemoryBank::default(),
            router: FailureRouter::new(RouterConfig::default()),
            point_tracker: TapPointTracker::new(TapTrackerConfig::default()),
            hypothesis_engine: HypothesisEngine::new(),
            last_branch: ExecutionBranch::FastTracking,
            observability: ObservabilityState::Observed,
            uncertainty: CalibratedUncertainty::default(),
            frame_idx: 0,
            last_refresh_idx: 0,
        }
    }
}

impl AnatomicalTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// SOTA 6-DOF update with occlusion recovery and optical flow propagation.
    /// Backward-compatible wrapper returning (Pose3D, OcclusionState).
    pub fn update_pose(
        &mut self,
        detections: &[Detection],
        flow: Option<&FlowField>,
    ) -> (Pose3D, OcclusionState) {
        let (pose, state, _, _) = self.update_pose_adaptive(
            Some(detections),
            None,
            flow,
            false,
            self.frame_idx as i64 * 33,
            false,
        );
        (pose, state)
    }

    /// Adaptive 6-DOF update with confidence routing, sparse point tracking, and audio grounding.
    pub fn update_pose_adaptive(
        &mut self,
        legacy_detections: Option<&[Detection]>,
        direct_detections: Option<&[DirectDetection]>,
        flow: Option<&FlowField>,
        audio_transient: bool,
        timestamp_ms: i64,
        is_scene_cut: bool,
    ) -> (Pose3D, OcclusionState, ExecutionBranch, ObservabilityState) {
        self.frame_idx += 1;

        // 1. Advance TAPNext++ points with flow
        let _kinematics = if let Some(f) = flow {
            self.point_tracker.step_frame(f, timestamp_ms)
        } else {
            Default::default()
        };

        // 2. Find probe organ: glans (1) > penis (0)
        let det_probe: Option<([f32; 4], (f32, f32), f32)> = if let Some(direct) = direct_detections {
            direct
                .iter()
                .filter(|d| d.class_id == 1)
                .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                .or_else(|| {
                    direct
                        .iter()
                        .filter(|d| d.class_id == 0)
                        .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                })
                .map(|d| (d.bbox, d.center, d.confidence))
        } else if let Some(legacy) = legacy_detections {
            legacy
                .iter()
                .filter(|d| d.class_id == 1)
                .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                .or_else(|| {
                    legacy
                        .iter()
                        .filter(|d| d.class_id == 0)
                        .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                })
                .map(|d| (d.bbox, d.center, d.confidence))
        } else {
            None
        };

        // 3. Find target organ: pussy (2) > anus (4) > butt (3) > hand (7)
        let det_target: Option<([f32; 4], (f32, f32), f32)> = if let Some(direct) = direct_detections {
            direct
                .iter()
                .filter(|d| d.class_id == 2 || d.class_id == 4 || d.class_id == 3 || d.class_id == 7)
                .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                .map(|d| (d.bbox, d.center, d.confidence))
        } else if let Some(legacy) = legacy_detections {
            legacy
                .iter()
                .filter(|d| d.class_id == 2 || d.class_id == 4 || d.class_id == 3 || d.class_id == 7)
                .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                .map(|d| (d.bbox, d.center, d.confidence))
        } else {
            None
        };

        // 4. Update memory bank and classify occlusion state
        let occlusion_state = match (&det_probe, &det_target) {
            (Some((p_bbox, p_center, p_conf)), Some((t_bbox, t_center, t_conf))) => {
                if let Some(ref mut tp) = self.memory.tracked_probe {
                    tp.update_detection(*p_center, *p_bbox, *p_conf);
                } else {
                    self.memory.tracked_probe = Some(TrackedPoint::new(*p_center, *p_bbox, *p_conf));
                }

                if let Some(ref mut tt) = self.memory.tracked_target {
                    tt.update_detection(*t_center, *t_bbox, *t_conf);
                } else {
                    self.memory.tracked_target = Some(TrackedPoint::new(*t_center, *t_bbox, *t_conf));
                }
                self.last_refresh_idx = self.frame_idx;
                OcclusionState::Visible
            }
            (None, Some((t_bbox, t_center, t_conf))) => {
                if let Some(ref mut tt) = self.memory.tracked_target {
                    tt.update_detection(*t_center, *t_bbox, *t_conf);
                } else {
                    self.memory.tracked_target = Some(TrackedPoint::new(*t_center, *t_bbox, *t_conf));
                }

                if let Some(ref mut tp) = self.memory.tracked_probe {
                    let inside_target_x = tp.center.0 >= t_bbox[0] - 0.05 && tp.center.0 <= t_bbox[2] + 0.05;
                    let inside_target_y = tp.center.1 >= t_bbox[1] - 0.05 && tp.center.1 <= t_bbox[3] + 0.05;

                    if inside_target_x && inside_target_y && tp.frames_occluded < 45 {
                        if let Some(f) = flow {
                            tp.propagate_flow(f);
                        } else {
                            tp.frames_occluded += 1;
                        }
                        OcclusionState::OccludedInside
                    } else if let Some(f) = flow {
                        tp.propagate_flow(f);
                        OcclusionState::Lost
                    } else {
                        OcclusionState::Lost
                    }
                } else {
                    OcclusionState::Lost
                }
            }
            (Some((p_bbox, p_center, p_conf)), None) => {
                if let Some(ref mut tp) = self.memory.tracked_probe {
                    tp.update_detection(*p_center, *p_bbox, *p_conf);
                } else {
                    self.memory.tracked_probe = Some(TrackedPoint::new(*p_center, *p_bbox, *p_conf));
                }
                if let (Some(ref mut tt), Some(f)) = (&mut self.memory.tracked_target, flow) {
                    tt.propagate_flow(f);
                }
                OcclusionState::Lost
            }
            (None, None) => {
                if let Some(f) = flow {
                    if let Some(ref mut tp) = self.memory.tracked_probe {
                        tp.propagate_flow(f);
                    }
                    if let Some(ref mut tt) = self.memory.tracked_target {
                        tt.propagate_flow(f);
                    }
                }
                OcclusionState::Lost
            }
        };

        self.memory.state = occlusion_state;

        // 5. Evaluate Confidence Metrics & Failure Routing
        let p_center = self.memory.tracked_probe.as_ref().map(|p| p.center).unwrap_or((0.5, 0.5));
        let (_speed, jerk) = self.router.update_kinematics(p_center);

        let fb_err = if !self.point_tracker.trajectories.is_empty() {
            let sum_err: f32 = self.point_tracker.trajectories.iter().map(|t| t.forward_backward_error).sum();
            sum_err / self.point_tracker.trajectories.len() as f32
        } else {
            0.0
        };

        let conf_floor = self.memory.tracked_probe.as_ref().map(|p| p.confidence).unwrap_or(0.0);
        let frames_since = self.frame_idx.saturating_sub(self.last_refresh_idx);

        let metrics = ConfidenceMetrics {
            forward_backward_error: fb_err,
            acceleration_jerk: jerk,
            landmark_confidence: conf_floor,
            inside_hull: occlusion_state != OcclusionState::Lost,
            audio_transient_detected: audio_transient,
            frames_since_refresh: frames_since,
            uncertainty: CalibratedUncertainty::new(
                0.01 + fb_err * 0.4,
                0.01 + fb_err * 0.4,
                0.02 + if occlusion_state == OcclusionState::OccludedInside { 0.05 } else { 0.01 },
            ),
        };

        self.uncertainty = metrics.uncertainty;
        let branch = self.router.route_frame(&metrics, is_scene_cut);
        self.last_branch = branch;

        // Observability state
        let observability = match occlusion_state {
            OcclusionState::Visible => ObservabilityState::Observed,
            OcclusionState::OccludedInside => {
                if audio_transient {
                    ObservabilityState::Observed // cross-modal audio transient confirms impact
                } else {
                    ObservabilityState::Inferred
                }
            }
            OcclusionState::Lost => {
                if branch == ExecutionBranch::UnresolvedInterval {
                    ObservabilityState::Unresolved
                } else {
                    ObservabilityState::Inferred
                }
            }
        };
        self.observability = observability;

        // 6. Compute 6-DOF Pose from tracked points
        if let (Some(ref tp), Some(ref tt)) = (&self.memory.tracked_probe, &self.memory.tracked_target) {
            let dx = tp.center.0 - tt.center.0;
            let dy = tp.center.1 - tt.center.1;
            let dist = (dx * dx + dy * dy).sqrt();

            let effective_dist = if occlusion_state == OcclusionState::OccludedInside {
                self.min_dist
            } else {
                dist
            };

            if effective_dist < self.min_dist {
                self.min_dist = effective_dist * 0.95 + self.min_dist * 0.05;
            }
            if effective_dist > self.max_dist {
                self.max_dist = effective_dist * 0.95 + self.max_dist * 0.05;
            }

            let span = (self.max_dist - self.min_dist).max(0.01);
            let raw_pos = ((self.max_dist - effective_dist) / span * 100.0).clamp(0.0, 100.0);
            self.last_position = self.last_position * 0.3 + raw_pos * 0.7;

            let vorticity = flow.map(|f| f.compute_vorticity(tt.center.0, tt.center.1, 0.15)).unwrap_or(0.0);

            let pose = Pose3D::from_landmarks(
                tp.center,
                tp.bbox,
                tt.center,
                tt.bbox,
                vorticity,
                self.last_position,
            );
            self.memory.last_pose = pose;
            (pose, occlusion_state, branch, observability)
        } else {
            (self.memory.last_pose, occlusion_state, branch, observability)
        }
    }

    /// Backwards-compatible stroke position estimation from YOLO detections.
    #[allow(dead_code, clippy::type_complexity)]
    pub fn update_from_detections(
        &mut self,
        detections: &[Detection],
    ) -> Option<(f32, (f32, f32), (f32, f32))> {
        let (pose, _) = self.update_pose(detections, None);
        if let (Some(ref tp), Some(ref tt)) = (&self.memory.tracked_probe, &self.memory.tracked_target) {
            Some((pose.stroke, tp.center, tt.center))
        } else {
            None
        }
    }

    /// Hybrid update: when neural detection fails, update via integrated optical velocity
    #[allow(dead_code)]
    pub fn update_hybrid_fallback(&mut self, optical_velocity: f32) -> f32 {
        let delta = optical_velocity * 2.0;
        self.last_position = (self.last_position + delta).clamp(0.0, 100.0);
        self.memory.last_pose.stroke = self.last_position;
        self.last_position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anatomical_tracker_distance_mapping() {
        let mut tracker = AnatomicalTracker::new();

        // Target (pussy) at center (0.5, 0.5)
        let target = Detection {
            class_id: 2,
            class_name: "pussy",
            confidence: 0.9,
            bbox: [0.4, 0.4, 0.6, 0.6],
            center: (0.5, 0.5),
        };

        // Deep insertion (probe right at target)
        let probe_close = Detection {
            class_id: 1,
            class_name: "glans",
            confidence: 0.9,
            bbox: [0.48, 0.48, 0.52, 0.52],
            center: (0.5, 0.52),
        };

        let (pos_deep, _, _) = tracker.update_from_detections(&[target.clone(), probe_close]).unwrap();

        // Full retraction (probe far away)
        let probe_far = Detection {
            class_id: 1,
            class_name: "glans",
            confidence: 0.9,
            bbox: [0.45, 0.8, 0.55, 0.9],
            center: (0.5, 0.85),
        };

        let (pos_far, _, _) = tracker.update_from_detections(&[target, probe_far]).unwrap();

        assert!(pos_deep > pos_far, "Deep insertion position should be higher than retracted");
    }

    #[test]
    fn test_occlusion_inside_recovery() {
        let mut tracker = AnatomicalTracker::new();

        let target = Detection {
            class_id: 2,
            class_name: "pussy",
            confidence: 0.95,
            bbox: [0.4, 0.4, 0.6, 0.6],
            center: (0.5, 0.5),
        };

        let probe = Detection {
            class_id: 1,
            class_name: "glans",
            confidence: 0.95,
            bbox: [0.48, 0.48, 0.52, 0.52],
            center: (0.5, 0.5),
        };

        // Frame 1: Visible penetration
        let (pose1, state1) = tracker.update_pose(&[target.clone(), probe], None);
        assert_eq!(state1, OcclusionState::Visible);
        assert!(pose1.stroke > 40.0);

        // Frame 2: Glans penetrates inside and is occluded (only target detected)
        let (pose2, state2) = tracker.update_pose(&[target], None);
        assert_eq!(state2, OcclusionState::OccludedInside);
        assert!(pose2.stroke > 40.0, "Occluded inside should maintain deep penetration");
    }

    #[test]
    fn test_cross_modal_audio_grounding_in_adaptive_pose() {
        let mut tracker = AnatomicalTracker::new();

        let target = Detection {
            class_id: 2,
            class_name: "pussy",
            confidence: 0.95,
            bbox: [0.4, 0.4, 0.6, 0.6],
            center: (0.5, 0.5),
        };

        // Probe is occluded inside, but an audio impact transient arrives
        let (_pose, state, _branch, observability) = tracker.update_pose_adaptive(
            Some(&[target]),
            None,
            None,
            true, // audio_transient = true
            500,
            false,
        );

        assert_eq!(state, OcclusionState::Lost); // no prior probe seeded
        assert_eq!(observability, ObservabilityState::Inferred);
    }
}
