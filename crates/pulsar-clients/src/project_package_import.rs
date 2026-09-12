//! Shared clone-import client. Archive validation/publication belong to the engine.
use crate::session::{new_request_id, SessionClient};
use anyhow::{bail, ensure, Context, Result};
use pulsar_protocol::*;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const LOCAL_IMPORT_BUDGET: Duration = Duration::from_secs(30 * 60);
pub const PACKAGE_IMPORT_NOTICE: &str = "Import creates a NEW project. Archived identities and provenance grant no local authority; archived jobs, model paths and device state are never activated.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageImportPhase {
    Idle,
    Hashing,
    Prepared,
    Uploading,
    AwaitingPublication,
    Completed,
    Terminal,
}
#[derive(Default)]
struct ImportProgress {
    active: AtomicBool,
    cancelled: AtomicBool,
    phase: AtomicU8,
    hashed: AtomicU64,
    accepted: AtomicU64,
    verified: AtomicU64,
    total: AtomicU64,
}
#[derive(Clone, Default)]
pub struct PackageImportControl {
    inner: Arc<ImportProgress>,
}
impl PackageImportControl {
    /// Stops this local helper, not an already submitted engine operation.
    /// Explicit engine cancellation returns its actual terminal/publication status.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
    }
    pub fn hashed_bytes(&self) -> u64 {
        self.inner.hashed.load(Ordering::Acquire)
    }
    pub fn accepted_bytes(&self) -> u64 {
        self.inner.accepted.load(Ordering::Acquire)
    }
    pub fn verified_bytes(&self) -> u64 {
        self.inner.verified.load(Ordering::Acquire)
    }
    pub fn total_bytes(&self) -> u64 {
        self.inner.total.load(Ordering::Acquire)
    }
    pub fn phase(&self) -> PackageImportPhase {
        match self.inner.phase.load(Ordering::Acquire) {
            1 => PackageImportPhase::Hashing,
            2 => PackageImportPhase::Prepared,
            3 => PackageImportPhase::Uploading,
            4 => PackageImportPhase::AwaitingPublication,
            5 => PackageImportPhase::Completed,
            6 => PackageImportPhase::Terminal,
            _ => PackageImportPhase::Idle,
        }
    }
    fn check(&self) -> Result<()> {
        ensure!(!self.inner.cancelled.load(Ordering::Acquire),
            "Local import helper interrupted; query the operation or explicitly cancel it before assuming the engine stopped");
        Ok(())
    }
    fn begin(&self, total: u64, prepared: bool) -> Result<ImportGuard<'_>> {
        self.check()?;
        ensure!(
            self.inner
                .active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "Another import helper already owns this progress control"
        );
        self.inner.total.store(total, Ordering::Release);
        self.inner
            .hashed
            .store(if prepared { total } else { 0 }, Ordering::Release);
        self.inner.accepted.store(0, Ordering::Release);
        self.inner.verified.store(0, Ordering::Release);
        self.inner
            .phase
            .store(if prepared { 2 } else { 1 }, Ordering::Release);
        Ok(ImportGuard(self))
    }
    fn observe(&self, status: &PackageImportStatus) {
        self.inner
            .accepted
            .store(status.progress.received_bytes, Ordering::Release);
        self.inner
            .verified
            .store(status.progress.verified_bytes, Ordering::Release);
        self.inner.phase.store(
            if status.state == PackageImportOperationState::Completed {
                5
            } else if status.state.is_terminal() {
                6
            } else if matches!(
                status.state,
                PackageImportOperationState::AwaitingUpload
                    | PackageImportOperationState::Uploading
            ) {
                3
            } else {
                4
            },
            Ordering::Release,
        );
    }
}
struct ImportGuard<'a>(&'a PackageImportControl);
impl Drop for ImportGuard<'_> {
    fn drop(&mut self) {
        self.0.inner.active.store(false, Ordering::Release);
    }
}

/// Retained read-only source handle and the exact bytes identity admitted by Start.
/// Paths never cross the engine API. Opening never modifies the selected file.
pub struct PreparedPackageImport {
    file: File,
    package: PackageUploadDeclaration,
}
impl PreparedPackageImport {
    pub fn declaration(&self) -> &PackageUploadDeclaration {
        &self.package
    }

    pub fn open(path: &Path, control: &PackageImportControl) -> Result<Self> {
        let mut file = open_package_source(path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file(),
            "Package import source must be a regular file"
        );
        let byte_len = metadata.len();
        ensure!(
            byte_len > PACKAGE_IMPORT_HEADER_BYTES as u64 && byte_len <= MAX_PACKAGE_IMPORT_BYTES,
            "Package import source length exceeds supported bounds"
        );
        let _guard = control.begin(byte_len, false)?;
        let deadline = Instant::now()
            .checked_add(LOCAL_IMPORT_BUDGET)
            .context("Import hashing deadline overflow")?;
        let mut header = [0u8; PACKAGE_IMPORT_HEADER_BYTES];
        file.read_exact(&mut header)?;
        let format_version = package_format_version(&header)?;
        let mut digest = Sha256::new();
        digest.update(header);
        let mut hashed = PACKAGE_IMPORT_HEADER_BYTES as u64;
        control.inner.hashed.store(hashed, Ordering::Release);
        let mut buffer = vec![0u8; MAX_ARTIFACT_CHUNK_BYTES];
        while hashed < byte_len {
            control.check()?;
            ensure!(
                Instant::now() < deadline,
                "Import source hashing deadline expired"
            );
            let count = (byte_len - hashed).min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..count])?;
            digest.update(&buffer[..count]);
            hashed = hashed
                .checked_add(count as u64)
                .context("Import hash counter overflow")?;
            control.inner.hashed.store(hashed, Ordering::Release);
        }
        let mut trailing = [0u8; 1];
        ensure!(
            file.read(&mut trailing)? == 0 && file.metadata()?.len() == byte_len,
            "Package source length changed during hashing"
        );
        control.check()?;
        ensure!(
            Instant::now() < deadline,
            "Import source hashing deadline expired"
        );
        let package = PackageUploadDeclaration {
            format_version,
            sha256: format!("{:x}", digest.finalize()),
            byte_len,
        };
        package.validate()?;
        file.seek(SeekFrom::Start(0))?;
        control.inner.phase.store(2, Ordering::Release);
        Ok(Self { file, package })
    }
}

