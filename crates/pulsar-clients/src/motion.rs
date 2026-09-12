//! Client-local hydrated motion. No partial artifact is visible to callers.
use anyhow::{bail, ensure, Context, Result};
use pulsar_protocol as wire;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use wire::*;

const CACHE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TRANSFER_LIFETIME: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Serialize)]
pub struct ProjectSnapshot {
    pub project_id: ProjectId,
    pub revision: RevisionId,
    pub name: String,
    pub motion: MotionDescriptor,
    pub program: Arc<MotionProgram>,
    pub sources: Vec<SourceSummary>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CandidateSnapshot {
    pub candidate_id: CandidateId,
    pub project_id: ProjectId,
    pub base_revision: RevisionId,
    pub motion: MotionDescriptor,
    pub program: Arc<MotionProgram>,
    pub job_id: Option<JobId>,
    pub review: Vec<ReviewFlag>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResponseBody {
    PackageImport(pulsar_protocol::PackageImportStatus),
    PackageUpload(pulsar_protocol::PackageUploadLease),
    PackageUploadAbandoned {
        lease_id: pulsar_protocol::PackageUploadId,
    },

    PackageExport(PackageExportStatus),
    PackageDownload(PackageDownloadLease),
    PackageDownloadPending(PackageDownloadPending),
    PackageDownloadAbandoned {
        lease_id: PackageDownloadId,
    },
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
    Ack,
}
impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Project(_) => "Project",
            Self::Job(_) => "Job",
            Self::Candidate(_) => "Candidate",
            Self::Diagnostics(_) => "Diagnostics",
            Self::Transfer(_) => "Transfer",
            Self::TransferAbandoned { .. } => "TransferAbandoned",
            Self::Source { .. } => "Source",
            Self::Exported { .. } => "Exported",
            Self::AxisExported { .. } => "AxisExported",
            Self::Preview(_) => "Preview",
            Self::Capabilities(_) => "Capabilities",
            Self::Events(_) => "Events",
            Self::Paired { .. } => "Paired { credentials: [REDACTED] }",
            Self::PackageImport(_) => "PackageImport",
            Self::PackageUpload(_) => "PackageUpload",
            Self::PackageUploadAbandoned { .. } => "PackageUploadAbandoned",
            Self::Ack => "Ack",
            Self::PackageExport(_) => "PackageExport",
            Self::PackageDownload(_) => "PackageDownload",
            Self::PackageDownloadPending(_) => "PackageDownloadPending",
            Self::PackageDownloadAbandoned { .. } => "PackageDownloadAbandoned",
        })
    }
}

/// A local draft, never a wire command. Evidence is discarded by the values codec.
#[derive(Clone, Debug)]
pub struct EditProposal {
    pub program: MotionProgram,
    pub label: String,
}

pub(crate) trait BulkIo {
    fn upload(&mut self, offset: u64, bytes: &[u8]) -> Result<u64>;
    fn download(&mut self, offset: u64, byte_len: u32) -> Result<Vec<u8>>;
}
impl BulkIo for wire::BulkClient {
    fn upload(&mut self, offset: u64, bytes: &[u8]) -> Result<u64> {
        Ok(wire::BulkClient::upload(self, offset, bytes)?)
    }
    fn download(&mut self, offset: u64, byte_len: u32) -> Result<Vec<u8>> {
        Ok(wire::BulkClient::download(self, offset, byte_len)?)
    }
}
pub(crate) trait MotionIo {
    fn control(
        &mut self,
        command: Command,
        project: ProjectId,
        revision: Option<RevisionId>,
    ) -> Result<wire::ResponseBody>;
    fn open_bulk(&mut self, lease: &TransferLease, deadline: Instant) -> Result<Box<dyn BulkIo>>;
    fn bulk_endpoint(&self) -> &Path;
}

