use crate::{
    artifacts, launch,
    resources::{Reservation, ResourcePool},
    worker, EngineConfig,
};
use anyhow::Result;
use pulsar_protocol::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use uuid::Uuid;

type PResult<T> = std::result::Result<T, ProtocolError>;
const PROGRAM_LIMIT: usize = 512 * 1024;
const WORKER_MEMORY: u64 = 1536 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 64 * 1024 * 1024;
macro_rules! id {
    ($ty:ty) => {
        <$ty>::new(Uuid::new_v4().to_string()).expect("UUID is a checked identifier")
    };
}
#[path = "editing.rs"]
mod editing;
#[path = "lineage.rs"]
mod lineage;
#[path = "motion_state.rs"]
mod motion_state;
#[path = "source_kinds.rs"]
mod source_kinds;
#[path = "transfers.rs"]
mod transfers;
#[path = "project_packages.rs"]
mod project_packages;
#[cfg(test)]
#[path = "../tests/project_package_closure/mod.rs"]
mod project_package_closure;

fn internal(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, error.to_string())
}
fn unavailable(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Unavailable, error.to_string())
}
fn encode(value: &impl Serialize) -> PResult<String> {
    serde_json::to_string(value).map_err(internal)
}
fn decode<T: for<'de> Deserialize<'de>>(value: &str) -> PResult<T> {
    serde_json::from_str(value).map_err(internal)
}
fn rev_sql(rev: RevisionId) -> PResult<i64> {
    i64::try_from(rev.value())
        .map_err(|_| ProtocolError::invalid("revision exceeds durable storage range"))
}
fn scope_name(scope: Scope) -> String {
    serde_json::to_string(&scope).expect("scope serialization")
}

pub struct Engine {
    pub(crate) config: EngineConfig,
    db: Mutex<Connection>,
    token: String,
    pool: ResourcePool,
    tools: std::result::Result<WorkerTools, String>,
    binary_identity: ArtifactIdentity,
    artifact_gate: Mutex<()>,
    workers: Mutex<HashMap<String, Arc<AtomicBool>>>,
    motion_store: crate::motion_artifacts::ProgramStore,
    transfers: Mutex<transfers::TransferState>,
    engine_epoch: String,
}

