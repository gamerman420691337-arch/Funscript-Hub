//! Non-authoritative gesture drafts. Only the engine commits the selected axis.
use anyhow::Result;
use pulsar_protocol::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EditorPoint { pub nanos: i64, pub position: f64 }

pub(crate) fn stroke_points(program: &MotionProgram) -> Vec<EditorPoint> {
    points_for_axis(program, Axis::Stroke)
}

pub(crate) fn axis_points(program: &MotionProgram, axis: Axis) -> Vec<EditorPoint> {
    if axis == Axis::Stroke { stroke_points(program) } else { points_for_axis(program, axis) }
}

fn points_for_axis(program: &MotionProgram, axis: Axis) -> Vec<EditorPoint> {
    program.track(axis).map(|track| track.actions().iter().map(|action| EditorPoint {
        nanos: action.time().as_nanos(), position: action.position().value(),
    }).collect()).unwrap_or_default()
}

pub(crate) fn replace_stroke(program: &MotionProgram, points: &[EditorPoint]) -> Result<MotionProgram> {
    replace_track(program, Axis::Stroke, points)
}

pub(crate) fn replace_axis(program: &MotionProgram, axis: Axis, points: &[EditorPoint]) -> Result<MotionProgram> {
    if axis == Axis::Stroke { replace_stroke(program, points) } else { replace_track(program, axis, points) }
}

fn replace_track(program: &MotionProgram, axis: Axis, points: &[EditorPoint]) -> Result<MotionProgram> {
    let original = program.track(axis);
    let mut points = points.to_vec();
    points.sort_by_key(|point| point.nanos);
    let actions = points.iter().map(|point| {
        // Unchanged points retain evidence; authored points are never observations.
        let evidence = original.and_then(|track| track.actions().binary_search_by_key(&point.nanos,
            |action| action.time().as_nanos()).ok().map(|index| &track.actions()[index]))
            .filter(|action| action.position().value() == point.position)
            .map(|action| action.evidence()).unwrap_or(EvidenceKind::Synthesized);
        Ok(MotionAction::new(ProjectTime::from_nanos(point.nanos),
            NormalizedPosition::new(point.position)?, evidence)?)
    }).collect::<Result<Vec<_>>>()?;
    let default_name = format!("{axis:?}");
    let track = MotionTrack::with_name_and_gaps(
        original.map(|track| track.name()).unwrap_or(&default_name), axis, actions,
        original.map(|track| track.gaps().to_vec()).unwrap_or_default())?;
    let mut tracks: Vec<_> = program.tracks().iter().filter(|track| track.axis() != axis).cloned().collect();
    tracks.push(track);
    Ok(MotionProgram::new(tracks)?)
}

pub(crate) struct EditGesture {
    pub project_id: ProjectId,
    pub base_revision: RevisionId,
    pub base_program: Arc<MotionProgram>,
    pub axis: Axis,
    pub points: Vec<EditorPoint>,
    pub selected: usize,
}

