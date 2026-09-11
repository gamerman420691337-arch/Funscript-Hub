//! Actual secured-engine qualification for large synthetic motion artifacts.
//! These cases do not decode a 30-minute video, benchmark neural accuracy,
//! qualify hardware, or actuate a physical device.
#![cfg(target_os = "linux")]

use anyhow::{bail, Context, Result};
use pulsar_clients::session::{EngineApi, SessionClient};
use pulsar_clients::{
    CandidateSnapshot as ClientCandidate, ProjectSnapshot as ClientProject,
    ResponseBody as ClientResponse,
};
use pulsar_protocol::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ACTIONS_PER_AXIS: usize = 54_000;
const EDIT_INDEX: usize = 1_001;
const OUTSIDE_INDEX: usize = 5_001;
const RPC_TIMEOUT: Duration = Duration::from_secs(120);
static NEXT: AtomicU64 = AtomicU64::new(1);

fn request_id(label: &str) -> RequestId {
    RequestId::new(format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))).unwrap()
}

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    child: Option<Child>,
    completed: bool,
    max_observed_control_bytes: Cell<usize>,
    max_observed_chunk_bytes: Cell<usize>,
}

impl Fixture {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("pulsar-large-{}-{nonce}", std::process::id()));
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let mut fixture = Self {
            state: root.join("state"), root, child: None, completed: false,
            max_observed_control_bytes: Cell::new(0),
            max_observed_chunk_bytes: Cell::new(0),
        };
        fixture.start()?;
        Ok(fixture)
    }

    fn endpoint(&self) -> PathBuf { self.state.join("engine.sock") }

    fn start(&mut self) -> Result<()> {
        let stderr = OpenOptions::new().create(true).append(true).open(self.root.join("engine.stderr"))?;
        self.child = Some(ProcessCommand::new(env!("CARGO_BIN_EXE_pulsar"))
            .arg("engine").env("PULSAR_STATE_DIR", &self.state)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(stderr).spawn()?);
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait()? {
                bail!("engine startup failed ({status}): {}", fs::read_to_string(self.root.join("engine.stderr"))?);
            }
            if std::os::unix::net::UnixStream::connect(self.endpoint()).is_ok() { return Ok(()); }
            if Instant::now() >= deadline { bail!("engine startup timeout: {}", self.root.display()); }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            if child.try_wait()?.is_none() { child.kill()?; }
            child.wait()?;
        }
        Ok(())
    }

    fn restart(&mut self) -> Result<()> { self.stop()?; self.start() }

    fn call(&self, request: &Request) -> Result<Response> {
        let request_bytes = serde_json::to_vec(request)?.len();
        assert!(request_bytes <= MAX_CONTROL_BYTES, "control request exceeded its contract");
        let mut connection = LocalClient::connect(self.endpoint(), RPC_TIMEOUT)?;
        let response = connection.call(request)?;
        assert_eq!(response.version, PROTOCOL_VERSION);
        assert_eq!(response.request_id, request.request_id);
        let response_bytes = serde_json::to_vec(&response)?.len();
        assert!(response_bytes <= MAX_CONTROL_BYTES, "descriptor response exceeded its contract");
        self.max_observed_control_bytes.set(self.max_observed_control_bytes.get().max(request_bytes).max(response_bytes));
        Ok(response)
    }

    fn pair(&self, name: &str) -> Result<Actor> {
        let reply = self.call(&Request::new(request_id("pair"), Command::Pair {
            client_name: name.into(),
            pairing_token: fs::read_to_string(self.state.join("pairing.token"))?.trim().to_owned(),
        }))?.result?;
        match reply {
            ResponseBody::Paired { session, auth_token } => Ok(Actor { session, auth_token }),
            _ => bail!("pairing did not return credentials"),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop();
        if std::thread::panicking() || !self.completed {
            eprintln!("Large-motion failure evidence preserved at {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct Actor { session: SessionId, auth_token: String }

impl Actor {
    fn client(&self, fixture: &Fixture) -> Result<SessionClient> {
        SessionClient::from_credentials(&fixture.endpoint(), self.session.clone(), self.auth_token.clone())
    }

    fn request(&self, command: Command, project: Option<ProjectId>, revision: Option<RevisionId>) -> Request {
        let mut request = Request::new(request_id("large"), command)
            .with_session(self.session.clone()).with_auth_token(self.auth_token.clone());
        request.project = project;
        request.expected_revision = revision;
        request
    }

    fn execute(&self, fixture: &Fixture, command: Command, project: Option<ProjectId>, revision: Option<RevisionId>) -> Result<ResponseBody> {
        Ok(fixture.call(&self.request(command, project, revision))?.result?)
    }
}

fn project(reply: ClientResponse) -> Result<ClientProject> {
    match reply { ClientResponse::Project(project) => Ok(project), _ => bail!("expected hydrated project") }
}

fn candidate(reply: ClientResponse) -> Result<ClientCandidate> {
    match reply { ClientResponse::Candidate(candidate) => Ok(candidate), _ => bail!("expected hydrated candidate") }
}

fn assert_code<T>(result: Result<T>, code: ErrorCode) {
    let error = match result { Ok(_) => panic!("operation must be rejected"), Err(error) => error };
    assert_eq!(error.downcast_ref::<ProtocolError>().expect("expected typed protocol error").code, code);
}

fn wire_project(reply: ResponseBody) -> Result<ProjectSnapshot> {
    match reply { ResponseBody::Project(project) => Ok(project), _ => bail!("expected wire project metadata") }
}

fn exact_time(index: usize) -> i64 { (index as i64 * 1_000_000_000) / 30 }

fn base_percent(axis_index: usize, index: usize) -> i32 {
    let phase = (index + axis_index * 7) % 64;
    match phase {
        0..=19 => 35 + phase as i32,
        20..=29 => 55,
        30..=49 => 55 - (phase - 30) as i32,
        _ => 35,
    }
}

fn expected_percent(axis_index: usize, index: usize, changes: &[(usize, usize, i32)]) -> i32 {
    changes.iter().rev().find(|(a, i, _)| *a == axis_index && *i == index)
        .map(|(_, _, value)| *value).unwrap_or_else(|| base_percent(axis_index, index))
}

fn fixture_program(axes: usize, changes: &[(usize, usize, i32)]) -> MotionProgram {
    MotionProgram::new(Axis::ALL.into_iter().take(axes).enumerate().map(|(axis_index, axis)| {
        let actions = (0..ACTIONS_PER_AXIS).map(|index| MotionAction::new(
            ProjectTime::from_nanos(exact_time(index)),
            NormalizedPosition::new(f64::from(expected_percent(axis_index, index, changes)) / 100.0).unwrap(),
            // A client must not elevate its input labels to observed evidence.
            EvidenceKind::Observed,
        ).unwrap()).collect();
        MotionTrack::new(axis, actions).unwrap()
    }).collect()).unwrap()
}

fn assert_program(program: &MotionProgram, axes: usize, changes: &[(usize, usize, i32)]) {
    assert_eq!(program.tracks().len(), axes);
    let mut total = 0;
    for (axis_index, axis) in Axis::ALL.into_iter().take(axes).enumerate() {
        let track = program.track(axis).expect("axis lost during artifact transport");
        assert_eq!(track.actions().len(), ACTIONS_PER_AXIS);
        assert!(track.gaps().is_empty());
        for (index, action) in track.actions().iter().enumerate() {
            assert_eq!(action.time().as_nanos(), exact_time(index), "timestamp {axis_index}/{index}");
            let expected = f64::from(expected_percent(axis_index, index, changes)) / 100.0;
            assert_eq!(action.position().value().to_bits(), expected.to_bits(), "position {axis_index}/{index}");
            assert_eq!(action.evidence(), EvidenceKind::Synthesized,
                "edit upload must not accept fabricated observed evidence");
        }
        total += track.actions().len();
    }
    assert_eq!(total, ACTIONS_PER_AXIS * axes);
}

fn assert_export(path: &Path, axis_index: usize, changes: &[(usize, usize, i32)]) -> Result<()> {
    let data: Value = serde_json::from_slice(&fs::read(path)?)?;
    let actions = data["actions"].as_array().context("standard export lacks actions")?;
    assert_eq!(actions.len(), ACTIONS_PER_AXIS, "export lost or invented actions");
    for (index, action) in actions.iter().enumerate() {
        assert_eq!(action.as_object().context("invalid exported action")?.len(), 2);
        // Standard funscript has integer milliseconds: nearest-ms quantization
        // is deliberate; the full master above must retain every exact ns value.
        let expected_ms = (exact_time(index) + 500_000) / 1_000_000;
        assert_eq!(action["at"].as_i64(), Some(expected_ms));
        assert!((expected_ms * 1_000_000 - exact_time(index)).abs() <= 500_000);
        assert_eq!(action["pos"].as_i64(), Some(i64::from(expected_percent(axis_index, index, changes))));
    }
    let serialized = serde_json::to_string(&data)?;
    for private in ["source_version", "attempt_id", "provenance", "auth_token", "evidence", "engine_epoch", "lease_id"] {
        assert!(!serialized.contains(private), "standard export leaked internal field {private}");
    }
    Ok(())
}

fn grant(owner: &Actor, fixture: &Fixture, target: &Actor, project_id: &ProjectId, scopes: Vec<Scope>) -> Result<()> {
    owner.execute(fixture, Command::Grant {
        session: target.session.clone(), project_id: project_id.clone(), scopes,
    }, None, None)?;
    Ok(())
}

fn metadata(owner: &Actor, fixture: &Fixture, id: &ProjectId, revision: RevisionId, actions: usize) -> Result<MotionDescriptor> {
    let snapshot = wire_project(owner.execute(fixture, Command::GetSnapshot, Some(id.clone()), None)?)?;
    assert_eq!(snapshot.revision, revision);
    snapshot.motion.validate()?;
    assert_eq!(snapshot.motion.program.action_count(), actions as u64);
    assert!(snapshot.motion.program.byte_len > MAX_CONTROL_BYTES as u64);
    assert!(matches!(&snapshot.motion.binding, MotionBinding::ProjectRevision {
        project_id, revision: bound_revision,
    } if project_id == id && *bound_revision == revision));
    let wire = serde_json::to_value(&snapshot)?;
    assert!(wire.get("program").is_none(), "wire snapshot must not inline hydrated programs");
    Ok(snapshot.motion)
}

fn run_roundtrip(axes: usize) -> Result<()> {
    let mut fixture = Fixture::new()?;
    let owner = fixture.pair("large-motion owner")?;
    let editor = fixture.pair("large-motion scoped editor")?;
    let mut client = owner.client(&fixture)?;
    let initial = project(client.execute(Command::CreateProject {
        name: format!("Large synthetic fixture: {axes} axes"),
    }, None, None)?)?;
    let id = initial.project_id.clone();
    grant(&owner, &fixture, &editor, &id, vec![Scope::Read, Scope::Edit])?;
    let mut editor_client = editor.client(&fixture)?;
    let base = fixture_program(axes, &[]);
    let changed_values = vec![(0, EDIT_INDEX, base_percent(0, EDIT_INDEX) + 1)];
    let changed = fixture_program(axes, &changed_values);
    eprintln!("large-motion {axes} axes: two large staged candidates against one base");
    let base_candidate = client.upload_edit(id.clone(), initial.revision, &base, "Initial authored values")?;
    assert_program(&base_candidate.program, axes, &[]);
    let stale_candidate = client.upload_edit(id.clone(), initial.revision, &changed, "Concurrent large proposal")?;
    assert_eq!(base_candidate.base_revision, initial.revision);
    assert_eq!(stale_candidate.base_revision, initial.revision);
    let unchanged = project(client.execute(Command::GetSnapshot, Some(id.clone()), None)?)?;
    assert_eq!(unchanged.revision, initial.revision, "upload finalization must not commit");
    let first = project(client.execute(Command::CommitCandidate {
        candidate_id: base_candidate.candidate_id,
    }, Some(id.clone()), Some(initial.revision))?)?;
    assert_program(&first.program, axes, &[]);
    assert_code(client.execute(Command::CommitCandidate {
        candidate_id: stale_candidate.candidate_id.clone(),
    }, Some(id.clone()), Some(initial.revision)), ErrorCode::RevisionConflict);
    let rebased = candidate(client.execute(Command::RebaseCandidate {
        candidate_id: stale_candidate.candidate_id.clone(),
    }, Some(id.clone()), Some(first.revision))?)?;
    assert_ne!(rebased.candidate_id, stale_candidate.candidate_id);
    assert_eq!(rebased.base_revision, first.revision);
    let second = project(client.execute(Command::CommitCandidate {
        candidate_id: rebased.candidate_id,
    }, Some(id.clone()), Some(first.revision))?)?;
    assert_program(&second.program, axes, &changed_values);
    let undone = project(client.execute(Command::Undo, Some(id.clone()), Some(second.revision))?)?;
    assert_program(&undone.program, axes, &[]);
    let redone = project(client.execute(Command::Redo, Some(id.clone()), Some(undone.revision))?)?;
    assert_program(&redone.program, axes, &changed_values);
    assert!(first.revision < second.revision && second.revision < undone.revision && undone.revision < redone.revision);
    metadata(&owner, &fixture, &id, redone.revision, axes * ACTIONS_PER_AXIS)?;

    eprintln!("large-motion {axes} axes: protection and interpolation-aware scoped edits");
    let protected = project(client.execute(Command::SetProtectedRegions {
        regions: vec![ProtectedRegion {
            axis: Some(Axis::Stroke),
            range: TimeRange::new(
                ProjectTime::from_nanos(exact_time(EDIT_INDEX) - 1_000_000),
                ProjectTime::from_nanos(exact_time(EDIT_INDEX) + 1_000_000),
            )?,
        }],
    }, Some(id.clone()), Some(redone.revision))?)?;
    let protected_changes = vec![(0, EDIT_INDEX, base_percent(0, EDIT_INDEX) + 2)];
    let protected_draft = fixture_program(axes, &protected_changes);
    let forbidden_candidate = editor_client.upload_edit(
        id.clone(), protected.revision, &protected_draft, "Must not bypass protected samples",
    ).context("finalizing the protected proposal")?;
    assert_code(editor_client.execute(Command::CommitCandidate {
        candidate_id: forbidden_candidate.candidate_id,
    }, Some(id.clone()), Some(protected.revision)), ErrorCode::Forbidden);
    let after_rejection = project(client.execute(Command::GetSnapshot, Some(id.clone()), None)?)?;
    assert_eq!(after_rejection.revision, protected.revision);
    assert_program(&after_rejection.program, axes, &changed_values);

    // Undo and redo the protection edit itself, rather than confusing it with
    // motion history. Redo must restore the protected interval at a fresh revision.
    let unprotected_history = project(client.execute(Command::Undo, Some(id.clone()), Some(protected.revision)).context("undoing the protection edit")?)?;
    assert_program(&unprotected_history.program, axes, &changed_values);
    let protected_history = project(client.execute(Command::Redo, Some(id.clone()), Some(unprotected_history.revision)).context("redoing the protection edit")?)?;
    assert_program(&protected_history.program, axes, &changed_values);
    let second_forbidden = editor_client.upload_edit(
        id.clone(), protected_history.revision, &protected_draft, "Protection must survive redo",
    ).context("finalizing protected proposal after redo")?;
    assert_code(editor_client.execute(Command::CommitCandidate {
        candidate_id: second_forbidden.candidate_id,
    }, Some(id.clone()), Some(protected_history.revision)), ErrorCode::Forbidden);

    let mut final_changes = changed_values.clone();
    final_changes.push((0, OUTSIDE_INDEX, base_percent(0, OUTSIDE_INDEX) + 1));
    let outside_draft = fixture_program(axes, &final_changes);
    let allowed = editor_client.upload_edit(
        id.clone(), protected_history.revision, &outside_draft, "Unprotected distant edit",
    ).context("finalizing the distant unprotected proposal")?;
    let final_project = project(editor_client.execute(Command::CommitCandidate {
        candidate_id: allowed.candidate_id,
    }, Some(id.clone()), Some(protected_history.revision)).context("committing the distant unprotected proposal")?)?;
    assert_program(&final_project.program, axes, &final_changes);
    metadata(&owner, &fixture, &id, final_project.revision, axes * ACTIONS_PER_AXIS)?;

    eprintln!("large-motion {axes} axes: crash/reopen and independent full export oracle");
    fixture.restart()?;
    let mut reopened_client = owner.client(&fixture)?;
    let reopened = project(reopened_client.execute(Command::OpenProject { project_id: id.clone() }, None, None)?)?;
    assert_eq!(reopened.revision, final_project.revision);
    assert_program(&reopened.program, axes, &final_changes);
    if axes > 1 {
        assert_code(reopened_client.execute(Command::Export {
            path: fixture.root.join("ambiguous.funscript"),
        }, Some(id.clone()), Some(reopened.revision)), ErrorCode::Unsupported);
        assert!(!fixture.root.join("ambiguous.funscript").exists());
    }
    for (axis_index, axis) in Axis::ALL.into_iter().take(axes).enumerate() {
        let output = fixture.root.join(format!("axis-{axis_index}.funscript"));
        let response = reopened_client.execute(Command::ExportAxis { path: output.clone(), axis },
            Some(id.clone()), Some(reopened.revision))?;
        assert!(matches!(response, ClientResponse::AxisExported {
            path, revision, axis: exported_axis,
        } if path == output && revision == reopened.revision && exported_axis == axis));
        assert_export(&output, axis_index, &final_changes)?;
    }
    assert!(fixture.max_observed_control_bytes.get() <= MAX_CONTROL_BYTES);
    eprintln!("large-motion {axes} axes PASS: {} actions, largest inspected control {} bytes",
        axes * ACTIONS_PER_AXIS, fixture.max_observed_control_bytes.get());
    fixture.completed = true;
    Ok(())
}

#[test]
fn large_stroke_artifact_roundtrip_rebase_history_restart_and_export() -> Result<()> { run_roundtrip(1) }

#[test]
fn large_six_axis_artifact_roundtrip_preserves_every_action() -> Result<()> { run_roundtrip(6) }

fn digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

fn values_bytes(program: &MotionProgram) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&pulsar_core::EditValuesProgram::from_program_values(program)?)?)
}

fn transfer(reply: ResponseBody) -> Result<TransferLease> {
    match reply { ResponseBody::Transfer(lease) => { lease.validate()?; Ok(lease) }, _ => bail!("expected transfer lease") }
}

fn begin_upload(actor: &Actor, fixture: &Fixture, id: &ProjectId, revision: RevisionId, bytes: &[u8], hash: &str) -> Result<TransferLease> {
    transfer(actor.execute(fixture, Command::BeginEditUpload {
        byte_len: bytes.len() as u64, sha256: hash.into(), label: "Raw independently checked transfer".into(),
    }, Some(id.clone()), Some(revision))?)
}

fn wire_rejected(response: Response, allowed: &[ErrorCode]) {
    match response.result {
        Ok(_) => panic!("untrusted operation must be rejected before publication"),
        Err(error) => assert!(allowed.contains(&error.code), "unexpected typed rejection: {error:?}"),
    }
}

/// Deliberately bypasses BulkClient's local validators so negative cases reach
/// the actual secured server. Uses the production bounded framing codecs.
struct RawBulk { stream: std::os::unix::net::UnixStream }

impl RawBulk {
    fn connect(actor: &Actor, lease: &TransferLease) -> Result<Self> {
        let mut stream = std::os::unix::net::UnixStream::connect(&lease.bulk_endpoint)?;
        stream.set_read_timeout(Some(RPC_TIMEOUT))?;
        stream.set_write_timeout(Some(RPC_TIMEOUT))?;
        write_bulk_header(&mut stream, &BulkHandshake {
            version: PROTOCOL_VERSION, session: actor.session.clone(),
            auth_token: actor.auth_token.clone(), lease_id: lease.lease_id.clone(),
            engine_epoch: lease.engine_epoch.clone(),
        })?;
        match read_bulk_header::<_, BulkReply>(&mut stream)? {
            BulkReply::Ready { lease: actual } => {
                assert_eq!(actual.lease_id, lease.lease_id);
                assert_eq!(actual.project_id, lease.project_id);
                assert_eq!(actual.engine_epoch, lease.engine_epoch);
            }
            BulkReply::Error(error) => return Err(error.into()),
            _ => bail!("bulk authentication returned wrong reply kind"),
        }
        Ok(Self { stream })
    }

    fn upload(&mut self, fixture: &Fixture, offset: u64, bytes: &[u8]) -> Result<BulkReply> {
        assert!(!bytes.is_empty() && bytes.len() <= MAX_ARTIFACT_CHUNK_BYTES);
        fixture.max_observed_chunk_bytes.set(fixture.max_observed_chunk_bytes.get().max(bytes.len()));
        write_bulk_header(&mut self.stream, &BulkOperation::Upload { offset, byte_len: bytes.len() as u32 })?;
        write_artifact_chunk(&mut self.stream, bytes)?;
        Ok(read_bulk_header(&mut self.stream)?)
    }

    fn download(&mut self, fixture: &Fixture, offset: u64, len: usize) -> Result<Vec<u8>> {
        assert!(len > 0 && len <= MAX_ARTIFACT_CHUNK_BYTES);
        write_bulk_header(&mut self.stream, &BulkOperation::Download { offset, byte_len: len as u32 })?;
        match read_bulk_header::<_, BulkReply>(&mut self.stream)? {
            BulkReply::Download { offset: actual, byte_len } => {
                assert_eq!(actual, offset); assert_eq!(byte_len as usize, len);
            }
            BulkReply::Error(error) => return Err(error.into()),
            _ => bail!("bulk download returned wrong reply kind"),
        }
        let bytes = read_artifact_chunk(&mut self.stream)?;
        assert_eq!(bytes.len(), len);
        fixture.max_observed_chunk_bytes.set(fixture.max_observed_chunk_bytes.get().max(bytes.len()));
        Ok(bytes)
    }
}

fn uploaded(reply: BulkReply, prefix: u64) {
    assert!(matches!(reply, BulkReply::Uploaded { accepted_prefix } if accepted_prefix == prefix),
        "upload acknowledgment must bind exact accepted prefix");
}

fn bulk_rejected(reply: BulkReply, allowed: &[ErrorCode]) {
    match reply {
        BulkReply::Error(error) => assert!(allowed.contains(&error.code), "unexpected bulk rejection: {error:?}"),
        _ => panic!("invalid bulk operation reached a successful effect"),
    }
}

fn upload_rest(actor: &Actor, fixture: &Fixture, lease: &TransferLease, bytes: &[u8], start: usize) -> Result<()> {
    let mut bulk = RawBulk::connect(actor, lease)?;
    let mut offset = start;
    while offset < bytes.len() {
        let end = (offset + MAX_ARTIFACT_CHUNK_BYTES).min(bytes.len());
        uploaded(bulk.upload(fixture, offset as u64, &bytes[offset..end])?, end as u64);
        offset = end;
    }
    Ok(())
}

fn current_revision(actor: &Actor, fixture: &Fixture, id: &ProjectId) -> Result<RevisionId> {
    Ok(wire_project(actor.execute(fixture, Command::GetSnapshot, Some(id.clone()), None)?)?.revision)
}

fn abandon(actor: &Actor, fixture: &Fixture, id: &ProjectId, lease: &TransferLease) -> Result<()> {
    let response = fixture.call(&actor.request(Command::AbandonTransfer {
        lease_id: lease.lease_id.clone(),
    }, Some(id.clone()), None))?;
    match response.result {
        Ok(ResponseBody::TransferAbandoned { lease_id }) => assert_eq!(lease_id, lease.lease_id),
        Err(error) if error.code == ErrorCode::NotFound => {}
        _ => bail!("transfer abandonment did not release or report absent lease"),
    }
    Ok(())
}

#[test]
fn large_transfer_authority_integrity_and_restart_fences() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let owner = fixture.pair("large broker owner")?;
    let reader = fixture.pair("large broker reader")?;
    let editor = fixture.pair("large broker editor")?;
    let mut owner_client = owner.client(&fixture)?;
    let initial = project(owner_client.execute(Command::CreateProject { name: "Broker fixture".into() }, None, None)?)?;
    let other = project(owner_client.execute(Command::CreateProject { name: "Other broker project".into() }, None, None)?)?;
    let id = initial.project_id.clone();
    grant(&owner, &fixture, &reader, &id, vec![Scope::Read])?;
    grant(&owner, &fixture, &editor, &id, vec![Scope::Read, Scope::Edit])?;
    let bytes = values_bytes(&fixture_program(1, &[]))?;
    assert!(bytes.len() > MAX_CONTROL_BYTES && bytes.len() < MAX_MOTION_BYTES as usize);
    let hash = digest(&bytes);

    eprintln!("large transfer: grants, project binding, raw offset/overlap and replay checks");
    wire_rejected(fixture.call(&reader.request(Command::BeginEditUpload {
        byte_len: bytes.len() as u64, sha256: hash.clone(), label: "read is not edit".into(),
    }, Some(id.clone()), Some(initial.revision)))?, &[ErrorCode::Forbidden]);
    let lease = begin_upload(&owner, &fixture, &id, initial.revision, &bytes, &hash)?;
    assert_code(RawBulk::connect(&reader, &lease), ErrorCode::Forbidden);
    wire_rejected(fixture.call(&owner.request(Command::TransferStatus {
        lease_id: lease.lease_id.clone(),
    }, Some(other.project_id.clone()), None))?, &[ErrorCode::Forbidden, ErrorCode::NotFound]);
    let mut raw = RawBulk::connect(&owner, &lease)?;
    bulk_rejected(raw.upload(&fixture, 1, &bytes[..16])?, &[ErrorCode::InvalidRequest, ErrorCode::RequestConflict]);
    let progress = transfer(owner.execute(&fixture, Command::TransferStatus {
        lease_id: lease.lease_id.clone(),
    }, Some(id.clone()), None)?)?;
    assert_eq!(progress.accepted_prefix, 0);

    let first = MAX_ARTIFACT_CHUNK_BYTES;
    let mut raw = RawBulk::connect(&owner, &lease)?;
    uploaded(raw.upload(&fixture, 0, &bytes[..first])?, first as u64);
    uploaded(raw.upload(&fixture, 0, &bytes[..first])?, first as u64);
    // A retry cannot overlap the unacknowledged suffix.
    bulk_rejected(raw.upload(&fixture, (first - 8) as u64, &bytes[first-8..first+8])?,
        &[ErrorCode::InvalidRequest, ErrorCode::RequestConflict]);
    let mut raw = RawBulk::connect(&owner, &lease)?;
    let mut conflict = bytes[..first].to_vec(); conflict[0] ^= 1;
    bulk_rejected(raw.upload(&fixture, 0, &conflict)?,
        &[ErrorCode::DependencyMismatch, ErrorCode::RequestConflict, ErrorCode::InvalidRequest]);
    abandon(&owner, &fixture, &id, &lease)?;
    assert_eq!(current_revision(&owner, &fixture, &id)?, initial.revision);

    eprintln!("large transfer: truncation then disconnect/reconnect with exact prefix");
    let lease = begin_upload(&owner, &fixture, &id, initial.revision, &bytes, &hash)?;
    let mut raw = RawBulk::connect(&owner, &lease)?;
    uploaded(raw.upload(&fixture, 0, &bytes[..first])?, first as u64);
    drop(raw);
    wire_rejected(fixture.call(&owner.request(Command::FinishEditUpload {
        lease_id: lease.lease_id.clone(),
    }, Some(id.clone()), None))?,
        &[ErrorCode::InvalidRequest, ErrorCode::DependencyMismatch]);
    assert_eq!(current_revision(&owner, &fixture, &id)?, initial.revision);
    let progress = transfer(owner.execute(&fixture, Command::TransferStatus {
        lease_id: lease.lease_id.clone(),
    }, Some(id.clone()), None)?)?;
    assert_eq!(progress.accepted_prefix, first as u64);
    upload_rest(&owner, &fixture, &progress, &bytes, first)?;
    let finalize_request = owner.request(Command::FinishEditUpload {
        lease_id: lease.lease_id.clone(),
    }, Some(id.clone()), None);
    let finished = match fixture.call(&finalize_request)?.result? {
        ResponseBody::Candidate(candidate) => candidate, _ => bail!("finalize did not publish candidate metadata"),
    };
    assert_eq!(finished.base_revision, initial.revision);
    assert_eq!(finished.motion.program.action_count(), ACTIONS_PER_AXIS as u64);
    assert_ne!(finished.motion.program.sha256, hash,
        "input values digest must not be mistaken for engine-authored program digest");
    assert_eq!(current_revision(&owner, &fixture, &id)?, initial.revision);
    let repeated = match fixture.call(&finalize_request)?.result? {
        ResponseBody::Candidate(candidate) => candidate, _ => bail!("duplicate finalize lost candidate outcome"),
    };
    assert_eq!(repeated.candidate_id, finished.candidate_id);

    eprintln!("large transfer: actual read-only download and cross-project denial");
    wire_rejected(fixture.call(&owner.request(Command::BeginMotionDownload {
        locator: MotionLocator::Candidate { candidate_id: finished.candidate_id.clone() },
        offset: 0, byte_len: finished.motion.program.byte_len,
    }, Some(other.project_id.clone()), None))?, &[ErrorCode::Forbidden, ErrorCode::NotFound]);
    let download = transfer(reader.execute(&fixture, Command::BeginMotionDownload {
        locator: MotionLocator::Candidate { candidate_id: finished.candidate_id.clone() },
        offset: 0, byte_len: finished.motion.program.byte_len,
    }, Some(id.clone()), None)?)?;
    let mut connection = RawBulk::connect(&reader, &download)?;
    let mut content = Vec::with_capacity(download.byte_len as usize);
    while (content.len() as u64) < download.byte_len {
        let length = MAX_ARTIFACT_CHUNK_BYTES.min(download.byte_len as usize - content.len());
        content.extend(connection.download(&fixture, content.len() as u64, length)?);
    }
    assert_eq!(digest(&content), finished.motion.program.sha256);
    assert_program(&serde_json::from_slice::<MotionProgram>(&content)?, 1, &[]);
    abandon(&reader, &fixture, &id, &download)?;

    eprintln!("large transfer: wrong digest and forged authority fields publish no revision");
    let wrong_hash = "0".repeat(64);
    let bad = begin_upload(&owner, &fixture, &id, initial.revision, &bytes, &wrong_hash)?;
    upload_rest(&owner, &fixture, &bad, &bytes, 0)?;
    wire_rejected(fixture.call(&owner.request(Command::FinishEditUpload {
        lease_id: bad.lease_id.clone(),
    }, Some(id.clone()), None))?, &[ErrorCode::DependencyMismatch]);
    abandon(&owner, &fixture, &id, &bad)?;
    assert_eq!(current_revision(&owner, &fixture, &id)?, initial.revision);

    // Independent malicious JSON, not a permissive production DTO serializer.
    // Isolate every authority field: one rejected field must not mask another
    // field that was accidentally accepted.
    let tiny = serde_json::json!({
        "tracks": [{
            "axis": "stroke",
            "actions": [
                {"time":0,"position":0.4},
                {"time":1000000,"position":0.5}
            ],
            "gaps":[]
        }]
    });
    for field in ["evidence", "provenance", "qualification", "worker_authority"] {
        let mut forged_value = tiny.clone();
        if field == "evidence" {
            forged_value["tracks"][0]["actions"][0]["evidence"] = serde_json::json!("observed");
        } else {
            forged_value[field] = serde_json::json!("forged-authority");
        }
        let forged = serde_json::to_vec(&forged_value)?;
        let malicious = begin_upload(&owner, &fixture, &id, initial.revision, &forged, &digest(&forged))?;
        upload_rest(&owner, &fixture, &malicious, &forged, 0)?;
        wire_rejected(fixture.call(&owner.request(Command::FinishEditUpload {
            lease_id: malicious.lease_id.clone(),
        }, Some(id.clone()), None))?, &[ErrorCode::InvalidRequest]);
        abandon(&owner, &fixture, &id, &malicious)?;
        assert_eq!(current_revision(&owner, &fixture, &id)?, initial.revision);
    }

    eprintln!("large transfer: revocation fences an authenticated active connection");
    let revoked = begin_upload(&editor, &fixture, &id, initial.revision, &bytes, &hash)?;
    let mut editor_bulk = RawBulk::connect(&editor, &revoked)?;
    uploaded(editor_bulk.upload(&fixture, 0, &bytes[..first])?, first as u64);
    owner.execute(&fixture, Command::Revoke {
        session: editor.session.clone(), project_id: id.clone(),
    }, None, None)?;
    bulk_rejected(editor_bulk.upload(&fixture, first as u64, &bytes[first..first+16])?,
        &[ErrorCode::Forbidden, ErrorCode::NotFound, ErrorCode::Unauthorized]);
    wire_rejected(fixture.call(&editor.request(Command::FinishEditUpload {
        lease_id: revoked.lease_id.clone(),
    }, Some(id.clone()), None))?, &[ErrorCode::Forbidden, ErrorCode::NotFound]);
    assert_eq!(current_revision(&owner, &fixture, &id)?, initial.revision);

    let commit_request = owner.request(Command::CommitCandidate {
        candidate_id: finished.candidate_id.clone(),
    }, Some(id.clone()), Some(initial.revision));
    let committed_metadata = wire_project(fixture.call(&commit_request)?.result?)?;
    let committed = project(owner_client.execute(Command::GetSnapshot, Some(id.clone()), None)?)?;
    assert_eq!(committed.revision, committed_metadata.revision);
    assert_program(&committed.program, 1, &[]);
    let unfinished = begin_upload(&owner, &fixture, &id, committed.revision, &bytes, &hash)?;
    let mut partial = RawBulk::connect(&owner, &unfinished)?;
    uploaded(partial.upload(&fixture, 0, &bytes[..first])?, first as u64);
    drop(partial);
    fixture.restart()?;
    wire_rejected(fixture.call(&owner.request(Command::TransferStatus {
        lease_id: unfinished.lease_id.clone(),
    }, Some(id.clone()), None))?,
        &[ErrorCode::NotFound, ErrorCode::DependencyMismatch, ErrorCode::InvalidRequest]);
    let fresh = begin_upload(&owner, &fixture, &id, committed.revision, &bytes, &hash)?;
    assert_ne!(fresh.engine_epoch, unfinished.engine_epoch);
    // Endpoint rotation is permitted. Present stale identity/epoch to the new
    // authenticated service rather than assuming its socket path is unchanged.
    let mut stale_at_current_endpoint = unfinished.clone();
    stale_at_current_endpoint.bulk_endpoint = fresh.bulk_endpoint.clone();
    let failed = match RawBulk::connect(&owner, &stale_at_current_endpoint) {
        Ok(_) => panic!("restart must invalidate unfinished transfer epochs"),
        Err(error) => error,
    };
    let stale_code = failed.downcast_ref::<ProtocolError>().context("restart rejection must be typed")?.code;
    assert!([ErrorCode::NotFound, ErrorCode::DependencyMismatch, ErrorCode::InvalidRequest, ErrorCode::Forbidden].contains(&stale_code));
    let replay = match fixture.call(&finalize_request)?.result? {
        ResponseBody::Candidate(candidate) => candidate, _ => bail!("durable finalization replay missing after restart"),
    };
    assert_eq!(replay.candidate_id, finished.candidate_id);
    assert_eq!(current_revision(&owner, &fixture, &id)?, committed.revision);
    let replayed_commit = wire_project(fixture.call(&commit_request)?.result?)?;
    assert_eq!(replayed_commit.revision, committed.revision, "replayed commit must not allocate a new revision");
    abandon(&owner, &fixture, &id, &fresh)?;
    assert_eq!(fixture.max_observed_chunk_bytes.get(), MAX_ARTIFACT_CHUNK_BYTES);
    assert!(fixture.max_observed_control_bytes.get() <= MAX_CONTROL_BYTES);
    eprintln!("large transfer fences PASS: max control {}, max bulk chunk {} bytes",
        fixture.max_observed_control_bytes.get(), fixture.max_observed_chunk_bytes.get());
    fixture.completed = true;
    Ok(())
}