impl Engine {
    pub fn open(mut config: EngineConfig) -> Result<Arc<Self>> {
        artifacts::private_directory(&config.state_dir)?;
        config.state_dir = config.state_dir.canonicalize()?;
        for sub in [
            "snapshots",
            "attempts",
            "motion-objects",
            "transfer-staging",
            "edit-inputs",
            "edit-receipts",
        ] {
            artifacts::private_directory(&config.state_dir.join(sub))?;
        }
        let (binary_path, binary_hash, binary_size) = artifacts::snapshot(
            &config.worker_executable,
            &config.state_dir.join("snapshots"),
            config.max_snapshot_bytes,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o500))?;
        }
        config.worker_executable = binary_path;
        let binary_identity = ArtifactIdentity {
            artifact_id: ArtifactId::new(binary_hash.clone())?,
            sha256: binary_hash,
            byte_len: binary_size,
        };
        let token_path = config.bootstrap_path();
        let token = if token_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = fs::symlink_metadata(&token_path)?;
                anyhow::ensure!(
                    !meta.file_type().is_symlink()
                        && meta.uid() == unsafe { libc::geteuid() }
                        && meta.mode() & 0o077 == 0,
                    "pairing token requires current-user ownership and mode 0600"
                );
            }
            let token = fs::read_to_string(&token_path)?;
            anyhow::ensure!(
                token.len() >= 64 && token.len() <= 256,
                "invalid bootstrap token"
            );
            token
        } else {
            let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&token_path)?;
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
            artifacts::sync_dir(&config.state_dir)?;
            token
        };
        let pool = ResourcePool::new(config.max_ram_bytes, config.max_jobs);
        let motion_store =
            crate::motion_artifacts::ProgramStore::new(&config.state_dir.join("motion-objects"))?;
        let mut db = Connection::open(config.state_dir.join("projects.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(2))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=100; PRAGMA journal_size_limit=8388608;")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(version == 0 || version == 2, "project database schema requires an explicit recoverable migration; original was not modified");
        if version == 0 {
            let tx = db.transaction()?;
            tx.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY, actor TEXT NOT NULL, label TEXT NOT NULL, token_hash TEXT NOT NULL);
                CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT NOT NULL, revision INTEGER NOT NULL, program TEXT NOT NULL, history_cursor INTEGER NOT NULL DEFAULT 0, protected TEXT NOT NULL DEFAULT '[]');
                CREATE TABLE grants(session TEXT NOT NULL REFERENCES sessions(id), project TEXT NOT NULL REFERENCES projects(id), scope TEXT NOT NULL, PRIMARY KEY(session,project,scope));
                CREATE TABLE revisions(project TEXT NOT NULL REFERENCES projects(id), revision INTEGER NOT NULL, actor TEXT NOT NULL, kind TEXT NOT NULL, label TEXT NOT NULL, program TEXT NOT NULL, protected TEXT NOT NULL DEFAULT '[]', PRIMARY KEY(project,revision));
                CREATE TABLE edit_states(project TEXT NOT NULL REFERENCES projects(id), position INTEGER NOT NULL, program TEXT NOT NULL, protected TEXT NOT NULL DEFAULT '[]', PRIMARY KEY(project,position));
                CREATE TABLE requests(session TEXT NOT NULL, request TEXT NOT NULL, fingerprint TEXT NOT NULL, response TEXT NOT NULL, PRIMARY KEY(session,request));
                CREATE TABLE sources(project TEXT NOT NULL REFERENCES projects(id), version TEXT NOT NULL, path TEXT NOT NULL, original TEXT NOT NULL, label TEXT NOT NULL, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0, evicted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(project,version));
                CREATE TABLE jobs(id TEXT PRIMARY KEY, attempt TEXT NOT NULL, project TEXT NOT NULL REFERENCES projects(id), base_revision INTEGER NOT NULL, session TEXT NOT NULL REFERENCES sessions(id), source TEXT NOT NULL, state TEXT NOT NULL, manifest TEXT NOT NULL, candidate TEXT, error TEXT, attempts INTEGER NOT NULL DEFAULT 1);
                CREATE TABLE attempts(id TEXT PRIMARY KEY, job TEXT NOT NULL REFERENCES jobs(id), manifest TEXT NOT NULL, state TEXT NOT NULL);
                CREATE TABLE candidates(id TEXT PRIMARY KEY, project TEXT NOT NULL REFERENCES projects(id), base_revision INTEGER NOT NULL, program TEXT NOT NULL, job TEXT, lineage TEXT NOT NULL, committed_revision INTEGER);
                CREATE TABLE events(cursor INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL REFERENCES projects(id), revision INTEGER NOT NULL, body TEXT NOT NULL);
                CREATE TABLE exports(session TEXT NOT NULL, request TEXT NOT NULL, project TEXT NOT NULL, revision INTEGER NOT NULL, path TEXT NOT NULL, sha256 TEXT NOT NULL, state TEXT NOT NULL, PRIMARY KEY(session,request));
                PRAGMA user_version=2;")?;
            tx.commit()?;
        }
        let has_review = {
            let mut statement = db.prepare("PRAGMA table_info(candidates)")?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            names.iter().any(|name| name == "review")
        };
        if !has_review {
            let tx = db.transaction()?;
            tx.execute(
                "ALTER TABLE candidates ADD COLUMN review TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
            tx.commit()?;
        }
        let has_export_axis = {
            let mut statement = db.prepare("PRAGMA table_info(exports)")?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            names.iter().any(|name| name == "axis")
        };
        if !has_export_axis {
            let tx = db.transaction()?;
            tx.execute(
                "ALTER TABLE exports ADD COLUMN axis TEXT NOT NULL DEFAULT 'stroke'",
                [],
            )?;
            tx.commit()?;
        }
        {
            let _migration = pool.reserve_memory(transfers::VALIDATION_MEMORY)?;
            motion_state::initialize(&mut db)?;
        }
        lineage::initialize(&mut db)?;
        transfers::initialize(&db)?;
        source_kinds::initialize(&mut db)?;
        db.execute("UPDATE jobs SET state='interrupted',error='engine restart; validated dependency restart required' WHERE state IN ('queued','running')", [])?;
        db.execute(
            "UPDATE attempts SET state='interrupted' WHERE state IN ('queued','running')",
            [],
        )?;
        let tools = worker::discover_tools()
            .and_then(|mut tools| {
                if let Some(runtime) = &mut tools.onnx_runtime {
                    let (path, hash, bytes) = artifacts::snapshot(
                        &runtime.path,
                        &config.state_dir.join("snapshots"),
                        config.max_snapshot_bytes,
                    )?;
                    runtime.path = path;
                    runtime.identity = ArtifactIdentity {
                        artifact_id: ArtifactId::new(hash.clone())?,
                        sha256: hash,
                        byte_len: bytes,
                    };
                }
                Ok(tools)
            })
            .map_err(|e| e.to_string());
        let engine = Arc::new(Self {
            config,
            db: Mutex::new(db),
            token,
            pool,
            tools,
            binary_identity,
            artifact_gate: Mutex::new(()),
            workers: Mutex::new(HashMap::new()),
            motion_store,
            transfers: Mutex::new(transfers::TransferState::default()),
            engine_epoch: Uuid::new_v4().to_string(),
        });
        transfers::start_sweeper(&engine);
        engine.recover_interrupted();
        Ok(engine)
    }

    pub fn handle(self: &Arc<Self>, request: Request) -> Response {
        let result = self.dispatch(&request);
        match result {
            Ok(body) => Response::success(request.request_id, body),
            Err(error) => Response::failure(request.request_id, error),
        }
    }

    fn dispatch(self: &Arc<Self>, request: &Request) -> PResult<ResponseBody> {
        request.validate()?;
        self.transfers.lock().map_err(internal)?.sweep();
        if let Command::Pair {
            client_name,
            pairing_token,
        } = &request.command
        {
            return self.pair(request, client_name, pairing_token);
        }
        let session = request
            .session
            .as_ref()
            .ok_or_else(|| ProtocolError::new(ErrorCode::Unauthorized, "pairing is required"))?;
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(request_fingerprint(request).map_err(internal)?)
        );
        {
            let db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, session)?;
            check_request_identity(&db, session.as_str(), request, &fingerprint)?;
            if request.command.is_mutating() {
                if let Some(body) = replay(&db, session.as_str(), request, &fingerprint)? {
                    return Ok(body);
                }
            }
        }
        if matches!(
            &request.command,
            Command::BeginEditUpload { .. }
                | Command::BeginMotionDownload { .. }
                | Command::FinishEditUpload { .. }
                | Command::TransferStatus { .. }
                | Command::AbandonTransfer { .. }
        ) {
            return self.transfer_control(request, &fingerprint);
        }
        if let Command::PreviewFrame {
            context,
            source_time,
            model_path,
            model_input,
        } = &request.command
        {
            return self.preview(
                request,
                context,
                *source_time,
                model_path.as_deref(),
                model_input.clone(),
            );
        }
        if let Command::Export { path } = &request.command {
            return self.export(request, path, &fingerprint, None);
        }
        if let Command::ExportAxis { path, axis } = &request.command {
            return self.export(request, path, &fingerprint, Some(*axis));
        }
        let _motion_admission = if matches!(
            &request.command,
            Command::ImportFunscript { .. }
                | Command::CommitCandidate { .. }
                | Command::MergeCandidate { .. }
                | Command::Undo
                | Command::Redo
                | Command::Diagnostics { .. }
                | Command::SetProtectedRegions { .. }
        ) {
            Some(self.pool.reserve_memory(transfers::VALIDATION_MEMORY)?)
        } else {
            None
        };
        let _artifact_guard = if matches!(
            &request.command,
            Command::ImportSource { .. }
                | Command::ImportFunscript { .. }
                | Command::GenerateInput { .. }
                | Command::Generate { .. }
                | Command::ResumeJob { .. }
                | Command::EvictSource { .. }
        ) {
            Some(self.artifact_gate.lock().map_err(internal)?)
        } else {
            None
        };
        // Expensive copying happens outside the database lock; authority and revision
        // are checked again in the transaction that publishes the source reference.
        let prepared_source = if let Command::ImportSource { path } = &request.command {
            Some(
                artifacts::snapshot_reserved(
                    path,
                    &self.config.state_dir.join("snapshots"),
                    self.config.max_snapshot_bytes,
                    self.pool.reserved_storage(),
                )
                .map_err(|e| ProtocolError::new(ErrorCode::ResourceExhausted, e.to_string()))?,
            )
        } else {
            None
        };
        let prepared_funscript = if let Command::ImportFunscript { path } = &request.command {
            Some(self.prepare_funscript(path)?)
        } else {
            None
        };
        let prepared_input = if let Command::GenerateInput { input } = &request.command {
            if input.source_version().is_none() {
                Some(self.materialize_input(input)?)
            } else {
                None
            }
        } else {
            None
        };
        let prepared_model = if let Command::Generate {
            model_path: Some(path),
            ..
        } = &request.command
        {
            Some(self.snapshot_model(path)?)
        } else {
            None
        };
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        self.authorize(&tx, request, session)?;
        if request.command.is_mutating() {
            if let Some(body) = replay(&tx, session.as_str(), request, &fingerprint)? {
                return Ok(body);
            }
        }
        check_revision(&tx, request)?;
        if request.command.is_mutating() {
            claim_request_identity(&tx, session.as_str(), request, &fingerprint)?;
        }
        let mut launch_work: Option<(WorkerRequest, Reservation, Arc<AtomicBool>)> = None;
        let body = match &request.command {
            Command::Capabilities => ResponseBody::Capabilities(self.capabilities()),
            Command::CreateProject { name } => {
                let project = id!(ProjectId);
                let actor = actor(&tx, session)?;
                let program = motion_state::save(&tx, &MotionProgram::default())?;
                tx.execute(
                    "INSERT INTO projects(id,name,revision,program) VALUES(?1,?2,0,?3)",
                    params![project.as_str(), name, program],
                )
                .map_err(internal)?;
                tx.execute(
                    "INSERT INTO revisions VALUES(?1,0,?2,'create','Create project',?3,'[]')",
                    params![project.as_str(), actor, program],
                )
                .map_err(internal)?;
                tx.execute(
                    "INSERT INTO edit_states VALUES(?1,0,?2,'[]')",
                    params![project.as_str(), program],
                )
                .map_err(internal)?;
                for scope in [
                    Scope::Read,
                    Scope::Edit,
                    Scope::Generate,
                    Scope::Export,
                    Scope::ManageGrants,
                    Scope::ManageProtection,
                    Scope::ImportSource,
                    Scope::PackageProject,
                ] {
                    tx.execute(
                        "INSERT INTO grants VALUES(?1,?2,?3)",
                        params![session.as_str(), project.as_str(), scope_name(scope)],
                    )
                    .map_err(internal)?;
                }
                lineage::created(&tx, &project)?;
                event(&tx, &project, RevisionId::new(0), EventBody::ProjectChanged)?;
                ResponseBody::Project(project_snapshot(&tx, &project)?)
            }
            Command::OpenProject { project_id } => {
                ResponseBody::Project(project_snapshot(&tx, project_id)?)
            }
            Command::GetSnapshot => {
                ResponseBody::Project(project_snapshot(&tx, project(request)?)?)
            }
            Command::ImportSource { path } => {
                let (snapshot, hash, bytes) = prepared_source.as_ref().expect("prepared import");
                let source_version = SourceVersionId::new(hash.clone()).map_err(internal)?;
                let label = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let count: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM sources WHERE project=?1",
                        [project(request)?.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                if count >= 128 {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceExhausted,
                        "project source catalog reached enabled control-path admission limit",
                    ));
                }
                tx.execute("INSERT INTO sources(project,version,path,original,label,sha256,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(project,version) DO UPDATE SET evicted=0,path=excluded.path",
                    params![project(request)?.as_str(),source_version.as_str(),snapshot.to_string_lossy(),path.to_string_lossy(),label,hash,*bytes as i64]).map_err(internal)?;
                let snapshot = project_snapshot(&tx, project(request)?)?;
                event(
                    &tx,
                    &snapshot.project_id,
                    snapshot.revision,
                    EventBody::ProjectChanged,
                )?;
                ResponseBody::Source { source_version }
            }
            Command::ImportFunscript { .. } => self.import_funscript(
                &tx,
                request,
                prepared_funscript.expect("prepared funscript"),
            )?,
            Command::SetProtectedRegions { regions } => {
                if regions.len() > 1024 {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceExhausted,
                        "too many protected regions",
                    ));
                }
                let state = project_state(&tx, project(request)?)?;
                let revision = state.revision.checked_next().map_err(internal)?;
                tx.execute(
                    "UPDATE projects SET protected=?2,revision=?3 WHERE id=?1",
                    params![
                        state.project_id.as_str(),
                        encode(regions)?,
                        rev_sql(revision)?
                    ],
                )
                .map_err(internal)?;
                tx.execute("INSERT INTO revisions VALUES(?1,?2,?3,'protection','Set protected regions',?4,?5)",params![state.project_id.as_str(),rev_sql(revision)?,actor(&tx,session)?,motion_state::register(&tx, &state.motion.program)?,encode(regions)?]).map_err(internal)?;
                append_history(
                    &tx,
                    &state.project_id,
                    &motion_state::register(&tx, &state.motion.program)?,
                    &encode(regions)?,
                )?;
                lineage::record(
                    &tx,
                    request,
                    revision,
                    state.revision,
                    &state.program,
                    &state.program,
                    true,
                )?;
                event(&tx, &state.project_id, revision, EventBody::ProjectChanged)?;
                ResponseBody::Project(project_snapshot(&tx, &state.project_id)?)
            }
            Command::PinSource {
                source_version,
                pinned,
            } => {
                let source = self.source(&tx, project(request)?, source_version)?;
                tx.execute(
                    "UPDATE sources SET pinned=?3 WHERE project=?1 AND version=?2",
                    params![
                        project(request)?.as_str(),
                        source.source_version.as_str(),
                        pinned
                    ],
                )
                .map_err(internal)?;
                ResponseBody::Ack
            }
            Command::EvictSource { source_version } => {
                let source = self.source(&tx, project(request)?, source_version)?;
                let protected:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sources WHERE version=?1 AND pinned=1) OR EXISTS(SELECT 1 FROM jobs WHERE source=?1 AND state IN ('queued','running','interrupted'))",[source_version.as_str()],|r|r.get(0)).map_err(internal)?;
                let preview_active = self.workers.lock().map_err(internal)?.keys().any(|key| {
                    key.starts_with("preview:")
                        && key.contains(&format!(":{}:", source_version.as_str()))
                });
                let runtime_pinned = self
                    .tools
                    .as_ref()
                    .ok()
                    .and_then(|tools| tools.onnx_runtime.as_ref())
                    .is_some_and(|runtime| runtime.identity.sha256 == source.identity.sha256);
                let mut manifests=tx.prepare("SELECT manifest FROM jobs WHERE state IN ('queued','running','interrupted')").map_err(internal)?;
                let active = manifests
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(internal)?;
                let mut dependency_pinned = false;
                for raw in active {
                    let manifest: WorkerRequest = decode(&raw.map_err(internal)?)?;
                    if manifest
                        .dependencies
                        .iter()
                        .any(|d| d.sha256 == source.identity.sha256)
                    {
                        dependency_pinned = true;
                        break;
                    }
                }
                if protected
                    || preview_active
                    || runtime_pinned
                    || dependency_pinned
                    || source.identity.sha256 == self.binary_identity.sha256
                {
                    return Err(ProtocolError::new(
                        ErrorCode::Forbidden,
                        "snapshot is pinned or required by active work",
                    ));
                }
                // Metadata is retained. An absent snapshot is an explicit dependency
                // failure, never permission to consume a changed original.
                fs::remove_file(&source.path).map_err(unavailable)?;
                artifacts::sync_dir(&self.config.state_dir.join("snapshots"))
                    .map_err(unavailable)?;
                tx.execute(
                    "UPDATE sources SET evicted=1 WHERE version=?1",
                    [source_version.as_str()],
                )
                .map_err(internal)?;
                ResponseBody::Ack
            }
            Command::Undo | Command::Redo => {
                let delta: i64 = if matches!(request.command, Command::Undo) {
                    -1
                } else {
                    1
                };
                let current: i64 = tx
                    .query_row(
                        "SELECT history_cursor FROM projects WHERE id=?1",
                        [project(request)?.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                let target = current
                    .checked_add(delta)
                    .ok_or_else(|| ProtocolError::invalid("history position overflow"))?;
                let entry: Option<(String,String)> = tx.query_row("SELECT program,protected FROM edit_states WHERE project=?1 AND position=?2", params![project(request)?.as_str(),target], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(internal)?;
                let (program, protected) = entry.ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::NotFound,
                        "no history entry in requested direction",
                    )
                })?;
                let existing: String = tx
                    .query_row(
                        "SELECT protected FROM projects WHERE id=?1",
                        [project(request)?.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                if existing != protected {
                    require_grant(&tx, session, project(request)?, Scope::ManageProtection)?;
                }
                let program = motion_state::load(&tx, &program)?;
                commit_program(
                    &tx,
                    request,
                    session,
                    &program,
                    if delta < 0 { "undo" } else { "redo" },
                    "Shared project history",
                    false,
                )?;
                tx.execute(
                    "UPDATE projects SET history_cursor=?2,protected=?3 WHERE id=?1",
                    params![project(request)?.as_str(), target, protected],
                )
                .map_err(internal)?;
                tx.execute("UPDATE revisions SET protected=?2 WHERE project=?1 AND revision=(SELECT revision FROM projects WHERE id=?1)",params![project(request)?.as_str(),protected]).map_err(internal)?;
                ResponseBody::Project(project_snapshot(&tx, project(request)?)?)
            }
            Command::Generate {
                source_version,
                preset,
                settings,
                ..
            } => {
                if !["fast", "default", "quality", "maximum"].contains(&preset.as_str()) {
                    return Err(ProtocolError::unsupported("generation preset"));
                }
                let (manifest, reservation, cancel) = self.prepare_job(
                    &tx,
                    request,
                    source_version,
                    preset,
                    settings.clone().unwrap_or_default(),
                    prepared_model,
                    None,
                )?;
                let snapshot = job_snapshot(&tx, &manifest.job_id, project(request)?)?;
                launch_work = Some((manifest, reservation, cancel));
                ResponseBody::Job(snapshot)
            }
            Command::GenerateInput { input } => {
                let source_version = if let Some(version) = input.source_version() {
                    version.clone()
                } else {
                    let source = prepared_input.expect("materialized generation input");
                    let count: i64 = tx
                        .query_row(
                            "SELECT COUNT(*) FROM sources WHERE project=?1",
                            [project(request)?.as_str()],
                            |row| row.get(0),
                        )
                        .map_err(internal)?;
                    if count >= 128 {
                        return Err(ProtocolError::new(
                            ErrorCode::ResourceExhausted,
                            "project source catalog is full",
                        ));
                    }
                    tx.execute("INSERT INTO sources(project,version,path,original,label,sha256,bytes,kind) VALUES(?1,?2,?3,'','Generation input',?4,?5,'generation_input') ON CONFLICT(project,version) DO UPDATE SET evicted=0,path=excluded.path,kind=excluded.kind",
                        params![project(request)?.as_str(),source.source_version.as_str(),source.path.to_string_lossy(),source.identity.sha256,source.identity.byte_len as i64]).map_err(internal)?;
                    source.source_version
                };
                let (manifest, reservation, cancel) = self.prepare_operation(
                    &tx,
                    request,
                    &source_version,
                    WorkerOperation::GenerateInput {
                        input: input.clone(),
                    },
                    None,
                )?;
                let snapshot = job_snapshot(&tx, &manifest.job_id, project(request)?)?;
                launch_work = Some((manifest, reservation, cancel));
                ResponseBody::Job(snapshot)
            }
            Command::JobStatus { job_id } => {
                ResponseBody::Job(job_snapshot(&tx, job_id, project(request)?)?)
            }
            Command::CancelJob { job_id } => {
                let old = job_snapshot(&tx, job_id, project(request)?)?;
                if ["queued", "running", "interrupted"].contains(&old.state.as_str()) {
                    tx.execute("UPDATE jobs SET state='cancelled',error='cancelled by authorized client' WHERE id=?1", [job_id.as_str()]).map_err(internal)?;
                    tx.execute(
                        "UPDATE attempts SET state='cancelled' WHERE id=?1",
                        [old.attempt_id.as_str()],
                    )
                    .map_err(internal)?;
                    if let Some(flag) = self.workers.lock().map_err(internal)?.get(job_id.as_str())
                    {
                        flag.store(true, Ordering::Release);
                    }
                }
                ResponseBody::Job(job_snapshot(&tx, job_id, project(request)?)?)
            }
            Command::ResumeJob { job_id } => {
                let old = job_snapshot(&tx, job_id, project(request)?)?;
                if old.state != "interrupted" {
                    return Err(ProtocolError::invalid("only interrupted jobs can resume"));
                }
                let raw: String = tx
                    .query_row(
                        "SELECT manifest FROM jobs WHERE id=?1",
                        [job_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                let previous: WorkerRequest = decode(&raw)?;
                self.validate_dependencies(&previous)?;
                self.validate_checkpoint(&previous)?;
                if matches!(&previous.operation, WorkerOperation::Preview { .. }) {
                    return Err(ProtocolError::unsupported("preview job resume"));
                }
                let (manifest, reservation, cancel) = self.prepare_operation(
                    &tx,
                    request,
                    &previous.source.source_version,
                    previous.operation.clone(),
                    Some(&previous),
                )?;
                let snapshot = job_snapshot(&tx, &manifest.job_id, project(request)?)?;
                launch_work = Some((manifest, reservation, cancel));
                ResponseBody::Job(snapshot)
            }
            Command::MergeCandidate {
                candidate_id,
                axes,
                range,
            } => self.merge_candidate(&tx, request, session, candidate_id, axes, *range)?,
            Command::Diagnostics { candidate_id } => {
                self.diagnostics(&tx, request, candidate_id.as_ref())?
            }
            Command::GetCandidate { candidate_id } => {
                ResponseBody::Candidate(candidate_snapshot(&tx, candidate_id, project(request)?)?)
            }
            Command::RebaseCandidate { candidate_id } => {
                let old = candidate_snapshot(&tx, candidate_id, project(request)?)?;
                let project = project_snapshot(&tx, project(request)?)?;
                let new_id = id!(CandidateId);
                let lineage: String = tx
                    .query_row(
                        "SELECT lineage FROM candidates WHERE id=?1",
                        [candidate_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                tx.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![new_id.as_str(),project.project_id.as_str(),rev_sql(project.revision)?,motion_state::register(&tx,&old.motion.program)?,old.job_id.as_ref().map(|j|j.as_str()),lineage,encode(&old.review)?]).map_err(internal)?;
                tx.execute("INSERT INTO edit_candidate_lineage SELECT ?1,receipt,review_state,authored_count,inherited_count FROM edit_candidate_lineage WHERE candidate=?2",params![new_id.as_str(),candidate_id.as_str()]).map_err(internal)?;
                ResponseBody::Candidate(candidate_snapshot(&tx, &new_id, &project.project_id)?)
            }
            Command::CommitCandidate { candidate_id } => {
                let candidate = candidate_state(&tx, candidate_id, project(request)?)?;
                let state = project_snapshot(&tx, project(request)?)?;
                if candidate.base_revision != state.revision {
                    return Err(ProtocolError::new(ErrorCode::RevisionConflict,"candidate is stale; explicitly rebase to a new candidate before committing"));
                }
                let committed: Option<i64> = tx
                    .query_row(
                        "SELECT committed_revision FROM candidates WHERE id=?1",
                        [candidate_id.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                if committed.is_some() {
                    return Err(ProtocolError::new(
                        ErrorCode::RequestConflict,
                        "candidate already committed",
                    ));
                }
                let revision = commit_program(
                    &tx,
                    request,
                    session,
                    &candidate.program,
                    "candidate",
                    "Commit generated candidate",
                    true,
                )?;
                tx.execute(
                    "UPDATE candidates SET committed_revision=?2 WHERE id=?1",
                    params![candidate_id.as_str(), rev_sql(revision)?],
                )
                .map_err(internal)?;
                ResponseBody::Project(project_snapshot(&tx, project(request)?)?)
            }
            Command::Grant {
                session: target,
                project_id,
                scopes,
            } => {
                actor(&tx, target)?;
                for scope in scopes {
                    require_grant(&tx, session, project_id, *scope)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO grants VALUES(?1,?2,?3)",
                        params![target.as_str(), project_id.as_str(), scope_name(*scope)],
                    )
                    .map_err(internal)?;
                }
                ResponseBody::Ack
            }
            Command::Revoke {
                session: target,
                project_id,
            } => {
                tx.execute(
                    "DELETE FROM grants WHERE session=?1 AND project=?2",
                    params![target.as_str(), project_id.as_str()],
                )
                .map_err(internal)?;
                let mut stmt = tx.prepare("SELECT id FROM jobs WHERE session=?1 AND project=?2 AND state IN ('queued','running','interrupted')").map_err(internal)?;
                let ids = stmt
                    .query_map(params![target.as_str(), project_id.as_str()], |r| {
                        r.get::<_, String>(0)
                    })
                    .map_err(internal)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(internal)?;
                for job in ids {
                    tx.execute("UPDATE jobs SET state='cancelled',error='authorizing grant revoked' WHERE id=?1",[&job]).map_err(internal)?;
                    tx.execute("UPDATE attempts SET state='cancelled' WHERE job=?1 AND state IN ('queued','running')",[&job]).map_err(internal)?;
                    if let Some(flag) = self.workers.lock().map_err(internal)?.get(&job) {
                        flag.store(true, Ordering::Release);
                    }
                }
                let prefix = format!("preview:{}:{}:", project_id.as_str(), target.as_str());
                for (key, flag) in self.workers.lock().map_err(internal)?.iter() {
                    if key.starts_with(&prefix) {
                        flag.store(true, Ordering::Release);
                    }
                }
                let state = project_snapshot(&tx, project_id)?;
                event(
                    &tx,
                    project_id,
                    state.revision,
                    EventBody::GrantRevoked {
                        session: target.clone(),
                    },
                )?;
                ResponseBody::Ack
            }
            Command::Events {
                after_cursor,
                limit,
            } => {
                let cursor = i64::try_from(after_cursor.unwrap_or(0))
                    .map_err(|_| ProtocolError::invalid("event cursor overflow"))?;
                let mut stmt = tx.prepare("SELECT cursor,revision,body FROM events WHERE project=?1 AND cursor>?2 ORDER BY cursor LIMIT ?3").map_err(internal)?;
                let rows = stmt
                    .query_map(
                        params![project(request)?.as_str(), cursor, (*limit).clamp(1, 256)],
                        |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .map_err(internal)?;
                let mut events = Vec::new();
                for row in rows {
                    let (cursor, revision, body) = row.map_err(internal)?;
                    events.push(Event {
                        version: PROTOCOL_VERSION,
                        cursor: cursor as u64,
                        project_id: project(request)?.clone(),
                        revision: RevisionId::new(revision as u64),
                        body: decode(&body)?,
                    });
                }
                let next_cursor = events.last().map(|e| e.cursor).unwrap_or(cursor as u64);
                ResponseBody::Events(EventPage {
                    events,
                    next_cursor,
                    resync_required: false,
                })
            }
            Command::Arm { .. } | Command::Stop { .. } | Command::SwitchPlaybackRevision { .. } => {
                return Err(ProtocolError::unsupported(
                    "unqualified physical device effects",
                ))
            }
            _ => return Err(ProtocolError::unsupported("operation")),
        };
        if request.command.is_mutating() {
            remember(&tx, session.as_str(), request, &fingerprint, &body)?;
        }
        tx.commit().map_err(internal)?;
        drop(db);
        if let Command::Revoke {
            session: target,
            project_id,
        } = &request.command
        {
            self.transfers
                .lock()
                .map_err(internal)?
                .revoke(target, project_id);
        }
        if let Some((manifest, reservation, cancel)) = launch_work {
            let engine = self.clone();
            std::thread::spawn(move || {
                let _reservation = reservation;
                engine.run_job(manifest, cancel);
            });
        }
        Ok(body)
    }

    fn pair(&self, request: &Request, name: &str, supplied: &str) -> PResult<ResponseBody> {
        let mismatch = supplied
            .as_bytes()
            .iter()
            .zip(self.token.as_bytes())
            .fold(0u8, |a, (b, c)| a | (b ^ c));
        if supplied.len() != self.token.len() || mismatch != 0 {
            return Err(ProtocolError::new(
                ErrorCode::Unauthorized,
                "invalid pairing credential",
            ));
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(request_fingerprint(request).map_err(internal)?)
        );
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db.transaction().map_err(internal)?;
        if let Some(body) = replay(&tx, "pair", request, &fingerprint)? {
            return Ok(body);
        }
        let session = id!(SessionId);
        let actor = id!(ActorId);
        let auth_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let token_hash = format!("{:x}", Sha256::digest(auth_token.as_bytes()));
        tx.execute(
            "INSERT INTO sessions VALUES(?1,?2,?3,?4)",
            params![session.as_str(), actor.as_str(), name, token_hash],
        )
        .map_err(internal)?;
        let body = ResponseBody::Paired {
            session,
            auth_token,
        };
        remember(&tx, "pair", request, &fingerprint, &body)?;
        tx.commit().map_err(internal)?;
        Ok(body)
    }

    fn authenticate(
        &self,
        db: &Connection,
        session: &SessionId,
        token: Option<&str>,
    ) -> PResult<()> {
        let supplied = token.ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::Unauthorized,
                "session authentication token required",
            )
        })?;
        let expected: Option<String> = db
            .query_row(
                "SELECT token_hash FROM sessions WHERE id=?1",
                [session.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)?;
        let actual = format!("{:x}", Sha256::digest(supplied.as_bytes()));
        if expected.as_ref().is_none_or(|expected| {
            expected.len() != actual.len()
                || expected
                    .bytes()
                    .zip(actual.bytes())
                    .fold(0u8, |sum, (a, b)| sum | (a ^ b))
                    != 0
        }) {
            return Err(ProtocolError::new(
                ErrorCode::Unauthorized,
                "invalid session authentication token",
            ));
        }
        actor(db, session)?;
        Ok(())
    }

    fn authorize(&self, db: &Connection, request: &Request, session: &SessionId) -> PResult<()> {
        self.authenticate(db, session, request.auth_token.as_deref())?;
        let (project, scope) = match &request.command {
            Command::CreateProject { .. }
            | Command::Capabilities
            | Command::TransferStatus { .. }
            | Command::AbandonTransfer { .. } => return Ok(()),
            Command::OpenProject { project_id } => (project_id, Scope::Read),
            Command::Grant { project_id, .. } | Command::Revoke { project_id, .. } => {
                (project_id, Scope::ManageGrants)
            }
            Command::Generate { .. }
            | Command::GenerateInput { .. }
            | Command::CancelJob { .. }
            | Command::ResumeJob { .. } => (project(request)?, Scope::Generate),
            Command::Export { .. } | Command::ExportAxis { .. } => {
                (project(request)?, Scope::Export)
            }
            Command::SetProtectedRegions { .. } => (project(request)?, Scope::ManageProtection),
            Command::ImportSource { .. } | Command::ImportFunscript { .. } => {
                (project(request)?, Scope::ImportSource)
            }
            Command::BeginEditUpload { .. }
            | Command::FinishEditUpload { .. }
            | Command::PinSource { .. }
            | Command::EvictSource { .. }
            | Command::Undo
            | Command::Redo
            | Command::CommitCandidate { .. }
            | Command::MergeCandidate { .. }
            | Command::RebaseCandidate { .. } => (project(request)?, Scope::Edit),
            _ => (project(request)?, Scope::Read),
        };
        require_grant(db, session, project, scope)?;
        if matches!(&request.command, Command::ImportFunscript { .. }) {
            require_grant(db, session, project, Scope::Edit)?;
        }
        if matches!(
            &request.command,
            Command::Generate {
                model_path: Some(_),
                ..
            } | Command::PreviewFrame {
                model_path: Some(_),
                ..
            }
        ) {
            require_grant(db, session, project, Scope::ImportSource)?;
        }
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        let enabled = self.tools.is_ok() && launch::available();
        Capabilities {
            protocol_version: PROTOCOL_VERSION,
            operations: [
                "create_project",
                "open_project",
                "import_source",
                "import_funscript",
                "generate_input",
                "merge_candidate",
                "diagnostics",
                "begin_edit_upload",
                "finish_edit_upload",
                "begin_motion_download",
                "transfer_status",
                "abandon_transfer",
                "undo",
                "redo",
                "generate",
                "job_status",
                "cancel_job",
                "resume_job",
                "get_candidate",
                "commit_candidate",
                "rebase_candidate",
                "get_snapshot",
                "preview_frame",
                "export",
                "export_axis",
                "grant",
                "revoke",
                "events",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            backends: vec![CapabilityStatus {
                name: "isolated_cpu_analysis".into(),
                available: enabled,
                qualification: if enabled {
                    QualificationStatus::Unqualified
                } else {
                    QualificationStatus::Unavailable
                },
                reason: if enabled {
                    Some(
                        "Architecture path only; model quality and performance are not qualified"
                            .into(),
                    )
                } else {
                    Some(
                        self.tools
                            .as_ref()
                            .err()
                            .cloned()
                            .unwrap_or_else(|| "qualified worker confinement unavailable".into()),
                    )
                },
            }],
            physical_playback: false,
        }
    }

    fn source(
        &self,
        db: &Connection,
        project: &ProjectId,
        version: &SourceVersionId,
    ) -> PResult<SourceArtifact> {
        let row: Option<(String, String, i64, i64)> = db
            .query_row(
                "SELECT path,sha256,bytes,evicted FROM sources WHERE project=?1 AND version=?2",
                params![project.as_str(), version.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(internal)?;
        let (path, hash, bytes, evicted) = row.ok_or_else(|| {
            ProtocolError::new(ErrorCode::NotFound, "source not imported into this project")
        })?;
        if evicted != 0 {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "source snapshot evicted; reimport matching content before execution",
            ));
        }
        Ok(SourceArtifact {
            source_version: version.clone(),
            path: PathBuf::from(path),
            identity: ArtifactIdentity {
                artifact_id: ArtifactId::new(hash.clone()).map_err(internal)?,
                sha256: hash,
                byte_len: bytes as u64,
            },
        })
    }

    fn snapshot_model(&self, path: &Path) -> PResult<SourceArtifact> {
        let (path, hash, bytes) = artifacts::snapshot_reserved(
            path,
            &self.config.state_dir.join("snapshots"),
            self.config.max_snapshot_bytes,
            self.pool.reserved_storage(),
        )
        .map_err(unavailable)?;
        Ok(SourceArtifact {
            source_version: SourceVersionId::new(hash.clone()).map_err(internal)?,
            path,
            identity: ArtifactIdentity {
                artifact_id: ArtifactId::new(hash.clone()).map_err(internal)?,
                sha256: hash,
                byte_len: bytes,
            },
        })
    }

    fn prepare_job(
        &self,
        tx: &Transaction<'_>,
        request: &Request,
        source: &SourceVersionId,
        preset: &str,
        settings: GenerationSettings,
        model: Option<SourceArtifact>,
        previous: Option<&WorkerRequest>,
    ) -> PResult<(WorkerRequest, Reservation, Arc<AtomicBool>)> {
        self.prepare_operation(
            tx,
            request,
            source,
            WorkerOperation::Generate {
                preset: preset.to_string(),
                settings,
                model,
            },
            previous,
        )
    }

    fn prepare_operation(
        &self,
        tx: &Transaction<'_>,
        request: &Request,
        source: &SourceVersionId,
        operation: WorkerOperation,
        previous: Option<&WorkerRequest>,
    ) -> PResult<(WorkerRequest, Reservation, Arc<AtomicBool>)> {
        if matches!(&operation, WorkerOperation::Generate { .. })
            || matches!(&operation, WorkerOperation::GenerateInput { input } if input.source_version().is_some())
        {
            source_kinds::require_media(tx, project(request)?, source)?;
        }
        if !launch::available() {
            return Err(ProtocolError::unsupported("qualified worker confinement"));
        }
        let offline: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state IN ('queued','running')",
                [],
                |r| r.get(0),
            )
            .map_err(internal)?;
        if offline as usize >= self.config.max_jobs.saturating_sub(1) {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "offline work cannot consume reserved interactive worker slot",
            ));
        }
        let tools = self.tools.as_ref().map_err(unavailable)?.clone();
        let reservation = self.pool.reserve(WORKER_MEMORY)?.with_storage(
            OUTPUT_LIMIT + 16 * 1024,
            fs2::available_space(&self.config.state_dir).map_err(internal)?,
        )?;
        if fs2::available_space(&self.config.state_dir).map_err(internal)?
            < OUTPUT_LIMIT + 16 * 1024 * 1024
        {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "insufficient attempt storage",
            ));
        }
        let project = project(request)?;
        let snapshot = project_snapshot(tx, project)?;
        let job_id = previous
            .map(|p| p.job_id.clone())
            .unwrap_or_else(|| id!(JobId));
        let attempt_id = id!(AttemptId);
        let output_dir = self
            .config
            .state_dir
            .join("attempts")
            .join(attempt_id.as_str());
        artifacts::private_directory(&output_dir).map_err(internal)?;
        let mut manifest = WorkerRequest {
            version: PROTOCOL_VERSION,
            attempt_id: attempt_id.clone(),
            job_id: job_id.clone(),
            project_id: project.clone(),
            base_revision: previous
                .map(|p| p.base_revision)
                .unwrap_or(snapshot.revision),
            source: self.source(tx, project, source)?,
            output_dir,
            operation,
            budget: ResourceBudget {
                memory_bytes: WORKER_MEMORY,
                output_bytes: OUTPUT_LIMIT,
                wall_time_ms: self.config.worker_timeout.as_millis().min(u64::MAX as u128) as u64,
                cpu_threads: 2,
            },
            dependencies: vec![
                tools.ffmpeg.identity.clone(),
                tools.ffprobe.identity.clone(),
            ],
            tools,
        };
        manifest.dependencies.push(self.binary_identity.clone());
        if let Some(runtime) = &manifest.tools.onnx_runtime {
            manifest.dependencies.push(runtime.identity.clone());
        }
        if let WorkerOperation::Generate {
            model: Some(model), ..
        } = &manifest.operation
        {
            manifest.dependencies.push(model.identity.clone());
        }
        manifest.validate()?;
        let raw = encode(&manifest)?;
        // This checkpoint is the complete immutable input boundary, not an
        // unchecked partial decoder/tracker state. Recovery starts here.
        let checkpoint_path = manifest.output_dir.join("initial-checkpoint.json");
        artifacts::publish_export(&checkpoint_path, raw.as_bytes()).map_err(unavailable)?;
        let mut permissions = fs::metadata(&checkpoint_path)
            .map_err(unavailable)?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&checkpoint_path, permissions).map_err(unavailable)?;
        if previous.is_some() {
            let count: i64 = tx
                .query_row(
                    "SELECT attempts FROM jobs WHERE id=?1",
                    [job_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(internal)?;
            if count >= 3 {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "bounded attempt retry limit reached",
                ));
            }
            tx.execute("UPDATE jobs SET attempt=?2,state='queued',manifest=?3,error=NULL,attempts=attempts+1 WHERE id=?1",params![job_id.as_str(),attempt_id.as_str(),raw]).map_err(internal)?;
        } else {
            tx.execute("INSERT INTO jobs(id,attempt,project,base_revision,session,source,state,manifest) VALUES(?1,?2,?3,?4,?5,?6,'queued',?7)",params![job_id.as_str(),attempt_id.as_str(),project.as_str(),rev_sql(manifest.base_revision)?,request.session.as_ref().unwrap().as_str(),source.as_str(),raw]).map_err(internal)?;
        }
        tx.execute(
            "INSERT INTO attempts VALUES(?1,?2,?3,'queued')",
            params![attempt_id.as_str(), job_id.as_str(), raw],
        )
        .map_err(internal)?;
        let cancel = Arc::new(AtomicBool::new(false));
        event(
            tx,
            project,
            snapshot.revision,
            EventBody::JobChanged(job_snapshot(tx, &job_id, project)?),
        )?;
        Ok((manifest, reservation, cancel))
    }

    fn validate_dependencies(&self, manifest: &WorkerRequest) -> PResult<()> {
        let tools = self.tools.as_ref().map_err(unavailable)?;
        if manifest.tools.ffmpeg.identity.sha256 != tools.ffmpeg.identity.sha256
            || manifest.tools.ffprobe.identity.sha256 != tools.ffprobe.identity.sha256
        {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "worker tool dependencies changed; new job required",
            ));
        }
        if manifest.tools.onnx_runtime.as_ref().map(|r| &r.identity)
            != tools.onnx_runtime.as_ref().map(|r| &r.identity)
            || !manifest.dependencies.contains(&self.binary_identity)
        {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "runtime or executable dependency changed; new job required",
            ));
        }
        let (hash, size) =
            artifacts::digest_file(&manifest.source.path, self.config.max_snapshot_bytes)
                .map_err(unavailable)?;
        if hash != manifest.source.identity.sha256 || size != manifest.source.identity.byte_len {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "immutable source identity mismatch",
            ));
        }
        let model = match &manifest.operation {
            WorkerOperation::Generate { model, .. } | WorkerOperation::Preview { model, .. } => {
                model.as_ref()
            }
            WorkerOperation::GenerateInput { .. } => None,
        };
        if let Some(model) = model {
            let (hash, size) = artifacts::digest_file(&model.path, self.config.max_snapshot_bytes)
                .map_err(unavailable)?;
            if hash != model.identity.sha256 || size != model.identity.byte_len {
                return Err(ProtocolError::new(
                    ErrorCode::DependencyMismatch,
                    "model dependency changed",
                ));
            }
        }
        Ok(())
    }

    fn validate_checkpoint(&self, manifest: &WorkerRequest) -> PResult<()> {
        let path = manifest.output_dir.join("initial-checkpoint.json");
        let (hash, size) = artifacts::digest_file(&path, MAX_CONTROL_BYTES as u64)
            .map_err(|e| ProtocolError::new(ErrorCode::DependencyMismatch, e.to_string()))?;
        let expected = encode(manifest)?;
        if size != expected.len() as u64
            || hash != format!("{:x}", Sha256::digest(expected.as_bytes()))
        {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "checkpoint does not match exact persisted attempt dependencies/configuration",
            ));
        }
        Ok(())
    }

    fn recover_interrupted(self: &Arc<Self>) {
        let result = (|| -> PResult<Vec<(WorkerRequest, Reservation, Arc<AtomicBool>)>> {
            let _artifact_guard = self.artifact_gate.lock().map_err(internal)?;
            let mut db = self.db.lock().map_err(internal)?;
            let pending = {
                let mut stmt=db.prepare("SELECT manifest,session FROM jobs WHERE state='interrupted' ORDER BY rowid LIMIT 16").map_err(internal)?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map_err(internal)?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(internal)?
            };
            let mut launches = Vec::new();
            for (raw, owner) in pending {
                let manifest: WorkerRequest = decode(&raw)?;
                let owner = SessionId::new(owner).map_err(internal)?;
                if self
                    .validate_dependencies(&manifest)
                    .and_then(|_| self.validate_checkpoint(&manifest))
                    .is_err()
                {
                    continue;
                }
                if require_grant(&db, &owner, &manifest.project_id, Scope::Generate).is_err() {
                    continue;
                }
                if matches!(&manifest.operation, WorkerOperation::Preview { .. }) {
                    continue;
                }
                let request = Request::new(
                    id!(RequestId),
                    Command::ResumeJob {
                        job_id: manifest.job_id.clone(),
                    },
                )
                .with_session(owner)
                .in_project(manifest.project_id.clone(), None);
                let tx = db.transaction().map_err(internal)?;
                match self.prepare_operation(
                    &tx,
                    &request,
                    &manifest.source.source_version,
                    manifest.operation.clone(),
                    Some(&manifest),
                ) {
                    Ok(work) => {
                        tx.commit().map_err(internal)?;
                        launches.push(work);
                    }
                    Err(error) if error.code == ErrorCode::ResourceExhausted => break,
                    Err(_) => continue,
                }
            }
            Ok(launches)
        })();
        if let Ok(launches) = result {
            for (manifest, reservation, cancel) in launches {
                let engine = self.clone();
                std::thread::spawn(move || {
                    let _reservation = reservation;
                    engine.run_job(manifest, cancel);
                });
            }
        }
    }

    fn run_job(self: &Arc<Self>, manifest: WorkerRequest, cancel: Arc<AtomicBool>) {
        let outcome = (|| -> PResult<WorkerOutput> {
            {
                let db = self.db.lock().map_err(internal)?;
                let changed=db.execute("UPDATE jobs SET state='running' WHERE id=?1 AND attempt=?2 AND state='queued'",params![manifest.job_id.as_str(),manifest.attempt_id.as_str()]).map_err(internal)?;
                if changed != 1 {
                    return Err(ProtocolError::new(
                        ErrorCode::Cancelled,
                        "job cancelled before dispatch",
                    ));
                }
                self.workers
                    .lock()
                    .map_err(internal)?
                    .insert(manifest.job_id.to_string(), cancel.clone());
                db.execute(
                    "UPDATE attempts SET state='running' WHERE id=?1",
                    [manifest.attempt_id.as_str()],
                )
                .map_err(internal)?;
            }
            self.validate_dependencies(&manifest)?;
            launch::execute(&self.config, &manifest, &cancel)
        })();
        let _ = self.finish_job(&manifest, outcome);
        if let Ok(mut workers) = self.workers.lock() {
            workers.remove(manifest.job_id.as_str());
        }
    }

    fn finish_job(&self, manifest: &WorkerRequest, outcome: PResult<WorkerOutput>) -> PResult<()> {
        // Validate worker bytes before entering the durable publication transaction.
        let result: PResult<(ProgramDescriptor, Vec<ArtifactIdentity>, Vec<ReviewFlag>)> =
            (|| match outcome? {
                WorkerOutput::Generated {
                    program,
                    receipt,
                    lineage,
                    review,
                } => {
                    validate_reviews(&review)?;
                    let path = program.path.canonicalize().map_err(unavailable)?;
                    let root = manifest.output_dir.canonicalize().map_err(unavailable)?;
                    if !path.starts_with(&root) {
                        return Err(ProtocolError::new(
                            ErrorCode::Forbidden,
                            "worker artifact escaped attempt directory",
                        ));
                    }
                    let (hash, bytes) =
                        artifacts::digest_file(&path, MAX_MOTION_BYTES).map_err(unavailable)?;
                    if hash != program.identity.sha256 || bytes != program.identity.byte_len {
                        return Err(ProtocolError::new(
                            ErrorCode::DependencyMismatch,
                            "worker program artifact identity mismatch",
                        ));
                    }
                    let file = artifacts::open_regular(&path).map_err(unavailable)?;
                    let program: MotionProgram = serde_json::from_reader(file)
                        .map_err(|e| ProtocolError::invalid(e.to_string()))?;
                    validate_program(&program)?;
                    let receipt_path = receipt.path.canonicalize().map_err(unavailable)?;
                    if !receipt_path.starts_with(&root) {
                        return Err(ProtocolError::new(
                            ErrorCode::Forbidden,
                            "worker receipt escaped attempt",
                        ));
                    }
                    let (receipt_hash, receipt_size) =
                        artifacts::digest_file(&receipt_path, manifest.budget.output_bytes)
                            .map_err(unavailable)?;
                    if receipt_hash != receipt.identity.sha256
                        || receipt_size != receipt.identity.byte_len
                    {
                        return Err(ProtocolError::new(
                            ErrorCode::DependencyMismatch,
                            "receipt identity mismatch",
                        ));
                    }
                    let mut lineage = lineage;
                    lineage.push(receipt.identity);
                    Ok((self.motion_store.publish(&program)?, lineage, review))
                }
                _ => Err(ProtocolError::invalid(
                    "generation worker returned non-generation output",
                )),
            })();
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db.transaction().map_err(internal)?;
        let state = job_snapshot(&tx, &manifest.job_id, &manifest.project_id)?;
        if state.attempt_id != manifest.attempt_id || state.state != "running" {
            return Ok(());
        }
        let owner: String = tx
            .query_row(
                "SELECT session FROM jobs WHERE id=?1",
                [manifest.job_id.as_str()],
                |r| r.get(0),
            )
            .map_err(internal)?;
        let owner = SessionId::new(owner).map_err(internal)?;
        if require_grant(&tx, &owner, &manifest.project_id, Scope::Generate).is_err() {
            tx.execute("UPDATE jobs SET state='cancelled',error='authorization revoked before publication' WHERE id=?1",[manifest.job_id.as_str()]).map_err(internal)?;
        } else {
            match result {
                Ok((program, lineage, review)) => {
                    let candidate = id!(CandidateId);
                    tx.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![candidate.as_str(),manifest.project_id.as_str(),rev_sql(manifest.base_revision)?,motion_state::register(&tx,&program)?,manifest.job_id.as_str(),encode(&lineage)?,encode(&review)?]).map_err(internal)?;
                    tx.execute(
                        "UPDATE jobs SET state='completed',candidate=?2 WHERE id=?1",
                        params![manifest.job_id.as_str(), candidate.as_str()],
                    )
                    .map_err(internal)?;
                    event(
                        &tx,
                        &manifest.project_id,
                        manifest.base_revision,
                        EventBody::CandidateReady {
                            candidate_id: candidate,
                        },
                    )?;
                }
                Err(error) => {
                    tx.execute(
                        "UPDATE jobs SET state='failed',error=?2 WHERE id=?1",
                        params![manifest.job_id.as_str(), error.to_string()],
                    )
                    .map_err(internal)?;
                }
            }
        }
        tx.execute(
            "UPDATE attempts SET state=(SELECT state FROM jobs WHERE id=?2) WHERE id=?1",
            params![manifest.attempt_id.as_str(), manifest.job_id.as_str()],
        )
        .map_err(internal)?;
        event(
            &tx,
            &manifest.project_id,
            manifest.base_revision,
            EventBody::JobChanged(job_snapshot(&tx, &manifest.job_id, &manifest.project_id)?),
        )?;
        tx.commit().map_err(internal)?;
        Ok(())
    }

    fn preview(
        &self,
        request: &Request,
        context: &FrameContext,
        source_time: SourceTimestamp,
        model_path: Option<&Path>,
        model_input: Option<ModelInputContract>,
    ) -> PResult<ResponseBody> {
        let artifact_guard = self.artifact_gate.lock().map_err(internal)?;
        {
            let db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, request.session.as_ref().unwrap())?;
            source_kinds::require_media(&db, project(request)?, &context.source_version)?;
        }
        let _reservation = self.pool.reserve(WORKER_MEMORY)?.with_storage(
            8 * 1024 * 1024,
            fs2::available_space(&self.config.state_dir).map_err(internal)?,
        )?;
        let model = model_path
            .map(|path| self.snapshot_model(path))
            .transpose()?;
        let tools = self.tools.as_ref().map_err(unavailable)?.clone();
        let source_version = context.source_version.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let key = format!(
            "preview:{}:{}:{}:{}:{}",
            project(request)?.as_str(),
            request.session.as_ref().unwrap().as_str(),
            source_version.as_str(),
            model
                .as_ref()
                .map(|m| m.identity.sha256.as_str())
                .unwrap_or("none"),
            request.request_id.as_str()
        );
        let source = {
            let db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, request.session.as_ref().unwrap())?;
            let source = self.source(&db, project(request)?, &source_version)?;
            self.workers
                .lock()
                .map_err(internal)?
                .insert(key.clone(), cancel.clone());
            source
        };
        struct PreviewLease<'a> {
            workers: &'a Mutex<HashMap<String, Arc<AtomicBool>>>,
            key: String,
        }
        impl Drop for PreviewLease<'_> {
            fn drop(&mut self) {
                if let Ok(mut workers) = self.workers.lock() {
                    workers.remove(&self.key);
                }
            }
        }
        let _lease = PreviewLease {
            workers: &self.workers,
            key,
        };
        let attempt_id = id!(AttemptId);
        let output_dir = self
            .config
            .state_dir
            .join("attempts")
            .join(attempt_id.as_str());
        artifacts::private_directory(&output_dir).map_err(internal)?;
        let revision = {
            let db = self.db.lock().map_err(internal)?;
            project_snapshot(&db, project(request)?)?.revision
        };
        drop(artifact_guard);
        let manifest = WorkerRequest {
            version: PROTOCOL_VERSION,
            attempt_id,
            job_id: id!(JobId),
            project_id: project(request)?.clone(),
            base_revision: revision,
            source,
            output_dir,
            operation: WorkerOperation::Preview {
                context: context.clone(),
                source_time,
                max_width: 640,
                max_height: 480,
                model,
                model_input,
            },
            budget: ResourceBudget {
                memory_bytes: WORKER_MEMORY,
                output_bytes: 8 * 1024 * 1024,
                wall_time_ms: 30_000,
                cpu_threads: 2,
            },
            dependencies: vec![
                tools.ffmpeg.identity.clone(),
                tools.ffprobe.identity.clone(),
            ],
            tools,
        };
        let output = launch::execute(&self.config, &manifest, &cancel)?;
        let db = self.db.lock().map_err(internal)?;
        self.authorize(&db, request, request.session.as_ref().unwrap())?;
        match output {
            WorkerOutput::Preview(preview)
                if preview.matches_seek_request(context, &source_time) =>
            {
                if let Some(frame) = &preview.frame {
                    frame.validate(manifest.budget.output_bytes)?;
                    let path = frame.artifact.path.canonicalize().map_err(unavailable)?;
                    if !path.starts_with(&manifest.output_dir) {
                        return Err(ProtocolError::new(
                            ErrorCode::Forbidden,
                            "preview artifact outside admitted attempt",
                        ));
                    }
                    let (hash, bytes) = artifacts::digest_file(&path, manifest.budget.output_bytes)
                        .map_err(unavailable)?;
                    if hash != frame.artifact.identity.sha256
                        || bytes != frame.artifact.identity.byte_len
                    {
                        return Err(ProtocolError::new(
                            ErrorCode::DependencyMismatch,
                            "preview artifact identity mismatch",
                        ));
                    }
                    let mut permissions = fs::metadata(&path).map_err(unavailable)?.permissions();
                    permissions.set_readonly(true);
                    fs::set_permissions(path, permissions).map_err(unavailable)?;
                }
                Ok(ResponseBody::Preview(preview))
            }
            _ => Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "preview evidence did not match requested source/frame/transforms/generation",
            )),
        }
    }

    fn export(
        &self,
        request: &Request,
        path: &Path,
        fingerprint: &str,
        selected_axis: Option<Axis>,
    ) -> PResult<ResponseBody> {
        let _artifact_guard = self.artifact_gate.lock().map_err(internal)?;
        let _motion_admission = self.pool.reserve_memory(transfers::VALIDATION_MEMORY)?;
        let session = request.session.as_ref().unwrap();
        let (bytes, revision) = {
            let mut db = self.db.lock().map_err(internal)?;
            let tx = db.transaction().map_err(internal)?;
            self.authorize(&tx, request, session)?;
            if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
                return Ok(body);
            }
            let existing: Option<String> = tx
                .query_row(
                    "SELECT state FROM exports WHERE session=?1 AND request=?2",
                    params![session.as_str(), request.request_id.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(internal)?;
            if existing.is_some() {
                return Err(ProtocolError::new(ErrorCode::UnknownOutcome,"previous export dispatch has an unresolved outcome; inspect destination, do not blindly replay"));
            }
            check_revision(&tx, request)?;
            claim_request_identity(&tx, session.as_str(), request, fingerprint)?;
            let snapshot = project_state(&tx, project(request)?)?;
            let bytes = match selected_axis {
                Some(axis) => neutral_export_axis(&snapshot.program, axis)?,
                None => neutral_export(&snapshot.program)?,
            };
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let needed = self
                .pool
                .reserved_storage()
                .checked_add(bytes.len() as u64)
                .and_then(|v| v.checked_add(16 * 1024 * 1024))
                .ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::ResourceExhausted,
                        "export storage reservation overflow",
                    )
                })?;
            if fs2::available_space(parent).map_err(unavailable)? < needed {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "insufficient unreserved export storage",
                ));
            }
            let hash = format!("{:x}", Sha256::digest(&bytes));
            tx.execute(
                "INSERT INTO exports(session,request,project,revision,path,sha256,state,axis) VALUES(?1,?2,?3,?4,?5,?6,'pending',?7)",
                params![
                    session.as_str(),
                    request.request_id.as_str(),
                    snapshot.project_id.as_str(),
                    rev_sql(snapshot.revision)?,
                    path.to_string_lossy(),
                    hash,
                    selected_axis.unwrap_or(Axis::Stroke).as_str()
                ],
            )
            .map_err(internal)?;
            tx.commit().map_err(internal)?;
            (bytes, snapshot.revision)
        };
        // The export reservation above is the authorization linearization point.
        // Revocation cannot erase already disclosed bytes or reverse a committed dispatch.
        let result = artifacts::publish_export(path, &bytes);
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db.transaction().map_err(internal)?;
        match result {
            Ok(()) => {
                let body = match selected_axis {
                    Some(axis) => ResponseBody::AxisExported {
                        path: path.to_path_buf(),
                        revision,
                        axis,
                    },
                    None => ResponseBody::Exported {
                        path: path.to_path_buf(),
                        revision,
                    },
                };
                tx.execute(
                    "UPDATE exports SET state='complete' WHERE session=?1 AND request=?2",
                    params![session.as_str(), request.request_id.as_str()],
                )
                .map_err(internal)?;
                remember(&tx, session.as_str(), request, fingerprint, &body)?;
                tx.commit().map_err(internal)?;
                Ok(body)
            }
            Err(error) => {
                tx.execute(
                    "UPDATE exports SET state='failed_or_unknown' WHERE session=?1 AND request=?2",
                    params![session.as_str(), request.request_id.as_str()],
                )
                .map_err(internal)?;
                tx.commit().map_err(internal)?;
                Err(ProtocolError::new(
                    ErrorCode::UnknownOutcome,
                    error.to_string(),
                ))
            }
        }
    }
}

