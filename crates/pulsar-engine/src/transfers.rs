//! Authenticated, epoch-fenced bulk leases. Upload finalization is a proposal, never a commit.
use super::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

pub(super) const VALIDATION_MEMORY: u64 = 1024 * 1024 * 1024;
const RECEIPT_LIMIT: usize = 128 * 1024 * 1024;
const LEASE_LIFETIME: Duration = Duration::from_secs(300);
const MAX_LEASES: usize = 8;
const MAX_REPLAY_CHUNKS: usize = 4096;

#[derive(Default)]
pub(super) struct TransferState {
    leases: HashMap<String, Lease>,
}
struct Lease {
    id: TransferId,
    session: SessionId,
    project: ProjectId,
    epoch: String,
    direction: TransferDirection,
    sha256: String,
    total: u64,
    offset: u64,
    length: u64,
    accepted: u64,
    deadline: Instant,
    base: RevisionId,
    label: String,
    file: Option<File>,
    staging: Option<tempfile::NamedTempFile>,
    reservation: Option<Reservation>,
    chunks: HashMap<u64, (u32, String)>,
    finalizing: bool,
    cancelled: bool,
    completed: bool,
}
impl Lease {
    fn check(&self, session: &SessionId, project: &ProjectId, epoch: &str) -> PResult<()> {
        if &self.session != session || &self.project != project {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "transfer belongs to another session or project",
            ));
        }
        if self.epoch != epoch {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "transfer belongs to a previous engine epoch",
            ));
        }
        if self.cancelled {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "transfer authorization was revoked or abandoned",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "transfer lease expired",
            ));
        }
        Ok(())
    }
    fn view(&self, endpoint: PathBuf) -> PResult<TransferLease> {
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        if remaining == 0 {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "transfer lease expired",
            ));
        }
        Ok(TransferLease {
            lease_id: self.id.clone(),
            engine_epoch: self.epoch.clone(),
            project_id: self.project.clone(),
            direction: self.direction,
            sha256: self.sha256.clone(),
            total_byte_len: self.total,
            offset: self.offset,
            byte_len: self.length,
            accepted_prefix: self.accepted,
            expires_after_ms: remaining.min(u64::MAX as u128) as u64,
            bulk_endpoint: endpoint,
        })
    }
    fn release(&mut self) {
        if !self.finalizing {
            self.file.take();
            self.staging.take();
            self.reservation.take();
        }
    }
}
impl TransferState {
    pub(super) fn sweep(&mut self) {
        let now = Instant::now();
        self.leases
            .retain(|_, lease| lease.finalizing || (!lease.cancelled && lease.deadline > now));
        if self.leases.len() >= 128 {
            self.leases.retain(|_, lease| !lease.completed);
        }
    }
    pub(super) fn revoke(&mut self, session: &SessionId, project: &ProjectId) {
        for lease in self
            .leases
            .values_mut()
            .filter(|lease| &lease.session == session && &lease.project == project)
        {
            lease.cancelled = true;
            lease.release();
        }
    }
}
pub(super) fn initialize(db: &Connection) -> PResult<()> {
    let tx = db.unchecked_transaction().map_err(internal)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS transfer_begins(session TEXT NOT NULL,request TEXT NOT NULL,fingerprint TEXT NOT NULL,lease_id TEXT NOT NULL,engine_epoch TEXT NOT NULL,PRIMARY KEY(session,request));
        CREATE TABLE IF NOT EXISTS edit_upload_receipts(lease_id TEXT PRIMARY KEY,project TEXT NOT NULL,session TEXT NOT NULL,base_revision INTEGER NOT NULL,candidate TEXT NOT NULL,receipt TEXT NOT NULL,FOREIGN KEY(candidate) REFERENCES candidates(id));
        CREATE TABLE IF NOT EXISTS edit_candidate_lineage(candidate TEXT PRIMARY KEY,receipt TEXT NOT NULL,review_state TEXT NOT NULL,authored_count INTEGER NOT NULL,inherited_count INTEGER NOT NULL,FOREIGN KEY(candidate) REFERENCES candidates(id));
        CREATE TABLE IF NOT EXISTS request_identities(session TEXT NOT NULL,request TEXT NOT NULL,fingerprint TEXT NOT NULL,PRIMARY KEY(session,request));
        INSERT OR IGNORE INTO request_identities SELECT session,request,fingerprint FROM requests;
        INSERT OR IGNORE INTO request_identities SELECT session,request,fingerprint FROM transfer_begins;").map_err(internal)?;
    let conflict:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM requests r JOIN request_identities i ON r.session=i.session AND r.request=i.request WHERE r.fingerprint<>i.fingerprint)
        OR EXISTS(SELECT 1 FROM transfer_begins r JOIN request_identities i ON r.session=i.session AND r.request=i.request WHERE r.fingerprint<>i.fingerprint)",[],|row|row.get(0)).map_err(internal)?;
    if conflict {
        return Err(internal(
            "legacy request identity journals conflict; explicit recovery required",
        ));
    }
    tx.commit().map_err(internal)?;
    // Engine startup owns this private staging namespace. Incomplete leases
    // never survive the epoch change, so their regular staging files are removed.
    let parent = Path::new(
        db.path()
            .ok_or_else(|| internal("file-backed transfer database required"))?,
    )
    .parent()
    .ok_or_else(|| internal("state path has no parent"))?;
    let staging = parent.join("transfer-staging");
    for entry in fs::read_dir(&staging).map_err(unavailable)? {
        let entry = entry.map_err(unavailable)?;
        let meta = fs::symlink_metadata(entry.path()).map_err(unavailable)?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(internal("untrusted nonregular transfer staging entry"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != unsafe { libc::geteuid() } {
                return Err(internal("transfer staging owner mismatch"));
            }
        }
        fs::remove_file(entry.path()).map_err(unavailable)?;
    }
    artifacts::sync_dir(&staging).map_err(unavailable)?;
    Ok(())
}
pub(super) fn start_sweeper(engine: &Arc<Engine>) {
    let engine = Arc::downgrade(engine);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        let Some(engine) = engine.upgrade() else {
            break;
        };
        if let Ok(mut transfers) = engine.transfers.lock() {
            transfers.sweep();
        };
    });
}
fn grant(direction: TransferDirection) -> Scope {
    if direction == TransferDirection::Upload {
        Scope::Edit
    } else {
        Scope::Read
    }
}