#[cfg(unix)]
fn open_package_source(path: &Path) -> Result<File> {
    use rustix::fs::{open, Mode, OFlags};
    Ok(File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .context(
            "Open the actual regular package file; symlink/device/pipe sources are not accepted",
        )?,
    ))
}
#[cfg(not(unix))]
fn open_package_source(_: &Path) -> Result<File> {
    bail!("Validated retained-handle package input is not implemented on this platform")
}

fn transient_import_io(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let io = match cause.downcast_ref::<TransportError>() {
            Some(TransportError::Io(error)) => Some(error),
            Some(_) => None,
            None => cause.downcast_ref::<std::io::Error>(),
        };
        io.is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::WouldBlock
            )
        })
    })
}

fn retryable_upload_connection_contention(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let protocol = match cause.downcast_ref::<TransportError>() {
            Some(TransportError::Protocol(error)) => Some(error),
            Some(_) => None,
            None => cause.downcast_ref::<ProtocolError>(),
        };
        protocol.is_some_and(|error| {
            error.code == ErrorCode::PackageUploadConnectionBusy && error.retryable
        })
    })
}

fn import_pause(deadline: Instant) -> Result<()> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .context("Import wait deadline expired")?;
    std::thread::sleep(remaining.min(Duration::from_millis(100)));
    Ok(())
}

impl SessionClient {
    /// The caller retains and displays Start identity before submission.
    pub fn start_project_import(
        &mut self,
        prepared: &PreparedPackageImport,
        request_id: RequestId,
    ) -> Result<PackageImportStatus> {
        let response = self.import_request_until(request_id.clone(), Command::StartProjectImport {
            package: prepared.package.clone(),
        }, Instant::now() + Duration::from_secs(10))
            .with_context(|| format!("Import Start request {}; retry this exact ID and declaration after an ambiguous response", request_id))?;
        let status = checked_import_status(response, None, Some(&prepared.package))?;
        ensure!(
            status.request_id == request_id,
            "Import Start response changed the caller request identity"
        );
        Ok(status)
    }

    pub fn project_import_status(
        &mut self,
        operation_id: PackageImportOperationId,
    ) -> Result<PackageImportStatus> {
        self.project_import_status_until(operation_id, Instant::now() + Duration::from_secs(10))
    }
    pub fn project_import_status_until(
        &mut self,
        operation_id: PackageImportOperationId,
        deadline: Instant,
    ) -> Result<PackageImportStatus> {
        checked_import_status(
            self.import_request_until(
                new_request_id(),
                Command::ProjectImportStatus {
                    operation_id: operation_id.clone(),
                },
                deadline,
            )?,
            Some(&operation_id),
            None,
        )
    }
    /// Returns actual engine status. A completed import is not represented as cancelled.
    pub fn cancel_project_import(
        &mut self,
        operation_id: PackageImportOperationId,
    ) -> Result<PackageImportStatus> {
        checked_import_status(
            self.import_request_until(
                new_request_id(),
                Command::CancelProjectImport {
                    operation_id: operation_id.clone(),
                },
                Instant::now() + Duration::from_secs(10),
            )?,
            Some(&operation_id),
            None,
        )
    }