impl EditGesture {
    pub fn finish(self) -> Result<(EditProposal, ProjectId, RevisionId)> {
        let program = replace_axis(&self.base_program, self.axis, &self.points)?;
        Ok((EditProposal { program, label: format!("Move {:?} keyframe", self.axis) },
            self.project_id, self.base_revision))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gesture_stays_draft_and_commits_against_original_revision() {
        let original = MotionProgram::default();
        let gesture = EditGesture { project_id: ProjectId::new("project").unwrap(),
            base_revision: RevisionId::new(4), base_program: original.clone().into(), axis: Axis::Stroke,
            points: vec![EditorPoint { nanos: 10_000_000, position: 0.2 }], selected: 0 };
        let (command, _, revision) = gesture.finish().unwrap();
        assert_eq!(revision.value(), 4);
        assert!(original.tracks().is_empty());
        let EditProposal { program, .. } = command;
        assert_eq!(program.track(Axis::Stroke).unwrap().actions()[0].evidence(), EvidenceKind::Synthesized);
    }

    #[test]
    fn duplicate_gesture_timestamps_are_not_silently_sanitized() {
        assert!(replace_stroke(&MotionProgram::default(), &[
            EditorPoint { nanos: 0, position: 0.2 }, EditorPoint { nanos: 0, position: 0.8 },
        ]).is_err());
    }

    #[test]
    fn selecting_secondary_axis_preserves_other_tracks() {
        let stroke = replace_stroke(&MotionProgram::default(), &[EditorPoint { nanos: 0, position: 0.3 }]).unwrap();
        let yaw = replace_axis(&stroke, Axis::Yaw, &[EditorPoint { nanos: 0, position: 0.8 }]).unwrap();
        assert_eq!(stroke_points(&yaw), stroke_points(&stroke));
        assert_eq!(axis_points(&yaw, Axis::Yaw), vec![EditorPoint { nanos: 0, position: 0.8 }]);
        assert_eq!(yaw.track(Axis::Yaw).unwrap().actions()[0].evidence(), EvidenceKind::Synthesized);
    }
}


use crate::motion::EditProposal;
use std::sync::Arc;

/// Emits exact visible pieces without allocating a second full program.
/// Checked gaps are ordered/disjoint; the forward cursor visits each once.
pub(crate) fn available_segments(
    points: &[EditorPoint], gaps: &[TimeRange], mut emit: impl FnMut([EditorPoint; 2]),
) {
    // A dragged knot may temporarily cross its neighbor. The forward gap
    // cursor is valid only for strictly ordered drafts; never bridge a gap
    // while that ordering is invalid. Point handles remain available.
    if points.windows(2).any(|pair| pair[1].nanos <= pair[0].nanos) { return; }
    let mut gap_index = 0;
    for pair in points.windows(2) {
        let [start, end] = [pair[0], pair[1]];
        if end.nanos <= start.nanos { continue; }
        while gap_index < gaps.len() && gaps[gap_index].end().as_nanos() <= start.nanos { gap_index += 1; }
        let at = |nanos| {
            let fraction = (i128::from(nanos) - i128::from(start.nanos)) as f64
                / (i128::from(end.nanos) - i128::from(start.nanos)) as f64;
            EditorPoint { nanos, position: start.position + (end.position - start.position) * fraction }
        };
        let mut cursor = start.nanos;
        let mut index = gap_index;
        while index < gaps.len() && gaps[index].start().as_nanos() < end.nanos {
            let gap_start = gaps[index].start().as_nanos().max(start.nanos);
            let gap_end = gaps[index].end().as_nanos().min(end.nanos);
            if gap_start > cursor { emit([at(cursor), at(gap_start)]); }
            cursor = cursor.max(gap_end);
            if gaps[index].end().as_nanos() <= end.nanos { index += 1; } else { break; }
        }
        gap_index = index;
        if cursor < end.nanos { emit([at(cursor), end]); }
    }
}
pub(crate) fn point_is_available(nanos: i64, gaps: &[TimeRange]) -> bool {
    let index = gaps.partition_point(|gap| gap.end().as_nanos() <= nanos);
    gaps.get(index).is_none_or(|gap| nanos < gap.start().as_nanos())
}

#[cfg(test)]
mod gap_tests {
    use super::*;
    fn gap(start: i64, end: i64) -> TimeRange {
        TimeRange::new(ProjectTime::from_nanos(start), ProjectTime::from_nanos(end)).unwrap()
    }
    fn points() -> [EditorPoint; 2] {
        [EditorPoint { nanos: 0, position: 0.0 }, EditorPoint { nanos: 100, position: 1.0 }]
    }
    fn segments(gaps: &[TimeRange]) -> Vec<[EditorPoint; 2]> {
        let mut segments = Vec::new(); available_segments(&points(), gaps, |segment| segments.push(segment)); segments
    }
    #[test]
    fn single_gap_clips_only_unavailable_interval_with_interpolated_boundaries() {
        let segments = segments(&[gap(40, 60)]);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], [points()[0], EditorPoint { nanos: 40, position: 0.4 }]);
        assert_eq!(segments[1], [EditorPoint { nanos: 60, position: 0.6 }, points()[1]]);
    }
    #[test]
    fn multiple_and_endpoint_gaps_never_bridge_or_extrapolate() {
        let spans: Vec<_> = segments(&[gap(0, 10), gap(20, 40), gap(90, 110)])
            .into_iter().map(|segment| (segment[0].nanos, segment[1].nanos)).collect();
        assert_eq!(spans, vec![(10, 20), (40, 90)]);
        assert!(segments(&[gap(0, 100)]).is_empty());
        assert!(!point_is_available(40, &[gap(40, 60)]));
        assert!(point_is_available(60, &[gap(40, 60)]));
    }
    #[test]
    fn edit_draft_preserves_explicit_gaps_and_full_large_action_detail() {
        let actions: Vec<_> = (0..54_000).map(|index| MotionAction::new(
            ProjectTime::from_nanos(index * 1_000_000), NormalizedPosition::new(0.5).unwrap(), EvidenceKind::Observed).unwrap()).collect();
        let original = MotionProgram::new(vec![MotionTrack::with_name_and_gaps("stroke", Axis::Stroke, actions,
            vec![gap(40_200_000, 40_800_000)]).unwrap()]).unwrap();
        let mut draft = axis_points(&original, Axis::Stroke);
        draft[0].position = 0.4;
        let edited = replace_axis(&original, Axis::Stroke, &draft).unwrap();
        assert_eq!(edited.track(Axis::Stroke).unwrap().gaps(), original.track(Axis::Stroke).unwrap().gaps());
        assert_eq!(edited.track(Axis::Stroke).unwrap().actions().len(), 54_000);
        assert_eq!(edited.track(Axis::Stroke).unwrap().actions()[53_999].time().as_nanos(), 53_999_000_000);
    }
    #[test]
    fn large_gap_clipping_visits_full_detail_without_bridging() {
        let points: Vec<_> = (0..54_000).map(|nanos| EditorPoint { nanos, position: 0.5 }).collect();
        let gaps: Vec<_> = (0..5_399).map(|index| gap(index * 10 + 4, index * 10 + 5)).collect();
        let mut count = 0;
        available_segments(&points, &gaps, |segment| { assert_eq!(segment[1].nanos - segment[0].nanos, 1); count += 1; });
        assert_eq!(count, points.len() - 1 - gaps.len());
    }
}


#[cfg(test)]
mod unordered_gap_tests {
    use super::*;
    #[test]
    fn transient_crossing_or_duplicate_knots_never_render_continuous_gap_motion() {
        let gap = TimeRange::new(ProjectTime::from_nanos(50), ProjectTime::from_nanos(60)).unwrap();
        for times in [[0, 80, 40, 100], [0, 40, 40, 100]] {
            let points = times.map(|nanos| EditorPoint { nanos, position: 0.5 });
            let mut rendered = Vec::new();
            available_segments(&points, std::slice::from_ref(&gap), |segment| rendered.push(segment));
            assert!(rendered.is_empty());
        }
    }
}
