//! P1a logical project closure. This is not an export/import operation or a byte hold.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

const MAX_CAPTURE_METADATA: usize = 256 * 1024 * 1024;
const MAX_CAPTURE_ROWS: usize = 100_000;
const MAX_COMPACT_EVIDENCE: u64 = 128 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;

enum ObjectLocation {
    File(PathBuf),
    Inline(Arc<[u8]>),
}
struct CapturedObject {
    descriptor: PackageObjectDescriptor,
    location: ObjectLocation,
}

/// The logical records are immutable, but filesystem availability was only
/// observed. P1b must acquire real eviction holds and revalidate while streaming.
pub(super) struct ProjectCapturePlan {
    pub(super) manifest: PortableProjectManifest,
    pub(super) manifest_sha256: String,
    objects: BTreeMap<String, CapturedObject>,
    _holds: Option<crate::project_package_storage::ArtifactHolds>,
    _memory: Reservation,
}
impl ProjectCapturePlan {
    pub(super) fn open_object(
        &self,
        descriptor: &PackageObjectDescriptor,
    ) -> PResult<Box<dyn Read>> {
        let object = self.objects.get(&descriptor.sha256).ok_or_else(|| {
            ProtocolError::invalid("object is not in the captured logical closure")
        })?;
        if object.descriptor != *descriptor {
            return Err(ProtocolError::invalid("capture object descriptor mismatch"));
        }
        open_object(object)
    }
}
fn missing(object: &PackageObjectDescriptor, detail: &str) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::DependencyMismatch,
        format!(
            "required {:?} object {}: {detail}",
            object.roles, object.sha256
        ),
    )
}
fn open_object(object: &CapturedObject) -> PResult<Box<dyn Read>> {
    match &object.location {
        ObjectLocation::Inline(bytes) => Ok(Box::new(Cursor::new(bytes.clone()))),
        ObjectLocation::File(path) => artifacts::open_regular(path)
            .map(|file| Box::new(file) as Box<dyn Read>)
            .map_err(|_| missing(&object.descriptor, "missing or unsafe evidence")),
    }
}
fn add_object(
    objects: &mut BTreeMap<String, CapturedObject>,
    reference: PackageObjectRef,
    role: PackageObjectRole,
    location: ObjectLocation,
) -> PResult<()> {
    ArtifactIdentity {
        artifact_id: ArtifactId::new(format!("package.{}", reference.sha256)).map_err(internal)?,
        sha256: reference.sha256.clone(),
        byte_len: reference.byte_len,
    }
    .validate()?;
    if let Some(old) = objects.get_mut(&reference.sha256) {
        if old.descriptor.byte_len != reference.byte_len {
            return Err(ProtocolError::invalid(
                "same digest has conflicting object lengths",
            ));
        }
        if !old.descriptor.roles.contains(&role) {
            old.descriptor.roles.push(role);
            old.descriptor.roles.sort();
        }
        return Ok(());
    }
    if objects.len() >= MAX_CAPTURE_ROWS {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "too many capture objects",
        ));
    }
    objects.insert(
        reference.sha256.clone(),
        CapturedObject {
            descriptor: PackageObjectDescriptor {
                sha256: reference.sha256,
                byte_len: reference.byte_len,
                roles: vec![role],
            },
            location,
        },
    );
    Ok(())
}
fn object_ref(identity: &ArtifactIdentity) -> PackageObjectRef {
    PackageObjectRef {
        sha256: identity.sha256.clone(),
        byte_len: identity.byte_len,
    }
}
fn inline_object(
    objects: &mut BTreeMap<String, CapturedObject>,
    bytes: Vec<u8>,
    role: PackageObjectRole,
) -> PResult<PackageObjectRef> {
    let reference = PackageObjectRef {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        byte_len: bytes.len() as u64,
    };
    add_object(
        objects,
        reference.clone(),
        role,
        ObjectLocation::Inline(bytes.into()),
    )?;
    Ok(reference)
}
fn motion(
    db: &Connection,
    root: &Path,
    reference: &str,
    objects: &mut BTreeMap<String, CapturedObject>,
) -> PResult<ProgramDescriptor> {
    let descriptor = motion_state::descriptor(db, reference)?;
    add_object(
        objects,
        PackageObjectRef {
            sha256: descriptor.sha256.clone(),
            byte_len: descriptor.byte_len,
        },
        PackageObjectRole::MotionProgram,
        ObjectLocation::File(root.join("motion-objects").join(&descriptor.sha256)),
    )?;
    Ok(descriptor)
}
fn unsigned(value: i64) -> PResult<u64> {
    u64::try_from(value).map_err(|_| ProtocolError::invalid("negative durable project coordinate"))
}
fn revision(value: i64) -> PResult<RevisionId> {
    Ok(RevisionId::new(unsigned(value)?))
}
fn optional_revision(value: Option<i64>) -> PResult<Option<RevisionId>> {
    value.map(revision).transpose()
}
fn review(raw: &str) -> PResult<PackageReviewState> {
    let state: lineage::ReviewState = decode(raw)?;
    validate_reviews(&state.flags)?;
    Ok(PackageReviewState {
        flags: state.flags,
        unknown: state.unknown,
        conservative: state.conservative,
    })
}
struct CaptureIndexes<'a> {
    candidates: BTreeMap<&'a str, &'a PackageCandidate>,
    revisions: BTreeMap<RevisionId, &'a PackageRevision>,
    sources: BTreeMap<&'a str, &'a PackageSource>,
    candidates_by_job: BTreeMap<&'a str, Vec<&'a PackageCandidate>>,
}
impl<'a> CaptureIndexes<'a> {
    fn new(
        candidates: &'a [PackageCandidate],
        revisions: &'a [PackageRevision],
        sources: &'a [PackageSource],
    ) -> Self {
        let mut candidates_by_job: BTreeMap<&str, Vec<&PackageCandidate>> = BTreeMap::new();
        for candidate in candidates {
            if let Some(job) = &candidate.job_origin {
                candidates_by_job
                    .entry(job.as_str())
                    .or_default()
                    .push(candidate);
            }
        }
        Self {
            candidates: candidates
                .iter()
                .map(|candidate| (candidate.candidate_id.as_str(), candidate))
                .collect(),
            revisions: revisions
                .iter()
                .map(|revision| (revision.revision, revision))
                .collect(),
            sources: sources
                .iter()
                .map(|source| (source.source_version.as_str(), source))
                .collect(),
            candidates_by_job,
        }
    }
}

