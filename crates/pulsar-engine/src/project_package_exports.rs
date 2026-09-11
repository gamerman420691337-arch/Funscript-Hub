//! Durable project-package operations. Package grants never imply motion or device authority.
use super::*;
use crate::project_package_format::PackageCodecLimits;
use crate::project_package_storage::{ArtifactHolds, PackageStore, StoredPackage};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

const MAX_ACTIVE: usize = 2;
const MAX_OPERATIONS: i64 = 10_000;
const MAX_DOWNLOAD_BEGINS: i64 = 100_000;
const MAX_DOWNLOADS: usize = 8;
const MAX_OPERATION_DOWNLOADS: usize = 2;
const DOWNLOAD_LIFETIME: Duration = Duration::from_secs(30 * 60);
const MAX_OPERATION_LIFETIME: Duration = Duration::from_secs(60 * 60);
const DOWNLOAD_MEMORY: u64 = 2 * MAX_ARTIFACT_CHUNK_BYTES as u64;

#[cfg(test)]
std::thread_local! {
    static BEFORE_LEASE_ADMISSION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    static FAIL_PACKAGE_SPAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
fn before_lease_admission() {
    BEFORE_LEASE_ADMISSION.with(|slot| {
        let action = slot.borrow_mut().take();
        if let Some(action) = action {
            action();
        }
    });
}
fn spawn_worker(
    task: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(test)]
    if FAIL_PACKAGE_SPAWN.with(|flag| flag.replace(false)) {
        return Err(std::io::Error::other(
            "injected package thread creation failure",
        ));
    }
    std::thread::Builder::new()
        .name("pulsar-package".into())
        .spawn(task)
}

#[derive(Default)]
pub(super) struct PackageRuntime {
    active: HashMap<String, ActiveOperation>,
    downloads: HashMap<String, Arc<Download>>,
    verification: HashMap<String, (u64, Verification)>,
}
#[derive(Clone)]
enum Verification {
    Verified,
    Pending { completed: u64, total: u64 },
    Failed(ProtocolError),
}
struct ActiveOperation {
    session: SessionId,
    project: ProjectId,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    reserved_bytes: u64,
    grant_generation: u64,
}
struct Download {
    id: PackageDownloadId,
    session: SessionId,
    project: ProjectId,
    operation: PackageOperationId,
    epoch: String,
    artifact: PackageArtifactDescriptor,
    offset: u64,
    byte_len: u64,
    deadline: Instant,
    grant_generation: u64,
    cursor: Mutex<DownloadCursor>,
    _holds: ArtifactHolds,
    _memory: Reservation,
}
struct DownloadCursor {
    next: u64,
    replay: Option<PackageChunkRange>,
    connection: Option<String>,
    abandoned: bool,
}
pub(crate) struct PackageConnection {
    download: Arc<Download>,
    nonce: String,
    file: Option<File>,
}
impl Drop for PackageConnection {
    fn drop(&mut self) {
        if let Ok(mut cursor) = self.download.cursor.lock() {
            if cursor.connection.as_deref() == Some(&self.nonce) {
                cursor.connection = None;
            }
        }
    }
}
impl PackageRuntime {
    fn sweep(&mut self) {
        let now = Instant::now();
        for active in self.active.values() {
            if now >= active.deadline {
                active.cancel.store(true, Ordering::Release);
            }
        }
        self.downloads.retain(|_, download| {
            let Ok(mut cursor) = download.cursor.lock() else {
                return true;
            };
            if now >= download.deadline {
                cursor.abandoned = true;
            }
            !cursor.abandoned || cursor.connection.is_some()
        });
    }
    fn invalidate(&mut self, operation: &PackageOperationId) {
        for download in self
            .downloads
            .values()
            .filter(|d| &d.operation == operation)
        {
            if let Ok(mut cursor) = download.cursor.lock() {
                cursor.abandoned = true;
            }
        }
        self.sweep();
    }
}
fn active(state: PackageExportOperationState) -> bool {
    matches!(
        state,
        PackageExportOperationState::Queued
            | PackageExportOperationState::Capturing
            | PackageExportOperationState::Streaming
    )
}
pub(super) fn is_command(command: &Command) -> bool {
    matches!(
        command,
        Command::AllowProjectPackaging { .. }
            | Command::StartProjectExport
            | Command::ProjectPackageStatus { .. }
            | Command::CancelProjectExport { .. }
            | Command::ReleaseProjectExport { .. }
            | Command::BeginProjectPackageDownload { .. }
            | Command::ProjectPackageDownloadStatus { .. }
            | Command::AbandonProjectPackageDownload { .. }
    )
}
pub(super) fn initialize(db: &Connection) -> PResult<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_package_exports(
            id TEXT PRIMARY KEY,project TEXT NOT NULL REFERENCES projects(id),
            session TEXT NOT NULL REFERENCES sessions(id),request TEXT NOT NULL,
            status TEXT NOT NULL,artifact_sha256 TEXT,disposed INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX IF NOT EXISTS project_package_exports_artifact ON project_package_exports(artifact_sha256);
         CREATE TABLE IF NOT EXISTS project_package_download_begins(
            session TEXT NOT NULL,request TEXT NOT NULL,fingerprint TEXT NOT NULL,operation TEXT NOT NULL,
            epoch TEXT NOT NULL,offset INTEGER NOT NULL,byte_len INTEGER NOT NULL,grant_generation INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(session,request));
         CREATE TABLE IF NOT EXISTS project_package_grant_fences(
            session TEXT NOT NULL,project TEXT NOT NULL,generation INTEGER NOT NULL,PRIMARY KEY(session,project));
         CREATE TABLE IF NOT EXISTS project_package_approvals(
            session TEXT NOT NULL,request TEXT NOT NULL,project TEXT NOT NULL,
            approval_kind TEXT NOT NULL,PRIMARY KEY(session,request));"
    ).map_err(internal)?;
    let has_generation = {
        let mut statement = db
            .prepare("PRAGMA table_info(project_package_download_begins)")
            .map_err(internal)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(internal)?;
        names
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(internal)?
            .iter()
            .any(|name| name == "grant_generation")
    };
    if !has_generation {
        db.execute("ALTER TABLE project_package_download_begins ADD COLUMN grant_generation INTEGER NOT NULL DEFAULT 0",[]).map_err(internal)?;
    }
    let rows = {
        let mut statement = db
            .prepare("SELECT id,status FROM project_package_exports")
            .map_err(internal)?;
        let mut cursor = statement.query([]).map_err(internal)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().map_err(internal)? {
            if rows.len() >= MAX_OPERATIONS as usize {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "package operation journal exceeds admission",
                ));
            }
            let raw: String = row.get(1).map_err(internal)?;
            if raw.len() > 16 * 1024 {
                return Err(ProtocolError::invalid(
                    "package operation record exceeds bound",
                ));
            }
            rows.push((
                row.get::<_, String>(0).map_err(internal)?,
                decode::<PackageExportStatus>(&raw)?,
            ));
        }
        rows
    };
    for (id, mut status) in rows {
        status.validate()?;
        if active(status.state) {
            status.state = PackageExportOperationState::Interrupted;
            status.error = Some(ProtocolError::new(
                ErrorCode::Unavailable,
                "engine restarted before durable package readiness; start a new operation",
            ));
            db.execute(
                "UPDATE project_package_exports SET status=?2 WHERE id=?1",
                params![id, encode(&status)?],
            )
            .map_err(internal)?;
        }
    }
    Ok(())
}

