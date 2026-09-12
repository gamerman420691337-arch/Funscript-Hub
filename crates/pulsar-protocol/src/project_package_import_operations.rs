//! Projectless, full-byte clone import. Descriptors remain declarations until
//! validation and atomic publication produce a completed import receipt.
use crate::{
    ArtifactIdentity, Command, ErrorCode, PackageCaptureBinding, PackageOperationId, ProtocolError,
    Request,
};
use pulsar_core::{ProjectId, RequestId, RevisionId};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

pub const PACKAGE_IMPORT_HEADER_BYTES: usize = 52;
pub const MAX_PACKAGE_IMPORT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_PACKAGE_UPLOAD_LIFETIME_MS: u64 = 30 * 60 * 1000;
pub const MAX_PACKAGE_UPLOAD_CHUNK_BYTES: u32 = 256 * 1024;

macro_rules! checked_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
                let value = value.into();
                PackageOperationId::new(value.clone())?;
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
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}
checked_id!(PackageImportOperationId);
checked_id!(PackageUploadId);

fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidRequest, message)
}

pub(crate) fn import_digest(value: &str) -> Result<(), ProtocolError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "package digest must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

/// Inspect exactly the fixed container header; this does not validate its
/// manifest, payload hashes, or logical closure.
pub fn package_format_version(header: &[u8]) -> Result<u16, ProtocolError> {
    if header.len() != PACKAGE_IMPORT_HEADER_BYTES || &header[..8] != b"PULSPKG\0" {
        return Err(invalid("invalid package header"));
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if !matches!(version, 1 | 2) {
        return Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "unsupported package format version",
        ));
    }
    if header[10..12] != [0, 0] {
        return Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "unsupported package header flags",
        ));
    }
    let length = u64::from_le_bytes(header[12..20].try_into().expect("fixed header slice"));
    if length == 0 || length > 16 * 1024 * 1024 {
        return Err(invalid("package manifest length exceeds its bound"));
    }
    Ok(version)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageUploadDeclaration {
    pub format_version: u16,
    pub sha256: String,
    pub byte_len: u64,
}
impl PackageUploadDeclaration {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !matches!(self.format_version, 1 | 2) {
            return Err(ProtocolError::new(
                ErrorCode::Unsupported,
                "unsupported package format version",
            ));
        }
        import_digest(&self.sha256)?;
        if self.byte_len <= PACKAGE_IMPORT_HEADER_BYTES as u64
            || self.byte_len > MAX_PACKAGE_IMPORT_BYTES
        {
            return Err(invalid("package upload length exceeds its bound"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageImportOperationState {
    AwaitingUpload,
    Uploading,
    Sealing,
    Sealed,
    Validating,
    Publishing,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl PackageImportOperationState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportProgress {
    pub received_bytes: u64,
    pub verified_bytes: u64,
    pub total_bytes: u64,
    pub completed_objects: u64,
    pub total_objects: Option<u64>,
}
impl PackageImportProgress {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.total_bytes > MAX_PACKAGE_IMPORT_BYTES
            || self.received_bytes > self.total_bytes
            || self.verified_bytes > self.received_bytes
            || self.completed_objects > 100_000
            || self
                .total_objects
                .is_some_and(|total| total > 100_000 || self.completed_objects > total)
        {
            return Err(invalid("invalid package import progress"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportReceipt {
    pub project_id: ProjectId,
    pub revision: RevisionId,
    pub capture: PackageCaptureBinding,
    pub package: PackageUploadDeclaration,
    pub origin_map: ArtifactIdentity,
}
impl PackageImportReceipt {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.package.validate()?;
        self.capture.validate()?;
        self.origin_map.validate()?;
        if self.project_id == self.capture.project_id
            || self.revision != self.capture.captured_revision
        {
            return Err(invalid(
                "import receipt must identify a fresh project with preserved revision",
            ));
        }
        if self.origin_map.byte_len == 0 || self.origin_map.byte_len > MAX_PACKAGE_IMPORT_BYTES {
            return Err(invalid("invalid import origin receipt length"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportStatus {
    pub operation_id: PackageImportOperationId,
    /// The initial StartProjectImport request, not this status query's ID.
    pub request_id: RequestId,
    pub package: PackageUploadDeclaration,
    pub state: PackageImportOperationState,
    pub progress: PackageImportProgress,
    /// Parsed capture identity is not a successful verification claim.
    pub capture: Option<PackageCaptureBinding>,
    pub receipt: Option<PackageImportReceipt>,
    pub error: Option<ProtocolError>,
}
impl PackageImportStatus {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        use PackageImportOperationState::*;
        self.package.validate()?;
        self.progress.validate()?;
        if self.progress.total_bytes != self.package.byte_len {
            return Err(invalid("import progress differs from its declared package"));
        }
        if let Some(capture) = &self.capture {
            capture.validate()?;
        }
        if matches!(
            self.state,
            Sealing | Sealed | Validating | Publishing | Completed
        ) && self.progress.received_bytes != self.package.byte_len
        {
            return Err(invalid("sealed import must contain every declared byte"));
        }
        if self.state == AwaitingUpload
            && (self.progress.received_bytes != 0 || self.capture.is_some())
        {
            return Err(invalid(
                "awaiting import cannot claim uploaded or parsed content",
            ));
        }
        if matches!(self.state, Publishing | Completed)
            && (self.progress.verified_bytes != self.package.byte_len || self.capture.is_none())
        {
            return Err(invalid(
                "publication requires verified package bytes and a capture identity",
            ));
        }
        if self.state == Completed {
            let receipt = self
                .receipt
                .as_ref()
                .ok_or_else(|| invalid("completed import has no receipt"))?;
            receipt.validate()?;
            if receipt.package != self.package
                || self.capture.as_ref() != Some(&receipt.capture)
                || self.progress.total_objects != Some(self.progress.completed_objects)
                || self.progress.completed_objects == 0
            {
                return Err(invalid(
                    "completed import receipt or progress differs from its operation",
                ));
            }
        } else if self.receipt.is_some() {
            return Err(invalid(
                "unpublished import cannot claim a fresh project receipt",
            ));
        }
        let failed = matches!(self.state, Failed | Cancelled | Interrupted);
        if failed != self.error.is_some() {
            return Err(invalid("import terminal error does not match its state"));
        }
        if let Some(error) = &self.error {
            if error.message.len() > 4096 || error.message.contains('\0') {
                return Err(invalid("import error exceeds its message bound"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageUploadChunkReceipt {
    pub offset: u64,
    pub byte_len: u32,
    pub sha256: String,
}
impl PackageUploadChunkReceipt {
    pub fn end(&self) -> Result<u64, ProtocolError> {
        import_digest(&self.sha256)?;
        if self.byte_len == 0 || self.byte_len > MAX_PACKAGE_UPLOAD_CHUNK_BYTES {
            return Err(invalid("package upload chunk exceeds its bound"));
        }
        self.offset
            .checked_add(u64::from(self.byte_len))
            .ok_or_else(|| invalid("package upload chunk overflows"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageUploadLease {
    pub lease_id: PackageUploadId,
    pub engine_epoch: String,
    pub operation_id: PackageImportOperationId,
    pub generation: u64,
    pub package: PackageUploadDeclaration,
    pub next_offset: u64,
    pub replay: Option<PackageUploadChunkReceipt>,
    pub expires_after_ms: u64,
    pub bulk_endpoint: PathBuf,
}
impl PackageUploadLease {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.package.validate()?;
        PackageOperationId::new(self.engine_epoch.clone())?;
        if self.generation == 0
            || self.next_offset > self.package.byte_len
            || self.expires_after_ms == 0
            || self.expires_after_ms > MAX_PACKAGE_UPLOAD_LIFETIME_MS
        {
            return Err(invalid(
                "invalid package upload generation, cursor, or lifetime",
            ));
        }
        match &self.replay {
            Some(replay) if replay.end()? == self.next_offset => (),
            None if self.next_offset == 0 => (),
            _ => return Err(invalid("package upload replay differs from its cursor")),
        }
        let endpoint = self
            .bulk_endpoint
            .to_str()
            .ok_or_else(|| invalid("invalid upload endpoint encoding"))?;
        if endpoint.is_empty() || endpoint.len() > 4096 || endpoint.contains('\0') {
            return Err(invalid("invalid package upload endpoint"));
        }
        let encoded =
            serde_json::to_vec(self).map_err(|_| invalid("cannot encode upload lease"))?;
        if encoded.len() > 15 * 1024 {
            return Err(invalid("upload lease exceeds its header bound"));
        }
        Ok(())
    }
    /// Exactly the most recent accepted payload can replay; every other chunk
    /// must start at the next contiguous cursor. No per-chunk history grows.
    pub fn validate_chunk(&self, chunk: &PackageUploadChunkReceipt) -> Result<bool, ProtocolError> {
        self.validate()?;
        if chunk.end()? > self.package.byte_len {
            return Err(invalid("upload exceeds declared package length"));
        }
        if self.replay.as_ref() == Some(chunk) {
            return Ok(true);
        }
        if chunk.offset != self.next_offset {
            return Err(invalid("upload is not contiguous or an exact replay"));
        }
        Ok(false)
    }
}

pub(crate) fn validate_import_request(request: &Request) -> Result<(), ProtocolError> {
    let import = match &request.command {
        Command::StartProjectImport { package } => {
            package.validate()?;
            true
        }
        Command::SealProjectImport {
            upload_generation, ..
        } => {
            if *upload_generation == 0 {
                return Err(invalid("zero upload generation"));
            }
            true
        }
        Command::ProjectImportStatus { .. }
        | Command::BeginProjectPackageUpload { .. }
        | Command::ProjectPackageUploadStatus { .. }
        | Command::AbandonProjectPackageUpload { .. }
        | Command::CancelProjectImport { .. } => true,
        _ => false,
    };
    if import && (request.project.is_some() || request.expected_revision.is_some()) {
        return Err(invalid("clone import has no existing-project envelope"));
    }
    Ok(())
}
