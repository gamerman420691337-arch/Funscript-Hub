#![forbid(unsafe_code)]
//! Versioned client and worker wire contracts. This crate never authorizes an
//! operation, commits project state, loads a model, or actuates a device.

mod bulk;
mod framing;
mod generation;
mod motion;
mod project_package;
mod project_package_download;
mod project_package_operations;
mod transport;
mod worker;

pub use bulk::*;
pub use framing::*;
pub use generation::*;
pub use motion::*;
pub use project_package::*;
pub use project_package_download::*;
pub use project_package_operations::*;
pub use pulsar_core::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use transport::*;
pub use worker::*;

pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_CONTROL_BYTES: usize = 1024 * 1024;
pub const MAX_ARTIFACT_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u16,
    pub request_id: RequestId,
    pub session: Option<SessionId>,
    #[serde(default)]
    pub auth_token: Option<String>,
    pub project: Option<ProjectId>,
    pub expected_revision: Option<RevisionId>,
    pub command: Command,
}

impl Request {
    pub fn new(request_id: RequestId, command: Command) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            session: None,
            auth_token: None,
            project: None,
            expected_revision: None,
            command,
        }
    }

    pub fn in_project(mut self, project: ProjectId, revision: Option<RevisionId>) -> Self {
        self.project = Some(project);
        self.expected_revision = revision;
        self
    }

    pub fn with_session(mut self, session: SessionId) -> Self {
        self.session = Some(session);
        self
    }

    pub fn with_auth_token(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    /// Structural validation is not authorization. The engine must authenticate
    /// the session and check grants again at the effect/commit boundary.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        project_package_operations::validate_request(self)?;
        if let Some(token) = &self.auth_token {
            bounded_text(token, 4096, "authentication token")?;
        }
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                ErrorCode::Unsupported,
                "unsupported protocol version",
            ));
        }
        if self.command.requires_project() && self.project.is_none() {
            return Err(ProtocolError::invalid("operation requires project scope"));
        }
        if self.command.requires_revision() && self.expected_revision.is_none() {
            return Err(ProtocolError::invalid(
                "operation requires expected_revision",
            ));
        }
        match &self.command {
            Command::CreateProject { name } => bounded_text(name, 256, "project name")?,
            Command::BeginEditUpload {
                byte_len,
                sha256,
                label,
            } => {
                validate_motion_length(*byte_len)?;
                validate_sha256(sha256)?;
                bounded_text(label, 256, "edit label")?;
            }
            Command::BeginMotionDownload {
                offset, byte_len, ..
            } => {
                validate_motion_range(*offset, *byte_len, MAX_MOTION_BYTES)?;
            }
            Command::Generate {
                preset, settings, ..
            } => {
                bounded_text(preset, 64, "preset")?;
                if let Some(settings) = settings {
                    settings.validate()?;
                }
            }
            Command::GenerateInput { input } => input.validate()?,
            Command::MergeCandidate { axes, range, .. } => {
                validate_merge_axes(axes)?;
                if range.as_ref().is_some_and(|range| {
                    range.start() < ProjectTime::ZERO || range.end() <= range.start()
                }) {
                    return Err(ProtocolError::invalid(
                        "merge range must be nonnegative and nonempty",
                    ));
                }
            }
            Command::Pair {
                client_name,
                pairing_token,
            } => {
                bounded_text(client_name, 256, "client name")?;
                bounded_text(pairing_token, 4096, "pairing token")?;
            }
            Command::ImportSource { path }
            | Command::ImportFunscript { path }
            | Command::Export { path } => {
                if path.as_os_str().is_empty() {
                    return Err(ProtocolError::invalid("empty path"));
                }
            }
            Command::ExportAxis { path, .. } => {
                let bytes = path.as_os_str().as_encoded_bytes();
                if bytes.is_empty() || bytes.len() > 32 * 1024 || bytes.contains(&0) {
                    return Err(ProtocolError::invalid(
                        "axis export path must be nonempty, NUL-free, and at most 32768 bytes",
                    ));
                }
            }
            Command::PreviewFrame { model_input, .. } => {
                if let Some(input) = model_input {
                    input.validate()?;
                }
            }
            Command::SetProtectedRegions { regions } if regions.len() > 4096 => {
                return Err(ProtocolError::invalid("too many protected regions"));
            }
            Command::Events { limit, .. } if *limit == 0 || *limit > 1024 => {
                return Err(ProtocolError::invalid(
                    "event page limit must be within 1..=1024",
                ));
            }
            Command::OpenProject { project_id }
            | Command::Grant { project_id, .. }
            | Command::Revoke { project_id, .. } => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|scope| scope != project_id)
                {
                    return Err(ProtocolError::invalid("conflicting project scopes"));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("version", &self.version)
            .field("request_id", &self.request_id)
            .field("session", &self.session)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("project", &self.project)
            .field("expected_revision", &self.expected_revision)
            .field("command", &self.command)
            .finish()
    }
}

