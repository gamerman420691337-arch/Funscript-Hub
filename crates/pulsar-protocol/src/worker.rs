use crate::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentity {
    pub artifact_id: ArtifactId,
    pub sha256: String,
    pub byte_len: u64,
}

impl ArtifactIdentity {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ProtocolError::invalid(
                "artifact SHA-256 must be 64 lowercase hexadecimal bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReference {
    pub identity: ArtifactIdentity,
    pub path: PathBuf,
}

impl ArtifactReference {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.identity.validate()?;
        if !self.path.is_absolute() {
            return Err(ProtocolError::invalid("artifact path must be absolute"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArtifact {
    pub source_version: SourceVersionId,
    pub path: PathBuf,
    pub identity: ArtifactIdentity,
}

impl SourceArtifact {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.identity.validate()?;
        if !self.path.is_absolute() {
            return Err(ProtocolError::invalid(
                "source snapshot path must be absolute",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub memory_bytes: u64,
    pub output_bytes: u64,
    pub wall_time_ms: u64,
    pub cpu_threads: u16,
}

impl ResourceBudget {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.memory_bytes == 0
            || self.output_bytes == 0
            || self.wall_time_ms == 0
            || self.cpu_threads == 0
        {
            return Err(ProtocolError::invalid(
                "worker budgets must be finite positive limits",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTools {
    pub ffmpeg: ArtifactReference,
    pub ffprobe: ArtifactReference,
    pub onnx_runtime: Option<ArtifactReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub version: u16,
    pub attempt_id: AttemptId,
    pub job_id: JobId,
    pub project_id: ProjectId,
    pub base_revision: RevisionId,
    pub source: SourceArtifact,
    pub output_dir: PathBuf,
    pub operation: WorkerOperation,
    pub budget: ResourceBudget,
    pub dependencies: Vec<ArtifactIdentity>,
    pub tools: WorkerTools,
}

impl WorkerRequest {
    /// These checks do not verify bytes, authorize filesystem paths, enforce
    /// operating-system resource limits, or attest to worker confinement.
    /// The engine and worker both enforce those effectful obligations.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::unsupported("worker protocol version"));
        }
        self.source.validate()?;
        self.budget.validate()?;
        if !self.output_dir.is_absolute() {
            return Err(ProtocolError::invalid(
                "worker output directory must be absolute",
            ));
        }
        self.tools.ffmpeg.validate()?;
        self.tools.ffprobe.validate()?;
        if let Some(runtime) = &self.tools.onnx_runtime {
            runtime.validate()?;
        }
        if self.dependencies.len() > 128 {
            return Err(ProtocolError::invalid("too many execution dependencies"));
        }
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        match &self.operation {
            WorkerOperation::Generate {
                preset,
                settings,
                model,
            } => {
                bounded_text(preset, 64, "preset")?;
                settings.validate()?;
                if let Some(model) = model {
                    model.validate()?;
                }
                if model.is_some() != settings.model_input.is_some() {
                    return Err(ProtocolError::invalid(
                        "model and input contract must be supplied together",
                    ));
                }
            }
            WorkerOperation::GenerateInput { input } => {
                input.validate()?;
                if input
                    .source_version()
                    .is_some_and(|version| version != &self.source.source_version)
                {
                    return Err(ProtocolError::invalid(
                        "modality input does not match its immutable source snapshot",
                    ));
                }
            }
            WorkerOperation::Preview {
                max_width,
                max_height,
                model,
                model_input,
                ..
            } => {
                validate_dimensions(*max_width, *max_height)?;
                if let Some(model) = model {
                    model.validate()?;
                }
                if let Some(input) = model_input {
                    input.validate()?;
                }
                if model.is_some() != model_input.is_some() {
                    return Err(ProtocolError::invalid(
                        "preview model and input contract must be supplied together",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkerOperation {
    Generate {
        preset: String,
        settings: GenerationSettings,
        model: Option<SourceArtifact>,
    },
    GenerateInput {
        input: GenerationInput,
    },
    Preview {
        context: FrameContext,
        source_time: SourceTimestamp,
        max_width: u32,
        max_height: u32,
        model: Option<SourceArtifact>,
        model_input: Option<ModelInputContract>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationSettings {
    pub analysis_width: u32,
    pub analysis_height: u32,
    pub source_sample_stride: u32,
    pub axis: Axis,
    pub vr_mode: bool,
    pub model_input: Option<ModelInputContract>,
}

impl Default for GenerationSettings {
    fn default() -> Self {
        Self {
            analysis_width: 320,
            analysis_height: 180,
            source_sample_stride: 1,
            axis: Axis::Stroke,
            vr_mode: false,
            model_input: None,
        }
    }
}

impl GenerationSettings {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_dimensions(self.analysis_width, self.analysis_height)?;
        if self.source_sample_stride == 0 {
            return Err(ProtocolError::invalid("sample stride must be positive"));
        }
        if let Some(input) = &self.model_input {
            input.validate()?;
        }
        Ok(())
    }
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), ProtocolError> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(ProtocolError::invalid(
            "dimensions must be within 1..=16384",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInputContract {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub layout: TensorLayout,
    pub class_count: u32,
    pub decoder: DetectorDecoder,
}

impl ModelInputContract {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_dimensions(self.width, self.height)?;
        if !(1..=4).contains(&self.channels) || self.class_count == 0 || self.class_count > 65536 {
            return Err(ProtocolError::invalid(
                "invalid model channel or class contract",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TensorLayout {
    Nchw,
    Nhwc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorDecoder {
    YoloChannelMajor,
    YoloEndToEnd,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResult {
    pub version: u16,
    pub attempt_id: AttemptId,
    pub job_id: JobId,
    pub result: Result<WorkerOutput, ProtocolError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkerOutput {
    Generated {
        program: ArtifactReference,
        receipt: ArtifactReference,
        lineage: Vec<ArtifactIdentity>,
        #[serde(default)]
        review: Vec<ReviewFlag>,
    },
    Preview(PreviewResult),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameArtifact {
    pub artifact: ArtifactReference,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u64,
    pub format: PixelFormat,
}

impl FrameArtifact {
    pub fn validate(&self, max_bytes: u64) -> Result<(), ProtocolError> {
        self.artifact.validate()?;
        validate_dimensions(self.width, self.height)?;
        let row = u64::from(self.width)
            .checked_mul(self.format.channels())
            .ok_or_else(|| ProtocolError::invalid("frame row overflow"))?;
        let len = self
            .stride_bytes
            .checked_mul(u64::from(self.height))
            .ok_or_else(|| ProtocolError::invalid("frame length overflow"))?;
        if self.stride_bytes < row || len != self.artifact.identity.byte_len || len > max_bytes {
            return Err(ProtocolError::invalid(
                "frame layout does not match bounded artifact",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Rgb8,
    Rgba8,
    Gray8,
}
impl PixelFormat {
    pub fn channels(self) -> u64 {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
            Self::Gray8 => 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointManifest {
    pub schema_version: u16,
    pub attempt_id: AttemptId,
    pub project_id: ProjectId,
    pub base_revision: RevisionId,
    pub source: ArtifactIdentity,
    pub dependencies: Vec<ArtifactIdentity>,
    pub configuration_sha256: String,
    pub checkpoint: ArtifactReference,
}

impl CheckpointManifest {
    /// Equality of declared identities is necessary, not sufficient. The engine
    /// must also validate each retained artifact's bytes before resuming.
    pub fn matches_dependencies(
        &self,
        request: &WorkerRequest,
        configuration_sha256: &str,
    ) -> bool {
        self.schema_version == PROTOCOL_VERSION
            && self.project_id == request.project_id
            && self.base_revision == request.base_revision
            && self.source == request.source.identity
            && self.dependencies == request.dependencies
            && self.configuration_sha256 == configuration_sha256
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransfer {
    pub artifact: ArtifactIdentity,
    pub session: SessionId,
    pub project: ProjectId,
    pub byte_offset: u64,
    pub byte_len: u64,
    pub max_chunk_bytes: u32,
}

impl ArtifactTransfer {
    pub fn validate(&self, admission_limit: u64) -> Result<(), ProtocolError> {
        self.artifact.validate()?;
        let end = self
            .byte_offset
            .checked_add(self.byte_len)
            .ok_or_else(|| ProtocolError::invalid("artifact transfer range overflow"))?;
        if end > self.artifact.byte_len
            || self.byte_len > admission_limit
            || self.max_chunk_bytes == 0
            || self.max_chunk_bytes as usize > MAX_ARTIFACT_CHUNK_BYTES
        {
            return Err(ProtocolError::invalid(
                "artifact transfer exceeds range or admission limit",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TensorDescriptor {
    pub artifact: ArtifactIdentity,
    pub shape: Vec<u64>,
    pub element_bytes: u8,
    pub byte_offset: u64,
    pub byte_len: u64,
}

impl TensorDescriptor {
    /// Only contiguous tensors are admitted in this ABI. Arbitrary strides,
    /// aliasing and writable host buffers require a future explicit contract.
    pub fn validate(&self, admission_limit: u64) -> Result<(), ProtocolError> {
        self.artifact.validate()?;
        if self.shape.is_empty()
            || self.shape.len() > 8
            || !matches!(self.element_bytes, 1 | 2 | 4 | 8)
        {
            return Err(ProtocolError::invalid(
                "unsupported tensor rank or element size",
            ));
        }
        let mut size = u64::from(self.element_bytes);
        for dimension in &self.shape {
            if *dimension == 0 {
                return Err(ProtocolError::invalid("zero tensor dimension"));
            }
            size = size
                .checked_mul(*dimension)
                .ok_or_else(|| ProtocolError::invalid("tensor shape overflow"))?;
        }
        let end = self
            .byte_offset
            .checked_add(size)
            .ok_or_else(|| ProtocolError::invalid("tensor range overflow"))?;
        if size != self.byte_len || size > admission_limit || end > self.artifact.byte_len {
            return Err(ProtocolError::invalid(
                "tensor layout exceeds artifact or admission",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionRole {
    Processor,
    Editor,
    Runtime,
    Driver,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfinementRequirement {
    Wasm,
    NativeSandbox,
    UnsandboxedExactBuild { approval_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub schema_version: u16,
    pub name: String,
    pub role: ExtensionRole,
    pub artifact: ArtifactIdentity,
    pub abi: String,
    pub requested_scopes: Vec<Scope>,
    pub budget: ResourceBudget,
    pub confinement: ConfinementRequirement,
}

impl ExtensionManifest {
    /// Manifest declarations never create grants, qualification, build approval,
    /// or assistant invocation approval. Those records are engine authority.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema_version != PROTOCOL_VERSION {
            return Err(ProtocolError::unsupported("extension schema"));
        }
        bounded_text(&self.name, 256, "extension name")?;
        bounded_text(&self.abi, 128, "extension ABI")?;
        self.artifact.validate()?;
        self.budget.validate()?;
        if self.requested_scopes.len() > 16 {
            return Err(ProtocolError::invalid("too many extension scopes"));
        }
        if let ConfinementRequirement::UnsandboxedExactBuild { approval_id } = &self.confinement {
            bounded_text(approval_id, 256, "approval identity")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(byte_len: u64) -> ArtifactIdentity {
        ArtifactIdentity {
            artifact_id: ArtifactId::new("artifact").unwrap(),
            sha256: "a".repeat(64),
            byte_len,
        }
    }

    #[test]
    fn malformed_digests_rejected() {
        for sha256 in ["A".repeat(64), "f".repeat(63), "g".repeat(64)] {
            assert!(ArtifactIdentity {
                sha256,
                ..identity(1)
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn tensor_overflow_zero_and_out_of_bounds_rejected() {
        for shape in [vec![u64::MAX, 2], vec![0, 1], vec![4, 4]] {
            assert!(TensorDescriptor {
                artifact: identity(8),
                shape,
                element_bytes: 4,
                byte_offset: 0,
                byte_len: 8
            }
            .validate(1024)
            .is_err());
        }
        assert!(TensorDescriptor {
            artifact: identity(8),
            shape: vec![2],
            element_bytes: 4,
            byte_offset: 0,
            byte_len: 8
        }
        .validate(8)
        .is_ok());
    }

    #[test]
    fn transfer_cannot_escape_artifact_or_budget() {
        let transfer = ArtifactTransfer {
            artifact: identity(100),
            session: SessionId::new("session").unwrap(),
            project: ProjectId::new("project").unwrap(),
            byte_offset: 95,
            byte_len: 10,
            max_chunk_bytes: 256,
        };
        assert!(transfer.validate(100).is_err());
        assert!(ArtifactTransfer {
            byte_offset: 0,
            ..transfer
        }
        .validate(5)
        .is_err());
    }

    #[test]
    fn invalid_frame_layout_rejected() {
        let frame = FrameArtifact {
            artifact: ArtifactReference {
                identity: identity(8),
                path: std::env::temp_dir().join("frame"),
            },
            width: 2,
            height: 2,
            stride_bytes: 4,
            format: PixelFormat::Rgb8,
        };
        assert!(frame.validate(8).is_err());
        assert!(FrameArtifact {
            format: PixelFormat::Gray8,
            ..frame
        }
        .validate(8)
        .is_ok());
    }

    #[test]
    fn zero_budget_and_stride_rejected() {
        assert!(ResourceBudget {
            memory_bytes: 1,
            output_bytes: 1,
            wall_time_ms: 0,
            cpu_threads: 1
        }
        .validate()
        .is_err());
        assert!(GenerationSettings {
            source_sample_stride: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