impl Engine {
    pub(super) fn transfer_control(
        &self,
        request: &Request,
        fingerprint: &str,
    ) -> PResult<ResponseBody> {
        match &request.command {
            Command::BeginEditUpload { .. } | Command::BeginMotionDownload { .. } => {
                self.begin_transfer(request, fingerprint)
            }
            Command::FinishEditUpload { lease_id } => {
                self.finish_edit_upload(request, fingerprint, lease_id)
            }
            Command::AbandonTransfer { lease_id } => {
                self.abandon_transfer(request, fingerprint, lease_id)
            }
            Command::TransferStatus { lease_id } => {
                let db = self.db.lock().map_err(internal)?;
                self.authenticate(
                    &db,
                    request.session.as_ref().unwrap(),
                    request.auth_token.as_deref(),
                )?;
                let mut transfers = self.transfers.lock().map_err(internal)?;
                transfers.sweep();
                let lease = transfers.leases.get_mut(lease_id.as_str()).ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::NotFound,
                        "transfer absent, expired, or invalidated by restart",
                    )
                })?;
                lease.check(
                    request.session.as_ref().unwrap(),
                    project(request)?,
                    &self.engine_epoch,
                )?;
                require_grant(&db, &lease.session, &lease.project, grant(lease.direction))?;
                Ok(ResponseBody::Transfer(
                    lease.view(self.config.bulk_endpoint())?,
                ))
            }
            _ => Err(ProtocolError::invalid("not a transfer command")),
        }
    }
    fn abandon_transfer(
        &self,
        request: &Request,
        fingerprint: &str,
        lease_id: &TransferId,
    ) -> PResult<ResponseBody> {
        let session = request.session.as_ref().unwrap();
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        self.authenticate(&tx, session, request.auth_token.as_deref())?;
        if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
            return Ok(body);
        }
        let mut transfers = self.transfers.lock().map_err(internal)?;
        let lease = transfers.leases.get_mut(lease_id.as_str()).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::NotFound,
                "transfer already expired or invalidated by restart",
            )
        })?;
        if &lease.session != session || &lease.project != project(request)? {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "transfer belongs to another session or project",
            ));
        }
        claim_request_identity(&tx, session.as_str(), request, fingerprint)?;
        let body = ResponseBody::TransferAbandoned {
            lease_id: lease_id.clone(),
        };
        remember(&tx, session.as_str(), request, fingerprint, &body)?;
        tx.commit().map_err(internal)?;
        lease.cancelled = true;
        lease.release();
        Ok(body)
    }
    fn prior_begin(
        &self,
        db: &Connection,
        request: &Request,
        fingerprint: &str,
    ) -> PResult<Option<ResponseBody>> {
        check_request_identity(
            db,
            request.session.as_ref().unwrap().as_str(),
            request,
            fingerprint,
        )?;
        let old:Option<(String,String,String)>=db.query_row("SELECT fingerprint,lease_id,engine_epoch FROM transfer_begins WHERE session=?1 AND request=?2",
            params![request.session.as_ref().unwrap().as_str(),request.request_id.as_str()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(internal)?;
        let Some((old, lease_id, epoch)) = old else {
            return Ok(None);
        };
        if old != fingerprint {
            return Err(ProtocolError::new(
                ErrorCode::RequestConflict,
                "request identity reused for a different transfer",
            ));
        }
        if epoch != self.engine_epoch {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                "unfinished transfer invalidated by engine restart; begin a new request explicitly",
            ));
        }
        let transfers = self.transfers.lock().map_err(internal)?;
        let lease = transfers.leases.get(&lease_id).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::NotFound,
                "prior transfer expired or was abandoned",
            )
        })?;
        lease.check(
            request.session.as_ref().unwrap(),
            project(request)?,
            &self.engine_epoch,
        )?;
        Ok(Some(ResponseBody::Transfer(
            lease.view(self.config.bulk_endpoint())?,
        )))
    }
    fn begin_transfer(&self, request: &Request, fingerprint: &str) -> PResult<ResponseBody> {
        let session = request.session.as_ref().unwrap();
        let (direction, sha256, total, offset, length, base, label, descriptor) = {
            let db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, session)?;
            if let Some(body) = self.prior_begin(&db, request, fingerprint)? {
                return Ok(body);
            }
            check_revision(&db, request)?;
            match &request.command {
                Command::BeginEditUpload {
                    byte_len,
                    sha256,
                    label,
                } => {
                    let revision: i64 = db
                        .query_row(
                            "SELECT revision FROM projects WHERE id=?1",
                            [project(request)?.as_str()],
                            |row| row.get(0),
                        )
                        .map_err(internal)?;
                    (
                        TransferDirection::Upload,
                        sha256.clone(),
                        *byte_len,
                        0,
                        *byte_len,
                        RevisionId::new(revision as u64),
                        label.clone(),
                        None,
                    )
                }
                Command::BeginMotionDownload {
                    locator,
                    offset,
                    byte_len,
                } => {
                    let descriptor = match locator {
                        MotionLocator::ProjectRevision { revision } => {
                            motion_state::revision_descriptor(&db, project(request)?, *revision)?
                        }
                        MotionLocator::Candidate { candidate_id } => {
                            candidate_snapshot(&db, candidate_id, project(request)?)?
                                .motion
                                .program
                        }
                    };
                    validate_motion_range(*offset, *byte_len, descriptor.byte_len)?;
                    (
                        TransferDirection::Download,
                        descriptor.sha256.clone(),
                        descriptor.byte_len,
                        *offset,
                        *byte_len,
                        RevisionId::new(0),
                        String::new(),
                        Some(descriptor),
                    )
                }
                _ => return Err(ProtocolError::invalid("not a begin-transfer command")),
            }
        };
        let reservation = self
            .pool
            .reserve_memory(if direction == TransferDirection::Upload {
                VALIDATION_MEMORY
            } else {
                2 * MAX_ARTIFACT_CHUNK_BYTES as u64
            })?;
        let reservation = if direction == TransferDirection::Upload {
            reservation.with_storage(
                total + MAX_MOTION_BYTES + 2 * RECEIPT_LIMIT as u64,
                fs2::available_space(&self.config.state_dir).map_err(unavailable)?,
            )?
        } else {
            reservation
        };
        let (file, staging) = if let Some(descriptor) = descriptor {
            (self.motion_store.open_verified(&descriptor)?, None)
        } else {
            let temporary =
                tempfile::NamedTempFile::new_in(self.config.state_dir.join("transfer-staging"))
                    .map_err(unavailable)?;
            (
                temporary.as_file().try_clone().map_err(unavailable)?,
                Some(temporary),
            )
        };
        let lease_id = id!(TransferId);
        let lease = Lease {
            id: lease_id.clone(),
            session: session.clone(),
            project: project(request)?.clone(),
            epoch: self.engine_epoch.clone(),
            direction,
            sha256,
            total,
            offset,
            length,
            accepted: 0,
            deadline: Instant::now() + LEASE_LIFETIME,
            base,
            label,
            file: Some(file),
            staging,
            reservation: Some(reservation),
            chunks: HashMap::new(),
            finalizing: false,
            cancelled: false,
            completed: false,
        };
        let mut db = self.db.lock().map_err(internal)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(internal)?;
        self.authorize(&tx, request, session)?;
        if let Some(body) = self.prior_begin(&tx, request, fingerprint)? {
            return Ok(body);
        }
        check_revision(&tx, request)?;
        let mut transfers = self.transfers.lock().map_err(internal)?;
        transfers.sweep();
        if transfers
            .leases
            .values()
            .filter(|lease| lease.reservation.is_some())
            .count()
            >= MAX_LEASES
        {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "aggregate transfer lease admission exhausted",
            ));
        }
        let view = lease.view(self.config.bulk_endpoint())?;
        claim_request_identity(&tx, session.as_str(), request, fingerprint)?;
        tx.execute(
            "INSERT INTO transfer_begins VALUES(?1,?2,?3,?4,?5)",
            params![
                session.as_str(),
                request.request_id.as_str(),
                fingerprint,
                lease_id.as_str(),
                self.engine_epoch
            ],
        )
        .map_err(internal)?;
        tx.commit().map_err(internal)?;
        transfers.leases.insert(lease_id.to_string(), lease);
        Ok(ResponseBody::Transfer(view))
    }
    pub(crate) fn bulk_ready(&self, handshake: &BulkHandshake) -> PResult<TransferLease> {
        handshake.validate()?;
        let db = self.db.lock().map_err(internal)?;
        self.authenticate(&db, &handshake.session, Some(&handshake.auth_token))?;
        let mut transfers = self.transfers.lock().map_err(internal)?;
        transfers.sweep();
        let lease = transfers
            .leases
            .get(handshake.lease_id.as_str())
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::NotFound,
                    "transfer absent or invalidated by restart",
                )
            })?;
        lease.check(&handshake.session, &lease.project, &handshake.engine_epoch)?;
        require_grant(
            &db,
            &handshake.session,
            &lease.project,
            grant(lease.direction),
        )?;
        lease.view(self.config.bulk_endpoint())
    }
    pub(crate) fn bulk_upload(
        &self,
        handshake: &BulkHandshake,
        offset: u64,
        bytes: &[u8],
    ) -> PResult<u64> {
        let operation = BulkOperation::Upload {
            offset,
            byte_len: u32::try_from(bytes.len()).map_err(internal)?,
        };
        operation.validate()?;
        let db = self.db.lock().map_err(internal)?;
        self.authenticate(&db, &handshake.session, Some(&handshake.auth_token))?;
        let mut transfers = self.transfers.lock().map_err(internal)?;
        let lease = transfers
            .leases
            .get_mut(handshake.lease_id.as_str())
            .ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "transfer absent"))?;
        lease.check(&handshake.session, &lease.project, &handshake.engine_epoch)?;
        require_grant(&db, &handshake.session, &lease.project, Scope::Edit)?;
        if lease.direction != TransferDirection::Upload || lease.finalizing || lease.completed {
            return Err(ProtocolError::invalid(
                "transfer is not accepting uploaded chunks",
            ));
        }
        drop(db);
        let digest = format!("{:x}", Sha256::digest(bytes));
        if let Some((len, hash)) = lease.chunks.get(&offset) {
            if *len as usize == bytes.len() && *hash == digest {
                return Ok(lease.accepted);
            }
            return Err(ProtocolError::new(
                ErrorCode::RequestConflict,
                "uploaded retry differs from acknowledged chunk",
            ));
        }
        if offset != lease.accepted
            || offset
                .checked_add(bytes.len() as u64)
                .is_none_or(|end| end > lease.length)
        {
            return Err(ProtocolError::invalid(
                "upload offset overlaps, skips, or exceeds admitted range",
            ));
        }
        if lease.chunks.len() >= MAX_REPLAY_CHUNKS {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "transfer chunk-count admission exhausted",
            ));
        }
        let file = lease
            .file
            .as_mut()
            .ok_or_else(|| ProtocolError::invalid("upload staging file unavailable"))?;
        file.seek(SeekFrom::Start(offset)).map_err(unavailable)?;
        file.write_all(bytes).map_err(unavailable)?;
        lease.chunks.insert(offset, (bytes.len() as u32, digest));
        lease.accepted += bytes.len() as u64;
        Ok(lease.accepted)
    }
    pub(crate) fn bulk_download(
        &self,
        handshake: &BulkHandshake,
        offset: u64,
        byte_len: u32,
    ) -> PResult<Vec<u8>> {
        BulkOperation::Download { offset, byte_len }.validate()?;
        let db = self.db.lock().map_err(internal)?;
        self.authenticate(&db, &handshake.session, Some(&handshake.auth_token))?;
        let mut transfers = self.transfers.lock().map_err(internal)?;
        let lease = transfers
            .leases
            .get_mut(handshake.lease_id.as_str())
            .ok_or_else(|| ProtocolError::new(ErrorCode::NotFound, "transfer absent"))?;
        lease.check(&handshake.session, &lease.project, &handshake.engine_epoch)?;
        require_grant(&db, &handshake.session, &lease.project, Scope::Read)?;
        if lease.direction != TransferDirection::Download {
            return Err(ProtocolError::invalid("not a download lease"));
        }
        let end = offset
            .checked_add(u64::from(byte_len))
            .ok_or_else(|| ProtocolError::invalid("chunk offset overflow"))?;
        if offset < lease.offset || end > lease.offset + lease.length {
            return Err(ProtocolError::invalid("download outside admitted range"));
        }
        let retry = lease
            .chunks
            .get(&offset)
            .is_some_and(|(length, _)| *length == byte_len);
        if !retry && offset != lease.offset + lease.accepted {
            return Err(ProtocolError::invalid(
                "download offset is not sequential or an exact retry",
            ));
        }
        if !retry && lease.chunks.len() >= MAX_REPLAY_CHUNKS {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "transfer chunk-count admission exhausted",
            ));
        }
        drop(db);
        let mut bytes = vec![0; byte_len as usize];
        let file = lease
            .file
            .as_mut()
            .ok_or_else(|| ProtocolError::invalid("download file unavailable"))?;
        file.seek(SeekFrom::Start(offset)).map_err(unavailable)?;
        file.read_exact(&mut bytes).map_err(unavailable)?;
        if !retry {
            lease.chunks.insert(offset, (byte_len, String::new()));
            lease.accepted += u64::from(byte_len);
        }
        if lease.accepted == lease.length {
            lease.completed = true;
            lease.reservation.take();
        }
        Ok(bytes)
    }
    fn finish_edit_upload(
        &self,
        request: &Request,
        fingerprint: &str,
        lease_id: &TransferId,
    ) -> PResult<ResponseBody> {
        let session = request.session.as_ref().unwrap();
        let (mut input, path, base, base_descriptor, prior, total, digest, label, actor_id) = {
            let mut db = self.db.lock().map_err(internal)?;
            self.authorize(&db, request, session)?;
            if let Some(body) = replay(&db, session.as_str(), request, fingerprint)? {
                return Ok(body);
            }
            let done:Option<String>=db.query_row("SELECT candidate FROM edit_upload_receipts WHERE lease_id=?1 AND session=?2 AND project=?3",
                params![lease_id.as_str(),session.as_str(),project(request)?.as_str()],|row|row.get(0)).optional().map_err(internal)?;
            if let Some(candidate) = done {
                let tx = db
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(internal)?;
                let body = ResponseBody::Candidate(candidate_snapshot(
                    &tx,
                    &CandidateId::new(candidate).map_err(internal)?,
                    project(request)?,
                )?);
                remember(&tx, session.as_str(), request, fingerprint, &body)?;
                tx.commit().map_err(internal)?;
                return Ok(body);
            }
            let mut transfers = self.transfers.lock().map_err(internal)?;
            let lease = transfers.leases.get_mut(lease_id.as_str()).ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::NotFound,
                    "upload absent or invalidated by restart",
                )
            })?;
            lease.check(session, project(request)?, &self.engine_epoch)?;
            if lease.direction != TransferDirection::Upload
                || lease.accepted != lease.length
                || lease.finalizing
            {
                return Err(ProtocolError::invalid(
                    "upload incomplete or finalization already in progress",
                ));
            }
            let file = lease
                .file
                .as_ref()
                .ok_or_else(|| ProtocolError::invalid("upload file unavailable"))?
                .try_clone()
                .map_err(unavailable)?;
            let path = lease
                .staging
                .as_ref()
                .ok_or_else(|| ProtocolError::invalid("upload staging unavailable"))?
                .path()
                .to_path_buf();
            let descriptor = motion_state::revision_descriptor(&db, &lease.project, lease.base)?;
            let prior = lineage::state(&db, &lease.project, lease.base)?;
            let actor_id = actor(&db, session)?;
            lease.finalizing = true;
            (
                file,
                path,
                lease.base,
                descriptor,
                prior,
                lease.total,
                lease.sha256.clone(),
                lease.label.clone(),
                actor_id,
            )
        };
        let result = (|| {
            input.sync_all().map_err(unavailable)?;
            let (actual, length) =
                artifacts::digest_file(&path, MAX_MOTION_BYTES).map_err(unavailable)?;
            if length != total || actual != digest {
                return Err(ProtocolError::new(
                    ErrorCode::DependencyMismatch,
                    "complete edit upload length or SHA-256 mismatch",
                ));
            }
            input.seek(SeekFrom::Start(0)).map_err(unavailable)?;
            let values: pulsar_core::EditValuesProgram =
                serde_json::from_reader(input.take(MAX_MOTION_BYTES + 1)).map_err(|error| {
                    ProtocolError::invalid(format!("invalid edit-values payload: {error}"))
                })?;
            let original = self.motion_store.read(&base_descriptor)?;
            let authored = pulsar_core::ProvenanceRef::new(format!("edit:{}", lease_id.as_str()))
                .map_err(internal)?;
            let reconciled = pulsar_core::reconcile_edit_values(&original, &values, &authored)
                .map_err(|error| ProtocolError::invalid(error.to_string()))?;
            let output = self.motion_store.publish(&reconciled.program)?;
            let mut review_state = prior;
            review_state.flags = lineage::clipped(
                &review_state.flags,
                &lineage::coverage(&reconciled.program)?,
            )?;
            // Review UI uses explicitly conservative envelopes; exact authored and
            // inherited ranges are preserved in the immutable receipt below.
            for axis in Axis::ALL {
                let ranges: Vec<_> = reconciled
                    .authored_ranges
                    .iter()
                    .filter(|(a, _)| *a == axis)
                    .map(|(_, range)| *range)
                    .collect();
                if let (Some(start), Some(end)) = (
                    ranges.iter().map(|r| r.start()).min(),
                    ranges.iter().map(|r| r.end()).max(),
                ) {
                    review_state.flags.push(ReviewFlag {
                        axis: Some(axis),
                        range: TimeRange::new(start, end).map_err(internal)?,
                        reason: ReviewReason::new("authored_edit_envelope")?,
                        evidence: EvidenceKind::Synthesized,
                        confidence: None,
                    });
                }
            }
            review_state.conservative = true;
            validate_reviews(&review_state.flags).map_err(|error| {
                ProtocolError::new(ErrorCode::ResourceExhausted, error.to_string())
            })?;
            #[derive(Serialize)]
            struct EditReceipt<'a> {
                format: &'static str,
                kernel: &'static str,
                lease_id: &'a TransferId,
                project_id: &'a ProjectId,
                base_revision: RevisionId,
                base_program: &'a ProgramDescriptor,
                upload_sha256: &'a str,
                upload_byte_len: u64,
                result_program: &'a ProgramDescriptor,
                authored_provenance: &'a pulsar_core::ProvenanceRef,
                authored_ranges: &'a [(Axis, TimeRange)],
                inherited_ranges: &'a [(Axis, TimeRange)],
                actor_id: &'a str,
                label: &'a str,
            }
            let receipt_value = EditReceipt {
                format: "pulsar-edit-receipt-v1",
                kernel: pulsar_core::EDIT_VALUES_KERNEL_VERSION,
                lease_id,
                project_id: project(request)?,
                base_revision: base,
                base_program: &base_descriptor,
                upload_sha256: &digest,
                upload_byte_len: total,
                result_program: &output,
                authored_provenance: &reconciled.authored_provenance,
                authored_ranges: &reconciled.authored_ranges,
                inherited_ranges: &reconciled.inherited_ranges,
                actor_id: &actor_id,
                label: &label,
            };
            let receipt_bytes = bounded_json(&receipt_value, RECEIPT_LIMIT)?;
            let receipt_sha = format!("{:x}", Sha256::digest(&receipt_bytes));
            let receipt_path = self
                .config
                .state_dir
                .join("edit-receipts")
                .join(&receipt_sha);
            if !receipt_path.exists() {
                artifacts::publish_export(&receipt_path, &receipt_bytes).map_err(unavailable)?;
            } else {
                let (sha, len) = artifacts::digest_file(&receipt_path, RECEIPT_LIMIT as u64)
                    .map_err(unavailable)?;
                if sha != receipt_sha || len != receipt_bytes.len() as u64 {
                    return Err(internal("immutable edit receipt identity conflict"));
                }
            }
            let (_, upload_sha, upload_length) = artifacts::snapshot_reserved(
                &path,
                &self.config.state_dir.join("edit-inputs"),
                MAX_MOTION_BYTES,
                0,
            )
            .map_err(unavailable)?;
            let receipt = ArtifactIdentity {
                artifact_id: ArtifactId::new(format!("edit-receipt.{receipt_sha}"))
                    .map_err(internal)?,
                sha256: receipt_sha,
                byte_len: receipt_bytes.len() as u64,
            };
            let uploaded = ArtifactIdentity {
                artifact_id: ArtifactId::new(format!("edit-input.{upload_sha}"))
                    .map_err(internal)?,
                sha256: upload_sha,
                byte_len: upload_length,
            };
            let mut db = self.db.lock().map_err(internal)?;
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(internal)?;
            self.authorize(&tx, request, session)?;
            if let Some(body) = replay(&tx, session.as_str(), request, fingerprint)? {
                return Ok(body);
            }
            let mut transfers = self.transfers.lock().map_err(internal)?;
            let lease = transfers.leases.get_mut(lease_id.as_str()).ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::NotFound,
                    "upload disappeared during finalization",
                )
            })?;
            lease.check(session, project(request)?, &self.engine_epoch)?;
            if !lease.finalizing || lease.base != base || lease.accepted != total {
                return Err(ProtocolError::invalid(
                    "upload state changed during finalization",
                ));
            }
            claim_request_identity(&tx, session.as_str(), request, fingerprint)?;
            let candidate = id!(CandidateId);
            let reference = motion_state::register(&tx, &output)?;
            tx.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,?3,?4,NULL,?5,?6)",
                params![candidate.as_str(),project(request)?.as_str(),rev_sql(base)?,reference,
                    encode(&vec![uploaded,receipt.clone(),ArtifactIdentity{artifact_id:base_descriptor.artifact_id.clone(),sha256:base_descriptor.sha256.clone(),byte_len:base_descriptor.byte_len}])?,
                    encode(&review_state.flags)?]).map_err(internal)?;
            tx.execute(
                "INSERT INTO edit_upload_receipts VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    lease_id.as_str(),
                    project(request)?.as_str(),
                    session.as_str(),
                    rev_sql(base)?,
                    candidate.as_str(),
                    encode(&receipt)?
                ],
            )
            .map_err(internal)?;
            tx.execute(
                "INSERT INTO edit_candidate_lineage VALUES(?1,?2,?3,?4,?5)",
                params![
                    candidate.as_str(),
                    encode(&receipt)?,
                    encode(&review_state)?,
                    reconciled.authored_ranges.len() as i64,
                    reconciled.inherited_ranges.len() as i64
                ],
            )
            .map_err(internal)?;
            let body =
                ResponseBody::Candidate(candidate_snapshot(&tx, &candidate, project(request)?)?);
            event(
                &tx,
                project(request)?,
                base,
                EventBody::CandidateReady {
                    candidate_id: candidate,
                },
            )?;
            remember(&tx, session.as_str(), request, fingerprint, &body)?;
            tx.commit().map_err(internal)?;
            lease.finalizing = false;
            lease.completed = true;
            lease.release();
            Ok(body)
        })();
        if let Ok(mut transfers) = self.transfers.lock() {
            if let Some(lease) = transfers.leases.get_mut(lease_id.as_str()) {
                lease.finalizing = false;
                if result.is_err() && (lease.cancelled || Instant::now() >= lease.deadline) {
                    lease.release();
                }
            }
        }
        result
    }
}
fn bounded_json(value: &impl Serialize, limit: usize) -> PResult<Vec<u8>> {
    struct Buffer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "immutable receipt size admission exceeded",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Buffer {
        bytes: vec![],
        limit,
    };
    serde_json::to_writer(&mut out, value)
        .map_err(|error| ProtocolError::new(ErrorCode::ResourceExhausted, error.to_string()))?;
    Ok(out.bytes)
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
        fn begin(&self, bytes: &[u8]) -> (TransferLease, BulkHandshake) {
            let lease = match self
                .call(
                    Command::BeginEditUpload {
                        byte_len: bytes.len() as u64,
                        sha256: format!("{:x}", Sha256::digest(bytes)),
                        label: "test edit".into(),
                    },
                    Some(0),
                )
                .unwrap()
            {
                ResponseBody::Transfer(lease) => lease,
                _ => panic!(),
            };
            let handshake = BulkHandshake {
                version: PROTOCOL_VERSION,
                session: self.session.clone(),
                auth_token: self.token.clone(),
                lease_id: lease.lease_id.clone(),
                engine_epoch: lease.engine_epoch.clone(),
            };
            (lease, handshake)
        }
    }
    fn fixture(memory: u64) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("worker");
        fs::write(&executable, b"nonexecuted transfer fixture").unwrap();
        let mut config = EngineConfig::new(dir.path().join("state"), executable);
        config.max_ram_bytes = memory;
        let engine = Engine::open(config).unwrap();
        let (session, token) = match engine
            .handle(Request::new(
                id!(RequestId),
                Command::Pair {
                    client_name: "transfer tests".into(),
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
                        name: "transfers".into(),
                    },
                )
                .with_session(session.clone())
                .with_auth_token(token.clone()),
            )
            .result
            .unwrap()
        {
            ResponseBody::Project(snapshot) => snapshot.project_id,
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
    fn values() -> Vec<u8> {
        br#"{"tracks":[{"axis":"stroke","actions":[{"time":0,"position":0.2},{"time":1000000000,"position":0.8}],"gaps":[]}]}"#.to_vec()
    }
    fn assert_empty(f: &Fixture) {
        let db = f.engine.db.lock().unwrap();
        assert_eq!(
            project_snapshot(&db, &f.project).unwrap().revision.value(),
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM candidates", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn upload_chunks_are_authenticated_exact_replay_and_sequential() {
        let f = fixture(4 * VALIDATION_MEMORY);
        let bytes = values();
        let (lease, handshake) = f.begin(&bytes);
        let mut bad = handshake.clone();
        bad.auth_token = "wrong".into();
        assert_eq!(
            f.engine.bulk_ready(&bad).unwrap_err().code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            f.engine
                .bulk_upload(&handshake, 1, &bytes[..4])
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(f.engine.bulk_upload(&handshake, 0, &bytes[..4]).unwrap(), 4);
        assert_eq!(f.engine.bulk_upload(&handshake, 0, &bytes[..4]).unwrap(), 4);
        assert_eq!(
            f.engine
                .bulk_upload(&handshake, 0, b"nope")
                .unwrap_err()
                .code,
            ErrorCode::RequestConflict
        );
        assert!(f.engine.bulk_upload(&handshake, 2, &bytes[2..4]).is_err());
        assert_eq!(
            f.call(
                Command::FinishEditUpload {
                    lease_id: lease.lease_id.clone()
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            f.engine.bulk_upload(&handshake, 4, &bytes[4..]).unwrap(),
            bytes.len() as u64
        );
        let ResponseBody::Candidate(candidate) = f
            .call(
                Command::FinishEditUpload {
                    lease_id: lease.lease_id,
                },
                None,
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(candidate.motion.program.action_count(), 2);
        assert_eq!(f.engine.pool.reserved_storage(), 0);
        let db = f.engine.db.lock().unwrap();
        assert_eq!(
            project_snapshot(&db, &f.project).unwrap().revision.value(),
            0
        );
    }
    #[test]
    fn upload_edit_grant_is_not_read_or_generate_and_revocation_fences_chunks() {
        let f = fixture(4 * VALIDATION_MEMORY);
        let bytes = values();
        f.engine
            .db
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope IN (?3,?4)",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::Read),
                    scope_name(Scope::Generate)
                ],
            )
            .unwrap();
        let (lease, handshake) = f.begin(&bytes);
        assert!(f
            .call(
                Command::TransferStatus {
                    lease_id: lease.lease_id.clone()
                },
                None
            )
            .is_ok());
        assert_eq!(
            f.call(
                Command::BeginMotionDownload {
                    locator: MotionLocator::ProjectRevision {
                        revision: RevisionId::new(0)
                    },
                    offset: 0,
                    byte_len: 1
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::Forbidden
        );
        f.engine.bulk_upload(&handshake, 0, &bytes[..4]).unwrap();
        f.engine
            .db
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
                params![
                    f.session.as_str(),
                    f.project.as_str(),
                    scope_name(Scope::Edit)
                ],
            )
            .unwrap();
        assert_eq!(
            f.engine
                .bulk_upload(&handshake, 4, &bytes[4..])
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            f.call(
                Command::FinishEditUpload {
                    lease_id: lease.lease_id.clone()
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::Forbidden
        );
        assert!(f
            .call(
                Command::AbandonTransfer {
                    lease_id: lease.lease_id
                },
                None
            )
            .is_ok());
        assert_eq!(f.engine.pool.reserved_storage(), 0);
        assert_empty(&f);
    }
    #[test]
    fn aggregate_upload_reservations_expiry_and_remaining_lifetime_are_bounded() {
        let f = fixture(VALIDATION_MEMORY);
        let bytes = values();
        let (lease, handshake) = f.begin(&bytes);
        let response = f.call(
            Command::BeginEditUpload {
                byte_len: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                label: "second".into(),
            },
            Some(0),
        );
        assert_eq!(response.unwrap_err().code, ErrorCode::ResourceExhausted);
        f.engine
            .transfers
            .lock()
            .unwrap()
            .leases
            .get_mut(lease.lease_id.as_str())
            .unwrap()
            .deadline -= Duration::from_secs(1);
        let current = f.engine.bulk_ready(&handshake).unwrap();
        assert!(current.expires_after_ms + 500 < lease.expires_after_ms);
        f.engine
            .transfers
            .lock()
            .unwrap()
            .leases
            .get_mut(lease.lease_id.as_str())
            .unwrap()
            .deadline = Instant::now();
        assert!(f.engine.bulk_ready(&handshake).is_err());
        assert_eq!(f.engine.pool.reserved_storage(), 0);
        f.begin(&bytes);
    }
    #[test]
    fn forged_edit_authority_and_digest_mismatch_publish_nothing() {
        for bytes in [
            br#"{"tracks":[{"axis":"stroke","actions":[{"time":0,"position":0.2,"evidence":"observed"}],"gaps":[]}]}"#.to_vec(),
            br#"{"tracks":[],"provenance":"trusted-worker"}"#.to_vec(),
        ] {
            let f=fixture(4*VALIDATION_MEMORY);let (lease,handshake)=f.begin(&bytes);
            f.engine.bulk_upload(&handshake,0,&bytes).unwrap();
            assert!(f.call(Command::FinishEditUpload{lease_id:lease.lease_id},None).is_err());assert_empty(&f);
        }
        let f = fixture(4 * VALIDATION_MEMORY);
        let bytes = values();
        let (lease, handshake) = f.begin(&bytes);
        let mut corrupted = bytes.clone();
        corrupted[0] = b' ';
        f.engine.bulk_upload(&handshake, 0, &corrupted).unwrap();
        assert_eq!(
            f.call(
                Command::FinishEditUpload {
                    lease_id: lease.lease_id
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::DependencyMismatch
        );
        assert_empty(&f);
    }
    #[test]
    fn failed_receipt_io_and_sql_publication_never_publish_partial_authority() {
        let f = fixture(4 * VALIDATION_MEMORY);
        let bytes = values();
        let (lease, handshake) = f.begin(&bytes);
        f.engine.bulk_upload(&handshake, 0, &bytes).unwrap();
        let receipts = f.engine.config.state_dir.join("edit-receipts");
        let moved = f.engine.config.state_dir.join("receipts-held");
        fs::rename(&receipts, &moved).unwrap();
        fs::write(&receipts, b"not a directory").unwrap();
        let finish = f.request(
            Command::FinishEditUpload {
                lease_id: lease.lease_id.clone(),
            },
            None,
        );
        assert!(f.engine.handle(finish.clone()).result.is_err());
        assert_empty(&f);
        fs::remove_file(&receipts).unwrap();
        fs::rename(&moved, &receipts).unwrap();
        f.engine.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER reject_candidate BEFORE INSERT ON candidates BEGIN SELECT RAISE(ABORT,'injected publication failure'); END;").unwrap();
        assert!(f.engine.handle(finish.clone()).result.is_err());
        assert_empty(&f);
        {
            let db = f.engine.db.lock().unwrap();
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM edit_upload_receipts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                0
            );
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM requests WHERE request=?1",
                    [finish.request_id.as_str()],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            db.execute_batch("DROP TRIGGER reject_candidate;").unwrap();
        }
        assert!(f.engine.handle(finish).result.is_ok());
    }

    #[test]
    fn completed_upload_alias_claims_request_identity_and_replays_after_restart() {
        let f = fixture(4 * VALIDATION_MEMORY);
        let bytes = values();
        let (lease, handshake) = f.begin(&bytes);
        f.engine.bulk_upload(&handshake, 0, &bytes).unwrap();
        let first = f
            .call(
                Command::FinishEditUpload {
                    lease_id: lease.lease_id.clone(),
                },
                None,
            )
            .unwrap();
        let alias = f.request(
            Command::FinishEditUpload {
                lease_id: lease.lease_id,
            },
            None,
        );
        assert_eq!(
            encode(&f.engine.handle(alias.clone()).result.unwrap()).unwrap(),
            encode(&first).unwrap()
        );
        let reserved = f.engine.pool.reserved_storage();
        let mut conflicting = f.request(
            Command::BeginEditUpload {
                byte_len: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                label: "must not allocate".into(),
            },
            Some(0),
        );
        conflicting.request_id = alias.request_id.clone();
        assert_eq!(
            f.engine
                .handle(conflicting.clone())
                .result
                .unwrap_err()
                .code,
            ErrorCode::RequestConflict
        );
        assert_eq!(f.engine.pool.reserved_storage(), reserved);
        {
            let db = f.engine.db.lock().unwrap();
            for table in ["requests", "request_identities"] {
                assert_eq!(
                    db.query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE session=?1 AND request=?2"),
                        params![f.session.as_str(), alias.request_id.as_str()],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1
                );
            }
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM candidates", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        let Fixture {
            engine,
            session: _,
            token: _,
            project: _,
            dir,
        } = f;
        let config = engine.config.clone();
        drop(engine);
        let engine = Engine::open(config).unwrap();
        assert_eq!(
            encode(&engine.handle(alias).result.unwrap()).unwrap(),
            encode(&first).unwrap()
        );
        assert_eq!(
            engine.handle(conflicting).result.unwrap_err().code,
            ErrorCode::RequestConflict
        );
        drop(dir);
    }

    #[test]
    fn completed_finalization_replays_durably_but_unfinished_epoch_does_not() {
        let f = fixture(4 * VALIDATION_MEMORY);
        let bytes = values();
        let (lease, handshake) = f.begin(&bytes);
        f.engine.bulk_upload(&handshake, 0, &bytes).unwrap();
        let finish = f.request(
            Command::FinishEditUpload {
                lease_id: lease.lease_id.clone(),
            },
            None,
        );
        let first = f.engine.handle(finish.clone()).result.unwrap();
        let (_, unfinished) = f.begin(&bytes);
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
        assert!(engine.bulk_ready(&unfinished).is_err());
        assert_eq!(
            encode(&engine.handle(finish).result.unwrap()).unwrap(),
            encode(&first).unwrap()
        );
        let replay = Request::new(
            id!(RequestId),
            Command::FinishEditUpload {
                lease_id: lease.lease_id,
            },
        )
        .with_session(session)
        .with_auth_token(token)
        .in_project(project, None);
        assert_eq!(
            encode(&engine.handle(replay).result.unwrap()).unwrap(),
            encode(&first).unwrap()
        );
        drop(dir);
    }
    #[test]
    fn expired_idle_upload_releases_admission_without_another_transfer_call() {
        let f = fixture(VALIDATION_MEMORY);
        let bytes = values();
        let (lease, _) = f.begin(&bytes);
        f.engine
            .transfers
            .lock()
            .unwrap()
            .leases
            .get_mut(lease.lease_id.as_str())
            .unwrap()
            .deadline = Instant::now();
        f.call(Command::Capabilities, None).unwrap();
        assert_eq!(f.engine.pool.reserved_storage(), 0);
        let (lease, _) = f.begin(&bytes);
        f.engine
            .transfers
            .lock()
            .unwrap()
            .leases
            .get_mut(lease.lease_id.as_str())
            .unwrap()
            .deadline = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(2);
        while f.engine.pool.reserved_storage() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(f.engine.pool.reserved_storage(), 0);
    }
    #[test]
    fn abandon_replays_after_sweep_and_restart_without_releasing_a_running_validator_early() {
        let f = fixture(VALIDATION_MEMORY);
        let bytes = values();
        let (lease, _) = f.begin(&bytes);
        f.engine
            .transfers
            .lock()
            .unwrap()
            .leases
            .get_mut(lease.lease_id.as_str())
            .unwrap()
            .finalizing = true;
        let abandon = f.request(
            Command::AbandonTransfer {
                lease_id: lease.lease_id.clone(),
            },
            None,
        );
        let first = f.engine.handle(abandon.clone()).result.unwrap();
        assert!(f.engine.pool.reserved_storage() > 0);
        {
            let mut transfers = f.engine.transfers.lock().unwrap();
            let lease = transfers.leases.get_mut(lease.lease_id.as_str()).unwrap();
            lease.finalizing = false;
            lease.release();
            transfers.sweep();
        }
        assert_eq!(f.engine.pool.reserved_storage(), 0);
        assert_eq!(
            encode(&f.engine.handle(abandon.clone()).result.unwrap()).unwrap(),
            encode(&first).unwrap()
        );
        let Fixture {
            engine,
            session: _,
            token: _,
            project: _,
            dir,
        } = f;
        let config = engine.config.clone();
        drop(engine);
        let engine = Engine::open(config).unwrap();
        assert_eq!(
            encode(&engine.handle(abandon).result.unwrap()).unwrap(),
            encode(&first).unwrap()
        );
        drop(dir);
    }
    #[test]
    fn restart_removes_unfinished_private_staging_bytes() {
        let f = fixture(VALIDATION_MEMORY);
        let orphan = f
            .engine
            .config
            .state_dir
            .join("transfer-staging/crash-leftover");
        fs::write(&orphan, b"unfinished upload").unwrap();
        let Fixture {
            engine,
            session: _,
            token: _,
            project: _,
            dir,
        } = f;
        let config = engine.config.clone();
        drop(engine);
        let _engine = Engine::open(config).unwrap();
        assert!(!orphan.exists());
        drop(dir);
    }
    #[test]
    fn legacy_inline_cells_migrate_to_objects_while_original_json_is_retained() {
        let f = fixture(VALIDATION_MEMORY);
        let program = MotionProgram::new(vec![MotionTrack::new(
            Axis::Stroke,
            vec![MotionAction::new(
                ProjectTime::ZERO,
                NormalizedPosition::new(0.2).unwrap(),
                EvidenceKind::Observed,
            )
            .unwrap()],
        )
        .unwrap()])
        .unwrap();
        let raw = format!(" \n{}\n ", encode(&program).unwrap());
        {
            let db = f.engine.db.lock().unwrap();
            for table in ["projects", "revisions", "edit_states"] {
                db.execute(&format!("UPDATE {table} SET program=?1"), [&raw])
                    .unwrap();
            }
            db.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES('legacy-candidate',?1,0,?2,NULL,'[]','[]')",
                params![f.project.as_str(),raw]).unwrap();
        }
        let Fixture {
            engine,
            session: _,
            token: _,
            project,
            dir,
        } = f;
        let config = engine.config.clone();
        drop(engine);
        let engine = Engine::open(config).unwrap();
        let db = engine.db.lock().unwrap();
        assert_eq!(project_state(&db, &project).unwrap().program, program);
        let mut statement = db
            .prepare("SELECT program FROM legacy_motion_bytes")
            .unwrap();
        let originals = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(originals.len(), 4);
        assert!(originals.iter().all(|value| *value == raw));
        let reference: String = db
            .query_row(
                "SELECT program FROM projects WHERE id=?1",
                [project.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(reference.starts_with("motion:"));
        drop(statement);
        drop(db);
        drop(dir);
    }
}
