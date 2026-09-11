//! Portable editable-project logical closure, not an import authority or ready export.
//!
//! All identities here are archival. Original evidence is carried separately as
//! exact, digest-bound bytes; opaque worker manifests must never be executed.
//! Resolver paths, credentials, grants, and live jobs are deliberately absent.
//! The admission limits below are engineering defaults, not product guarantees.

use crate::{
    validate_reviews, validate_sha256, ArtifactIdentity, AttemptId, Axis, CandidateId, ErrorCode,
    JobId, ProgramDescriptor, ProjectId, ProtectedRegion, ProtocolError, ReviewFlag, RevisionId,
    SourceKind, SourceVersionId, TimeRange,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

pub const PROJECT_PACKAGE_FORMAT_VERSION: u16 = 1;
pub const MAX_PROJECT_PACKAGE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PROJECT_PACKAGE_ENTRIES: usize = 100_000;
pub const DEFAULT_MAX_PROJECT_PACKAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageObjectRole {
    MotionProgram,
    SourceSnapshot,
    EditInput,
    EditReceipt,
    WorkerReceipt,
    WorkerManifest,
    LegacyMotionBytes,
    DependencyEvidence,
    ExportReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageObjectRef {
    pub sha256: String,
    pub byte_len: u64,
}

impl PackageObjectRef {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_sha256(&self.sha256)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageObjectDescriptor {
    pub sha256: String,
    pub byte_len: u64,
    pub roles: Vec<PackageObjectRole>,
}

impl PackageObjectDescriptor {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_sha256(&self.sha256)?;
        if self.roles.is_empty() || !strictly_sorted(self.roles.iter()) {
            return invalid("package object roles must be nonempty, unique, and ordered");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageLogicalClosure {
    /// A coherent logical capture, not a proof that all referenced bytes exist.
    #[default]
    Captured,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageSourceBytes {
    /// P1a never upgrades this into a self-contained/export-ready assertion.
    #[default]
    NeedsMaterialization,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageGenerationDependencies {
    /// Model/runtime identities may survive without redistributing their bytes.
    #[default]
    IdentitiesOnly,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCompleteness {
    pub logical_closure: PackageLogicalClosure,
    pub source_bytes: PackageSourceBytes,
    pub generation_dependencies: PackageGenerationDependencies,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRecordCounts {
    pub revisions: u32,
    pub edit_states: u32,
    pub revision_lineage: u32,
    pub candidates: u32,
    pub authored_lineage: u32,
    pub sources: u32,
    pub legacy_cells: u32,
    pub generated_origins: u32,
    pub export_receipts: u32,
    pub objects: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageProjectHead {
    pub name: String,
    pub revision: RevisionId,
    pub history_cursor: u64,
    pub motion: ProgramDescriptor,
    pub protected: Vec<ProtectedRegion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRevision {
    pub revision: RevisionId,
    /// Historical attribution only, never a local authenticated actor.
    pub actor: String,
    pub kind: String,
    pub label: String,
    pub motion: ProgramDescriptor,
    pub protected: Vec<ProtectedRegion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageEditState {
    pub position: u64,
    pub motion: ProgramDescriptor,
    pub protected: Vec<ProtectedRegion>,
    /// None preserves a genuinely absent legacy history-lineage row.
    pub source_revision: Option<RevisionId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageReviewState {
    pub flags: Vec<ReviewFlag>,
    pub unknown: bool,
    pub conservative: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRevisionLineage {
    pub revision: RevisionId,
    pub parent_revision: Option<RevisionId>,
    pub restored_from_revision: Option<RevisionId>,
    /// This historical position may have been removed by branch truncation.
    pub restored_history_position: Option<u64>,
    pub candidate: Option<CandidateId>,
    pub operation: String,
    pub contributions: Vec<(Axis, TimeRange)>,
    pub review: PackageReviewState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCandidate {
    pub candidate_id: CandidateId,
    pub base_revision: RevisionId,
    pub motion: ProgramDescriptor,
    /// Inert origin only. It cannot resume, cancel, or authorize work.
    pub job_origin: Option<JobId>,
    /// Exact compact identities; model/runtime bytes need not be bundled.
    pub lineage: Vec<ArtifactIdentity>,
    pub committed_revision: Option<RevisionId>,
    pub review: Vec<ReviewFlag>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageAuthoredLineage {
    pub candidate_id: CandidateId,
    /// Original receipt identity, including its original (possibly older) base.
    pub receipt: ArtifactIdentity,
    pub input: PackageObjectRef,
    pub review: PackageReviewState,
    pub authored_count: u64,
    pub inherited_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSource {
    pub source_version: SourceVersionId,
    pub kind: SourceKind,
    pub label: String,
    pub identity: ArtifactIdentity,
    pub pinned: bool,
    /// Captured catalog state, not permission to follow the mutable original.
    pub evicted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageLegacyOwner {
    Projects,
    Revisions,
    EditStates,
    Candidates,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageLegacyCell {
    pub owner: PackageLegacyOwner,
    /// Original engine row identity, never SQL or a filesystem resolver path.
    pub row_key: String,
    pub object: PackageObjectRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageGeneratedOrigin {
    pub job_id: JobId,
    pub attempt_id: AttemptId,
    pub base_revision: RevisionId,
    pub source_version: SourceVersionId,
    /// Exact historical bytes. Imported data must not become a WorkerRequest.
    pub manifest: PackageObjectRef,
    pub receipt: Option<ArtifactIdentity>,
    /// Exact raw program identity named by the original receipt, not a rewritten canonical object.
    pub program: Option<ArtifactIdentity>,
    /// Historical status, not an executable queue state.
    pub state: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageExportState {
    Pending,
    Complete,
    FailedOrUnknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageExportReceipt {
    pub revision: RevisionId,
    pub axis: Axis,
    pub sha256: String,
    pub state: PackageExportState,
    /// Optional private archival text; never an output or extraction path.
    pub archival_path: Option<String>,
}

/// Strict logical snapshot. No live authority, paths to resolve, or executable
/// requests may be added here as a convenience for a future importer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableProjectManifest {
    pub format_version: u16,
    pub origin_project_id: ProjectId,
    pub captured_revision: RevisionId,
    /// Historical capture cursor, never a reconnect cursor in the destination.
    pub captured_event_cursor: u64,
    pub completeness: PackageCompleteness,
    pub counts: PackageRecordCounts,
    pub head: PackageProjectHead,
    pub revisions: Vec<PackageRevision>,
    pub edit_states: Vec<PackageEditState>,
    /// Absent legacy rows stay absent; numeric adjacency is not ancestry.
    pub revision_lineage: Vec<PackageRevisionLineage>,
    pub candidates: Vec<PackageCandidate>,
    pub authored_lineage: Vec<PackageAuthoredLineage>,
    pub sources: Vec<PackageSource>,
    pub legacy_cells: Vec<PackageLegacyCell>,
    pub generated_origins: Vec<PackageGeneratedOrigin>,
    pub export_receipts: Vec<PackageExportReceipt>,
    /// SHA-256 order, one payload per digest even if it serves several roles.
    pub objects: Vec<PackageObjectDescriptor>,
}

impl PortableProjectManifest {
    /// Computes counts; capture must explicitly install these before validation.
    pub fn record_counts(&self) -> Result<PackageRecordCounts, ProtocolError> {
        let lengths = [
            self.revisions.len(),
            self.edit_states.len(),
            self.revision_lineage.len(),
            self.candidates.len(),
            self.authored_lineage.len(),
            self.sources.len(),
            self.legacy_cells.len(),
            self.generated_origins.len(),
            self.export_receipts.len(),
            self.objects.len(),
        ];
        let total = lengths
            .iter()
            .try_fold(1usize, |total, n| total.checked_add(*n))
            .ok_or_else(|| resource("package entry count overflow"))?;
        if total > MAX_PROJECT_PACKAGE_ENTRIES {
            return Err(resource(
                "package exceeds the logical/object entry admission limit",
            ));
        }
        Ok(PackageRecordCounts {
            revisions: lengths[0] as u32,
            edit_states: lengths[1] as u32,
            revision_lineage: lengths[2] as u32,
            candidates: lengths[3] as u32,
            authored_lineage: lengths[4] as u32,
            sources: lengths[5] as u32,
            legacy_cells: lengths[6] as u32,
            generated_origins: lengths[7] as u32,
            export_receipts: lengths[8] as u32,
            objects: lengths[9] as u32,
        })
    }

    /// Validates bounded logical consistency, not possession or truth of bytes.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.canonical_bytes().map(|_| ())
    }

    /// Field order and array order are canonical. Reordering is never silent.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let bytes = bounded_json(self, MAX_PROJECT_PACKAGE_MANIFEST_BYTES)?;
        self.validate_structure()?;
        Ok(bytes)
    }

    fn validate_structure(&self) -> Result<(), ProtocolError> {
        if self.format_version != PROJECT_PACKAGE_FORMAT_VERSION {
            return Err(ProtocolError::unsupported(
                "unsupported portable project format",
            ));
        }
        if self.record_counts()? != self.counts {
            return invalid("package declared record counts do not match its closure");
        }
        if self.head.revision != self.captured_revision {
            return invalid("package head differs from captured revision");
        }
        if self
            .revisions
            .last()
            .is_none_or(|r| r.revision != self.captured_revision)
        {
            return invalid("captured head must be the maximum persisted revision");
        }
        if self
            .edit_states
            .iter()
            .enumerate()
            .any(|(position, state)| state.position != position as u64)
        {
            return invalid("retained history positions must be contiguous from zero");
        }
        text(&self.head.name, 256, true)?;
        if !strictly_sorted(self.revisions.iter().map(|r| &r.revision))
            || !strictly_sorted(self.edit_states.iter().map(|s| s.position))
            || !strictly_sorted(self.revision_lineage.iter().map(|r| &r.revision))
            || !strictly_sorted(self.candidates.iter().map(|c| c.candidate_id.as_str()))
            || !strictly_sorted(
                self.authored_lineage
                    .iter()
                    .map(|c| c.candidate_id.as_str()),
            )
            || !strictly_sorted(self.sources.iter().map(|s| s.source_version.as_str()))
            || !strictly_sorted(
                self.legacy_cells
                    .iter()
                    .map(|c| (c.owner, c.row_key.as_str())),
            )
            || !strictly_sorted(
                self.generated_origins
                    .iter()
                    .map(|g| (g.job_id.as_str(), g.attempt_id.as_str())),
            )
            || !strictly_sorted(self.objects.iter().map(|o| o.sha256.as_str()))
        {
            return invalid("package records must have unique identities in canonical order");
        }
        let revisions: BTreeMap<_, _> = self.revisions.iter().map(|r| (r.revision, r)).collect();
        let states: BTreeMap<_, _> = self.edit_states.iter().map(|s| (s.position, s)).collect();
        let candidates: BTreeMap<_, _> = self
            .candidates
            .iter()
            .map(|c| (c.candidate_id.as_str(), c))
            .collect();
        let sources: BTreeSet<_> = self
            .sources
            .iter()
            .map(|s| s.source_version.as_str())
            .collect();
        let mut job_receipts: BTreeMap<&str, BTreeSet<(&str, &str, u64)>> = BTreeMap::new();
        for origin in &self.generated_origins {
            if let Some(receipt) = &origin.receipt {
                job_receipts
                    .entry(origin.job_id.as_str())
                    .or_default()
                    .insert((
                        receipt.artifact_id.as_str(),
                        receipt.sha256.as_str(),
                        receipt.byte_len,
                    ));
            }
        }
        let head_revision = revisions
            .get(&self.head.revision)
            .ok_or_else(|| ProtocolError::invalid("package head revision is missing"))?;
        let head_state = states
            .get(&self.head.history_cursor)
            .ok_or_else(|| ProtocolError::invalid("package head history state is missing"))?;
        if !same(&self.head.motion, &head_revision.motion)?
            || !same(&self.head.protected, &head_revision.protected)?
            || !same(&self.head.motion, &head_state.motion)?
            || !same(&self.head.protected, &head_state.protected)?
        {
            return invalid("package head disagrees with its revision or retained history state");
        }
        let mut required = RequiredObjects::default();
        required.motion(&self.head.motion)?;
        protection(&self.head.protected)?;
        for revision in &self.revisions {
            text(&revision.actor, 256, true)?;
            text(&revision.kind, 128, true)?;
            text(&revision.label, 4096, false)?;
            required.motion(&revision.motion)?;
            protection(&revision.protected)?;
        }
        for state in &self.edit_states {
            required.motion(&state.motion)?;
            protection(&state.protected)?;
            if let Some(revision) = state.source_revision {
                let associated = revisions.get(&revision).ok_or_else(|| {
                    ProtocolError::invalid("retained history references a missing revision")
                })?;
                if !same(&state.motion, &associated.motion)?
                    || state.protected != associated.protected
                {
                    return invalid("retained history content differs from its source revision");
                }
            }
        }
        for lineage in &self.revision_lineage {
            if !revisions.contains_key(&lineage.revision) {
                return invalid("lineage references a missing revision");
            }
            for parent in [lineage.parent_revision, lineage.restored_from_revision]
                .into_iter()
                .flatten()
            {
                if !revisions.contains_key(&parent) || parent >= lineage.revision {
                    return invalid("lineage must reference an existing earlier revision");
                }
            }
            if let Some(candidate) = &lineage.candidate {
                let candidate = candidates.get(candidate.as_str()).ok_or_else(|| {
                    ProtocolError::invalid("revision lineage references a missing candidate")
                })?;
                if candidate.committed_revision != Some(lineage.revision) {
                    return invalid("revision lineage disagrees with its candidate commit");
                }
            }
            text(&lineage.operation, 128, true)?;
            bounded_entries(lineage.contributions.len())?;
            // TimeRange already enforces checked, nonempty interval construction.
            review(&lineage.review)?;
        }
        for candidate in &self.candidates {
            if !revisions.contains_key(&candidate.base_revision)
                || candidate
                    .committed_revision
                    .is_some_and(|r| !revisions.contains_key(&r) || r <= candidate.base_revision)
            {
                return invalid("candidate references a missing base or committed revision");
            }
            if let Some(job) = &candidate.job_origin {
                let matched = job_receipts.get(job.as_str()).is_some_and(|receipts| {
                    candidate.lineage.iter().any(|identity| {
                        receipts.contains(&(
                            identity.artifact_id.as_str(),
                            identity.sha256.as_str(),
                            identity.byte_len,
                        ))
                    })
                });
                if !matched {
                    return invalid(
                        "generated candidate is missing its exact archived worker receipt",
                    );
                }
            }
            bounded_entries(candidate.lineage.len())?;
            for identity in &candidate.lineage {
                identity.validate()?;
            }
            validate_reviews(&candidate.review)?;
            required.motion(&candidate.motion)?;
        }
        for authored in &self.authored_lineage {
            if !candidates.contains_key(authored.candidate_id.as_str()) {
                return invalid("authored lineage references a missing candidate");
            }
            if authored
                .authored_count
                .checked_add(authored.inherited_count)
                .is_none()
            {
                return invalid("authored lineage counts overflow");
            }
            review(&authored.review)?;
            required.identity(&authored.receipt, PackageObjectRole::EditReceipt)?;
            required.reference(&authored.input, PackageObjectRole::EditInput)?;
        }
        for source in &self.sources {
            text(&source.label, 4096, false)?;
            required.identity(&source.identity, PackageObjectRole::SourceSnapshot)?;
        }
        for cell in &self.legacy_cells {
            text(&cell.row_key, 512, true)?;
            let present = match cell.owner {
                PackageLegacyOwner::Projects => cell.row_key == self.origin_project_id.as_str(),
                PackageLegacyOwner::Revisions => {
                    legacy_position(&cell.row_key, &self.origin_project_id)
                        .is_some_and(|revision| revisions.contains_key(&RevisionId::new(revision)))
                }
                // Exact original cells survive truncation of the retained history.
                PackageLegacyOwner::EditStates => {
                    legacy_position(&cell.row_key, &self.origin_project_id).is_some()
                }
                PackageLegacyOwner::Candidates => candidates.contains_key(cell.row_key.as_str()),
            };
            if !present {
                return invalid("legacy recovery cell is outside the captured logical closure");
            }
            required.reference(&cell.object, PackageObjectRole::LegacyMotionBytes)?;
        }
        for origin in &self.generated_origins {
            if !revisions.contains_key(&origin.base_revision)
                || !sources.contains(origin.source_version.as_str())
            {
                return invalid("generation origin references a missing revision or source");
            }
            if !matches!(
                origin.state.as_str(),
                "queued" | "running" | "interrupted" | "completed" | "failed" | "cancelled"
            ) {
                return invalid("unsupported archival generation state");
            }
            required.reference(&origin.manifest, PackageObjectRole::WorkerManifest)?;
            if origin.receipt.is_some() != origin.program.is_some() {
                return invalid("archived worker receipt and exact raw program must be paired");
            }
            if let Some(program) = &origin.program {
                required.identity(program, PackageObjectRole::DependencyEvidence)?;
            }
            if let Some(receipt) = &origin.receipt {
                required.identity(receipt, PackageObjectRole::WorkerReceipt)?;
            }
        }
        let mut previous: Option<Vec<u8>> = None;
        for receipt in &self.export_receipts {
            if !revisions.contains_key(&receipt.revision) {
                return invalid("export receipt references a missing revision");
            }
            validate_sha256(&receipt.sha256)?;
            if let Some(path) = &receipt.archival_path {
                text(path, 4096, false)?;
            }
            let key = bounded_json(receipt, 16 * 1024)?;
            if previous.as_ref().is_some_and(|p| p > &key) {
                return invalid("export receipts must be ordered by canonical record bytes");
            }
            previous = Some(key);
        }
        let mut total_bytes = 0u64;
        for object in &self.objects {
            object.validate()?;
            total_bytes = total_bytes
                .checked_add(object.byte_len)
                .ok_or_else(|| resource("package object byte total overflow"))?;
            let expected = required
                .0
                .remove(&object.sha256)
                .ok_or_else(|| ProtocolError::invalid("unreferenced object in package closure"))?;
            if object.byte_len != expected.0
                || object.roles.iter().copied().collect::<BTreeSet<_>>() != expected.1
            {
                return invalid("package object length or roles differ from logical references");
            }
        }
        if !required.0.is_empty() {
            return invalid("package is missing a required object descriptor");
        }
        Ok(())
    }
}

#[derive(Default)]
struct RequiredObjects(BTreeMap<String, (u64, BTreeSet<PackageObjectRole>)>);
impl RequiredObjects {
    fn reference(
        &mut self,
        object: &PackageObjectRef,
        role: PackageObjectRole,
    ) -> Result<(), ProtocolError> {
        object.validate()?;
        if let Some((length, roles)) = self.0.get_mut(&object.sha256) {
            if *length != object.byte_len {
                return invalid("one object digest has inconsistent lengths");
            }
            roles.insert(role);
        } else {
            self.0.insert(
                object.sha256.clone(),
                (object.byte_len, BTreeSet::from([role])),
            );
        }
        Ok(())
    }
    fn identity(
        &mut self,
        identity: &ArtifactIdentity,
        role: PackageObjectRole,
    ) -> Result<(), ProtocolError> {
        identity.validate()?;
        self.reference(
            &PackageObjectRef {
                sha256: identity.sha256.clone(),
                byte_len: identity.byte_len,
            },
            role,
        )
    }
    fn motion(&mut self, motion: &ProgramDescriptor) -> Result<(), ProtocolError> {
        motion.validate()?;
        self.reference(
            &PackageObjectRef {
                sha256: motion.sha256.clone(),
                byte_len: motion.byte_len,
            },
            PackageObjectRole::MotionProgram,
        )
    }
}

fn strictly_sorted<T: PartialOrd>(values: impl IntoIterator<Item = T>) -> bool {
    let mut previous = None;
    for value in values {
        if previous.as_ref().is_some_and(|p| p >= &value) {
            return false;
        }
        previous = Some(value);
    }
    true
}
fn bounded_entries(n: usize) -> Result<(), ProtocolError> {
    if n > MAX_PROJECT_PACKAGE_ENTRIES {
        return Err(resource("package nested record limit exceeded"));
    }
    Ok(())
}
fn protection(regions: &[ProtectedRegion]) -> Result<(), ProtocolError> {
    if regions.len() > 1024 {
        return invalid("package protection limit exceeded");
    }
    Ok(())
}
fn review(state: &PackageReviewState) -> Result<(), ProtocolError> {
    validate_reviews(&state.flags)
}
fn text(value: &str, limit: usize, nonempty: bool) -> Result<(), ProtocolError> {
    if value.len() > limit || value.contains('\0') || (nonempty && value.is_empty()) {
        return invalid("package text field exceeds its bounded contract");
    }
    Ok(())
}
fn legacy_position(key: &str, project: &ProjectId) -> Option<u64> {
    let (owner, suffix) = key.rsplit_once(':')?;
    if owner != project.as_str() {
        return None;
    }
    let value = suffix.parse::<u64>().ok()?;
    (suffix == value.to_string()).then_some(value)
}
fn same<T: Serialize>(left: &T, right: &T) -> Result<bool, ProtocolError> {
    Ok(bounded_json(left, MAX_PROJECT_PACKAGE_MANIFEST_BYTES)?
        == bounded_json(right, MAX_PROJECT_PACKAGE_MANIFEST_BYTES)?)
}
fn invalid<T>(message: &str) -> Result<T, ProtocolError> {
    Err(ProtocolError::invalid(message))
}
fn resource(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ResourceExhausted, message)
}
fn bounded_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, ProtocolError> {
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self
                .bytes
                .len()
                .checked_add(bytes.len())
                .is_none_or(|n| n > self.limit)
            {
                return Err(io::Error::other("package JSON admission limit exceeded"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| resource("package canonical JSON exceeds its admission limit"))?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactId, MotionCodec};
    const EMPTY_SHA: &str = "ac11c4570c1e4918245c0ca34c2b951091b867a5aef252556ae323d06df61cf8";

    fn fixture() -> PortableProjectManifest {
        let motion = ProgramDescriptor {
            artifact_id: ArtifactId::new(format!("motion.{EMPTY_SHA}")).unwrap(),
            sha256: EMPTY_SHA.into(),
            byte_len: 13,
            codec: MotionCodec::MotionProgramJsonV1,
            axes: vec![],
        };
        let mut m = PortableProjectManifest {
            format_version: 1,
            origin_project_id: ProjectId::new("p").unwrap(),
            captured_revision: RevisionId::new(0),
            captured_event_cursor: 0,
            completeness: PackageCompleteness::default(),
            counts: PackageRecordCounts::default(),
            head: PackageProjectHead {
                name: "P".into(),
                revision: RevisionId::new(0),
                history_cursor: 0,
                motion: motion.clone(),
                protected: vec![],
            },
            revisions: vec![PackageRevision {
                revision: RevisionId::new(0),
                actor: "actor".into(),
                kind: "create".into(),
                label: "".into(),
                motion: motion.clone(),
                protected: vec![],
            }],
            edit_states: vec![PackageEditState {
                position: 0,
                motion,
                protected: vec![],
                source_revision: None,
            }],
            revision_lineage: vec![],
            candidates: vec![],
            authored_lineage: vec![],
            sources: vec![],
            legacy_cells: vec![],
            generated_origins: vec![],
            export_receipts: vec![],
            objects: vec![PackageObjectDescriptor {
                sha256: EMPTY_SHA.into(),
                byte_len: 13,
                roles: vec![PackageObjectRole::MotionProgram],
            }],
        };
        m.counts = m.record_counts().unwrap();
        m
    }
    #[test]
    fn sparse_legacy_closure_and_exact_canonical_roundtrip() {
        let m = fixture();
        let bytes = m.canonical_bytes().unwrap();
        let decoded: PortableProjectManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.canonical_bytes().unwrap(), bytes);
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains("\"source_bytes\":\"needs_materialization\""));
    }
    #[test]
    fn missing_declared_record_or_required_object_fails() {
        let mut m = fixture();
        m.edit_states.clear();
        assert!(m.validate().is_err());
        let mut m = fixture();
        m.objects.clear();
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
    }
    #[test]
    fn duplicate_object_and_mismatched_roles_fail() {
        let mut m = fixture();
        m.objects.push(m.objects[0].clone());
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
        let mut m = fixture();
        m.objects[0].roles.push(PackageObjectRole::WorkerReceipt);
        assert!(m.validate().is_err());
    }
    #[test]
    fn authority_and_paths_are_not_manifest_fields() {
        for key in [
            "sessions",
            "grants",
            "auth_token",
            "worker_request",
            "sql",
            "destination_path",
        ] {
            let mut json = serde_json::to_value(fixture()).unwrap();
            json[key] = serde_json::json!([]);
            assert!(
                serde_json::from_value::<PortableProjectManifest>(json).is_err(),
                "{key}"
            );
        }
        let mut json = serde_json::to_value(fixture()).unwrap();
        json["objects"][0]["path"] = serde_json::json!("/etc/passwd");
        assert!(serde_json::from_value::<PortableProjectManifest>(json).is_err());
    }
    #[test]
    fn unknown_version_and_false_readiness_fail() {
        let mut m = fixture();
        m.format_version = 2;
        assert_eq!(m.validate().unwrap_err().code, ErrorCode::Unsupported);
        let mut json = serde_json::to_value(fixture()).unwrap();
        json["completeness"]["source_bytes"] = serde_json::json!("verified");
        assert!(serde_json::from_value::<PortableProjectManifest>(json).is_err());
    }
    #[test]
    fn exact_legacy_cell_reference_is_required_and_scoped() {
        let mut m = fixture();
        m.legacy_cells.push(PackageLegacyCell {
            owner: PackageLegacyOwner::Projects,
            row_key: "p".into(),
            object: PackageObjectRef {
                sha256: EMPTY_SHA.into(),
                byte_len: 13,
            },
        });
        m.objects[0]
            .roles
            .push(PackageObjectRole::LegacyMotionBytes);
        m.counts = m.record_counts().unwrap();
        m.validate().unwrap();
        m.legacy_cells[0].row_key = "other-project".into();
        assert!(m.validate().is_err());
    }
    #[test]
    fn broken_edges_fail_but_truncated_restoration_position_is_historical() {
        let mut m = fixture();
        let mut r = m.revisions[0].clone();
        r.revision = RevisionId::new(1);
        m.revisions.push(r);
        m.captured_revision = RevisionId::new(1);
        m.head.revision = RevisionId::new(1);
        m.revision_lineage.push(PackageRevisionLineage {
            revision: RevisionId::new(1),
            parent_revision: Some(RevisionId::new(0)),
            restored_from_revision: Some(RevisionId::new(0)),
            restored_history_position: Some(99),
            candidate: None,
            operation: "undo".into(),
            contributions: vec![],
            review: PackageReviewState {
                flags: vec![],
                unknown: true,
                conservative: true,
            },
        });
        m.counts = m.record_counts().unwrap();
        m.validate().unwrap();
        m.revision_lineage[0].parent_revision = Some(RevisionId::new(7));
        assert!(m.validate().is_err());
    }
    #[test]
    fn missing_authored_evidence_is_not_an_identity_only_dependency() {
        let mut m = fixture();
        let id = CandidateId::new("c").unwrap();
        m.candidates.push(PackageCandidate {
            candidate_id: id.clone(),
            base_revision: RevisionId::new(0),
            motion: m.head.motion.clone(),
            job_origin: None,
            lineage: vec![],
            committed_revision: None,
            review: vec![],
        });
        m.authored_lineage.push(PackageAuthoredLineage {
            candidate_id: id,
            receipt: ArtifactIdentity {
                artifact_id: ArtifactId::new("receipt").unwrap(),
                sha256: "0".repeat(64),
                byte_len: 9,
            },
            input: PackageObjectRef {
                sha256: EMPTY_SHA.into(),
                byte_len: 13,
            },
            review: PackageReviewState {
                flags: vec![],
                unknown: true,
                conservative: false,
            },
            authored_count: 1,
            inherited_count: 0,
        });
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
    }
    #[test]
    fn current_head_must_not_have_future_persisted_revisions() {
        let mut m = fixture();
        let mut r = m.revisions[0].clone();
        r.revision = RevisionId::new(1);
        m.revisions.push(r);
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
    }
    #[test]
    fn retained_history_cannot_claim_unrelated_revision_content() {
        let mut m = fixture();
        let old = m.head.motion.clone();
        let mut new = old.clone();
        new.sha256 = "1".repeat(64);
        new.artifact_id = ArtifactId::new(format!("motion.{}", new.sha256)).unwrap();
        let mut r = m.revisions[0].clone();
        r.revision = RevisionId::new(1);
        r.motion = new.clone();
        m.revisions.push(r);
        m.captured_revision = RevisionId::new(1);
        m.head.revision = RevisionId::new(1);
        m.head.motion = new.clone();
        m.edit_states[0].motion = new;
        m.edit_states.push(PackageEditState {
            position: 1,
            motion: old,
            protected: vec![],
            source_revision: Some(RevisionId::new(1)),
        });
        m.objects.push(PackageObjectDescriptor {
            sha256: "1".repeat(64),
            byte_len: 13,
            roles: vec![PackageObjectRole::MotionProgram],
        });
        m.objects.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
    }
    #[test]
    fn truncated_recovery_retains_exact_colon_underscore_project_ownership() {
        let mut m = fixture();
        m.origin_project_id = ProjectId::new("p_:nested").unwrap();
        m.legacy_cells.push(PackageLegacyCell {
            owner: PackageLegacyOwner::EditStates,
            row_key: "p_:nested:99".into(),
            object: PackageObjectRef {
                sha256: EMPTY_SHA.into(),
                byte_len: 13,
            },
        });
        m.objects[0]
            .roles
            .push(PackageObjectRole::LegacyMotionBytes);
        m.counts = m.record_counts().unwrap();
        m.validate().unwrap();
        for key in [
            "pX:nested:99",
            "p_:nested_extra:99",
            "p_:nested:099",
            "p_:nested:+99",
            "p_:nested:18446744073709551616",
            "p_:nested:other:99",
        ] {
            m.legacy_cells[0].row_key = key.into();
            assert!(m.validate().is_err(), "{key}");
        }
    }
    #[test]
    fn generated_candidate_cannot_drop_its_exact_worker_receipt() {
        let mut m = fixture();
        let job = JobId::new("j").unwrap();
        let source = SourceVersionId::new("s").unwrap();
        let receipt = ArtifactIdentity {
            artifact_id: ArtifactId::new("receipt").unwrap(),
            sha256: "2".repeat(64),
            byte_len: 9,
        };
        m.candidates.push(PackageCandidate {
            candidate_id: CandidateId::new("c").unwrap(),
            base_revision: RevisionId::new(0),
            motion: m.head.motion.clone(),
            job_origin: Some(job.clone()),
            lineage: vec![receipt],
            committed_revision: None,
            review: vec![],
        });
        m.sources.push(PackageSource {
            source_version: source.clone(),
            kind: SourceKind::Media,
            label: "source".into(),
            identity: ArtifactIdentity {
                artifact_id: ArtifactId::new("source").unwrap(),
                sha256: "3".repeat(64),
                byte_len: 9,
            },
            pinned: false,
            evicted: false,
        });
        m.generated_origins.push(PackageGeneratedOrigin {
            job_id: job,
            attempt_id: AttemptId::new("attempt").unwrap(),
            base_revision: RevisionId::new(0),
            source_version: source,
            manifest: PackageObjectRef {
                sha256: "4".repeat(64),
                byte_len: 9,
            },
            receipt: None,
            program: None,
            state: "completed".into(),
        });
        for (sha, role) in [
            ("3", PackageObjectRole::SourceSnapshot),
            ("4", PackageObjectRole::WorkerManifest),
        ] {
            m.objects.push(PackageObjectDescriptor {
                sha256: sha.repeat(64),
                byte_len: 9,
                roles: vec![role],
            });
        }
        m.objects.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
        m.generated_origins[0].receipt = Some(m.candidates[0].lineage[0].clone());
        m.generated_origins[0].program = Some(ArtifactIdentity {
            artifact_id: m.head.motion.artifact_id.clone(),
            sha256: m.head.motion.sha256.clone(),
            byte_len: m.head.motion.byte_len,
        });
        m.objects
            .iter_mut()
            .find(|o| o.sha256 == EMPTY_SHA)
            .unwrap()
            .roles
            .push(PackageObjectRole::DependencyEvidence);
        m.objects.push(PackageObjectDescriptor {
            sha256: "2".repeat(64),
            byte_len: 9,
            roles: vec![PackageObjectRole::WorkerReceipt],
        });
        m.objects.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        m.counts = m.record_counts().unwrap();
        m.validate().unwrap();
        let program = m.generated_origins[0].program.take();
        assert!(m.validate().is_err());
        m.generated_origins[0].program = program;
        m.generated_origins[0].receipt.as_mut().unwrap().artifact_id =
            ArtifactId::new("different-origin-receipt").unwrap();
        assert!(m.validate().is_err());
    }
    #[test]
    fn retained_history_positions_cannot_have_holes() {
        let mut m = fixture();
        let mut state = m.edit_states[0].clone();
        state.position = 2;
        m.edit_states.push(state);
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
    }
    #[test]
    fn candidate_commit_and_present_revision_lineage_must_agree() {
        let mut m = fixture();
        let candidate = CandidateId::new("c").unwrap();
        let mut r = m.revisions[0].clone();
        r.revision = RevisionId::new(1);
        m.revisions.push(r);
        m.captured_revision = RevisionId::new(1);
        m.head.revision = RevisionId::new(1);
        m.candidates.push(PackageCandidate {
            candidate_id: candidate.clone(),
            base_revision: RevisionId::new(0),
            motion: m.head.motion.clone(),
            job_origin: None,
            lineage: vec![],
            committed_revision: None,
            review: vec![],
        });
        m.revision_lineage.push(PackageRevisionLineage {
            revision: RevisionId::new(1),
            parent_revision: Some(RevisionId::new(0)),
            restored_from_revision: None,
            restored_history_position: None,
            candidate: Some(candidate),
            operation: "candidate_commit".into(),
            contributions: vec![],
            review: PackageReviewState {
                flags: vec![],
                unknown: false,
                conservative: false,
            },
        });
        m.counts = m.record_counts().unwrap();
        assert!(m.validate().is_err());
        m.candidates[0].committed_revision = Some(RevisionId::new(0));
        assert!(m.validate().is_err());
        m.candidates[0].committed_revision = Some(RevisionId::new(1));
        m.validate().unwrap();
    }
    #[test]
    fn manifest_json_is_bounded_before_growing_past_limit() {
        assert!(bounded_json(&"0123456789", 5).is_err());
        let mut m = fixture();
        m.head.name = "x".repeat(257);
        assert!(m.validate().is_err());
    }
}
