use pulsar_core::*;
use serde_json::{json, Value};

fn time(value: i64) -> ProjectTime {
    ProjectTime::from_nanos(value)
}
fn position(value: f64) -> NormalizedPosition {
    NormalizedPosition::new(value).unwrap()
}
fn range(start: i64, end: i64) -> TimeRange {
    TimeRange::new(time(start), time(end)).unwrap()
}
fn action(at: i64, value: f64) -> MotionAction {
    MotionAction::new(time(at), position(value), EvidenceKind::Observed).unwrap()
}
fn program(points: &[(i64, f64)]) -> MotionProgram {
    MotionProgram::new(vec![MotionTrack::with_name(
        "Trusted original name",
        Axis::Stroke,
        points
            .iter()
            .map(|&(at, value)| action(at, value))
            .collect(),
    )
    .unwrap()])
    .unwrap()
}
fn values(axis: Axis, points: &[(i64, f64)], gaps: Vec<TimeRange>) -> EditValuesProgram {
    EditValuesProgram::new(vec![EditValueTrack::new(
        axis,
        points
            .iter()
            .map(|&(at, value)| EditValueAction::new(time(at), position(value)).unwrap())
            .collect(),
        gaps,
    )
    .unwrap()])
    .unwrap()
}
fn reconcile(base: &MotionProgram, edit: &EditValuesProgram) -> ReconciledEdit {
    reconcile_edit_values(
        base,
        edit,
        &ProvenanceRef::new("engine:authored-receipt").unwrap(),
    )
    .unwrap()
}
fn covered(ranges: &[(Axis, TimeRange)], axis: Axis, at: i64) -> bool {
    ranges
        .iter()
        .any(|(candidate, span)| *candidate == axis && span.contains(time(at)))
}
fn wire() -> Value {
    json!({"tracks":[{"axis":"stroke","actions":[{"time":0,"position":0.5}],"gaps":[]}]})
}

#[test]
fn known_answer_wire_contains_values_only_and_round_trips() {
    let base = program(&[(0, 0.5)]);
    let edit = EditValuesProgram::from_program_values(&base).unwrap();
    assert_eq!(serde_json::to_value(&edit).unwrap(), wire());
    assert_eq!(
        serde_json::from_value::<EditValuesProgram>(wire()).unwrap(),
        edit
    );
    assert_eq!(EDIT_VALUES_CODEC, "edit-values-json-v1");
    assert_eq!(EDIT_VALUES_KERNEL_VERSION, "pulsar-edit-values-v1");
}

#[test]
fn hostile_authority_fields_rejected_at_every_object_boundary() {
    for key in [
        "evidence",
        "provenance",
        "lineage",
        "qualification",
        "model",
        "worker",
        "review",
    ] {
        for level in 0..3 {
            let mut payload = wire();
            let target = match level {
                0 => &mut payload,
                1 => &mut payload["tracks"][0],
                _ => &mut payload["tracks"][0]["actions"][0],
            };
            target
                .as_object_mut()
                .unwrap()
                .insert(key.to_owned(), json!("forged"));
            assert!(
                serde_json::from_value::<EditValuesProgram>(payload).is_err(),
                "{key}/{level}"
            );
        }
        let mut payload = wire();
        payload["tracks"][0]["gaps"] = json!([{"start":1,"end":2,key:"forged"}]);
        assert!(
            serde_json::from_value::<EditValuesProgram>(payload).is_err(),
            "gap/{key}"
        );
    }
    let mut named = wire();
    named["tracks"][0]["name"] = json!("spoofed trusted model output");
    assert!(serde_json::from_value::<EditValuesProgram>(named).is_err());
}

