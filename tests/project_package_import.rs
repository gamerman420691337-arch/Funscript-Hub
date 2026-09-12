//! P1c actual secured-process fresh-project import QA. Synthetic inputs only.
//! These small fixtures do not qualify GUI parity, media quality, or physical devices.

#![cfg(target_os = "linux")]

use anyhow::{bail, ensure, Context, Result};
use pulsar_clients::project_package::{PackageDownloadControl, PackagePrivacyConsent};
use pulsar_clients::project_package_import::{PackageImportControl, PreparedPackageImport};
use pulsar_clients::{EngineApi, ResponseBody as ClientResponse, SessionClient};
use pulsar_protocol::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(120);
const BLOCK: usize = 64 * 1024;
const MEDIA: &[u8] = b"P6\n2 2\n255\n\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\xff\x00";
static NEXT: AtomicU64 = AtomicU64::new(1);

fn request_id() -> RequestId {
    RequestId::new(format!(
        "package-import-qa-{}-{}",
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
            "pulsar-package-import-{}-{nonce}",
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
    fn unscoped(&self, command: Command) -> Request {
        Request::new(request_id(), command)
            .with_session(self.session.clone())
            .with_auth_token(self.auth_token.clone())
    }
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

fn motion(branch: u8) -> MotionProgram {
    let stroke = match branch {
        1 => [35, 45, 40, 52],
        2 | 3 => [35, 45, 40, 48],
        4 => [35, 45, 40, 49],
        5 => [35, 46, 40, 49],
        _ => [35, 45, 40, 50],
    };
    let sway = if branch >= 3 {
        [20, 22, 21, 25]
    } else {
        [20, 22, 21, 23]
    };
    MotionProgram::new(
        [(Axis::Stroke, stroke), (Axis::Sway, sway)]
            .into_iter()
            .map(|(axis, values)| {
                MotionTrack::new(
                    axis,
                    values
                        .into_iter()
                        .enumerate()
                        .map(|(i, pos)| {
                            MotionAction::new(
                                ProjectTime::from_nanos(i as i64 * 100_000_000),
                                NormalizedPosition::new(f64::from(pos) / 100.0).unwrap(),
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
fn protected() -> Vec<ProtectedRegion> {
    vec![ProtectedRegion {
        axis: Some(Axis::Stroke),
        range: TimeRange::new(
            ProjectTime::from_nanos(100_000_000),
            ProjectTime::from_nanos(150_000_000),
        )
        .unwrap(),
    }]
}

const PROJECT_NAME: &str = "P1c independent import";
const IMPORT_BYTES: &[u8] = br#"{"version":"1.0","actions":[{"at":0,"pos":35},{"at":100,"pos":45},{"at":200,"pos":40},{"at":300,"pos":50}]}"#;
const FOREIGN_MARKER: &str = "OTHER_PROJECT_MUST_NOT_LEAK_P1C_6c1182";

struct ExpectedGraph {
    project: ProjectId,
    head_revision: u64,
    media: SourceVersionId,
    candidates: Vec<(CandidateId, u64, Option<u64>, u8)>,
    export_hash: String,
}
struct ArchiveFixture {
    expected: ExpectedGraph,
    path: PathBuf,
    artifact: PackageArtifactDescriptor,
    checked: CheckedBundle,
    bytes: Vec<u8>,
}

fn current(
    client: &mut SessionClient,
    project: &ProjectId,
) -> Result<pulsar_clients::ProjectSnapshot> {
    client_project(client.execute(Command::GetSnapshot, Some(project.clone()), None)?)
}
fn commit(
    client: &mut SessionClient,
    project: &ProjectId,
    base: u64,
    candidate: &CandidateId,
) -> Result<()> {
    let snapshot = client_project(client.execute(
        Command::CommitCandidate {
            candidate_id: candidate.clone(),
        },
        Some(project.clone()),
        Some(RevisionId::new(base)),
    )?)?;
    ensure!(
        snapshot.revision == RevisionId::new(base + 1),
        "commit did not create exactly one fresh revision"
    );
    Ok(())
}
fn candidate_response(response: ClientResponse) -> Result<pulsar_clients::CandidateSnapshot> {
    match response {
        ClientResponse::Candidate(candidate) => Ok(candidate),
        _ => bail!("expected candidate"),
    }
}
fn export_ready(
    client: &mut SessionClient,
    project: &ProjectId,
    operation: &PackageOperationId,
) -> Result<PackageExportStatus> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let status = client.project_package_status(project.clone(), operation.clone())?;
        status.validate()?;
        if status.state.is_terminal() {
            ensure!(
                status.state == PackageExportOperationState::Ready,
                "genuine export did not become Ready: {:?}",
                status.error
            );
            return Ok(status);
        }
        ensure!(Instant::now() < deadline, "genuine export timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn export_project(
    client: &mut SessionClient,
    project: &ProjectId,
    revision: u64,
    path: &Path,
) -> Result<PackageArtifactDescriptor> {
    let operation =
        client.start_project_export(project.clone(), RevisionId::new(revision), &consent())?;
    let status = export_ready(client, project, &operation.operation_id)?;
    let publication = client.download_project_package(
        project.clone(),
        operation.operation_id,
        path,
        &consent(),
        &PackageDownloadControl::default(),
    )?;
    ensure!(
        publication.file_and_directory_synced,
        "genuine source archive was not durably published"
    );
    status.artifact.context("ready export lacks artifact")
}

fn build_archive(f: &Fixture, owner: &mut SessionClient) -> Result<ArchiveFixture> {
    let created = client_project(owner.execute(
        Command::CreateProject {
            name: PROJECT_NAME.into(),
        },
        None,
        None,
    )?)?;
    let project = created.project_id;
    ensure!(
        created.revision == RevisionId::new(0),
        "new project not at revision zero"
    );
    let media_path = f.root.join("authored-media.ppm");
    fs::write(&media_path, MEDIA)?;
    let media = match owner.execute(
        Command::ImportSource {
            path: media_path.clone(),
        },
        Some(project.clone()),
        None,
    )? {
        ClientResponse::Source { source_version } => source_version,
        _ => bail!("source import lacks version"),
    };
    owner.execute(
        Command::PinSource {
            source_version: media.clone(),
            pinned: true,
        },
        Some(project.clone()),
        None,
    )?;
    fs::write(&media_path, b"MUTABLE ORIGINAL IS NOT THE IMMUTABLE MEDIA")?;
    let script_path = f.root.join("authored-input.funscript");
    fs::write(&script_path, IMPORT_BYTES)?;
    let imported = candidate_response(owner.execute(
        Command::ImportFunscript { path: script_path },
        Some(project.clone()),
        Some(RevisionId::new(0)),
    )?)?
    .candidate_id;
    commit(owner, &project, 0, &imported)?;
    let authored = owner
        .upload_edit(
            project.clone(),
            RevisionId::new(1),
            &motion(0),
            "two-axis authored proposal",
        )?
        .candidate_id;
    commit(owner, &project, 1, &authored)?;
    let abandoned = owner
        .upload_edit(
            project.clone(),
            RevisionId::new(2),
            &motion(1),
            "later abandoned branch",
        )?
        .candidate_id;
    commit(owner, &project, 2, &abandoned)?;
    let undone = client_project(owner.execute(
        Command::Undo,
        Some(project.clone()),
        Some(RevisionId::new(3)),
    )?)?;
    ensure!(
        undone.revision == RevisionId::new(4),
        "Undo reused historical revision"
    );
    let branch = owner
        .upload_edit(
            project.clone(),
            RevisionId::new(4),
            &motion(2),
            "replacement branch",
        )?
        .candidate_id;
    commit(owner, &project, 4, &branch)?;
    let guarded = client_project(owner.execute(
        Command::SetProtectedRegions {
            regions: protected(),
        },
        Some(project.clone()),
        Some(RevisionId::new(5)),
    )?)?;
    ensure!(
        guarded.revision == RevisionId::new(6),
        "protection did not create fresh revision"
    );
    let uncommitted = owner
        .upload_edit(
            project.clone(),
            RevisionId::new(6),
            &motion(3),
            "retained uncommitted proposal",
        )?
        .candidate_id;
    ensure!(
        current(owner, &project)?.revision == RevisionId::new(6),
        "proposal implicitly committed"
    );
    let exported_path = f.root.join("explicit-stroke-export.funscript");
    match owner.execute(
        Command::ExportAxis {
            path: exported_path.clone(),
            axis: Axis::Stroke,
        },
        Some(project.clone()),
        Some(RevisionId::new(6)),
    )? {
        ClientResponse::AxisExported {
            path,
            revision,
            axis,
        } => ensure!(
            path == exported_path && revision == RevisionId::new(6) && axis == Axis::Stroke,
            "export receipt changed binding"
        ),
        _ => bail!("explicit axis export response changed"),
    }
    let exported_bytes = fs::read(exported_path)?;
    let exported: Value = serde_json::from_slice(&exported_bytes)?;
    let exported_actions = exported["actions"]
        .as_array()
        .context("export lacks actions")?;
    ensure!(
        exported_actions.len() == 4,
        "standard export changed action count"
    );
    for (i, action) in exported_actions.iter().enumerate() {
        ensure!(
            action["at"].as_u64() == Some(i as u64 * 100)
                && action["pos"].as_u64() == Some([35, 45, 40, 48][i]),
            "independent source export oracle changed"
        );
    }
    let expected = ExpectedGraph {
        project: project.clone(),
        head_revision: 6,
        media,
        candidates: vec![
            (imported, 0, Some(1), 11),
            (authored, 1, Some(2), 0),
            (abandoned, 2, Some(3), 1),
            (branch, 4, Some(5), 2),
            (uncommitted, 6, None, 3),
        ],
        export_hash: sha(&exported_bytes),
    };
    let other = client_project(owner.execute(
        Command::CreateProject {
            name: FOREIGN_MARKER.into(),
        },
        None,
        None,
    )?)?;
    let foreign = f.root.join("foreign-project-private.bin");
    fs::write(&foreign, FOREIGN_MARKER.as_bytes())?;
    owner.execute(
        Command::ImportSource { path: foreign },
        Some(other.project_id),
        None,
    )?;
    let path = f.root.join("genuine-source-project.pulsar");
    let artifact = export_project(owner, &project, 6, &path)?;
    ensure!(
        artifact.byte_len <= 2 * 1024 * 1024,
        "small fixture unexpectedly became large"
    );
    let checked = inspect_bundle(&path, &artifact)?;
    ensure!(
        checked.manifest["authored_lineage"]
            .as_array()
            .context("source authored lineage missing")?
            .len()
            == 4,
        "genuine source fixture lacks four authored receipts"
    );
    let mut bytes = Vec::new();
    File::open(&path)?
        .take(artifact.byte_len + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == artifact.byte_len,
        "source archive length mismatch"
    );
    ensure!(
        !bytes
            .windows(FOREIGN_MARKER.len())
            .any(|v| v == FOREIGN_MARKER.as_bytes()),
        "another project leaked into archive"
    );
    Ok(ArchiveFixture {
        expected,
        path,
        artifact,
        checked,
        bytes,
    })
}

fn check_motion(bytes: &[u8], shape: u8) -> Result<()> {
    let program: MotionProgram = serde_json::from_slice(bytes)?;
    check_motion_program(&program, shape)
}
fn check_motion_program(program: &MotionProgram, shape: u8) -> Result<()> {
    if shape == 10 {
        ensure!(program.tracks().is_empty(), "initial empty program changed");
        return Ok(());
    }
    let axes = if shape == 11 {
        vec![Axis::Stroke]
    } else {
        vec![Axis::Stroke, Axis::Sway]
    };
    ensure!(
        program.tracks().len() == axes.len(),
        "motion axis dropped or invented"
    );
    let stroke = match shape {
        1 => [35, 45, 40, 52],
        2 | 3 => [35, 45, 40, 48],
        4 => [35, 45, 40, 49],
        _ => [35, 45, 40, 50],
    };
    let sway = if shape == 3 || shape == 4 {
        [20, 22, 21, 25]
    } else {
        [20, 22, 21, 23]
    };
    for axis in axes {
        let track = program.track(axis).context("expected axis missing")?;
        ensure!(
            track.actions().len() == 4 && track.gaps().is_empty(),
            "actions or gaps changed"
        );
        for (i, action) in track.actions().iter().enumerate() {
            let expected = if axis == Axis::Stroke {
                stroke[i]
            } else {
                sway[i]
            };
            ensure!(
                action.time().as_nanos() == i as i64 * 100_000_000,
                "exact time changed"
            );
            ensure!(
                action.position().value().to_bits() == (f64::from(expected) / 100.0).to_bits(),
                "motion value changed"
            );
        }
    }
    Ok(())
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
        matches!(u16::from_le_bytes(header[8..10].try_into()?), 1 | 2)
            && u16::from_le_bytes(header[8..10].try_into()?) == artifact.format_version
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
    ensure!(
        manifest["format_version"].as_u64() == Some(u64::from(artifact.format_version)),
        "manifest/container/control format binding differs"
    );
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

fn as_id(value: &Value) -> Result<CandidateId> {
    Ok(serde_json::from_value(value.clone())?)
}
fn checked_object<'a>(bundle: &'a CheckedBundle, descriptor: &Value) -> Result<&'a [u8]> {
    let digest = descriptor["sha256"]
        .as_str()
        .context("object identity missing")?;
    bundle
        .small_objects
        .get(digest)
        .map(Vec::as_slice)
        .context("small object omitted")
}
fn check_import_graph(
    bundle: &CheckedBundle,
    source: &ArchiveFixture,
    project: &ProjectId,
) -> Result<BTreeMap<String, CandidateId>> {
    let m = &bundle.manifest;
    let head = source.expected.head_revision;
    ensure!(matches!(head, 6 | 7), "unknown independent fixture head");
    let count = source.expected.candidates.len();
    ensure!(
        m["origin_project_id"] == serde_json::to_value(project)?
            && project != &source.expected.project,
        "import/re-export did not use a fresh project"
    );
    ensure!(
        m["captured_revision"].as_u64() == Some(head)
            && m["head"]["revision"].as_u64() == Some(head),
        "numeric revisions changed"
    );
    ensure!(
        m["head"]["name"] == PROJECT_NAME
            && m["head"]["history_cursor"].as_u64() == Some(if head == 6 { 4 } else { 5 }),
        "project/history head changed"
    );
    ensure!(
        m["head"]["protected"] == serde_json::to_value(protected())?,
        "protection changed"
    );
    let revisions = m["revisions"].as_array().context("revision list missing")?;
    ensure!(
        revisions
            .iter()
            .map(|r| r["revision"].as_u64())
            .collect::<Vec<_>>()
            == (0..=head).map(Some).collect::<Vec<_>>(),
        "abandoned or ordinary revisions omitted"
    );
    for row in revisions {
        let n = row["revision"].as_u64().unwrap();
        let shape = match n {
            0 => 10,
            1 => 11,
            2 | 4 => 0,
            3 => 1,
            5 | 6 => 2,
            7 => 3,
            _ => bail!("unexpected revision"),
        };
        check_motion(checked_object(bundle, &row["motion"])?, shape)?;
        ensure!(
            row["protected"]
                .as_array()
                .context("historical protection missing")?
                .len()
                == usize::from(n >= 6),
            "historical protection changed"
        );
    }
    let history = m["edit_states"].as_array().context("history missing")?;
    ensure!(
        history
            .iter()
            .map(|r| r["position"].as_u64())
            .collect::<Vec<_>>()
            == (0..=if head == 6 { 4 } else { 5 })
                .map(Some)
                .collect::<Vec<_>>(),
        "retained history positions changed"
    );
    ensure!(
        history
            .iter()
            .map(|r| r["source_revision"].as_u64())
            .collect::<Vec<_>>()
            == if head == 6 {
                vec![0, 1, 2, 5, 6]
            } else {
                vec![0, 1, 2, 5, 6, 7]
            }
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>(),
        "history restoration sources changed"
    );
    check_motion(
        checked_object(bundle, &m["head"]["motion"])?,
        if head == 6 { 2 } else { 3 },
    )?;
    for row in history {
        let position = row["position"]
            .as_u64()
            .context("history position absent")?;
        let shape = match position {
            0 => 10,
            1 => 11,
            2 => 0,
            3 | 4 => 2,
            5 => 3,
            _ => bail!("unexpected retained history position"),
        };
        check_motion(checked_object(bundle, &row["motion"])?, shape)?;
        let expected_protected = if position >= 4 {
            protected()
        } else {
            Vec::new()
        };
        ensure!(
            row["protected"] == serde_json::to_value(expected_protected)?,
            "retained history protection changed at position {position}"
        );
    }
    let candidates = m["candidates"]
        .as_array()
        .context("candidate list missing")?;
    ensure!(candidates.len() == count, "candidate lost or invented");
    let mut mapping = BTreeMap::new();
    for (old, base, committed, shape) in &source.expected.candidates {
        let matches = candidates
            .iter()
            .filter(|r| {
                r["base_revision"].as_u64() == Some(*base)
                    && r["committed_revision"].as_u64() == *committed
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "candidate base/commit association changed"
        );
        let row = matches[0];
        let new = as_id(&row["candidate_id"])?;
        ensure!(
            !source
                .expected
                .candidates
                .iter()
                .any(|(id, _, _, _)| id == &new),
            "candidate ID was reused across projects"
        );
        check_motion(checked_object(bundle, &row["motion"])?, *shape)?;
        let original = source.checked.manifest["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| as_id(&r["candidate_id"]).is_ok_and(|id| id == *old))
            .unwrap();
        ensure!(
            row["review"] == original["review"],
            "archival candidate review was rewritten"
        );
        ensure!(
            row["job_origin"].is_null(),
            "ordinary imported proposal gained a live job"
        );
        mapping.insert(old.as_str().to_owned(), new);
    }
    ensure!(
        mapping.len() == count
            && mapping
                .values()
                .map(|id| id.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == count,
        "candidate mapping is not injective"
    );
    let lineage = m["revision_lineage"]
        .as_array()
        .context("lineage missing")?;
    ensure!(
        lineage.len() == (head + 1) as usize,
        "revision ancestry omitted"
    );
    let undo = lineage
        .iter()
        .find(|r| r["revision"].as_u64() == Some(4))
        .context("undo edge missing")?;
    ensure!(
        undo["parent_revision"].as_u64() == Some(3)
            && undo["restored_from_revision"].as_u64() == Some(2)
            && undo["restored_history_position"].as_u64() == Some(2)
            && undo["operation"] == "undo",
        "restoration edge changed"
    );
    for (old, _, committed, _) in &source.expected.candidates {
        if let Some(revision) = committed {
            let row = lineage
                .iter()
                .find(|r| r["revision"].as_u64() == Some(*revision))
                .context("candidate revision edge missing")?;
            ensure!(
                as_id(&row["candidate"])? == mapping[old.as_str()],
                "revision refers to an origin candidate rather than mapped active candidate"
            );
        }
    }
    ensure!(
        m["authored_lineage"]
            .as_array()
            .context("authored projection missing")?
            .is_empty(),
        "archived authored receipts became trusted local edit authority"
    );
    let sources = m["sources"].as_array().context("source catalog missing")?;
    ensure!(
        sources.len() == 2,
        "source omitted or foreign catalog leaked"
    );
    for (kind, bytes) in [("media", MEDIA), ("funscript", IMPORT_BYTES)] {
        let pinned = if kind == "media" {
            true
        } else {
            source.checked.manifest["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["kind"] == kind)
                .context("original script source missing")?["pinned"]
                .as_bool()
                .context("original pin intent missing")?
        };
        let row = sources
            .iter()
            .find(|r| r["kind"] == kind)
            .context("source kind missing")?;
        ensure!(
            row["identity"]["sha256"] == sha(bytes)
                && row["identity"]["byte_len"].as_u64() == Some(bytes.len() as u64),
            "source content identity changed"
        );
        ensure!(
            row["pinned"].as_bool() == Some(pinned) && row["evicted"].as_bool() == Some(false),
            "source pin/availability changed"
        );
        ensure!(
            checked_object(bundle, &row["identity"])? == bytes,
            "source bytes changed"
        );
    }
    let receipts = m["export_receipts"]
        .as_array()
        .context("export receipt list missing")?;
    ensure!(
        receipts.len() == 1
            && receipts[0]["revision"].as_u64() == Some(6)
            && receipts[0]["axis"] == "stroke"
            && receipts[0]["sha256"] == source.expected.export_hash,
        "inert export history changed"
    );
    ensure!(
        m["generated_origins"]
            .as_array()
            .context("generated origins missing")?
            .is_empty(),
        "ordinary fixture invented job origins"
    );
    for (hash, bytes) in &source.checked.small_objects {
        ensure!(
            bundle.small_objects.get(hash) == Some(bytes),
            "required original object bytes lost"
        );
    }
    Ok(mapping)
}

fn import_status(response: ResponseBody) -> Result<PackageImportStatus> {
    match response {
        ResponseBody::PackageImport(status) => {
            status.validate()?;
            Ok(status)
        }
        _ => bail!("expected import operation"),
    }
}
fn upload_lease(response: ResponseBody) -> Result<PackageUploadLease> {
    match response {
        ResponseBody::PackageUpload(lease) => {
            lease.validate()?;
            Ok(lease)
        }
        _ => bail!("expected import upload lease"),
    }
}
fn declaration(bytes: &[u8]) -> PackageUploadDeclaration {
    PackageUploadDeclaration {
        format_version: if bytes.len() >= 10
            && matches!(u16::from_le_bytes([bytes[8], bytes[9]]), 1 | 2)
        {
            u16::from_le_bytes([bytes[8], bytes[9]])
        } else {
            1
        },
        sha256: sha(bytes),
        byte_len: bytes.len() as u64,
    }
}
fn upload_handshake(actor: &Actor, lease: &PackageUploadLease) -> PackageUploadHandshake {
    PackageUploadHandshake {
        version: PROTOCOL_VERSION,
        channel: PackageUploadChannel::ProjectPackageUpload,
        session: actor.session.clone(),
        auth_token: actor.auth_token.clone(),
        lease_id: lease.lease_id.clone(),
        engine_epoch: lease.engine_epoch.clone(),
        operation_id: lease.operation_id.clone(),
        generation: lease.generation,
        package_sha256: lease.package.sha256.clone(),
    }
}
fn upload_client(actor: &Actor, lease: &PackageUploadLease) -> Result<PackageUploadClient> {
    let deadline = Instant::now() + Duration::from_secs(2);
    for attempt in 0..=20 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "typed connection contention exceeded test deadline"
        );
        let mut client = PackageUploadClient::connect(&lease.bulk_endpoint, remaining)?;
        match client.handshake(&upload_handshake(actor, lease)) {
            Ok(_) => return Ok(client),
            Err(TransportError::Protocol(error))
                if error.code == ErrorCode::PackageUploadConnectionBusy && error.retryable =>
            {
                ensure!(
                    attempt < 20 && Instant::now() < deadline,
                    "typed connection contention exhausted test budget"
                );
                // Explicit transient backoff, never an assumed retirement fence.
                std::thread::sleep(
                    Duration::from_millis(50)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!()
}
fn begin_upload(
    actor: &Actor,
    f: &Fixture,
    operation: &PackageImportOperationId,
) -> Result<PackageUploadLease> {
    upload_lease(f.call(&actor.unscoped(Command::BeginProjectPackageUpload {
        operation_id: operation.clone(),
    }))?)
}
fn raw_start(actor: &Actor, f: &Fixture, bytes: &[u8]) -> Result<(Request, PackageImportStatus)> {
    let request = actor.unscoped(Command::StartProjectImport {
        package: declaration(bytes),
    });
    let status = import_status(f.call(&request)?)?;
    ensure!(
        status.state == PackageImportOperationState::AwaitingUpload && status.receipt.is_none(),
        "Start published or skipped full-byte upload"
    );
    Ok((request, status))
}
fn send_all(actor: &Actor, lease: &PackageUploadLease, bytes: &[u8]) -> Result<()> {
    let mut client = upload_client(actor, lease)?;
    let mut offset = 0u64;
    for chunk in bytes.chunks(MAX_PACKAGE_UPLOAD_CHUNK_BYTES as usize) {
        let receipt = client.upload(offset, chunk)?;
        ensure!(
            receipt.offset == offset
                && receipt.byte_len as usize == chunk.len()
                && receipt.sha256 == sha(chunk),
            "upload acknowledgment lost payload identity"
        );
        offset += chunk.len() as u64;
    }
    ensure!(
        offset == lease.package.byte_len,
        "fixture did not send every declared byte"
    );
    Ok(())
}
fn seal_request(actor: &Actor, lease: &PackageUploadLease) -> Request {
    actor.unscoped(Command::SealProjectImport {
        operation_id: lease.operation_id.clone(),
        upload_generation: lease.generation,
    })
}
fn terminal_import(
    client: &mut SessionClient,
    operation: &PackageImportOperationId,
) -> Result<PackageImportStatus> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let status = client.project_import_status(operation.clone())?;
        status.validate()?;
        if status.state.is_terminal() {
            return Ok(status);
        }
        ensure!(
            status.receipt.is_none(),
            "unfinished import exposed a project receipt"
        );
        ensure!(
            Instant::now() < deadline,
            "import did not finish within bounded fixture deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn complete_receipt(status: &PackageImportStatus) -> Result<&PackageImportReceipt> {
    status.validate()?;
    ensure!(
        status.state == PackageImportOperationState::Completed,
        "import failed: {:?} {:?}",
        status.state,
        status.error
    );
    let receipt = status
        .receipt
        .as_ref()
        .context("completed import lacks receipt")?;
    ensure!(
        status.progress.received_bytes == status.package.byte_len
            && status.progress.verified_bytes == status.package.byte_len,
        "completed import skipped full bytes"
    );
    Ok(receipt)
}
fn shared_import(f: &Fixture, actor: &Actor, path: &Path) -> Result<PackageImportStatus> {
    let mut client = actor.client(f)?;
    let control = PackageImportControl::default();
    let mut prepared = PreparedPackageImport::open(path, &control)?;
    let expected = prepared.declaration().clone();
    ensure!(
        control.hashed_bytes() == expected.byte_len,
        "prepared file was not fully hashed"
    );
    let start_id = request_id();
    let started = client.start_project_import(&prepared, start_id.clone())?;
    ensure!(
        started.request_id == start_id && started.package == expected && started.receipt.is_none(),
        "shared Start changed declaration or published early"
    );
    let completed = client.continue_project_import(
        &mut prepared,
        started.operation_id,
        request_id(),
        request_id(),
        &control,
    )?;
    let receipt = complete_receipt(&completed)?;
    ensure!(
        receipt.package == expected,
        "publication changed prepared file identity"
    );
    ensure!(
        control.accepted_bytes() == expected.byte_len
            && control.verified_bytes() == expected.byte_len,
        "shared import progress did not report all bytes"
    );
    Ok(completed)
}
fn reexport(
    f: &Fixture,
    client: &mut SessionClient,
    project: &ProjectId,
    revision: u64,
    label: &str,
) -> Result<(PackageArtifactDescriptor, CheckedBundle, PathBuf)> {
    let path = f.root.join(format!("{label}.pulsar"));
    let artifact = export_project(client, project, revision, &path)?;
    let checked = inspect_bundle(&path, &artifact)?;
    Ok((artifact, checked, path))
}
fn original_json(source: &ArchiveFixture) -> Result<&[u8]> {
    let length = u64::from_le_bytes(source.bytes[12..20].try_into()?) as usize;
    source
        .bytes
        .get(52..52 + length)
        .context("source manifest range changed")
}
fn assert_origin_namespace(
    bundle: &CheckedBundle,
    source: &ArchiveFixture,
    mapping: &BTreeMap<String, CandidateId>,
) -> Result<()> {
    let origins = bundle.manifest["imported_origins"]
        .as_array()
        .context("inert origin namespaces missing")?;
    let origin = origins
        .iter()
        .find(|o| o["container"]["sha256"] == source.artifact.sha256)
        .context("immediate origin container identity missing")?;
    ensure!(
        origin["container"] == serde_json::to_value(&source.artifact)?,
        "origin container receipt changed"
    );
    ensure!(
        origin["manifest"]["sha256"] == source.artifact.manifest_sha256,
        "origin namespace not bound to exact manifest"
    );
    ensure!(
        checked_object(bundle, &origin["manifest"])? == original_json(source)?,
        "canonical original manifest bytes were rewritten"
    );
    ensure!(
        origin["objects"] == source.checked.manifest["objects"],
        "original declared object catalog changed"
    );
    let candidates = origin["candidates"]
        .as_array()
        .context("origin candidate map missing")?;
    ensure!(
        candidates.len() == mapping.len(),
        "candidate origin mapping incomplete"
    );
    for (old, new) in mapping {
        ensure!(
            candidates
                .iter()
                .any(|m| m["local"] == serde_json::to_value(new).unwrap() && m["origin"] == *old),
            "candidate origin mapping changed"
        );
    }
    for (hash, bytes) in &source.checked.small_objects {
        ensure!(
            bundle.small_objects.get(hash) == Some(bytes),
            "raw origin object lost or changed"
        );
    }
    Ok(())
}

fn raw_chunk_reply(
    actor: &Actor,
    lease: &PackageUploadLease,
    offset: u64,
    bytes: &[u8],
) -> Result<PackageUploadReply> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut admitted = None;
    for attempt in 0..=20 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "raw connection contention exceeded deadline"
        );
        let mut stream = UnixStream::connect(&lease.bulk_endpoint)?;
        stream.set_read_timeout(Some(remaining))?;
        stream.set_write_timeout(Some(remaining))?;
        write_bulk_header(&mut stream, &upload_handshake(actor, lease))?;
        let response: PackageUploadReply = read_bulk_header(&mut stream)?;
        match response {
            PackageUploadReply::Ready { lease: ready } => {
                ensure!(
                    ready.lease_id == lease.lease_id && ready.generation == lease.generation,
                    "raw upload bound wrong generation"
                );
                admitted = Some(stream);
                break;
            }
            PackageUploadReply::Error(error)
                if error.code == ErrorCode::PackageUploadConnectionBusy && error.retryable =>
            {
                ensure!(
                    attempt < 20 && Instant::now() < deadline,
                    "raw typed connection contention exhausted budget"
                );
                drop(stream);
                std::thread::sleep(
                    Duration::from_millis(50)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            other => bail!("raw upload handshake was not admitted: {other:?}"),
        }
    }
    let mut stream = admitted.context("raw upload lacked an admitted connection")?;
    let request = PackageUploadChunkReceipt {
        offset,
        byte_len: bytes.len() as u32,
        sha256: sha(bytes),
    };
    write_bulk_header(&mut stream, &request)?;
    write_artifact_chunk(&mut stream, bytes)?;
    Ok(read_bulk_header(&mut stream)?)
}

fn project_catalog(f: &Fixture) -> Result<std::collections::BTreeSet<String>> {
    // Read-only observation of this fixture's private SQLite state. No table,
    // grant, request identity, or project row is injected by the test.
    let connection = rusqlite::Connection::open_with_flags(
        f.state.join("projects.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    connection.busy_timeout(Duration::from_secs(2))?;
    let transaction = connection.unchecked_transaction()?;
    let projects = {
        let mut statement = transaction.prepare("SELECT id FROM projects ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?
    };
    transaction.commit()?;
    Ok(projects)
}
fn no_new_projects(f: &Fixture, before: &std::collections::BTreeSet<String>) -> Result<()> {
    ensure!(
        &project_catalog(f)? == before,
        "unfinished/failed import made a project visible"
    );
    Ok(())
}
fn exactly_one_new_project(
    f: &Fixture,
    before: &std::collections::BTreeSet<String>,
    project: &ProjectId,
) -> Result<()> {
    let mut expected = before.clone();
    ensure!(
        expected.insert(project.as_str().to_owned()),
        "import reused an existing project ID"
    );
    ensure!(
        project_catalog(f)? == expected,
        "import did not atomically publish exactly one project"
    );
    Ok(())
}
fn retained_original_container(f: &Fixture, source: &ArchiveFixture) -> Result<()> {
    let path = f
        .state
        .join("imported-project-containers")
        .join(&source.artifact.sha256);
    ensure!(
        fs::metadata(&path)?.permissions().mode() & 0o222 == 0,
        "recovery container remains writable"
    );
    ensure!(
        fs::read(path)? == source.bytes,
        "original full container bytes were reconstructed or changed"
    );
    Ok(())
}

fn import_diagnostics(
    client: &mut SessionClient,
    project: &ProjectId,
) -> Result<DiagnosticsReport> {
    match client.execute(
        Command::Diagnostics { candidate_id: None },
        Some(project.clone()),
        None,
    )? {
        ClientResponse::Diagnostics(report) => Ok(report),
        _ => bail!("diagnostics response changed"),
    }
}
fn assert_import_trust(client: &mut SessionClient, project: &ProjectId) -> Result<()> {
    let report = import_diagnostics(client, project)?;
    ensure!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "imported_origin_unverified"),
        "archive origin was silently promoted to verified local lineage"
    );
    ensure!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "review_history_unknown"),
        "computed imported review lost unknown state"
    );
    Ok(())
}
#[test]
fn project_package_import_clones_exact_closure_with_fresh_authority_and_ids() -> Result<()> {
    let mut f = Fixture::new()?;
    let source_actor = f.pair("origin-owner")?;
    let mut source_client = source_actor.client(&f)?;
    let source = build_archive(&f, &mut source_client)?;
    let importer = f.pair("fresh-import-owner")?;
    let before = project_catalog(&f)?;
    let completed = shared_import(&f, &importer, &source.path)?;
    let receipt = complete_receipt(&completed)?;
    let project = receipt.project_id.clone();
    ensure!(
        receipt.revision == RevisionId::new(6)
            && receipt.capture.project_id == source.expected.project,
        "import receipt lost origin/current distinction"
    );
    exactly_one_new_project(&f, &before, &project)?;
    retained_original_container(&f, &source)?;
    let mut client = importer.client(&f)?;
    rejected(
        source_client.execute(Command::GetSnapshot, Some(project.clone()), None),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    rejected(
        client.execute(
            Command::GetSnapshot,
            Some(source.expected.project.clone()),
            None,
        ),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    for (old, _, _, _) in &source.expected.candidates {
        rejected(
            client.execute(
                Command::GetCandidate {
                    candidate_id: old.clone(),
                },
                Some(project.clone()),
                None,
            ),
            &[ErrorCode::NotFound, ErrorCode::Forbidden],
        )?;
    }
    assert_import_trust(&mut client, &project)?;
    let (_, bundle, _) = reexport(&f, &mut client, &project, 6, "clone-before-edit")?;
    let mapping = check_import_graph(&bundle, &source, &project)?;
    assert_origin_namespace(&bundle, &source, &mapping)?;
    let pending = &source
        .expected
        .candidates
        .iter()
        .find(|(_, _, committed, _)| committed.is_none())
        .unwrap()
        .0;
    let active = mapping[pending.as_str()].clone();
    commit(&mut client, &project, 6, &active)?;
    check_motion_program(&current(&mut client, &project)?.program, 3)?;
    let replay = client.project_import_status(completed.operation_id)?;
    ensure!(
        complete_receipt(&replay)?.revision == RevisionId::new(6),
        "later local edit rewrote durable import receipt"
    );
    f.completed = true;
    Ok(())
}

#[test]
fn project_package_import_reopens_inert_history_and_native_edit_boundaries() -> Result<()> {
    let mut source_engine = Fixture::new()?;
    let source_actor = source_engine.pair("origin-author")?;
    // The private request journal is deliberately absent from the archive. Retain a
    // genuine originating request identity in the independent fixture instead.
    let historical_request = source_actor.unscoped(Command::CreateProject {
        name: "historical request identity fixture".into(),
    });
    let historical_project = match source_engine.call(&historical_request)? {
        ResponseBody::Project(snapshot) => snapshot.project_id,
        _ => bail!("historical request did not create its source project"),
    };
    let source = build_archive(&source_engine, &mut source_actor.client(&source_engine)?)?;
    let mut f = Fixture::new()?;
    let actor = f.pair("destination-owner")?;
    let path = f.root.join("relocated-small.pulsar");
    fs::write(&path, &source.bytes)?;
    source_engine.stop()?;
    // Only the explicitly provided container is available to the destination.
    let completed = shared_import(&f, &actor, &path)?;
    let project = complete_receipt(&completed)?.project_id.clone();
    retained_original_container(&f, &source)?;
    let mut client = actor.client(&f)?;
    let (_, bundle, _) = reexport(&f, &mut client, &project, 6, "destination-initial")?;
    let mapping = check_import_graph(&bundle, &source, &project)?;
    assert_origin_namespace(&bundle, &source, &mapping)?;
    f.restart()?;
    client = actor.client(&f)?;
    ensure!(
        current(&mut client, &project)?.revision == RevisionId::new(6),
        "durable reopen changed imported revision"
    );
    check_motion_program(&current(&mut client, &project)?.program, 2)?;
    assert_import_trust(&mut client, &project)?;
    let mut inert_id_request = actor.unscoped(Command::CreateProject {
        name: "origin request ID is inert".into(),
    });
    inert_id_request.request_id = historical_request.request_id;
    match f.call(&inert_id_request)? {
        ResponseBody::Project(snapshot) => {
            ensure!(
                snapshot.project_id != historical_project,
                "source request replay leaked its original project"
            );
            ensure!(
                snapshot.revision == RevisionId::new(0),
                "inert source identity did not create a fresh project"
            );
        }
        _ => bail!("origin request ID became active replay authority"),
    }
    let undo = client_project(client.execute(
        Command::Undo,
        Some(project.clone()),
        Some(RevisionId::new(6)),
    )?)?;
    ensure!(
        undo.revision == RevisionId::new(7),
        "Undo did not create a fresh revision"
    );
    check_motion_program(&undo.program, 2)?;
    ensure!(
        import_diagnostics(&mut client, &project)?
            .protected_regions
            .is_empty(),
        "Undo did not restore pre-protection state"
    );
    let redo = client_project(client.execute(
        Command::Redo,
        Some(project.clone()),
        Some(RevisionId::new(7)),
    )?)?;
    ensure!(
        redo.revision == RevisionId::new(8),
        "Redo did not create a fresh revision"
    );
    check_motion_program(&redo.program, 2)?;
    ensure!(
        serde_json::to_value(import_diagnostics(&mut client, &project)?.protected_regions)?
            == serde_json::to_value(protected())?,
        "Redo lost original protection"
    );
    let old_pending = &source
        .expected
        .candidates
        .iter()
        .find(|(_, _, committed, _)| committed.is_none())
        .unwrap()
        .0;
    let pending = mapping[old_pending.as_str()].clone();
    rejected(
        client.execute(
            Command::CommitCandidate {
                candidate_id: pending.clone(),
            },
            Some(project.clone()),
            Some(RevisionId::new(8)),
        ),
        &[ErrorCode::RevisionConflict],
    )?;
    let rebased = candidate_response(client.execute(
        Command::RebaseCandidate {
            candidate_id: pending,
        },
        Some(project.clone()),
        Some(RevisionId::new(8)),
    )?)?;
    ensure!(
        rebased.base_revision == RevisionId::new(8),
        "explicit rebase did not bind current revision"
    );
    commit(&mut client, &project, 8, &rebased.candidate_id)?;
    check_motion_program(&current(&mut client, &project)?.program, 3)?;
    let outside = client.upload_edit(
        project.clone(),
        RevisionId::new(9),
        &motion(4),
        "new local outside-protection edit",
    )?;
    commit(&mut client, &project, 9, &outside.candidate_id)?;
    check_motion_program(&current(&mut client, &project)?.program, 4)?;
    let inside = client.upload_edit(
        project.clone(),
        RevisionId::new(10),
        &motion(5),
        "must not cross protected region",
    )?;
    rejected(
        client.execute(
            Command::CommitCandidate {
                candidate_id: inside.candidate_id,
            },
            Some(project.clone()),
            Some(RevisionId::new(10)),
        ),
        &[ErrorCode::Forbidden],
    )?;
    ensure!(
        current(&mut client, &project)?.revision == RevisionId::new(10),
        "rejected protected edit changed revision"
    );
    check_motion_program(&current(&mut client, &project)?.program, 4)?;
    source_engine.completed = true;
    f.completed = true;
    Ok(())
}

#[test]
fn project_package_import_prepared_handle_does_not_follow_replaced_path() -> Result<()> {
    let mut f = Fixture::new()?;
    let source_actor = f.pair("source-owner")?;
    let source = build_archive(&f, &mut source_actor.client(&f)?)?;
    let actor = f.pair("prepared-handle-owner")?;
    let mut client = actor.client(&f)?;
    let control = PackageImportControl::default();
    let mut prepared = PreparedPackageImport::open(&source.path, &control)?;
    fs::rename(&source.path, f.root.join("original-handle-held.pulsar"))?;
    fs::write(
        &source.path,
        b"REPLACEMENT PATH IS NOT THE PREPARED PACKAGE",
    )?;
    let started = client.start_project_import(&prepared, request_id())?;
    let completed = client.continue_project_import(
        &mut prepared,
        started.operation_id,
        request_id(),
        request_id(),
        &control,
    )?;
    let receipt = complete_receipt(&completed)?;
    ensure!(
        receipt.package.sha256 == source.artifact.sha256,
        "shared helper followed mutable path replacement"
    );
    let (_, bundle, _) = reexport(
        &f,
        &mut client,
        &receipt.project_id,
        6,
        "prepared-handle-clone",
    )?;
    let mapping = check_import_graph(&bundle, &source, &receipt.project_id)?;
    assert_origin_namespace(&bundle, &source, &mapping)?;
    f.completed = true;
    Ok(())
}

#[test]
fn project_package_import_reexports_mixed_local_history_into_a_second_fresh_clone() -> Result<()> {
    let mut f = Fixture::new()?;
    let source_actor = f.pair("original-author")?;
    let source = build_archive(&f, &mut source_actor.client(&f)?)?;
    let actor_a = f.pair("first-clone-author")?;
    let imported_a = shared_import(&f, &actor_a, &source.path)?;
    let project_a = complete_receipt(&imported_a)?.project_id.clone();
    let mut a = actor_a.client(&f)?;
    let (_, initial_a, _) = reexport(&f, &mut a, &project_a, 6, "first-clone-before-local-edit")?;
    let map_a = check_import_graph(&initial_a, &source, &project_a)?;
    assert_origin_namespace(&initial_a, &source, &map_a)?;
    let local = a.upload_edit(
        project_a.clone(),
        RevisionId::new(6),
        &motion(3),
        "genuine new edit between imports",
    )?;
    commit(&mut a, &project_a, 6, &local.candidate_id)?;
    let (artifact, checked, path) =
        reexport(&f, &mut a, &project_a, 7, "mixed-local-and-imported")?;
    ensure!(
        artifact.format_version == 2,
        "mixed imported/local archive did not use v2"
    );
    let local_rows = checked.manifest["authored_lineage"]
        .as_array()
        .context("local authored rows missing")?;
    ensure!(
        local_rows.len() == 1 && as_id(&local_rows[0]["candidate_id"])? == local.candidate_id,
        "archived edit proof was promoted or genuine new proof lost"
    );
    let mut candidates = source
        .expected
        .candidates
        .iter()
        .map(|(id, base, committed, shape)| (map_a[id.as_str()].clone(), *base, *committed, *shape))
        .collect::<Vec<_>>();
    candidates.push((local.candidate_id.clone(), 6, Some(7), 3));
    let media = serde_json::from_value(
        checked.manifest["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "media")
            .unwrap()["source_version"]
            .clone(),
    )?;
    let bytes = fs::read(&path)?;
    let mixed = ArchiveFixture {
        expected: ExpectedGraph {
            project: project_a.clone(),
            head_revision: 7,
            media,
            candidates,
            export_hash: source.expected.export_hash.clone(),
        },
        path,
        artifact,
        checked,
        bytes,
    };
    let actor_b = f.pair("second-clone-author")?;
    let before = project_catalog(&f)?;
    let imported_b = shared_import(&f, &actor_b, &mixed.path)?;
    let project_b = complete_receipt(&imported_b)?.project_id.clone();
    ensure!(
        project_b != project_a && project_b != source.expected.project,
        "second clone reused prior project identity"
    );
    exactly_one_new_project(&f, &before, &project_b)?;
    retained_original_container(&f, &mixed)?;
    let mut b = actor_b.client(&f)?;
    rejected(
        a.execute(Command::GetSnapshot, Some(project_b.clone()), None),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    let (_, bundle, _) = reexport(&f, &mut b, &project_b, 7, "second-clone-before-new-edit")?;
    let map_b = check_import_graph(&bundle, &mixed, &project_b)?;
    assert_origin_namespace(&bundle, &mixed, &map_b)?;
    for id in map_b.values() {
        ensure!(
            !source
                .expected
                .candidates
                .iter()
                .any(|(old, _, _, _)| old == id),
            "second clone reused root-origin candidate"
        );
    }
    for (hash, bytes) in &source.checked.small_objects {
        ensure!(
            bundle.small_objects.get(hash) == Some(bytes),
            "second hop lost a transitive original object"
        );
    }
    assert_import_trust(&mut b, &project_b)?;
    let next = b.upload_edit(
        project_b.clone(),
        RevisionId::new(7),
        &motion(4),
        "new local edit after second clone",
    )?;
    commit(&mut b, &project_b, 7, &next.candidate_id)?;
    check_motion_program(&current(&mut b, &project_b)?.program, 4)?;
    f.completed = true;
    Ok(())
}

fn mutate_manifest_digest_value(bytes: &[u8], old: &str) -> Result<Vec<u8>> {
    let mut output = bytes.to_vec();
    let length = u64::from_le_bytes(bytes[12..20].try_into()?) as usize;
    let json = &bytes[52..52 + length];
    let matches = json
        .windows(old.len())
        .enumerate()
        .filter(|(_, value)| *value == old.as_bytes())
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    ensure!(
        old.len() == 64 && matches.len() == 1,
        "ancestor digest mutation is not uniquely targeted"
    );
    let index = 52 + matches[0];
    output[index] = if output[index] == b'a' { b'b' } else { b'a' };
    let digest = Sha256::digest(&output[52..52 + length]);
    output[20..52].copy_from_slice(&digest);
    Ok(output)
}
fn import_malformed(
    actor: &Actor,
    f: &Fixture,
    bytes: &[u8],
    before: &std::collections::BTreeSet<String>,
    label: &str,
) -> Result<()> {
    let (_, status) = raw_start(actor, f, bytes)?;
    no_new_projects(f, before)?;
    let lease = begin_upload(actor, f, &status.operation_id)?;
    send_all(actor, &lease, bytes)?;
    import_status(f.call(&seal_request(actor, &lease))?)?;
    let failed = terminal_import(&mut actor.client(f)?, &status.operation_id)?;
    ensure!(
        failed.state == PackageImportOperationState::Failed && failed.receipt.is_none(),
        "malformed {label} published or did not fail: {:?}",
        failed.state
    );
    ensure!(
        failed.error.as_ref().is_some_and(|e| matches!(
            e.code,
            ErrorCode::InvalidRequest | ErrorCode::DependencyMismatch | ErrorCode::Unsupported
        )),
        "malformed {label} failed for unrelated reason: {:?}",
        failed.error
    );
    no_new_projects(f, before)?;
    Ok(())
}
#[test]
fn project_package_import_rejects_inner_corruption_even_when_objects_are_cached() -> Result<()> {
    let mut f = Fixture::new()?;
    let owner = f.pair("source-owner")?;
    let source = build_archive(&f, &mut owner.client(&f)?)?;
    let actor = f.pair("malformed-import-owner")?;
    let before = project_catalog(&f)?;
    let mut cases = Vec::new();
    let mut bad = source.bytes.clone();
    bad[0] ^= 1;
    cases.push(("bad magic", bad));
    let mut bad = source.bytes.clone();
    bad[8] = 99;
    cases.push(("unknown header version", bad));
    let mut bad = source.bytes.clone();
    bad[20] ^= 1;
    cases.push(("manifest digest", bad));
    let mut bad = source.bytes.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    cases.push(("cached object payload digest", bad));
    let mut bad = source.bytes.clone();
    bad.pop();
    cases.push(("truncated last payload", bad));
    let mut bad = source.bytes.clone();
    bad.push(b'!');
    cases.push(("trailing undeclared byte", bad));
    for (label, bytes) in cases {
        import_malformed(&actor, &f, &bytes, &before, label)
            .with_context(|| format!("malformed case {label}"))?;
    }
    // A valid second run must still consume all bytes despite cache hits.
    let valid = shared_import(&f, &actor, &source.path)?;
    let project = complete_receipt(&valid)?.project_id.clone();
    exactly_one_new_project(&f, &before, &project)?;
    let (_, v2, path) = reexport(
        &f,
        &mut actor.client(&f)?,
        &project,
        6,
        "v2-for-ancestor-corruption",
    )?;
    let origins = v2.manifest["imported_origins"]
        .as_array()
        .context("v2 origins absent")?;
    let ancestor = origins
        .iter()
        .find(|o| o["container"]["sha256"] == source.artifact.sha256)
        .context("origin container missing")?;
    let target = ancestor["container"]["sha256"].as_str().unwrap();
    let bytes = fs::read(path)?;
    let corrupted = mutate_manifest_digest_value(&bytes, target)?;
    let before_v2 = project_catalog(&f)?;
    import_malformed(
        &actor,
        &f,
        &corrupted,
        &before_v2,
        "ancestor container digest",
    )?;
    f.completed = true;
    Ok(())
}

#[test]
fn project_package_import_scopes_uploads_and_recovers_lost_acknowledgements_exactly_once(
) -> Result<()> {
    let mut f = Fixture::new()?;
    let owner = f.pair("source-owner")?;
    let source = build_archive(&f, &mut owner.client(&f)?)?;
    let actor = f.pair("import-owner")?;
    let other = f.pair("other-valid-actor")?;
    let before = project_catalog(&f)?;
    let start = actor.unscoped(Command::StartProjectImport {
        package: declaration(&source.bytes),
    });
    let lost = f.lost_ack(start.clone())?;
    let admitted = import_status(lost.result?)?;
    let replay = import_status(f.call(&start)?)?;
    ensure!(
        admitted.operation_id == replay.operation_id,
        "lost Start acknowledgement created another operation"
    );
    no_new_projects(&f, &before)?;
    let mut changed = start.clone();
    if let Command::StartProjectImport { package } = &mut changed.command {
        package.sha256 = "a".repeat(64);
    }
    rejected(f.call(&changed), &[ErrorCode::RequestConflict])?;
    rejected(
        f.call(&other.unscoped(Command::ProjectImportStatus {
            operation_id: admitted.operation_id.clone(),
        })),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    rejected(
        f.call(&other.unscoped(Command::BeginProjectPackageUpload {
            operation_id: admitted.operation_id.clone(),
        })),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    rejected(
        f.call(&other.unscoped(Command::CancelProjectImport {
            operation_id: admitted.operation_id.clone(),
        })),
        &[ErrorCode::Forbidden, ErrorCode::NotFound],
    )?;
    let begin = actor.unscoped(Command::BeginProjectPackageUpload {
        operation_id: admitted.operation_id.clone(),
    });
    let lost = f.lost_ack(begin.clone())?;
    let lease = upload_lease(lost.result?)?;
    let retried = upload_lease(f.call(&begin)?)?;
    ensure!(
        retried.lease_id == lease.lease_id && retried.generation == lease.generation,
        "lost Begin acknowledgement created another generation"
    );
    rejected(
        upload_client(&other, &lease),
        &[
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::Unauthorized,
        ],
    )?;
    let n = (source.bytes.len() / 2).min(4096);
    let mut bulk = upload_client(&actor, &lease)?;
    let first = bulk.upload(0, &source.bytes[..n])?;
    drop(bulk);
    let status = upload_lease(f.call(&actor.unscoped(Command::ProjectPackageUploadStatus {
        lease_id: lease.lease_id.clone(),
    }))?)?;
    ensure!(
        status.next_offset == n as u64 && status.replay.as_ref() == Some(&first),
        "disconnect lost accepted prefix"
    );
    let mut conflict = source.bytes[..n].to_vec();
    conflict[0] ^= 1;
    for (offset, bytes) in [(0, conflict.as_slice()), (n as u64 + 1, &source.bytes[..1])] {
        match raw_chunk_reply(&actor, &lease, offset, bytes)? {
            PackageUploadReply::Error(error) => ensure!(
                error.code == ErrorCode::InvalidRequest,
                "invalid raw chunk failed for unrelated reason"
            ),
            _ => bail!("engine accepted conflicting replay or noncontiguous raw chunk"),
        }
    }
    let unchanged =
        upload_lease(f.call(&actor.unscoped(Command::ProjectPackageUploadStatus {
            lease_id: lease.lease_id.clone(),
        }))?)?;
    ensure!(
        unchanged.next_offset == n as u64 && unchanged.generation == lease.generation,
        "invalid chunk changed admitted cursor/generation"
    );
    let mut resumed = upload_client(&actor, &lease)?;
    ensure!(
        resumed.upload(0, &source.bytes[..n])? == first,
        "exact most-recent payload retry changed receipt"
    );
    let mut offset = n;
    for chunk in source.bytes[n..].chunks(MAX_PACKAGE_UPLOAD_CHUNK_BYTES as usize) {
        resumed.upload(offset as u64, chunk)?;
        offset += chunk.len();
    }
    drop(resumed);
    ensure!(
        offset == source.bytes.len(),
        "test did not upload all bytes"
    );
    let seal = seal_request(&actor, &lease);
    let lost = f.lost_ack(seal.clone())?;
    let sealing = import_status(lost.result?)?;
    let repeated = import_status(f.call(&seal)?)?;
    ensure!(
        sealing.operation_id == admitted.operation_id
            && repeated.operation_id == admitted.operation_id,
        "lost Seal acknowledgement created a second import"
    );
    let completed = terminal_import(&mut actor.client(&f)?, &admitted.operation_id)?;
    let project = complete_receipt(&completed)?.project_id.clone();
    exactly_one_new_project(&f, &before, &project)?;
    f.restart()?;
    for request in [&start, &seal] {
        let replay = import_status(f.call(request)?)?;
        ensure!(
            replay.operation_id == admitted.operation_id
                && complete_receipt(&replay)?.project_id == project,
            "completed replay after restart changed publication"
        );
    }
    exactly_one_new_project(&f, &before, &project)?;
    ensure!(
        f.max_control.load(Ordering::Relaxed) <= MAX_CONTROL_BYTES as u64,
        "control response exceeded bound"
    );
    f.completed = true;
    Ok(())
}

#[test]
fn project_package_import_cancellation_and_restart_do_not_publish_partial_projects() -> Result<()> {
    let mut f = Fixture::new()?;
    let owner = f.pair("source-owner")?;
    let source = build_archive(&f, &mut owner.client(&f)?)?;
    let actor = f.pair("interruptible-import-owner")?;
    let before = project_catalog(&f)?;
    let (_, cancelled_start) = raw_start(&actor, &f, &source.bytes)?;
    let cancelled_lease = begin_upload(&actor, &f, &cancelled_start.operation_id)?;
    let n = (source.bytes.len() / 2).min(4096);
    let mut bulk = upload_client(&actor, &cancelled_lease)?;
    bulk.upload(0, &source.bytes[..n])?;
    let cancelled = actor
        .client(&f)?
        .cancel_project_import(cancelled_start.operation_id.clone())?;
    ensure!(
        cancelled.state == PackageImportOperationState::Cancelled && cancelled.receipt.is_none(),
        "active cancellation published"
    );
    ensure!(
        bulk.upload(
            n as u64,
            &source.bytes[n..(n + 4096).min(source.bytes.len())]
        )
        .is_err(),
        "cancelled upload kept accepting bytes"
    );
    no_new_projects(&f, &before)?;
    let (start, active) = raw_start(&actor, &f, &source.bytes)?;
    let old = begin_upload(&actor, &f, &active.operation_id)?;
    let mut bulk = upload_client(&actor, &old)?;
    bulk.upload(0, &source.bytes[..n])?;
    drop(bulk);
    let partial = actor
        .client(&f)?
        .project_import_status(active.operation_id.clone())?;
    ensure!(
        partial.state == PackageImportOperationState::Uploading
            && partial.progress.received_bytes == n as u64,
        "partial upload checkpoint was not observed"
    );
    no_new_projects(&f, &before)?;
    f.restart()?;
    let interrupted = actor
        .client(&f)?
        .project_import_status(active.operation_id.clone())?;
    ensure!(
        interrupted.state == PackageImportOperationState::Interrupted
            && interrupted.receipt.is_none(),
        "unfinished restart silently resumed or published"
    );
    let replay = import_status(f.call(&start)?)?;
    ensure!(
        replay.operation_id == active.operation_id
            && replay.state == PackageImportOperationState::Interrupted,
        "old Start replay manufactured work"
    );
    ensure!(
        upload_client(&actor, &old).is_err(),
        "old epoch upload became valid again"
    );
    no_new_projects(&f, &before)?;
    let fresh = shared_import(&f, &actor, &source.path)?;
    ensure!(
        fresh.operation_id != active.operation_id,
        "fresh Start reused interrupted operation"
    );
    exactly_one_new_project(&f, &before, &complete_receipt(&fresh)?.project_id)?;
    f.completed = true;
    Ok(())
}

mod qualification {
    use super::*;
    use std::sync::{atomic::AtomicBool, Arc};

    const LARGE_SOURCE_BYTES: u64 = 1024 * 1024 * 1024 + 256 * 1024;
    const RSS_DELTA_TRIPWIRE: u64 = 256 * 1024 * 1024;

    fn indexed_source(path: &Path, length: u64) -> Result<String> {
        ensure!(
            length % BLOCK as u64 == 0,
            "indexed sparse fixture requires whole blocks"
        );
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.set_len(length)?;
        let mut buffer = [0u8; BLOCK];
        let mut digest = Sha256::new();
        for index in 0..length / BLOCK as u64 {
            buffer[..8].copy_from_slice(&index.to_le_bytes());
            buffer[8..16].copy_from_slice(&(!index).to_le_bytes());
            buffer[16..24].copy_from_slice(&length.to_le_bytes());
            buffer[24..32].copy_from_slice(b"P1CIMPRT");
            file.seek(SeekFrom::Start(index * BLOCK as u64))?;
            file.write_all(&buffer[..32])?;
            // Hash every logical byte, including every zero-filled sparse extent.
            digest.update(buffer);
        }
        file.sync_all()?;
        Ok(format!("{:x}", digest.finalize()))
    }
    fn rss(pid: u32) -> Result<u64> {
        let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
        let kib = status
            .lines()
            .find(|line| line.starts_with("VmRSS:"))
            .context("RSS measurement unavailable")?
            .split_whitespace()
            .nth(1)
            .context("RSS value absent")?
            .parse::<u64>()?;
        kib.checked_mul(1024).context("RSS byte overflow")
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
            ensure!(samples > 1, "RSS observation window not sampled");
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
    #[derive(Default)]
    struct ControlReport {
        uploading: u64,
        validating: u64,
        other_active: u64,
        max_nanos: u128,
        max_frame: usize,
    }
    struct ControlSampler {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<Result<ControlReport>>>,
    }
    impl ControlSampler {
        fn start(
            f: &Fixture,
            actor: &Actor,
            operation: PackageImportOperationId,
            probe: ProjectId,
        ) -> Self {
            let endpoint = f.endpoint();
            let session = actor.session.clone();
            let auth_token = actor.auth_token.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let done = stop.clone();
            let thread = std::thread::spawn(move || -> Result<ControlReport> {
                let mut report = ControlReport::default();
                while !done.load(Ordering::Relaxed) {
                    let request = Request::new(
                        request_id(),
                        Command::ProjectImportStatus {
                            operation_id: operation.clone(),
                        },
                    )
                    .with_session(session.clone())
                    .with_auth_token(auth_token.clone());
                    let response =
                        LocalClient::connect(&endpoint, Duration::from_secs(2))?.call(&request)?;
                    report.max_frame = report
                        .max_frame
                        .max(serde_json::to_vec(&request)?.len())
                        .max(serde_json::to_vec(&response)?.len());
                    let status = import_status(response.result?)?;
                    let active = match status.state {
                        PackageImportOperationState::Uploading
                            if status.progress.received_bytes > 0 =>
                        {
                            report.uploading += 1;
                            true
                        }
                        PackageImportOperationState::Validating => {
                            report.validating += 1;
                            true
                        }
                        PackageImportOperationState::Sealing
                        | PackageImportOperationState::Sealed
                        | PackageImportOperationState::Publishing => {
                            report.other_active += 1;
                            true
                        }
                        _ => false,
                    };
                    if active {
                        let mut request = Request::new(request_id(), Command::GetSnapshot)
                            .with_session(session.clone())
                            .with_auth_token(auth_token.clone());
                        request.project = Some(probe.clone());
                        let started = Instant::now();
                        let response = LocalClient::connect(&endpoint, Duration::from_secs(2))?
                            .call(&request)?;
                        report.max_nanos = report.max_nanos.max(started.elapsed().as_nanos());
                        report.max_frame = report
                            .max_frame
                            .max(serde_json::to_vec(&request)?.len())
                            .max(serde_json::to_vec(&response)?.len());
                        match response.result? {
                            ResponseBody::Project(snapshot) => ensure!(
                                snapshot.project_id == probe
                                    && snapshot.revision == RevisionId::new(0),
                                "concurrent control snapshot changed"
                            ),
                            _ => bail!("concurrent snapshot response changed"),
                        }
                    }
                    ensure!(
                        report.max_frame <= MAX_CONTROL_BYTES,
                        "control exceeded framing limit"
                    );
                    if status.state.is_terminal() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(report)
            });
            Self {
                stop,
                thread: Some(thread),
            }
        }
        fn finish(mut self) -> Result<ControlReport> {
            self.stop.store(true, Ordering::Relaxed);
            self.thread
                .take()
                .unwrap()
                .join()
                .map_err(|_| anyhow::anyhow!("control sampler panicked"))?
        }
    }
    impl Drop for ControlSampler {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
    fn check_large_bundle(
        bundle: &CheckedBundle,
        project: &ProjectId,
        source_hash: &str,
    ) -> Result<()> {
        ensure!(
            bundle.manifest["origin_project_id"] == serde_json::to_value(project)?
                && bundle.manifest["captured_revision"] == 1,
            "large project/revision identity changed"
        );
        check_motion(
            checked_object(bundle, &bundle.manifest["head"]["motion"])?,
            0,
        )?;
        let sources = bundle.manifest["sources"]
            .as_array()
            .context("large source catalog absent")?;
        ensure!(
            sources.len() == 1
                && sources[0]["kind"] == "media"
                && sources[0]["identity"]["sha256"] == source_hash
                && sources[0]["identity"]["byte_len"] == LARGE_SOURCE_BYTES,
            "large generated source identity changed"
        );
        ensure!(
            sources[0]["pinned"] == true && sources[0]["evicted"] == false,
            "large source pin/availability changed"
        );
        ensure!(
            bundle.manifest["revisions"]
                .as_array()
                .context("large revisions absent")?
                .len()
                == 2,
            "large import lost base revision"
        );
        Ok(())
    }

    #[test]
    #[ignore = "explicit >1 GiB generated-data import streaming/RSS/control qualification; private temporary disk"]
    fn project_package_import_streams_more_than_one_gib_with_bounded_rss_and_control() -> Result<()>
    {
        let mut source = Fixture::new()?;
        let source_actor = source.pair("large synthetic source")?;
        let mut owner = source_actor.client(&source)?;
        let project = client_project(owner.execute(
            Command::CreateProject {
                name: "P1c large synthetic import".into(),
            },
            None,
            None,
        )?)?
        .project_id;
        let source_path = source.root.join("indexed-source.bin");
        let source_hash = indexed_source(&source_path, LARGE_SOURCE_BYTES)?;
        let source_id = match owner.execute(
            Command::ImportSource { path: source_path },
            Some(project.clone()),
            None,
        )? {
            ClientResponse::Source { source_version } => source_version,
            _ => bail!("large source not imported"),
        };
        owner.execute(
            Command::PinSource {
                source_version: source_id,
                pinned: true,
            },
            Some(project.clone()),
            None,
        )?;
        let candidate = owner.upload_edit(
            project.clone(),
            RevisionId::new(0),
            &motion(0),
            "large source shallow motion",
        )?;
        commit(&mut owner, &project, 0, &candidate.candidate_id)?;
        let source_archive = source.root.join("large-source.pulsar");
        let export_started = Instant::now();
        let artifact = export_project(&mut owner, &project, 1, &source_archive)?;
        let export_seconds = export_started.elapsed().as_secs_f64();
        ensure!(
            artifact.byte_len > LARGE_SOURCE_BYTES
                && artifact.byte_len.div_ceil(MAX_ARTIFACT_CHUNK_BYTES as u64) > 4096,
            "large fixture did not cross old chunk boundary"
        );
        let checked = inspect_bundle(&source_archive, &artifact)?;
        check_large_bundle(&checked, &project, &source_hash)?;
        ensure!(
            checked.bytes == artifact.byte_len && checked.sha256 == artifact.sha256,
            "independent source container hash changed"
        );
        let original_manifest_length = {
            let mut file = File::open(&source_archive)?;
            file.seek(SeekFrom::Start(12))?;
            let mut length = [0; 8];
            file.read_exact(&mut length)?;
            u64::from_le_bytes(length)
        };
        let mut destination = Fixture::new()?;
        let incoming = destination.root.join("relocated-large.pulsar");
        fs::rename(&source_archive, &incoming)?;
        drop(owner);
        source.completed = true;
        drop(source); // Removes only our original engine/snapshots before import.
        let actor = destination.pair("large fresh destination")?;
        let mut client = actor.client(&destination)?;
        let probe = client_project(client.execute(
            Command::CreateProject {
                name: "control responsiveness probe".into(),
            },
            None,
            None,
        )?)?
        .project_id;
        let before = project_catalog(&destination)?;
        let sampler = RssSampler::start(destination.child.as_ref().unwrap().id())?;
        let control = PackageImportControl::default();
        let hash_started = Instant::now();
        let mut prepared = PreparedPackageImport::open(&incoming, &control)?;
        let hash_seconds = hash_started.elapsed().as_secs_f64();
        ensure!(
            prepared.declaration().sha256 == artifact.sha256
                && control.hashed_bytes() == artifact.byte_len,
            "large preparation skipped declared bytes"
        );
        let started = client.start_project_import(&prepared, request_id())?;
        no_new_projects(&destination, &before)?;
        let responsiveness =
            ControlSampler::start(&destination, &actor, started.operation_id.clone(), probe);
        let import_started = Instant::now();
        let completed = client.continue_project_import(
            &mut prepared,
            started.operation_id,
            request_id(),
            request_id(),
            &control,
        )?;
        let import_seconds = import_started.elapsed().as_secs_f64();
        let receipt = complete_receipt(&completed)?;
        let imported = receipt.project_id.clone();
        let control_report = responsiveness.finish()?;
        let (engine_delta, client_delta, samples) = sampler.finish()?;
        ensure!(
            control_report.uploading > 0 && control_report.validating > 0,
            "large qualification did not observe both upload and validation; no early-Ready skip"
        );
        ensure!(
            control_report.max_nanos < 2_000_000_000,
            "active import blocked control beyond probe deadline"
        );
        ensure!(
            engine_delta < RSS_DELTA_TRIPWIRE && client_delta < RSS_DELTA_TRIPWIRE,
            "sampled RSS regression: engine {engine_delta}, client {client_delta}"
        );
        ensure!(
            control.accepted_bytes() == artifact.byte_len
                && control.verified_bytes() == artifact.byte_len,
            "large importer skipped full bytes"
        );
        exactly_one_new_project(&destination, &before, &imported)?;
        check_motion_program(&current(&mut client, &imported)?.program, 0)?;
        let recovery = destination
            .state
            .join("imported-project-containers")
            .join(&artifact.sha256);
        ensure!(
            fs::metadata(&recovery)?.len() == artifact.byte_len
                && digest_file(&recovery)? == artifact.sha256,
            "retained full original container changed"
        );
        ensure!(
            fs::metadata(&recovery)?.permissions().mode() & 0o222 == 0,
            "retained original is writable"
        );
        let reexport_started = Instant::now();
        let (_, second, _) = reexport(
            &destination,
            &mut client,
            &imported,
            1,
            "large-clone-export",
        )?;
        let reexport_seconds = reexport_started.elapsed().as_secs_f64();
        check_large_bundle(&second, &imported, &source_hash)?;
        let origins = second.manifest["imported_origins"]
            .as_array()
            .context("large origin map absent")?;
        ensure!(
            origins.len() == 1 && origins[0]["container"] == serde_json::to_value(&artifact)?,
            "large original container receipt changed"
        );
        ensure!(
            origins[0]["manifest"]["sha256"] == artifact.manifest_sha256
                && origins[0]["manifest"]["byte_len"] == original_manifest_length,
            "large original manifest identity changed"
        );
        ensure!(
            origins[0]["objects"] == checked.manifest["objects"],
            "large original catalog changed"
        );
        eprintln!("P1c LARGE: source_bytes={} package_bytes={} minimum_256k_chunks={} export_seconds={:.3} prepared_hash_seconds={:.3} import_seconds={:.3} reexport_seconds={:.3} sampled_engine_rss_delta={} sampled_client_rss_delta={} rss_samples={} upload_control_probes={} validation_control_probes={} other_active_probes={} max_get_snapshot_ms={:.3} max_observed_control_bytes={} disk=private_generated_sparse_source_removed_before_import+upload+source_CAS+original_container+reexport",
            LARGE_SOURCE_BYTES,artifact.byte_len,artifact.byte_len.div_ceil(MAX_ARTIFACT_CHUNK_BYTES as u64),export_seconds,hash_seconds,import_seconds,reexport_seconds,engine_delta,client_delta,samples,control_report.uploading,control_report.validating,control_report.other_active,control_report.max_nanos as f64/1_000_000.0,control_report.max_frame);
        // These are sampled host-specific tripwires, not hard heap bounds,
        // minimum-hardware qualification, real-media speed, or GUI/P1d parity.
        destination.completed = true;
        Ok(())
    }

    fn no_live_execution(f: &Fixture) -> Result<()> {
        let connection = rusqlite::Connection::open_with_flags(
            f.state.join("projects.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        let transaction = connection.unchecked_transaction()?;
        for table in ["jobs", "attempts"] {
            let count: i64 =
                transaction.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            ensure!(count == 0, "archive import activated live {table}");
        }
        transaction.commit()?;
        Ok(())
    }
    fn hold_oracle(program: &MotionProgram) -> Result<()> {
        let track = program.track(Axis::Stroke).context("hold stroke missing")?;
        ensure!(
            program.tracks().len() == 1 && track.actions().len() == 2,
            "hold axes/actions changed"
        );
        for (index, action) in track.actions().iter().enumerate() {
            ensure!(
                action.time().as_nanos() == index as i64 * 2_000_000_000
                    && action.position().value().to_bits() == 0.42f64.to_bits()
                    && action.evidence() == EvidenceKind::Synthesized,
                "literal constrained hold oracle changed"
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires PULSAR_PACKAGE_TEST_WORKER and qualified Linux worker confinement"]
    fn project_package_import_preserves_actual_worker_origins_without_job_authority() -> Result<()>
    {
        let worker = std::env::var_os("PULSAR_PACKAGE_TEST_WORKER")
            .context("explicit worker qualification requires PULSAR_PACKAGE_TEST_WORKER")?;
        ensure!(
            fs::canonicalize(worker)? == fs::canonicalize(env!("CARGO_BIN_EXE_pulsar"))?,
            "qualification worker differs from actual engine executable"
        );
        let mut source = Fixture::new()?;
        let source_actor = source.pair("actual constrained worker source")?;
        let mut owner = source_actor.client(&source)?;
        let project = client_project(owner.execute(
            Command::CreateProject {
                name: "P1c actual worker origin".into(),
            },
            None,
            None,
        )?)?
        .project_id;
        let input = GenerationInput::Text {
            prompt: "pattern=hold axis=stroke duration=2s frequency=0hz amplitude=0 offset=0.42"
                .into(),
        };
        let input_bytes = serde_json::to_vec(&input)?;
        let initial = match owner.execute(
            Command::GenerateInput { input },
            Some(project.clone()),
            Some(RevisionId::new(0)),
        )? {
            ClientResponse::Job(job) => job,
            _ => bail!("actual worker admission returned no job"),
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        let completed_job = loop {
            let job = match owner.execute(
                Command::JobStatus {
                    job_id: initial.job_id.clone(),
                },
                Some(project.clone()),
                None,
            )? {
                ClientResponse::Job(job) => job,
                _ => bail!("job status response changed"),
            };
            match job.state.as_str() {
                "completed" => break job,
                "failed" | "cancelled" | "interrupted" => bail!(
                    "actual confined worker failed qualification: {} {:?}",
                    job.state,
                    job.error
                ),
                _ => {}
            }
            ensure!(Instant::now() < deadline, "actual worker timed out");
            std::thread::sleep(Duration::from_millis(20));
        };
        let original_candidate = completed_job
            .candidate_id
            .context("completed actual worker lacks candidate")?;
        let original = candidate_response(owner.execute(
            Command::GetCandidate {
                candidate_id: original_candidate.clone(),
            },
            Some(project.clone()),
            None,
        )?)?;
        hold_oracle(&original.program)?;
        let path = source.root.join("actual-worker-origin.pulsar");
        let artifact = export_project(&mut owner, &project, 0, &path)?;
        let checked = inspect_bundle(&path, &artifact)?;
        let bytes = fs::read(&path)?;
        ensure!(
            bytes.len() < 2 * 1024 * 1024,
            "small actual worker fixture unexpectedly large"
        );
        let m = &checked.manifest;
        let sources = m["sources"]
            .as_array()
            .context("worker source catalog absent")?;
        ensure!(
            sources.len() == 1
                && sources[0]["kind"] == "generation_input"
                && checked_object(&checked, &sources[0]["identity"])? == input_bytes,
            "actual generated input identity changed"
        );
        let origins = m["generated_origins"]
            .as_array()
            .context("worker origin absent")?;
        ensure!(origins.len() == 1, "actual worker origin count changed");
        let origin = &origins[0];
        ensure!(
            origin["job_id"] == serde_json::to_value(&initial.job_id)?
                && origin["attempt_id"] == serde_json::to_value(&initial.attempt_id)?
                && origin["state"] == "completed",
            "actual worker job/attempt association changed"
        );
        let raw_program: MotionProgram =
            serde_json::from_slice(checked_object(&checked, &origin["program"])?)?;
        hold_oracle(&raw_program)?;
        let receipt: Value = serde_json::from_slice(checked_object(&checked, &origin["receipt"])?)?;
        ensure!(
            receipt["qualification"] == "unqualified"
                && receipt["program"]["sha256"] == origin["program"]["sha256"],
            "actual raw receipt was misqualified or rebound"
        );
        let mut destination = Fixture::new()?;
        let actor = destination.pair("fresh inert worker-origin destination")?;
        let incoming = destination.root.join("explicit-worker-origin.pulsar");
        fs::write(&incoming, &bytes)?;
        source.stop()?;
        no_live_execution(&destination)?;
        let imported_status = shared_import(&destination, &actor, &incoming)?;
        let imported = complete_receipt(&imported_status)?.project_id.clone();
        let mut client = actor.client(&destination)?;
        let snapshot = current(&mut client, &imported)?;
        ensure!(
            snapshot.revision == RevisionId::new(0) && snapshot.program.tracks().is_empty(),
            "archive import activated uncommitted worker proposal"
        );
        no_live_execution(&destination)?;
        rejected(
            client.execute(
                Command::JobStatus {
                    job_id: initial.job_id.clone(),
                },
                Some(imported.clone()),
                None,
            ),
            &[ErrorCode::NotFound, ErrorCode::Forbidden],
        )?;
        let (_, clone, _) = reexport(
            &destination,
            &mut client,
            &imported,
            0,
            "inert-worker-origin-clone",
        )?;
        ensure!(
            clone.manifest["generated_origins"]
                .as_array()
                .context("clone generated origins absent")?
                .is_empty()
                && clone.manifest["authored_lineage"]
                    .as_array()
                    .context("clone authored lineage absent")?
                    .is_empty(),
            "archive origin became locally executable/trusted"
        );
        let candidate_rows = clone.manifest["candidates"]
            .as_array()
            .context("clone candidates absent")?;
        ensure!(
            candidate_rows.len() == 1
                && candidate_rows[0]["job_origin"].is_null()
                && candidate_rows[0]["committed_revision"].is_null(),
            "imported worker proposal authority changed"
        );
        let candidate = as_id(&candidate_rows[0]["candidate_id"])?;
        ensure!(
            candidate != original_candidate,
            "worker candidate ID was not fresh"
        );
        let held: MotionProgram =
            serde_json::from_slice(checked_object(&clone, &candidate_rows[0]["motion"])?)?;
        hold_oracle(&held)?;
        let ancestor = clone.manifest["imported_origins"]
            .as_array()
            .context("worker archive namespace absent")?
            .iter()
            .find(|o| o["container"]["sha256"] == artifact.sha256)
            .context("worker ancestor missing")?;
        let length = u64::from_le_bytes(bytes[12..20].try_into()?) as usize;
        ensure!(
            checked_object(&clone, &ancestor["manifest"])? == &bytes[52..52 + length]
                && ancestor["objects"] == m["objects"],
            "exact worker origin manifest/catalog changed"
        );
        for (hash, original) in &checked.small_objects {
            ensure!(
                clone.small_objects.get(hash) == Some(original),
                "exact worker receipt/program/input bytes lost"
            );
        }
        assert_import_trust(&mut client, &imported)?;
        commit(&mut client, &imported, 0, &candidate)?;
        hold_oracle(&current(&mut client, &imported)?.program)?;
        let undone = client_project(client.execute(
            Command::Undo,
            Some(imported.clone()),
            Some(RevisionId::new(1)),
        )?)?;
        ensure!(
            undone.revision == RevisionId::new(2) && undone.program.tracks().is_empty(),
            "worker proposal Undo changed"
        );
        let redone = client_project(client.execute(
            Command::Redo,
            Some(imported.clone()),
            Some(RevisionId::new(2)),
        )?)?;
        ensure!(
            redone.revision == RevisionId::new(3),
            "worker proposal Redo reused revision"
        );
        hold_oracle(&redone.program)?;
        no_live_execution(&destination)?;
        eprintln!("P1c ACTUAL WORKER: source_kind=generation_input backend=constrained_text qualification=unqualified fresh_import_jobs=0 fresh_import_attempts=0 raw_origin_bytes=preserved candidate=uncommitted_until_explicit_commit undo_redo=fresh_revisions no_gpu_or_device_actuation");
        source.completed = true;
        destination.completed = true;
        Ok(())
    }
}

mod connection_contention {
    use super::*;
    use std::net::Shutdown;
    use std::sync::{atomic::AtomicBool, mpsc, Arc};

    struct BusyProxy {
        public: PathBuf,
        upstream: PathBuf,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<Result<(u64, u64)>>>,
        busy: mpsc::Receiver<()>,
    }
    struct RecordedHeader<'a> {
        stream: &'a mut UnixStream,
        bytes: Vec<u8>,
    }
    impl Read for RecordedHeader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let n = self.stream.read(buffer)?;
            if self.bytes.len().saturating_add(n) > 32 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "proxy header exceeds recording budget",
                ));
            }
            self.bytes.extend_from_slice(&buffer[..n]);
            Ok(n)
        }
    }
    fn proxy_connection(
        mut downstream: UnixStream,
        path: &Path,
        busy: &mpsc::Sender<()>,
        expected: &PackageUploadLease,
    ) -> Result<bool> {
        downstream.set_read_timeout(Some(Duration::from_secs(3)))?;
        downstream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut upstream = UnixStream::connect(path)?;
        upstream.set_read_timeout(Some(Duration::from_secs(3)))?;
        upstream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut header = RecordedHeader {
            stream: &mut downstream,
            bytes: Vec::new(),
        };
        let handshake: PackageUploadHandshake = read_bulk_header(&mut header)?;
        upstream.write_all(&header.bytes)?;
        ensure!(
            handshake.lease_id == expected.lease_id
                && handshake.generation == expected.generation
                && handshake.operation_id == expected.operation_id
                && handshake.engine_epoch == expected.engine_epoch
                && handshake.package_sha256 == expected.package.sha256,
            "shared helper changed its busy lease identity"
        );
        let mut header = RecordedHeader {
            stream: &mut upstream,
            bytes: Vec::new(),
        };
        let response: PackageUploadReply = read_bulk_header(&mut header)?;
        downstream.write_all(&header.bytes)?;
        match response {
            PackageUploadReply::Error(error) => {
                ensure!(
                    error.code == ErrorCode::PackageUploadConnectionBusy && error.retryable,
                    "real engine rejected reconnect for non-contention reason: {:?}",
                    error.code
                );
                // This is an actual authenticated engine reply, not an injected error.
                // Only this observation releases the originally held connection.
                let _ = busy.send(());
                Ok(true)
            }
            PackageUploadReply::Ready { lease } => {
                ensure!(
                    lease.lease_id == expected.lease_id
                        && lease.generation == expected.generation
                        && lease.operation_id == expected.operation_id
                        && lease.next_offset == expected.next_offset,
                    "shared helper replaced lease/generation/prefix after contention"
                );
                let mut client_read = downstream.try_clone()?;
                let mut engine_write = upstream.try_clone()?;
                let to_engine = std::thread::spawn(move || -> std::io::Result<u64> {
                    let result = std::io::copy(&mut client_read, &mut engine_write);
                    let _ = engine_write.shutdown(Shutdown::Write);
                    result
                });
                let to_client = std::io::copy(&mut upstream, &mut downstream);
                let _ = downstream.shutdown(Shutdown::Write);
                let copied = to_engine
                    .join()
                    .map_err(|_| anyhow::anyhow!("proxy upload forwarding panicked"))?;
                copied?;
                to_client?;
                Ok(false)
            }
            _ => bail!("real engine returned a non-handshake response"),
        }
    }
    impl BusyProxy {
        fn install(f: &Fixture, lease: &PackageUploadLease) -> Result<Self> {
            let public = lease.bulk_endpoint.clone();
            let upstream = f.root.join("held-upstream.sock");
            ensure!(
                public == f.state.join("bulk.sock"),
                "proxy path is not the fixture engine endpoint"
            );
            fs::rename(&public, &upstream)?;
            let listener = match UnixListener::bind(&public) {
                Ok(listener) => listener,
                Err(error) => {
                    fs::rename(&upstream, &public)?;
                    return Err(error.into());
                }
            };
            fs::set_permissions(&public, fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            let stop = Arc::new(AtomicBool::new(false));
            let done = stop.clone();
            let actual = upstream.clone();
            let expected = lease.clone();
            let (sender, busy) = mpsc::channel();
            let thread = std::thread::spawn(move || -> Result<(u64, u64)> {
                let mut connections = Vec::new();
                while !done.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((downstream, _)) => {
                            ensure!(
                                connections.len() < 32,
                                "shared helper exceeded bounded contention attempts"
                            );
                            let path = actual.clone();
                            let expected = expected.clone();
                            let sender = sender.clone();
                            connections.push(std::thread::spawn(move || {
                                proxy_connection(downstream, &path, &sender, &expected)
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                let (mut rejected, mut admitted) = (0, 0);
                for connection in connections {
                    if connection
                        .join()
                        .map_err(|_| anyhow::anyhow!("proxy connection panicked"))??
                    {
                        rejected += 1;
                    } else {
                        admitted += 1;
                    }
                }
                Ok((rejected, admitted))
            });
            Ok(Self {
                public,
                upstream,
                stop,
                thread: Some(thread),
                busy,
            })
        }
        fn restore(&mut self) -> Result<()> {
            if self.upstream.exists() {
                fs::remove_file(&self.public)?;
                fs::rename(&self.upstream, &self.public)?;
            }
            Ok(())
        }
        fn finish(mut self) -> Result<(u64, u64)> {
            self.stop.store(true, Ordering::Relaxed);
            let outcome = self
                .thread
                .take()
                .unwrap()
                .join()
                .map_err(|_| anyhow::anyhow!("proxy accept loop panicked"))?;
            self.restore()?;
            outcome
        }
    }
    impl Drop for BusyProxy {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            let _ = self.restore();
        }
    }
    #[test]
    fn project_package_import_shared_client_waits_for_actual_connection_retirement() -> Result<()> {
        let mut f = Fixture::new()?;
        let origin = f.pair("contention source")?;
        let source = build_archive(&f, &mut origin.client(&f)?)?;
        let actor = f.pair("contention importing actor")?;
        let before = project_catalog(&f)?;
        let control = PackageImportControl::default();
        let mut prepared = PreparedPackageImport::open(&source.path, &control)?;
        let mut client = actor.client(&f)?;
        let started = client.start_project_import(&prepared, request_id())?;
        let begin = actor.unscoped(Command::BeginProjectPackageUpload {
            operation_id: started.operation_id.clone(),
        });
        let first = upload_lease(f.call(&begin)?)?;
        let mut held = upload_client(&actor, &first)?;
        let prefix = 4096.min(source.bytes.len() / 2);
        held.upload(0, &source.bytes[..prefix])?;
        let lease =
            upload_lease(f.call(&actor.unscoped(Command::ProjectPackageUploadStatus {
                lease_id: first.lease_id.clone(),
            }))?)?;
        ensure!(
            lease.next_offset == prefix as u64,
            "held connection prefix missing"
        );
        let proxy = BusyProxy::install(&f, &lease)?;
        let endpoint = f.endpoint();
        let session = actor.session.clone();
        let token = actor.auth_token.clone();
        let operation = started.operation_id.clone();
        let continuation = std::thread::spawn(move || -> Result<PackageImportStatus> {
            SessionClient::from_credentials(&endpoint, session, token)?.continue_project_import(
                &mut prepared,
                operation,
                begin.request_id,
                request_id(),
                &control,
            )
        });
        let observed = proxy.busy.recv_timeout(Duration::from_secs(3));
        // Release only after the real engine rejected the shared helper. Even a
        // failed barrier releases our connection before joining bounded I/O.
        drop(held);
        let outcome = continuation
            .join()
            .map_err(|_| anyhow::anyhow!("shared continuation panicked"))?;
        let (rejected, admitted) = proxy.finish()?;
        observed.context("shared helper never reached actual occupied-connection guard")?;
        let completed = outcome?;
        let imported = complete_receipt(&completed)?.project_id.clone();
        ensure!(
            completed.operation_id == started.operation_id && rejected >= 1 && admitted == 1,
            "contention silently replaced operation or duplicated admitted upload"
        );
        exactly_one_new_project(&f, &before, &imported)?;
        check_motion_program(&current(&mut client, &imported)?.program, 2)?;
        eprintln!("P1c REAL CONTENTION: observed_typed_busy={} admitted_reconnects={} same_operation=true same_lease=true same_generation=true prefix_preserved={} publication_count=1",rejected,admitted,prefix);
        f.completed = true;
        Ok(())
    }
}