fn small_edit_bytes() -> Vec<u8> {
    // Independent literal values-only fixture, not the production DTO serializer.
    br#"{"tracks":[{"axis":"stroke","actions":[{"time":0,"position":0.4},{"time":1000000,"position":0.5}],"gaps":[]}]}"#.to_vec()
}

fn small_candidate(actor: &Actor, fixture: &Fixture, id: &ProjectId, revision: RevisionId) -> Result<CandidateSnapshot> {
    let bytes = small_edit_bytes();
    let lease = begin_upload(actor, fixture, id, revision, &bytes, &digest(&bytes))?;
    upload_rest(actor, fixture, &lease, &bytes, 0)?;
    match actor.execute(fixture, Command::FinishEditUpload { lease_id: lease.lease_id },
        Some(id.clone()), None)? {
        ResponseBody::Candidate(candidate) => Ok(candidate),
        _ => bail!("small collision fixture did not finalize a candidate"),
    }
}

fn collision_begin(actor: &Actor, id: &ProjectId, revision: RevisionId) -> Request {
    let bytes = small_edit_bytes();
    actor.request(Command::BeginEditUpload {
        byte_len: bytes.len() as u64, sha256: digest(&bytes), label: "Request identity collision probe".into(),
    }, Some(id.clone()), Some(revision))
}