    /// Continues only an existing same-epoch operation. Interrupted is terminal;
    /// a deliberate new Start, not a hidden retry, is required after engine restart.
    /// Terminal failures are returned as statuses; transport/local errors retain IDs.
    pub fn continue_project_import(
        &mut self,
        prepared: &mut PreparedPackageImport,
        operation_id: PackageImportOperationId,
        begin_request_id: RequestId,
        seal_request_id: RequestId,
        control: &PackageImportControl,
    ) -> Result<PackageImportStatus> {
        let deadline = Instant::now()
            .checked_add(LOCAL_IMPORT_BUDGET)
            .context("Import deadline overflow")?;
        let _guard = control.begin(prepared.package.byte_len, true)?;
        let mut status = self.project_import_status_until(operation_id.clone(), deadline)?;
        validate_import_status(&status, Some(&operation_id), Some(&prepared.package))?;
        let start_request_id = status.request_id.clone();
        control.observe(&status);
        if status.state.is_terminal() {
            return Ok(status);
        }
        if matches!(
            status.state,
            PackageImportOperationState::AwaitingUpload | PackageImportOperationState::Uploading
        ) {
            let sent_at = Instant::now();
            let response = self.recover_import_control(begin_request_id.clone(), Command::BeginProjectPackageUpload {
                operation_id: operation_id.clone(),
            }, deadline, control).with_context(|| format!(
                "Import Begin request {} for operation {}; recover this exact ID after a lost lease acknowledgement", begin_request_id, operation_id))?;
            let lease = match response {
                ResponseBody::PackageUpload(lease) => lease,
                _ => bail!(
                    "Import Begin returned no package-upload lease; operation {} retained",
                    operation_id
                ),
            };
            validate_import_lease(
                &lease,
                &operation_id,
                &prepared.package,
                &self.package_bulk_endpoint()?,
            )?;
            let upload_deadline = sent_at
                .checked_add(Duration::from_millis(lease.expires_after_ms))
                .context("Import lease deadline overflow")?
                .min(deadline);
            let upload = self.stream_project_import(prepared, &lease, upload_deadline, control);
            if let Err(error) = upload {
                // Only validated, owned leases are abandoned. Do not send cleanup
                // using identities from an invalid or substituted response.
                let abandoned = matches!(self.import_request_until(new_request_id(),
                    Command::AbandonProjectPackageUpload { lease_id: lease.lease_id.clone() }, deadline),
                    Ok(ResponseBody::PackageUploadAbandoned { lease_id }) if lease_id == lease.lease_id);
                return Err(error.context(format!("Import {} upload stopped; lease abandonment confirmed: {}. Operation was not cancelled; use status or explicit cancel",
                    operation_id, abandoned)));
            }
            control.check()?;
            control.inner.phase.store(4, Ordering::Release);
            // Once Seal has been submitted, an acknowledgement can be lost after
            // acceptance. Never abandon or restart that upload on an ambiguous Seal.
            let response = self.recover_import_control(seal_request_id.clone(), Command::SealProjectImport {
                operation_id: operation_id.clone(), upload_generation: lease.generation,
            }, deadline, control).with_context(|| format!(
                "Import Seal request {}, generation {}, operation {}; query status or replay that exact Seal, not a new import",
                seal_request_id, lease.generation, operation_id))?;
            status = checked_import_status(response, Some(&operation_id), Some(&prepared.package))?;
            ensure!(
                !matches!(
                    status.state,
                    PackageImportOperationState::AwaitingUpload
                        | PackageImportOperationState::Uploading
                ),
                "Import Seal response did not acknowledge sealing or a terminal outcome"
            );
        }
        loop {
            validate_import_status(&status, Some(&operation_id), Some(&prepared.package))?;
            ensure!(
                status.request_id == start_request_id,
                "Import status changed its original Start identity"
            );
            control.observe(&status);
            if status.state.is_terminal() {
                return Ok(status);
            }
            control.check()?;
            import_pause(deadline)?;
            status = self
                .project_import_status_until(operation_id.clone(), deadline)
                .with_context(|| {
                    format!(
                        "Import {} may still be running; status failure is not cancellation",
                        operation_id
                    )
                })?;
        }
    }