/// Separate OS-owner authority, never the pairing token or a claim of human presence.
pub(super) fn owner_token(root: &Path) -> Result<String> {
    #[cfg(not(unix))]
    {
        let _ = root;
        anyhow::bail!("secured local owner approval is unavailable");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let path = root.join("package-owner.token");
        let read = || -> Result<String> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o777 == 0o600
                    && metadata.nlink() == 1
                    && metadata.len() == 64,
                "package owner capability requires a private current-user regular0600 file"
            );
            let mut token = String::new();
            file.take(65).read_to_string(&mut token)?;
            anyhow::ensure!(
                valid_owner_token(&token),
                "invalid package owner capability"
            );
            Ok(token)
        };
        match read() {
            Ok(value) => Ok(value),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)
                {
                    Ok(mut file) => {
                        file.write_all(token.as_bytes())?;
                        file.sync_all()?;
                        artifacts::sync_dir(root)?;
                        Ok(token)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read(),
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error),
        }
    }
}
fn valid_owner_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn same_secret(expected: &str, supplied: &str) -> bool {
    let difference = expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0u8, |value, (a, b)| value | (a ^ b));
    expected.len() == supplied.len() && difference == 0
}
fn stored(artifact: &PackageArtifactDescriptor) -> PResult<StoredPackage> {
    artifact.validate()?;
    Ok(StoredPackage {
        identity: ArtifactIdentity {
            artifact_id: ArtifactId::new(format!("package.{}", artifact.sha256))
                .map_err(internal)?,
            sha256: artifact.sha256.clone(),
            byte_len: artifact.byte_len,
        },
        manifest_sha256: artifact.manifest_sha256.clone(),
    })
}
fn load_status(
    db: &Connection,
    operation: &PackageOperationId,
    project: &ProjectId,
) -> PResult<(SessionId, PackageExportStatus)> {
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT session,status FROM project_package_exports WHERE id=?1 AND project=?2",
            params![operation.as_str(), project.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(internal)?;
    let (session, raw) = row.ok_or_else(|| {
        ProtocolError::new(ErrorCode::NotFound, "package export operation not found")
    })?;
    let status: PackageExportStatus = decode(&raw)?;
    status.validate()?;
    Ok((SessionId::new(session).map_err(internal)?, status))
}
fn owned_status(
    db: &Connection,
    operation: &PackageOperationId,
    project: &ProjectId,
    session: &SessionId,
) -> PResult<PackageExportStatus> {
    let (owner, status) = load_status(db, operation, project)?;
    if &owner != session {
        return Err(ProtocolError::new(
            ErrorCode::Forbidden,
            "package operation belongs to another session",
        ));
    }
    require_grant(db, session, project, Scope::PackageProject)?;
    Ok(status)
}
fn save_status(db: &Connection, status: &PackageExportStatus) -> PResult<()> {
    status.validate()?;
    let raw = encode(status)?;
    if raw.len() > 16 * 1024 {
        return Err(ProtocolError::invalid(
            "package operation status exceeds bound",
        ));
    }
    db.execute(
        "UPDATE project_package_exports SET status=?2,artifact_sha256=?3 WHERE id=?1",
        params![
            status.operation_id.as_str(),
            raw,
            status.artifact.as_ref().map(|a| a.sha256.as_str())
        ],
    )
    .map_err(internal)?;
    Ok(())
}
fn grant_generation(db: &Connection, session: &SessionId, project: &ProjectId) -> PResult<u64> {
    let value: Option<i64> = db
        .query_row(
            "SELECT generation FROM project_package_grant_fences WHERE session=?1 AND project=?2",
            params![session.as_str(), project.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    u64::try_from(value.unwrap_or(0))
        .map_err(|_| ProtocolError::invalid("negative package grant generation"))
}
fn checked_range(offset: u64, length: u64, total: u64) -> PResult<()> {
    if length == 0 || offset.checked_add(length).is_none_or(|end| end > total) {
        return Err(ProtocolError::invalid(
            "package download range exceeds immutable artifact",
        ));
    }
    Ok(())
}
impl Download {
    fn check(&self, session: &SessionId, project: &ProjectId, epoch: &str) -> PResult<()> {
        if &self.session != session || &self.project != project {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package lease belongs to another session or project",
            ));
        }
        if self.epoch != epoch {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "package download belongs to an old engine epoch",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "package download expired",
            ));
        }
        if self.cursor.lock().map_err(internal)?.abandoned {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package download was abandoned or released",
            ));
        }
        Ok(())
    }
    fn view(&self, endpoint: PathBuf) -> PResult<PackageDownloadLease> {
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        if remaining == 0 {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "package download expired",
            ));
        }
        let cursor = self.cursor.lock().map_err(internal)?;
        if cursor.abandoned {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package download was abandoned or released",
            ));
        }
        let view = PackageDownloadLease {
            lease_id: self.id.clone(),
            engine_epoch: self.epoch.clone(),
            operation_id: self.operation.clone(),
            artifact: self.artifact.clone(),
            offset: self.offset,
            byte_len: self.byte_len,
            next_offset: cursor.next,
            replay: cursor.replay.clone(),
            expires_after_ms: remaining,
            bulk_endpoint: endpoint,
        };
        view.validate()?;
        Ok(view)
    }
}
impl Engine {
    pub(super) fn package_control(
        self: &Arc<Self>,
        request: &Request,
        fingerprint: &str,
    ) -> PResult<ResponseBody> {
        let session = request
            .session
            .as_ref()
            .ok_or_else(|| ProtocolError::new(ErrorCode::Unauthorized, "pairing required"))?;
        let project = project(request)?;
        {
            let db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, session)?;
            if let Some(body) = replay(&db, session.as_str(), request, fingerprint)? {
                if let ResponseBody::PackageDownload(old) = body {
                    return self
                        .package_download_view(&db, session, project, &old.lease_id)
                        .map(ResponseBody::PackageDownload);
                }
                return Ok(body);
            }
        }
        match &request.command {
            Command::StartProjectExport => self.start_package_export(request, fingerprint),
            Command::AllowProjectPackaging { owner_proof } => {
                if !same_secret(&self.package_owner_token, owner_proof) {
                    return Err(ProtocolError::new(
                        ErrorCode::Forbidden,
                        "invalid OS-owner packaging approval capability",
                    ));
                }
                let mut db = self.db.lock().map_err(internal)?;
                let tx = db
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(internal)?;
                self.authorize(&tx, request, session)?;
                if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
                    return Ok(body);
                }
                tx.execute(
                    "INSERT OR IGNORE INTO grants(session,project,scope) VALUES(?1,?2,?3)",
                    params![
                        session.as_str(),
                        project.as_str(),
                        scope_name(Scope::PackageProject)
                    ],
                )
                .map_err(internal)?;
                tx.execute("INSERT INTO project_package_approvals(session,request,project,approval_kind) VALUES(?1,?2,?3,'os_owner_capability')",
                    params![session.as_str(),request.request_id.as_str(),project.as_str()]).map_err(internal)?;
                remember(
                    &tx,
                    session.as_str(),
                    request,
                    fingerprint,
                    &ResponseBody::Ack,
                )?;
                tx.commit().map_err(internal)?;
                Ok(ResponseBody::Ack)
            }
            Command::ProjectPackageStatus { operation_id } => {
                let db = self.db.lock().map_err(internal)?;
                self.authorize(&db, request, session)?;
                Ok(ResponseBody::PackageExport(owned_status(
                    &db,
                    operation_id,
                    project,
                    session,
                )?))
            }
            Command::CancelProjectExport { operation_id }
            | Command::ReleaseProjectExport { operation_id } => {
                let release = matches!(request.command, Command::ReleaseProjectExport { .. });
                let mut db = self.db.lock().map_err(internal)?;
                let tx = db
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(internal)?;
                self.authorize(&tx, request, session)?;
                if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
                    return Ok(body);
                }
                let mut status = owned_status(&tx, operation_id, project, session)?;
                if release {
                    if status.state != PackageExportOperationState::Ready
                        && status.state != PackageExportOperationState::Released
                    {
                        return Err(ProtocolError::invalid(
                            "only Ready packages can be released; cancel active work instead",
                        ));
                    }
                    status.state = PackageExportOperationState::Released;
                } else if active(status.state) {
                    status.state = PackageExportOperationState::Cancelled;
                    status.error = Some(ProtocolError::new(
                        ErrorCode::Forbidden,
                        "package export cancelled by its initiating session",
                    ));
                } else if status.state == PackageExportOperationState::Ready
                    || status.state == PackageExportOperationState::Released
                {
                    return Err(ProtocolError::invalid(
                        "completed packages require explicit release, not cancellation",
                    ));
                }
                save_status(&tx, &status)?;
                let body = ResponseBody::PackageExport(status);
                remember(&tx, session.as_str(), request, fingerprint, &body)?;
                tx.commit().map_err(internal)?;
                drop(db);
                let mut runtime = self.package_exports.lock().map_err(internal)?;
                if let Some(active) = runtime.active.get(operation_id.as_str()) {
                    active.cancel.store(true, Ordering::Release);
                }
                runtime.invalidate(operation_id);
                drop(runtime);
                if release {
                    let _ = self.collect_released_packages();
                }
                Ok(body)
            }
            Command::BeginProjectPackageDownload {
                operation_id,
                offset,
                byte_len,
            } => {
                self.begin_package_download(request, fingerprint, operation_id, *offset, *byte_len)
            }
            Command::ProjectPackageDownloadStatus { lease_id } => {
                let db = self.db.lock().map_err(internal)?;
                self.authorize(&db, request, session)?;
                self.package_download_view(&db, session, project, lease_id)
                    .map(ResponseBody::PackageDownload)
            }
            Command::AbandonProjectPackageDownload { lease_id } => {
                let mut db = self.db.lock().map_err(internal)?;
                let tx = db
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(internal)?;
                self.authorize(&tx, request, session)?;
                if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
                    return Ok(body);
                }
                let download = self.package_download(lease_id)?;
                download.check(session, project, &self.engine_epoch)?;
                owned_status(&tx, &download.operation, project, session)?;
                let body = ResponseBody::PackageDownloadAbandoned {
                    lease_id: lease_id.clone(),
                };
                remember(&tx, session.as_str(), request, fingerprint, &body)?;
                tx.commit().map_err(internal)?;
                drop(db);
                download.cursor.lock().map_err(internal)?.abandoned = true;
                self.package_exports.lock().map_err(internal)?.sweep();
                Ok(body)
            }
            _ => Err(ProtocolError::invalid("not a package operation")),
        }
    }
    fn start_package_export(
        self: &Arc<Self>,
        request: &Request,
        fingerprint: &str,
    ) -> PResult<ResponseBody> {
        let session = request.session.as_ref().unwrap();
        let project = project(request)?;
        let expected = request
            .expected_revision
            .ok_or_else(|| ProtocolError::invalid("package export requires expected revision"))?;
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        self.authorize(&tx, request, session)?;
        if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
            return Ok(body);
        }
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM project_package_exports", [], |row| {
                row.get(0)
            })
            .map_err(internal)?;
        if count >= MAX_OPERATIONS {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "durable package operation journal is full; tombstones are never silently evicted",
            ));
        }
        let mut runtime = self.package_exports.lock().map_err(internal)?;
        runtime.sweep();
        if runtime.active.len() >= MAX_ACTIVE {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "two package export operations are already active",
            ));
        }
        let operation = id!(PackageOperationId);
        let status = PackageExportStatus {
            operation_id: operation.clone(),
            project_id: project.clone(),
            request_id: request.request_id.clone(),
            requested_revision: expected,
            state: PackageExportOperationState::Queued,
            capture: None,
            progress: PackageExportProgress {
                completed_objects: 0,
                total_objects: None,
                completed_bytes: 0,
                total_bytes: None,
            },
            artifact: None,
            error: None,
        };
        status.validate()?;
        tx.execute("INSERT INTO project_package_exports(id,project,session,request,status) VALUES(?1,?2,?3,?4,?5)",
            params![operation.as_str(),project.as_str(),session.as_str(),request.request_id.as_str(),encode(&status)?]).map_err(internal)?;
        let body = ResponseBody::PackageExport(status);
        remember(&tx, session.as_str(), request, fingerprint, &body)?;
        let generation = grant_generation(&tx, session, project)?;
        tx.commit().map_err(internal)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + self.config.worker_timeout.min(MAX_OPERATION_LIFETIME);
        runtime.active.insert(
            operation.to_string(),
            ActiveOperation {
                session: session.clone(),
                project: project.clone(),
                cancel: cancel.clone(),
                deadline,
                reserved_bytes: 0,
                grant_generation: generation,
            },
        );
        drop(runtime);
        drop(db);
        let engine = self.clone();
        let request = request.clone();
        let failure_request = request.clone();
        let failure_operation = operation.clone();
        let spawned = spawn_worker(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.run_package_export(&request, &operation, &cancel, deadline)
            }))
            .unwrap_or_else(|_| Err(internal("package materialization worker panicked")));
            if let Err(error) = result {
                let _ = engine.fail_package_export(&request, &operation, error);
            }
            if let Ok(mut runtime) = engine.package_exports.lock() {
                runtime.active.remove(operation.as_str());
            }
        });
        if let Err(error) = spawned {
            let outcome = self.fail_package_export(
                &failure_request,
                &failure_operation,
                unavailable(format!("cannot start package worker: {error}")),
            );
            if let Ok(mut runtime) = self.package_exports.lock() {
                runtime.active.remove(failure_operation.as_str());
            }
            outcome?;
        }
        Ok(body)
    }
    fn check_package_work(
        &self,
        request: &Request,
        operation: &PackageOperationId,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> PResult<()> {
        if Instant::now() >= deadline {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "package export deadline expired",
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package export was cancelled or revoked",
            ));
        }
        let db = self.db.lock().map_err(internal)?;
        let session = request.session.as_ref().unwrap();
        self.authenticate(&db, session, request.auth_token.as_deref())?;
        let status = owned_status(&db, operation, project(request)?, session)?;
        if !active(status.state) {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package export is no longer active",
            ));
        }
        Ok(())
    }
    fn run_package_export(
        &self,
        request: &Request,
        operation: &PackageOperationId,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> PResult<()> {
        self.check_package_work(request, operation, cancel, deadline)?;
        {
            let db = self.db.lock().map_err(internal)?;
            let mut status = owned_status(
                &db,
                operation,
                project(request)?,
                request.session.as_ref().unwrap(),
            )?;
            if !active(status.state) {
                return Err(ProtocolError::new(
                    ErrorCode::Forbidden,
                    "package export is no longer active",
                ));
            }
            status.state = PackageExportOperationState::Capturing;
            save_status(&db, &status)?;
        }
        let plan = project_packages::capture_retained(
            self,
            request.session.as_ref().unwrap(),
            request.auth_token.as_deref().unwrap_or(""),
            project(request)?,
            request.expected_revision.unwrap(),
        )?;
        self.check_package_work(request, operation, cancel, deadline)?;
        let estimate = PackageStore::estimate(&plan.manifest)?;
        // Package-store policy is separate from source snapshot policy, while
        // physical free-space reservations are shared with imports and workers.
        let disk_memory = self.pool.reserve_memory(128 * 1024)?;
        let disk_reservation = disk_memory.with_storage(
            estimate.byte_len,
            fs2::available_space(&self.config.state_dir).map_err(unavailable)?,
        )?;
        {
            let _gate = self.artifact_gate.lock().map_err(internal)?;
            // Keep a conservative reservation snapshot while filesystem
            // enumeration runs without the runtime or authority mutex.
            // Gate ownership prevents new materialization reservations; old
            // reservations remain counted even if their workers finish.
            let reserved = {
                let runtime = self.package_exports.lock().map_err(internal)?;
                runtime
                    .active
                    .values()
                    .try_fold(0u64, |sum, entry| sum.checked_add(entry.reserved_bytes))
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::ResourceExhausted,
                            "package storage accounting overflow",
                        )
                    })?
            };
            let allocated = self.package_store.allocated_bytes()?;
            let mut runtime = self.package_exports.lock().map_err(internal)?;
            if allocated
                .checked_add(reserved)
                .and_then(|sum| sum.checked_add(estimate.byte_len))
                .is_none_or(|sum| sum > self.config.max_snapshot_bytes)
            {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "aggregate complete plus reserved package storage budget exhausted",
                ));
            }
            runtime
                .active
                .get_mut(operation.as_str())
                .ok_or_else(|| internal("active package operation disappeared"))?
                .reserved_bytes = estimate.byte_len;
        }
        let binding = PackageCaptureBinding {
            project_id: plan.manifest.origin_project_id.clone(),
            captured_revision: plan.manifest.captured_revision,
            captured_event_cursor: plan.manifest.captured_event_cursor,
            manifest_sha256: plan.manifest_sha256.clone(),
        };
        {
            let db = self.db.lock().map_err(internal)?;
            let mut status = owned_status(
                &db,
                operation,
                project(request)?,
                request.session.as_ref().unwrap(),
            )?;
            if !active(status.state) {
                return Err(ProtocolError::new(
                    ErrorCode::Forbidden,
                    "package export is no longer active",
                ));
            }
            status.state = PackageExportOperationState::Streaming;
            status.capture = Some(binding.clone());
            status.progress.total_objects = Some(plan.manifest.objects.len() as u64);
            status.progress.total_bytes = Some(estimate.byte_len);
            save_status(&db, &status)?;
        }
        let mut last_update = Instant::now();
        let published = self.package_store.materialize(
            &plan.manifest,
            |object| plan.open_object(object),
            PackageCodecLimits {
                max_package_bytes: self.config.max_snapshot_bytes,
            },
            || self.check_package_work(request, operation, cancel, deadline),
            |objects, bytes| {
                if last_update.elapsed() < Duration::from_millis(100) && bytes != estimate.byte_len
                {
                    return Ok(());
                }
                self.check_package_work(request, operation, cancel, deadline)?;
                let db = self.db.lock().map_err(internal)?;
                let mut status = owned_status(
                    &db,
                    operation,
                    project(request)?,
                    request.session.as_ref().unwrap(),
                )?;
                if !active(status.state) {
                    return Err(ProtocolError::new(
                        ErrorCode::Forbidden,
                        "package export is no longer active",
                    ));
                }
                status.progress.completed_objects = objects;
                status.progress.completed_bytes = bytes;
                save_status(&db, &status)?;
                last_update = Instant::now();
                Ok(())
            },
        )?;
        self.check_package_work(request, operation, cancel, deadline)?;
        if published.manifest_sha256 != binding.manifest_sha256
            || published.identity.byte_len != estimate.byte_len
        {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "published package differs from retained capture",
            ));
        }
        {
            let mut db = self.db.lock().map_err(internal)?;
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(internal)?;
            self.authenticate(
                &tx,
                request.session.as_ref().unwrap(),
                request.auth_token.as_deref(),
            )?;
            let mut status = owned_status(
                &tx,
                operation,
                project(request)?,
                request.session.as_ref().unwrap(),
            )?;
            if !active(status.state) || cancel.load(Ordering::Acquire) || Instant::now() >= deadline
            {
                return Err(ProtocolError::new(
                    ErrorCode::Forbidden,
                    "package publication cancelled, revoked or expired",
                ));
            }
            let artifact = PackageArtifactDescriptor {
                format_version: 1,
                sha256: published.identity.sha256,
                byte_len: published.identity.byte_len,
                manifest_sha256: published.manifest_sha256,
                project_id: binding.project_id.clone(),
                captured_revision: binding.captured_revision,
                captured_event_cursor: binding.captured_event_cursor,
                object_count: plan.manifest.objects.len() as u32,
                verification: PackageReadyVerification::AllDeclaredObjectsVerified,
            };
            status.state = PackageExportOperationState::Ready;
            status.capture = Some(binding);
            status.progress.completed_objects = artifact.object_count as u64;
            status.progress.completed_bytes = artifact.byte_len;
            status.artifact = Some(artifact);
            status.error = None;
            save_status(&tx, &status)?;
            let generation =
                grant_generation(&tx, request.session.as_ref().unwrap(), project(request)?)?;
            tx.commit().map_err(internal)?;
            self.package_exports
                .lock()
                .map_err(internal)?
                .verification
                .insert(operation.to_string(), (generation, Verification::Verified));
        }
        // Source holds outlive durable bundle and SQL readiness. The disk
        // reservation is released only once physical complete bytes are counted.
        drop(plan);
        drop(disk_reservation);
        Ok(())
    }
    fn fail_package_export(
        &self,
        request: &Request,
        operation: &PackageOperationId,
        error: ProtocolError,
    ) -> PResult<()> {
        let db = self.db.lock().map_err(internal)?;
        let (_, mut status) = load_status(&db, operation, project(request)?)?;
        if active(status.state) {
            let revoked = require_grant(
                &db,
                request.session.as_ref().unwrap(),
                project(request)?,
                Scope::PackageProject,
            )
            .is_err();
            status.state = if revoked {
                PackageExportOperationState::Cancelled
            } else {
                PackageExportOperationState::Failed
            };
            status.error = Some(error);
            save_status(&db, &status)?;
        }
        Ok(())
    }
    fn package_download(&self, id: &PackageDownloadId) -> PResult<Arc<Download>> {
        let mut runtime = self.package_exports.lock().map_err(internal)?;
        runtime.sweep();
        runtime.downloads.get(id.as_str()).cloned().ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::Unavailable,
                "package download expired, was abandoned, or belongs to a prior engine epoch",
            )
        })
    }
    fn package_download_view(
        &self,
        db: &Connection,
        session: &SessionId,
        project: &ProjectId,
        id: &PackageDownloadId,
    ) -> PResult<PackageDownloadLease> {
        let download = self.package_download(id)?;
        download.check(session, project, &self.engine_epoch)?;
        let status = owned_status(db, &download.operation, project, session)?;
        if grant_generation(db, session, project)? != download.grant_generation {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package lease grant generation was revoked",
            ));
        }
        if status.state != PackageExportOperationState::Ready
            || status.artifact.as_ref() != Some(&download.artifact)
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package download is no longer authorized by its Ready operation",
            ));
        }
        download.view(self.config.bulk_endpoint())
    }
    fn begin_package_download(
        self: &Arc<Self>,
        request: &Request,
        fingerprint: &str,
        operation: &PackageOperationId,
        offset: u64,
        byte_len: u64,
    ) -> PResult<ResponseBody> {
        let session = request.session.as_ref().unwrap();
        let project = project(request)?;
        let artifact = {
            let db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, session)?;
            let status = owned_status(&db, operation, project, session)?;
            if status.state != PackageExportOperationState::Ready {
                return Err(ProtocolError::new(
                    ErrorCode::Unavailable,
                    "package is not Ready",
                ));
            }
            status
                .artifact
                .ok_or_else(|| internal("Ready package artifact missing"))?
        };
        checked_range(offset, byte_len, artifact.byte_len)?;
        if let Some(body) = self.ensure_package_verified(
            request,
            fingerprint,
            operation,
            &artifact,
            offset,
            byte_len,
        )? {
            return Ok(body);
        }
        #[cfg(test)]
        before_lease_admission();
        let memory = self.pool.reserve_memory(DOWNLOAD_MEMORY)?;
        // No global authority lock during immutable file validation.
        let stored = stored(&artifact)?;
        let file = self.package_store.open_readonly(&stored)?;
        drop(file);
        let _gate = self.artifact_gate.lock().map_err(internal)?;
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        self.authorize(&tx, request, session)?;
        if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
            if let ResponseBody::PackageDownload(old) = body {
                return self
                    .package_download_view(&tx, session, project, &old.lease_id)
                    .map(ResponseBody::PackageDownload);
            }
            return Err(ProtocolError::new(
                ErrorCode::RequestConflict,
                "request identity belongs to another command",
            ));
        }
        let status = owned_status(&tx, operation, project, session)?;
        if status.state != PackageExportOperationState::Ready
            || status.artifact.as_ref() != Some(&artifact)
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package readiness changed before lease admission",
            ));
        }
        let generation = grant_generation(&tx, session, project)?;
        let binding:Option<(String,i64,String,i64,i64,String)>=tx.query_row(
            "SELECT epoch,grant_generation,operation,offset,byte_len,fingerprint FROM project_package_download_begins WHERE session=?1 AND request=?2",
            params![session.as_str(),request.request_id.as_str()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))
        ).optional().map_err(internal)?;
        if !binding.is_some_and(
            |(
                epoch,
                original_generation,
                original_operation,
                original_offset,
                original_length,
                original_fingerprint,
            )| {
                epoch == self.engine_epoch
                    && original_generation as u64 == generation
                    && original_operation == operation.as_str()
                    && original_offset as u64 == offset
                    && original_length as u64 == byte_len
                    && original_fingerprint == fingerprint
            },
        ) {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "original package begin binding was revoked or changed before final admission",
            ));
        }
        let holds = self.package_holds.acquire(&[stored.identity])?;
        let mut runtime = self.package_exports.lock().map_err(internal)?;
        runtime.sweep();
        if !runtime.verification.get(operation.as_str()).is_some_and(
            |(verified_generation, state)| {
                *verified_generation == generation && matches!(state, Verification::Verified)
            },
        ) {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "package grant changed before verified lease admission",
            ));
        }
        if runtime.downloads.len() >= MAX_DOWNLOADS
            || runtime
                .downloads
                .values()
                .filter(|d| &d.operation == operation)
                .count()
                >= MAX_OPERATION_DOWNLOADS
        {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "package download lease admission exhausted",
            ));
        }
        let id = id!(PackageDownloadId);
        let download = Arc::new(Download {
            id: id.clone(),
            session: session.clone(),
            project: project.clone(),
            operation: operation.clone(),
            epoch: self.engine_epoch.clone(),
            artifact,
            offset,
            byte_len,
            deadline: Instant::now() + DOWNLOAD_LIFETIME,
            grant_generation: generation,
            cursor: Mutex::new(DownloadCursor {
                next: offset,
                replay: None,
                connection: None,
                abandoned: false,
            }),
            _holds: holds,
            _memory: memory,
        });
        let view = download.view(self.config.bulk_endpoint())?;
        let body = ResponseBody::PackageDownload(view);
        remember(&tx, session.as_str(), request, fingerprint, &body)?;
        tx.commit().map_err(internal)?;
        runtime.downloads.insert(id.to_string(), download);
        Ok(body)
    }

    fn ensure_package_verified(
        self: &Arc<Self>,
        request: &Request,
        fingerprint: &str,
        operation: &PackageOperationId,
        artifact: &PackageArtifactDescriptor,
        offset: u64,
        byte_len: u64,
    ) -> PResult<Option<ResponseBody>> {
        let session = request.session.as_ref().unwrap();
        let project = project(request)?;
        // Small bounded reservation precedes a short admission transaction, not a hash.
        let memory = self.pool.reserve_memory(DOWNLOAD_MEMORY)?;
        let _gate = self.artifact_gate.lock().map_err(internal)?;
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        self.authorize(&tx, request, session)?;
        check_request_identity(&tx, session.as_str(), request, fingerprint)?;
        if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
            if let ResponseBody::PackageDownload(old) = body {
                return self
                    .package_download_view(&tx, session, project, &old.lease_id)
                    .map(ResponseBody::PackageDownload)
                    .map(Some);
            }
            return Err(ProtocolError::new(
                ErrorCode::RequestConflict,
                "package begin identity has another durable outcome",
            ));
        }
        let status = owned_status(&tx, operation, project, session)?;
        if status.state != PackageExportOperationState::Ready
            || status.artifact.as_ref() != Some(artifact)
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package no longer Ready",
            ));
        }
        let generation = grant_generation(&tx, session, project)?;
        let prior:Option<(String,String,i64,i64,i64)>=tx.query_row(
            "SELECT epoch,operation,offset,byte_len,grant_generation FROM project_package_download_begins WHERE session=?1 AND request=?2",
            params![session.as_str(),request.request_id.as_str()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))
        ).optional().map_err(internal)?;
        if let Some((epoch, prior_operation, prior_offset, prior_length, prior_generation)) = prior
        {
            if prior_generation as u64 != generation {
                return Err(ProtocolError::new(
                    ErrorCode::Unavailable,
                    "package begin was revoked; use a new request after explicit regrant",
                ));
            }
            if epoch != self.engine_epoch {
                return Err(ProtocolError::new(
                    ErrorCode::Unavailable,
                    "package begin belongs to a previous engine epoch; use a new request",
                ));
            }
            if prior_operation != operation.as_str()
                || prior_offset as u64 != offset
                || prior_length as u64 != byte_len
            {
                return Err(ProtocolError::new(
                    ErrorCode::RequestConflict,
                    "package download begin binding differs",
                ));
            }
        } else {
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM project_package_download_begins",
                    [],
                    |row| row.get(0),
                )
                .map_err(internal)?;
            if count >= MAX_DOWNLOAD_BEGINS {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "durable package download journal is full",
                ));
            }
            claim_request_identity(&tx, session.as_str(), request, fingerprint)?;
            tx.execute(
                "INSERT INTO project_package_download_begins VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    session.as_str(),
                    request.request_id.as_str(),
                    fingerprint,
                    operation.as_str(),
                    &self.engine_epoch,
                    offset as i64,
                    byte_len as i64,
                    generation as i64
                ],
            )
            .map_err(internal)?;
        }
        let mut runtime = self.package_exports.lock().map_err(internal)?;
        runtime.sweep();
        let current = runtime
            .verification
            .get(operation.as_str())
            .filter(|(epoch, _)| *epoch == generation)
            .map(|(_, value)| value.clone());
        let pending = |completed, total| {
            ResponseBody::PackageDownloadPending(PackageDownloadPending {
                operation_id: operation.clone(),
                artifact_sha256: artifact.sha256.clone(),
                verification: PackageDownloadVerificationState::Verifying,
                completed_bytes: completed,
                total_bytes: total,
            })
        };
        match current {
            Some(Verification::Verified) => {
                tx.commit().map_err(internal)?;
                Ok(None)
            }
            Some(Verification::Failed(error)) => Err(error),
            Some(Verification::Pending { completed, total }) => {
                tx.commit().map_err(internal)?;
                Ok(Some(pending(completed, total)))
            }
            None => {
                if runtime.active.contains_key(operation.as_str())
                    || runtime.active.len() >= MAX_ACTIVE
                {
                    return Err(ProtocolError::new(ErrorCode::ResourceExhausted,"package verification waits for the previous worker to drain or a free worker slot"));
                }
                let stored = stored(artifact)?;
                let holds = self.package_holds.acquire(&[stored.identity.clone()])?;
                let cancel = Arc::new(AtomicBool::new(false));
                let deadline =
                    Instant::now() + self.config.worker_timeout.min(MAX_OPERATION_LIFETIME);
                tx.commit().map_err(internal)?;
                runtime.active.insert(
                    operation.to_string(),
                    ActiveOperation {
                        session: session.clone(),
                        project: project.clone(),
                        cancel: cancel.clone(),
                        deadline,
                        reserved_bytes: 0,
                        grant_generation: generation,
                    },
                );
                runtime.verification.insert(
                    operation.to_string(),
                    (
                        generation,
                        Verification::Pending {
                            completed: 0,
                            total: artifact.byte_len,
                        },
                    ),
                );
                drop(runtime);
                drop(db);
                drop(_gate);
                let engine = self.clone();
                let request = request.clone();
                let operation = operation.clone();
                let artifact = artifact.clone();
                let body = pending(0, artifact.byte_len);
                let failure_operation = operation.clone();
                let spawned = spawn_worker(move || {
                    let _memory = memory;
                    let _holds = holds;
                    let check = || -> PResult<()> {
                        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                            return Err(ProtocolError::new(
                                ErrorCode::Unavailable,
                                "package verification cancelled, revoked or expired",
                            ));
                        }
                        let db = engine.db.lock().map_err(internal)?;
                        engine.authenticate(
                            &db,
                            request.session.as_ref().unwrap(),
                            request.auth_token.as_deref(),
                        )?;
                        let current = owned_status(
                            &db,
                            &operation,
                            super::project(&request)?,
                            request.session.as_ref().unwrap(),
                        )?;
                        if grant_generation(
                            &db,
                            request.session.as_ref().unwrap(),
                            super::project(&request)?,
                        )? != generation
                        {
                            return Err(ProtocolError::new(
                                ErrorCode::Forbidden,
                                "package verification grant generation was revoked",
                            ));
                        }
                        if current.state != PackageExportOperationState::Ready
                            || current.artifact.as_ref() != Some(&artifact)
                        {
                            return Err(ProtocolError::new(
                                ErrorCode::Forbidden,
                                "package released or changed during verification",
                            ));
                        }
                        Ok(())
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.package_store.open_verified(
                            &stored,
                            || check(),
                            |completed| {
                                if let Some((
                                    _,
                                    Verification::Pending {
                                        completed: progress,
                                        ..
                                    },
                                )) = engine
                                    .package_exports
                                    .lock()
                                    .map_err(internal)?
                                    .verification
                                    .get_mut(operation.as_str())
                                {
                                    *progress = completed;
                                }
                                Ok(())
                            },
                        )?;
                        check()?;
                        Ok::<(), ProtocolError>(())
                    }))
                    .unwrap_or_else(|_| Err(internal("package verification worker panicked")));
                    // Grant/release checks precede result publication; a lease
                    // independently rechecks both before it can be admitted.
                    if let Ok(mut runtime) = engine.package_exports.lock() {
                        runtime.active.remove(operation.as_str());
                        runtime.verification.insert(
                            operation.to_string(),
                            (
                                generation,
                                match result {
                                    Ok(()) => Verification::Verified,
                                    Err(error) => Verification::Failed(error),
                                },
                            ),
                        );
                    }
                });
                if let Err(error) = spawned {
                    let mut runtime = self.package_exports.lock().map_err(internal)?;
                    runtime.active.remove(failure_operation.as_str());
                    runtime.verification.insert(
                        failure_operation.to_string(),
                        (
                            generation,
                            Verification::Failed(unavailable(format!(
                                "cannot start package verification worker: {error}"
                            ))),
                        ),
                    );
                }
                Ok(Some(body))
            }
        }
    }

    fn check_package_connection(
        &self,
        connection: &PackageConnection,
        handshake: &PackageBulkHandshake,
    ) -> PResult<()> {
        handshake.validate()?;
        let download = &connection.download;
        download.check(
            &handshake.session,
            &download.project,
            &handshake.engine_epoch,
        )?;
        if handshake.lease_id != download.id
            || handshake.operation_id != download.operation
            || handshake.artifact_sha256 != download.artifact.sha256
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package handshake binding mismatch",
            ));
        }
        if download
            .cursor
            .lock()
            .map_err(internal)?
            .connection
            .as_deref()
            != Some(&connection.nonce)
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package connection no longer owns this lease",
            ));
        }
        let db = self.db.lock().map_err(internal)?;
        self.authenticate(&db, &handshake.session, Some(&handshake.auth_token))?;
        let status = owned_status(
            &db,
            &download.operation,
            &download.project,
            &handshake.session,
        )?;
        if grant_generation(&db, &handshake.session, &download.project)?
            != download.grant_generation
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package lease grant generation was revoked",
            ));
        }
        if status.state != PackageExportOperationState::Ready
            || status.artifact.as_ref() != Some(&download.artifact)
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package was released or changed",
            ));
        }
        Ok(())
    }
    pub(crate) fn package_bulk_open(
        self: &Arc<Self>,
        handshake: &PackageBulkHandshake,
    ) -> PResult<(PackageConnection, PackageDownloadLease)> {
        handshake.validate()?;
        let download = self.package_download(&handshake.lease_id)?;
        download.check(
            &handshake.session,
            &download.project,
            &handshake.engine_epoch,
        )?;
        {
            let db = self.db.lock().map_err(internal)?;
            self.authenticate(&db, &handshake.session, Some(&handshake.auth_token))?;
            self.package_download_view(&db, &handshake.session, &download.project, &download.id)?;
        }
        if handshake.operation_id != download.operation
            || handshake.artifact_sha256 != download.artifact.sha256
        {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "package handshake binding mismatch",
            ));
        }
        let nonce = Uuid::new_v4().to_string();
        {
            let mut cursor = download.cursor.lock().map_err(internal)?;
            if cursor.connection.is_some() {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "package lease already has an active connection",
                ));
            }
            cursor.connection = Some(nonce.clone());
        }
        let mut connection = PackageConnection {
            download,
            nonce,
            file: None,
        };
        connection.file = Some(
            self.package_store
                .open_readonly(&stored(&connection.download.artifact)?)?,
        );
        self.check_package_connection(&connection, handshake)?;
        let view = connection.download.view(self.config.bulk_endpoint())?;
        Ok((connection, view))
    }
    pub(crate) fn package_bulk_read(
        &self,
        connection: &mut PackageConnection,
        handshake: &PackageBulkHandshake,
        offset: u64,
        byte_len: u32,
    ) -> PResult<(Vec<u8>, u64)> {
        if byte_len == 0 || byte_len as usize > MAX_ARTIFACT_CHUNK_BYTES {
            return Err(ProtocolError::invalid(
                "package chunk exceeds bounded range",
            ));
        }
        self.check_package_connection(connection, handshake)?;
        let download = &connection.download;
        checked_range(
            offset,
            byte_len as u64,
            download
                .offset
                .checked_add(download.byte_len)
                .ok_or_else(|| ProtocolError::invalid("package range overflow"))?,
        )?;
        if offset < download.offset {
            return Err(ProtocolError::invalid(
                "package chunk precedes granted range",
            ));
        }
        let next = {
            let cursor = download.cursor.lock().map_err(internal)?;
            if offset == cursor.next {
                offset + byte_len as u64
            } else if cursor
                .replay
                .as_ref()
                .is_some_and(|last| last.offset == offset && last.byte_len == byte_len)
            {
                cursor.next
            } else {
                return Err(ProtocolError::invalid(
                    "package chunks must be contiguous or exact last-delivery replay",
                ));
            }
        };
        let file = connection
            .file
            .as_mut()
            .ok_or_else(|| internal("package read handle unavailable"))?;
        let mut bytes = vec![0; byte_len as usize];
        file.seek(SeekFrom::Start(offset)).map_err(unavailable)?;
        file.read_exact(&mut bytes).map_err(unavailable)?;
        // Recheck after potentially blocking file I/O, before any response bytes.
        self.check_package_connection(connection, handshake)?;
        Ok((bytes, next))
    }
    pub(crate) fn package_bulk_delivered(
        &self,
        connection: &PackageConnection,
        handshake: &PackageBulkHandshake,
        offset: u64,
        byte_len: u32,
    ) -> PResult<()> {
        self.check_package_connection(connection, handshake)?;
        let mut cursor = connection.download.cursor.lock().map_err(internal)?;
        if offset == cursor.next {
            cursor.next = offset
                .checked_add(byte_len as u64)
                .ok_or_else(|| ProtocolError::invalid("package cursor overflow"))?;
            cursor.replay = Some(PackageChunkRange { offset, byte_len });
        } else if !cursor
            .replay
            .as_ref()
            .is_some_and(|last| last.offset == offset && last.byte_len == byte_len)
        {
            return Err(ProtocolError::invalid(
                "package delivery cursor changed unexpectedly",
            ));
        }
        Ok(())
    }
    pub(super) fn package_revoke(&self, session: &SessionId, project: &ProjectId) {
        // This advisory hook may run after a regrant. Only generations already
        // revoked in durable state are affected; fresh work is not collateral.
        let generation = {
            let Ok(db) = self.db.lock() else {
                return;
            };
            let Ok(value) = grant_generation(&db, session, project) else {
                return;
            };
            value
        };
        if let Ok(mut runtime) = self.package_exports.lock() {
            for operation in runtime.active.values().filter(|a| {
                &a.session == session && &a.project == project && a.grant_generation < generation
            }) {
                operation.cancel.store(true, Ordering::Release);
            }
            for download in runtime.downloads.values().filter(|d| {
                &d.session == session && &d.project == project && d.grant_generation < generation
            }) {
                if let Ok(mut cursor) = download.cursor.lock() {
                    cursor.abandoned = true;
                }
            }
            runtime.sweep();
        }
    }
    fn collect_released_packages(&self) -> PResult<()> {
        // Same order as source eviction and capture holds. No hash/copy under DB.
        let _gate = self.artifact_gate.lock().map_err(internal)?;
        if self
            .package_exports
            .lock()
            .map_err(internal)?
            .active
            .values()
            .any(|entry| entry.reserved_bytes > 0)
        {
            return Ok(());
        }
        let candidates = {
            let db = self.db.lock().map_err(internal)?;
            let mut statement=db.prepare("SELECT status FROM project_package_exports e WHERE disposed=0 AND artifact_sha256 IS NOT NULL AND NOT EXISTS(SELECT 1 FROM project_package_exports r WHERE r.artifact_sha256=e.artifact_sha256 AND json_extract(r.status,'$.state')='ready') LIMIT 16").map_err(internal)?;
            let rows = statement
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(internal)?;
            let mut candidates = Vec::new();
            for row in rows {
                let status: PackageExportStatus = decode(&row.map_err(internal)?)?;
                if status.state == PackageExportOperationState::Released {
                    candidates.push(status);
                }
            }
            candidates
        };
        for status in candidates {
            let artifact = status
                .artifact
                .as_ref()
                .ok_or_else(|| internal("released package receipt lacks artifact"))?;
            if self.package_holds.contains(&artifact.sha256)? {
                continue;
            }
            self.package_store.remove(&stored(artifact)?)?;
            let db = self.db.lock().map_err(internal)?;
            db.execute("UPDATE project_package_exports SET disposed=1 WHERE artifact_sha256=?1 AND json_extract(status,'$.state')='released'",[&artifact.sha256]).map_err(internal)?;
        }
        Ok(())
    }
}
pub(super) fn revoke(db: &Connection, session: &SessionId, project: &ProjectId) -> PResult<()> {
    let next = grant_generation(db, session, project)?
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "package grant generation exhausted",
            )
        })?;
    db.execute("INSERT INTO project_package_grant_fences(session,project,generation) VALUES(?1,?2,?3) ON CONFLICT(session,project) DO UPDATE SET generation=excluded.generation",
        params![session.as_str(),project.as_str(),next as i64]).map_err(internal)?;
    let rows = {
        let mut statement = db
            .prepare("SELECT status FROM project_package_exports WHERE session=?1 AND project=?2")
            .map_err(internal)?;
        let rows = statement
            .query_map(params![session.as_str(), project.as_str()], |r| {
                r.get::<_, String>(0)
            })
            .map_err(internal)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(internal)?
    };
    for raw in rows {
        let mut status: PackageExportStatus = decode(&raw)?;
        if active(status.state) {
            status.state = PackageExportOperationState::Cancelled;
            status.error = Some(ProtocolError::new(
                ErrorCode::Forbidden,
                "authorizing PackageProject grant revoked",
            ));
            save_status(db, &status)?;
        }
    }
    Ok(())
}
pub(super) fn start_sweeper(engine: &Arc<Engine>) {
    let weak = Arc::downgrade(engine);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        let Some(engine) = weak.upgrade() else {
            break;
        };
        if let Ok(mut runtime) = engine.package_exports.lock() {
            runtime.sweep();
        }
        let _ = engine.collect_released_packages();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        engine: Arc<Engine>,
        session: SessionId,
        token: String,
        project: ProjectId,
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn request(&self, command: Command, revision: Option<u64>) -> Request {
            Request::new(id!(RequestId), command)
                .with_session(self.session.clone())
                .with_auth_token(self.token.clone())
                .in_project(self.project.clone(), revision.map(RevisionId::new))
        }
        fn call(&self, command: Command, revision: Option<u64>) -> PResult<ResponseBody> {
            self.engine.handle(self.request(command, revision)).result
        }
        fn source(&self, bytes: &[u8]) -> SourceVersionId {
            let path = self.dir.path().join(format!("source-{}", Uuid::new_v4()));
            fs::write(&path, bytes).unwrap();
            match self.call(Command::ImportSource { path }, None).unwrap() {
                ResponseBody::Source { source_version } => source_version,
                _ => panic!(),
            }
        }
        fn status(&self, id: &PackageOperationId) -> PackageExportStatus {
            match self
                .call(
                    Command::ProjectPackageStatus {
                        operation_id: id.clone(),
                    },
                    None,
                )
                .unwrap()
            {
                ResponseBody::PackageExport(value) => value,
                _ => panic!(),
            }
        }
        fn ready(&self) -> (Request, PackageExportStatus) {
            let request = self.request(Command::StartProjectExport, Some(0));
            let initial = match self.engine.handle(request.clone()).result.unwrap() {
                ResponseBody::PackageExport(value) => value,
                _ => panic!(),
            };
            assert_eq!(initial.state, PackageExportOperationState::Queued);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let status = self.status(&initial.operation_id);
                if status.state.is_terminal() {
                    assert_eq!(
                        status.state,
                        PackageExportOperationState::Ready,
                        "{:?}",
                        status.error
                    );
                    return (request, status);
                }
                assert!(
                    Instant::now() < deadline,
                    "package operation did not finish"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        fn begin(&self, status: &PackageExportStatus) -> (Request, PackageDownloadLease) {
            let artifact = status.artifact.as_ref().unwrap();
            let request = self.request(
                Command::BeginProjectPackageDownload {
                    operation_id: status.operation_id.clone(),
                    offset: 0,
                    byte_len: artifact.byte_len,
                },
                None,
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match self.engine.handle(request.clone()).result.unwrap() {
                    ResponseBody::PackageDownload(lease) => return (request, lease),
                    ResponseBody::PackageDownloadPending(value) => {
                        value.validate().unwrap();
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    _ => panic!(),
                }
            }
        }
        fn handshake(&self, lease: &PackageDownloadLease) -> PackageBulkHandshake {
            PackageBulkHandshake {
                version: PROTOCOL_VERSION,
                channel: PackageBulkChannel::ProjectPackageDownload,
                session: self.session.clone(),
                auth_token: self.token.clone(),
                lease_id: lease.lease_id.clone(),
                engine_epoch: lease.engine_epoch.clone(),
                operation_id: lease.operation_id.clone(),
                artifact_sha256: lease.artifact.sha256.clone(),
            }
        }
    }
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let worker = dir.path().join("nonexecuted-worker");
        fs::write(&worker, b"nonexecuted package fixture").unwrap();
        let engine = Engine::open(EngineConfig::new(dir.path().join("state"), worker)).unwrap();
        let (session, token) = match engine
            .handle(Request::new(
                id!(RequestId),
                Command::Pair {
                    client_name: "package tests".into(),
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
            _ => panic!(),
        };
        let project = match engine
            .handle(
                Request::new(
                    id!(RequestId),
                    Command::CreateProject {
                        name: "package tests".into(),
                    },
                )
                .with_session(session.clone())
                .with_auth_token(token.clone()),
            )
            .result
            .unwrap()
        {
            ResponseBody::Project(value) => value.project_id,
            _ => panic!(),
        };
        Fixture {
            engine,
            session,
            token,
            project,
            dir,
        }
    }
    fn restart(f: Fixture) -> Fixture {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f.engine.package_exports.lock().unwrap().active.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let Fixture {
            engine,
            session,
            token,
            project,
            dir,
        } = f;
        let config = engine.config.clone();
        drop(engine);
        let engine = Engine::open(config).unwrap();
        Fixture {
            engine,
            session,
            token,
            project,
            dir,
        }
    }
    #[test]
    fn explicit_owner_approval_is_scoped_redacted_and_durably_replayed() {
        let f = fixture();
        f.engine
            .db
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::PackageProject)
                ],
            )
            .unwrap();
        assert_eq!(
            f.call(Command::StartProjectExport, Some(0))
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            f.call(
                Command::AllowProjectPackaging {
                    owner_proof: f.engine.token.clone()
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::Forbidden
        );
        let proof = f.engine.package_owner_token.clone();
        let request = f.request(
            Command::AllowProjectPackaging {
                owner_proof: proof.clone(),
            },
            None,
        );
        assert!(!format!("{:?}", request.command).contains(&proof));
        assert!(matches!(
            f.engine.handle(request.clone()).result.unwrap(),
            ResponseBody::Ack
        ));
        assert!(matches!(
            f.engine.handle(request.clone()).result.unwrap(),
            ResponseBody::Ack
        ));
        let db = f.engine.db.lock().unwrap();
        require_grant(&db, &f.session, &f.project, Scope::PackageProject).unwrap();
        let response: String = db
            .query_row(
                "SELECT response FROM requests WHERE session=?1 AND request=?2",
                params![f.session.as_str(), request.request_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!response.contains(&proof));
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM project_package_approvals", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        drop(db);
        let mut conflict = request;
        conflict.command = Command::StartProjectExport;
        conflict.expected_revision = Some(RevisionId::new(0));
        assert_eq!(
            f.engine.handle(conflict).result.unwrap_err().code,
            ErrorCode::RequestConflict
        );
    }
    #[test]
    #[cfg(unix)]
    fn owner_capability_rejects_unsafe_modes_symlinks_and_nonregular_files() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let first = owner_token(dir.path()).unwrap();
        assert!(valid_owner_token(&first));
        assert_eq!(first, owner_token(dir.path()).unwrap());
        let path = dir.path().join("package-owner.token");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(owner_token(dir.path()).is_err());
        fs::remove_file(&path).unwrap();
        symlink(dir.path().join("missing"), &path).unwrap();
        assert!(owner_token(dir.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(owner_token(dir.path()).is_err());
    }
    #[test]
    fn retained_capture_blocks_eviction_without_freezing_later_catalog_changes() {
        let f = fixture();
        let source = f.source(b"captured bytes");
        let plan = project_packages::capture_retained(
            &f.engine,
            &f.session,
            &f.token,
            &f.project,
            RevisionId::new(0),
        )
        .unwrap();
        f.source(b"later bytes");
        assert_eq!(plan.manifest.sources.len(), 1);
        assert_eq!(
            f.call(
                Command::EvictSource {
                    source_version: source.clone()
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::Forbidden
        );
        drop(plan);
        f.call(
            Command::EvictSource {
                source_version: source,
            },
            None,
        )
        .unwrap();
    }
    #[test]
    fn start_identity_replays_and_ready_package_does_not_follow_later_source_catalog() {
        let f = fixture();
        f.source(b"first byte source");
        let (request, status) = f.ready();
        f.source(b"later source");
        match f.engine.handle(request).result.unwrap() {
            ResponseBody::PackageExport(value) => {
                assert_eq!(value.state, PackageExportOperationState::Queued)
            }
            _ => panic!(),
        }
        let artifact = status.artifact.as_ref().unwrap();
        let mut file = f
            .engine
            .package_store
            .open_readonly(&stored(artifact).unwrap())
            .unwrap();
        let decoded = crate::project_package_format::read_package(
            &mut file,
            |_| Ok(Vec::<u8>::new()),
            PackageCodecLimits {
                max_package_bytes: f.engine.config.max_snapshot_bytes,
            },
        )
        .unwrap();
        assert_eq!(decoded.manifest.sources.len(), 1);
        assert_eq!(decoded.manifest.captured_revision, RevisionId::new(0));
        assert_eq!(decoded.digest.sha256, artifact.sha256);
        assert_eq!(
            f.status(&status.operation_id).state,
            PackageExportOperationState::Ready
        );
    }
    #[test]
    fn cancellation_acknowledges_before_blocked_capture_but_never_publishes_ready() {
        let f = fixture();
        let gate = f.engine.artifact_gate.lock().unwrap();
        let initial = match f.call(Command::StartProjectExport, Some(0)).unwrap() {
            ResponseBody::PackageExport(value) => value,
            _ => panic!(),
        };
        let cancelled = match f
            .call(
                Command::CancelProjectExport {
                    operation_id: initial.operation_id.clone(),
                },
                None,
            )
            .unwrap()
        {
            ResponseBody::PackageExport(value) => value,
            _ => panic!(),
        };
        assert_eq!(cancelled.state, PackageExportOperationState::Cancelled);
        drop(gate);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f.engine.package_exports.lock().unwrap().active.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            f.status(&initial.operation_id).state,
            PackageExportOperationState::Cancelled
        );
        assert_eq!(f.engine.package_store.allocated_bytes().unwrap(), 0);
    }
    #[test]
    fn restart_uses_pending_verification_and_fences_prior_leases_and_request_bindings() {
        let f = fixture();
        let (start, status) = f.ready();
        let (_, old_lease) = f.begin(&status);
        let old_handshake = f.handshake(&old_lease);
        let f = restart(f);
        assert_eq!(
            f.status(&status.operation_id).state,
            PackageExportOperationState::Ready
        );
        assert!(f.engine.package_bulk_open(&old_handshake).is_err());
        let request = f.request(
            Command::BeginProjectPackageDownload {
                operation_id: status.operation_id.clone(),
                offset: 0,
                byte_len: status.artifact.as_ref().unwrap().byte_len,
            },
            None,
        );
        assert!(matches!(
            f.engine.handle(request.clone()).result.unwrap(),
            ResponseBody::PackageDownloadPending(_)
        ));
        let mut conflict = request.clone();
        conflict.command = Command::ReleaseProjectExport {
            operation_id: status.operation_id.clone(),
        };
        assert_eq!(
            f.engine.handle(conflict).result.unwrap_err().code,
            ErrorCode::RequestConflict
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let lease = loop {
            match f.engine.handle(request.clone()).result.unwrap() {
                ResponseBody::PackageDownload(value) => break value,
                ResponseBody::PackageDownloadPending(_) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => panic!(),
            }
        };
        let repeated = match f.engine.handle(request.clone()).result.unwrap() {
            ResponseBody::PackageDownload(value) => value,
            _ => panic!(),
        };
        assert_eq!(lease.lease_id, repeated.lease_id);
        assert!(matches!(
            f.engine.handle(start).result.unwrap(),
            ResponseBody::PackageExport(_)
        ));
        let db = f.engine.db.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM project_package_download_begins WHERE request=?1",
                [request.request_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn package_cursor_is_constant_space_beyond_4096_chunks_and_disconnect_is_not_revoke() {
        let f = fixture();
        f.source(&vec![7u8; 8000]);
        let (_, status) = f.ready();
        let (_, lease) = f.begin(&status);
        let handshake = f.handshake(&lease);
        let (mut connection, _) = f.engine.package_bulk_open(&handshake).unwrap();
        assert!(f.engine.package_bulk_open(&handshake).is_err());
        assert!(f
            .engine
            .package_bulk_read(&mut connection, &handshake, 1, 1)
            .is_err());
        for offset in 0..5000 {
            let (bytes, next) = f
                .engine
                .package_bulk_read(&mut connection, &handshake, offset, 1)
                .unwrap();
            assert_eq!(bytes.len(), 1);
            assert_eq!(next, offset + 1);
            f.engine
                .package_bulk_delivered(&connection, &handshake, offset, 1)
                .unwrap();
        }
        let replay = f
            .engine
            .package_bulk_read(&mut connection, &handshake, 4999, 1)
            .unwrap();
        assert_eq!(replay.1, 5000);
        f.engine
            .package_bulk_delivered(&connection, &handshake, 4999, 1)
            .unwrap();
        drop(connection);
        let (mut connection, view) = f.engine.package_bulk_open(&handshake).unwrap();
        assert_eq!(view.next_offset, 5000);
        assert!(serde_json::to_vec(&view).unwrap().len() < 4096);
        f.engine
            .package_bulk_read(&mut connection, &handshake, 5000, 1)
            .unwrap();
        drop(connection);
        let (_, view) = f.engine.package_bulk_open(&handshake).unwrap();
        assert_eq!(
            view.next_offset, 5000,
            "unreported delivery must not advance cursor"
        );
    }
    #[test]
    fn release_retains_receipt_and_waits_for_active_read_hold_before_unlink() {
        let f = fixture();
        let (_, status) = f.ready();
        let (_, lease) = f.begin(&status);
        let handshake = f.handshake(&lease);
        let (mut connection, _) = f.engine.package_bulk_open(&handshake).unwrap();
        let release = f.request(
            Command::ReleaseProjectExport {
                operation_id: status.operation_id.clone(),
            },
            None,
        );
        let released = match f.engine.handle(release.clone()).result.unwrap() {
            ResponseBody::PackageExport(value) => value,
            _ => panic!(),
        };
        assert_eq!(released.state, PackageExportOperationState::Released);
        assert!(released.artifact.is_some());
        assert!(f.engine.package_store.allocated_bytes().unwrap() > 0);
        assert!(f
            .engine
            .package_bulk_read(&mut connection, &handshake, 0, 1)
            .is_err());
        drop(connection);
        f.engine.package_exports.lock().unwrap().sweep();
        f.engine.collect_released_packages().unwrap();
        assert_eq!(f.engine.package_store.allocated_bytes().unwrap(), 0);
        assert!(matches!(
            f.engine.handle(release).result.unwrap(),
            ResponseBody::PackageExport(_)
        ));
        assert!(f
            .call(
                Command::BeginProjectPackageDownload {
                    operation_id: status.operation_id,
                    offset: 0,
                    byte_len: 1
                },
                None
            )
            .is_err());
    }
    #[test]
    fn durable_revoke_then_regrant_fences_chunks_before_advisory_runtime_callback() {
        let f = fixture();
        let (_, status) = f.ready();
        let (_, lease) = f.begin(&status);
        let handshake = f.handshake(&lease);
        let (mut connection, _) = f.engine.package_bulk_open(&handshake).unwrap();
        {
            // Isolate the durable transaction boundary. Runtime callbacks are
            // deliberately delayed until after a concurrent grant restoration.
            let mut db = f.engine.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            tx.execute(
                "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::PackageProject)
                ],
            )
            .unwrap();
            revoke(&tx, &f.session, &f.project).unwrap();
            tx.commit().unwrap();
            db.execute(
                "INSERT INTO grants VALUES(?1,?2,?3)",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::PackageProject)
                ],
            )
            .unwrap();
        }
        assert!(f.engine.package_bulk_read(&mut connection,&handshake,0,1).is_err(),
            "a committed revoke must permanently fence old leases even before the advisory callback runs");
    }
    #[test]
    fn new_generation_cannot_replace_a_still_draining_verification_worker() {
        let f = fixture();
        let (_, status) = f.ready();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f.engine.package_exports.lock().unwrap().active.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let old_cancel = Arc::new(AtomicBool::new(false));
        {
            let mut runtime = f.engine.package_exports.lock().unwrap();
            runtime.active.insert(
                status.operation_id.to_string(),
                ActiveOperation {
                    session: f.session.clone(),
                    project: f.project.clone(),
                    cancel: old_cancel.clone(),
                    deadline,
                    reserved_bytes: 0,
                    grant_generation: 0,
                },
            );
            runtime.verification.insert(
                status.operation_id.to_string(),
                (
                    0,
                    Verification::Pending {
                        completed: 0,
                        total: status.artifact.as_ref().unwrap().byte_len,
                    },
                ),
            );
        }
        {
            let mut db = f.engine.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            revoke(&tx, &f.session, &f.project).unwrap();
            tx.commit().unwrap();
        }
        let result = f.call(
            Command::BeginProjectPackageDownload {
                operation_id: status.operation_id.clone(),
                offset: 0,
                byte_len: 1,
            },
            None,
        );
        assert!(
            result.is_err(),
            "new verification must wait for the previous generation worker to leave its slot"
        );
        let runtime = f.engine.package_exports.lock().unwrap();
        assert!(Arc::ptr_eq(
            &runtime.active[status.operation_id.as_str()].cancel,
            &old_cancel
        ));
    }
    #[test]
    fn pending_begin_generation_cannot_resume_after_revoke_and_regrant() {
        let f = fixture();
        let (_, status) = f.ready();
        let f = restart(f);
        let request = f.request(
            Command::BeginProjectPackageDownload {
                operation_id: status.operation_id.clone(),
                offset: 0,
                byte_len: 1,
            },
            None,
        );
        assert!(matches!(
            f.engine.handle(request.clone()).result.unwrap(),
            ResponseBody::PackageDownloadPending(_)
        ));
        {
            let mut db = f.engine.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            tx.execute(
                "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::PackageProject)
                ],
            )
            .unwrap();
            revoke(&tx, &f.session, &f.project).unwrap();
            tx.commit().unwrap();
            db.execute(
                "INSERT INTO grants VALUES(?1,?2,?3)",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::PackageProject)
                ],
            )
            .unwrap();
        }
        assert!(f.engine.handle(request).result.is_err());
    }
    #[test]
    #[cfg(unix)]
    fn corrupted_ready_bytes_fail_async_restart_verification_without_lease() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        let (_, status) = f.ready();
        let path = f
            .engine
            .config
            .state_dir
            .join("project-packages")
            .join(&status.artifact.as_ref().unwrap().sha256);
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        let f = restart(f);
        let request = f.request(
            Command::BeginProjectPackageDownload {
                operation_id: status.operation_id.clone(),
                offset: 0,
                byte_len: 1,
            },
            None,
        );
        assert!(matches!(
            f.engine.handle(request.clone()).result.unwrap(),
            ResponseBody::PackageDownloadPending(_)
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match f.engine.handle(request.clone()).result {
                Err(_) => break,
                Ok(ResponseBody::PackageDownloadPending(_)) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(_) => panic!("corrupt historical Ready bytes must never produce a lease"),
            }
        }
    }

    fn durable_regrant(f: &Fixture) {
        let mut db = f.engine.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        tx.execute(
            "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
            params![
                f.session.as_str(),
                f.project.as_str(),
                scope_name(Scope::PackageProject)
            ],
        )
        .unwrap();
        revoke(&tx, &f.session, &f.project).unwrap();
        tx.commit().unwrap();
        db.execute(
            "INSERT INTO grants VALUES(?1,?2,?3)",
            params![
                f.session.as_str(),
                f.project.as_str(),
                scope_name(Scope::PackageProject)
            ],
        )
        .unwrap();
    }
    #[test]
    fn final_lease_transaction_rechecks_original_begin_generation_after_reverification() {
        let f = fixture();
        let (_, status) = f.ready();
        let engine = f.engine.clone();
        let session = f.session.clone();
        let token = f.token.clone();
        let project = f.project.clone();
        let operation = status.operation_id.clone();
        BEFORE_LEASE_ADMISSION.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                {
                    let mut db = engine.db.lock().unwrap();
                    let tx = db.transaction().unwrap();
                    tx.execute(
                        "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
                        params![
                            session.as_str(),
                            project.as_str(),
                            scope_name(Scope::PackageProject)
                        ],
                    )
                    .unwrap();
                    revoke(&tx, &session, &project).unwrap();
                    tx.commit().unwrap();
                    db.execute(
                        "INSERT INTO grants VALUES(?1,?2,?3)",
                        params![
                            session.as_str(),
                            project.as_str(),
                            scope_name(Scope::PackageProject)
                        ],
                    )
                    .unwrap();
                }
                // Complete a genuine fresh-generation verification and lease while
                // the original request is paused outside its final transaction.
                let newer = Request::new(
                    id!(RequestId),
                    Command::BeginProjectPackageDownload {
                        operation_id: operation,
                        offset: 0,
                        byte_len: 1,
                    },
                )
                .with_session(session)
                .with_auth_token(token)
                .in_project(project, None);
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    match engine.handle(newer.clone()).result {
                        Ok(ResponseBody::PackageDownload(_)) => break,
                        Ok(ResponseBody::PackageDownloadPending(_))
                        | Err(ProtocolError {
                            code: ErrorCode::ResourceExhausted,
                            ..
                        }) => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        result => panic!("fresh verification failed: {result:?}"),
                    }
                }
            }))
        });
        let original = f.request(
            Command::BeginProjectPackageDownload {
                operation_id: status.operation_id,
                offset: 0,
                byte_len: 1,
            },
            None,
        );
        assert!(
            f.engine.handle(original.clone()).result.is_err(),
            "revoked original begin cannot adopt newer verification"
        );
        let db = f.engine.db.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM requests WHERE request=?1",
                [original.request_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "revoked request must have no durable lease outcome"
        );
    }
    #[test]
    fn delayed_revoke_callback_preserves_fresh_generation_download_and_export() {
        let f = fixture();
        let (_, status) = f.ready();
        let (_, old_lease) = f.begin(&status);
        durable_regrant(&f);
        let (_, fresh_lease) = f.begin(&status);
        let fresh_handshake = f.handshake(&fresh_lease);
        let (mut connection, _) = f.engine.package_bulk_open(&fresh_handshake).unwrap();
        let gate = f.engine.artifact_gate.lock().unwrap();
        let fresh = match f.call(Command::StartProjectExport, Some(0)).unwrap() {
            ResponseBody::PackageExport(value) => value,
            _ => panic!(),
        };
        f.engine.package_revoke(&f.session, &f.project);
        assert!(
            f.engine
                .package_bulk_read(&mut connection, &fresh_handshake, 0, 1)
                .is_ok(),
            "delayed callback must not revoke a freshly authorized lease"
        );
        assert!(
            !f.engine.package_exports.lock().unwrap().active[fresh.operation_id.as_str()]
                .cancel
                .load(Ordering::Acquire)
        );
        assert!(f
            .engine
            .package_bulk_open(&f.handshake(&old_lease))
            .is_err());
        drop(gate);
    }
    #[test]
    fn export_spawn_failure_terminalizes_without_leaking_active_admission() {
        let f = fixture();
        FAIL_PACKAGE_SPAWN.with(|flag| flag.set(true));
        let request = f.request(Command::StartProjectExport, Some(0));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f.engine.handle(request.clone())
        }));
        assert!(
            result.is_ok(),
            "OS thread creation failure must not panic after durable admission"
        );
        let initial = match result.unwrap().result.unwrap() {
            ResponseBody::PackageExport(value) => value,
            _ => panic!(),
        };
        assert_eq!(
            f.status(&initial.operation_id).state,
            PackageExportOperationState::Failed
        );
        assert!(f.engine.package_exports.lock().unwrap().active.is_empty());
        assert!(matches!(
            f.engine.handle(request).result.unwrap(),
            ResponseBody::PackageExport(_)
        ));
    }
    #[test]
    fn verification_spawn_failure_releases_holds_and_preserves_pending_identity() {
        let f = fixture();
        let (_, status) = f.ready();
        let f = restart(f);
        FAIL_PACKAGE_SPAWN.with(|flag| flag.set(true));
        let request = f.request(
            Command::BeginProjectPackageDownload {
                operation_id: status.operation_id.clone(),
                offset: 0,
                byte_len: 1,
            },
            None,
        );
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f.engine.handle(request.clone())
        }));
        assert!(
            result.is_ok(),
            "verification spawn failure must not strand its admitted slot"
        );
        assert!(matches!(
            result.unwrap().result.unwrap(),
            ResponseBody::PackageDownloadPending(_)
        ));
        assert!(f.engine.package_exports.lock().unwrap().active.is_empty());
        assert!(f.engine.handle(request).result.is_err());
        assert!(!f
            .engine
            .package_holds
            .contains(&status.artifact.as_ref().unwrap().sha256)
            .unwrap());
    }
}
