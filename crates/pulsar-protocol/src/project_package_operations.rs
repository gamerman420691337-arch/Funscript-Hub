//! Package export lifecycle. No imported authority and no motion-lease reuse.
use crate::*;
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

pub const MAX_PACKAGE_DOWNLOAD_LIFETIME_MS: u64 = 30 * 60 * 1000;
macro_rules! package_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
                let value = value.into();
                TransferId::new(value.clone())?;
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = ProtocolError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
package_id!(PackageOperationId);
package_id!(PackageDownloadId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageExportOperationState {
    Queued,
    Capturing,
    Streaming,
    Ready,
    Failed,
    Cancelled,
    Interrupted,
    Released,
}
impl PackageExportOperationState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Failed | Self::Cancelled | Self::Interrupted | Self::Released
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCaptureBinding {
    pub project_id: ProjectId,
    pub captured_revision: RevisionId,
    pub captured_event_cursor: u64,
    pub manifest_sha256: String,
}
impl PackageCaptureBinding {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_sha256(&self.manifest_sha256)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageExportProgress {
    pub completed_objects: u64,
    pub total_objects: Option<u64>,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
}
impl PackageExportProgress {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.completed_objects > MAX_PROJECT_PACKAGE_ENTRIES as u64
            || self.total_objects.is_some_and(|n| {
                n > MAX_PROJECT_PACKAGE_ENTRIES as u64 || n < self.completed_objects
            })
            || self.completed_bytes > DEFAULT_MAX_PROJECT_PACKAGE_BYTES
            || self
                .total_bytes
                .is_some_and(|n| n > DEFAULT_MAX_PROJECT_PACKAGE_BYTES || n < self.completed_bytes)
        {
            return Err(ProtocolError::invalid(
                "package progress exceeds its admitted totals",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageReadyVerification {
    AllDeclaredObjectsVerified,
}
/// A durable verification receipt. Availability is controlled by operation state:
/// Released retains this historical receipt but is never downloadable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageArtifactDescriptor {
    pub format_version: u16,
    pub sha256: String,
    pub byte_len: u64,
    pub manifest_sha256: String,
    pub project_id: ProjectId,
    pub captured_revision: RevisionId,
    pub captured_event_cursor: u64,
    pub object_count: u32,
    pub verification: PackageReadyVerification,
}
impl PackageArtifactDescriptor {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !matches!(self.format_version, 1 | 2) {
            return Err(ProtocolError::unsupported("unsupported package format"));
        }
        validate_sha256(&self.sha256)?;
        validate_sha256(&self.manifest_sha256)?;
        if self.byte_len <= 52
            || self.byte_len > DEFAULT_MAX_PROJECT_PACKAGE_BYTES
            || self.object_count == 0
            || self.object_count as usize > MAX_PROJECT_PACKAGE_ENTRIES
        {
            return Err(ProtocolError::invalid(
                "package artifact exceeds its admitted bounds",
            ));
        }
        Ok(())
    }
    pub fn capture_binding(&self) -> PackageCaptureBinding {
        PackageCaptureBinding {
            project_id: self.project_id.clone(),
            captured_revision: self.captured_revision,
            captured_event_cursor: self.captured_event_cursor,
            manifest_sha256: self.manifest_sha256.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageExportStatus {
    pub operation_id: PackageOperationId,
    pub project_id: ProjectId,
    /// Original Start request, not the current status-query request identity.
    pub request_id: RequestId,
    pub requested_revision: RevisionId,
    pub state: PackageExportOperationState,
    pub capture: Option<PackageCaptureBinding>,
    pub progress: PackageExportProgress,
    pub artifact: Option<PackageArtifactDescriptor>,
    pub error: Option<ProtocolError>,
}
impl PackageExportStatus {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.progress.validate()?;
        if let Some(capture) = &self.capture {
            capture.validate()?;
            if capture.project_id != self.project_id
                || capture.captured_revision != self.requested_revision
            {
                return Err(ProtocolError::invalid("package capture binding mismatch"));
            }
        }
        if matches!(
            self.state,
            PackageExportOperationState::Streaming
                | PackageExportOperationState::Ready
                | PackageExportOperationState::Released
        ) && self.capture.is_none()
        {
            return Err(ProtocolError::invalid(
                "package state requires a pinned capture",
            ));
        }
        if self.state == PackageExportOperationState::Queued && self.capture.is_some() {
            return Err(ProtocolError::invalid(
                "queued package cannot claim a capture",
            ));
        }
        let verified = matches!(
            self.state,
            PackageExportOperationState::Ready | PackageExportOperationState::Released
        );
        if verified != self.artifact.is_some() {
            return Err(ProtocolError::invalid(
                "package ready receipt disagrees with its state",
            ));
        }
        if let Some(artifact) = &self.artifact {
            artifact.validate()?;
            if self.capture.as_ref() != Some(&artifact.capture_binding())
                || self.progress.total_bytes != Some(artifact.byte_len)
                || self.progress.completed_bytes != artifact.byte_len
                || self.progress.total_objects != Some(u64::from(artifact.object_count))
                || self.progress.completed_objects != u64::from(artifact.object_count)
            {
                return Err(ProtocolError::invalid(
                    "package verification receipt differs from capture or progress",
                ));
            }
        }
        let failed = matches!(
            self.state,
            PackageExportOperationState::Failed
                | PackageExportOperationState::Cancelled
                | PackageExportOperationState::Interrupted
        );
        if failed != self.error.is_some() {
            return Err(ProtocolError::invalid(
                "package terminal error disagrees with its state",
            ));
        }
        if let Some(error) = &self.error {
            bounded_text(&error.message, 4096, false)?;
        }
        Ok(())
    }
}
/// Epoch-local read admission is separate from durable Ready export history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageDownloadVerificationState {
    Verifying,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDownloadPending {
    pub operation_id: PackageOperationId,
    pub artifact_sha256: String,
    pub verification: PackageDownloadVerificationState,
    pub completed_bytes: u64,
    pub total_bytes: u64,
}
impl PackageDownloadPending {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_sha256(&self.artifact_sha256)?;
        if self.total_bytes <= 52
            || self.total_bytes > DEFAULT_MAX_PROJECT_PACKAGE_BYTES
            || self.completed_bytes > self.total_bytes
        {
            return Err(ProtocolError::invalid(
                "package verification progress exceeds artifact bounds",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageChunkRange {
    pub offset: u64,
    pub byte_len: u32,
}
impl PackageChunkRange {
    pub fn end(&self) -> Result<u64, ProtocolError> {
        if self.byte_len == 0 || self.byte_len as usize > MAX_ARTIFACT_CHUNK_BYTES {
            return Err(ProtocolError::invalid(
                "package chunk length is outside its bounded contract",
            ));
        }
        self.offset
            .checked_add(u64::from(self.byte_len))
            .ok_or_else(|| ProtocolError::invalid("package chunk range overflow"))
    }
}
/// A distinct epoch-bound download capability; it is not a motion transfer lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDownloadLease {
    pub lease_id: PackageDownloadId,
    pub engine_epoch: String,
    pub operation_id: PackageOperationId,
    pub artifact: PackageArtifactDescriptor,
    pub offset: u64,
    pub byte_len: u64,
    pub next_offset: u64,
    pub replay: Option<PackageChunkRange>,
    pub expires_after_ms: u64,
    pub bulk_endpoint: PathBuf,
}
impl PackageDownloadLease {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.artifact.validate()?;
        bounded_text(&self.engine_epoch, 128, true)?;
        let end = self
            .offset
            .checked_add(self.byte_len)
            .ok_or_else(|| ProtocolError::invalid("package download range overflow"))?;
        if self.byte_len == 0
            || end > self.artifact.byte_len
            || self.next_offset < self.offset
            || self.next_offset > end
            || self.expires_after_ms == 0
            || self.expires_after_ms > MAX_PACKAGE_DOWNLOAD_LIFETIME_MS
        {
            return Err(ProtocolError::invalid(
                "package lease range, cursor, or lifetime is invalid",
            ));
        }
        match self.replay {
            Some(replay) if replay.offset >= self.offset && replay.end()? == self.next_offset => {}
            None if self.next_offset == self.offset => {}
            _ => {
                return Err(ProtocolError::invalid(
                    "package replay slot does not bind the last delivered chunk",
                ))
            }
        }
        bounded_text(&self.bulk_endpoint.to_string_lossy(), 4096, true)?;
        if serde_json::to_vec(self)
            .map_err(|_| ProtocolError::invalid("invalid package lease"))?
            .len()
            > 16 * 1024 - 1024
        {
            return Err(ProtocolError::invalid(
                "package lease exceeds the bulk header budget",
            ));
        }
        Ok(())
    }
    pub fn validate_chunk(&self, chunk: PackageChunkRange) -> Result<bool, ProtocolError> {
        self.validate()?;
        let end = chunk.end()?;
        if chunk.offset < self.offset || end > self.offset + self.byte_len {
            return Err(ProtocolError::invalid(
                "package chunk is outside the leased range",
            ));
        }
        if self.replay == Some(chunk) {
            return Ok(true);
        }
        if chunk.offset != self.next_offset {
            return Err(ProtocolError::invalid(
                "package chunk is neither next nor exact replay",
            ));
        }
        Ok(false)
    }
}

pub(crate) fn bounded_text(text: &str, limit: usize, nonempty: bool) -> Result<(), ProtocolError> {
    if text.len() > limit || text.contains('\0') || (nonempty && text.is_empty()) {
        return Err(ProtocolError::invalid(
            "package field exceeds its bounded contract",
        ));
    }
    Ok(())
}
pub(crate) fn validate_request(request: &Request) -> Result<(), ProtocolError> {
    let package = matches!(
        &request.command,
        Command::AllowProjectPackaging { .. }
            | Command::StartProjectExport
            | Command::ProjectPackageStatus { .. }
            | Command::CancelProjectExport { .. }
            | Command::ReleaseProjectExport { .. }
            | Command::BeginProjectPackageDownload { .. }
            | Command::ProjectPackageDownloadStatus { .. }
            | Command::AbandonProjectPackageDownload { .. }
    );
    if !package {
        return Ok(());
    }
    if request.version != PROTOCOL_VERSION {
        return Err(ProtocolError::unsupported("unsupported protocol version"));
    }
    if request.project.is_none() {
        return Err(ProtocolError::invalid(
            "package operation requires a project",
        ));
    }
    match &request.command {
        Command::AllowProjectPackaging { owner_proof } => {
            if owner_proof.len() != 64
                || !owner_proof
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(ProtocolError::invalid("invalid OS-owner packaging proof"));
            }
        }
        Command::StartProjectExport if request.expected_revision.is_none() => {
            return Err(ProtocolError::invalid(
                "package export requires an expected revision",
            ))
        }
        Command::BeginProjectPackageDownload {
            offset, byte_len, ..
        } => {
            if *byte_len == 0
                || offset
                    .checked_add(*byte_len)
                    .is_none_or(|n| n > DEFAULT_MAX_PROJECT_PACKAGE_BYTES)
            {
                return Err(ProtocolError::invalid(
                    "package download range exceeds admission bounds",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifact() -> PackageArtifactDescriptor {
        PackageArtifactDescriptor {
            format_version: 1,
            sha256: "a".repeat(64),
            byte_len: DEFAULT_MAX_PROJECT_PACKAGE_BYTES,
            manifest_sha256: "b".repeat(64),
            project_id: ProjectId::new("p").unwrap(),
            captured_revision: RevisionId::new(3),
            captured_event_cursor: 7,
            object_count: 1,
            verification: PackageReadyVerification::AllDeclaredObjectsVerified,
        }
    }
    fn lease() -> PackageDownloadLease {
        PackageDownloadLease {
            lease_id: PackageDownloadId::new("l").unwrap(),
            engine_epoch: "e".into(),
            operation_id: PackageOperationId::new("o").unwrap(),
            artifact: artifact(),
            offset: 0,
            byte_len: DEFAULT_MAX_PROJECT_PACKAGE_BYTES,
            next_offset: 0,
            replay: None,
            expires_after_ms: MAX_PACKAGE_DOWNLOAD_LIFETIME_MS,
            bulk_endpoint: PathBuf::from("/tmp/bulk.sock"),
        }
    }
    #[test]
    fn package_range_supports_more_than_motion_limits_without_a_replay_map() {
        let mut l = lease();
        let n = MAX_ARTIFACT_CHUNK_BYTES as u32;
        for _ in 0..5000 {
            let chunk = PackageChunkRange {
                offset: l.next_offset,
                byte_len: n,
            };
            assert!(!l.validate_chunk(chunk).unwrap());
            l.next_offset = chunk.end().unwrap();
            l.replay = Some(chunk);
            assert!(l.validate_chunk(chunk).unwrap());
        }
        assert!(l.next_offset > 1024 * 1024 * 1024);
        assert!(serde_json::to_vec(&l).unwrap().len() < 2048);
    }
    #[test]
    fn malformed_package_ranges_and_lifetimes_fail() {
        let mut l = lease();
        l.next_offset = 1;
        assert!(l.validate().is_err());
        let mut l = lease();
        l.expires_after_ms = MAX_PACKAGE_DOWNLOAD_LIFETIME_MS + 1;
        assert!(l.validate().is_err());
        assert!(lease()
            .validate_chunk(PackageChunkRange {
                offset: u64::MAX,
                byte_len: 1
            })
            .is_err());
        assert!(lease()
            .validate_chunk(PackageChunkRange {
                offset: 0,
                byte_len: MAX_ARTIFACT_CHUNK_BYTES as u32 + 1
            })
            .is_err());
    }
    #[test]
    fn ready_and_released_need_verified_binding_and_complete_progress() {
        let a = artifact();
        let mut s = PackageExportStatus {
            operation_id: PackageOperationId::new("o").unwrap(),
            project_id: a.project_id.clone(),
            request_id: RequestId::new("r").unwrap(),
            requested_revision: a.captured_revision,
            state: PackageExportOperationState::Ready,
            capture: Some(a.capture_binding()),
            progress: PackageExportProgress {
                completed_objects: 1,
                total_objects: Some(1),
                completed_bytes: a.byte_len,
                total_bytes: Some(a.byte_len),
            },
            artifact: Some(a),
            error: None,
        };
        s.validate().unwrap();
        s.state = PackageExportOperationState::Released;
        s.validate().unwrap();
        s.progress.completed_bytes -= 1;
        assert!(s.validate().is_err());
    }
    #[test]
    fn package_control_requires_explicit_revision_and_bounded_owner_proof() {
        let req = Request::new(RequestId::new("r").unwrap(), Command::StartProjectExport)
            .in_project(ProjectId::new("p").unwrap(), None);
        assert!(req.validate().is_err());
        let req = Request::new(
            RequestId::new("r").unwrap(),
            Command::AllowProjectPackaging {
                owner_proof: "secret".into(),
            },
        )
        .in_project(ProjectId::new("p").unwrap(), None);
        assert!(req.validate().is_err());
    }
    #[test]
    fn pending_verification_never_changes_durable_ready_receipt() {
        let mut pending = PackageDownloadPending {
            operation_id: PackageOperationId::new("o").unwrap(),
            artifact_sha256: "a".repeat(64),
            verification: PackageDownloadVerificationState::Verifying,
            completed_bytes: 0,
            total_bytes: 1024,
        };
        pending.validate().unwrap();
        pending.completed_bytes = 1025;
        assert!(pending.validate().is_err());
        let mut value = serde_json::to_value(pending).unwrap();
        value["verification"] = serde_json::json!("ready");
        assert!(serde_json::from_value::<PackageDownloadPending>(value).is_err());
    }
    #[test]
    fn packaging_approval_is_redacted_and_start_revision_is_explicit() {
        let secret = "e".repeat(64);
        let command = Command::AllowProjectPackaging {
            owner_proof: secret.clone(),
        };
        assert!(!format!("{command:?}").contains(&secret));
        assert!(Command::StartProjectExport.requires_revision());
        assert!(Command::StartProjectExport.requires_project());
        let op = PackageOperationId::new("o").unwrap();
        assert!(!Command::ProjectPackageStatus { operation_id: op }.is_mutating());
        let request = Request::new(RequestId::new("r").unwrap(), command)
            .in_project(ProjectId::new("p").unwrap(), None);
        request.validate().unwrap();
        assert!(!format!("{request:?}").contains(&secret));
    }
    #[test]
    fn package_ids_and_unknown_fields_do_not_smuggle_other_authority() {
        assert!(PackageDownloadId::new("../bad").is_err());
        let mut value = serde_json::to_value(lease()).unwrap();
        value["auth_token"] = serde_json::json!("secret");
        assert!(serde_json::from_value::<PackageDownloadLease>(value).is_err());
    }
}