pub(crate) fn bounded_text(value: &str, limit: usize, name: &str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(ProtocolError::invalid(format!("invalid {name}")));
    }
    Ok(())
}

/// Exact typed request bytes, excluding the request ID. Store and compare these
/// within the authenticated actor's request-ID namespace. This is deliberately
/// not a cryptographic identity for files, models, or executable artifacts.
pub fn request_fingerprint(request: &Request) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&(
        request.version,
        &request.session,
        &request.auth_token,
        &request.project,
        &request.expected_revision,
        &request.command,
    ))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Command {
    CreateProject {
        name: String,
    },
    OpenProject {
        project_id: ProjectId,
    },
    ImportSource {
        path: PathBuf,
    },
    BeginEditUpload {
        byte_len: u64,
        sha256: String,
        label: String,
    },
    FinishEditUpload {
        lease_id: TransferId,
    },
    TransferStatus {
        lease_id: TransferId,
    },
    AbandonTransfer {
        lease_id: TransferId,
    },
    BeginMotionDownload {
        locator: MotionLocator,
        offset: u64,
        byte_len: u64,
    },
    SetProtectedRegions {
        regions: Vec<ProtectedRegion>,
    },
    Undo,
    Redo,
    Generate {
        source_version: SourceVersionId,
        preset: String,
        #[serde(default)]
        settings: Option<GenerationSettings>,
        #[serde(default)]
        model_path: Option<PathBuf>,
    },
    GenerateInput {
        input: GenerationInput,
    },
    ImportFunscript {
        path: PathBuf,
    },
    MergeCandidate {
        candidate_id: CandidateId,
        axes: Vec<Axis>,
        range: Option<TimeRange>,
    },
    Diagnostics {
        candidate_id: Option<CandidateId>,
    },
    PinSource {
        source_version: SourceVersionId,
        pinned: bool,
    },
    EvictSource {
        source_version: SourceVersionId,
    },
    JobStatus {
        job_id: JobId,
    },
    CancelJob {
        job_id: JobId,
    },
    ResumeJob {
        job_id: JobId,
    },
    GetCandidate {
        candidate_id: CandidateId,
    },
    CommitCandidate {
        candidate_id: CandidateId,
    },
    RebaseCandidate {
        candidate_id: CandidateId,
    },
    GetSnapshot,
    AllowProjectPackaging {
        owner_proof: String,
    },
    StartProjectExport,
    ProjectPackageStatus {
        operation_id: PackageOperationId,
    },
    CancelProjectExport {
        operation_id: PackageOperationId,
    },
    ReleaseProjectExport {
        operation_id: PackageOperationId,
    },
    BeginProjectPackageDownload {
        operation_id: PackageOperationId,
        offset: u64,
        byte_len: u64,
    },
    ProjectPackageDownloadStatus {
        lease_id: PackageDownloadId,
    },
    AbandonProjectPackageDownload {
        lease_id: PackageDownloadId,
    },
    Capabilities,
    PreviewFrame {
        context: FrameContext,
        source_time: SourceTimestamp,
        #[serde(default)]
        model_path: Option<PathBuf>,
        #[serde(default)]
        model_input: Option<ModelInputContract>,
    },
    Export {
        path: PathBuf,
    },
    ExportAxis {
        path: PathBuf,
        axis: Axis,
    },
    Pair {
        client_name: String,
        pairing_token: String,
    },
    Grant {
        session: SessionId,
        project_id: ProjectId,
        scopes: Vec<Scope>,
    },
    Revoke {
        session: SessionId,
        project_id: ProjectId,
    },
    Events {
        after_cursor: Option<u64>,
        limit: u16,
    },
    Arm {
        device_id: DeviceId,
        revision: RevisionId,
    },
    Stop {
        device_session: DeviceSessionId,
    },
    SwitchPlaybackRevision {
        device_session: DeviceSessionId,
        revision: RevisionId,
    },
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Arguments can contain pairing credentials, private paths or media
        // prompts. Debug logging intentionally exposes only operation names.
        f.write_str(match self {
            Self::CreateProject { .. } => "CreateProject",
            Self::OpenProject { .. } => "OpenProject",
            Self::ImportSource { .. } => "ImportSource",
            Self::BeginEditUpload { .. } => "BeginEditUpload",
            Self::FinishEditUpload { .. } => "FinishEditUpload",
            Self::TransferStatus { .. } => "TransferStatus",
            Self::AbandonTransfer { .. } => "AbandonTransfer",
            Self::BeginMotionDownload { .. } => "BeginMotionDownload",
            Self::SetProtectedRegions { .. } => "SetProtectedRegions",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Generate { .. } => "Generate",
            Self::GenerateInput { .. } => "GenerateInput",
            Self::ImportFunscript { .. } => "ImportFunscript",
            Self::MergeCandidate { .. } => "MergeCandidate",
            Self::Diagnostics { .. } => "Diagnostics",
            Self::PinSource { .. } => "PinSource",
            Self::EvictSource { .. } => "EvictSource",
            Self::JobStatus { .. } => "JobStatus",
            Self::CancelJob { .. } => "CancelJob",
            Self::ResumeJob { .. } => "ResumeJob",
            Self::GetCandidate { .. } => "GetCandidate",
            Self::CommitCandidate { .. } => "CommitCandidate",
            Self::RebaseCandidate { .. } => "RebaseCandidate",
            Self::GetSnapshot => "GetSnapshot",
            Self::AllowProjectPackaging { .. } => "AllowProjectPackaging",
            Self::StartProjectExport => "StartProjectExport",
            Self::ProjectPackageStatus { .. } => "ProjectPackageStatus",
            Self::CancelProjectExport { .. } => "CancelProjectExport",
            Self::ReleaseProjectExport { .. } => "ReleaseProjectExport",
            Self::BeginProjectPackageDownload { .. } => "BeginProjectPackageDownload",
            Self::ProjectPackageDownloadStatus { .. } => "ProjectPackageDownloadStatus",
            Self::AbandonProjectPackageDownload { .. } => "AbandonProjectPackageDownload",
            Self::Capabilities => "Capabilities",
            Self::PreviewFrame { .. } => "PreviewFrame",
            Self::Export { .. } => "Export",
            Self::ExportAxis { .. } => "ExportAxis",
            Self::Pair { .. } => "Pair",
            Self::Grant { .. } => "Grant",
            Self::Revoke { .. } => "Revoke",
            Self::Events { .. } => "Events",
            Self::Arm { .. } => "Arm",
            Self::Stop { .. } => "Stop",
            Self::SwitchPlaybackRevision { .. } => "SwitchPlaybackRevision",
        })
    }
}

