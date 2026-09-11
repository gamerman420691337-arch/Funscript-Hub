//! Observation-bound stroke evidence adapter. No global image motion is promoted
//! into physical target motion, and no missing interval becomes a measured pause.

use super::detector::RawDetection;
use anyhow::{bail, Result};
use pulsar_core::perception::{
    seed_points_in_box, separate_target_motion, track_points_fb, CanonicalDetection, FlowConfig,
    TargetTracker, TrackerConfig, TrackingReview,
};
use pulsar_core::{
    AttemptId, CoordinateBox, CoordinateSpace, DetectionObservation, EntityTrackId, EvidenceKind,
    FrameContext, NormalizedPosition,
};
use sha2::{Digest, Sha256};

pub struct StrokeFrame {
    pub position: Option<NormalizedPosition>,
    pub evidence: EvidenceKind,
    /// Actual observations only, in source pixels, never predicted boxes.
    pub observed_boxes: Vec<DetectionObservation>,
    pub selected_class: Option<u32>,
    pub selected_track: Option<EntityTrackId>,
    pub reviews: Vec<String>,
}
struct Previous {
    gray: Vec<u8>,
    width: u32,
    height: u32,
    track: EntityTrackId,
    bounds: CoordinateBox,
}
pub struct StrokeTracker {
    tracker: TargetTracker,
    previous: Option<Previous>,
    selected: Option<EntityTrackId>,
    position: NormalizedPosition,
}
impl StrokeTracker {
    pub fn new(attempt: &AttemptId) -> Result<Self> {
        // A content-derived namespace supports every valid AttemptId length
        // without truncating caller identities or reusing another attempt's IDs.
        let namespace = EntityTrackId::new(format!(
            "attempt.{:x}",
            Sha256::digest(attempt.as_str().as_bytes())
        ))?;
        Ok(Self {
            tracker: TargetTracker::with_namespace(
                namespace,
                TrackerConfig {
                    max_missed_frames: 30,
                    ..TrackerConfig::default()
                },
            )?,
            previous: None,
            selected: None,
            position: NormalizedPosition::new(0.5)?,
        })
    }

