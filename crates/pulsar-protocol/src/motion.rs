//! Immutable motion metadata. Descriptors locate content; they never grant access.
use crate::{ArtifactId, Axis, CandidateId, ErrorCode, ProjectId, ProtocolError, RevisionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_MOTION_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_MOTION_ACTIONS: u64 = 1_000_000;
pub const MAX_MOTION_GAPS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionCodec {
    MotionProgramJsonV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisSummary {
    pub axis: Axis,
    pub action_count: u64,
    pub gap_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramDescriptor {
    pub artifact_id: ArtifactId,
    pub sha256: String,
    pub byte_len: u64,
    pub codec: MotionCodec,
    pub axes: Vec<AxisSummary>,
}
impl ProgramDescriptor {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_motion_length(self.byte_len)?;
        validate_sha256(&self.sha256)?;
        if self.axes.len() > 6 {
            return Err(ProtocolError::invalid("too many motion axes"));
        }
        let mut actions = 0_u64;
        let mut gaps = 0_u64;
        for (i, axis) in self.axes.iter().enumerate() {
            if self.axes[..i].iter().any(|other| other.axis == axis.axis) {
                return Err(ProtocolError::invalid("duplicate motion axis"));
            }
            actions = actions
                .checked_add(axis.action_count)
                .ok_or_else(|| ProtocolError::invalid("action count overflow"))?;
            gaps = gaps
                .checked_add(axis.gap_count)
                .ok_or_else(|| ProtocolError::invalid("gap count overflow"))?;
        }
        if actions > MAX_MOTION_ACTIONS || gaps > MAX_MOTION_GAPS {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "motion count exceeds admission limit",
            ));
        }
        Ok(())
    }
    pub fn action_count(&self) -> u64 {
        self.axes
            .iter()
            .fold(0, |n, a| n.saturating_add(a.action_count))
    }
    pub fn gap_count(&self) -> u64 {
        self.axes
            .iter()
            .fold(0, |n, a| n.saturating_add(a.gap_count))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MotionBinding {
    ProjectRevision {
        project_id: ProjectId,
        revision: RevisionId,
    },
    Candidate {
        project_id: ProjectId,
        candidate_id: CandidateId,
        base_revision: RevisionId,
    },
}
impl MotionBinding {
    pub fn project_id(&self) -> &ProjectId {
        match self {
            Self::ProjectRevision { project_id, .. } | Self::Candidate { project_id, .. } => {
                project_id
            }
        }
    }
    pub fn locator(&self) -> MotionLocator {
        match self {
            Self::ProjectRevision { revision, .. } => MotionLocator::ProjectRevision {
                revision: *revision,
            },
            Self::Candidate { candidate_id, .. } => MotionLocator::Candidate {
                candidate_id: candidate_id.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionDescriptor {
    pub program: ProgramDescriptor,
    pub binding: MotionBinding,
}
impl MotionDescriptor {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.program.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MotionLocator {
    ProjectRevision { revision: RevisionId },
    Candidate { candidate_id: CandidateId },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TransferId(String);
impl TransferId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        validate_opaque_id(&value, "transfer identity")?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for TransferId {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<TransferId> for String {
    fn from(value: TransferId) -> Self {
        value.0
    }
}
impl std::fmt::Display for TransferId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Upload,
    Download,
}

/// A lease is authorization-scoped server state, not an authorization token.
/// accepted_prefix counts acknowledged bytes relative to the granted range.
/// Uploads always cover the complete artifact, starting at offset zero.
/// expires_after_ms is remaining lifetime when the response is produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferLease {
    pub lease_id: TransferId,
    pub engine_epoch: String,
    pub project_id: ProjectId,
    pub direction: TransferDirection,
    pub sha256: String,
    pub total_byte_len: u64,
    pub offset: u64,
    pub byte_len: u64,
    pub accepted_prefix: u64,
    pub expires_after_ms: u64,
    pub bulk_endpoint: PathBuf,
}
impl TransferLease {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_opaque_id(&self.engine_epoch, "engine epoch")?;
        validate_sha256(&self.sha256)?;
        validate_motion_length(self.total_byte_len)?;
        validate_motion_range(self.offset, self.byte_len, self.total_byte_len)?;
        if self.accepted_prefix > self.byte_len || self.expires_after_ms == 0 {
            return Err(ProtocolError::invalid(
                "invalid transfer progress or expiry",
            ));
        }
        if self.direction == TransferDirection::Upload
            && (self.offset != 0 || self.byte_len != self.total_byte_len)
        {
            return Err(ProtocolError::invalid(
                "upload must cover the whole artifact",
            ));
        }
        let path = self.bulk_endpoint.as_os_str().as_encoded_bytes();
        if path.is_empty() || path.len() > 4096 || path.contains(&0) {
            return Err(ProtocolError::invalid("invalid bulk endpoint"));
        }
        Ok(())
    }
}

pub fn validate_sha256(value: &str) -> Result<(), ProtocolError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ProtocolError::invalid(
            "SHA-256 must contain 64 lowercase hexadecimal digits",
        ));
    }
    Ok(())
}
pub fn validate_motion_length(byte_len: u64) -> Result<(), ProtocolError> {
    if byte_len == 0 || byte_len > MAX_MOTION_BYTES {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "motion artifact length outside admission limits",
        ));
    }
    Ok(())
}
pub fn validate_motion_range(offset: u64, byte_len: u64, total: u64) -> Result<(), ProtocolError> {
    if byte_len == 0 || offset.checked_add(byte_len).is_none_or(|end| end > total) {
        return Err(ProtocolError::invalid("motion range outside artifact"));
    }
    Ok(())
}
pub(crate) fn validate_opaque_id(value: &str, name: &str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
    {
        return Err(ProtocolError::invalid(format!("invalid {name}")));
    }
    Ok(())
}

impl crate::ProjectSnapshot {
    /// Check duplicate outer/binding identities before a client resolves content.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.motion.validate()?;
        crate::bounded_text(&self.name, 256, "project name")?;
        if self.motion.binding
            != (MotionBinding::ProjectRevision {
                project_id: self.project_id.clone(),
                revision: self.revision,
            })
        {
            return Err(ProtocolError::invalid(
                "project snapshot motion binding mismatch",
            ));
        }
        Ok(())
    }
}
impl crate::CandidateSnapshot {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.motion.validate()?;
        crate::validate_reviews(&self.review)?;
        if self.motion.binding
            != (MotionBinding::Candidate {
                project_id: self.project_id.clone(),
                candidate_id: self.candidate_id.clone(),
                base_revision: self.base_revision,
            })
        {
            return Err(ProtocolError::invalid(
                "candidate snapshot motion binding mismatch",
            ));
        }
        Ok(())
    }
}