#[derive(Default)]
pub(crate) struct MotionCache {
    entries: VecDeque<(ProgramDescriptor, Arc<MotionProgram>)>,
    byte_len: u64,
}
impl MotionCache {
    pub(crate) fn hydrate(
        &mut self,
        response: wire::ResponseBody,
        expected_project: Option<&ProjectId>,
        expected_candidate: Option<&CandidateId>,
        io: &mut impl MotionIo,
    ) -> Result<ResponseBody> {
        Ok(match response {
            wire::ResponseBody::Project(snapshot) => {
                snapshot.validate()?;
                ensure!(
                    expected_project.is_none_or(|id| id == &snapshot.project_id),
                    "Project response identity mismatch"
                );
                let binding = MotionBinding::ProjectRevision {
                    project_id: snapshot.project_id.clone(),
                    revision: snapshot.revision,
                };
                let program = self.program(&snapshot.motion, &binding, io)?;
                ResponseBody::Project(ProjectSnapshot {
                    project_id: snapshot.project_id,
                    revision: snapshot.revision,
                    name: snapshot.name,
                    sources: snapshot.sources,
                    motion: snapshot.motion,
                    program,
                })
            }
            wire::ResponseBody::Candidate(snapshot) => {
                snapshot.validate()?;
                ensure!(
                    expected_project.is_none_or(|id| id == &snapshot.project_id),
                    "Candidate project identity mismatch"
                );
                ensure!(
                    expected_candidate.is_none_or(|id| id == &snapshot.candidate_id),
                    "Candidate response identity mismatch"
                );
                let binding = MotionBinding::Candidate {
                    project_id: snapshot.project_id.clone(),
                    candidate_id: snapshot.candidate_id.clone(),
                    base_revision: snapshot.base_revision,
                };
                let program = self
                    .program(&snapshot.motion, &binding, io)
                    .with_context(|| {
                        format!(
                            "Candidate {} exists, but its motion was not hydrated",
                            snapshot.candidate_id
                        )
                    })?;
                ResponseBody::Candidate(CandidateSnapshot {
                    candidate_id: snapshot.candidate_id,
                    project_id: snapshot.project_id,
                    base_revision: snapshot.base_revision,
                    job_id: snapshot.job_id,
                    review: snapshot.review,
                    motion: snapshot.motion,
                    program,
                })
            }
            wire::ResponseBody::PackageExport(value) => ResponseBody::PackageExport(value),
            wire::ResponseBody::PackageDownload(value) => ResponseBody::PackageDownload(value),
            wire::ResponseBody::PackageDownloadPending(value) => {
                ResponseBody::PackageDownloadPending(value)
            }
            wire::ResponseBody::PackageDownloadAbandoned { lease_id } => {
                ResponseBody::PackageDownloadAbandoned { lease_id }
            }
            wire::ResponseBody::PackageImport(value) => ResponseBody::PackageImport(value),
            wire::ResponseBody::PackageUpload(value) => ResponseBody::PackageUpload(value),
            wire::ResponseBody::PackageUploadAbandoned { lease_id } => {
                ResponseBody::PackageUploadAbandoned { lease_id }
            }
            wire::ResponseBody::Job(value) => ResponseBody::Job(value),
            wire::ResponseBody::Diagnostics(value) => ResponseBody::Diagnostics(value),
            wire::ResponseBody::Transfer(value) => ResponseBody::Transfer(value),
            wire::ResponseBody::TransferAbandoned { lease_id } => {
                ResponseBody::TransferAbandoned { lease_id }
            }
            wire::ResponseBody::Source { source_version } => {
                ResponseBody::Source { source_version }
            }
            wire::ResponseBody::Exported { path, revision } => {
                ResponseBody::Exported { path, revision }
            }
            wire::ResponseBody::AxisExported {
                path,
                revision,
                axis,
            } => ResponseBody::AxisExported {
                path,
                revision,
                axis,
            },
            wire::ResponseBody::Preview(value) => ResponseBody::Preview(value),
            wire::ResponseBody::Capabilities(value) => ResponseBody::Capabilities(value),
            wire::ResponseBody::Events(value) => ResponseBody::Events(value),
            wire::ResponseBody::Paired {
                session,
                auth_token,
            } => ResponseBody::Paired {
                session,
                auth_token,
            },
            wire::ResponseBody::Ack => ResponseBody::Ack,
        })
    }