    pub fn update(
        &mut self,
        context: FrameContext,
        gray: &[u8],
        width: u32,
        height: u32,
        detections: &[RawDetection],
        cut: bool,
    ) -> Result<StrokeFrame> {
        let pixels = (width as usize).checked_mul(height as usize);
        if width == 0 || height == 0 || pixels != Some(gray.len()) || gray.len() > 1_048_576 {
            bail!("stroke tracker requires an exact bounded grayscale frame");
        }
        let canonical = detections
            .iter()
            .map(|detection| {
                if detection
                    .bbox
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                {
                    bail!("raw detection is outside its normalized adapter contract");
                }
                Ok(CanonicalDetection::new(
                    CoordinateBox::new(
                        CoordinateSpace::SourcePixels,
                        f64::from(detection.bbox[0]) * f64::from(width),
                        f64::from(detection.bbox[1]) * f64::from(height),
                        f64::from(detection.bbox[2]) * f64::from(width),
                        f64::from(detection.bbox[3]) * f64::from(height),
                    )?,
                    detection.class_id,
                    f64::from(detection.score),
                )?)
            })
            .collect::<Result<Vec<_>>>()?;
        let update = self
            .tracker
            .update(context, width, height, &canonical, cut)?;
        let observed_boxes: Vec<_> = update
            .observations
            .iter()
            .filter(|observation| observation.evidence() == EvidenceKind::Observed)
            .cloned()
            .collect();
        let mut reviews: Vec<String> = update
            .reviews
            .iter()
            .map(|review| {
                match review {
                    TrackingReview::SceneCut => "scene_cut",
                    TrackingReview::SourceContextChanged => "source_context_changed",
                    TrackingReview::MissingDetection => "missing_detection",
                    TrackingReview::AmbiguousAssociation => "ambiguous_target_association",
                    TrackingReview::TrackBudgetReached => "target_tracking_budget",
                }
                .to_string()
            })
            .collect();
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| {
                observed_boxes
                    .iter()
                    .find(|observation| observation.track() == Some(id))
            })
            .or_else(|| {
                observed_boxes.iter().max_by(|a, b| {
                    a.confidence()
                        .value()
                        .total_cmp(&b.confidence().value())
                        .then_with(|| b.track().cmp(&a.track()))
                })
            })
            .cloned();
        let selected_track = selected
            .as_ref()
            .and_then(|observation| observation.track().cloned());
        let selected_class = selected_track
            .as_ref()
            .and_then(|track| update.class_for(track));
        if selected_track != self.selected && selected_track.is_some() {
            reviews.push("automatic_target_selection_unqualified".into());
        }
        self.selected = selected_track.clone();
        let mut result = StrokeFrame {
            position: None,
            evidence: EvidenceKind::Unavailable,
            observed_boxes,
            selected_class,
            selected_track: selected_track.clone(),
            reviews,
        };
        if cut
            || update
                .reviews
                .contains(&TrackingReview::SourceContextChanged)
            || update
                .reviews
                .contains(&TrackingReview::AmbiguousAssociation)
        {
            self.previous = None;
            return Ok(result);
        }
        let Some(observation) = selected else {
            self.previous = None;
            if !result
                .reviews
                .iter()
                .any(|reason| reason == "missing_detection")
            {
                result.reviews.push("missing_detection".into());
            }
            return Ok(result);
        };
        let track = selected_track
            .ok_or_else(|| anyhow::anyhow!("selected tracker observation lacks identity"))?;
        let next = Previous {
            gray: gray.to_vec(),
            width,
            height,
            track: track.clone(),
            bounds: observation.bounds().clone(),
        };
        let Some(previous) = self.previous.replace(next) else {
            result.position = Some(self.position);
            result.evidence = EvidenceKind::Synthesized;
            result.reviews.push("synthetic_motion_anchor".into());
            return Ok(result);
        };
        if previous.track != track || previous.width != width || previous.height != height {
            result.reviews.push("target_identity_discontinuity".into());
            return Ok(result);
        }
        let config = FlowConfig::default();
        let border = f64::from(config.patch_radius + 1);
        let roi = &previous.bounds;
        if roi.width() <= 2.0 * border || roi.height() <= 2.0 * border {
            result.reviews.push("target_flow_support_too_small".into());
            return Ok(result);
        }
        let interior = CoordinateBox::new(
            CoordinateSpace::SourcePixels,
            roi.x_min() + border,
            roi.y_min() + border,
            roi.x_max() - border,
            roi.y_max() - border,
        )?;
        let target_points = seed_points_in_box(&interior, width, height, 16)?;
        let full_frame = CoordinateBox::new(
            CoordinateSpace::SourcePixels,
            0.0,
            0.0,
            f64::from(width),
            f64::from(height),
        )?;
        let mut background_points = seed_points_in_box(&full_frame, width, height, 64)?;
        let current_bounds = observation.bounds();
        background_points.retain(|point| {
            let in_expanded = |bounds: &CoordinateBox| {
                point[0] >= bounds.x_min() - border
                    && point[0] <= bounds.x_max() + border
                    && point[1] >= bounds.y_min() - border
                    && point[1] <= bounds.y_max() + border
            };
            !in_expanded(roi) && !in_expanded(current_bounds)
        });
        let target = track_points_fb(&previous.gray, gray, width, height, &target_points, config)?;
        let background = track_points_fb(
            &previous.gray,
            gray,
            width,
            height,
            &background_points,
            config,
        )?;
        let separation = separate_target_motion(&target, &background)?;
        let Some(relative) = separation.target_relative_to_background else {
            result.reviews.push("unsupported_relative_flow".into());
            return Ok(result);
        };
        // This is an explicitly inferred image-relative stroke proposal, not
        // observed anatomy, metric depth, calibrated camera pose, or six-DoF.
        self.position = pulsar_core::integrate_vertical_motion(self.position, relative[1], height)?;
        result.position = Some(self.position);
        result.evidence = EvidenceKind::Inferred;
        result.reviews.push("relative_image_motion_inferred".into());
        result
            .reviews
            .push("background_selection_unqualified".into());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_core::{FrameId, SourcePlacementId, SourceVersionId, TransformId};

