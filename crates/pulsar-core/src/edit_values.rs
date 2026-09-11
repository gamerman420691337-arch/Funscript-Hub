//! Values-only edits are untrusted input, never observation or provenance authority.
//!
//! A motion action's evidence describes its knot, not every interpolated sample.
//! Identical axis/time/position knots retain their base evidence. New or changed
//! knots are synthesized. Span ancestry is separate: only identical interpolation
//! stencils (or unchanged explicit gaps/knots) inherit base span lineage.
//!
//! Returned authored ranges conservatively include structural interpolation
//! changes even when two different stencils happen to draw the same line. They
//! also include deleted support. They must not be inferred from knot evidence.

use crate::{
    Axis, CoreError, EvidenceKind, MotionAction, MotionProgram, MotionTrack, NormalizedPosition,
    ProjectTime, ProvenanceRef, TimeRange,
};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::fmt;

pub const EDIT_VALUES_CODEC: &str = "edit-values-json-v1";
pub const EDIT_VALUES_KERNEL_VERSION: &str = "pulsar-edit-values-v1";
pub const EDIT_VALUES_MAX_AXES: usize = 6;
pub const EDIT_VALUES_MAX_ACTIONS: usize = 1_000_000;
pub const EDIT_VALUES_MAX_GAPS: usize = 1_000_000;
// Prevent adversarial fragmentation from creating an unbounded receipt.
// Engines may enforce a much smaller publication limit, without dropping ranges.
pub const EDIT_VALUES_MAX_RANGES: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ActionWire")]
pub struct EditValueAction {
    time: ProjectTime,
    position: NormalizedPosition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionWire {
    time: ProjectTime,
    position: NormalizedPosition,
}

impl TryFrom<ActionWire> for EditValueAction {
    type Error = CoreError;

    fn try_from(value: ActionWire) -> Result<Self, Self::Error> {
        Self::new(value.time, value.position)
    }
}

impl EditValueAction {
    pub fn new(time: ProjectTime, position: NormalizedPosition) -> Result<Self, CoreError> {
        if time.as_nanos() < 0 {
            return Err(invalid("edit action time must be nonnegative"));
        }
        time.checked_add(1)?;
        Ok(Self { time, position })
    }

    pub fn time(&self) -> ProjectTime {
        self.time
    }