    fn open_package_upload_after_contention(
        &self,
        lease: &PackageUploadLease,
        deadline: Instant,
        chunk: &PackageUploadChunkReceipt,
        control: &PackageImportControl,
    ) -> Result<PackageUploadClient> {
        // Engineering retry defaults, not a guarantee that a live peer will retire.
        // Keep every retry under the original lease/operation absolute deadline.
        const MAX_CONTENTION_RETRIES: usize = 20;
        const CONTENTION_WINDOW: Duration = Duration::from_secs(2);
        const CONTENTION_PAUSE: Duration = Duration::from_millis(50);
        let mut retry_deadline = deadline;
        let mut contention_started = false;
        for attempt in 0..=MAX_CONTENTION_RETRIES {
            control.check()?;
            match self.open_package_upload(lease, deadline, retry_deadline, chunk) {
                Ok(transport) => return Ok(transport),
                Err(error) if retryable_upload_connection_contention(&error) => {
                    if attempt == MAX_CONTENTION_RETRIES {
                        return Err(error.context("Upload connection contention retry budget exhausted; no new lease or import was created"));
                    }
                    if !contention_started {
                        retry_deadline = retry_deadline.min(
                            Instant::now()
                                .checked_add(CONTENTION_WINDOW)
                                .context("Upload contention deadline overflow")?,
                        );
                        contention_started = true;
                    }
                    let remaining = retry_deadline
                        .checked_duration_since(Instant::now())
                        .context("Upload connection contention reached the original deadline")?;
                    std::thread::sleep(remaining.min(CONTENTION_PAUSE));
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("finite contention attempts return on their final iteration")
    }

    fn recover_import_control(
        &self,
        request_id: RequestId,
        command: Command,
        deadline: Instant,
        control: &PackageImportControl,
    ) -> Result<ResponseBody> {
        let mut retries = 0;
        loop {
            control.check()?;
            match self.import_request_until(request_id.clone(), command.clone(), deadline) {
                Ok(response) => return Ok(response),
                Err(error) if transient_import_io(&error) && retries < 2 => {
                    retries += 1;
                    import_pause(deadline)?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn stream_project_import(
        &self,
        prepared: &mut PreparedPackageImport,
        lease: &PackageUploadLease,
        deadline: Instant,
        control: &PackageImportControl,
    ) -> Result<()> {
        let mut offset = lease.next_offset;
        prepared.file.seek(SeekFrom::Start(offset))?;
        control.inner.phase.store(3, Ordering::Release);
        control.inner.accepted.store(offset, Ordering::Release);
        let mut buffer = vec![0u8; MAX_ARTIFACT_CHUNK_BYTES];
        let mut transport = None;
        let mut reconnects = 0;
        while offset < prepared.package.byte_len {
            control.check()?;
            ensure!(
                Instant::now() < deadline,
                "Import upload lease deadline expired"
            );
            let count = (prepared.package.byte_len - offset).min(buffer.len() as u64) as usize;
            prepared.file.read_exact(&mut buffer[..count])?;
            let expected = PackageUploadChunkReceipt {
                offset,
                byte_len: count as u32,
                sha256: format!("{:x}", Sha256::digest(&buffer[..count])),
            };
            let mut retries = 0;
            loop {
                if transport.is_none() {
                    transport = Some(self.open_package_upload_after_contention(
                        lease, deadline, &expected, control,
                    )?);
                }
                match transport
                    .as_mut()
                    .expect("upload transport established")
                    .upload(offset, &buffer[..count])
                {
                    Ok(receipt) => {
                        ensure!(
                            receipt == expected,
                            "Import upload acknowledgement changed offset, length or digest"
                        );
                        break;
                    }
                    Err(error) => {
                        let error: anyhow::Error = error.into();
                        if !transient_import_io(&error) || retries >= 1 || reconnects >= 8 {
                            return Err(error.context(
                                "Import chunk failed; no unacknowledged progress was claimed",
                            ));
                        }
                        retries += 1;
                        reconnects += 1;
                        transport = None;
                        control.check()?;
                    }
                }
            }
            offset = offset
                .checked_add(count as u64)
                .context("Import upload counter overflow")?;
            control.inner.accepted.store(offset, Ordering::Release);
        }
        ensure!(
            offset == prepared.package.byte_len
                && prepared.file.metadata()?.len() == prepared.package.byte_len,
            "Import source length changed after its declaration"
        );
        control.check()?;
        // The engine verifies the entire declared hash again before publication.
        Ok(())
    }
}

fn checked_import_status(
    response: ResponseBody,
    operation: Option<&PackageImportOperationId>,
    package: Option<&PackageUploadDeclaration>,
) -> Result<PackageImportStatus> {
    match response {
        ResponseBody::PackageImport(status) => {
            validate_import_status(&status, operation, package)?;
            Ok(status)
        }
        _ => bail!("Expected typed project-import status"),
    }
}
fn validate_import_status(
    status: &PackageImportStatus,
    operation: Option<&PackageImportOperationId>,
    package: Option<&PackageUploadDeclaration>,
) -> Result<()> {
    status.validate()?;
    ensure!(
        operation.is_none_or(|id| id == &status.operation_id)
            && package.is_none_or(|package| package == &status.package),
        "Import status changed its operation or exact source declaration"
    );
    Ok(())
}
pub(crate) fn validate_import_lease(
    lease: &PackageUploadLease,
    operation: &PackageImportOperationId,
    package: &PackageUploadDeclaration,
    endpoint: &Path,
) -> Result<()> {
    lease.validate()?;
    ensure!(
        &lease.operation_id == operation && &lease.package == package,
        "Import upload lease substituted operation, source digest, format or length"
    );
    ensure!(
        lease.bulk_endpoint == endpoint,
        "Import bulk endpoint redirect rejected before authentication"
    );
    Ok(())
}
pub(crate) fn validate_import_handshake(
    expected: &PackageUploadLease,
    actual: &PackageUploadLease,
    chunk: &PackageUploadChunkReceipt,
) -> Result<()> {
    actual.validate()?;
    ensure!(actual.lease_id == expected.lease_id && actual.engine_epoch == expected.engine_epoch
        && actual.operation_id == expected.operation_id && actual.generation == expected.generation
        && actual.package == expected.package && actual.bulk_endpoint == expected.bulk_endpoint
        && actual.expires_after_ms <= expected.expires_after_ms,
        "Import handshake changed lease authority, generation, bytes identity, endpoint or lifetime");
    let end = chunk
        .offset
        .checked_add(u64::from(chunk.byte_len))
        .context("Import chunk endpoint overflow")?;
    ensure!(
        actual.next_offset == chunk.offset
            || (actual.next_offset == end && actual.replay.as_ref() == Some(chunk)),
        "Import reconnect cursor is neither next nor the exact lost acknowledgement replay"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::fs;

    fn declaration() -> PackageUploadDeclaration {
        PackageUploadDeclaration {
            format_version: 1,
            sha256: "a".repeat(64),
            byte_len: 1024,
        }
    }
    fn lease() -> PackageUploadLease {
        PackageUploadLease {
            lease_id: PackageUploadId::new("lease").unwrap(),
            engine_epoch: "epoch".into(),
            operation_id: PackageImportOperationId::new("import").unwrap(),
            generation: 1,
            package: declaration(),
            next_offset: 0,
            replay: None,
            expires_after_ms: 1000,
            bulk_endpoint: "/private/bulk.sock".into(),
        }
    }
    fn status(state: PackageImportOperationState) -> PackageImportStatus {
        PackageImportStatus {
            operation_id: PackageImportOperationId::new("import").unwrap(),
            request_id: RequestId::new("start").unwrap(),
            package: declaration(),
            state,
            progress: PackageImportProgress {
                received_bytes: 0,
                verified_bytes: 0,
                total_bytes: 1024,
                completed_objects: 0,
                total_objects: None,
            },
            capture: None,
            receipt: None,
            error: if state.is_terminal() && state != PackageImportOperationState::Completed {
                Some(ProtocolError::invalid("terminal fixture"))
            } else {
                None
            },
        }
    }
    /// A fixed valid header PROBE fixture, not a valid archive/closure fixture.
    fn probe_bytes(version: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; PACKAGE_IMPORT_HEADER_BYTES + 8];
        bytes[..8].copy_from_slice(b"PULSPKG\0");
        bytes[8..10].copy_from_slice(&version.to_le_bytes());
        bytes[12..20].copy_from_slice(&8u64.to_le_bytes());
        bytes[PACKAGE_IMPORT_HEADER_BYTES..].copy_from_slice(b"manifest");
        bytes
    }
    #[test]
    fn import_cli_preserves_explicit_recovery_ids_and_distinct_status_cancel_resume() {
        let parsed = crate::cli::Cli::try_parse_from([
            "pulsar",
            "package-import",
            "/tmp/source",
            "--request-id",
            "start",
            "--begin-request-id",
            "begin",
            "--seal-request-id",
            "seal",
        ])
        .unwrap();
        assert!(
            matches!(parsed.command, Some(crate::cli::Commands::PackageImport {
            request_id: Some(start), begin_request_id: Some(begin), seal_request_id: Some(seal), ..
        }) if start=="start" && begin=="begin" && seal=="seal")
        );
        for args in [
            vec!["pulsar", "package-import-status", "operation"],
            vec!["pulsar", "package-import-cancel", "operation"],
            vec![
                "pulsar",
                "package-import-resume",
                "operation",
                "/tmp/source",
                "--begin-request-id",
                "begin",
                "--seal-request-id",
                "seal",
            ],
        ] {
            assert!(crate::cli::Cli::try_parse_from(args).is_ok());
        }
    }
    #[cfg(unix)]
    #[test]
    fn prepared_input_hashes_exact_bytes_and_preserves_actual_supported_version() {
        let root = tempfile::tempdir().unwrap();
        for version in [1, 2] {
            let bytes = probe_bytes(version);
            let path = root.path().join(format!("v{version}.pkg"));
            fs::write(&path, &bytes).unwrap();
            let progress = PackageImportControl::default();
            let mut prepared = PreparedPackageImport::open(&path, &progress).unwrap();
            assert_eq!(prepared.package.format_version, version);
            assert_eq!(prepared.package.byte_len, bytes.len() as u64);
            assert_eq!(
                prepared.package.sha256,
                format!("{:x}", Sha256::digest(&bytes))
            );
            assert_eq!(progress.hashed_bytes(), bytes.len() as u64);
            assert_eq!(progress.phase(), PackageImportPhase::Prepared);
            let mut retained = Vec::new();
            prepared.file.read_to_end(&mut retained).unwrap();
            assert_eq!(retained, bytes);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[cfg(unix)]
    #[test]
    fn source_handle_is_not_reopened_when_its_path_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.pkg");
        let bytes = probe_bytes(1);
        fs::write(&path, &bytes).unwrap();
        let mut prepared =
            PreparedPackageImport::open(&path, &PackageImportControl::default()).unwrap();
        fs::rename(&path, root.path().join("original.pkg")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        let mut retained = Vec::new();
        prepared.file.read_to_end(&mut retained).unwrap();
        assert_eq!(retained, bytes);
        assert_eq!(fs::read(path).unwrap(), b"replacement");
    }
    #[cfg(unix)]
    #[test]
    fn malformed_symlink_nonregular_and_overbudget_sources_fail_before_start() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.pkg");
        fs::write(&path, vec![0u8; 60]).unwrap();
        assert!(PreparedPackageImport::open(&path, &PackageImportControl::default()).is_err());
        fs::write(&path, probe_bytes(9)).unwrap();
        assert!(PreparedPackageImport::open(&path, &PackageImportControl::default()).is_err());
        let link = root.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(PreparedPackageImport::open(&link, &PackageImportControl::default()).is_err());
        assert!(
            PreparedPackageImport::open(root.path(), &PackageImportControl::default()).is_err()
        );
        let file = File::options().write(true).open(&path).unwrap();
        file.set_len(MAX_PACKAGE_IMPORT_BYTES + 1).unwrap();
        assert!(PreparedPackageImport::open(&path, &PackageImportControl::default()).is_err());
    }
    #[test]
    fn invalid_status_never_exposes_an_active_project_or_changes_source_identity() {
        let awaiting = status(PackageImportOperationState::AwaitingUpload);
        validate_import_status(
            &awaiting,
            Some(&awaiting.operation_id),
            Some(&awaiting.package),
        )
        .unwrap();
        assert!(validate_import_status(
            &awaiting,
            Some(&PackageImportOperationId::new("other").unwrap()),
            None
        )
        .is_err());
        let mut changed = declaration();
        changed.sha256 = "b".repeat(64);
        assert!(validate_import_status(&awaiting, None, Some(&changed)).is_err());
        let mut completed = awaiting;
        completed.state = PackageImportOperationState::Completed;
        assert!(validate_import_status(&completed, None, None).is_err());
        assert!(completed.receipt.is_none());
        for state in [
            PackageImportOperationState::Failed,
            PackageImportOperationState::Cancelled,
            PackageImportOperationState::Interrupted,
        ] {
            let failed = status(state);
            validate_import_status(&failed, None, None).unwrap();
            assert!(failed.receipt.is_none());
        }
    }
    #[test]
    fn upload_leases_and_reconnects_pin_operation_generation_package_and_last_payload() {
        let expected = lease();
        validate_import_lease(
            &expected,
            &expected.operation_id,
            &expected.package,
            Path::new("/private/bulk.sock"),
        )
        .unwrap();
        let chunk = PackageUploadChunkReceipt {
            offset: 0,
            byte_len: 128,
            sha256: "c".repeat(64),
        };
        validate_import_handshake(&expected, &expected, &chunk).unwrap();
        let mut accepted = expected.clone();
        accepted.next_offset = 128;
        accepted.replay = Some(chunk.clone());
        validate_import_handshake(&expected, &accepted, &chunk).unwrap();
        for index in 0..5 {
            let mut changed = accepted.clone();
            match index {
                0 => changed.generation += 1,
                1 => changed.engine_epoch = "new-epoch".into(),
                2 => changed.package.sha256 = "d".repeat(64),
                3 => changed.bulk_endpoint = "/attacker/bulk.sock".into(),
                _ => changed.expires_after_ms += 1,
            }
            assert!(validate_import_handshake(&expected, &changed, &chunk).is_err());
        }
        let mut changed = chunk.clone();
        changed.sha256 = "e".repeat(64);
        assert!(validate_import_handshake(&expected, &accepted, &changed).is_err());
        assert!(validate_import_lease(
            &expected,
            &expected.operation_id,
            &expected.package,
            Path::new("/other/bulk.sock")
        )
        .is_err());
    }
    #[test]
    fn import_progress_reuse_resets_counters_and_local_cancel_is_not_engine_status() {
        let progress = PackageImportControl::default();
        {
            let _guard = progress.begin(1024, true).unwrap();
            progress.inner.accepted.store(512, Ordering::Release);
            progress.inner.verified.store(256, Ordering::Release);
            assert!(progress.begin(1024, false).is_err());
        }
        let _guard = progress.begin(4096, false).unwrap();
        assert_eq!(progress.hashed_bytes(), 0);
        assert_eq!(progress.accepted_bytes(), 0);
        assert_eq!(progress.verified_bytes(), 0);
        assert_eq!(progress.total_bytes(), 4096);
        progress.cancel();
        assert!(progress.check().is_err());
    }
    #[test]
    fn import_retries_only_typed_transient_io_not_semantic_or_identity_errors() {
        assert!(transient_import_io(&anyhow::Error::from(
            TransportError::Io(std::io::ErrorKind::UnexpectedEof.into())
        )));
        assert!(!transient_import_io(&anyhow::Error::from(
            TransportError::Protocol(ProtocolError::invalid("denied"))
        )));
        assert!(!transient_import_io(&anyhow::Error::from(
            TransportError::ResponseMismatch
        )));
        assert!(!transient_import_io(&anyhow::Error::from(
            TransportError::InvalidLength(u32::MAX)
        )));
        assert!(import_pause(Instant::now() - Duration::from_millis(1)).is_err());
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    use super::*;
    use std::{
        fs, io,
        os::unix::net::{UnixListener, UnixStream},
        thread,
    };

    fn accept(listener: &UnixListener) -> UnixStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    return stream;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "fixture accept deadline");
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("{error}"),
            }
        }
    }
    fn receive(listener: &UnixListener) -> (UnixStream, Request) {
        let mut stream = accept(listener);
        let request: Request = read_message(&mut stream).unwrap();
        assert!(request.project.is_none() && request.expected_revision.is_none());
        assert_eq!(request.session.as_ref().unwrap().as_str(), "owner");
        assert_eq!(request.auth_token.as_deref(), Some("fixture-secret"));
        (stream, request)
    }
    fn respond(
        stream: &mut UnixStream,
        request: &Request,
        result: Result<ResponseBody, ProtocolError>,
    ) {
        write_message(
            stream,
            &Response {
                version: PROTOCOL_VERSION,
                request_id: request.request_id.clone(),
                result,
            },
        )
        .unwrap();
    }
    fn status(
        package: &PackageUploadDeclaration,
        state: PackageImportOperationState,
    ) -> PackageImportStatus {
        let received = if state == PackageImportOperationState::AwaitingUpload {
            0
        } else {
            package.byte_len
        };
        PackageImportStatus {
            operation_id: PackageImportOperationId::new("import").unwrap(),
            request_id: RequestId::new("start").unwrap(),
            package: package.clone(),
            state,
            progress: PackageImportProgress {
                received_bytes: received,
                verified_bytes: 0,
                total_bytes: package.byte_len,
                completed_objects: 0,
                total_objects: None,
            },
            capture: None,
            receipt: None,
            error: (state == PackageImportOperationState::Failed).then(|| {
                ProtocolError::invalid("fixture deliberately stops before archive qualification")
            }),
        }
    }
    fn prepared(root: &Path) -> PreparedPackageImport {
        let mut bytes = vec![0u8; PACKAGE_IMPORT_HEADER_BYTES + 8];
        bytes[..8].copy_from_slice(b"PULSPKG\0");
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        bytes[12..20].copy_from_slice(&8u64.to_le_bytes());
        bytes[PACKAGE_IMPORT_HEADER_BYTES..].copy_from_slice(b"manifest");
        let path = root.join("header-probe.pkg");
        fs::write(&path, bytes).unwrap();
        PreparedPackageImport::open(&path, &PackageImportControl::default()).unwrap()
    }
    fn client(endpoint: &Path) -> SessionClient {
        SessionClient::from_credentials(
            endpoint,
            SessionId::new("owner").unwrap(),
            "fixture-secret".into(),
        )
        .unwrap()
    }

    #[test]
    fn lost_start_ack_is_recoverable_with_same_explicit_identity_and_no_project_envelope() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("engine.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let prepared = prepared(root.path());
        let package = prepared.package.clone();
        let server = thread::spawn(move || {
            for retry in 0..2 {
                let (mut stream, request) = receive(&listener);
                assert_eq!(request.request_id.as_str(), "start");
                assert!(
                    matches!(&request.command, Command::StartProjectImport { package: actual } if actual == &package)
                );
                if retry == 1 {
                    respond(
                        &mut stream,
                        &request,
                        Ok(ResponseBody::PackageImport(status(
                            &package,
                            PackageImportOperationState::AwaitingUpload,
                        ))),
                    );
                }
            }
        });
        let mut client = client(&endpoint);
        let error = client
            .start_project_import(&prepared, RequestId::new("start").unwrap())
            .unwrap_err();
        assert!(error.to_string().contains("start"));
        let status = client
            .start_project_import(&prepared, RequestId::new("start").unwrap())
            .unwrap();
        assert_eq!(status.operation_id.as_str(), "import");
        assert!(status.receipt.is_none());
        server.join().unwrap();
    }

    #[test]
    fn lost_begin_chunk_and_seal_acks_keep_exact_ids_and_do_not_publish_or_cancel() {
        lost_upload_ack_flow(false);
    }

    #[test]
    fn lost_chunk_ack_retries_only_typed_connection_contention_before_exact_replay() {
        lost_upload_ack_flow(true);
    }

    fn lost_upload_ack_flow(contention: bool) {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("engine.sock");
        let bulk_endpoint = root.path().join("bulk.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let bulk_listener = UnixListener::bind(&bulk_endpoint).unwrap();
        let mut prepared = prepared(root.path());
        let package = prepared.package.clone();
        let lease = PackageUploadLease {
            lease_id: PackageUploadId::new("lease").unwrap(),
            engine_epoch: "epoch".into(),
            operation_id: PackageImportOperationId::new("import").unwrap(),
            generation: 1,
            package: package.clone(),
            next_offset: 0,
            replay: None,
            expires_after_ms: 30_000,
            bulk_endpoint,
        };
        let control_package = package.clone();
        let control_lease = lease.clone();
        let server = thread::spawn(move || {
            let (mut stream, request) = receive(&listener);
            assert!(matches!(
                request.command,
                Command::ProjectImportStatus { .. }
            ));
            respond(
                &mut stream,
                &request,
                Ok(ResponseBody::PackageImport(status(
                    &control_package,
                    PackageImportOperationState::AwaitingUpload,
                ))),
            );
            drop(stream);
            for retry in 0..2 {
                let (mut stream, request) = receive(&listener);
                assert_eq!(request.request_id.as_str(), "begin");
                assert!(matches!(
                    request.command,
                    Command::BeginProjectPackageUpload { .. }
                ));
                if retry == 1 {
                    respond(
                        &mut stream,
                        &request,
                        Ok(ResponseBody::PackageUpload(control_lease.clone())),
                    );
                }
            }
            for retry in 0..2 {
                let (mut stream, request) = receive(&listener);
                assert_eq!(request.request_id.as_str(), "seal");
                assert!(matches!(
                    request.command,
                    Command::SealProjectImport {
                        upload_generation: 1,
                        ..
                    }
                ));
                if retry == 1 {
                    respond(
                        &mut stream,
                        &request,
                        Ok(ResponseBody::PackageImport(status(
                            &control_package,
                            PackageImportOperationState::Sealing,
                        ))),
                    );
                }
            }
            let (mut stream, request) = receive(&listener);
            assert!(matches!(
                request.command,
                Command::ProjectImportStatus { .. }
            ));
            respond(
                &mut stream,
                &request,
                Ok(ResponseBody::PackageImport(status(
                    &control_package,
                    PackageImportOperationState::Failed,
                ))),
            );
        });
        let uploader = thread::spawn(move || {
            let mut accepted: Option<PackageUploadChunkReceipt> = None;
            for connection in 0..if contention { 4 } else { 2 } {
                let mut stream = accept(&bulk_listener);
                let handshake: PackageUploadHandshake = read_bulk_header(&mut stream).unwrap();
                assert_eq!(handshake.generation, 1);
                assert_eq!(handshake.auth_token, "fixture-secret");
                assert_eq!(handshake.package_sha256, package.sha256);
                assert_eq!(handshake.lease_id, lease.lease_id);
                assert_eq!(handshake.engine_epoch, lease.engine_epoch);
                assert_eq!(handshake.operation_id, lease.operation_id);
                if contention && matches!(connection, 1 | 2) {
                    let mut error = ProtocolError::new(
                        ErrorCode::PackageUploadConnectionBusy,
                        "fixture retains the prior connection guard",
                    );
                    error.retryable = true;
                    write_bulk_header(&mut stream, &PackageUploadReply::Error(error)).unwrap();
                    continue;
                }
                let retry = usize::from(connection != 0);
                let mut actual = lease.clone();
                if let Some(receipt) = &accepted {
                    actual.next_offset = receipt.end().unwrap();
                    actual.replay = Some(receipt.clone());
                }
                write_bulk_header(&mut stream, &PackageUploadReply::Ready { lease: actual })
                    .unwrap();
                let chunk: PackageUploadRequest = read_bulk_header(&mut stream).unwrap();
                let mut bytes = vec![0u8; chunk.byte_len as usize];
                // Same four-byte artifact framing consumed explicitly by the fixture.
                let mut prefix = [0u8; 4];
                stream.read_exact(&mut prefix).unwrap();
                assert_eq!(u32::from_be_bytes(prefix), chunk.byte_len);
                stream.read_exact(&mut bytes).unwrap();
                assert_eq!(format!("{:x}", Sha256::digest(&bytes)), chunk.sha256);
                if let Some(previous) = &accepted {
                    assert_eq!(previous, &chunk);
                }
                if retry == 1 {
                    write_bulk_header(
                        &mut stream,
                        &PackageUploadReply::Uploaded {
                            offset: chunk.offset,
                            byte_len: chunk.byte_len,
                            sha256: chunk.sha256.clone(),
                            next_offset: chunk.end().unwrap(),
                        },
                    )
                    .unwrap();
                }
                accepted = Some(chunk);
            }
        });
        let mut client = client(&endpoint);
        let progress = PackageImportControl::default();
        let terminal = client
            .continue_project_import(
                &mut prepared,
                PackageImportOperationId::new("import").unwrap(),
                RequestId::new("begin").unwrap(),
                RequestId::new("seal").unwrap(),
                &progress,
            )
            .unwrap();
        assert_eq!(terminal.state, PackageImportOperationState::Failed);
        assert!(terminal.receipt.is_none());
        assert_eq!(progress.accepted_bytes(), prepared.package.byte_len);
        assert_eq!(progress.verified_bytes(), 0);
        assert_eq!(progress.phase(), PackageImportPhase::Terminal);
        server.join().unwrap();
        uploader.join().unwrap();
    }

    #[test]
    fn generic_resource_auth_and_nonretryable_contention_errors_are_fatal() {
        for (code, retryable) in [
            (ErrorCode::ResourceExhausted, true),
            (ErrorCode::Forbidden, true),
            (ErrorCode::PackageUploadConnectionBusy, false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let endpoint = root.path().join("engine.sock");
            let bulk_endpoint = root.path().join("bulk.sock");
            let listener = UnixListener::bind(&bulk_endpoint).unwrap();
            let package = prepared(root.path()).package;
            let lease = PackageUploadLease {
                lease_id: PackageUploadId::new("lease").unwrap(),
                engine_epoch: "epoch".into(),
                operation_id: PackageImportOperationId::new("import").unwrap(),
                generation: 1,
                package,
                next_offset: 0,
                replay: None,
                expires_after_ms: 30_000,
                bulk_endpoint,
            };
            let server = thread::spawn(move || {
                let mut stream = accept(&listener);
                let _: PackageUploadHandshake = read_bulk_header(&mut stream).unwrap();
                let mut error = ProtocolError::new(code, "fatal fixture response");
                error.retryable = retryable;
                write_bulk_header(&mut stream, &PackageUploadReply::Error(error)).unwrap();
                drop(stream);
                // The accepted error is fatal; the helper must not open another lane.
                listener.set_nonblocking(true).unwrap();
                thread::sleep(Duration::from_millis(150));
                assert!(
                    matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
                );
            });
            let client = client(&endpoint);
            let chunk = PackageUploadChunkReceipt {
                offset: 0,
                byte_len: 1,
                sha256: "a".repeat(64),
            };
            let result = client.open_package_upload_after_contention(
                &lease,
                Instant::now() + Duration::from_secs(2),
                &chunk,
                &PackageImportControl::default(),
            );
            assert!(result.is_err());
            assert!(!retryable_upload_connection_contention(
                &result.err().unwrap()
            ));
            server.join().unwrap();
        }
    }

    #[test]
    fn semantic_begin_failure_is_not_retried_or_translated_to_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("engine.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, request) = receive(&listener);
            assert_eq!(request.request_id.as_str(), "begin");
            respond(
                &mut stream,
                &request,
                Err(ProtocolError::new(ErrorCode::Forbidden, "denied")),
            );
            drop(stream);
            listener.set_nonblocking(true).unwrap();
            thread::sleep(Duration::from_millis(150));
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
            );
        });
        let client = client(&endpoint);
        let error = client
            .recover_import_control(
                RequestId::new("begin").unwrap(),
                Command::BeginProjectPackageUpload {
                    operation_id: PackageImportOperationId::new("import").unwrap(),
                },
                Instant::now() + Duration::from_secs(2),
                &PackageImportControl::default(),
            )
            .unwrap_err();
        assert!(!transient_import_io(&error));
        server.join().unwrap();
    }
}