fn require_request_conflict(response: Response, description: &str) {
    match response.result {
        Err(error) => assert_eq!(error.code, ErrorCode::RequestConflict, "{description}"),
        Ok(_) => panic!("{description}: changed payload reused a request identity successfully"),
    }
}

#[test]
fn large_transfer_create_request_id_cannot_be_reused_for_begin() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let owner = fixture.pair("create to begin collision")?;
    let create = owner.request(Command::CreateProject { name: "Create collision".into() }, None, None);
    let initial = wire_project(fixture.call(&create)?.result?)?;
    let mut collision = collision_begin(&owner, &initial.project_id, initial.revision);
    collision.request_id = create.request_id.clone();
    let response = fixture.call(&collision)?;
    assert_eq!(current_revision(&owner, &fixture, &initial.project_id)?, initial.revision);
    require_request_conflict(response, "CreateProject ID -> BeginEditUpload");
    assert_eq!(wire_project(fixture.call(&create)?.result?)?.project_id, initial.project_id);
    fixture.completed = true;
    Ok(())
}

#[test]
fn large_transfer_commit_request_id_cannot_be_reused_for_begin() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let owner = fixture.pair("commit to begin collision")?;
    let initial = wire_project(owner.execute(&fixture, Command::CreateProject {
        name: "Commit collision".into(),
    }, None, None)?)?;
    let candidate = small_candidate(&owner, &fixture, &initial.project_id, initial.revision)?;
    let commit = owner.request(Command::CommitCandidate { candidate_id: candidate.candidate_id },
        Some(initial.project_id.clone()), Some(initial.revision));
    let committed = wire_project(fixture.call(&commit)?.result?)?;
    let mut collision = collision_begin(&owner, &initial.project_id, committed.revision);
    collision.request_id = commit.request_id.clone();
    let response = fixture.call(&collision)?;
    assert_eq!(current_revision(&owner, &fixture, &initial.project_id)?, committed.revision);
    require_request_conflict(response, "CommitCandidate ID -> BeginEditUpload");
    assert_eq!(wire_project(fixture.call(&commit)?.result?)?.revision, committed.revision);
    fixture.completed = true;
    Ok(())
}