impl Command {
    pub fn requires_project(&self) -> bool {
        if matches!(
            self,
            Self::AllowProjectPackaging { .. }
                | Self::StartProjectExport
                | Self::ProjectPackageStatus { .. }
                | Self::CancelProjectExport { .. }
                | Self::ReleaseProjectExport { .. }
                | Self::BeginProjectPackageDownload { .. }
                | Self::ProjectPackageDownloadStatus { .. }
                | Self::AbandonProjectPackageDownload { .. }
        ) {
            return true;
        }
        !matches!(
            self,
            Self::CreateProject { .. }
                | Self::OpenProject { .. }
                | Self::Capabilities
                | Self::Pair { .. }
                | Self::Grant { .. }
                | Self::Revoke { .. }
        )
    }

    pub fn requires_revision(&self) -> bool {
        matches!(
            self,
            Self::BeginEditUpload { .. }
                | Self::Undo
                | Self::Redo
                | Self::SetProtectedRegions { .. }
                | Self::CommitCandidate { .. }
                | Self::RebaseCandidate { .. }
                | Self::GenerateInput { .. }
                | Self::ImportFunscript { .. }
                | Self::MergeCandidate { .. }
                | Self::ExportAxis { .. }
                | Self::StartProjectExport
        )
    }

    pub fn is_mutating(&self) -> bool {
        match self {
            Self::ProjectPackageStatus { .. } | Self::ProjectPackageDownloadStatus { .. } => {
                return false
            }
            Self::AllowProjectPackaging { .. }
            | Self::StartProjectExport
            | Self::CancelProjectExport { .. }
            | Self::ReleaseProjectExport { .. }
            | Self::BeginProjectPackageDownload { .. }
            | Self::AbandonProjectPackageDownload { .. } => return true,
            _ => {}
        }
        !matches!(
            self,
            Self::OpenProject { .. }
                | Self::GetSnapshot
                | Self::Capabilities
                | Self::GetCandidate { .. }
                | Self::JobStatus { .. }
                | Self::Events { .. }
                | Self::Diagnostics { .. }
                | Self::TransferStatus { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Edit,
    Generate,
    Export,
    ManageGrants,
    ManageProtection,
    ImportSource,
    /// Full editable-project capture, distinct from neutral motion export.
    PackageProject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u16,
    pub request_id: RequestId,
    pub result: Result<ResponseBody, ProtocolError>,
}

impl Response {
    pub fn success(request_id: RequestId, body: ResponseBody) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            result: Ok(body),
        }
    }
    pub fn failure(request_id: RequestId, error: ProtocolError) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            result: Err(error),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ResponseBody {
    Project(ProjectSnapshot),
    Job(JobSnapshot),
    Candidate(CandidateSnapshot),
    Diagnostics(DiagnosticsReport),
    Transfer(TransferLease),
    TransferAbandoned {
        lease_id: TransferId,
    },
    Source {
        source_version: SourceVersionId,
    },
    Exported {
        path: PathBuf,
        revision: RevisionId,
    },
    AxisExported {
        path: PathBuf,
        revision: RevisionId,
        axis: Axis,
    },
    Preview(PreviewResult),
    Capabilities(Capabilities),
    Events(EventPage),
    Paired {
        session: SessionId,
        auth_token: String,
    },
    PackageExport(PackageExportStatus),
    PackageDownload(PackageDownloadLease),
    PackageDownloadPending(PackageDownloadPending),
    PackageDownloadAbandoned {
        lease_id: PackageDownloadId,
    },
    Ack,
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(value) => f.debug_tuple("Project").field(value).finish(),
            Self::Job(value) => f.debug_tuple("Job").field(value).finish(),
            Self::Candidate(value) => f.debug_tuple("Candidate").field(value).finish(),
            Self::Diagnostics(value) => f.debug_tuple("Diagnostics").field(value).finish(),
            Self::Transfer(value) => f.debug_tuple("Transfer").field(value).finish(),
            Self::TransferAbandoned { lease_id } => f
                .debug_struct("TransferAbandoned")
                .field("lease_id", lease_id)
                .finish(),
            Self::Source { source_version } => f
                .debug_struct("Source")
                .field("source_version", source_version)
                .finish(),
            Self::Exported { path, revision } => f
                .debug_struct("Exported")
                .field("path", path)
                .field("revision", revision)
                .finish(),
            Self::AxisExported {
                path,
                revision,
                axis,
            } => f
                .debug_struct("AxisExported")
                .field("path", path)
                .field("revision", revision)
                .field("axis", axis)
                .finish(),
            Self::Preview(value) => f.debug_tuple("Preview").field(value).finish(),
            Self::Capabilities(value) => f.debug_tuple("Capabilities").field(value).finish(),
            Self::Events(value) => f.debug_tuple("Events").field(value).finish(),
            Self::Paired { session, .. } => f
                .debug_struct("Paired")
                .field("session", session)
                .field("auth_token", &"[REDACTED]")
                .finish(),
            Self::PackageExport(value) => f.debug_tuple("PackageExport").field(value).finish(),
            Self::PackageDownload(value) => f.debug_tuple("PackageDownload").field(value).finish(),
            Self::PackageDownloadPending(value) => f
                .debug_tuple("PackageDownloadPending")
                .field(value)
                .finish(),
            Self::PackageDownloadAbandoned { lease_id } => f
                .debug_struct("PackageDownloadAbandoned")
                .field("lease_id", lease_id)
                .finish(),
            Self::Ack => f.write_str("Ack"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSnapshot {
    pub project_id: ProjectId,
    pub revision: RevisionId,
    pub name: String,
    pub motion: MotionDescriptor,
    pub sources: Vec<SourceSummary>,
}

/// Engine-assigned source purpose, not MIME detection or a capability grant.
/// Unknown media imported through ImportSource remains Media until a worker
/// validates its format; this must not be inferred from filenames or labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    Media,
    GenerationInput,
    Funscript,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSummary {
    pub source_version: SourceVersionId,
    pub label: String,
    /// Legacy wire summaries describe the original generic media-import path.
    /// Persisted newer synthetic sources require an engine-owned migration.
    #[serde(default)]
    pub kind: SourceKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSnapshot {
    pub job_id: JobId,
    pub attempt_id: AttemptId,
    pub project_id: ProjectId,
    pub base_revision: RevisionId,
    pub state: String,
    pub candidate_id: Option<CandidateId>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSnapshot {
    pub candidate_id: CandidateId,
    pub project_id: ProjectId,
    pub base_revision: RevisionId,
    pub motion: MotionDescriptor,
    pub job_id: Option<JobId>,
    #[serde(default)]
    pub review: Vec<ReviewFlag>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewResult {
    pub context: FrameContext,
    pub requested_source_time: SourceTimestamp,
    pub source_time: SourceTimestamp,
    pub source_frame_index: u64,
    pub observations: Vec<DetectionObservation>,
    pub frame: Option<FrameArtifact>,
    pub analysis: AnalysisStatus,
}

impl PreviewResult {
    /// Exact displayed-frame association. A neighboring frame is not a match.
    pub fn matches_request(&self, context: &FrameContext) -> bool {
        &self.context == context && self.actual_frame_is_consistent()
    }

    /// A time-based seek may resolve to a different source-frame ordinal.
    /// Matching this request does not mean its guessed frame was displayed:
    /// clients must adopt the returned actual context and timestamp, then use
    /// exact frame matching when attaching observations to displayed pixels.
    pub fn matches_seek_request(
        &self,
        context: &FrameContext,
        requested_source_time: &SourceTimestamp,
    ) -> bool {
        &self.requested_source_time == requested_source_time
            && self.context.source_version == context.source_version
            && self.context.source_placement == context.source_placement
            && self.context.transform == context.transform
            && self.context.seek_generation == context.seek_generation
            && self.context.request_generation == context.request_generation
            && self.actual_frame_is_consistent()
    }

    fn actual_frame_is_consistent(&self) -> bool {
        self.context.frame.value() == self.source_frame_index
            && self
                .observations
                .iter()
                .all(|o| o.matches_frame(&self.context))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisStatus {
    Available,
    Pending,
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub protocol_version: u16,
    pub operations: Vec<String>,
    pub backends: Vec<CapabilityStatus>,
    pub physical_playback: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityStatus {
    pub name: String,
    pub available: bool,
    pub qualification: QualificationStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum QualificationStatus {
    Qualified { record: String },
    Unqualified,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventPage {
    pub events: Vec<Event>,
    pub next_cursor: u64,
    pub resync_required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u16,
    pub cursor: u64,
    pub project_id: ProjectId,
    pub revision: RevisionId,
    pub body: EventBody,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EventBody {
    ProjectChanged,
    JobChanged(JobSnapshot),
    CandidateReady {
        candidate_id: CandidateId,
    },
    GrantRevoked {
        session: SessionId,
    },
    PlaybackDisarmed {
        device_session: DeviceSessionId,
        stopping_confirmed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    RevisionConflict,
    RequestConflict,
    ResourceExhausted,
    Unsupported,
    Unavailable,
    Cancelled,
    DependencyMismatch,
    Internal,
    UnknownOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }
    pub fn unsupported(operation: impl AsRef<str>) -> Self {
        Self::new(
            ErrorCode::Unsupported,
            format!("{} is not supported by this build", operation.as_ref()),
        )
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for ProtocolError {}
