//! P1b actual secured-process export QA. Synthetic inputs only.
//! No portable import, GUI parity, media decoding, neural throughput, or device
//! qualification is claimed. The >1 GiB case is separately invoked.

#![cfg(target_os = "linux")]

use anyhow::{bail, ensure, Context, Result};
use pulsar_clients::project_package::{PackageDownloadControl, PackagePrivacyConsent};
use pulsar_clients::{EngineApi, ResponseBody as ClientResponse, SessionClient};
use pulsar_protocol::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(120);
const BLOCK: usize = 64 * 1024;
const RACE_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const LARGE_SOURCE_BYTES: u64 = 1024 * 1024 * 1024 + 256 * 1024;
const RSS_DELTA_TRIPWIRE: u64 = 256 * 1024 * 1024;
const MEDIA: &[u8] = b"P6\n2 2\n255\n\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\xff\x00";
static NEXT: AtomicU64 = AtomicU64::new(1);

fn request_id() -> RequestId {
    RequestId::new(format!(
        "package-qa-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
    .unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn consent() -> PackagePrivacyConsent {
    PackagePrivacyConsent::acknowledge_unencrypted_private_data()
}
fn digest_file(path: &Path) -> Result<String> {
    let mut reader = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; BLOCK];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn error_code(error: &anyhow::Error) -> Option<ErrorCode> {
    if let Some(error) = error.downcast_ref::<ProtocolError>() {
        Some(error.code)
    } else if let Some(TransportError::Protocol(error)) = error.downcast_ref::<TransportError>() {
        Some(error.code)
    } else {
        None
    }
}
fn rejected<T>(result: Result<T>, allowed: &[ErrorCode]) -> Result<()> {
    let error = match result {
        Ok(_) => bail!("operation unexpectedly succeeded"),
        Err(error) => error,
    };
    ensure!(
        error_code(&error).is_some_and(|code| allowed.contains(&code)),
        "wrong typed rejection: {error}"
    );
    Ok(())
}

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    child: Option<Child>,
    completed: bool,
    always_cleanup: bool,
    max_control: AtomicU64,
}
impl Fixture {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pulsar-package-export-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let mut fixture = Self {
            state: root.join("state"),
            root,
            child: None,
            completed: false,
            always_cleanup: false,
            max_control: AtomicU64::new(0),
        };
        fixture.start()?;
        Ok(fixture)
    }
    fn endpoint(&self) -> PathBuf {
        self.state.join("engine.sock")
    }
    fn start(&mut self) -> Result<()> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("engine.stderr"))?;
        self.child = Some(
            ProcessCommand::new(env!("CARGO_BIN_EXE_pulsar"))
                .arg("engine")
                .env("PULSAR_STATE_DIR", &self.state)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()?,
        );
        let until = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait()? {
                bail!(
                    "engine failed {status}: {}",
                    fs::read_to_string(self.root.join("engine.stderr"))?
                );
            }
            if UnixStream::connect(self.endpoint()).is_ok() {
                return Ok(());
            }
            ensure!(Instant::now() < until, "engine startup timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
        }
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        self.stop()?;
        self.start()
    }
    fn owner(&self) -> Result<SessionClient> {
        SessionClient::connect(&self.endpoint(), &self.state.join("pairing.token"))
    }
    fn approve(&self, project: &ProjectId) -> Result<()> {
        // The actual operator-only CLI reads the distinct OS-owner capability.
        // No capability file is read or a grant row injected by this fixture.
        let output = ProcessCommand::new(env!("CARGO_BIN_EXE_pulsar"))
            .args([
                "allow-packaging",
                project.as_str(),
                "--acknowledge-unencrypted-private-data",
            ])
            .env("PULSAR_STATE_DIR", &self.state)
            .stdin(Stdio::null())
            .output()?;
        ensure!(
            output.status.success(),
            "actual host-local approval failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
    fn call(&self, request: &Request) -> Result<ResponseBody> {
        let n = serde_json::to_vec(request)?.len();
        ensure!(n <= MAX_CONTROL_BYTES, "oversized test control request");
        let response = LocalClient::connect(self.endpoint(), TIMEOUT)?.call(request)?;
        ensure!(
            response.request_id == request.request_id && response.version == PROTOCOL_VERSION,
            "control response binding changed"
        );
        let length = serde_json::to_vec(&response)?.len();
        ensure!(length <= MAX_CONTROL_BYTES, "oversized control reply");
        self.max_control
            .fetch_max(n.max(length) as u64, Ordering::Relaxed);
        Ok(response.result?)
    }
    fn pair(&self, label: &str) -> Result<Actor> {
        let response = self.call(&Request::new(
            request_id(),
            Command::Pair {
                client_name: label.into(),
                pairing_token: fs::read_to_string(self.state.join("pairing.token"))?
                    .trim()
                    .into(),
            },
        ))?;
        match response {
            ResponseBody::Paired {
                session,
                auth_token,
            } => Ok(Actor {
                session,
                auth_token,
            }),
            _ => bail!("pairing failed"),
        }
    }
    fn lost_ack(&self, request: Request) -> Result<Response> {
        // A real forwarding proxy consumes the engine acknowledgement but
        // closes the downstream connection without delivering it to the client.
        let path = self.root.join(format!(
            "drop-ack-{}.sock",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&path)?;
        let endpoint = self.endpoint();
        let thread = std::thread::spawn(move || -> Result<Response> {
            let (mut downstream, _) = listener.accept()?;
            downstream.set_read_timeout(Some(TIMEOUT))?;
            downstream.set_write_timeout(Some(TIMEOUT))?;
            let actual: Request = read_message(&mut downstream)?;
            let mut upstream = UnixStream::connect(endpoint)?;
            upstream.set_read_timeout(Some(TIMEOUT))?;
            upstream.set_write_timeout(Some(TIMEOUT))?;
            write_message(&mut upstream, &actual)?;
            let response: Response = read_message(&mut upstream)?;
            drop(downstream);
            Ok(response)
        });
        let error = LocalClient::connect(&path, TIMEOUT)?.call(&request);
        ensure!(
            error.is_err(),
            "proxy delivered an acknowledgement it must discard"
        );
        let response = thread
            .join()
            .map_err(|_| anyhow::anyhow!("ack proxy panicked"))??;
        fs::remove_file(path)?;
        Ok(response)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop();
        if self.completed || self.always_cleanup {
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!("Package QA diagnostics retained at {}", self.root.display());
        }
    }
}
struct Actor {
    session: SessionId,
    auth_token: String,
}
impl Actor {
    fn request(
        &self,
        command: Command,
        project: &ProjectId,
        revision: Option<RevisionId>,
    ) -> Request {
        let mut request = Request::new(request_id(), command)
            .with_session(self.session.clone())
            .with_auth_token(self.auth_token.clone());
        request.project = Some(project.clone());
        request.expected_revision = revision;
        request
    }
    fn call(
        &self,
        f: &Fixture,
        command: Command,
        project: &ProjectId,
        revision: Option<RevisionId>,
    ) -> Result<ResponseBody> {
        f.call(&self.request(command, project, revision))
    }
    fn client(&self, f: &Fixture) -> Result<SessionClient> {
        SessionClient::from_credentials(
            &f.endpoint(),
            self.session.clone(),
            self.auth_token.clone(),
        )
    }
    fn handshake(&self, lease: &PackageDownloadLease) -> PackageBulkHandshake {
        PackageBulkHandshake {
            version: PROTOCOL_VERSION,
            channel: PackageBulkChannel::ProjectPackageDownload,
            session: self.session.clone(),
            auth_token: self.auth_token.clone(),
            lease_id: lease.lease_id.clone(),
            engine_epoch: lease.engine_epoch.clone(),
            operation_id: lease.operation_id.clone(),
            artifact_sha256: lease.artifact.sha256.clone(),
        }
    }
}

fn client_project(response: ClientResponse) -> Result<pulsar_clients::ProjectSnapshot> {
    match response {
        ClientResponse::Project(project) => Ok(project),
        _ => bail!("expected hydrated project"),
    }
}
fn package(response: ResponseBody) -> Result<PackageExportStatus> {
    match response {
        ResponseBody::PackageExport(status) => {
            status.validate()?;
            Ok(status)
        }
        _ => bail!("expected package status"),
    }
}
fn lease(response: ResponseBody) -> Result<PackageDownloadLease> {
    match response {
        ResponseBody::PackageDownload(lease) => {
            lease.validate()?;
            Ok(lease)
        }
        _ => bail!("expected package-only lease"),
    }
}
fn motion(changed: bool) -> MotionProgram {
    MotionProgram::new(
        [
            (Axis::Stroke, [35, 45, 40, if changed { 48 } else { 50 }]),
            (Axis::Sway, [20, 22, 21, 23]),
        ]
        .into_iter()
        .map(|(axis, values)| {
            MotionTrack::new(
                axis,
                values
                    .into_iter()
                    .enumerate()
                    .map(|(i, value)| {
                        MotionAction::new(
                            ProjectTime::from_nanos(i as i64 * 100_000_000),
                            NormalizedPosition::new(f64::from(value) / 100.0).unwrap(),
                            EvidenceKind::Observed,
                        )
                        .unwrap()
                    })
                    .collect(),
            )
            .unwrap()
        })
        .collect(),
    )
    .unwrap()
}
struct ProjectFixture {
    project: ProjectId,
    revision: RevisionId,
    source: SourceVersionId,
    source_path: PathBuf,
    source_hash: String,
    source_bytes: u64,
}
fn indexed_source(path: &Path, length: u64) -> Result<String> {
    ensure!(
        length % BLOCK as u64 == 0,
        "indexed sparse fixture needs whole blocks"
    );
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.set_len(length)?;
    let mut buffer = [0u8; BLOCK];
    let mut digest = Sha256::new();
    for index in 0..length / BLOCK as u64 {
        buffer[..8].copy_from_slice(&index.to_le_bytes());
        buffer[8..16].copy_from_slice(&(!index).to_le_bytes());
        buffer[16..24].copy_from_slice(&length.to_le_bytes());
        buffer[24..32].copy_from_slice(b"P1BLARGE");
        file.seek(SeekFrom::Start(index * BLOCK as u64))?;
        file.write_all(&buffer[..32])?;
        digest.update(buffer);
    }
    file.sync_all()?;
    Ok(format!("{:x}", digest.finalize()))
}
fn setup(f: &Fixture, owner: &mut SessionClient, length: Option<u64>) -> Result<ProjectFixture> {
    let created = client_project(owner.execute(
        Command::CreateProject {
            name: "P1b synthetic export".into(),
        },
        None,
        None,
    )?)?;
    let source_path = f.root.join("authored-source.bin");
    let (source_hash, source_bytes) = if let Some(length) = length {
        (indexed_source(&source_path, length)?, length)
    } else {
        fs::write(&source_path, MEDIA)?;
        (sha(MEDIA), MEDIA.len() as u64)
    };
    let source = match owner.execute(
        Command::ImportSource {
            path: source_path.clone(),
        },
        Some(created.project_id.clone()),
        None,
    )? {
        ClientResponse::Source { source_version } => source_version,
        _ => bail!("source import did not return identity"),
    };
    let candidate = owner.upload_edit(
        created.project_id.clone(),
        created.revision,
        &motion(false),
        "P1b authored two axes",
    )?;
    let committed = client_project(owner.execute(
        Command::CommitCandidate {
            candidate_id: candidate.candidate_id,
        },
        Some(created.project_id.clone()),
        Some(created.revision),
    )?)?;
    ensure!(
        committed.revision == RevisionId::new(1),
        "fixture commit did not create revision one"
    );
    Ok(ProjectFixture {
        project: created.project_id,
        revision: committed.revision,
        source,
        source_path,
        source_hash,
        source_bytes,
    })
}
fn delegate(
    owner: &mut SessionClient,
    actor: &Actor,
    p: &ProjectFixture,
    scopes: Vec<Scope>,
) -> Result<()> {
    owner.execute(
        Command::Grant {
            session: actor.session.clone(),
            project_id: p.project.clone(),
            scopes,
        },
        None,
        None,
    )?;
    Ok(())
}
fn terminal(
    client: &mut SessionClient,
    p: &ProjectFixture,
    operation: &PackageOperationId,
) -> Result<PackageExportStatus> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status = client.project_package_status(p.project.clone(), operation.clone())?;
        status.validate()?;
        if status.state.is_terminal() {
            return Ok(status);
        }
        ensure!(
            Instant::now() < deadline,
            "package did not reach a terminal state: {:?}",
            status.state
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn ready(
    client: &mut SessionClient,
    p: &ProjectFixture,
    operation: &PackageOperationId,
) -> Result<PackageExportStatus> {
    let status = terminal(client, p, operation)?;
    ensure!(
        status.state == PackageExportOperationState::Ready,
        "package failed: {:?} {:?}",
        status.state,
        status.error
    );
    Ok(status)
}
fn streaming(
    client: &mut SessionClient,
    p: &ProjectFixture,
    operation: &PackageOperationId,
) -> Result<PackageExportStatus> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status = client.project_package_status(p.project.clone(), operation.clone())?;
        status.validate()?;
        if status.state == PackageExportOperationState::Streaming
            && status.capture.is_some()
            && status.progress.completed_bytes > 0
            && status
                .progress
                .total_bytes
                .is_some_and(|total| total > status.progress.completed_bytes)
        {
            return Ok(status);
        }
        ensure!(
            !status.state.is_terminal(),
            "active streaming checkpoint not observed; terminal {:?} {:?}",
            status.state,
            status.error
        );
        ensure!(
            Instant::now() < deadline,
            "streaming checkpoint not observed by deadline"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn begin(
    actor: &Actor,
    f: &Fixture,
    p: &ProjectFixture,
    status: &PackageExportStatus,
) -> Result<PackageDownloadLease> {
    let artifact = status.artifact.as_ref().context("ready artifact absent")?;
    let request = actor.request(
        Command::BeginProjectPackageDownload {
            operation_id: status.operation_id.clone(),
            offset: 0,
            byte_len: artifact.byte_len,
        },
        &p.project,
        None,
    );
    let deadline = Instant::now() + TIMEOUT;
    let mut probed_pending = false;
    loop {
        match f.call(&request)? {
            ResponseBody::PackageDownload(download) => {
                download.validate()?;
                return Ok(download);
            }
            ResponseBody::PackageDownloadPending(pending) => {
                pending.validate()?;
                ensure!(
                    pending.operation_id == status.operation_id
                        && pending.artifact_sha256 == artifact.sha256
                        && pending.total_bytes == artifact.byte_len,
                    "pending verification lost exact Begin binding"
                );
                if !probed_pending {
                    let started = Instant::now();
                    let snapshot = actor.call(f, Command::GetSnapshot, &p.project, None)?;
                    ensure!(
                        matches!(snapshot,ResponseBody::Project(ref project) if project.project_id==p.project && project.revision==p.revision),
                        "control snapshot changed during epoch verification"
                    );
                    eprintln!(
                        "P1b CONTROL: phase=epoch_verification_pending get_snapshot_ms={:.3}",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    probed_pending = true;
                }
                ensure!(
                    Instant::now() < deadline,
                    "same Begin request did not finish current-epoch verification"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => bail!("Begin did not return package lease or explicit verification state"),
        }
    }
}
fn abandon(
    actor: &Actor,
    f: &Fixture,
    p: &ProjectFixture,
    download: &PackageDownloadLease,
) -> Result<()> {
    actor.call(
        f,
        Command::AbandonProjectPackageDownload {
            lease_id: download.lease_id.clone(),
        },
        &p.project,
        None,
    )?;
    Ok(())
}
fn connect_bulk(actor: &Actor, download: &PackageDownloadLease) -> Result<PackageBulkClient> {
    let mut client = PackageBulkClient::connect(&download.bulk_endpoint, TIMEOUT)?;
    client.handshake(&actor.handshake(download))?;
    Ok(client)
}

/// Independent format parser: literal header layout, independently hashed JSON,
/// sequential declared objects, and exact EOF. Does not call package decoder.
struct CheckedBundle {
    manifest: Value,
    small_objects: BTreeMap<String, Vec<u8>>,
    bytes: u64,
    sha256: String,
}
fn inspect_bundle(path: &Path, artifact: &PackageArtifactDescriptor) -> Result<CheckedBundle> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 52];
    file.read_exact(&mut header)?;
    ensure!(&header[..8] == b"PULSPKG\0", "wrong literal package magic");
    ensure!(
        u16::from_le_bytes(header[8..10].try_into()?) == 1
            && u16::from_le_bytes(header[10..12].try_into()?) == 0,
        "unknown format/flags"
    );
    let length = u64::from_le_bytes(header[12..20].try_into()?);
    ensure!(
        length > 0 && length <= 16 * 1024 * 1024,
        "manifest length exceeded independent allocation bound"
    );
    let mut json = vec![0; length as usize];
    file.read_exact(&mut json)?;
    ensure!(
        Sha256::digest(&json).as_slice() == &header[20..52],
        "manifest header digest mismatch"
    );
    ensure!(
        sha(&json) == artifact.manifest_sha256,
        "control receipt does not bind manifest bytes"
    );
    let manifest: Value = serde_json::from_slice(&json)?;
    let objects = manifest["objects"]
        .as_array()
        .context("object descriptor array absent")?;
    ensure!(
        objects.len() == artifact.object_count as usize && objects.len() <= 100_000,
        "object count changed"
    );
    let mut entire = Sha256::new();
    entire.update(header);
    entire.update(&json);
    let mut total = 52 + length;
    let mut previous = String::new();
    let mut small_objects = BTreeMap::new();
    let mut buffer = [0u8; BLOCK];
    for object in objects {
        let digest = object["sha256"].as_str().context("object digest absent")?;
        ensure!(
            digest.len() == 64 && digest > previous.as_str(),
            "object ordering/identity changed"
        );
        previous = digest.into();
        let bytes = object["byte_len"]
            .as_u64()
            .context("object length absent")?;
        total = total.checked_add(bytes).context("object total overflow")?;
        let mut remaining = bytes;
        let mut content = Sha256::new();
        let mut small = Vec::new();
        while remaining > 0 {
            let n = remaining.min(BLOCK as u64) as usize;
            file.read_exact(&mut buffer[..n])?;
            content.update(&buffer[..n]);
            entire.update(&buffer[..n]);
            if bytes <= 1024 * 1024 {
                small.extend_from_slice(&buffer[..n]);
            }
            remaining -= n as u64;
        }
        ensure!(
            format!("{:x}", content.finalize()) == digest,
            "object content digest mismatch"
        );
        if bytes <= 1024 * 1024 {
            small_objects.insert(digest.into(), small);
        }
    }
    let mut trailing = [0u8; 1];
    ensure!(
        file.read(&mut trailing)? == 0,
        "undeclared trailing package bytes"
    );
    ensure!(
        total == artifact.byte_len && file.metadata()?.len() == total,
        "package exact byte length changed"
    );
    let digest = format!("{:x}", entire.finalize());
    ensure!(digest == artifact.sha256, "package receipt digest mismatch");
    Ok(CheckedBundle {
        manifest,
        small_objects,
        bytes: total,
        sha256: digest,
    })
}
fn check_project(
    bundle: &CheckedBundle,
    p: &ProjectFixture,
    revision: u64,
    source_count: usize,
) -> Result<()> {
    let m = &bundle.manifest;
    ensure!(
        m["origin_project_id"] == serde_json::to_value(&p.project)?
            && m["captured_revision"].as_u64() == Some(revision),
        "wrong captured project/revision"
    );
    ensure!(
        m["head"]["name"] == "P1b synthetic export",
        "project name changed"
    );
    let sources = m["sources"].as_array().context("source catalog absent")?;
    ensure!(
        sources.len() == source_count,
        "captured source set is mixed or incomplete"
    );
    let source = sources
        .iter()
        .find(|s| s["source_version"] == serde_json::to_value(&p.source).unwrap())
        .context("expected source omitted")?;
    ensure!(
        source["kind"] == "media"
            && source["identity"]["sha256"] == p.source_hash
            && source["identity"]["byte_len"].as_u64() == Some(p.source_bytes),
        "source kind/identity changed or mutable original substituted"
    );
    let hash = m["head"]["motion"]["sha256"]
        .as_str()
        .context("head motion absent")?;
    let program: Value = serde_json::from_slice(
        bundle
            .small_objects
            .get(hash)
            .context("head motion payload missing")?,
    )?;
    let tracks = program["tracks"]
        .as_array()
        .context("motion tracks absent")?;
    ensure!(tracks.len() == 2, "axis dropped or invented");
    for (axis, positions) in [
        ("stroke", [35, 45, 40, if revision == 1 { 50 } else { 48 }]),
        ("sway", [20, 22, 21, 23]),
    ] {
        let track = tracks
            .iter()
            .find(|t| t["axis"] == axis)
            .context("expected axis absent")?;
        let actions = track["actions"].as_array().context("actions absent")?;
        ensure!(actions.len() == 4, "motion actions dropped or invented");
        for (index, action) in actions.iter().enumerate() {
            ensure!(
                action["time"].as_i64() == Some(index as i64 * 100_000_000),
                "exact nanosecond timeline changed"
            );
            let actual = action["position"].as_f64().context("position absent")?;
            ensure!(
                actual.to_bits() == (f64::from(positions[index]) / 100.0).to_bits(),
                "motion value changed"
            );
        }
    }
    Ok(())
}

#[test]
fn package_export_shared_client_preserves_closure_sources_and_destinations() -> Result<()> {
    let mut f = Fixture::new()?;
    let mut owner = f.owner()?;
    let p = setup(&f, &mut owner, None)?;
    // New creators intentionally receive PackageProject. Establish the old
    // grant shape through the public authority API, never by rewriting SQL.
    // Only the session ID is retained; credential bytes are never logged.
    let owner_session: SessionId = {
        let mut credentials: Value =
            serde_json::from_reader(File::open(f.state.join("first-party.session.json"))?)?;
        serde_json::from_value(
            credentials
                .get_mut("session")
                .context("first-party session ID missing")?
                .take(),
        )?
    };
    let manager = f.pair("legacy-grant-fixture-manager")?;
    let legacy_scopes = vec![Scope::Read, Scope::Edit, Scope::ManageGrants];
    delegate(&mut owner, &manager, &p, legacy_scopes.clone())?;
    let mut manager_client = manager.client(&f)?;
    manager_client.execute(
        Command::Revoke {
            session: owner_session.clone(),
            project_id: p.project.clone(),
        },
        None,
        None,
    )?;
    manager_client.execute(
        Command::Grant {
            session: owner_session,
            project_id: p.project.clone(),
            scopes: legacy_scopes,
        },
        None,
        None,
    )?;
    rejected(
        owner.start_project_export(p.project.clone(), p.revision, &consent()),
        &[ErrorCode::Forbidden],
    )
    .context("start before explicit package approval")?;
    let pair_proof = fs::read_to_string(f.state.join("pairing.token"))?
        .trim()
        .to_owned();
    rejected(
        owner.execute(
            Command::AllowProjectPackaging {
                owner_proof: pair_proof,
            },
            Some(p.project.clone()),
            None,
        ),
        &[ErrorCode::Forbidden, ErrorCode::Unauthorized],
    )
    .context("pairing token must not authorize owner package approval")?;
    f.approve(&p.project)?;
    fs::rename(&p.source_path, f.root.join("moved-original.bin"))?;
    fs::write(
        &p.source_path,
        b"The mutable original now has contradictory bytes.",
    )?;
    let started = owner.start_project_export(p.project.clone(), p.revision, &consent())?;
    let status = ready(&mut owner, &p, &started.operation_id)?;
    let destination = f.root.join("project.pulsar");
    let receipt = owner.download_project_package(
        p.project.clone(),
        status.operation_id.clone(),
        &destination,
        &consent(),
        &PackageDownloadControl::default(),
    )?;
    ensure!(
        receipt.file_and_directory_synced && !receipt.engine_release_requested,
        "shared helper did not report durable retained publication"
    );
    let artifact = status.artifact.as_ref().unwrap();
    let checked = inspect_bundle(&destination, artifact)?;
    check_project(&checked, &p, 1, 1)?;
    ensure!(
        checked.bytes == artifact.byte_len && checked.sha256 == artifact.sha256,
        "independent package identity changed"
    );

    let unchanged = digest_file(&destination)?;
    ensure!(
        owner
            .download_project_package(
                p.project.clone(),
                status.operation_id.clone(),
                &destination,
                &consent(),
                &PackageDownloadControl::default()
            )
            .is_err(),
        "existing destination was overwritten"
    );
    ensure!(
        digest_file(&destination)? == unchanged,
        "nonclobber failure changed existing bytes"
    );
    let target = f.root.join("private-canary");
    fs::write(&target, b"UNTOUCHED")?;
    let link = f.root.join("destination-symlink.pulsar");
    symlink(&target, &link)?;
    ensure!(
        owner
            .download_project_package(
                p.project.clone(),
                status.operation_id.clone(),
                &link,
                &consent(),
                &PackageDownloadControl::default()
            )
            .is_err(),
        "symlink destination was followed"
    );
    ensure!(fs::read(&target)? == b"UNTOUCHED", "symlink target changed");

    owner.execute(
        Command::EvictSource {
            source_version: p.source.clone(),
        },
        Some(p.project.clone()),
        None,
    )?;
    let retained = f.root.join("after-source-eviction.pulsar");
    owner.download_project_package(
        p.project.clone(),
        status.operation_id.clone(),
        &retained,
        &consent(),
        &PackageDownloadControl::default(),
    )?;
    check_project(&inspect_bundle(&retained, artifact)?, &p, 1, 1)?;
    let missing = owner.start_project_export(p.project.clone(), p.revision, &consent())?;
    let failed = terminal(&mut owner, &p, &missing.operation_id)?;
    ensure!(
        failed.state == PackageExportOperationState::Failed && failed.artifact.is_none(),
        "missing source was replaced or falsely declared ready"
    );
    ensure!(
        failed
            .error
            .as_ref()
            .is_some_and(|e| e.message.contains(&p.source_hash)),
        "missing source error omitted content identity"
    );
    ensure!(
        client_project(owner.execute(Command::GetSnapshot, Some(p.project.clone()), None)?)?
            .revision
            == p.revision,
        "export failure mutated project"
    );
    let released = owner.release_project_export(p.project.clone(), status.operation_id.clone())?;
    ensure!(
        released.state == PackageExportOperationState::Released,
        "explicit release did not retire availability"
    );
    ensure!(
        owner
            .download_project_package(
                p.project.clone(),
                status.operation_id,
                &f.root.join("released.pulsar"),
                &consent(),
                &PackageDownloadControl::default()
            )
            .is_err(),
        "released artifact remained downloadable"
    );
    f.completed = true;
    Ok(())
}

#[test]
fn package_export_scope_replay_disconnect_and_revocation_remain_distinct() -> Result<()> {
    let mut f = Fixture::new()?;
    let mut owner = f.owner()?;
    let p = setup(&f, &mut owner, Some(2 * 1024 * 1024))?;
    f.approve(&p.project)?;
    let exporter = f.pair("scoped package exporter")?;
    let other = f.pair("other permitted exporter")?;
    let reader = f.pair("neutral-only reader")?;
    delegate(
        &mut owner,
        &exporter,
        &p,
        vec![Scope::Read, Scope::PackageProject],
    )?;
    delegate(
        &mut owner,
        &other,
        &p,
        vec![Scope::Read, Scope::PackageProject],
    )?;
    delegate(&mut owner, &reader, &p, vec![Scope::Read, Scope::Export])?;
    rejected(
        reader.call(
            &f,
            Command::StartProjectExport,
            &p.project,
            Some(p.revision),
        ),
        &[ErrorCode::Forbidden],
    )?;
    let request = exporter.request(Command::StartProjectExport, &p.project, Some(p.revision));
    let started = package(f.call(&request)?)?;
    ensure!(
        package(f.call(&request)?)?.operation_id == started.operation_id,
        "same start request created another operation"
    );
    let mut changed = request.clone();
    changed.expected_revision = Some(RevisionId::new(2));
    rejected(f.call(&changed), &[ErrorCode::RequestConflict])?;
    let mut client = exporter.client(&f)?;
    ensure!(
        client
            .start_project_export_with_request_id(
                p.project.clone(),
                p.revision,
                &consent(),
                request.request_id.clone()
            )?
            .operation_id
            == started.operation_id,
        "shared explicit Start replay changed operation"
    );
    let status = ready(&mut client, &p, &started.operation_id)?;
    rejected(
        other.call(
            &f,
            Command::ProjectPackageStatus {
                operation_id: status.operation_id.clone(),
            },
            &p.project,
            None,
        ),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    let alien = client_project(owner.execute(
        Command::CreateProject {
            name: "unrelated".into(),
        },
        None,
        None,
    )?)?;
    rejected(
        exporter.call(
            &f,
            Command::ProjectPackageStatus {
                operation_id: status.operation_id.clone(),
            },
            &alien.project_id,
            None,
        ),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;

    let download = begin(&exporter, &f, &p, &status)?;
    rejected(
        connect_bulk(&other, &download),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )
    .context("different actor presenting a live lease")?;
    let mut bulk = connect_bulk(&exporter, &download)?;
    let first = bulk.download(0, 4096)?;
    drop(bulk);
    ensure!(
        client
            .project_package_status(p.project.clone(), status.operation_id.clone())?
            .state
            == PackageExportOperationState::Ready,
        "disconnect revoked the export"
    );
    let retained = lease(exporter.call(
        &f,
        Command::ProjectPackageDownloadStatus {
            lease_id: download.lease_id.clone(),
        },
        &p.project,
        None,
    )?)?;
    ensure!(
        retained.next_offset == 4096,
        "disconnect discarded download cursor"
    );
    let mut resumed = connect_bulk(&exporter, &retained)?;
    ensure!(
        resumed.download(0, 4096)? == first,
        "exact last-chunk replay changed bytes"
    );
    ensure!(
        resumed.download(4096, 4096)?.len() == 4096,
        "next chunk after reconnect failed"
    );
    drop(resumed);
    abandon(&exporter, &f, &p, &retained)?;

    let revoked = begin(&exporter, &f, &p, &status)?;
    let mut active = connect_bulk(&exporter, &revoked)?;
    active.download(0, 4096)?;
    owner.execute(
        Command::Revoke {
            session: exporter.session.clone(),
            project_id: p.project.clone(),
        },
        None,
        None,
    )?;
    rejected(
        active.download(4096, 4096).map_err(Into::into),
        &[
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::Unauthorized,
        ],
    )
    .context("active connection after revocation")?;
    rejected(
        exporter.call(
            &f,
            Command::ProjectPackageStatus {
                operation_id: status.operation_id.clone(),
            },
            &p.project,
            None,
        ),
        &[ErrorCode::Forbidden],
    )?;
    delegate(
        &mut owner,
        &exporter,
        &p,
        vec![Scope::Read, Scope::PackageProject],
    )?;
    rejected(
        connect_bulk(&exporter, &revoked),
        &[
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::InvalidRequest,
            ErrorCode::Unavailable,
        ],
    )
    .context("old revoked lease after explicit regrant")?;
    let fresh = begin(&exporter, &f, &p, &status)?;
    let mut new = connect_bulk(&exporter, &fresh)?;
    ensure!(
        new.download(0, 4096)? == first,
        "fresh reauthorized lease cannot read retained artifact"
    );
    drop(new);
    abandon(&exporter, &f, &p, &fresh)?;
    ensure!(
        f.max_control.load(Ordering::Relaxed) <= MAX_CONTROL_BYTES as u64,
        "control cap exceeded"
    );
    f.completed = true;
    Ok(())
}

#[test]
fn package_export_capture_stays_coherent_during_edit_catalog_change_and_eviction() -> Result<()> {
    let mut f = Fixture::new()?;
    let mut owner = f.owner()?;
    let p = setup(&f, &mut owner, Some(RACE_SOURCE_BYTES))?;
    f.approve(&p.project)?;
    let operation = owner.start_project_export(p.project.clone(), p.revision, &consent())?;
    let captured = streaming(&mut owner, &p, &operation.operation_id)?;
    let binding = captured.capture.clone().unwrap();
    let probe = Instant::now();
    ensure!(
        client_project(owner.execute(Command::GetSnapshot, Some(p.project.clone()), None)?)?
            .revision
            == p.revision,
        "control snapshot changed during Streaming"
    );
    eprintln!(
        "P1b CONTROL: phase=streaming_concurrent_edit get_snapshot_ms={:.3}",
        probe.elapsed().as_secs_f64() * 1000.0
    );
    let candidate = owner.upload_edit(
        p.project.clone(),
        p.revision,
        &motion(true),
        "edit after package capture",
    )?;
    owner.execute(
        Command::CommitCandidate {
            candidate_id: candidate.candidate_id,
        },
        Some(p.project.clone()),
        Some(p.revision),
    )?;
    let late = f.root.join("source-after-capture.bin");
    fs::write(&late, b"CATALOG ENTRY AFTER PINNED CAPTURE")?;
    owner.execute(
        Command::ImportSource { path: late },
        Some(p.project.clone()),
        None,
    )?;
    let eviction = owner.execute(
        Command::EvictSource {
            source_version: p.source.clone(),
        },
        Some(p.project.clone()),
        None,
    );
    let following =
        owner.project_package_status(p.project.clone(), operation.operation_id.clone())?;
    if eviction.is_ok() {
        ensure!(
            following.state == PackageExportOperationState::Ready,
            "held source evicted before durable package readiness"
        );
        eprintln!("P1b eviction raced after Ready; internal held-source gate supplies deterministic hold rejection coverage");
    } else {
        rejected(eviction, &[ErrorCode::Forbidden])
            .context("held-source eviction must fail with authority-bound retention")?;
        eprintln!("P1b actual held-source eviction rejected before Ready");
    }
    let status = ready(&mut owner, &p, &operation.operation_id)?;
    ensure!(
        status.capture.as_ref() == Some(&binding),
        "capture identity changed after edits/catalog mutation"
    );
    let path = f.root.join("coherent-capture.pulsar");
    owner.download_project_package(
        p.project.clone(),
        operation.operation_id,
        &path,
        &consent(),
        &PackageDownloadControl::default(),
    )?;
    check_project(
        &inspect_bundle(&path, status.artifact.as_ref().unwrap())?,
        &p,
        1,
        1,
    )?;
    ensure!(
        client_project(owner.execute(Command::GetSnapshot, Some(p.project.clone()), None)?)?
            .revision
            == RevisionId::new(2),
        "concurrent edit did not remain committed"
    );
    f.completed = true;
    Ok(())
}

#[test]
fn package_export_active_cancellation_releases_holds_without_publishing_ready() -> Result<()> {
    let mut f = Fixture::new()?;
    let mut owner = f.owner()?;
    let p = setup(&f, &mut owner, Some(RACE_SOURCE_BYTES))?;
    f.approve(&p.project)?;
    let operation = owner.start_project_export(p.project.clone(), p.revision, &consent())?;
    streaming(&mut owner, &p, &operation.operation_id)?;
    let requested =
        owner.cancel_project_export(p.project.clone(), operation.operation_id.clone())?;
    ensure!(
        requested.state != PackageExportOperationState::Ready,
        "fixture cancellation raced after Ready; active cancellation not qualified"
    );
    let cancelled = terminal(&mut owner, &p, &operation.operation_id)?;
    ensure!(
        cancelled.state == PackageExportOperationState::Cancelled && cancelled.artifact.is_none(),
        "cancelled export falsely published availability"
    );
    ensure!(
        owner
            .cancel_project_export(p.project.clone(), operation.operation_id.clone())?
            .state
            == PackageExportOperationState::Cancelled,
        "repeated cancellation changed terminal state"
    );
    ensure!(
        owner
            .download_project_package(
                p.project.clone(),
                operation.operation_id,
                &f.root.join("cancelled.pulsar"),
                &consent(),
                &PackageDownloadControl::default()
            )
            .is_err(),
        "cancelled export downloaded"
    );
    // Cancelled prevents any later Ready publication. Parent contract permits
    // worker teardown to retain source holds briefly after that durable ack.
    let teardown_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match owner.execute(
            Command::EvictSource {
                source_version: p.source.clone(),
            },
            Some(p.project.clone()),
            None,
        ) {
            Ok(_) => break,
            Err(error)
                if error_code(&error) == Some(ErrorCode::Forbidden)
                    && error.to_string().contains("active project package") =>
            {
                ensure!(
                    Instant::now() < teardown_deadline,
                    "cancelled export never released its source hold: {error}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error).context("evict after bounded cancellation teardown"),
        }
    }
    ensure!(
        client_project(owner.execute(Command::GetSnapshot, Some(p.project.clone()), None)?)?
            .revision
            == p.revision,
        "cancellation mutated project"
    );
    f.completed = true;
    Ok(())
}

#[test]
fn package_export_lost_ack_and_restart_preserve_ready_and_interrupt_active_work() -> Result<()> {
    let mut f = Fixture::new()?;
    let mut owner = f.owner()?;
    let p = setup(&f, &mut owner, Some(RACE_SOURCE_BYTES))?;
    f.approve(&p.project)?;
    let actor = f.pair("restart export owner")?;
    delegate(
        &mut owner,
        &actor,
        &p,
        vec![Scope::Read, Scope::PackageProject],
    )?;
    let request = actor.request(Command::StartProjectExport, &p.project, Some(p.revision));
    let discarded = package(f.lost_ack(request.clone())?.result?)?;
    let replay = package(f.call(&request)?)?;
    ensure!(
        replay.operation_id == discarded.operation_id,
        "lost acknowledgement duplicated operation"
    );
    let mut client = actor.client(&f)?;
    let status = ready(&mut client, &p, &replay.operation_id)?;
    let old_lease = begin(&actor, &f, &p, &status)?;
    let mut bulk = connect_bulk(&actor, &old_lease)?;
    bulk.download(0, 4096)?;
    drop(bulk);
    f.restart()?;
    client = actor.client(&f)?;
    ensure!(
        package(f.call(&request)?)?.operation_id == status.operation_id,
        "restart changed durable Start replay identity"
    );
    let verified = ready(&mut client, &p, &status.operation_id)?;
    ensure!(
        verified.artifact.as_ref().unwrap().sha256 == status.artifact.as_ref().unwrap().sha256,
        "restart changed ready package bytes"
    );
    let fresh = begin(&actor, &f, &p, &verified)?;
    let mut stale = old_lease.clone();
    stale.bulk_endpoint = fresh.bulk_endpoint.clone();
    rejected(
        connect_bulk(&actor, &stale),
        &[
            ErrorCode::NotFound,
            ErrorCode::Forbidden,
            ErrorCode::DependencyMismatch,
            ErrorCode::InvalidRequest,
            ErrorCode::Unavailable,
        ],
    )?;
    abandon(&actor, &f, &p, &fresh)?;
    let path = f.root.join("restarted-ready.pulsar");
    client.download_project_package(
        p.project.clone(),
        status.operation_id.clone(),
        &path,
        &consent(),
        &PackageDownloadControl::default(),
    )?;
    check_project(
        &inspect_bundle(&path, verified.artifact.as_ref().unwrap())?,
        &p,
        1,
        1,
    )?;

    let active_request = actor.request(Command::StartProjectExport, &p.project, Some(p.revision));
    let active = package(f.call(&active_request)?)?;
    streaming(&mut client, &p, &active.operation_id)?;
    f.restart()?;
    client = actor.client(&f)?;
    let interrupted = terminal(&mut client, &p, &active.operation_id)?;
    ensure!(
        interrupted.state == PackageExportOperationState::Interrupted
            && interrupted.artifact.is_none(),
        "unfinished export resurrected as ready"
    );
    ensure!(
        package(f.call(&active_request)?)?.operation_id == active.operation_id,
        "interrupted Start replay created a new export"
    );
    ensure!(
        client
            .download_project_package(
                p.project.clone(),
                active.operation_id,
                &f.root.join("interrupted.pulsar"),
                &consent(),
                &PackageDownloadControl::default()
            )
            .is_err(),
        "interrupted export became downloadable"
    );
    f.completed = true;
    Ok(())
}

fn rss(pid: u32) -> Result<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let line = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .context("VmRSS measurement unavailable")?;
    let kib = line
        .split_whitespace()
        .nth(1)
        .context("VmRSS value absent")?
        .parse::<u64>()?;
    kib.checked_mul(1024).context("VmRSS byte overflow")
}
struct RssSampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<(u64, u64, u64)>>>,
    engine_base: u64,
    client_base: u64,
}
impl RssSampler {
    fn start(engine: u32) -> Result<Self> {
        let engine_base = rss(engine)?;
        let client = std::process::id();
        let client_base = rss(client)?;
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let thread = std::thread::spawn(move || {
            let (mut engine_peak, mut client_peak, mut samples) = (engine_base, client_base, 0);
            while !done.load(Ordering::Relaxed) {
                engine_peak = engine_peak.max(rss(engine)?);
                client_peak = client_peak.max(rss(client)?);
                samples += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok((engine_peak, client_peak, samples))
        });
        Ok(Self {
            stop,
            thread: Some(thread),
            engine_base,
            client_base,
        })
    }
    fn finish(mut self) -> Result<(u64, u64, u64)> {
        self.stop.store(true, Ordering::Relaxed);
        let (engine, client, samples) = self
            .thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow::anyhow!("RSS sampler panicked"))??;
        ensure!(samples > 1, "RSS observation window was not sampled");
        Ok((
            engine.saturating_sub(self.engine_base),
            client.saturating_sub(self.client_base),
            samples,
        ))
    }
}
impl Drop for RssSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
#[ignore = "explicit >1 GiB generated-data streaming and RSS qualification; uses private temporary disk"]
fn package_export_streams_more_than_one_gib_with_bounded_chunks_and_sampled_rss() -> Result<()> {
    let mut f = Fixture::new()?;
    f.always_cleanup = true;
    let mut owner = f.owner()?;
    let sampler = RssSampler::start(f.child.as_ref().unwrap().id())?;
    let started = Instant::now();
    let p = setup(&f, &mut owner, Some(LARGE_SOURCE_BYTES))?;
    f.approve(&p.project)?;
    let operation = owner.start_project_export(p.project.clone(), p.revision, &consent())?;
    let active = streaming(&mut owner, &p, &operation.operation_id)?;
    ensure!(
        active
            .capture
            .as_ref()
            .is_some_and(|capture| capture.captured_revision == p.revision),
        "large Streaming capture lost pinned revision"
    );
    let probe = Instant::now();
    ensure!(
        client_project(owner.execute(Command::GetSnapshot, Some(p.project.clone()), None)?)?
            .revision
            == p.revision,
        "control snapshot changed during large Streaming"
    );
    eprintln!(
        "P1b CONTROL: phase=large_streaming get_snapshot_ms={:.3}",
        probe.elapsed().as_secs_f64() * 1000.0
    );
    let status = ready(&mut owner, &p, &operation.operation_id)?;
    let capture_seconds = started.elapsed().as_secs_f64();
    let destination = f.root.join("greater-than-one-gib.pulsar");
    let control = PackageDownloadControl::default();
    let download_started = Instant::now();
    let publication = owner.download_project_package(
        p.project.clone(),
        operation.operation_id.clone(),
        &destination,
        &consent(),
        &control,
    )?;
    let download_seconds = download_started.elapsed().as_secs_f64();
    let artifact = status.artifact.as_ref().unwrap();
    ensure!(
        artifact.byte_len > 1024 * 1024 * 1024
            && artifact.byte_len.div_ceil(MAX_ARTIFACT_CHUNK_BYTES as u64) > 4096,
        "fixture did not cross the old 4096-chunk boundary"
    );
    ensure!(
        control.received_bytes() == artifact.byte_len && control.total_bytes() == artifact.byte_len,
        "shared helper progress did not retain exact byte count"
    );
    ensure!(
        publication.file_and_directory_synced,
        "large client publication lacks durable completion"
    );
    let checked = inspect_bundle(&destination, artifact)?;
    check_project(&checked, &p, 1, 1)?;
    let (engine_delta, client_delta, samples) = sampler.finish()?;
    // Host-specific sampled RSS tripwires, not formal heap bounds, reference
    // hardware qualification, a total-RAM guarantee, or media-generation speed.
    ensure!(
        engine_delta < RSS_DELTA_TRIPWIRE && client_delta < RSS_DELTA_TRIPWIRE,
        "sampled RSS regression: engine delta {engine_delta}, client delta {client_delta}"
    );
    eprintln!("P1b LARGE: source_bytes={} package_bytes={} minimum_256k_chunks={} capture_plus_import_seconds={:.3} download_seconds={:.3} sampled_engine_rss_delta={} sampled_client_rss_delta={} samples={} temporary_disk=private_sparse_source+snapshot+uncompressed_engine_package+client_copy",
        p.source_bytes,artifact.byte_len,artifact.byte_len.div_ceil(MAX_ARTIFACT_CHUNK_BYTES as u64),capture_seconds,download_seconds,engine_delta,client_delta,samples);
    owner.release_project_export(p.project.clone(), operation.operation_id)?;
    f.completed = true;
    Ok(())
}