#[test]
fn gaps_are_required_and_unknown_duplicate_fields_fail() {
    let mut payload = wire();
    payload["tracks"][0].as_object_mut().unwrap().remove("gaps");
    assert!(serde_json::from_value::<EditValuesProgram>(payload).is_err());
    assert!(serde_json::from_str::<EditValuesProgram>(r#"{"tracks":[],"tracks":[]}"#).is_err());
    assert!(serde_json::from_str::<EditValuesProgram>(
        r#"{"tracks":[{"axis":"stroke","actions":[{"time":0,"time":1,"position":0.5}],"gaps":[]}]}"#
    )
    .is_err());
}

#[test]
fn checked_values_times_order_axes_and_gaps_cannot_be_bypassed_by_serde() {
    for bad in [
        json!({"axis":"stroke","actions":[{"time":-1,"position":0.5}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":9223372036854775807i64,"position":0.5}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":0,"position":1.1}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":0,"position":-0.1}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":0,"position":null}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":2,"position":0.1},{"time":1,"position":0.2}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":1,"position":0.1},{"time":1,"position":0.2}],"gaps":[]}),
        json!({"axis":"stroke","actions":[{"time":2,"position":0.5}],"gaps":[{"start":1,"end":3}]}),
        json!({"axis":"stroke","actions":[],"gaps":[{"start":-1,"end":1}]}),
        json!({"axis":"stroke","actions":[],"gaps":[{"start":2,"end":2}]}),
        json!({"axis":"stroke","actions":[],"gaps":[{"start":1,"end":4},{"start":3,"end":5}]}),
        json!({"axis":"stroke","actions":[],"gaps":[{"start":4,"end":5},{"start":1,"end":3}]}),
    ] {
        assert!(serde_json::from_value::<EditValuesProgram>(json!({"tracks":[bad]})).is_err());
    }
    let duplicate = wire()["tracks"][0].clone();
    assert!(serde_json::from_value::<EditValuesProgram>(
        json!({"tracks":[duplicate.clone(),duplicate]})
    )
    .is_err());
    assert!(EditValueAction::new(time(-1), position(0.5)).is_err());
    assert!(EditValueAction::new(time(i64::MAX), position(0.5)).is_err());
    assert!(NormalizedPosition::new(f64::NAN).is_err());
    assert!(NormalizedPosition::new(f64::INFINITY).is_err());
}

#[test]
fn unchanged_program_preserves_evidence_names_gaps_and_span_ancestry() {
    let mut actions = vec![action(0, 0.2), action(100, 0.3), action(200, 0.1)];
    actions[1] = MotionAction::new(time(100), position(0.3), EvidenceKind::Inferred).unwrap();
    let base = MotionProgram::new(vec![MotionTrack::with_name_and_gaps(
        "Original evidence",
        Axis::Stroke,
        actions,
        vec![range(120, 160)],
    )
    .unwrap()])
    .unwrap();
    let edit = EditValuesProgram::from_program_values(&base).unwrap();
    let result = reconcile(&base, &edit);
    assert_eq!(result.program, base);
    assert!(result.authored_ranges.is_empty());
    assert_eq!(result.inherited_ranges, vec![(Axis::Stroke, range(0, 201))]);
    assert_eq!(
        result.authored_provenance.as_str(),
        "engine:authored-receipt"
    );
}

#[test]
fn singleton_insertion_marks_both_adjacent_ramps_not_unchanged_anchors() {
    let base = program(&[(0, 0.1), (100, 0.2), (200, 0.3), (300, 0.4)]);
    let edit = values(
        Axis::Stroke,
        &[(0, 0.1), (100, 0.2), (150, 0.8), (200, 0.3), (300, 0.4)],
        vec![],
    );
    let result = reconcile(&base, &edit);
    assert_eq!(
        result.authored_ranges,
        vec![(Axis::Stroke, range(101, 200))]
    );
    assert!(covered(&result.authored_ranges, Axis::Stroke, 125));
    assert!(covered(&result.authored_ranges, Axis::Stroke, 175));
    assert!(!covered(&result.authored_ranges, Axis::Stroke, 100));
    assert!(!covered(&result.authored_ranges, Axis::Stroke, 200));
    let track = result.program.track(Axis::Stroke).unwrap();
    for action in track.actions() {
        assert_eq!(
            action.evidence(),
            if action.time() == time(150) {
                EvidenceKind::Synthesized
            } else {
                EvidenceKind::Observed
            }
        );
    }
    assert_eq!(track.name(), "Trusted original name");
    ensure_protected_segments_unchanged(
        &base,
        &result.program,
        &[
            ProtectedRegion {
                axis: Some(Axis::Stroke),
                range: range(0, 100),
            },
            ProtectedRegion {
                axis: Some(Axis::Stroke),
                range: range(200, 301),
            },
        ],
    )
    .unwrap();
}

#[test]
fn changed_and_deleted_knots_cover_interpolation_but_do_not_forge_evidence() {
    let base = program(&[(0, 0.1), (100, 0.2), (200, 0.3)]);
    for points in [
        vec![(0, 0.1), (100, 0.9), (200, 0.3)],
        vec![(0, 0.1), (200, 0.3)],
        vec![(0, 0.1), (110, 0.2), (200, 0.3)],
    ] {
        let result = reconcile(&base, &values(Axis::Stroke, &points, vec![]));
        assert_eq!(result.authored_ranges, vec![(Axis::Stroke, range(1, 200))]);
        for action in result.program.track(Axis::Stroke).unwrap().actions() {
            let same = base
                .track(Axis::Stroke)
                .unwrap()
                .actions()
                .iter()
                .any(|old| old.time() == action.time() && old.position() == action.position());
            assert_eq!(
                action.evidence(),
                if same {
                    EvidenceKind::Observed
                } else {
                    EvidenceKind::Synthesized
                }
            );
        }
    }
}

