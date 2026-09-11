//! Candidate selection preserves complete interpolation segments around protected spans.
use crate::{Axis, CoreError, MotionAction, MotionProgram, MotionTrack, ProjectTime, ProtectedRegion, TimeRange};
use std::collections::BTreeSet;

fn guard(track: Option<&MotionTrack>, range: TimeRange) -> (i64, i64) {
    let Some(track) = track else { return (0, i64::MAX); };
    let actions = track.actions();
    let left = actions.partition_point(|action| action.time() < range.start());
    let right = actions.partition_point(|action| action.time() < range.end());
    (
        (if actions.get(left).is_some_and(|action| action.time() == range.start()) {
            Some(left)
        } else {
            left.checked_sub(1)
        }).map(|i| actions[i].time().as_nanos()).unwrap_or(0),
        actions.get(right).map(|a| a.time().as_nanos()).unwrap_or(i64::MAX),
    )
}

fn local_actions(track: Option<&MotionTrack>, range: TimeRange) -> Vec<&MotionAction> {
    let Some(track) = track else { return vec![]; };
    let actions = track.actions();
    let start = actions.partition_point(|action| action.time() < range.start());
    let end = actions.partition_point(|action| action.time() < range.end());
    let first = if actions.get(start).is_some_and(|action| action.time() == range.start()) {
        start
    } else {
        start.saturating_sub(1)
    };
    let last = (end + usize::from(end < actions.len())).min(actions.len());
    actions[first..last].iter().collect()
}

fn local_gaps(track: Option<&MotionTrack>, range: TimeRange) -> Vec<(ProjectTime, ProjectTime)> {
    track.into_iter().flat_map(|track| track.gaps()).filter(|gap| gap.overlaps(range))
        .map(|gap| (gap.start().max(range.start()), gap.end().min(range.end()))).collect()
}

/// Dedicated local-edit proof. Equality includes neighbor anchors and evidence,
/// not merely points whose timestamps happen to fall inside a protected span.
pub fn ensure_protected_segments_unchanged(
    before: &MotionProgram,
    after: &MotionProgram,
    protected: &[ProtectedRegion],
) -> Result<(), CoreError> {
    for region in protected {
        for axis in Axis::ALL {
            if region.axis.is_some_and(|protected_axis| protected_axis != axis) { continue; }
            let old = before.track(axis);
            let new = after.track(axis);
            if local_actions(old, region.range) != local_actions(new, region.range)
                || local_gaps(old, region.range) != local_gaps(new, region.range)
            {
                return Err(CoreError::ProtectedRegion);
            }
        }
    }
    Ok(())
}

/// Merge selected axes/time while retaining protected boundary-neighbor segments.
/// The boundary neighborhoods are intentionally conservative: candidate points
/// adjacent to a protected span are skipped rather than altering its interpolation.
pub fn merge_candidate_preserving_protection(
    current: &MotionProgram,
    candidate: &MotionProgram,
    axes: &[Axis],
    range: Option<TimeRange>,
    protected: &[ProtectedRegion],
) -> Result<MotionProgram, CoreError> {
    if axes.is_empty() || axes.len() > Axis::ALL.len()
        || axes.iter().copied().collect::<BTreeSet<_>>().len() != axes.len()
        || range.is_some_and(|range| range.start() < ProjectTime::ZERO)
    {
        return Err(CoreError::InvalidMotion("invalid candidate selection".into()));
    }
    let mut result = current.clone();
    let fixed = selection_boundaries(axes, range, protected)?;    for axis in axes {
        let incoming = candidate.track(*axis)
            .ok_or_else(|| CoreError::InvalidMotion("selected axis is absent from candidate".into()))?;
        incoming.span()?;
        let old = current.track(*axis);
        if let Some(old) = old { old.span()?; }
        let guards: Vec<_> = fixed.iter()
            .filter(|region| region.axis.is_none() || region.axis == Some(*axis))
            .map(|region| guard(old, region.range)).collect();
        let writable = |time: ProjectTime| {
            range.is_none_or(|range| range.contains(time))
                && !guards.iter().any(|(start, end)| *start <= time.as_nanos() && time.as_nanos() <= *end)
        };
        let mut actions: Vec<_> = old.into_iter().flat_map(|track| track.actions())
            .filter(|action| !writable(action.time())).cloned().collect();
        actions.extend(incoming.actions().iter().filter(|action| writable(action.time())).cloned());
        actions.sort_by_key(|action| action.time());
        let mut gaps = Vec::new();
        for (track, retain_writable) in old.into_iter().map(|track| (track, false))
            .chain(std::iter::once((incoming, true)))
        {
            for gap in track.gaps() {
                let mut cuts = vec![gap.start().as_nanos(), gap.end().as_nanos()];
                if let Some(range) = range { cuts.extend([range.start().as_nanos(), range.end().as_nanos()]); }
                for (start, end) in &guards { cuts.extend([*start, end.saturating_add(1)]); }
                cuts.retain(|time| gap.start().as_nanos() <= *time && *time <= gap.end().as_nanos());
                cuts.sort_unstable();
                cuts.dedup();
                for interval in cuts.windows(2) {
                    let start = ProjectTime::from_nanos(interval[0]);
                    if writable(start) == retain_writable {
                        gaps.push(TimeRange::new(start, ProjectTime::from_nanos(interval[1]))?);
                    }
                }
            }
        }
        gaps.sort_by_key(|gap| gap.start());
        let mut combined: Vec<TimeRange> = Vec::new();
        for gap in gaps {
            if let Some(last) = combined.last_mut() {
                if gap.start() <= last.end() {
                    *last = TimeRange::new(last.start(), last.end().max(gap.end()))?;
                    continue;
                }
            }
            combined.push(gap);
        }
        let track = MotionTrack::with_name_and_gaps(
            old.map(|track| track.name()).unwrap_or_else(|| incoming.name()),
            *axis, actions, combined,
        )?;
        // Avoid creating a new empty axis when all selected content was protected.
        if old.is_some() || !track.actions().is_empty() || !track.gaps().is_empty() {
            result = result.replaced_track(track)?;
        }
    }
    ensure_protected_segments_unchanged(current, &result, &fixed)?;
    Ok(result)
}