#[derive(Default)]
struct SnapshotBudget {
    rows: usize,
    bytes: usize,
}
fn rows<T>(
    db: &Connection,
    sql: &str,
    project: &ProjectId,
    budget: &mut SnapshotBudget,
    mut convert: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> PResult<Vec<T>> {
    let mut statement = db.prepare(sql).map_err(internal)?;
    let mut cursor = statement.query([project.as_str()]).map_err(internal)?;
    let mut output = Vec::new();
    while let Some(row) = cursor.next().map_err(internal)? {
        budget.rows = budget
            .rows
            .checked_add(1)
            .ok_or_else(|| internal("capture count overflow"))?;
        for column in 0..row.as_ref().column_count() {
            let length = match row.get_ref(column).map_err(internal)? {
                rusqlite::types::ValueRef::Text(value) | rusqlite::types::ValueRef::Blob(value) => {
                    value.len()
                }
                _ => 16,
            };
            budget.bytes = budget
                .bytes
                .checked_add(length + 64)
                .ok_or_else(|| internal("capture size overflow"))?;
        }
        if budget.rows > MAX_CAPTURE_ROWS || budget.bytes > MAX_CAPTURE_METADATA {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "P1a capture metadata admission exceeded",
            ));
        }
        output.push(convert(row).map_err(internal)?);
    }
    Ok(output)
}
fn authenticate_capture(
    engine: &Engine,
    db: &Connection,
    session: &SessionId,
    token: &str,
    project: &ProjectId,
    expected: RevisionId,
) -> PResult<()> {
    engine.authenticate(db, session, Some(token))?;
    require_grant(db, session, project, Scope::PackageProject)?;
    let actual: Option<i64> = db
        .query_row(
            "SELECT revision FROM projects WHERE id=?1",
            [project.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let actual =
        actual.ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "project not found"))?;
    if revision(actual)? != expected {
        return Err(ProtocolError::new(
            ErrorCode::RevisionConflict,
            "package capture requires current expected revision",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditReceipt {
    format: String,
    kernel: String,
    lease_id: TransferId,
    project_id: ProjectId,
    base_revision: RevisionId,
    base_program: ProgramDescriptor,
    upload_sha256: String,
    upload_byte_len: u64,
    result_program: ProgramDescriptor,
    authored_provenance: pulsar_core::ProvenanceRef,
    authored_ranges: Vec<(Axis, TimeRange)>,
    inherited_ranges: Vec<(Axis, TimeRange)>,
    actor_id: String,
    label: String,
}

#[derive(Deserialize)]
struct WorkerReceiptEnvelope {
    schema_version: u16,
    recipe: String,
    project_id: ProjectId,
    base_revision: RevisionId,
    job_id: JobId,
    attempt_id: AttemptId,
    source_version: SourceVersionId,
    program: ArtifactIdentity,
    dependencies: Vec<ArtifactIdentity>,
}

fn read_compact(
    path: &Path,
    identity: &ArtifactIdentity,
    role: PackageObjectRole,
) -> PResult<Vec<u8>> {
    let descriptor = PackageObjectDescriptor {
        sha256: identity.sha256.clone(),
        byte_len: identity.byte_len,
        roles: vec![role],
    };
    if identity.byte_len > MAX_COMPACT_EVIDENCE {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "compact evidence exceeds capture bound",
        ));
    }
    let mut bytes = Vec::new();
    artifacts::open_regular(path)
        .map_err(|_| missing(&descriptor, "missing or unsafe evidence"))?
        .take(MAX_COMPACT_EVIDENCE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| missing(&descriptor, "evidence read failed"))?;
    if bytes.len() as u64 != identity.byte_len
        || format!("{:x}", Sha256::digest(&bytes)) != identity.sha256
    {
        return Err(missing(&descriptor, "evidence digest or length mismatch"));
    }
    Ok(bytes)
}

/// Private capture seam only: no request replay, operation readiness or transfer
/// authority is created. All record reads share one mutex-protected transaction.

pub(super) fn capture(
    engine: &Engine,
    session: &SessionId,
    token: &str,
    project: &ProjectId,
    expected_revision: RevisionId,
) -> PResult<ProjectCapturePlan> {
    capture_inner(engine, session, token, project, expected_revision, false)
}
/// Retains the captured closure before releasing the SQL snapshot. Full payload
/// verification is deferred to bounded materialization, not claimed by capture.
pub(super) fn capture_retained(
    engine: &Engine,
    session: &SessionId,
    token: &str,
    project: &ProjectId,
    expected_revision: RevisionId,
) -> PResult<ProjectCapturePlan> {
    capture_inner(engine, session, token, project, expected_revision, true)
}

fn capture_inner(
    engine: &Engine,
    session: &SessionId,
    token: &str,
    project: &ProjectId,
    expected_revision: RevisionId,
    retained: bool,
) -> PResult<ProjectCapturePlan> {
    let memory = engine.pool.reserve_memory(transfers::VALIDATION_MEMORY)?;
    let gate = if retained {
        Some(engine.artifact_gate.lock().map_err(internal)?)
    } else {
        None
    };
    let mut retained_holds = None;
    let root = &engine.config.state_dir;
    let (mut manifest, mut objects, legacy, authored, attempts) = {
        let mut db = engine.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .map_err(internal)?;
        authenticate_capture(engine, &tx, session, token, project, expected_revision)?;
        let mut budget = SnapshotBudget::default();
        let mut objects = BTreeMap::new();
        let head: (String, i64, String, i64, String) = tx
            .query_row(
                "SELECT name,revision,program,history_cursor,protected FROM projects WHERE id=?1",
                [project.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(internal)?;
        let head = PackageProjectHead {
            name: head.0,
            revision: revision(head.1)?,
            motion: motion(&tx, root, &head.2, &mut objects)?,
            history_cursor: unsigned(head.3)?,
            protected: decode(&head.4)?,
        };
        let event_cursor: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(cursor),0) FROM events WHERE project=?1",
                [project.as_str()],
                |r| r.get(0),
            )
            .map_err(internal)?;
        let mut revisions = Vec::new();
        for (rev, actor, kind, label, program, protected) in rows(&tx,
            "SELECT revision,actor,kind,label,program,protected FROM revisions WHERE project=?1 ORDER BY revision",
            project, &mut budget, |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))? {
            revisions.push(PackageRevision {revision: revision(rev)?,actor,kind,label,
                motion: motion(&tx,root,&program,&mut objects)?,protected: decode(&protected)?});
        }
        let mut edit_states = Vec::new();
        for (position, program, protected, origin) in rows(&tx,
            "SELECT e.position,e.program,e.protected,h.revision FROM edit_states e LEFT JOIN history_lineage h ON h.project=e.project AND h.position=e.position WHERE e.project=?1 ORDER BY e.position",
            project, &mut budget, |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<i64>>(3)?)))? {
            edit_states.push(PackageEditState {position: unsigned(position)?,motion: motion(&tx,root,&program,&mut objects)?,
                protected:decode(&protected)?,source_revision:optional_revision(origin)?});
        }
        let mut revision_lineage = Vec::new();
        for (rev,parent,restored,position,candidate,operation,contributions,state) in rows(&tx,
            "SELECT revision,parent_revision,restored_from_revision,restored_history_position,candidate,operation,contributions,review_state FROM revision_lineage WHERE project=?1 ORDER BY revision",
            project,&mut budget,|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,Option<i64>>(3)?,
                r.get::<_,Option<String>>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?)))? {
            revision_lineage.push(PackageRevisionLineage {revision:revision(rev)?,parent_revision:optional_revision(parent)?,
                restored_from_revision:optional_revision(restored)?,restored_history_position:position.map(unsigned).transpose()?,
                candidate:candidate.map(CandidateId::new).transpose().map_err(internal)?,operation,
                contributions:decode(&contributions)?,review:review(&state)?});
        }
        let mut candidates = Vec::new();
        for (id,base,program,job,lineage,committed,flags) in rows(&tx,
            "SELECT id,base_revision,program,job,lineage,committed_revision,review FROM candidates WHERE project=?1 ORDER BY id",
            project,&mut budget,|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,
                r.get::<_,String>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,String>(6)?)))? {
            candidates.push(PackageCandidate {candidate_id:CandidateId::new(id).map_err(internal)?,base_revision:revision(base)?,
                motion:motion(&tx,root,&program,&mut objects)?,job_origin:job.map(JobId::new).transpose().map_err(internal)?,
                lineage:decode(&lineage)?,committed_revision:optional_revision(committed)?,review:decode(&flags)?});
        }
        let mut sources = Vec::new();
        for (version,kind,label,hash,bytes,pinned,evicted,path) in rows(&tx,
            "SELECT version,kind,label,sha256,bytes,pinned,evicted,path FROM sources WHERE project=?1 ORDER BY version",
            project,&mut budget,|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,
                r.get::<_,i64>(4)?,r.get::<_,bool>(5)?,r.get::<_,bool>(6)?,r.get::<_,String>(7)?)))? {
            let identity=ArtifactIdentity {artifact_id:ArtifactId::new(hash.clone()).map_err(internal)?,sha256:hash.clone(),byte_len:unsigned(bytes)?};
            identity.validate()?;
            let expected_path=root.join("snapshots").join(&hash);
            let descriptor=PackageObjectDescriptor {sha256:hash,byte_len:identity.byte_len,roles:vec![PackageObjectRole::SourceSnapshot]};
            if evicted || Path::new(&path)!=expected_path {
                return Err(missing(&descriptor,"snapshot evicted or trusted locator unavailable"));
            }
            add_object(&mut objects,object_ref(&identity),PackageObjectRole::SourceSnapshot,ObjectLocation::File(expected_path))?;
            sources.push(PackageSource {source_version:SourceVersionId::new(version).map_err(internal)?,
                kind:decode(&encode(&kind)?)?,label,identity,pinned,evicted});
        }
        let authored=rows(&tx,
            "SELECT l.candidate,l.receipt,l.review_state,l.authored_count,l.inherited_count FROM edit_candidate_lineage l JOIN candidates c ON c.id=l.candidate WHERE c.project=?1 ORDER BY l.candidate",
            project,&mut budget,|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?)))?;
        let legacy=rows(&tx,
            "SELECT owner_table,row_key,program FROM legacy_motion_bytes WHERE (owner_table='projects' AND row_key=?1) OR (owner_table IN ('revisions','edit_states') AND substr(row_key,1,length(?1)+1)=?1||':' AND instr(substr(row_key,length(?1)+2),':')=0) OR (owner_table='candidates' AND row_key IN (SELECT id FROM candidates WHERE project=?1)) ORDER BY owner_table,row_key",
            project,&mut budget,|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
        let attempts=rows(&tx,
            "SELECT a.id,a.job,a.manifest,a.state FROM attempts a JOIN jobs j ON j.id=a.job WHERE j.project=?1 ORDER BY a.job,a.id",
            project,&mut budget,|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?;
        let mut export_receipts = Vec::new();
        for (rev,axis,hash,state,path) in rows(&tx,
            "SELECT revision,axis,sha256,state,path FROM exports WHERE project=?1 ORDER BY revision,axis,sha256,state,path",
            project,&mut budget,|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))? {
            let state=match state.as_str() {"pending"=>PackageExportState::Pending,"complete"=>PackageExportState::Complete,
                "failed_or_unknown"=>PackageExportState::FailedOrUnknown,_=>return Err(ProtocolError::unsupported("unknown archival export state"))};
            export_receipts.push(PackageExportReceipt {revision:revision(rev)?,axis:decode(&encode(&axis)?)?,sha256:hash,state,archival_path:Some(path)});
        }
        let manifest = PortableProjectManifest {
            format_version: 1,
            origin_project_id: project.clone(),
            captured_revision: expected_revision,
            captured_event_cursor: unsigned(event_cursor)?,
            completeness: PackageCompleteness::default(),
            counts: PackageRecordCounts::default(),
            head,
            revisions,
            edit_states,
            revision_lineage,
            candidates,
            authored_lineage: vec![],
            sources,
            legacy_cells: vec![],
            generated_origins: vec![],
            export_receipts,
            objects: vec![],
        };
        if retained {
            let mut identities = objects
                .values()
                .map(|object| ArtifactIdentity {
                    artifact_id: ArtifactId::new(format!(
                        "package.hold.{}",
                        object.descriptor.sha256
                    ))
                    .expect("digest identifier"),
                    sha256: object.descriptor.sha256.clone(),
                    byte_len: object.descriptor.byte_len,
                })
                .collect::<Vec<_>>();
            identities.extend(
                manifest
                    .candidates
                    .iter()
                    .flat_map(|candidate| candidate.lineage.iter().cloned()),
            );
            retained_holds = Some(engine.package_holds.acquire(&identities)?);
        }
        tx.commit().map_err(internal)?;
        (manifest, objects, legacy, authored, attempts)
    };
    drop(gate);
    // No external object read or hash occurs while the authority mutex is held.
    // Snapshot/source holds already exist; receipt/input/attempt files have no deletion path.
    for (owner, key, raw) in legacy {
        let owner = match owner.as_str() {
            "projects" => PackageLegacyOwner::Projects,
            "candidates" => PackageLegacyOwner::Candidates,
            "revisions" | "edit_states" => {
                let Some((owner_id, suffix)) = key.rsplit_once(':') else {
                    return Err(ProtocolError::invalid("malformed legacy recovery key"));
                };
                if owner_id != project.as_str() {
                    continue;
                }
                if suffix.is_empty()
                    || !suffix.bytes().all(|byte| byte.is_ascii_digit())
                    || suffix.parse::<u64>().is_err()
                {
                    return Err(ProtocolError::invalid(
                        "malformed legacy recovery coordinate",
                    ));
                }
                if owner == "revisions" {
                    PackageLegacyOwner::Revisions
                } else {
                    PackageLegacyOwner::EditStates
                }
            }
            _ => return Err(ProtocolError::unsupported("unknown legacy recovery owner")),
        };
        let object = inline_object(
            &mut objects,
            raw.into_bytes(),
            PackageObjectRole::LegacyMotionBytes,
        )?;
        manifest.legacy_cells.push(PackageLegacyCell {
            owner,
            row_key: key,
            object,
        });
    }
    manifest
        .legacy_cells
        .sort_by(|a, b| (&a.owner, &a.row_key).cmp(&(&b.owner, &b.row_key)));
    let indexes = CaptureIndexes::new(&manifest.candidates, &manifest.revisions, &manifest.sources);
    for (candidate, identity, state, authored_count, inherited_count) in authored {
        let candidate_id = CandidateId::new(candidate).map_err(internal)?;
        let identity: ArtifactIdentity = decode(&identity)?;
        identity.validate()?;
        let path = root.join("edit-receipts").join(&identity.sha256);
        let bytes = read_compact(&path, &identity, PackageObjectRole::EditReceipt)?;
        let receipt: EditReceipt = serde_json::from_slice(&bytes)
            .map_err(|_| ProtocolError::invalid("unsupported authored receipt schema"))?;
        let candidate = indexes
            .candidates
            .get(candidate_id.as_str())
            .copied()
            .ok_or_else(|| ProtocolError::invalid("authored lineage candidate is absent"))?;
        let base = indexes
            .revisions
            .get(&receipt.base_revision)
            .copied()
            .ok_or_else(|| ProtocolError::invalid("authored receipt origin revision is absent"))?;
        if receipt.format != "pulsar-edit-receipt-v1"
            || receipt.kernel != pulsar_core::EDIT_VALUES_KERNEL_VERSION
            || receipt.project_id != *project
            || receipt.base_program != base.motion
            || receipt.result_program != candidate.motion
            || receipt.authored_ranges.len() as u64 != unsigned(authored_count)?
            || receipt.inherited_ranges.len() as u64 != unsigned(inherited_count)?
        {
            return Err(ProtocolError::invalid(
                "authored receipt lineage disagrees with captured graph",
            ));
        }
        for (_, range) in receipt
            .authored_ranges
            .iter()
            .chain(&receipt.inherited_ranges)
        {
            if range.start() < ProjectTime::ZERO || range.start() >= range.end() {
                return Err(ProtocolError::invalid("invalid authored receipt interval"));
            }
        }
        let _inert_attribution = (
            &receipt.lease_id,
            &receipt.authored_provenance,
            &receipt.actor_id,
            &receipt.label,
        );
        let input = PackageObjectRef {
            sha256: receipt.upload_sha256,
            byte_len: receipt.upload_byte_len,
        };
        add_object(
            &mut objects,
            input.clone(),
            PackageObjectRole::EditInput,
            ObjectLocation::File(root.join("edit-inputs").join(&input.sha256)),
        )?;
        add_object(
            &mut objects,
            object_ref(&identity),
            PackageObjectRole::EditReceipt,
            ObjectLocation::File(path),
        )?;
        manifest.authored_lineage.push(PackageAuthoredLineage {
            candidate_id,
            receipt: identity,
            input,
            review: review(&state)?,
            authored_count: unsigned(authored_count)?,
            inherited_count: unsigned(inherited_count)?,
        });
    }
    let mut dependency_identities = BTreeSet::new();
    for (attempt, job, raw, state) in attempts {
        let request: WorkerRequest = decode(&raw)?;
        request.validate()?;
        let attempt_id = AttemptId::new(attempt).map_err(internal)?;
        let job_id = JobId::new(job).map_err(internal)?;
        if request.project_id != *project
            || request.attempt_id != attempt_id
            || request.job_id != job_id
            || request.output_dir != root.join("attempts").join(attempt_id.as_str())
        {
            return Err(ProtocolError::invalid(
                "historical worker manifest identity mismatch",
            ));
        }
        let primary = indexes
            .sources
            .get(request.source.source_version.as_str())
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::DependencyMismatch,
                    format!(
                        "required primary source {} is absent from the captured catalog",
                        request.source.source_version
                    ),
                )
            })?;
        if primary.identity != request.source.identity
            || !objects.get(&primary.identity.sha256).is_some_and(|object| {
                object.descriptor.byte_len == primary.identity.byte_len
                    && object
                        .descriptor
                        .roles
                        .contains(&PackageObjectRole::SourceSnapshot)
            })
        {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                format!(
                    "primary source {} identity differs from the captured source payload",
                    request.source.source_version
                ),
            ));
        }
        let mut allowed_dependencies = request.dependencies.clone();
        allowed_dependencies.push(request.source.identity.clone());
        allowed_dependencies.push(request.tools.ffmpeg.identity.clone());
        allowed_dependencies.push(request.tools.ffprobe.identity.clone());
        if let Some(runtime) = &request.tools.onnx_runtime {
            allowed_dependencies.push(runtime.identity.clone());
        }
        if let WorkerOperation::Generate {
            model: Some(model), ..
        }
        | WorkerOperation::Preview {
            model: Some(model), ..
        } = &request.operation
        {
            allowed_dependencies.push(model.identity.clone());
        }
        for identity in &allowed_dependencies {
            identity.validate()?;
            dependency_identities.insert((identity.sha256.clone(), identity.byte_len));
        }
        let candidates = indexes
            .candidates_by_job
            .get(job_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut receipt = None;
        let mut original_program_identity = None;
        if !candidates.is_empty() && state == "completed" {
            for candidate in candidates.iter() {
                let identity = candidate.lineage.last().ok_or_else(|| {
                    ProtocolError::invalid("generated candidate receipt identity is absent")
                })?;
                if receipt.as_ref().is_some_and(|old| old != identity) {
                    return Err(ProtocolError::invalid(
                        "generated candidates disagree on receipt identity",
                    ));
                }
                receipt = Some(identity.clone());
            }
        }
        if let Some(identity) = &receipt {
            let attempt_root = root.join("attempts").join(attempt_id.as_str());
            let path = attempt_root.join("receipt.json");
            let raw_receipt = read_compact(&path, identity, PackageObjectRole::WorkerReceipt)?;
            let envelope: WorkerReceiptEnvelope = serde_json::from_slice(&raw_receipt)
                .map_err(|_| ProtocolError::unsupported("unsupported generated receipt schema"))?;
            if !matches!(envelope.schema_version, 1 | 2)
                || envelope.recipe.is_empty()
                || envelope.project_id != *project
                || envelope.job_id != job_id
                || envelope.attempt_id != attempt_id
                || envelope.base_revision != request.base_revision
                || envelope.source_version != request.source.source_version
            {
                return Err(ProtocolError::invalid(
                    "generated receipt disagrees with captured origin",
                ));
            }
            for dependency in &envelope.dependencies {
                dependency.validate()?;
                if !allowed_dependencies.contains(dependency) {
                    return Err(ProtocolError::new(
                        ErrorCode::DependencyMismatch,
                        format!(
                            "unresolved generated receipt dependency {}",
                            dependency.artifact_id
                        ),
                    ));
                }
            }
            for candidate in candidates.iter() {
                if candidate.lineage[..candidate.lineage.len() - 1] != envelope.dependencies {
                    return Err(ProtocolError::invalid(
                        "generated receipt dependency list differs from candidate lineage",
                    ));
                }
            }
            envelope.program.validate()?;
            original_program_identity = Some(envelope.program.clone());
            if envelope.program.byte_len > MAX_MOTION_BYTES {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "original worker motion exceeds motion admission",
                ));
            }
            let program_path = attempt_root.join("program.json");
            let raw_program = read_compact(
                &program_path,
                &envelope.program,
                PackageObjectRole::DependencyEvidence,
            )?;
            let original_program: MotionProgram =
                serde_json::from_slice(&raw_program).map_err(|_| {
                    ProtocolError::invalid("original worker program is not checked motion")
                })?;
            let mut checked = BTreeSet::new();
            for candidate in candidates.iter() {
                if checked.insert(candidate.motion.sha256.clone())
                    && engine.motion_store.read(&candidate.motion)? != original_program
                {
                    return Err(ProtocolError::invalid(
                        "original worker program differs from canonical candidate motion",
                    ));
                }
            }
            add_object(
                &mut objects,
                object_ref(&envelope.program),
                PackageObjectRole::DependencyEvidence,
                ObjectLocation::File(program_path),
            )?;
            add_object(
                &mut objects,
                object_ref(identity),
                PackageObjectRole::WorkerReceipt,
                ObjectLocation::File(path),
            )?;
        }
        let object = inline_object(
            &mut objects,
            raw.into_bytes(),
            PackageObjectRole::WorkerManifest,
        )?;
        manifest.generated_origins.push(PackageGeneratedOrigin {
            job_id,
            attempt_id,
            base_revision: request.base_revision,
            source_version: request.source.source_version,
            manifest: object,
            receipt,
            program: original_program_identity,
            state,
        });
    }
    // Flat lineage may name executable dependencies without packaging them. Every
    // other declared identity must resolve to an explicitly required payload.
    for candidate in &manifest.candidates {
        for identity in &candidate.lineage {
            identity.validate()?;
            if !objects
                .get(&identity.sha256)
                .is_some_and(|object| object.descriptor.byte_len == identity.byte_len)
                && !dependency_identities.contains(&(identity.sha256.clone(), identity.byte_len))
            {
                return Err(ProtocolError::new(
                    ErrorCode::DependencyMismatch,
                    format!(
                        "unresolved required candidate evidence {}",
                        identity.artifact_id
                    ),
                ));
            }
        }
    }
    let mut receipts = manifest
        .export_receipts
        .into_iter()
        .map(|record| Ok((serde_json::to_vec(&record).map_err(internal)?, record)))
        .collect::<PResult<Vec<_>>>()?;
    receipts.sort_by(|a, b| a.0.cmp(&b.0));
    manifest.export_receipts = receipts.into_iter().map(|(_, record)| record).collect();
    manifest.objects = objects
        .values()
        .map(|object| object.descriptor.clone())
        .collect();
    manifest.counts = manifest.record_counts()?;
    manifest.validate()?;
    let total = manifest
        .objects
        .iter()
        .try_fold(0u64, |total, object| total.checked_add(object.byte_len))
        .ok_or_else(|| ProtocolError::invalid("package byte accounting overflow"))?;
    if total > MAX_PACKAGE_BYTES {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "P1a package byte admission exceeded",
        ));
    }
    if let Some(holds) = retained_holds.as_mut() {
        let _gate = engine.artifact_gate.lock().map_err(internal)?;
        let identities = objects
            .values()
            .map(|object| ArtifactIdentity {
                artifact_id: ArtifactId::new(format!("package.hold.{}", object.descriptor.sha256))
                    .expect("digest identifier"),
                sha256: object.descriptor.sha256.clone(),
                byte_len: object.descriptor.byte_len,
            })
            .collect::<Vec<_>>();
        holds.extend(&identities)?;
    }
    if !retained {
        for object in objects.values() {
            let mut reader = open_object(object)?;
            let mut hash = Sha256::new();
            let mut length = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = match reader.read(&mut buffer) {
                    Ok(read) => read,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(missing(&object.descriptor, "evidence read failed")),
                };
                if read == 0 {
                    break;
                }
                length = length
                    .checked_add(read as u64)
                    .ok_or_else(|| missing(&object.descriptor, "length overflow"))?;
                if length > object.descriptor.byte_len {
                    return Err(missing(
                        &object.descriptor,
                        "evidence exceeds declared length",
                    ));
                }
                hash.update(&buffer[..read]);
            }
            if length != object.descriptor.byte_len
                || format!("{:x}", hash.finalize()) != object.descriptor.sha256
            {
                return Err(missing(
                    &object.descriptor,
                    "evidence digest or length mismatch",
                ));
            }
        }
    }
    {
        let db = engine.db.lock().map_err(internal)?;
        if retained {
            engine.authenticate(&db, session, Some(token))?;
            require_grant(&db, session, project, Scope::PackageProject)?;
        } else {
            authenticate_capture(engine, &db, session, token, project, expected_revision)?;
        }
    }
    let manifest_sha256 = format!("{:x}", Sha256::digest(manifest.canonical_bytes()?));
    Ok(ProjectCapturePlan {
        manifest,
        manifest_sha256,
        objects,
        _holds: retained_holds,
        _memory: memory,
    })
}