#[test]
fn collinear_added_knot_is_conservatively_authored_not_inherited_segment_proof() {
    let base = program(&[(0, 0.0), (100, 1.0)]);
    let result = reconcile(
        &base,
        &values(Axis::Stroke, &[(0, 0.0), (50, 0.5), (100, 1.0)], vec![]),
    );
    assert_eq!(result.authored_ranges, vec![(Axis::Stroke, range(1, 100))]);
    assert_eq!(
        result.program.track(Axis::Stroke).unwrap().actions()[1].evidence(),
        EvidenceKind::Synthesized
    );
}

#[test]
fn gap_addition_removal_and_gap_only_support_are_explicit_authored_changes() {
    let base = program(&[(0, 0.1), (100, 0.9)]);
    let with_gap = values(Axis::Stroke, &[(0, 0.1), (100, 0.9)], vec![range(40, 60)]);
    let added = reconcile(&base, &with_gap);
    assert_eq!(added.authored_ranges, vec![(Axis::Stroke, range(40, 60))]);
    assert!(added.program.has_unresolved_gaps());
    let removed = reconcile(
        &added.program,
        &EditValuesProgram::from_program_values(&base).unwrap(),
    );
    assert_eq!(removed.authored_ranges, vec![(Axis::Stroke, range(40, 60))]);
    let gap_only = values(Axis::Yaw, &[], vec![range(10, 20), range(20, 30)]);
    let gap_result = reconcile(&MotionProgram::default(), &gap_only);
    assert_eq!(gap_result.authored_ranges, vec![(Axis::Yaw, range(10, 30))]);
    assert!(gap_result.program.has_unresolved_gaps());
    let cleared = reconcile(&gap_result.program, &EditValuesProgram::default());
    assert_eq!(cleared.authored_ranges, vec![(Axis::Yaw, range(10, 30))]);
}

#[test]
fn axis_identity_is_not_transferred_and_deletion_is_recorded() {
    let base = program(&[(0, 0.2), (100, 0.8)]);
    let result = reconcile(&base, &values(Axis::Yaw, &[(0, 0.2), (100, 0.8)], vec![]));
    assert_eq!(
        result.authored_ranges,
        vec![(Axis::Stroke, range(0, 101)), (Axis::Yaw, range(0, 101))]
    );
    assert!(result.inherited_ranges.is_empty());
    assert!(result.program.track(Axis::Stroke).is_none());
    let yaw = result.program.track(Axis::Yaw).unwrap();
    assert_eq!(yaw.name(), "yaw");
    assert!(yaw
        .actions()
        .iter()
        .all(|action| action.evidence() == EvidenceKind::Synthesized));
}

#[test]
fn terminal_integer_boundary_is_checked_without_panics() {
    let edge = values(Axis::Stroke, &[(i64::MAX - 1, 0.4)], vec![]);
    let result = reconcile(&MotionProgram::default(), &edge);
    assert_eq!(
        result.authored_ranges,
        vec![(Axis::Stroke, range(i64::MAX - 1, i64::MAX))]
    );
    let bad_base = program(&[(i64::MAX, 0.5)]);
    assert!(reconcile_edit_values(
        &bad_base,
        &EditValuesProgram::default(),
        &ProvenanceRef::new("receipt").unwrap()
    )
    .is_err());
}

#[test]
fn six_axis_order_is_canonical_and_values_are_not_stretched() {
    let tracks = Axis::ALL
        .into_iter()
        .rev()
        .map(|axis| {
            EditValueTrack::new(
                axis,
                vec![
                    EditValueAction::new(time(0), position(0.49)).unwrap(),
                    EditValueAction::new(time(5), position(0.51)).unwrap(),
                ],
                vec![],
            )
            .unwrap()
        })
        .collect();
    let edits = EditValuesProgram::new(tracks).unwrap();
    assert_eq!(
        edits
            .tracks()
            .iter()
            .map(EditValueTrack::axis)
            .collect::<Vec<_>>(),
        Axis::ALL
    );
    let result = reconcile(&MotionProgram::default(), &edits);
    for track in result.program.tracks() {
        assert_eq!(track.actions()[0].position().value(), 0.49);
        assert_eq!(track.actions()[1].position().value(), 0.51);
    }
}

