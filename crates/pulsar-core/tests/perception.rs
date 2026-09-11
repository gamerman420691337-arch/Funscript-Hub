use pulsar_core::perception::*;
use pulsar_core::{
    CoordinateBox, CoordinateSpace, EvidenceKind, FrameContext, FrameId, SourcePlacementId,
    SourceVersionId, TransformId,
};

fn context(frame: u64) -> FrameContext {
    FrameContext {
        source_version: SourceVersionId::new("source").unwrap(),
        source_placement: SourcePlacementId::new("placement").unwrap(),
        frame: FrameId::new(frame),
        transform: TransformId::new("transform").unwrap(),
        seek_generation: 0,
        request_generation: frame,
    }
}
fn detection(x: f64, class: u32) -> CanonicalDetection {
    CanonicalDetection::new(
        CoordinateBox::new(CoordinateSpace::SourcePixels, x, 20.0, x + 10.0, 35.0).unwrap(),
        class,
        0.9,
    )
    .unwrap()
}
fn transform() -> DetectorTransform {
    DetectorTransform::resize(100, 80, 100, 80).unwrap()
}
fn decode(
    v: &[f32],
    shape: &[usize],
    classes: usize,
) -> Result<Vec<CanonicalDetection>, pulsar_core::CoreError> {
    decode_detections(
        v,
        shape,
        DetectorLayout::YoloChannelMajor,
        classes,
        &transform(),
        0.25,
        0.45,
    )
}
#[test]
fn detector_rejects_rank_count_and_negative_geometry() {
    assert!(decode(&[50.0, 40.0, 20.0, 20.0, 0.9], &[5, 1], 1).is_err());
    assert!(decode(&[50.0, 40.0, 20.0, 20.0], &[1, 5, 1], 1).is_err());
    assert!(decode(&[50.0, 40.0, -20.0, 20.0, 0.9], &[1, 5, 1], 1).is_err());
}
#[test]
fn detector_rejects_nonfinite_even_below_threshold() {
    assert!(decode(&[f32::NAN, 40.0, 20.0, 20.0, 0.01], &[1, 5, 1], 1).is_err());
    assert!(decode(&[50.0, 40.0, 20.0, 20.0, f32::INFINITY], &[1, 5, 1], 1).is_err());
}
#[test]
fn detector_clips_endpoints_and_recomputes_center() {
    let result = decode(&[1.0, 40.0, 20.0, 20.0, 0.9], &[1, 5, 1], 1).unwrap();
    assert_eq!(result[0].bounds().x_min(), 0.0);
    assert_eq!(result[0].bounds().x_max(), 11.0);
    assert_eq!(result[0].bounds().center().0, 5.5);
}
#[test]
fn detector_drops_fully_outside_and_validates_classes() {
    assert!(decode(&[-50.0, 40.0, 20.0, 20.0, 0.9], &[1, 5, 1], 1)
        .unwrap()
        .is_empty());
    assert!(decode(&[], &[1, 4, 0], 0).is_err());
    assert!(decode(&[50.0, 40.0, 20.0, 20.0, 1.01], &[1, 5, 1], 1).is_err());
}
#[test]
fn detector_inverts_explicit_letterbox() {
    let mapping = DetectorTransform::new(100, 50, 100, 100, [1.0, 1.0], [0.0, 25.0]).unwrap();
    let result = decode_detections(
        &[50.0, 50.0, 20.0, 20.0, 0.9],
        &[1, 5, 1],
        DetectorLayout::YoloChannelMajor,
        1,
        &mapping,
        0.25,
        0.45,
    )
    .unwrap();
    assert_eq!(result[0].bounds().y_min(), 15.0);
    assert!(DetectorTransform::new(100, 50, 100, 100, [1.0, 1.0], [50.0, 0.0]).is_err());
}
#[test]
fn detector_nms_is_class_aware_and_deterministic() {
    let values = [
        50., 50., 50., 40., 40., 40., 20., 20., 20., 20., 20., 20., 0.9, 0.8, 0.1, 0.1, 0.1, 0.85,
    ];
    let result = decode(&values, &[1, 6, 3], 2).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].class_id(), 0);
    assert_eq!(result[1].class_id(), 1);
}
#[test]
fn detector_rows_contract_rejects_fractional_class_and_bad_shape() {
    let run = |values: &[f32]| {
        decode_detections(
            values,
            &[1, 1, 6],
            DetectorLayout::YoloEndToEnd,
            2,
            &transform(),
            0.25,
            0.45,
        )
    };
    assert!(run(&[10., 10., 30., 30., 0.9, 0.5]).is_err());
    assert!(run(&[10., 10., 30., 30., 0.9, 2.]).is_err());
    assert_eq!(
        run(&[10., 10., 30., 30., 0.9, 1.]).unwrap()[0].class_id(),
        1
    );
}
#[test]
fn tracker_keeps_identity_with_permuted_detections_and_moves_boxes() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(
            context(0),
            100,
            80,
            &[detection(10., 0), detection(70., 1)],
            false,
        )
        .unwrap();
    let second = tracker
        .update(
            context(1),
            100,
            80,
            &[detection(68., 1), detection(12., 0)],
            false,
        )
        .unwrap();
    for observation in &first.observations {
        let next = second
            .observations
            .iter()
            .find(|next| next.track() == observation.track())
            .unwrap();
        assert_ne!(next.bounds(), observation.bounds());
        assert_eq!(next.context(), &context(1));
        assert_eq!(next.evidence(), EvidenceKind::Observed);
    }
}
#[test]
fn tracker_prediction_translates_box_not_only_center() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    tracker
        .update(context(0), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    tracker
        .update(context(1), 100, 80, &[detection(12., 0)], false)
        .unwrap();
    let missing = tracker.update(context(2), 100, 80, &[], false).unwrap();
    assert_eq!(missing.observations[0].bounds().x_min(), 14.0);
    assert_eq!(missing.observations[0].bounds().x_max(), 24.0);
    assert_eq!(missing.observations[0].evidence(), EvidenceKind::Predicted);
    assert!(missing.reviews.contains(&TrackingReview::MissingDetection));
}
#[test]
fn tracker_counts_skipped_frames_when_expiring() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(context(0), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    assert!(tracker
        .update(context(30), 100, 80, &[], false)
        .unwrap()
        .observations
        .is_empty());
    let next = tracker
        .update(context(31), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    assert_ne!(first.observations[0].track(), next.observations[0].track());
}
#[test]
fn tracker_cut_retires_identity_even_for_same_geometry() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(context(0), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    let next = tracker
        .update(context(1), 100, 80, &[detection(10., 0)], true)
        .unwrap();
    assert_ne!(first.observations[0].track(), next.observations[0].track());
    assert!(next.reviews.contains(&TrackingReview::SceneCut));
}
#[test]
fn tracker_seek_and_transform_reset_identity() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(context(10), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    let mut changed = context(0);
    changed.seek_generation = 1;
    let next = tracker
        .update(changed, 100, 80, &[detection(10., 0)], false)
        .unwrap();
    assert_ne!(first.observations[0].track(), next.observations[0].track());
    let mut changed = context(0);
    changed.transform = TransformId::new("new-transform").unwrap();
    let third = tracker
        .update(changed, 100, 80, &[detection(10., 0)], false)
        .unwrap();
    assert_ne!(next.observations[0].track(), third.observations[0].track());
}
#[test]
fn tracker_rejects_out_of_order_without_mutating_state() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(context(5), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    assert!(tracker
        .update(context(4), 100, 80, &[detection(40., 0)], false)
        .is_err());
    let next = tracker
        .update(context(6), 100, 80, &[detection(11., 0)], false)
        .unwrap();
    assert_eq!(first.observations[0].track(), next.observations[0].track());
}
#[test]
fn tracker_ambiguous_association_flags_review_instead_of_arbitrary_identity() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    tracker
        .update(
            context(0),
            100,
            80,
            &[detection(20., 0), detection(40., 0)],
            false,
        )
        .unwrap();
    let next = tracker
        .update(context(1), 100, 80, &[detection(30., 0)], false)
        .unwrap();
    assert!(next.reviews.contains(&TrackingReview::AmbiguousAssociation));
}
fn texture(width: usize, height: usize) -> Vec<u8> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| ((x * 37 + y * 61 + x * y * 7) % 251) as u8))
        .collect()
}
fn shift(source: &[u8], w: usize, h: usize, dx: isize, dy: isize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let sx = x as isize - dx;
                let sy = y as isize - dy;
                if sx >= 0 && sy >= 0 && sx < w as isize && sy < h as isize {
                    source[sy as usize * w + sx as usize]
                } else {
                    0
                }
            })
        })
        .collect()
}
#[test]
fn flow_rejects_shape_mismatch_and_nonfinite_points() {
    assert!(track_points_fb(&[0; 64], &[0; 63], 8, 8, &[], FlowConfig::default()).is_err());
    assert!(track_points_fb(
        &[0; 64],
        &[0; 64],
        8,
        8,
        &[[f64::NAN, 4.]],
        FlowConfig::default()
    )
    .is_err());
}
#[test]
fn flow_zero_texture_is_missing_not_observed_pause() {
    let result = track_points_fb(
        &[40; 4096],
        &[40; 4096],
        64,
        64,
        &[[32., 32.]],
        FlowConfig::default(),
    )
    .unwrap();
    assert_eq!(result[0].rejection, Some(FlowRejection::LowTexture));
    assert!(result[0].displacement.is_none());
}
#[test]
fn flow_independent_reverse_pass_accepts_translation() {
    let old = texture(64, 64);
    let new = shift(&old, 64, 64, 3, -2);
    let result = track_points_fb(&old, &new, 64, 64, &[[32., 32.]], FlowConfig::default()).unwrap();
    assert!(result[0].rejection.is_none(), "{:?}", result[0]);
    let delta = result[0].displacement.unwrap();
    assert!(
        (delta[0] - 3.).abs() < 0.05 && (delta[1] + 2.).abs() < 0.05,
        "{delta:?}"
    );
    assert!(result[0].forward_backward_error.unwrap() < 0.05);
}
#[test]
fn flow_stationary_texture_is_valid_zero() {
    let old = texture(64, 64);
    let result = track_points_fb(&old, &old, 64, 64, &[[32., 32.]], FlowConfig::default()).unwrap();
    assert_eq!(result[0].displacement, Some([0., 0.]));
}
#[test]
fn flow_occlusion_rejects_instead_of_clamping_point_to_edge() {
    let old = texture(64, 64);
    let result = track_points_fb(
        &old,
        &[0; 4096],
        64,
        64,
        &[[32., 32.], [0., 0.]],
        FlowConfig::default(),
    )
    .unwrap();
    assert!(result[0].displacement.is_none());
    assert_eq!(result[1].rejection, Some(FlowRejection::OutOfBounds));
}
#[test]
fn flow_translation_reversal_is_metamorphic() {
    let old = texture(64, 64);
    let new = shift(&old, 64, 64, -2, 1);
    let fw = track_points_fb(&old, &new, 64, 64, &[[30., 30.]], FlowConfig::default()).unwrap();
    let bw = track_points_fb(&new, &old, 64, 64, &[[28., 31.]], FlowConfig::default()).unwrap();
    let a = fw[0].displacement.unwrap();
    let b = bw[0].displacement.unwrap();
    assert!((a[0] + b[0]).abs() < 0.05 && (a[1] + b[1]).abs() < 0.05);
}
#[test]
fn flow_budgets_and_config_are_checked() {
    let cfg = FlowConfig {
        max_iterations: 0,
        ..FlowConfig::default()
    };
    assert!(track_points_fb(&[0; 4096], &[0; 4096], 64, 64, &[], cfg).is_err());
    assert!(track_points_fb(
        &[0; 4096],
        &[0; 4096],
        64,
        64,
        &vec![[32., 32.]; MAX_FLOW_POINTS + 1],
        FlowConfig::default()
    )
    .is_err());
}
#[test]
fn relative_motion_requires_background_evidence() {
    let old = texture(64, 64);
    let moved = shift(&old, 64, 64, 2, 3);
    let points = [[20., 20.], [32., 32.], [44., 44.]];
    let target = track_points_fb(&old, &moved, 64, 64, &points, FlowConfig::default()).unwrap();
    let missing = separate_target_motion(&target, &[]).unwrap();
    assert!(missing.target_relative_to_background.is_none());
    let same = separate_target_motion(&target, &target).unwrap();
    assert_eq!(same.target_relative_to_background, Some([0., 0.]));
    assert!(same.target_image_translation.unwrap()[1] > 2.9);
}