fn selection_boundaries(
    axes: &[Axis], range: Option<TimeRange>, protected: &[ProtectedRegion],
) -> Result<Vec<ProtectedRegion>, CoreError> {
    let mut fixed = protected.to_vec();
    if let Some(range) = range {
        for axis in axes {
            if range.start() > ProjectTime::ZERO {
                fixed.push(ProtectedRegion { axis: Some(*axis), range: TimeRange::new(ProjectTime::ZERO, range.start())? });
            }
            if range.end().as_nanos() < i64::MAX {
                fixed.push(ProtectedRegion { axis: Some(*axis), range: TimeRange::new(range.end(), ProjectTime::from_nanos(i64::MAX))? });
            }
        }
    }
    Ok(fixed)
}

/// Conservative contribution envelopes using precisely the merge's writable
/// mask. They exclude protected anchors and omitted axes; an envelope may still
/// contain unchanged samples and is not a point-by-point provenance claim.
pub fn candidate_merge_writable_regions(
    current: &MotionProgram, candidate: &MotionProgram, axes: &[Axis],
    range: Option<TimeRange>, protected: &[ProtectedRegion],
) -> Result<Vec<(Axis, TimeRange)>, CoreError> {
    if axes.is_empty() || axes.len() > Axis::ALL.len()
        || axes.iter().copied().collect::<BTreeSet<_>>().len() != axes.len()
        || range.is_some_and(|range| range.start() < ProjectTime::ZERO)
    {
        return Err(CoreError::InvalidMotion("invalid candidate selection".into()));
    }
    let fixed = selection_boundaries(axes, range, protected)?;
    // A new point also changes interpolation toward retained boundary anchors.
    // Incoming-only support would under-report those ramps. Use the checked
    // merged signal's support, then clip it to the same conservative write mask.
    let merged = merge_candidate_preserving_protection(current, candidate, axes, range, protected)?;
    let mut regions = Vec::new();
    for axis in axes {
        let Some(track) = merged.track(*axis) else { continue; };
        let guards: Vec<_> = fixed.iter()
            .filter(|region| region.axis.is_none() || region.axis == Some(*axis))
            .map(|region| guard(current.track(*axis), region.range)).collect();
        let mut support: Vec<_> = track.span()?.into_iter().chain(track.gaps().iter().copied()).collect();
        support.sort_by_key(|span| span.start());
        let mut combined: Vec<TimeRange> = Vec::new();
        for span in support {
            if let Some(last) = combined.last_mut() {
                if span.start() <= last.end() {
                    *last = TimeRange::new(last.start(), last.end().max(span.end()))?;
                    continue;
                }
            }
            combined.push(span);
        }
        for span in combined {
            let mut cuts = vec![span.start().as_nanos(), span.end().as_nanos()];
            if let Some(range) = range { cuts.extend([range.start().as_nanos(), range.end().as_nanos()]); }
            for (start, end) in &guards { cuts.extend([*start, end.saturating_add(1)]); }
            cuts.retain(|time| span.start().as_nanos() <= *time && *time <= span.end().as_nanos());
            cuts.sort_unstable();
            cuts.dedup();
            for pair in cuts.windows(2) {
                let start = ProjectTime::from_nanos(pair[0]);
                if range.is_none_or(|range| range.contains(start))
                    && !guards.iter().any(|(a, b)| *a <= pair[0] && pair[0] <= *b)
                {
                    if regions.len() >= 1024 {
                        return Err(CoreError::InvalidMotion("too many merge contribution envelopes".into()));
                    }
                    regions.push((*axis, TimeRange::new(start, ProjectTime::from_nanos(pair[1]))?));
                }
            }
        }
    }
    Ok(regions)
}