#[test]
fn large_transfer_begin_request_id_cannot_be_reused_for_commit() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let owner = fixture.pair("begin to commit collision")?;
    let initial = wire_project(owner.execute(&fixture, Command::CreateProject {
        name: "Begin commit collision".into(),
    }, None, None)?)?;
    let candidate = small_candidate(&owner, &fixture, &initial.project_id, initial.revision)?;
    let begin = collision_begin(&owner, &initial.project_id, initial.revision);
    let lease = transfer(fixture.call(&begin)?.result?)?;
    let mut collision = owner.request(Command::CommitCandidate { candidate_id: candidate.candidate_id },
        Some(initial.project_id.clone()), Some(initial.revision));
    collision.request_id = begin.request_id.clone();
    let response = fixture.call(&collision)?;
    assert_eq!(current_revision(&owner, &fixture, &initial.project_id)?, initial.revision,
        "BeginEditUpload ID -> CommitCandidate must not advance the revision");
    require_request_conflict(response, "BeginEditUpload ID -> CommitCandidate");
    let retained = transfer(fixture.call(&begin)?.result?)?;
    assert_eq!(retained.lease_id, lease.lease_id);
    assert_eq!(retained.accepted_prefix, 0);
    abandon(&owner, &fixture, &initial.project_id, &lease)?;
    fixture.completed = true;
    Ok(())
}

