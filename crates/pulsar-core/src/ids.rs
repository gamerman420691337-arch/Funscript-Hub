use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

fn check_identifier(value: &str) -> Result<(), CoreError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
    {
        return Err(CoreError::InvalidIdentifier);
    }
    Ok(())
}

macro_rules! string_id {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
                let value = value.into();
                check_identifier(&value)?;
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl TryFrom<String> for $name {
            type Error = CoreError;
            fn try_from(value: String) -> Result<Self, Self::Error> { Self::new(value) }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self { value.0 }
        }
        impl FromStr for $name {
            type Err = CoreError;
            fn from_str(value: &str) -> Result<Self, Self::Err> { Self::new(value) }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
    )+};
}

string_id!(
    ProjectId,
    SourceVersionId,
    SourcePlacementId,
    EntityTrackId,
    MotionTargetId,
    MotionOutputId,
    JobId,
    AttemptId,
    ArtifactId,
    DeviceId,
    DeviceSessionId,
    GrantId,
    ClientId,
    ActorId,
    ProvenanceRef,
    TransformId,
    RequestId,
    SessionId,
    CandidateId,
);

pub type CoordinateTransformId = TransformId;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct RevisionId(u64);

impl RevisionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn value(self) -> u64 {
        self.0
    }
    pub fn checked_next(self) -> Result<Self, CoreError> {
        self.0.checked_add(1).map(Self).ok_or(CoreError::Overflow)
    }
}

impl fmt::Display for RevisionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct FrameId(u64);

impl FrameId {
    pub const fn new(ordinal: u64) -> Self {
        Self(ordinal)
    }
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FrameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
