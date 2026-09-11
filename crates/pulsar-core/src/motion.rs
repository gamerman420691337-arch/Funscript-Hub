use crate::{CoreError, ProjectTime, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Stroke,
    Sway,
    Surge,
    Roll,
    Pitch,
    Yaw,
}

impl Axis {
    pub const ALL: [Self; 6] = [
        Self::Stroke,
        Self::Sway,
        Self::Surge,
        Self::Roll,
        Self::Pitch,
        Self::Yaw,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stroke => "stroke",
            Self::Sway => "sway",
            Self::Surge => "surge",
            Self::Roll => "roll",
            Self::Pitch => "pitch",
            Self::Yaw => "yaw",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Observed,
    Predicted,
    Inferred,
    Synthesized,
    Unavailable,
}

/// Device-neutral position. Conversion to physical units belongs to explicit
/// profile adaptation; missing axes are not implicit zero-valued tracks.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct NormalizedPosition(f64);

impl NormalizedPosition {
    pub fn new(value: f64) -> Result<Self, CoreError> {
        if !value.is_finite() {
            return Err(CoreError::NonFinite);
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(CoreError::OutOfRange);
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
    pub const fn value(self) -> f64 {
        self.0
    }
}
impl TryFrom<f64> for NormalizedPosition {
    type Error = CoreError;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<NormalizedPosition> for f64 {
    fn from(value: NormalizedPosition) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MotionActionWire", into = "MotionActionWire")]
pub struct MotionAction {
    time: ProjectTime,
    position: NormalizedPosition,
    evidence: EvidenceKind,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionActionWire {
    time: ProjectTime,
    position: NormalizedPosition,
    evidence: EvidenceKind,
}

impl MotionAction {
    pub fn new(
        time: ProjectTime,
        position: NormalizedPosition,
        evidence: EvidenceKind,
    ) -> Result<Self, CoreError> {
        if time < ProjectTime::ZERO {
            return Err(CoreError::InvalidMotion(
                "motion action time must be nonnegative".into(),
            ));
        }
        if evidence == EvidenceKind::Unavailable {
            return Err(CoreError::InvalidMotion(
                "unavailable motion must remain a gap, not an action".into(),
            ));
        }
        Ok(Self {
            time,
            position,
            evidence,
        })
    }
    pub const fn time(&self) -> ProjectTime {
        self.time
    }
    pub const fn position(&self) -> NormalizedPosition {
        self.position
    }
    pub const fn evidence(&self) -> EvidenceKind {
        self.evidence
    }
}
impl TryFrom<MotionActionWire> for MotionAction {
    type Error = CoreError;
    fn try_from(v: MotionActionWire) -> Result<Self, Self::Error> {
        Self::new(v.time, v.position, v.evidence)
    }
}
impl From<MotionAction> for MotionActionWire {
    fn from(v: MotionAction) -> Self {
        Self {
            time: v.time,
            position: v.position,
            evidence: v.evidence,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MotionTrackWire", into = "MotionTrackWire")]
pub struct MotionTrack {
    name: String,
    axis: Axis,
    actions: Vec<MotionAction>,
    gaps: Vec<TimeRange>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionTrackWire {
    name: String,
    axis: Axis,
    actions: Vec<MotionAction>,
    #[serde(default)]
    gaps: Vec<TimeRange>,
}

impl MotionTrack {
    pub fn new(axis: Axis, actions: Vec<MotionAction>) -> Result<Self, CoreError> {
        Self::with_name(axis.as_str(), axis, actions)
    }
    pub fn with_name(
        name: impl Into<String>,
        axis: Axis,
        actions: Vec<MotionAction>,
    ) -> Result<Self, CoreError> {
        Self::with_name_and_gaps(name, axis, actions, vec![])
    }
    pub fn with_gaps(
        axis: Axis,
        actions: Vec<MotionAction>,
        gaps: Vec<TimeRange>,
    ) -> Result<Self, CoreError> {
        Self::with_name_and_gaps(axis.as_str(), axis, actions, gaps)
    }
    pub fn with_name_and_gaps(
        name: impl Into<String>,
        axis: Axis,
        actions: Vec<MotionAction>,
        gaps: Vec<TimeRange>,
    ) -> Result<Self, CoreError> {
        let name = name.into();
        if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(CoreError::InvalidMotion(
                "track name must be nonempty, at most 128 bytes, and contain no control characters"
                    .into(),
            ));
        }
        if actions.windows(2).any(|pair| pair[0].time >= pair[1].time) {
            return Err(CoreError::InvalidMotion(
                "action times must be strictly increasing".into(),
            ));
        }
        if gaps.iter().any(|gap| gap.start() < ProjectTime::ZERO)
            || gaps.windows(2).any(|pair| pair[0].end() > pair[1].start())
        {
            return Err(CoreError::InvalidMotion(
                "gaps must be nonnegative, ordered, and disjoint".into(),
            ));
        }
        let mut gap_index = 0;
        for action in &actions {
            while gap_index < gaps.len() && gaps[gap_index].end() <= action.time {
                gap_index += 1;
            }
            if gap_index < gaps.len() && gaps[gap_index].contains(action.time) {
                return Err(CoreError::InvalidMotion(
                    "motion action cannot occupy unavailable interval".into(),
                ));
            }
        }
        Ok(Self {
            name,
            axis,
            actions,
            gaps,
        })
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn axis(&self) -> Axis {
        self.axis
    }
    pub fn actions(&self) -> &[MotionAction] {
        &self.actions
    }
    /// Consumers must not interpolate across these explicitly unavailable
    /// intervals. Standard exports without gap semantics must reject unresolved
    /// gaps, unless the user separately accepts a documented resolution.
    pub fn gaps(&self) -> &[TimeRange] {
        &self.gaps
    }
    pub fn span(&self) -> Result<Option<TimeRange>, CoreError> {
        match (self.actions.first(), self.actions.last()) {
            (Some(first), Some(last)) => {
                TimeRange::new(first.time, last.time.checked_add(1)?).map(Some)
            }
            _ => Ok(None),
        }
    }
}
impl TryFrom<MotionTrackWire> for MotionTrack {
    type Error = CoreError;
    fn try_from(v: MotionTrackWire) -> Result<Self, Self::Error> {
        Self::with_name_and_gaps(v.name, v.axis, v.actions, v.gaps)
    }
}
impl From<MotionTrack> for MotionTrackWire {
    fn from(v: MotionTrack) -> Self {
        Self {
            name: v.name,
            axis: v.axis,
            actions: v.actions,
            gaps: v.gaps,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MotionProgramWire", into = "MotionProgramWire")]
pub struct MotionProgram {
    tracks: Vec<MotionTrack>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionProgramWire {
    tracks: Vec<MotionTrack>,
}

impl MotionProgram {
    pub fn new(mut tracks: Vec<MotionTrack>) -> Result<Self, CoreError> {
        if tracks.len() > 6 {
            return Err(CoreError::InvalidMotion(
                "one neutral program supports at most six axes".into(),
            ));
        }
        let mut axes = BTreeSet::new();
        if tracks.iter().any(|track| !axes.insert(track.axis)) {
            return Err(CoreError::InvalidMotion(
                "each axis appears at most once".into(),
            ));
        }
        tracks.sort_by_key(|track| track.axis);
        Ok(Self { tracks })
    }
    pub fn tracks(&self) -> &[MotionTrack] {
        &self.tracks
    }
    pub fn has_unresolved_gaps(&self) -> bool {
        self.tracks.iter().any(|track| !track.gaps.is_empty())
    }
    pub fn track(&self, axis: Axis) -> Option<&MotionTrack> {
        self.tracks.iter().find(|track| track.axis == axis)
    }
    pub fn replaced_track(&self, replacement: MotionTrack) -> Result<Self, CoreError> {
        let mut tracks: Vec<_> = self
            .tracks
            .iter()
            .filter(|track| track.axis != replacement.axis)
            .cloned()
            .collect();
        tracks.push(replacement);
        Self::new(tracks)
    }
}
impl TryFrom<MotionProgramWire> for MotionProgram {
    type Error = CoreError;
    fn try_from(v: MotionProgramWire) -> Result<Self, Self::Error> {
        Self::new(v.tracks)
    }
}
impl From<MotionProgram> for MotionProgramWire {
    fn from(v: MotionProgram) -> Self {
        Self { tracks: v.tracks }
    }
}