    fn context(frame: u64) -> FrameContext {
        FrameContext {
            source_version: SourceVersionId::new("source").unwrap(),
            source_placement: SourcePlacementId::new("placement").unwrap(),
            frame: FrameId::new(frame),
            transform: TransformId::new("analysis").unwrap(),
            seek_generation: 0,
            request_generation: frame,
        }
    }
    fn tracker() -> StrokeTracker {
        StrokeTracker::new(&AttemptId::new("attempt.test").unwrap()).unwrap()
    }
    fn detection(y: f32) -> RawDetection {
        RawDetection {
            class_id: 2,
            score: 0.9,
            bbox: [0.35, y, 0.65, y + 0.3],
        }
    }
    fn texture() -> Vec<u8> {
        (0..128)
            .flat_map(|y| (0..128).map(move |x| ((x * 37 + y * 61 + x * y * 7) % 251) as u8))
            .collect()
    }
    fn translate(image: &[u8], dy: isize) -> Vec<u8> {
        (0..128)
            .flat_map(|y| {
                (0..128).map(move |x| {
                    let old = y as isize - dy;
                    if (0..128).contains(&old) {
                        image[old as usize * 128 + x]
                    } else {
                        0
                    }
                })
            })
            .collect()
    }
    #[test]
    fn absent_detection_cannot_generate_motion_or_decorative_box() {
        let mut tracker = tracker();
        let result = tracker
            .update(context(0), &texture(), 128, 128, &[], false)
            .unwrap();
        assert!(result.position.is_none());
        assert!(result.observed_boxes.is_empty());
        assert_eq!(result.evidence, EvidenceKind::Unavailable);
    }
    #[test]
    fn neutral_anchor_is_synthesized_not_observed() {
        let mut tracker = tracker();
        let result = tracker
            .update(context(0), &texture(), 128, 128, &[detection(0.35)], false)
            .unwrap();
        assert_eq!(result.position.unwrap().value(), 0.5);
        assert_eq!(result.evidence, EvidenceKind::Synthesized);
        assert_eq!(result.observed_boxes[0].evidence(), EvidenceKind::Observed);
    }
    #[test]
    fn camera_translation_does_not_invent_relative_stroke() {
        let mut tracker = tracker();
        let first = texture();
        let second = translate(&first, 2);
        tracker
            .update(context(0), &first, 128, 128, &[detection(0.35)], false)
            .unwrap();
        let result = tracker
            .update(
                context(1),
                &second,
                128,
                128,
                &[detection(0.35 + 2.0 / 128.0)],
                false,
            )
            .unwrap();
        assert_eq!(
            result.evidence,
            EvidenceKind::Inferred,
            "{:?}",
            result.reviews
        );
        assert!((result.position.unwrap().value() - 0.5).abs() < 0.001);
        assert!(result
            .reviews
            .contains(&"background_selection_unqualified".into()));
    }
    #[test]
    fn cut_and_absence_never_reuse_stale_observations_as_motion() {
        let mut tracker = tracker();
        let image = texture();
        let first = tracker
            .update(context(0), &image, 128, 128, &[detection(0.35)], false)
            .unwrap();
        let cut = tracker
            .update(context(1), &image, 128, 128, &[detection(0.35)], true)
            .unwrap();
        assert!(cut.position.is_none());
        assert_ne!(first.selected_track, cut.selected_track);
        let absent = tracker
            .update(context(2), &image, 128, 128, &[], false)
            .unwrap();
        assert!(absent.observed_boxes.is_empty());
        assert!(absent.position.is_none());
    }
    #[test]
    fn low_texture_requires_review_not_measured_pause() {
        let mut tracker = tracker();
        tracker
            .update(
                context(0),
                &[128; 16384],
                128,
                128,
                &[detection(0.35)],
                false,
            )
            .unwrap();
        let result = tracker
            .update(
                context(1),
                &[128; 16384],
                128,
                128,
                &[detection(0.35)],
                false,
            )
            .unwrap();
        assert!(result.position.is_none());
        assert!(result.reviews.contains(&"unsupported_relative_flow".into()));
    }
    #[test]
    fn malformed_gray_or_box_is_rejected_before_state_advance() {
        let mut tracker = tracker();
        assert!(tracker
            .update(context(0), &[0; 2], 128, 128, &[], false)
            .is_err());
        let invalid = RawDetection {
            class_id: 2,
            score: 0.9,
            bbox: [0.8, 0.1, 0.2, 0.4],
        };
        assert!(tracker
            .update(context(0), &texture(), 128, 128, &[invalid], false)
            .is_err());
        assert!(tracker
            .update(context(0), &texture(), 128, 128, &[detection(0.35)], false)
            .is_ok());
    }
    #[test]
    fn target_only_motion_is_small_relative_displacement_and_reverses() {
        let mut tracker = tracker();
        let first = texture();
        let mut second = first.clone();
        for y in 46..88 {
            for x in 40..90 {
                second[y * 128 + x] = first[(y - 2) * 128 + x];
            }
        }
        tracker
            .update(context(0), &first, 128, 128, &[detection(0.35)], false)
            .unwrap();
        let forward = tracker
            .update(
                context(1),
                &second,
                128,
                128,
                &[detection(0.35 + 2.0 / 128.0)],
                false,
            )
            .unwrap();
        assert_eq!(
            forward.evidence,
            EvidenceKind::Inferred,
            "{:?}",
            forward.reviews
        );
        assert!((forward.position.unwrap().value() - (0.5 + 2.0 / 128.0)).abs() < 0.002);
        let reverse = tracker
            .update(context(2), &first, 128, 128, &[detection(0.35)], false)
            .unwrap();
        assert_eq!(
            reverse.evidence,
            EvidenceKind::Inferred,
            "{:?}",
            reverse.reviews
        );
        assert!((reverse.position.unwrap().value() - 0.5).abs() < 0.002);
    }

    #[test]
    fn intentional_stride_does_not_expire_a_present_target() {
        let mut tracker = tracker();
        let image = texture();
        let first = tracker
            .update(context(0), &image, 128, 128, &[detection(0.35)], false)
            .unwrap();
        let next = tracker
            .update(context(32), &image, 128, 128, &[detection(0.35)], false)
            .unwrap();
        assert_eq!(first.selected_track, next.selected_track);
        assert_eq!(next.evidence, EvidenceKind::Inferred);
        assert_eq!(next.position.unwrap().value(), 0.5);
    }
    #[test]
    fn full_length_attempt_ids_have_valid_namespaces() {
        assert!(StrokeTracker::new(&AttemptId::new("x".repeat(128)).unwrap()).is_ok());
    }
}
