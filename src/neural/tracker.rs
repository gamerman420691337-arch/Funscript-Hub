//! Anatomical stroke tracking, temporal memory bank, and 6-DOF pose fusion.

use crate::neural::pose::Pose3D;
use crate::neural::yolo::Detection;
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
    last_position: f32,
    /// Temporal landmark memory bank
    pub memory: TemporalMemoryBank,
}

impl Default for AnatomicalTracker {
    fn default() -> Self {
        Self {
            min_dist: 0.05,
            max_dist: 0.35,
            last_position: 50.0,
            memory: TemporalMemoryBank::default(),
        }
    }
}

impl AnatomicalTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// SOTA 6-DOF update with occlusion recovery and optical flow propagation.
    /// Returns (Pose3D, OcclusionState)
    pub fn update_pose(
        &mut self,
        detections: &[Detection],
        flow: Option<&FlowField>,
    ) -> (Pose3D, OcclusionState) {
        // 1. Find probe organ: glans (1) > penis (0)
        let det_probe = detections
            .iter()
            .filter(|d| d.class_id == 1)
            .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
            .or_else(|| {
                detections
                    .iter()
                    .filter(|d| d.class_id == 0)
                    .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
            });

        // 2. Find target organ: pussy (2) > anus (4) > butt (3) > hand (7)
        let det_target = detections
            .iter()
            .filter(|d| d.class_id == 2)
            .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
            .or_else(|| {
                detections
                    .iter()
                    .filter(|d| d.class_id == 4)
                    .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
            })
            .or_else(|| {
                detections
                    .iter()
                    .filter(|d| d.class_id == 3)
                    .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
            })
            .or_else(|| {
                detections
                    .iter()
                    .filter(|d| d.class_id == 7)
                    .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
            });

        // 3. Update memory bank and classify occlusion state
        let occlusion_state = match (&det_probe, &det_target) {
            (Some(probe), Some(target)) => {
                if let Some(ref mut tp) = self.memory.tracked_probe {
                    tp.update_detection(probe.center, probe.bbox, probe.confidence);
                } else {
                    self.memory.tracked_probe = Some(TrackedPoint::new(probe.center, probe.bbox, probe.confidence));
                }

                if let Some(ref mut tt) = self.memory.tracked_target {
                    tt.update_detection(target.center, target.bbox, target.confidence);
                } else {
                    self.memory.tracked_target = Some(TrackedPoint::new(target.center, target.bbox, target.confidence));
                }
                OcclusionState::Visible
            }
            (None, Some(target)) => {
                // Probe lost but target detected
                if let Some(ref mut tt) = self.memory.tracked_target {
                    tt.update_detection(target.center, target.bbox, target.confidence);
                } else {
                    self.memory.tracked_target = Some(TrackedPoint::new(target.center, target.bbox, target.confidence));
                }

                if let Some(ref mut tp) = self.memory.tracked_probe {
                    let inside_target_x = tp.center.0 >= target.bbox[0] - 0.05 && tp.center.0 <= target.bbox[2] + 0.05;
                    let inside_target_y = tp.center.1 >= target.bbox[1] - 0.05 && tp.center.1 <= target.bbox[3] + 0.05;

                    if inside_target_x && inside_target_y && tp.frames_occluded < 45 {
                        // Tip is occluded inside target during penetration
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
            (Some(probe), None) => {
                if let Some(ref mut tp) = self.memory.tracked_probe {
                    tp.update_detection(probe.center, probe.bbox, probe.confidence);
                } else {
                    self.memory.tracked_probe = Some(TrackedPoint::new(probe.center, probe.bbox, probe.confidence));
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

        // 4. Compute 6-DOF Pose from tracked points
        if let (Some(ref tp), Some(ref tt)) = (&self.memory.tracked_probe, &self.memory.tracked_target) {
            let dx = tp.center.0 - tt.center.0;
            let dy = tp.center.1 - tt.center.1;
            let dist = (dx * dx + dy * dy).sqrt();

            // When occluded inside, penetration is at maximum depth
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
            (pose, occlusion_state)
        } else {
            (self.memory.last_pose, occlusion_state)
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
}