#[test]
fn aggregate_action_cap_is_checked_across_axes() {
    let per_axis = EDIT_VALUES_MAX_ACTIONS / 2 + 1;
    let make_track = |axis| {
        EditValueTrack::new(
            axis,
            (0..per_axis)
                .map(|at| EditValueAction::new(time(at as i64), position(0.5)).unwrap())
                .collect(),
            vec![],
        )
        .unwrap()
    };
    assert!(EditValuesProgram::new(vec![make_track(Axis::Stroke), make_track(Axis::Yaw)]).is_err());
    let too_many =
        vec![EditValueAction::new(time(0), position(0.5)).unwrap(); EDIT_VALUES_MAX_ACTIONS + 1];
    assert!(EditValueTrack::new(Axis::Stroke, too_many, vec![]).is_err());
}

fn sample(program: &MotionProgram, at: i64) -> Option<f64> {
    let track = program.track(Axis::Stroke)?;
    if track.gaps().iter().any(|gap| gap.contains(time(at))) {
        return None;
    }
    let actions = track.actions();
    let index = actions.partition_point(|action| action.time() <= time(at));
    if index == 0 {
        return None;
    }
    let left = &actions[index - 1];
    if left.time() == time(at) {
        return Some(left.position().value());
    }
    let right = actions.get(index)?;
    let fraction = (at - left.time().as_nanos()) as f64
        / (right.time().as_nanos() - left.time().as_nanos()) as f64;
    Some(left.position().value() + (right.position().value() - left.position().value()) * fraction)
}

#[test]
fn generated_mutations_cover_every_changed_sample_and_inherited_ranges_are_disjoint() {
    for seed in 0..128 {
        let base = program(&[(0, 0.2), (16, 0.3), (32, 0.4), (48, 0.6), (64, 0.7)]);
        let mut points = vec![(0, 0.2), (16, 0.3), (32, 0.4), (48, 0.6), (64, 0.7)];
        match seed % 4 {
            0 => {
                points[2].1 = (seed % 11) as f64 / 10.0;
            }
            1 => {
                points.insert(2, (17 + (seed % 14), 0.8));
            }
            2 => {
                points.remove(2);
            }
            _ => {
                points[2].0 = 33 + (seed % 10);
            }
        }
        let result = reconcile(&base, &values(Axis::Stroke, &points, vec![]));
        for at in 0..66 {
            let before = sample(&base, at);
            let after = sample(&result.program, at);
            let same = match (before, after) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-12,
                (None, None) => true,
                _ => false,
            };
            if !same {
                assert!(
                    covered(&result.authored_ranges, Axis::Stroke, at),
                    "seed={seed},at={at}"
                );
            }
            assert!(
                !(covered(&result.authored_ranges, Axis::Stroke, at)
                    && covered(&result.inherited_ranges, Axis::Stroke, at))
            );
            if covered(&result.inherited_ranges, Axis::Stroke, at) {
                assert!(same, "inherited changed sample seed={seed},at={at}");
            }
        }
        let no_op = reconcile(
            &result.program,
            &EditValuesProgram::from_program_values(&result.program).unwrap(),
        );
        assert_eq!(no_op.program, result.program);
        assert!(no_op.authored_ranges.is_empty());
    }
}

#[test]
fn six_axis_full_detail_324000_actions_reconcile_without_thinning() {
    let tracks = Axis::ALL
        .into_iter()
        .map(|axis| {
            EditValueTrack::new(
                axis,
                (0..54_000i64)
                    .map(|at| {
                        EditValueAction::new(
                            time(at * 20_000_000),
                            position(0.49 + (at % 3) as f64 / 100.0),
                        )
                        .unwrap()
                    })
                    .collect(),
                vec![],
            )
            .unwrap()
        })
        .collect();
    let edits = EditValuesProgram::new(tracks).unwrap();
    let result = reconcile(&MotionProgram::default(), &edits);
    assert_eq!(
        result
            .program
            .tracks()
            .iter()
            .map(|track| track.actions().len())
            .sum::<usize>(),
        324_000
    );
    assert_eq!(result.authored_ranges.len(), 6);
    assert!(result.inherited_ranges.is_empty());
    let unchanged = reconcile(
        &result.program,
        &EditValuesProgram::from_program_values(&result.program).unwrap(),
    );
    assert!(unchanged.authored_ranges.is_empty());
    assert_eq!(unchanged.inherited_ranges.len(), 6);
}