#[test]
fn large_transfer_begin_request_id_cannot_be_reused_for_finish() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let owner = fixture.pair("begin to finish collision")?;
    let initial = wire_project(owner.execute(&fixture, Command::CreateProject {
        name: "Begin finish collision".into(),
    }, None, None)?)?;
    let begin = collision_begin(&owner, &initial.project_id, initial.revision);
    let lease = transfer(fixture.call(&begin)?.result?)?;
    let bytes = small_edit_bytes();
    upload_rest(&owner, &fixture, &lease, &bytes, 0)?;
    let mut collision = owner.request(Command::FinishEditUpload { lease_id: lease.lease_id.clone() },
        Some(initial.project_id.clone()), None);
    collision.request_id = begin.request_id.clone();
    let response = fixture.call(&collision)?;
    assert_eq!(current_revision(&owner, &fixture, &initial.project_id)?, initial.revision);
    require_request_conflict(response, "BeginEditUpload ID -> FinishEditUpload");
    let retained = transfer(owner.execute(&fixture, Command::TransferStatus {
        lease_id: lease.lease_id.clone(),
    }, Some(initial.project_id.clone()), None)?)?;
    assert_eq!(retained.accepted_prefix, bytes.len() as u64,
        "conflicting finalize must leave the original upload resumable");
    match owner.execute(&fixture, Command::FinishEditUpload { lease_id: lease.lease_id },
        Some(initial.project_id.clone()), None)? {
        ResponseBody::Candidate(candidate) => assert_eq!(candidate.base_revision, initial.revision),
        _ => bail!("a fresh finalization identity must still publish the original upload"),
    }
    assert_eq!(current_revision(&owner, &fixture, &initial.project_id)?, initial.revision);
    fixture.completed = true;
    Ok(())
}
