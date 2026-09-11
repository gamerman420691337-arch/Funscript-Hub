use crate::CoreError;
use serde::{Deserialize, Serialize};

/// Signed project nanoseconds. This is not a wall clock or a device deadline.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ProjectTime(i64);

impl ProjectTime {
    pub const ZERO: Self = Self(0);
    pub const fn from_nanos(value: i64) -> Self {
        Self(value)
    }
    pub const fn as_nanos(self) -> i64 {
        self.0
    }
    pub fn checked_add(self, nanoseconds: i64) -> Result<Self, CoreError> {
        self.0
            .checked_add(nanoseconds)
            .map(Self)
            .ok_or(CoreError::Overflow)
    }
    pub fn checked_sub(self, other: Self) -> Result<i64, CoreError> {
        self.0.checked_sub(other.0).ok_or(CoreError::Overflow)
    }
}

/// A deadline in the engine's monotonic clock domain, never project time.
/// The engine must additionally bind persisted/session deadlines to a clock
/// epoch. Raw monotonic values must not be reused across process restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MonotonicDeadline(u64);

impl MonotonicDeadline {
    pub const fn from_nanos(value: u64) -> Self {
        Self(value)
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    pub fn checked_add(self, nanoseconds: u64) -> Result<Self, CoreError> {
        self.0
            .checked_add(nanoseconds)
            .map(Self)
            .ok_or(CoreError::Overflow)
    }
    pub const fn reached_by(self, now: Self) -> bool {
        now.0 >= self.0
    }
}

/// Exact rational seconds; reduction is not required to preserve source PTS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RationalTimestampWire", into = "RationalTimestampWire")]
pub struct RationalTimestamp {
    numerator: i64,
    denominator: u32,
}

pub type SourceTimestamp = RationalTimestamp;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RationalTimestampWire {
    numerator: i64,
    denominator: u32,
}

impl RationalTimestamp {
    pub fn new(numerator: i64, denominator: u32) -> Result<Self, CoreError> {
        if denominator == 0 {
            return Err(CoreError::InvalidTimebase);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
    pub const fn numerator(self) -> i64 {
        self.numerator
    }
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    /// Quantization rounds toward negative infinity and reports the exact,
    /// nonnegative fractional-nanosecond remainder. Callers cannot silently
    /// mistake rounded timestamps for exact rational timestamps.
    pub fn quantize_nanoseconds(self) -> Result<TimeQuantization, CoreError> {
        let scaled = i128::from(self.numerator) * 1_000_000_000;
        let denominator = i128::from(self.denominator);
        let quotient = scaled.div_euclid(denominator);
        let remainder = scaled.rem_euclid(denominator);
        Ok(TimeQuantization {
            time: ProjectTime::from_nanos(
                i64::try_from(quotient).map_err(|_| CoreError::Overflow)?,
            ),
            remainder_numerator: remainder as u32,
            denominator: self.denominator,
        })
    }
}

impl TryFrom<RationalTimestampWire> for RationalTimestamp {
    type Error = CoreError;
    fn try_from(value: RationalTimestampWire) -> Result<Self, Self::Error> {
        Self::new(value.numerator, value.denominator)
    }
}
impl From<RationalTimestamp> for RationalTimestampWire {
    fn from(value: RationalTimestamp) -> Self {
        Self {
            numerator: value.numerator,
            denominator: value.denominator,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeQuantization {
    pub time: ProjectTime,
    pub remainder_numerator: u32,
    pub denominator: u32,
}

/// A nonempty half-open range [start, end) on the project timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TimeRangeWire", into = "TimeRangeWire")]
pub struct TimeRange {
    start: ProjectTime,
    end: ProjectTime,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeWire {
    start: ProjectTime,
    end: ProjectTime,
}

impl TimeRange {
    pub fn new(start: ProjectTime, end: ProjectTime) -> Result<Self, CoreError> {
        if start >= end {
            return Err(CoreError::InvalidMotion(
                "time range must be nonempty and ordered".into(),
            ));
        }
        Ok(Self { start, end })
    }
    pub const fn start(self) -> ProjectTime {
        self.start
    }
    pub const fn end(self) -> ProjectTime {
        self.end
    }
    pub fn contains(self, time: ProjectTime) -> bool {
        self.start <= time && time < self.end
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}
impl TryFrom<TimeRangeWire> for TimeRange {
    type Error = CoreError;
    fn try_from(value: TimeRangeWire) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}
impl From<TimeRange> for TimeRangeWire {
    fn from(value: TimeRange) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}