    fn program(
        &mut self,
        descriptor: &MotionDescriptor,
        binding: &MotionBinding,
        io: &mut impl MotionIo,
    ) -> Result<Arc<MotionProgram>> {
        descriptor.validate()?;
        ensure!(
            &descriptor.binding == binding,
            "Motion descriptor authority binding mismatch"
        );
        if let Some((_, program)) = self
            .entries
            .iter()
            .find(|(key, _)| key == &descriptor.program)
        {
            return Ok(program.clone());
        }
        let started = Instant::now();
        let lease = match io.control(
            Command::BeginMotionDownload {
                locator: binding.locator(),
                offset: 0,
                byte_len: descriptor.program.byte_len,
            },
            binding.project_id().clone(),
            None,
        )? {
            wire::ResponseBody::Transfer(lease) => lease,
            _ => bail!("Unexpected motion download response"),
        };
        let result = (|| {
            validate_lease(
                &lease,
                io.bulk_endpoint(),
                binding.project_id(),
                TransferDirection::Download,
                descriptor.program.byte_len,
                &descriptor.program.sha256,
            )?;
            let deadline = transfer_deadline(&lease, started)?;
            let mut bulk = io.open_bulk(&lease, deadline)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(usize::try_from(descriptor.program.byte_len)?)
                .context("Cannot reserve bounded motion download memory")?;
            while (bytes.len() as u64) < descriptor.program.byte_len {
                ensure!(
                    Instant::now() < deadline,
                    "Motion download lease deadline elapsed"
                );
                let offset = bytes.len() as u64;
                let count = (descriptor.program.byte_len - offset)
                    .min(MAX_ARTIFACT_CHUNK_BYTES as u64) as u32;
                let chunk = bulk.download(offset, count)?;
                ensure!(
                    chunk.len() == count as usize,
                    "Truncated or oversized motion chunk"
                );
                bytes.extend_from_slice(&chunk);
            }
            ensure!(
                Instant::now() < deadline,
                "Motion download lease deadline elapsed"
            );
            validate_program_bytes(&descriptor.program, &bytes).map(Arc::new)
        })();
        if result.is_err() {
            let _ = io.control(
                Command::AbandonTransfer {
                    lease_id: lease.lease_id.clone(),
                },
                binding.project_id().clone(),
                None,
            );
        }
        let program = result?;
        while self.entries.len() >= 8 || self.byte_len + descriptor.program.byte_len > CACHE_BYTES {
            if let Some((old, _)) = self.entries.pop_front() {
                self.byte_len -= old.byte_len;
            } else {
                break;
            }
        }
        self.byte_len += descriptor.program.byte_len;
        self.entries
            .push_back((descriptor.program.clone(), program.clone()));
        Ok(program)
    }