    pub fn position(&self) -> NormalizedPosition {
        self.position
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TrackWire")]
pub struct EditValueTrack {
    axis: Axis,
    actions: Vec<EditValueAction>,
    gaps: Vec<TimeRange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackWire {
    axis: Axis,
    #[serde(deserialize_with = "deserialize_actions")]
    actions: Vec<EditValueAction>,
    // Required: absence is not implicit permission to erase uncertainty.
    #[serde(deserialize_with = "deserialize_gaps")]
    gaps: Vec<TimeRange>,
}

impl TryFrom<TrackWire> for EditValueTrack {
    type Error = CoreError;

    fn try_from(value: TrackWire) -> Result<Self, Self::Error> {
        Self::new(value.axis, value.actions, value.gaps)
    }
}

impl EditValueTrack {
    pub fn new(
        axis: Axis,
        actions: Vec<EditValueAction>,
        gaps: Vec<TimeRange>,
    ) -> Result<Self, CoreError> {
        if actions.len() > EDIT_VALUES_MAX_ACTIONS || gaps.len() > EDIT_VALUES_MAX_GAPS {
            return Err(invalid("edit track exceeds the action or gap limit"));
        }
        if actions.windows(2).any(|pair| pair[0].time >= pair[1].time) {
            return Err(invalid("edit action times must be strictly increasing"));
        }
        let mut previous_end = None;
        for gap in &gaps {
            if gap.start().as_nanos() < 0 || previous_end.is_some_and(|end| gap.start() < end) {
                return Err(invalid(
                    "edit gaps must be nonnegative, ordered and disjoint",
                ));
            }
            previous_end = Some(gap.end());
        }
        let mut gap_index = 0;
        for action in &actions {
            while gap_index < gaps.len() && gaps[gap_index].end() <= action.time {
                gap_index += 1;
            }
            if gap_index < gaps.len() && gaps[gap_index].contains(action.time) {
                return Err(invalid("edit action lies inside an unavailable gap"));
            }
        }
        Ok(Self {
            axis,
            actions,
            gaps,
        })
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    pub fn actions(&self) -> &[EditValueAction] {
        &self.actions
    }

    pub fn gaps(&self) -> &[TimeRange] {
        &self.gaps
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ProgramWire")]
pub struct EditValuesProgram {
    tracks: Vec<EditValueTrack>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgramWire {
    #[serde(deserialize_with = "deserialize_tracks")]
    tracks: Vec<EditValueTrack>,
}

impl TryFrom<ProgramWire> for EditValuesProgram {
    type Error = CoreError;

    fn try_from(value: ProgramWire) -> Result<Self, Self::Error> {
        Self::new(value.tracks)
    }
}

impl EditValuesProgram {
    pub fn new(mut tracks: Vec<EditValueTrack>) -> Result<Self, CoreError> {
        if tracks.len() > EDIT_VALUES_MAX_AXES {
            return Err(invalid("edit program exceeds six axes"));
        }
        let mut actions = 0usize;
        let mut gaps = 0usize;
        for (index, track) in tracks.iter().enumerate() {
            if tracks[..index].iter().any(|other| other.axis == track.axis) {
                return Err(invalid("edit program has a duplicate axis"));
            }
            actions = actions
                .checked_add(track.actions.len())
                .ok_or(CoreError::Overflow)?;
            gaps = gaps
                .checked_add(track.gaps.len())
                .ok_or(CoreError::Overflow)?;
        }
        if actions > EDIT_VALUES_MAX_ACTIONS || gaps > EDIT_VALUES_MAX_GAPS {
            return Err(invalid(
                "edit program exceeds aggregate action or gap limit",
            ));
        }
        tracks.sort_by_key(|track| {
            Axis::ALL
                .iter()
                .position(|axis| *axis == track.axis)
                .unwrap_or(usize::MAX)
        });
        Ok(Self { tracks })
    }

    pub fn tracks(&self) -> &[EditValueTrack] {
        &self.tracks
    }

    /// Strip all authority-bearing metadata from a client-side draft.
    ///
    /// Track names are not accepted from an upload. Existing engine names are
    /// preserved during reconciliation; new tracks receive their axis name.
    pub fn from_program_values(program: &MotionProgram) -> Result<Self, CoreError> {
        validate_base(program)?;
        Self::new(
            program
                .tracks()
                .iter()
                .map(|track| {
                    EditValueTrack::new(
                        track.axis(),
                        track
                            .actions()
                            .iter()
                            .map(|action| EditValueAction::new(action.time(), action.position()))
                            .collect::<Result<_, _>>()?,
                        track.gaps().to_vec(),
                    )
                })
                .collect::<Result<_, CoreError>>()?,
        )
    }
}

/// Trusted result of reconciliation. Intentionally not deserializable.
///
/// Ranges are half-open integer-nanosecond coverage, sorted by axis/time.
/// Authored and inherited ranges never overlap. A point at t occupies [t,t+1).
/// An unchanged knot does not confer evidence on its changed adjacent ramps.
/// This kernel does not authorize a commit, bypass protection, or resolve review.
#[derive(Clone, Debug, PartialEq)]
pub struct ReconciledEdit {
    pub program: MotionProgram,
    pub authored_ranges: Vec<(Axis, TimeRange)>,
    pub inherited_ranges: Vec<(Axis, TimeRange)>,
    pub authored_provenance: ProvenanceRef,
}

pub fn reconcile_edit_values(
    base: &MotionProgram,
    values: &EditValuesProgram,
    authored: &ProvenanceRef,
) -> Result<ReconciledEdit, CoreError> {
    validate_base(base)?;
    let mut tracks = Vec::with_capacity(values.tracks.len());
    for track in &values.tracks {
        let old = base.track(track.axis);
        let old_actions = old.map(MotionTrack::actions).unwrap_or(&[]);
        let mut old_index = 0;
        let mut actions = Vec::with_capacity(track.actions.len());
        for value in &track.actions {
            while old_index < old_actions.len() && old_actions[old_index].time() < value.time {
                old_index += 1;
            }
            let evidence = old_actions
                .get(old_index)
                .filter(|action| action.time() == value.time && action.position() == value.position)
                .map(MotionAction::evidence)
                .unwrap_or(EvidenceKind::Synthesized);
            actions.push(MotionAction::new(value.time, value.position, evidence)?);
        }
        tracks.push(MotionTrack::with_name_and_gaps(
            old.map(MotionTrack::name).unwrap_or(track.axis.as_str()),
            track.axis,
            actions,
            track.gaps.clone(),
        )?);
    }
    let program = MotionProgram::new(tracks)?;
    let mut authored_ranges = Vec::new();
    let mut inherited_ranges = Vec::new();
    for axis in Axis::ALL {
        compare_axis(
            axis,
            base.track(axis),
            program.track(axis),
            &mut authored_ranges,
            &mut inherited_ranges,
        )?;
    }
    Ok(ReconciledEdit {
        program,
        authored_ranges,
        inherited_ranges,
        authored_provenance: authored.clone(),
    })
}

fn invalid(message: &str) -> CoreError {
    CoreError::InvalidMotion(message.to_owned())
}

fn validate_base(base: &MotionProgram) -> Result<(), CoreError> {
    let mut actions = 0usize;
    let mut gaps = 0usize;
    for track in base.tracks() {
        actions = actions
            .checked_add(track.actions().len())
            .ok_or(CoreError::Overflow)?;
        gaps = gaps
            .checked_add(track.gaps().len())
            .ok_or(CoreError::Overflow)?;
        // A terminal knot needs a representable half-open coverage endpoint.
        track.span()?;
    }
    if actions > EDIT_VALUES_MAX_ACTIONS || gaps > EDIT_VALUES_MAX_GAPS {
        return Err(invalid("base program exceeds edit reconciliation limits"));
    }
    Ok(())
}

fn deserialize_bounded<'de, D, T, const LIMIT: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const LIMIT: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const LIMIT: usize> de::Visitor<'de> for Bounded<T, LIMIT> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "an array containing at most {LIMIT} entries")
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == LIMIT {
                    return Err(de::Error::custom("edit array limit exceeded"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, LIMIT>(std::marker::PhantomData))
}

fn deserialize_actions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<EditValueAction>, D::Error> {
    deserialize_bounded::<D, EditValueAction, EDIT_VALUES_MAX_ACTIONS>(deserializer)
}

fn deserialize_tracks<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<EditValueTrack>, D::Error> {
    deserialize_bounded::<D, EditValueTrack, EDIT_VALUES_MAX_AXES>(deserializer)
}

fn deserialize_gaps<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<TimeRange>, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GapWire {
        start: ProjectTime,
        end: ProjectTime,
    }
    let gaps = deserialize_bounded::<D, GapWire, EDIT_VALUES_MAX_GAPS>(deserializer)?;
    gaps.into_iter()
        .map(|gap| TimeRange::new(gap.start, gap.end).map_err(de::Error::custom))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cell {
    Absent,
    Gap,
    Knot(i64, NormalizedPosition),
    Segment(i64, NormalizedPosition, i64, NormalizedPosition),
}

/// Incremental cursor, so comparison is linear and does not allocate a timeline
/// proportional to the union of every knot and gap boundary.
struct TrackCursor<'a> {
    actions: &'a [MotionAction],
    gaps: &'a [TimeRange],
    action: usize,
    gap: usize,
}

impl<'a> TrackCursor<'a> {
    fn new(track: Option<&'a MotionTrack>) -> Self {
        Self {
            actions: track.map(MotionTrack::actions).unwrap_or(&[]),
            gaps: track.map(MotionTrack::gaps).unwrap_or(&[]),
            action: 0,
            gap: 0,
        }
    }

    fn cell(&mut self, time: i64) -> Cell {
        while self.action < self.actions.len() && self.actions[self.action].time().as_nanos() < time
        {
            self.action += 1;
        }
        while self.gap < self.gaps.len() && self.gaps[self.gap].end().as_nanos() <= time {
            self.gap += 1;
        }
        if self.gap < self.gaps.len() && self.gaps[self.gap].start().as_nanos() <= time {
            return Cell::Gap;
        }
        if let Some(action) = self.actions.get(self.action) {
            if action.time().as_nanos() == time {
                return Cell::Knot(time, action.position());
            }
            if self.action > 0 {
                let left = &self.actions[self.action - 1];
                return Cell::Segment(
                    left.time().as_nanos(),
                    left.position(),
                    action.time().as_nanos(),
                    action.position(),
                );
            }
        }
        Cell::Absent
    }
}

fn compare_axis(
    axis: Axis,
    before: Option<&MotionTrack>,
    after: Option<&MotionTrack>,
    authored: &mut Vec<(Axis, TimeRange)>,
    inherited: &mut Vec<(Axis, TimeRange)>,
) -> Result<(), CoreError> {
    let old_actions = before.map(MotionTrack::actions).unwrap_or(&[]);
    let new_actions = after.map(MotionTrack::actions).unwrap_or(&[]);
    let old_gaps = before.map(MotionTrack::gaps).unwrap_or(&[]);
    let new_gaps = after.map(MotionTrack::gaps).unwrap_or(&[]);
    // All four streams are individually ordered. checked +1 was established
    // before this sweep, including the immutable base.
    let mut old_knots = old_actions
        .iter()
        .flat_map(|action| [action.time().as_nanos(), action.time().as_nanos() + 1])
        .peekable();
    let mut new_knots = new_actions
        .iter()
        .flat_map(|action| [action.time().as_nanos(), action.time().as_nanos() + 1])
        .peekable();
    let mut old_gap_edges = old_gaps
        .iter()
        .flat_map(|gap| [gap.start().as_nanos(), gap.end().as_nanos()])
        .peekable();
    let mut new_gap_edges = new_gaps
        .iter()
        .flat_map(|gap| [gap.start().as_nanos(), gap.end().as_nanos()])
        .peekable();
    let mut previous = None;
    let mut old_cursor = TrackCursor::new(before);
    let mut new_cursor = TrackCursor::new(after);
    loop {
        let next = [
            old_knots.peek().copied(),
            new_knots.peek().copied(),
            old_gap_edges.peek().copied(),
            new_gap_edges.peek().copied(),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(next) = next else { break };
        if let Some(start) = previous {
            if start < next {
                let old = old_cursor.cell(start);
                let new = new_cursor.cell(start);
                if old != Cell::Absent || new != Cell::Absent {
                    append_range(
                        if old == new {
                            &mut *inherited
                        } else {
                            &mut *authored
                        },
                        axis,
                        start,
                        next,
                    )?;
                }
            }
        }
        while old_knots.peek().copied() == Some(next) {
            old_knots.next();
        }
        while new_knots.peek().copied() == Some(next) {
            new_knots.next();
        }
        while old_gap_edges.peek().copied() == Some(next) {
            old_gap_edges.next();
        }
        while new_gap_edges.peek().copied() == Some(next) {
            new_gap_edges.next();
        }
        previous = Some(next);
    }
    Ok(())
}

fn append_range(
    ranges: &mut Vec<(Axis, TimeRange)>,
    axis: Axis,
    start: i64,
    end: i64,
) -> Result<(), CoreError> {
    let start = ProjectTime::from_nanos(start);
    let end = ProjectTime::from_nanos(end);
    if let Some((last_axis, last)) = ranges.last_mut() {
        if *last_axis == axis && last.end() == start {
            *last = TimeRange::new(last.start(), end)?;
            return Ok(());
        }
    }
    if ranges.len() == EDIT_VALUES_MAX_RANGES {
        return Err(invalid("edit lineage fragmentation exceeds range limit"));
    }
    ranges.push((axis, TimeRange::new(start, end)?));
    Ok(())
}