fn project(request: &Request) -> PResult<&ProjectId> {
    request
        .project
        .as_ref()
        .ok_or_else(|| ProtocolError::invalid("project scope required"))
}
fn check_revision(db: &Connection, request: &Request) -> PResult<()> {
    if let Some(expected) = request.expected_revision {
        let actual = project_snapshot(db, project(request)?)?.revision;
        if expected != actual {
            return Err(ProtocolError::new(
                ErrorCode::RevisionConflict,
                "expected revision is stale",
            ));
        }
    }
    Ok(())
}
fn actor(db: &Connection, session: &SessionId) -> PResult<String> {
    db.query_row(
        "SELECT actor FROM sessions WHERE id=?1",
        [session.as_str()],
        |r| r.get(0),
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| ProtocolError::new(ErrorCode::Unauthorized, "unknown pairing session"))
}
fn require_grant(
    db: &Connection,
    session: &SessionId,
    project: &ProjectId,
    scope: Scope,
) -> PResult<()> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM grants WHERE session=?1 AND project=?2 AND scope=?3)",
            params![session.as_str(), project.as_str(), scope_name(scope)],
            |r| r.get(0),
        )
        .map_err(internal)?;
    if !exists {
        return Err(ProtocolError::new(
            ErrorCode::Forbidden,
            "client lacks required project capability",
        ));
    }
    Ok(())
}
fn check_request_identity(
    db: &Connection,
    session: &str,
    request: &Request,
    fingerprint: &str,
) -> PResult<()> {
    let old: Option<String> = db
        .query_row(
            "SELECT fingerprint FROM request_identities WHERE session=?1 AND request=?2",
            params![session, request.request_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    if old.is_some_and(|old| old != fingerprint) {
        return Err(ProtocolError::new(
            ErrorCode::RequestConflict,
            "request identity reused across commands or transfer namespaces",
        ));
    }
    Ok(())
}
fn claim_request_identity(
    db: &Connection,
    session: &str,
    request: &Request,
    fingerprint: &str,
) -> PResult<()> {
    check_request_identity(db, session, request, fingerprint)?;
    db.execute(
        "INSERT OR IGNORE INTO request_identities VALUES(?1,?2,?3)",
        params![session, request.request_id.as_str(), fingerprint],
    )
    .map_err(internal)?;
    Ok(())
}
fn replay(
    db: &Connection,
    session: &str,
    request: &Request,
    fingerprint: &str,
) -> PResult<Option<ResponseBody>> {
    check_request_identity(db, session, request, fingerprint)?;
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT fingerprint,response FROM requests WHERE session=?1 AND request=?2",
            params![session, request.request_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(internal)?;
    match row {
        Some((old, body)) if old == fingerprint => Ok(Some(decode(&body)?)),
        Some(_) => Err(ProtocolError::new(
            ErrorCode::RequestConflict,
            "request identity reused with different content",
        )),
        None => Ok(None),
    }
}
fn remember(
    tx: &Transaction<'_>,
    session: &str,
    request: &Request,
    fingerprint: &str,
    body: &ResponseBody,
) -> PResult<()> {
    claim_request_identity(tx, session, request, fingerprint)?;
    tx.execute(
        "INSERT INTO requests VALUES(?1,?2,?3,?4)",
        params![
            session,
            request.request_id.as_str(),
            fingerprint,
            encode(body)?
        ],
    )
    .map_err(internal)?;
    Ok(())
}

struct ProjectState {
    project_id: ProjectId,
    revision: RevisionId,
    program: MotionProgram,
    motion: MotionDescriptor,
}
fn project_state(db: &Connection, id: &ProjectId) -> PResult<ProjectState> {
    let snapshot = project_snapshot(db, id)?;
    let program = motion_state::store_for(db)?.read(&snapshot.motion.program)?;
    Ok(ProjectState {
        project_id: snapshot.project_id,
        revision: snapshot.revision,
        program,
        motion: snapshot.motion,
    })
}
struct CandidateState {
    candidate_id: CandidateId,
    project_id: ProjectId,
    base_revision: RevisionId,
    program: MotionProgram,
    review: Vec<ReviewFlag>,
}
fn candidate_state(
    db: &Connection,
    id: &CandidateId,
    project: &ProjectId,
) -> PResult<CandidateState> {
    let snapshot = candidate_snapshot(db, id, project)?;
    let program = motion_state::store_for(db)?.read(&snapshot.motion.program)?;
    Ok(CandidateState {
        candidate_id: snapshot.candidate_id,
        project_id: snapshot.project_id,
        base_revision: snapshot.base_revision,
        program,
        review: snapshot.review,
    })
}

fn project_snapshot(db: &Connection, id: &ProjectId) -> PResult<ProjectSnapshot> {
    let row: Option<(String, i64, String)> = db
        .query_row(
            "SELECT name,revision,program FROM projects WHERE id=?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(internal)?;
    let (name, revision, program) =
        row.ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "project not found"))?;
    let mut stmt = db
        .prepare("SELECT version,label,kind FROM sources WHERE project=?1 ORDER BY version")
        .map_err(internal)?;
    let rows = stmt
        .query_map([id.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(internal)?;
    let mut sources = Vec::new();
    for row in rows {
        let (version, label, kind) = row.map_err(internal)?;
        sources.push(SourceSummary {
            source_version: SourceVersionId::new(version).map_err(internal)?,
            label,
            kind: source_kinds::parse(&kind)?,
        });
    }
    Ok(ProjectSnapshot {
        project_id: id.clone(),
        revision: RevisionId::new(revision as u64),
        name,
        motion: MotionDescriptor {
            program: motion_state::descriptor(db, &program)?,
            binding: MotionBinding::ProjectRevision {
                project_id: id.clone(),
                revision: RevisionId::new(revision as u64),
            },
        },
        sources,
    })
}
fn job_snapshot(db: &Connection, id: &JobId, project: &ProjectId) -> PResult<JobSnapshot> {
    let row:Option<(String,i64,String,Option<String>,Option<String>)>=db.query_row("SELECT attempt,base_revision,state,candidate,error FROM jobs WHERE id=?1 AND project=?2",params![id.as_str(),project.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(internal)?;
    let (attempt, revision, state, candidate, error) =
        row.ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "job not found in project"))?;
    Ok(JobSnapshot {
        job_id: id.clone(),
        attempt_id: AttemptId::new(attempt).map_err(internal)?,
        project_id: project.clone(),
        base_revision: RevisionId::new(revision as u64),
        state,
        candidate_id: candidate
            .map(CandidateId::new)
            .transpose()
            .map_err(internal)?,
        error,
    })
}
fn candidate_snapshot(
    db: &Connection,
    id: &CandidateId,
    project: &ProjectId,
) -> PResult<CandidateSnapshot> {
    let row: Option<(i64, String, Option<String>, String)> = db
        .query_row(
            "SELECT base_revision,program,job,review FROM candidates WHERE id=?1 AND project=?2",
            params![id.as_str(), project.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(internal)?;
    let (revision, program, job, review) = row
        .ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "candidate not found in project"))?;
    let review: Vec<ReviewFlag> = decode(&review)?;
    validate_reviews(&review)?;
    Ok(CandidateSnapshot {
        candidate_id: id.clone(),
        project_id: project.clone(),
        base_revision: RevisionId::new(revision as u64),
        motion: MotionDescriptor {
            program: motion_state::descriptor(db, &program)?,
            binding: MotionBinding::Candidate {
                project_id: project.clone(),
                candidate_id: id.clone(),
                base_revision: RevisionId::new(revision as u64),
            },
        },
        job_id: job.map(JobId::new).transpose().map_err(internal)?,
        review,
    })
}
fn event(
    tx: &Transaction<'_>,
    project: &ProjectId,
    revision: RevisionId,
    body: EventBody,
) -> PResult<()> {
    tx.execute(
        "INSERT INTO events(project,revision,body) VALUES(?1,?2,?3)",
        params![project.as_str(), rev_sql(revision)?, encode(&body)?],
    )
    .map_err(internal)?;
    Ok(())
}
fn validate_program(program: &MotionProgram) -> PResult<()> {
    let actions = program
        .tracks()
        .iter()
        .try_fold(0_u64, |sum, track| {
            sum.checked_add(track.actions().len() as u64)
        })
        .ok_or_else(|| ProtocolError::invalid("motion action count overflow"))?;
    let gaps = program
        .tracks()
        .iter()
        .try_fold(0_u64, |sum, track| {
            sum.checked_add(track.gaps().len() as u64)
        })
        .ok_or_else(|| ProtocolError::invalid("motion gap count overflow"))?;
    if actions > MAX_MOTION_ACTIONS || gaps > MAX_MOTION_GAPS {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "motion count exceeds artifact admission",
        ));
    }
    Ok(())
}
fn commit_program(
    tx: &Transaction<'_>,
    request: &Request,
    session: &SessionId,
    program: &MotionProgram,
    kind: &str,
    label: &str,
    append: bool,
) -> PResult<RevisionId> {
    validate_program(program)?;
    let id = project(request)?;
    let current = project_state(tx, id)?;
    if request.expected_revision != Some(current.revision) {
        return Err(ProtocolError::new(
            ErrorCode::RevisionConflict,
            "expected revision does not match project",
        ));
    }
    let protected: String = tx
        .query_row(
            "SELECT protected FROM projects WHERE id=?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .map_err(internal)?;
    let regions: Vec<ProtectedRegion> = decode(&protected)?;
    let authored_candidate = if let Command::CommitCandidate { candidate_id } = &request.command {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM edit_candidate_lineage WHERE candidate=?1)",
            [candidate_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(internal)?
    } else {
        false
    };
    if authored_candidate || matches!(kind, "candidate_merge" | "undo" | "redo") {
        pulsar_core::ensure_protected_segments_unchanged(&current.program, program, &regions)
    } else {
        ensure_protected_regions_unchanged(&current.program, program, &regions)
    }
    .map_err(|e| ProtocolError::new(ErrorCode::Forbidden, e.to_string()))?;
    let next = current.revision.checked_next().map_err(internal)?;
    let encoded = motion_state::save(tx, program)?;
    let actor = actor(tx, session)?;
    tx.execute(
        "UPDATE projects SET revision=?2,program=?3 WHERE id=?1",
        params![id.as_str(), rev_sql(next)?, encoded],
    )
    .map_err(internal)?;
    tx.execute(
        "INSERT INTO revisions VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            id.as_str(),
            rev_sql(next)?,
            actor,
            kind,
            label,
            encoded,
            protected
        ],
    )
    .map_err(internal)?;
    if append {
        append_history(tx, id, &encoded, &protected)?;
    }
    lineage::record(
        tx,
        request,
        next,
        current.revision,
        &current.program,
        program,
        append,
    )?;
    event(tx, id, next, EventBody::ProjectChanged)?;
    Ok(next)
}
fn append_history(
    tx: &Transaction<'_>,
    id: &ProjectId,
    program: &str,
    protected: &str,
) -> PResult<()> {
    let cursor: i64 = tx
        .query_row(
            "SELECT history_cursor FROM projects WHERE id=?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .map_err(internal)?;
    let next_position = cursor
        .checked_add(1)
        .ok_or_else(|| ProtocolError::invalid("history overflow"))?;
    tx.execute(
        "DELETE FROM edit_states WHERE project=?1 AND position>?2",
        params![id.as_str(), cursor],
    )
    .map_err(internal)?;
    tx.execute(
        "INSERT INTO edit_states VALUES(?1,?2,?3,?4)",
        params![id.as_str(), next_position, program, protected],
    )
    .map_err(internal)?;
    tx.execute(
        "UPDATE projects SET history_cursor=?2 WHERE id=?1",
        params![id.as_str(), next_position],
    )
    .map_err(internal)?;
    Ok(())
}
fn neutral_export(program: &MotionProgram) -> PResult<Vec<u8>> {
    if program
        .tracks()
        .iter()
        .any(|track| track.axis() != Axis::Stroke)
    {
        return Err(ProtocolError::unsupported(
            "ambiguous multi-axis export; use ExportAxis with an explicit axis",
        ));
    }
    neutral_export_axis(program, Axis::Stroke)
}

fn neutral_export_axis(program: &MotionProgram, axis: Axis) -> PResult<Vec<u8>> {
    let track = program.track(axis).ok_or_else(|| {
        ProtocolError::invalid(format!(
            "selected export axis {} is absent from the project",
            axis.as_str()
        ))
    })?;
    if !track.gaps().is_empty() {
        return Err(ProtocolError::unsupported(
            "standard export of selected-axis unresolved evidence gaps; explicit resolution required",
        ));
    }
    if track.actions().is_empty() {
        return Err(ProtocolError::invalid(
            "selected export axis contains no motion actions",
        ));
    }
    let mut actions = Vec::with_capacity(track.actions().len());
    let mut last = None;
    for action in track.actions() {
        let ns = action.time().as_nanos();
        if ns < 0 {
            return Err(ProtocolError::invalid(
                "standard funscript cannot encode negative project time",
            ));
        }
        let ms = ns
            .checked_add(500_000)
            .ok_or_else(|| ProtocolError::invalid("export timestamp overflow"))?
            / 1_000_000;
        if last.is_some_and(|previous| previous >= ms) {
            return Err(ProtocolError::invalid(
                "millisecond quantization merges ordered actions; explicit resampling required",
            ));
        }
        last = Some(ms);
        actions.push(serde_json::json!({
            "at": ms,
            "pos": (action.position().value() * 100.0).round() as u32,
        }));
    }
    serde_json::to_vec_pretty(
        &serde_json::json!({"version":"1.0","inverted":false,"range":90,"actions":actions}),
    )
    .map_err(internal)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        engine: Arc<Engine>,
        session: SessionId,
        project: ProjectId,
        dir: tempfile::TempDir,
    }
    thread_local! {static TOKENS:std::cell::RefCell<HashMap<String,String>>=std::cell::RefCell::new(HashMap::new());}
    fn request(
        session: &SessionId,
        project: Option<&ProjectId>,
        revision: Option<u64>,
        command: Command,
    ) -> Request {
        Request {
            version: PROTOCOL_VERSION,
            request_id: id!(RequestId),
            session: Some(session.clone()),
            auth_token: TOKENS.with(|tokens| tokens.borrow().get(session.as_str()).cloned()),
            project: project.cloned(),
            expected_revision: revision.map(RevisionId::new),
            command,
        }
    }
    fn pair(engine: &Arc<Engine>, name: &str) -> SessionId {
        let r = Request::new(
            id!(RequestId),
            Command::Pair {
                client_name: name.into(),
                pairing_token: engine.token.clone(),
            },
        );
        match engine.handle(r).result.unwrap() {
            ResponseBody::Paired {
                session,
                auth_token,
            } => {
                TOKENS.with(|tokens| tokens.borrow_mut().insert(session.to_string(), auth_token));
                session
            }
            _ => panic!("pairing response"),
        }
    }
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("nonexecuted-worker-fixture");
        fs::write(&binary, b"authority-test executable identity").unwrap();
        let engine =
            Engine::open(EngineConfig::new(dir.path().join("private-state"), binary)).unwrap();
        let session = pair(&engine, "test client");
        let r = request(
            &session,
            None,
            None,
            Command::CreateProject {
                name: "Test project".into(),
            },
        );
        let project = match engine.handle(r).result.unwrap() {
            ResponseBody::Project(p) => p.project_id,
            _ => panic!("project response"),
        };
        Fixture {
            engine,
            session,
            project,
            dir,
        }
    }
    fn program(position: f64) -> MotionProgram {
        MotionProgram::new(vec![MotionTrack::new(
            Axis::Stroke,
            vec![
                MotionAction::new(
                    ProjectTime::ZERO,
                    NormalizedPosition::new(position).unwrap(),
                    EvidenceKind::Observed,
                )
                .unwrap(),
                MotionAction::new(
                    ProjectTime::from_nanos(1_000_000_000),
                    NormalizedPosition::new(position).unwrap(),
                    EvidenceKind::Observed,
                )
                .unwrap(),
            ],
        )
        .unwrap()])
        .unwrap()
    }
    // Stage only a proposal; the public command still checks all authority and revision rules.
    fn commit_program(f: &Fixture, revision: u64, program: MotionProgram) -> Command {
        let candidate_id = id!(CandidateId);
        let db = f.engine.db.lock().unwrap();
        let reference = crate::authority::motion_state::save(&db, &program).unwrap();
        db.execute(
            "INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,?3,?4,NULL,'[]','[]')",
            params![candidate_id.as_str(), f.project.as_str(), revision as i64, reference],
        ).unwrap();
        Command::CommitCandidate { candidate_id }
    }
    #[test]
    fn duplicate_edit_is_durable_replay_not_second_revision() {
        let f = fixture();
        let r = request(
            &f.session,
            Some(&f.project),
            Some(0),
            commit_program(&f, 0, program(0.3)),
        );
        let first = f.engine.handle(r.clone()).result.unwrap();
        let second = f.engine.handle(r.clone()).result.unwrap();
        assert_eq!(encode(&first).unwrap(), encode(&second).unwrap());
        let config = f.engine.config.clone();
        drop(f.engine);
        let reopened = Engine::open(config).unwrap();
        assert_eq!(
            encode(&first).unwrap(),
            encode(&reopened.handle(r).result.unwrap()).unwrap()
        );
        let db = reopened.db.lock().unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
    #[test]
    fn stale_revision_and_reused_request_identity_reject() {
        let f = fixture();
        let r = request(
            &f.session,
            Some(&f.project),
            Some(0),
            commit_program(&f, 0, program(0.3)),
        );
        assert!(f.engine.handle(r.clone()).result.is_ok());
        let mut changed = r.clone();
        changed.command = commit_program(&f, 0, program(0.9));
        assert_eq!(
            f.engine.handle(changed).result.unwrap_err().code,
            ErrorCode::RequestConflict
        );
        let mut stale = r;
        stale.request_id = id!(RequestId);
        assert_eq!(
            f.engine.handle(stale).result.unwrap_err().code,
            ErrorCode::RevisionConflict
        );
    }
    #[test]
    fn independently_paired_client_has_no_existing_project_authority() {
        let f = fixture();
        let other = pair(&f.engine, "PulsarDesktop");
        let r = request(
            &other,
            None,
            None,
            Command::OpenProject {
                project_id: f.project.clone(),
            },
        );
        assert_eq!(
            f.engine.handle(r).result.unwrap_err().code,
            ErrorCode::Forbidden
        );
        let grant = request(
            &f.session,
            None,
            None,
            Command::Grant {
                session: other.clone(),
                project_id: f.project.clone(),
                scopes: vec![Scope::Read],
            },
        );
        assert!(f.engine.handle(grant).result.is_ok());
        assert!(f
            .engine
            .handle(request(
                &other,
                Some(&f.project),
                None,
                Command::GetSnapshot
            ))
            .result
            .is_ok());
        assert_eq!(
            f.engine
                .handle(request(
                    &other,
                    Some(&f.project),
                    Some(0),
                    commit_program(&f, 0, program(0.2))
                ))
                .result
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
    }
    #[test]
    fn revocation_denies_cached_queries_and_undo_preserves_fresh_revisions() {
        let f = fixture();
        let other = pair(&f.engine, "reviewer");
        f.engine
            .handle(request(
                &f.session,
                None,
                None,
                Command::Grant {
                    session: other.clone(),
                    project_id: f.project.clone(),
                    scopes: vec![Scope::Read],
                },
            ))
            .result
            .unwrap();
        let query = request(&other, Some(&f.project), None, Command::GetSnapshot);
        f.engine.handle(query.clone()).result.unwrap();
        f.engine
            .handle(request(
                &f.session,
                None,
                None,
                Command::Revoke {
                    session: other,
                    project_id: f.project.clone(),
                },
            ))
            .result
            .unwrap();
        assert_eq!(
            f.engine.handle(query).result.unwrap_err().code,
            ErrorCode::Forbidden
        );
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(0),
                commit_program(&f, 0, program(0.2)),
            ))
            .result
            .unwrap();
        let undo = f
            .engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(1),
                Command::Undo,
            ))
            .result
            .unwrap();
        match undo {
            ResponseBody::Project(p) => {
                assert_eq!(p.revision.value(), 2);
                assert_eq!(
                    f.engine.motion_store.read(&p.motion.program).unwrap(),
                    MotionProgram::default()
                );
            }
            _ => panic!(),
        }
        let redo = f
            .engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(2),
                Command::Redo,
            ))
            .result
            .unwrap();
        match redo {
            ResponseBody::Project(p) => {
                assert_eq!(p.revision.value(), 3);
                assert_eq!(
                    f.engine.motion_store.read(&p.motion.program).unwrap(),
                    program(0.2)
                );
            }
            _ => panic!(),
        }
    }
    #[test]
    fn protected_regions_cover_replacement_and_undo() {
        let f = fixture();
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(0),
                commit_program(&f, 0, program(0.2)),
            ))
            .result
            .unwrap();
        let regions = vec![ProtectedRegion {
            axis: Some(Axis::Stroke),
            range: TimeRange::new(ProjectTime::ZERO, ProjectTime::from_nanos(1_000_000_001))
                .unwrap(),
        }];
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(1),
                Command::SetProtectedRegions { regions },
            ))
            .result
            .unwrap();
        assert_eq!(
            f.engine
                .handle(request(
                    &f.session,
                    Some(&f.project),
                    Some(2),
                    commit_program(&f, 2, program(0.8))
                ))
                .result
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        let editor = pair(&f.engine, "ordinary editor");
        f.engine
            .handle(request(
                &f.session,
                None,
                None,
                Command::Grant {
                    session: editor.clone(),
                    project_id: f.project.clone(),
                    scopes: vec![Scope::Read, Scope::Edit],
                },
            ))
            .result
            .unwrap();
        for command in [
            Command::Undo,
            Command::SetProtectedRegions { regions: vec![] },
        ] {
            assert_eq!(
                f.engine
                    .handle(request(&editor, Some(&f.project), Some(2), command))
                    .result
                    .unwrap_err()
                    .code,
                ErrorCode::Forbidden
            );
        }
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(2),
                Command::Undo,
            ))
            .result
            .unwrap();
        {
            let db = f.engine.db.lock().unwrap();
            let protected: String = db
                .query_row("SELECT protected FROM projects", [], |r| r.get(0))
                .unwrap();
            assert_eq!(protected, "[]");
        }
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(3),
                Command::Redo,
            ))
            .result
            .unwrap();
        assert_eq!(
            f.engine
                .handle(request(
                    &f.session,
                    Some(&f.project),
                    Some(4),
                    commit_program(&f, 4, program(0.8))
                ))
                .result
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
    }
    #[test]
    fn physical_effects_fail_closed() {
        let f = fixture();
        let result = f
            .engine
            .handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::Arm {
                    device_id: id!(DeviceId),
                    revision: RevisionId::new(0),
                },
            ))
            .result;
        assert_eq!(result.unwrap_err().code, ErrorCode::Unsupported);
        assert!(serde_json::from_str::<Request>(r#"{"version":1,"request_id":"unknown","session":null,"project":null,"expected_revision":null,"command":{"operation":"shell","arguments":{"command":"rm"}}}"#).is_err());
    }
    #[test]
    fn export_has_no_provenance_and_replays_without_overwrite() {
        let f = fixture();
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                Some(0),
                commit_program(&f, 0, program(0.2)),
            ))
            .result
            .unwrap();
        let path = f.dir.path().join("neutral.funscript");
        let r = request(
            &f.session,
            Some(&f.project),
            None,
            Command::Export { path: path.clone() },
        );
        f.engine.handle(r.clone()).result.unwrap();
        f.engine.handle(r).result.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["actions"][0]["pos"], 20);
        assert!(value.get("source").is_none());
        assert!(value.get("lineage").is_none());
        let db = f.engine.db.lock().unwrap();
        let state: String = db
            .query_row("SELECT state FROM exports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "complete");
    }
    #[test]
    fn explicit_gaps_block_standard_export() {
        let gap = TimeRange::new(ProjectTime::ZERO, ProjectTime::from_nanos(1000)).unwrap();
        let p = MotionProgram::new(vec![MotionTrack::with_gaps(
            Axis::Stroke,
            vec![],
            vec![gap],
        )
        .unwrap()])
        .unwrap();
        assert_eq!(neutral_export(&p).unwrap_err().code, ErrorCode::Unsupported);
    }
    #[test]
    fn imported_snapshot_pin_and_evict_are_scoped() {
        let f = fixture();
        let original = f.dir.path().join("original.mp4");
        fs::write(&original, b"immutable test").unwrap();
        let source = match f
            .engine
            .handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::ImportSource {
                    path: original.clone(),
                },
            ))
            .result
            .unwrap()
        {
            ResponseBody::Source { source_version } => source_version,
            _ => panic!(),
        };
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::PinSource {
                    source_version: source.clone(),
                    pinned: true,
                },
            ))
            .result
            .unwrap();
        assert_eq!(
            f.engine
                .handle(request(
                    &f.session,
                    Some(&f.project),
                    None,
                    Command::EvictSource {
                        source_version: source.clone()
                    }
                ))
                .result
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::PinSource {
                    source_version: source.clone(),
                    pinned: false,
                },
            ))
            .result
            .unwrap();
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::EvictSource {
                    source_version: source,
                },
            ))
            .result
            .unwrap();
        assert_eq!(fs::read(original).unwrap(), b"immutable test");
    }
    #[test]
    fn invalid_credentials_never_create_session() {
        let f = fixture();
        let r = Request::new(
            id!(RequestId),
            Command::Pair {
                client_name: "test".into(),
                pairing_token: "incorrect".into(),
            },
        );
        assert_eq!(
            f.engine.handle(r).result.unwrap_err().code,
            ErrorCode::Unauthorized
        );
        let db = f.engine.db.lock().unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn full_durability_is_selected() {
        let f = fixture();
        let db = f.engine.db.lock().unwrap();
        let synchronous: i64 = db
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(synchronous, 2);
    }
    #[test]
    fn public_session_identity_is_not_a_bearer_credential() {
        let f = fixture();
        let other = pair(&f.engine, "other client");
        let mut stolen = request(&other, Some(&f.project), None, Command::GetSnapshot);
        stolen.session = Some(f.session.clone());
        assert_eq!(
            f.engine.handle(stolen).result.unwrap_err().code,
            ErrorCode::Unauthorized
        );
        let mut absent = request(&f.session, Some(&f.project), None, Command::GetSnapshot);
        absent.auth_token = None;
        assert_eq!(
            f.engine.handle(absent).result.unwrap_err().code,
            ErrorCode::Unauthorized
        );
    }
    #[test]
    fn queries_do_not_accumulate_durable_request_rows() {
        let f = fixture();
        let count = || {
            let db = f.engine.db.lock().unwrap();
            db.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        let before = count();
        let query = request(&f.session, Some(&f.project), None, Command::GetSnapshot);
        for _ in 0..20 {
            f.engine.handle(query.clone()).result.unwrap();
        }
        assert_eq!(count(), before);
    }
    #[test]
    fn ordinary_edit_grant_does_not_authorize_filesystem_import() {
        let f = fixture();
        let editor = pair(&f.engine, "editor");
        f.engine
            .handle(request(
                &f.session,
                None,
                None,
                Command::Grant {
                    session: editor.clone(),
                    project_id: f.project.clone(),
                    scopes: vec![Scope::Edit],
                },
            ))
            .result
            .unwrap();
        let result = f
            .engine
            .handle(request(
                &editor,
                Some(&f.project),
                None,
                Command::ImportSource {
                    path: f.dir.path().join("not-read"),
                },
            ))
            .result;
        assert_eq!(result.unwrap_err().code, ErrorCode::Forbidden);
    }
    fn staged_manifest(f: &Fixture) -> (WorkerRequest, SourceVersionId) {
        let import = |name: &str, bytes: &[u8]| {
            let path = f.dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            match f
                .engine
                .handle(request(
                    &f.session,
                    Some(&f.project),
                    None,
                    Command::ImportSource { path },
                ))
                .result
                .unwrap()
            {
                ResponseBody::Source { source_version } => source_version,
                _ => panic!(),
            }
        };
        let source_version = import("media.fixture", b"media");
        let model_version = import("model.fixture", b"model");
        let db = f.engine.db.lock().unwrap();
        let source = f.engine.source(&db, &f.project, &source_version).unwrap();
        let model = f.engine.source(&db, &f.project, &model_version).unwrap();
        let identity = source.identity.clone();
        let reference = ArtifactReference {
            identity: identity.clone(),
            path: source.path.clone(),
        };
        let tools = WorkerTools {
            ffmpeg: reference.clone(),
            ffprobe: reference,
            onnx_runtime: None,
        };
        let job = id!(JobId);
        let attempt = id!(AttemptId);
        let output_dir = f
            .engine
            .config
            .state_dir
            .join("attempts")
            .join(attempt.as_str());
        artifacts::private_directory(&output_dir).unwrap();
        let manifest = WorkerRequest {
            version: PROTOCOL_VERSION,
            attempt_id: attempt.clone(),
            job_id: job.clone(),
            project_id: f.project.clone(),
            base_revision: RevisionId::new(0),
            source,
            output_dir,
            operation: WorkerOperation::Generate {
                preset: "fast".into(),
                settings: GenerationSettings {
                    model_input: Some(ModelInputContract {
                        width: 64,
                        height: 64,
                        channels: 3,
                        layout: TensorLayout::Nchw,
                        class_count: 1,
                        decoder: DetectorDecoder::YoloChannelMajor,
                    }),
                    ..Default::default()
                },
                model: Some(model.clone()),
            },
            budget: ResourceBudget {
                memory_bytes: 1024,
                output_bytes: 1024,
                wall_time_ms: 1000,
                cpu_threads: 1,
            },
            dependencies: vec![model.identity, identity],
            tools,
        };
        db.execute("INSERT INTO jobs(id,attempt,project,base_revision,session,source,state,manifest) VALUES(?1,?2,?3,0,?4,?5,'queued',?6)",params![job.as_str(),attempt.as_str(),f.project.as_str(),f.session.as_str(),source_version.as_str(),encode(&manifest).unwrap()]).unwrap();
        db.execute(
            "INSERT INTO attempts VALUES(?1,?2,?3,'queued')",
            params![attempt.as_str(), job.as_str(), encode(&manifest).unwrap()],
        )
        .unwrap();
        (manifest, model_version)
    }
    #[test]
    fn active_model_bytes_cannot_be_evicted_through_source_alias() {
        let f = fixture();
        let (manifest, model) = staged_manifest(&f);
        for state in ["queued", "running", "interrupted"] {
            {
                let db = f.engine.db.lock().unwrap();
                db.execute(
                    "UPDATE jobs SET state=?2 WHERE id=?1",
                    params![manifest.job_id.as_str(), state],
                )
                .unwrap();
            }
            let response = f.engine.handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::EvictSource {
                    source_version: model.clone(),
                },
            ));
            assert_eq!(response.result.unwrap_err().code, ErrorCode::Forbidden);
        }
    }
    #[test]
    fn cancelled_and_revoked_work_cannot_publish_late_candidates() {
        let f = fixture();
        let (manifest, _) = staged_manifest(&f);
        let flag = Arc::new(AtomicBool::new(false));
        f.engine
            .workers
            .lock()
            .unwrap()
            .insert(manifest.job_id.to_string(), flag.clone());
        f.engine
            .handle(request(
                &f.session,
                Some(&f.project),
                None,
                Command::CancelJob {
                    job_id: manifest.job_id.clone(),
                },
            ))
            .result
            .unwrap();
        assert!(flag.load(Ordering::Acquire));
        f.engine
            .finish_job(
                &manifest,
                Err(ProtocolError::invalid("late malformed result")),
            )
            .unwrap();
        {
            let db = f.engine.db.lock().unwrap();
            assert_eq!(
                job_snapshot(&db, &manifest.job_id, &f.project)
                    .unwrap()
                    .state,
                "cancelled"
            );
            let count: i64 = db
                .query_row("SELECT COUNT(*) FROM candidates", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
        {
            let db = f.engine.db.lock().unwrap();
            db.execute(
                "UPDATE jobs SET state='running' WHERE id=?1",
                [manifest.job_id.as_str()],
            )
            .unwrap();
        }
        f.engine
            .handle(request(
                &f.session,
                None,
                None,
                Command::Revoke {
                    session: f.session.clone(),
                    project_id: f.project.clone(),
                },
            ))
            .result
            .unwrap();
        f.engine
            .finish_job(
                &manifest,
                Err(ProtocolError::invalid("another late result")),
            )
            .unwrap();
        let db = f.engine.db.lock().unwrap();
        assert_eq!(
            job_snapshot(&db, &manifest.job_id, &f.project)
                .unwrap()
                .state,
            "cancelled"
        );
    }
}