    pub(crate) fn upload_edit(
        &mut self,
        project: ProjectId,
        revision: RevisionId,
        program: &MotionProgram,
        label: &str,
        io: &mut impl MotionIo,
    ) -> Result<CandidateSnapshot> {
        let values = EditValuesProgram::from_program_values(program)?;
        let bytes = bounded_json(&values)?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let started = Instant::now();
        let lease = match io.control(
            Command::BeginEditUpload {
                byte_len: bytes.len() as u64,
                sha256: sha256.clone(),
                label: label.into(),
            },
            project.clone(),
            Some(revision),
        )? {
            wire::ResponseBody::Transfer(lease) => lease,
            _ => bail!("Unexpected edit upload admission response"),
        };
        let uploaded = (|| -> Result<()> {
            validate_lease(
                &lease,
                io.bulk_endpoint(),
                &project,
                TransferDirection::Upload,
                bytes.len() as u64,
                &sha256,
            )?;
            let deadline = transfer_deadline(&lease, started)?;
            let mut bulk = io.open_bulk(&lease, deadline)?;
            let mut offset = 0_usize;
            for chunk in bytes.chunks(MAX_ARTIFACT_CHUNK_BYTES) {
                ensure!(
                    Instant::now() < deadline,
                    "Edit upload lease deadline elapsed"
                );
                let accepted = bulk.upload(offset as u64, chunk)?;
                offset += chunk.len();
                ensure!(
                    accepted == offset as u64,
                    "Edit upload acknowledgement mismatch"
                );
            }
            ensure!(
                Instant::now() < deadline,
                "Edit upload lease deadline elapsed"
            );
            Ok(())
        })();
        if let Err(error) = uploaded {
            let _ = io.control(
                Command::AbandonTransfer {
                    lease_id: lease.lease_id.clone(),
                },
                project,
                None,
            );
            return Err(error.context("Edit upload was not finalized"));
        }
        let response = io.control(Command::FinishEditUpload { lease_id: lease.lease_id.clone() }, project.clone(), Some(revision))
            .with_context(|| format!("Edit finalization for lease {} was not confirmed; inspect TransferStatus before retrying", lease.lease_id))?;
        match &response {
            wire::ResponseBody::Candidate(candidate) => {
                ensure!(
                    candidate.project_id == project && candidate.base_revision == revision,
                    "Finalized candidate did not retain the admitted project/base revision"
                );
            }
            _ => bail!("Unexpected edit finalization response; no commit was requested"),
        }
        match self.hydrate(response, Some(&project), None, io)? {
            ResponseBody::Candidate(candidate) => Ok(candidate),
            _ => unreachable!(),
        }
    }
}

pub(crate) fn validate_lease(
    lease: &TransferLease,
    endpoint: &Path,
    project: &ProjectId,
    direction: TransferDirection,
    byte_len: u64,
    sha256: &str,
) -> Result<()> {
    lease.validate()?;
    ensure!(
        lease.bulk_endpoint == endpoint,
        "Bulk endpoint does not match authenticated engine endpoint"
    );
    ensure!(
        &lease.project_id == project && lease.direction == direction,
        "Transfer lease authority/direction mismatch"
    );
    ensure!(
        lease.sha256 == sha256
            && lease.total_byte_len == byte_len
            && lease.offset == 0
            && lease.byte_len == byte_len
            && lease.accepted_prefix == 0,
        "Transfer lease does not match complete motion artifact"
    );
    ensure!(
        lease.expires_after_ms <= MAX_TRANSFER_LIFETIME.as_millis() as u64,
        "Transfer lease lifetime exceeds client bound"
    );
    Ok(())
}

pub(crate) fn validate_handshake(expected: &TransferLease, actual: &TransferLease) -> Result<()> {
    actual.validate()?;
    ensure!(
        actual.lease_id == expected.lease_id
            && actual.engine_epoch == expected.engine_epoch
            && actual.project_id == expected.project_id
            && actual.direction == expected.direction
            && actual.sha256 == expected.sha256
            && actual.total_byte_len == expected.total_byte_len
            && actual.offset == expected.offset
            && actual.byte_len == expected.byte_len
            && actual.accepted_prefix == expected.accepted_prefix
            && actual.bulk_endpoint == expected.bulk_endpoint
            && actual.expires_after_ms <= expected.expires_after_ms,
        "Bulk handshake receipt changed the admitted lease"
    );
    Ok(())
}
fn transfer_deadline(lease: &TransferLease, started: Instant) -> Result<Instant> {
    started
        .checked_add(Duration::from_millis(lease.expires_after_ms))
        .context("Transfer deadline overflow")
}
fn validate_program_bytes(descriptor: &ProgramDescriptor, bytes: &[u8]) -> Result<MotionProgram> {
    descriptor.validate()?;
    ensure!(
        bytes.len() as u64 == descriptor.byte_len,
        "Motion artifact length mismatch"
    );
    ensure!(
        format!("{:x}", Sha256::digest(bytes)) == descriptor.sha256,
        "Motion artifact SHA-256 mismatch"
    );
    let program: MotionProgram =
        serde_json::from_slice(bytes).context("Motion artifact failed checked decoding")?;
    ensure!(
        program.tracks().len() == descriptor.axes.len(),
        "Motion axis summary mismatch"
    );
    for track in program.tracks() {
        let summary = descriptor
            .axes
            .iter()
            .find(|axis| axis.axis == track.axis())
            .context("Motion axis missing from descriptor")?;
        ensure!(
            summary.action_count == track.actions().len() as u64
                && summary.gap_count == track.gaps().len() as u64,
            "Motion action/gap summary mismatch"
        );
    }
    Ok(program)
}
struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > MAX_MOTION_BYTES.saturating_sub(self.0.len() as u64) {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "motion upload exceeds 64 MiB admission",
            ));
        }
        let needed = self.0.len() + bytes.len();
        if needed > self.0.capacity() {
            let capacity = needed.next_power_of_two().min(MAX_MOTION_BYTES as usize);
            self.0
                .try_reserve_exact(capacity - self.0.len())
                .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded_json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut output = BoundedJson(Vec::new());
    serde_json::to_writer(&mut output, value)?;
    validate_motion_length(output.0.len() as u64)?;
    Ok(output.0)
}