#[cfg(test)]
mod capture_review_tests {
    use super::*;

    struct Fixture {
        engine: Arc<Engine>,
        session: SessionId,
        token: String,
        project: ProjectId,
        _dir: tempfile::TempDir,
    }
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("nonexecuted-worker");
        fs::write(&executable, b"capture review fixture, never executed").unwrap();
        let engine = Engine::open(EngineConfig::new(dir.path().join("state"), executable)).unwrap();
        let (session, token) = match engine
            .handle(Request::new(
                id!(RequestId),
                Command::Pair {
                    client_name: "capture review".into(),
                    pairing_token: engine.token.clone(),
                },
            ))
            .result
            .unwrap()
        {
            ResponseBody::Paired {
                session,
                auth_token,
            } => (session, auth_token),
            _ => panic!("paired response required"),
        };
        let project = match engine
            .handle(
                Request::new(
                    id!(RequestId),
                    Command::CreateProject {
                        name: "capture review".into(),
                    },
                )
                .with_session(session.clone())
                .with_auth_token(token.clone()),
            )
            .result
            .unwrap()
        {
            ResponseBody::Project(snapshot) => snapshot.project_id,
            _ => panic!("project response required"),
        };
        Fixture {
            engine,
            session,
            token,
            project,
            _dir: dir,
        }
    }
    impl Fixture {
        fn capture(&self) -> PResult<ProjectCapturePlan> {
            capture(
                &self.engine,
                &self.session,
                &self.token,
                &self.project,
                RevisionId::new(0),
            )
        }
        fn import(&self, bytes: &[u8]) -> SourceArtifact {
            let path = self._dir.path().join("source");
            fs::write(&path, bytes).unwrap();
            let version = match self
                .engine
                .handle(
                    Request::new(id!(RequestId), Command::ImportSource { path })
                        .with_session(self.session.clone())
                        .with_auth_token(self.token.clone())
                        .in_project(self.project.clone(), Some(RevisionId::new(0))),
                )
                .result
                .unwrap()
            {
                ResponseBody::Source { source_version } => source_version,
                _ => panic!("source response required"),
            };
            self.engine
                .source(&self.engine.db.lock().unwrap(), &self.project, &version)
                .unwrap()
        }
        fn failed_origin(&self, source: SourceArtifact) -> WorkerRequest {
            let job_id = id!(JobId);
            let attempt_id = id!(AttemptId);
            let tool = ArtifactReference {
                identity: self.engine.binary_identity.clone(),
                path: self.engine.config.worker_executable.clone(),
            };
            let manifest = WorkerRequest {
                version: PROTOCOL_VERSION,
                job_id,
                attempt_id: attempt_id.clone(),
                project_id: self.project.clone(),
                base_revision: RevisionId::new(0),
                dependencies: vec![source.identity.clone()],
                source,
                output_dir: self
                    .engine
                    .config
                    .state_dir
                    .join("attempts")
                    .join(attempt_id.as_str()),
                operation: WorkerOperation::Generate {
                    preset: "fixture".into(),
                    settings: GenerationSettings::default(),
                    model: None,
                },
                budget: ResourceBudget {
                    memory_bytes: 1,
                    output_bytes: 1,
                    wall_time_ms: 1,
                    cpu_threads: 1,
                },
                tools: WorkerTools {
                    ffmpeg: tool.clone(),
                    ffprobe: tool,
                    onnx_runtime: None,
                },
            };
            manifest.validate().unwrap();
            let mut db = self.engine.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            tx.execute("INSERT INTO jobs(id,attempt,project,base_revision,session,source,state,manifest) VALUES(?1,?2,?3,0,?4,?5,'failed',?6)",
                params![manifest.job_id.as_str(),manifest.attempt_id.as_str(),self.project.as_str(),
                    self.session.as_str(),manifest.source.source_version.as_str(),encode(&manifest).unwrap()]).unwrap();
            tx.execute(
                "INSERT INTO attempts(id,job,manifest,state) VALUES(?1,?2,?3,'failed')",
                params![
                    manifest.attempt_id.as_str(),
                    manifest.job_id.as_str(),
                    encode(&manifest).unwrap()
                ],
            )
            .unwrap();
            tx.commit().unwrap();
            manifest
        }
    }

    #[test]
    fn unrelated_nested_project_recovery_does_not_consume_capture_admission() {
        let f = fixture();
        drop(f.capture().unwrap());
        let foreign = ProjectId::new(format!("{}:unrelated", f.project)).unwrap();
        {
            let mut db = f.engine.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            tx.execute("INSERT INTO projects SELECT ?1,'unrelated',revision,program,history_cursor,protected FROM projects WHERE id=?2",
                params![foreign.as_str(),f.project.as_str()]).unwrap();
            tx.execute("INSERT INTO revisions SELECT ?1,revision,actor,kind,label,program,protected FROM revisions WHERE project=?2",
                params![foreign.as_str(),f.project.as_str()]).unwrap();
            tx.execute("INSERT INTO edit_states SELECT ?1,position,program,protected FROM edit_states WHERE project=?2",
                params![foreign.as_str(),f.project.as_str()]).unwrap();
            lineage::created(&tx, &foreign).unwrap();
            {
                let mut insert = tx
                    .prepare("INSERT INTO legacy_motion_bytes VALUES('edit_states',?1,?2)")
                    .unwrap();
                for position in 0..MAX_CAPTURE_ROWS {
                    insert
                        .execute(params![
                            format!("{foreign}:{position}"),
                            "{\n \"tracks\": []\n}"
                        ])
                        .unwrap();
                }
            }
            tx.commit().unwrap();
        }
        let plan = f.capture().unwrap_or_else(|error| {
            panic!("unrelated project exhausted selected-project capture: {error}")
        });
        assert!(plan.manifest.legacy_cells.is_empty());
        assert_eq!(plan.manifest.revisions.len(), 1);
        assert_eq!(plan.manifest.origin_project_id, f.project);
    }

    #[test]
    fn primary_source_identity_must_match_the_captured_source_catalog() {
        let f = fixture();
        let source = f.import(b"actual immutable primary source");
        let mut manifest = f.failed_origin(source);
        drop(f.capture().unwrap());
        let digest = format!("{:x}", Sha256::digest(b"missing different primary source"));
        manifest.source.identity = ArtifactIdentity {
            artifact_id: ArtifactId::new(digest.clone()).unwrap(),
            sha256: digest.clone(),
            byte_len: 32,
        };
        // The typed worker manifest is structurally valid; the catalog binding
        // is the separate authority/closure invariant this regression tests.
        manifest.validate().unwrap();
        {
            let db = f.engine.db.lock().unwrap();
            db.execute(
                "UPDATE attempts SET manifest=?2 WHERE id=?1",
                params![manifest.attempt_id.as_str(), encode(&manifest).unwrap()],
            )
            .unwrap();
            db.execute(
                "UPDATE jobs SET manifest=?2 WHERE id=?1",
                params![manifest.job_id.as_str(), encode(&manifest).unwrap()],
            )
            .unwrap();
        }
        match f.capture() {
            Err(error) => assert_eq!(error.code, ErrorCode::DependencyMismatch),
            Ok(plan) => panic!("mismatched primary source was accepted as identity-only: missing object bundled={}",
                plan.manifest.objects.iter().any(|object|object.sha256==digest)),
        }
    }

    #[test]
    fn capture_indexes_preserve_large_graph_membership_without_reordering_records() {
        let f = fixture();
        let plan = f.capture().unwrap();
        let motion = plan.manifest.head.motion.clone();
        drop(plan);
        let revisions: Vec<_> = (0..10_000)
            .map(|number| PackageRevision {
                revision: RevisionId::new(number),
                actor: "archival-test".into(),
                kind: "fixture".into(),
                label: String::new(),
                motion: motion.clone(),
                protected: vec![],
            })
            .collect();
        let candidates: Vec<_> = (0..10_000)
            .map(|number| PackageCandidate {
                candidate_id: CandidateId::new(format!("candidate-{number:05}")).unwrap(),
                base_revision: RevisionId::new(number),
                motion: motion.clone(),
                job_origin: Some(JobId::new(format!("job-{:03}", number % 128)).unwrap()),
                lineage: vec![],
                committed_revision: None,
                review: vec![],
            })
            .collect();
        let indexes = CaptureIndexes::new(&candidates, &revisions, &[]);
        assert_eq!(indexes.candidates.len(), 10_000);
        assert_eq!(indexes.revisions.len(), 10_000);
        assert_eq!(
            indexes
                .candidates_by_job
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            10_000
        );
        for number in (0..10_000).rev() {
            let id = format!("candidate-{number:05}");
            let candidate = indexes.candidates[id.as_str()];
            assert_eq!(candidate.base_revision, RevisionId::new(number));
            assert_eq!(
                indexes.revisions[&candidate.base_revision].revision,
                candidate.base_revision
            );
        }
        for (number, candidate) in candidates.iter().enumerate() {
            assert_eq!(
                candidate.candidate_id.as_str(),
                format!("candidate-{number:05}")
            );
        }
        for group in indexes.candidates_by_job.values() {
            assert!(group
                .windows(2)
                .all(|pair| pair[0].candidate_id.as_str() < pair[1].candidate_id.as_str()));
        }
    }

    #[test]
    fn absent_executable_dependency_bytes_remain_identity_only() {
        let f = fixture();
        let source = f.import(b"actual primary source remains required");
        let mut manifest = f.failed_origin(source);
        let digest = format!("{:x}", Sha256::digest(b"unavailable historical executable"));
        let dependency = ArtifactIdentity {
            artifact_id: ArtifactId::new(digest.clone()).unwrap(),
            sha256: digest.clone(),
            byte_len: 33,
        };
        manifest.dependencies.push(dependency.clone());
        manifest.tools.ffmpeg = ArtifactReference {
            identity: dependency,
            path: f._dir.path().join("not-installed-ffmpeg"),
        };
        manifest.validate().unwrap();
        {
            let db = f.engine.db.lock().unwrap();
            db.execute(
                "UPDATE attempts SET manifest=?2 WHERE id=?1",
                params![manifest.attempt_id.as_str(), encode(&manifest).unwrap()],
            )
            .unwrap();
        }
        let plan = f.capture().unwrap();
        assert_eq!(plan.manifest.generated_origins.len(), 1);
        assert!(!plan
            .manifest
            .objects
            .iter()
            .any(|object| object.sha256 == digest));
    }
}
