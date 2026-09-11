//! Constrained, deterministic synthesis. This is not a language model or a
//! semantic media detector. Every generated action is explicitly synthesized.
use crate::{
    Axis, CoreError, EvidenceKind, MotionAction, MotionProgram, MotionTrack, NormalizedPosition,
    ProjectTime, TimeRange,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SYNTHESIS_ACTIONS: usize = 1_000_000;
pub const MAX_SYNTHESIS_DURATION_NS: i64 = 86_400_000_000_000;
pub const MAX_SYNTHESIS_SAMPLE_HZ: u32 = 1_000;
pub const MAX_SYNTHESIS_FREQUENCY_HZ: f64 = 50.0;

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidMotion(message.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternKind {
    Sine,
    Triangle,
    Hold,
    Pulse,
}

/// Untrusted construction parameters. Only PatternSpec is a checked plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternParameters {
    pub pattern: PatternKind,
    pub axis: Axis,
    pub duration: ProjectTime,
    pub frequency_hz: f64,
    /// Peak deviation around offset, never an automatic range normalization.
    pub amplitude: f64,
    pub offset: NormalizedPosition,
    /// Fraction of one cycle in [0, 1).
    pub phase_cycles: f64,
    pub sample_hz: u32,
    /// High fraction of a pulse cycle. Other patterns require the default 0.5.
    pub duty_cycle: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PatternParameters", into = "PatternParameters")]
pub struct PatternSpec {
    parameters: PatternParameters,
}

impl PatternSpec {
    pub fn new(parameters: PatternParameters) -> Result<Self, CoreError> {
        check_duration(parameters.duration)?;
        if ![
            parameters.frequency_hz,
            parameters.amplitude,
            parameters.phase_cycles,
            parameters.duty_cycle,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            return Err(CoreError::NonFinite);
        }
        if parameters.sample_hz == 0 || parameters.sample_hz > MAX_SYNTHESIS_SAMPLE_HZ {
            return Err(invalid("sample_hz must be 1..=1000"));
        }
        if parameters.amplitude < 0.0
            || parameters.offset.value() - parameters.amplitude < 0.0
            || parameters.offset.value() + parameters.amplitude > 1.0
        {
            return Err(invalid(
                "symmetric amplitude must remain within neutral [0,1]",
            ));
        }
        if !(0.0..1.0).contains(&parameters.phase_cycles) {
            return Err(invalid("phase must be in [0,1) cycles"));
        }
        if parameters.pattern == PatternKind::Hold {
            if parameters.frequency_hz != 0.0
                || parameters.amplitude != 0.0
                || parameters.phase_cycles != 0.0
            {
                return Err(invalid(
                    "hold requires zero frequency, amplitude, and phase",
                ));
            }
        } else if parameters.frequency_hz <= 0.0
            || parameters.frequency_hz > MAX_SYNTHESIS_FREQUENCY_HZ
            || f64::from(parameters.sample_hz) < 4.0 * parameters.frequency_hz
        {
            return Err(invalid(
                "frequency must be in (0,50] Hz with at least four samples per cycle",
            ));
        }
        if parameters.pattern == PatternKind::Pulse {
            if parameters.duty_cycle <= 0.0 || parameters.duty_cycle >= 1.0 {
                return Err(invalid("pulse duty must be in (0,1)"));
            }
            let shortest_plateau_samples = f64::from(parameters.sample_hz)
                / parameters.frequency_hz
                * parameters.duty_cycle.min(1.0 - parameters.duty_cycle);
            if shortest_plateau_samples < 2.0 {
                return Err(invalid(
                    "pulse high and low plateaus each require at least two sample intervals",
                ));
            }
        } else if parameters.duty_cycle != 0.5 {
            return Err(invalid("duty is only configurable for pulse patterns"));
        }
        let spec = Self { parameters };
        if spec.action_count()? > MAX_SYNTHESIS_ACTIONS {
            return Err(invalid("synthesis exceeds the action budget"));
        }
        Ok(spec)
    }

    pub fn axis(&self) -> Axis {
        self.parameters.axis
    }

    pub fn parameters(&self) -> &PatternParameters {
        &self.parameters
    }

    pub fn action_count(&self) -> Result<usize, CoreError> {
        if self.parameters.pattern == PatternKind::Hold {
            return Ok(2);
        }
        sampled_action_count(self.parameters.duration, self.parameters.sample_hz)
    }
}

impl TryFrom<PatternParameters> for PatternSpec {
    type Error = CoreError;
    fn try_from(value: PatternParameters) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PatternSpec> for PatternParameters {
    fn from(value: PatternSpec) -> Self {
        value.parameters
    }
}

/// Grammar is whitespace-separated key=value fields, not natural language:
///
/// pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5
///
/// Required: pattern, axis, duration, frequency, amplitude, offset.
/// Optional: phase=0 sample_hz=100 duty=0.5. Unknown and duplicate keys fail.
pub fn parse_pattern_prompt(prompt: &str) -> Result<PatternSpec, CoreError> {
    if prompt.is_empty() || prompt.len() > 4096 || !prompt.is_ascii() {
        return Err(invalid(
            "preset prompt must be nonempty ASCII and at most 4096 bytes",
        ));
    }
    let mut fields = BTreeMap::new();
    for token in prompt.split_ascii_whitespace() {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| invalid("preset fields must use key=value"))?;
        if !matches!(
            key,
            "pattern"
                | "axis"
                | "duration"
                | "frequency"
                | "amplitude"
                | "offset"
                | "phase"
                | "sample_hz"
                | "duty"
        ) || value.is_empty()
            || fields.insert(key, value).is_some()
        {
            return Err(invalid("unknown, empty, or duplicate preset field"));
        }
    }
    let required = |name| {
        fields
            .get(name)
            .copied()
            .ok_or_else(|| invalid(format!("missing preset field: {name}")))
    };
    let pattern = match required("pattern")? {
        "sine" => PatternKind::Sine,
        "triangle" => PatternKind::Triangle,
        "hold" => PatternKind::Hold,
        "pulse" => PatternKind::Pulse,
        _ => return Err(invalid("unsupported preset pattern")),
    };
    let axis = match required("axis")? {
        "stroke" => Axis::Stroke,
        "sway" => Axis::Sway,
        "surge" => Axis::Surge,
        "roll" => Axis::Roll,
        "pitch" => Axis::Pitch,
        "yaw" => Axis::Yaw,
        _ => return Err(invalid("unsupported motion axis")),
    };
    let frequency = required("frequency")?
        .strip_suffix("hz")
        .ok_or_else(|| invalid("frequency requires lowercase hz suffix"))?;
    let sample_hz = match fields.get("sample_hz") {
        Some(value) if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => value
            .parse::<u32>()
            .map_err(|_| invalid("sample_hz overflow"))?,
        Some(_) => return Err(invalid("sample_hz must be an unsigned decimal integer")),
        None => 100,
    };
    PatternSpec::new(PatternParameters {
        pattern,
        axis,
        duration: parse_duration(required("duration")?)?,
        frequency_hz: parse_decimal(frequency)?,
        amplitude: parse_decimal(required("amplitude")?)?,
        offset: NormalizedPosition::new(parse_decimal(required("offset")?)?)?,
        phase_cycles: parse_decimal(fields.get("phase").copied().unwrap_or("0"))?,
        sample_hz,
        duty_cycle: parse_decimal(fields.get("duty").copied().unwrap_or("0.5"))?,
    })
}

/// Still-image synthesis requires an explicit supported prompt. Image pixels
/// alone never imply motion, and this parser does not inspect image content.
pub fn validate_still_image_prompt(prompt: Option<&str>) -> Result<PatternSpec, CoreError> {
    parse_pattern_prompt(
        prompt
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| invalid("still-image synthesis requires an explicit preset prompt"))?,
    )
}

pub fn synthesize_pattern(spec: &PatternSpec) -> Result<MotionProgram, CoreError> {
    MotionProgram::new(vec![synthesize_track(spec)?])
}

/// Six aligned axes at most. Preflight checks aggregate allocation before any
/// track is generated; duplicate axes or mismatched master durations fail.
pub fn synthesize_patterns(specs: &[PatternSpec]) -> Result<MotionProgram, CoreError> {
    if specs.is_empty() || specs.len() > 6 {
        return Err(invalid("synthesis requires one through six unique axes"));
    }
    let mut axes = BTreeSet::new();
    let mut action_count = 0_usize;
    for spec in specs {
        if !axes.insert(spec.parameters.axis)
            || spec.parameters.duration != specs[0].parameters.duration
        {
            return Err(invalid(
                "synthesis axes must be unique and share one duration",
            ));
        }
        action_count = action_count
            .checked_add(spec.action_count()?)
            .ok_or(CoreError::Overflow)?;
    }
    if action_count > MAX_SYNTHESIS_ACTIONS {
        return Err(invalid("combined synthesis exceeds the action budget"));
    }
    MotionProgram::new(
        specs
            .iter()
            .map(synthesize_track)
            .collect::<Result<Vec<_>, _>>()?,
    )
}

fn synthesize_track(spec: &PatternSpec) -> Result<MotionTrack, CoreError> {
    let p = &spec.parameters;
    let count = spec.action_count()?;
    let mut actions = Vec::with_capacity(count);
    if p.pattern == PatternKind::Hold {
        actions.push(synthesized_action(ProjectTime::ZERO, p.offset.value())?);
        actions.push(synthesized_action(p.duration, p.offset.value())?);
        return MotionTrack::new(p.axis, actions);
    }
    for index in 0..count {
        let time = if index == count - 1 {
            p.duration
        } else {
            ProjectTime::from_nanos(
                i64::try_from((index as i128) * 1_000_000_000 / i128::from(p.sample_hz))
                    .map_err(|_| CoreError::Overflow)?,
            )
        };
        let phase = (p.phase_cycles + p.frequency_hz * (time.as_nanos() as f64 / 1_000_000_000.0))
            .rem_euclid(1.0);
        let wave = match p.pattern {
            PatternKind::Sine => (std::f64::consts::TAU * phase).sin(),
            PatternKind::Triangle if phase < 0.25 => 4.0 * phase,
            PatternKind::Triangle if phase < 0.75 => 2.0 - 4.0 * phase,
            PatternKind::Triangle => 4.0 * phase - 4.0,
            PatternKind::Pulse if phase < p.duty_cycle => 1.0,
            PatternKind::Pulse => -1.0,
            PatternKind::Hold => 0.0,
        };
        actions.push(synthesized_action(
            time,
            p.offset.value() + p.amplitude * wave,
        )?);
    }
    MotionTrack::new(p.axis, actions)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AudioEnvelopeWire", into = "AudioEnvelopeWire")]
pub struct AudioEnvelopeSample {
    time: ProjectTime,
    level: NormalizedPosition,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioEnvelopeWire {
    time: ProjectTime,
    level: f64,
}

impl AudioEnvelopeSample {
    pub fn new(time: ProjectTime, level: f64) -> Result<Self, CoreError> {
        if time.as_nanos() < 0 || time.as_nanos() > MAX_SYNTHESIS_DURATION_NS {
            return Err(invalid(
                "audio feature timestamp outside supported project interval",
            ));
        }
        Ok(Self {
            time,
            level: NormalizedPosition::new(level)?,
        })
    }
    pub fn time(&self) -> ProjectTime {
        self.time
    }
    pub fn level(&self) -> f64 {
        self.level.value()
    }
}

impl TryFrom<AudioEnvelopeWire> for AudioEnvelopeSample {
    type Error = CoreError;
    fn try_from(value: AudioEnvelopeWire) -> Result<Self, Self::Error> {
        Self::new(value.time, value.level)
    }
}
impl From<AudioEnvelopeSample> for AudioEnvelopeWire {
    fn from(value: AudioEnvelopeSample) -> Self {
        Self {
            time: value.time,
            level: value.level.value(),
        }
    }
}

/// Unipolar audio mapping: offset + amplitude * level. Zero energy holds
/// offset; neither silence nor a quiet recording is stretched to full range.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AudioMappingWire", into = "AudioMappingWire")]
pub struct AudioMapping {
    axis: Axis,
    amplitude: f64,
    offset: NormalizedPosition,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioMappingWire {
    axis: Axis,
    amplitude: f64,
    offset: NormalizedPosition,
}

impl AudioMapping {
    pub fn new(axis: Axis, amplitude: f64, offset: NormalizedPosition) -> Result<Self, CoreError> {
        if !amplitude.is_finite() {
            return Err(CoreError::NonFinite);
        }
        if amplitude < 0.0 || offset.value() + amplitude > 1.0 {
            return Err(invalid(
                "audio amplitude and offset must remain in neutral [0,1]",
            ));
        }
        Ok(Self {
            axis,
            amplitude,
            offset,
        })
    }
    pub fn axis(&self) -> Axis {
        self.axis
    }
    pub fn amplitude(&self) -> f64 {
        self.amplitude
    }
    pub fn offset(&self) -> NormalizedPosition {
        self.offset
    }
}
impl TryFrom<AudioMappingWire> for AudioMapping {
    type Error = CoreError;
    fn try_from(v: AudioMappingWire) -> Result<Self, Self::Error> {
        Self::new(v.axis, v.amplitude, v.offset)
    }
}
impl From<AudioMapping> for AudioMappingWire {
    fn from(v: AudioMapping) -> Self {
        Self {
            axis: v.axis,
            amplitude: v.amplitude,
            offset: v.offset,
        }
    }
}

pub fn synthesize_audio_envelope(
    samples: &[AudioEnvelopeSample],
    mapping: &AudioMapping,
) -> Result<MotionProgram, CoreError> {
    if samples.is_empty() || samples.len() > MAX_SYNTHESIS_ACTIONS {
        return Err(invalid(
            "audio envelope must contain a bounded nonempty sample sequence",
        ));
    }
    if samples.windows(2).any(|pair| pair[0].time >= pair[1].time) {
        return Err(invalid(
            "audio feature timestamps must be strictly increasing",
        ));
    }
    let actions = samples
        .iter()
        .map(|sample| {
            synthesized_action(
                sample.time,
                mapping.offset.value() + mapping.amplitude * sample.level.value(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let gaps = if samples[0].time > ProjectTime::ZERO {
        vec![TimeRange::new(ProjectTime::ZERO, samples[0].time)?]
    } else {
        vec![]
    };
    MotionProgram::new(vec![MotionTrack::with_gaps(mapping.axis, actions, gaps)?])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BeatMappingWire", into = "BeatMappingWire")]
pub struct BeatMapping {
    mapping: AudioMapping,
    rise_ns: u64,
    fall_ns: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BeatMappingWire {
    axis: Axis,
    amplitude: f64,
    offset: NormalizedPosition,
    rise_ns: u64,
    fall_ns: u64,
}

impl BeatMapping {
    pub fn new(
        axis: Axis,
        amplitude: f64,
        offset: NormalizedPosition,
        rise_ns: u64,
        fall_ns: u64,
    ) -> Result<Self, CoreError> {
        if rise_ns == 0
            || fall_ns == 0
            || rise_ns.checked_add(fall_ns).ok_or(CoreError::Overflow)?
                > MAX_SYNTHESIS_DURATION_NS as u64
        {
            return Err(invalid("beat rise and fall must be positive and bounded"));
        }
        Ok(Self {
            mapping: AudioMapping::new(axis, amplitude, offset)?,
            rise_ns,
            fall_ns,
        })
    }
    pub fn rise_ns(&self) -> u64 {
        self.rise_ns
    }
    pub fn fall_ns(&self) -> u64 {
        self.fall_ns
    }
    pub fn mapping(&self) -> &AudioMapping {
        &self.mapping
    }
}
impl TryFrom<BeatMappingWire> for BeatMapping {
    type Error = CoreError;
    fn try_from(v: BeatMappingWire) -> Result<Self, Self::Error> {
        Self::new(v.axis, v.amplitude, v.offset, v.rise_ns, v.fall_ns)
    }
}
impl From<BeatMapping> for BeatMappingWire {
    fn from(v: BeatMapping) -> Self {
        Self {
            axis: v.mapping.axis,
            amplitude: v.mapping.amplitude,
            offset: v.mapping.offset,
            rise_ns: v.rise_ns,
            fall_ns: v.fall_ns,
        }
    }
}

/// Beats are supplied peak timestamps, not detected here. Every triangular
/// pulse must fit in duration and not overlap another pulse; conflicting
/// timing fails instead of shifting peaks or silently shortening pauses.
pub fn synthesize_beats(
    beats: &[ProjectTime],
    duration: ProjectTime,
    mapping: &BeatMapping,
) -> Result<MotionProgram, CoreError> {
    check_duration(duration)?;
    let budget = beats
        .len()
        .checked_mul(3)
        .and_then(|n| n.checked_add(2))
        .ok_or(CoreError::Overflow)?;
    if budget > MAX_SYNTHESIS_ACTIONS {
        return Err(invalid("beat synthesis exceeds the action budget"));
    }
    let rise = i64::try_from(mapping.rise_ns).map_err(|_| CoreError::Overflow)?;
    let fall = i64::try_from(mapping.fall_ns).map_err(|_| CoreError::Overflow)?;
    let mut previous_end = ProjectTime::ZERO;
    let mut previous_beat = None;
    // Validate the entire schedule before allocation or partial generation.
    for beat in beats {
        let start = beat.checked_add(-rise)?;
        let end = beat.checked_add(fall)?;
        if start < ProjectTime::ZERO
            || end > duration
            || start < previous_end
            || previous_beat.is_some_and(|previous| *beat <= previous)
        {
            return Err(invalid(
                "beat pulses must be ordered, nonoverlapping, and inside duration",
            ));
        }
        previous_end = end;
        previous_beat = Some(*beat);
    }
    let baseline = mapping.mapping.offset.value();
    let peak = baseline + mapping.mapping.amplitude;
    let mut actions = Vec::with_capacity(budget);
    push_distinct(&mut actions, ProjectTime::ZERO, baseline)?;
    for beat in beats {
        push_distinct(&mut actions, beat.checked_add(-rise)?, baseline)?;
        push_distinct(&mut actions, *beat, peak)?;
        push_distinct(&mut actions, beat.checked_add(fall)?, baseline)?;
    }
    push_distinct(&mut actions, duration, baseline)?;
    MotionProgram::new(vec![MotionTrack::new(mapping.mapping.axis, actions)?])
}

fn push_distinct(
    actions: &mut Vec<MotionAction>,
    time: ProjectTime,
    position: f64,
) -> Result<(), CoreError> {
    if let Some(last) = actions.last() {
        if last.time() == time {
            return if last.position().value() == position {
                Ok(())
            } else {
                Err(invalid("different positions at the same timestamp"))
            };
        }
    }
    actions.push(synthesized_action(time, position)?);
    Ok(())
}

fn synthesized_action(time: ProjectTime, value: f64) -> Result<MotionAction, CoreError> {
    MotionAction::new(
        time,
        NormalizedPosition::new(value)?,
        EvidenceKind::Synthesized,
    )
}

fn check_duration(duration: ProjectTime) -> Result<(), CoreError> {
    if duration.as_nanos() <= 0 || duration.as_nanos() > MAX_SYNTHESIS_DURATION_NS {
        return Err(invalid("duration must be positive and at most 24 hours"));
    }
    Ok(())
}

fn sampled_action_count(duration: ProjectTime, sample_hz: u32) -> Result<usize, CoreError> {
    let scaled = i128::from(duration.as_nanos())
        .checked_mul(i128::from(sample_hz))
        .ok_or(CoreError::Overflow)?;
    let count = scaled / 1_000_000_000 + 1 + i128::from(scaled % 1_000_000_000 != 0);
    usize::try_from(count).map_err(|_| CoreError::Overflow)
}

fn parse_decimal(value: &str) -> Result<f64, CoreError> {
    let mut pieces = value.split('.');
    let whole = pieces.next().unwrap_or("");
    let fraction = pieces.next();
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || pieces.next().is_some()
        || fraction.is_some_and(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid("numeric fields require unsigned decimal notation"));
    }
    let result = value
        .parse::<f64>()
        .map_err(|_| invalid("invalid decimal value"))?;
    if !result.is_finite() {
        return Err(CoreError::NonFinite);
    }
    Ok(result)
}

fn parse_duration(value: &str) -> Result<ProjectTime, CoreError> {
    let value = value
        .strip_suffix('s')
        .ok_or_else(|| invalid("duration requires lowercase s suffix"))?;
    let mut pieces = value.split('.');
    let whole = pieces.next().unwrap_or("");
    let fraction = pieces.next().unwrap_or("");
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || pieces.next().is_some()
        || fraction.len() > 9
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || value.ends_with('.')
    {
        return Err(invalid(
            "duration requires exact decimal seconds with at most nine fractional digits",
        ));
    }
    let seconds = whole.parse::<i64>().map_err(|_| CoreError::Overflow)?;
    let fractional_ns = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i64>()
            .map_err(|_| CoreError::Overflow)?
            .checked_mul(10_i64.pow((9 - fraction.len()) as u32))
            .ok_or(CoreError::Overflow)?
    };
    let nanos = seconds
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(fractional_ns))
        .ok_or(CoreError::Overflow)?;
    let duration = ProjectTime::from_nanos(nanos);
    check_duration(duration)?;
    Ok(duration)
}

pub const MAX_AUDIO_PCM_SAMPLES: usize = 96_000_000;
pub const MAX_AUDIO_SAMPLE_RATE: u32 = 192_000;
pub const AUDIO_ONSET_THRESHOLD: f64 = 0.35;

/// Bounded RMS features and amplitude-threshold crossings, not musical beats.
/// The worker owns decoding, channel mixing, sample calibration and lineage.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioFeatures {
    pub envelope: Vec<AudioEnvelopeSample>,
    pub amplitude_onsets: Vec<ProjectTime>,
    pub duration: ProjectTime,
}

/// Extract nonoverlapping-window RMS without peak/range normalization.
/// Each feature and threshold crossing is located at its window's first
/// sample. Temporal uncertainty is at most one analysis window; threshold
/// crossings do not establish musical beat timing or semantic motion.
///
/// Validate all PCM values and aggregate budgets before feature allocation.
pub fn extract_audio_features(
    mono_pcm: &[f32],
    sample_rate: u32,
    window_samples: u32,
) -> Result<AudioFeatures, CoreError> {
    if mono_pcm.is_empty() || mono_pcm.len() > MAX_AUDIO_PCM_SAMPLES {
        return Err(invalid("PCM input must contain 1..=96000000 mono samples"));
    }
    if sample_rate == 0
        || sample_rate > MAX_AUDIO_SAMPLE_RATE
        || window_samples == 0
        || window_samples > sample_rate
    {
        return Err(invalid(
            "audio rate must be 1..=192000 Hz and window must span 1 sample through 1 second",
        ));
    }
    let window = usize::try_from(window_samples).map_err(|_| CoreError::Overflow)?;
    let windows = mono_pcm
        .len()
        .checked_add(window - 1)
        .ok_or(CoreError::Overflow)?
        / window;
    let envelope_count = windows.checked_add(1).ok_or(CoreError::Overflow)?;
    if envelope_count > MAX_SYNTHESIS_ACTIONS {
        return Err(invalid("audio features exceed the action budget"));
    }
    let sample_time = |index: usize| -> Result<ProjectTime, CoreError> {
        let numerator = i128::try_from(index)
            .map_err(|_| CoreError::Overflow)?
            .checked_mul(1_000_000_000)
            .ok_or(CoreError::Overflow)?;
        Ok(ProjectTime::from_nanos(
            i64::try_from(numerator / i128::from(sample_rate)).map_err(|_| CoreError::Overflow)?,
        ))
    };
    let duration = sample_time(mono_pcm.len())?;
    check_duration(duration)?;
    for sample in mono_pcm {
        if !sample.is_finite() {
            return Err(CoreError::NonFinite);
        }
        if !(-1.0..=1.0).contains(sample) {
            return Err(invalid(
                "PCM samples must be calibrated to [-1,1] before extraction",
            ));
        }
    }
    let mut envelope = Vec::with_capacity(envelope_count);
    let mut amplitude_onsets = Vec::with_capacity(windows);
    let mut previous_level = 0.0;
    for (window_index, chunk) in mono_pcm.chunks(window).enumerate() {
        let start = window_index
            .checked_mul(window)
            .ok_or(CoreError::Overflow)?;
        let time = sample_time(start)?;
        let square_sum = chunk.iter().fold(0.0_f64, |sum, sample| {
            let value = f64::from(*sample);
            sum + value * value
        });
        let level = (square_sum / chunk.len() as f64).sqrt();
        if previous_level < AUDIO_ONSET_THRESHOLD && level >= AUDIO_ONSET_THRESHOLD {
            amplitude_onsets.push(time);
        }
        envelope.push(AudioEnvelopeSample::new(time, level)?);
        previous_level = level;
    }
    // A right endpoint preserves the final window level; it is not a
    // fabricated return to baseline or an invented extra amplitude onset.
    envelope.push(AudioEnvelopeSample::new(duration, previous_level)?);
    Ok(AudioFeatures {
        envelope,
        amplitude_onsets,
        duration,
    })
}