#[cfg(test)]
pub(crate) fn fixture_descriptor(
    program: &MotionProgram,
    binding: MotionBinding,
) -> MotionDescriptor {
    let bytes = serde_json::to_vec(program).unwrap();
    MotionDescriptor {
        program: ProgramDescriptor {
            artifact_id: ArtifactId::new("fixture-motion").unwrap(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            byte_len: bytes.len() as u64,
            codec: MotionCodec::MotionProgramJsonV1,
            axes: program
                .tracks()
                .iter()
                .map(|track| AxisSummary {
                    axis: track.axis(),
                    action_count: track.actions().len() as u64,
                    gap_count: track.gaps().len() as u64,
                })
                .collect(),
        },
        binding,
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    fn project() -> ProjectId {
        ProjectId::new("project").unwrap()
    }
    fn program() -> MotionProgram {
        MotionProgram::new(vec![MotionTrack::new(
            Axis::Stroke,
            vec![
                MotionAction::new(
                    ProjectTime::from_nanos(0),
                    NormalizedPosition::new(0.2).unwrap(),
                    EvidenceKind::Observed,
                )
                .unwrap(),
                MotionAction::new(
                    ProjectTime::from_nanos(100),
                    NormalizedPosition::new(0.8).unwrap(),
                    EvidenceKind::Observed,
                )
                .unwrap(),
            ],
        )
        .unwrap()])
        .unwrap()
    }
    fn binding() -> MotionBinding {
        MotionBinding::ProjectRevision {
            project_id: project(),
            revision: RevisionId::new(1),
        }
    }
    struct MockBulk {
        bytes: Vec<u8>,
        uploaded: Rc<RefCell<Vec<u8>>>,
        corrupt: bool,
        truncated: bool,
        bad_ack: bool,
    }
    impl BulkIo for MockBulk {
        fn upload(&mut self, offset: u64, bytes: &[u8]) -> Result<u64> {
            self.uploaded.borrow_mut().extend_from_slice(bytes);
            Ok(offset + bytes.len() as u64 + u64::from(self.bad_ack))
        }
        fn download(&mut self, offset: u64, len: u32) -> Result<Vec<u8>> {
            let mut bytes = self.bytes[offset as usize..offset as usize + len as usize].to_vec();
            if self.corrupt {
                bytes[0] ^= 1;
            }
            if self.truncated {
                bytes.pop();
            }
            Ok(bytes)
        }
    }
    struct MockIo {
        program: MotionProgram,
        commands: Vec<Command>,
        opens: usize,
        endpoint: PathBuf,
        uploaded: Rc<RefCell<Vec<u8>>>,
        corrupt: bool,
        truncated: bool,
        bad_ack: bool,
        redirect: bool,
    }
    impl MockIo {
        fn new() -> Self {
            Self {
                program: program(),
                commands: Vec::new(),
                opens: 0,
                endpoint: PathBuf::from("/private/bulk.sock"),
                uploaded: Rc::new(RefCell::new(Vec::new())),
                corrupt: false,
                truncated: false,
                bad_ack: false,
                redirect: false,
            }
        }
        fn lease(&self, direction: TransferDirection, len: u64, hash: String) -> TransferLease {
            TransferLease {
                lease_id: TransferId::new("lease").unwrap(),
                engine_epoch: "epoch".into(),
                project_id: project(),
                direction,
                sha256: hash,
                total_byte_len: len,
                offset: 0,
                byte_len: len,
                accepted_prefix: 0,
                expires_after_ms: 300_000,
                bulk_endpoint: if self.redirect {
                    PathBuf::from("/attacker/bulk.sock")
                } else {
                    self.endpoint.clone()
                },
            }
        }
        fn descriptor(&self) -> MotionDescriptor {
            fixture_descriptor(&self.program, binding())
        }
    }
    impl MotionIo for MockIo {
        fn control(
            &mut self,
            command: Command,
            scope: ProjectId,
            revision: Option<RevisionId>,
        ) -> Result<wire::ResponseBody> {
            assert_eq!(scope, project());
            self.commands.push(command.clone());
            Ok(match command {
                Command::BeginMotionDownload {
                    offset, byte_len, ..
                } => {
                    assert_eq!(offset, 0);
                    wire::ResponseBody::Transfer(self.lease(
                        TransferDirection::Download,
                        byte_len,
                        self.descriptor().program.sha256,
                    ))
                }
                Command::BeginEditUpload {
                    byte_len, sha256, ..
                } => {
                    assert_eq!(revision, Some(RevisionId::new(1)));
                    wire::ResponseBody::Transfer(self.lease(
                        TransferDirection::Upload,
                        byte_len,
                        sha256,
                    ))
                }
                Command::FinishEditUpload { .. } => {
                    let id = CandidateId::new("candidate").unwrap();
                    wire::ResponseBody::Candidate(wire::CandidateSnapshot {
                        candidate_id: id.clone(),
                        project_id: project(),
                        base_revision: RevisionId::new(1),
                        motion: fixture_descriptor(
                            &self.program,
                            MotionBinding::Candidate {
                                project_id: project(),
                                candidate_id: id,
                                base_revision: RevisionId::new(1),
                            },
                        ),
                        job_id: None,
                        review: Vec::new(),
                    })
                }
                Command::AbandonTransfer { lease_id } => {
                    wire::ResponseBody::TransferAbandoned { lease_id }
                }
                _ => bail!("Unexpected mock transfer command"),
            })
        }
        fn open_bulk(&mut self, _: &TransferLease, _: Instant) -> Result<Box<dyn BulkIo>> {
            self.opens += 1;
            Ok(Box::new(MockBulk {
                bytes: serde_json::to_vec(&self.program)?,
                uploaded: self.uploaded.clone(),
                corrupt: self.corrupt,
                truncated: self.truncated,
                bad_ack: self.bad_ack,
            }))
        }
        fn bulk_endpoint(&self) -> &Path {
            &self.endpoint
        }
    }

    #[test]
    fn complete_digest_verified_program_is_cached_and_reused() {
        let mut io = MockIo::new();
        let descriptor = io.descriptor();
        let mut cache = MotionCache::default();
        let first = cache.program(&descriptor, &binding(), &mut io).unwrap();
        let second = cache.program(&descriptor, &binding(), &mut io).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(io.opens, 1);
    }
    #[test]
    fn descriptor_wrong_authority_rejects_before_transfer_or_cache_lookup() {
        let mut io = MockIo::new();
        let descriptor = io.descriptor();
        let mut cache = MotionCache::default();
        cache.program(&descriptor, &binding(), &mut io).unwrap();
        let mut bad = descriptor.clone();
        bad.binding = MotionBinding::ProjectRevision {
            project_id: ProjectId::new("other").unwrap(),
            revision: RevisionId::new(1),
        };
        assert!(cache.program(&bad, &binding(), &mut io).is_err());
        assert_eq!(io.opens, 1);
    }
    #[test]
    fn bad_hash_truncated_data_and_false_summaries_are_never_cached() {
        for fault in 0..3 {
            let mut io = MockIo::new();
            let mut descriptor = io.descriptor();
            let mut cache = MotionCache::default();
            if fault == 0 {
                io.corrupt = true;
            }
            if fault == 1 {
                io.truncated = true;
            }
            if fault == 2 {
                descriptor.program.axes[0].action_count += 1;
            }
            assert!(cache.program(&descriptor, &binding(), &mut io).is_err());
            assert!(cache.entries.is_empty());
            assert!(io
                .commands
                .iter()
                .any(|command| matches!(command, Command::AbandonTransfer { .. })));
        }
    }
    #[test]
    fn redirected_bulk_endpoint_never_receives_authentication() {
        let mut io = MockIo::new();
        io.redirect = true;
        let descriptor = io.descriptor();
        let mut cache = MotionCache::default();
        assert!(cache.program(&descriptor, &binding(), &mut io).is_err());
        assert_eq!(io.opens, 0);
    }
    #[test]
    fn handshake_cannot_change_grant_binding_epoch_or_expiry() {
        let io = MockIo::new();
        let descriptor = io.descriptor();
        let lease = io.lease(
            TransferDirection::Download,
            descriptor.program.byte_len,
            descriptor.program.sha256,
        );
        for mutation in 0..4 {
            let mut reply = lease.clone();
            match mutation {
                0 => reply.engine_epoch = "other-epoch".into(),
                1 => reply.project_id = ProjectId::new("other").unwrap(),
                2 => reply.expires_after_ms += 1,
                _ => reply.accepted_prefix = 1,
            }
            assert!(validate_handshake(&lease, &reply).is_err());
        }
    }
    #[test]
    fn bad_upload_ack_never_finalizes_and_wire_values_contain_no_evidence() {
        let mut io = MockIo::new();
        io.bad_ack = true;
        let mut cache = MotionCache::default();
        assert!(cache
            .upload_edit(project(), RevisionId::new(1), &program(), "Edit", &mut io)
            .is_err());
        assert!(!io
            .commands
            .iter()
            .any(|command| matches!(command, Command::FinishEditUpload { .. })));
        let text = String::from_utf8(io.uploaded.borrow().clone()).unwrap();
        assert!(!text.contains("evidence"));
        assert!(!text.contains("observed"));
        assert!(!text.contains("provenance"));
        let _: EditValuesProgram = serde_json::from_str(&text).unwrap();
    }
    #[test]
    fn successful_shared_edit_upload_returns_candidate_without_a_commit() {
        let mut io = MockIo::new();
        let mut cache = MotionCache::default();
        let candidate = cache
            .upload_edit(project(), RevisionId::new(1), &program(), "Edit", &mut io)
            .unwrap();
        assert_eq!(candidate.base_revision, RevisionId::new(1));
        assert_eq!(candidate.program.tracks()[0].actions().len(), 2);
        assert!(!io
            .commands
            .iter()
            .any(|command| matches!(command, Command::CommitCandidate { .. })));
    }
    #[test]
    fn cache_has_an_entry_count_bound_even_for_tiny_programs() {
        let mut io = MockIo::new();
        let mut cache = MotionCache::default();
        for index in 0..12 {
            let mut descriptor = io.descriptor();
            descriptor.program.artifact_id = ArtifactId::new(format!("artifact-{index}")).unwrap();
            cache.program(&descriptor, &binding(), &mut io).unwrap();
        }
        assert!(
            cache.entries.len() <= 8,
            "byte bounds alone do not bound cache metadata"
        );
    }
}