#[test]
fn m1_unknown_class_does_not_panic() {
    let mut values = vec![0.0; 15];
    values[..4].copy_from_slice(&[50., 40., 20., 20.]);
    values[14] = 0.9;
    assert_eq!(decode(&values, &[1, 15, 1], 11).unwrap()[0].class_id(), 10);
}
#[test]
fn m1_direct_rejects_reversed_corners() {
    assert!(decode_detections(
        &[80., 20., 10., 60., 0.9, 0.],
        &[1, 1, 6],
        DetectorLayout::YoloEndToEnd,
        1,
        &transform(),
        0.5,
        0.45
    )
    .is_err());
}
#[test]
fn m1_direct_clips_corners_without_guessing_center_format() {
    let result = decode_detections(
        &[-10., 20., 20., 60., 0.9, 0.],
        &[1, 1, 6],
        DetectorLayout::YoloEndToEnd,
        1,
        &transform(),
        0.5,
        0.45,
    )
    .unwrap();
    assert_eq!(result[0].bounds().x_min(), 0.0);
    assert_eq!(result[0].bounds().x_max(), 20.0);
}
#[test]
fn m1_direct_rejects_nonfinite_score() {
    assert!(decode_detections(
        &[20., 20., 40., 40., f32::INFINITY, 0.],
        &[1, 1, 6],
        DetectorLayout::YoloEndToEnd,
        1,
        &transform(),
        0.5,
        0.45
    )
    .is_err());
}
#[test]
fn m1_confidence_change_does_not_replace_spatial_match() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(context(0), 100, 80, &[detection(25., 2)], false)
        .unwrap();
    let mut distant = detection(80., 2);
    distant = CanonicalDetection::new(distant.bounds().clone(), 2, 0.99).unwrap();
    let close = CanonicalDetection::new(detection(25., 2).bounds().clone(), 2, 0.7).unwrap();
    let next = tracker
        .update(context(1), 100, 80, &[distant, close], false)
        .unwrap();
    let same = next
        .observations
        .iter()
        .find(|observation| observation.track() == first.observations[0].track())
        .unwrap();
    assert_eq!(same.bounds().x_min(), 25.0);
}
#[test]
fn m1_degenerate_boxes_do_not_seed_points() {
    assert!(CoordinateBox::new(CoordinateSpace::SourcePixels, 90., 20., 10., 80.).is_err());
    assert!(CoordinateBox::new(CoordinateSpace::SourcePixels, 100., 100., 100., 100.).is_err());
    let bounds = CoordinateBox::new(CoordinateSpace::SourcePixels, 10., 20., 30., 40.).unwrap();
    let points = seed_points_in_box(&bounds, 100, 100, 4).unwrap();
    assert_eq!(points.len(), 4);
    assert!(points
        .iter()
        .all(|p| p[0] > 10. && p[0] < 30. && p[1] > 20. && p[1] < 40.));
}
#[test]
fn m1_zero_requested_mask_points_do_not_panic() {
    assert!(seed_points_in_mask(&[255; 4], 2, 2, 0).unwrap().is_empty());
    assert!(seed_points_in_mask(&[255; 3], 2, 2, 0).is_err());
}
#[test]
fn m1_equal_area_resize_updates_flow_geometry() {
    for (width, height, point) in [(64, 32, [32., 16.]), (32, 64, [16., 32.])] {
        let old = texture(width, height);
        let new = shift(&old, width, height, 0, 1);
        let result = track_points_fb(
            &old,
            &new,
            width as u32,
            height as u32,
            &[point],
            FlowConfig::default(),
        )
        .unwrap();
        assert_eq!(result[0].displacement, Some([0., 1.]));
    }
}
#[test]
fn distinct_attempt_namespaces_never_share_track_identity() {
    let mut first = TargetTracker::with_namespace(
        pulsar_core::EntityTrackId::new("attempt.first").unwrap(),
        TrackerConfig::default(),
    )
    .unwrap();
    let mut second = TargetTracker::with_namespace(
        pulsar_core::EntityTrackId::new("attempt.second").unwrap(),
        TrackerConfig::default(),
    )
    .unwrap();
    let a = first
        .update(context(0), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    let b = second
        .update(context(0), 100, 80, &[detection(10., 0)], false)
        .unwrap();
    assert_ne!(a.observations[0].track(), b.observations[0].track());
}

#[test]
fn sampled_frame_stride_is_not_an_observed_absence() {
    let mut tracker = TargetTracker::new(TrackerConfig::default()).unwrap();
    let first = tracker
        .update(context(0), 100, 80, &[detection(20., 0)], false)
        .unwrap();
    let next = tracker
        .update(context(32), 100, 80, &[detection(21., 0)], false)
        .unwrap();
    assert_eq!(first.observations[0].track(), next.observations[0].track());
    assert_eq!(next.observations[0].evidence(), EvidenceKind::Observed);
}
