//! Authenticated portable export client. No engine persistence or import authority.
use crate::session::{new_request_id, SessionClient};
use anyhow::{bail, ensure, Context, Result};
use pulsar_protocol::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}},
    time::{Duration, Instant},
};

pub const PACKAGE_PRIVACY_WARNING: &str = "Portable packages are UNENCRYPTED and include media, prompts, labels, edit history and private provenance. Anyone with the file can read them. Model/runtime executables are not bundled.";

/// Records explicit UI/CLI acknowledgement, not a capability or proof of human presence.
#[derive(Clone, Debug)]
pub struct PackagePrivacyConsent { _acknowledged: () }
impl PackagePrivacyConsent {
    pub fn acknowledge_unencrypted_private_data() -> Self { Self { _acknowledged: () } }
}

#[derive(Default)]
struct DownloadProgress {
    cancelled: AtomicBool, active: AtomicBool, received: AtomicU64, total: AtomicU64,
    verification_bytes: AtomicU64,
}
#[derive(Clone, Default)]
pub struct PackageDownloadControl { inner: Arc<DownloadProgress> }
impl PackageDownloadControl {
    /// Cancels local transfer only. Explicit server cancellation is a separate operation.
    pub fn cancel(&self) { self.inner.cancelled.store(true, Ordering::Release); }
    pub fn received_bytes(&self) -> u64 { self.inner.received.load(Ordering::Acquire) }
    pub fn total_bytes(&self) -> u64 { self.inner.total.load(Ordering::Acquire) }
    pub fn verification_bytes(&self) -> u64 { self.inner.verification_bytes.load(Ordering::Acquire) }
    fn check(&self) -> Result<()> {
        ensure!(!self.inner.cancelled.load(Ordering::Acquire),
            "Local package download cancelled; this does not cancel or release the engine export");
        Ok(())
    }
    fn begin(&self, total: u64) -> Result<DownloadGuard<'_>> {
        self.check()?;
        ensure!(self.inner.active.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            "A package download already owns this progress control");
        self.inner.total.store(total, Ordering::Release);
        self.inner.received.store(0, Ordering::Release);
        self.inner.verification_bytes.store(0, Ordering::Release);
        Ok(DownloadGuard(self))
    }
}
struct DownloadGuard<'a>(&'a PackageDownloadControl);
impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) { self.0.inner.active.store(false, Ordering::Release); }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublishedProjectPackage {
    pub operation_id: PackageOperationId,
    pub destination: PathBuf,
    pub artifact: PackageArtifactDescriptor,
    /// Only length and full file SHA-256 were independently checked by this client.
    pub verification: &'static str,
    pub file_and_directory_synced: bool,
    pub lease_cleanup_confirmed: bool,
    /// Action history only; another authorized client may release the engine copy.
    pub engine_release_requested: bool,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackagePublicationOutcome { NotPublished, OutcomeUnknown, PublishedDurabilityUnconfirmed }
#[derive(Debug, Serialize)]
pub struct PackagePublicationError {
    pub destination: PathBuf,
    pub outcome: PackagePublicationOutcome,
    pub recovery_staging_path: Option<PathBuf>,
    pub detail: String,
}
impl std::fmt::Display for PackagePublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Package publication {:?} at {}: {}", self.outcome, self.destination.display(), self.detail)?;
        if let Some(path) = &self.recovery_staging_path {
            write!(f, "; verified staging retained at {} (locate the original directory if it was renamed)", path.display())?;
        }
        f.write_str("; engine export remains available unless separately released")
    }
}
impl std::error::Error for PackagePublicationError {}

impl SessionClient {
    /// No owner-proof access or implicit grant upgrade occurs here.
    pub fn start_project_export(&mut self, project: ProjectId, revision: RevisionId,
        consent: &PackagePrivacyConsent) -> Result<PackageExportStatus> {
        self.start_project_export_with_request_id(project, revision, consent, new_request_id())
    }

    /// Callers retain this identity before submission and may explicitly replay it
    /// with the exact same project/revision after an ambiguous response.
    pub fn start_project_export_with_request_id(&mut self, project: ProjectId, revision: RevisionId,
        _consent: &PackagePrivacyConsent, request_id: RequestId) -> Result<PackageExportStatus> {
        let response = self.package_request(request_id.clone(), Command::StartProjectExport, project.clone(), Some(revision))
            .with_context(|| format!("Package Start request {}; after a transport failure retry this exact ID, project and revision, not a new Start", request_id))?;
        let status = checked_status(response, &project, None)?;
        ensure!(status.request_id == request_id && status.requested_revision == revision,
            "Package start response does not bind the submitted request and revision");
        Ok(status)
    }

    pub fn project_package_status(&mut self, project: ProjectId, operation_id: PackageOperationId) -> Result<PackageExportStatus> {
        self.project_package_status_until(project, operation_id, Instant::now() + Duration::from_secs(10))
    }

    pub fn project_package_status_until(&mut self, project: ProjectId, operation_id: PackageOperationId,
        deadline: Instant) -> Result<PackageExportStatus> {
        let response = self.package_request_until(new_request_id(), Command::ProjectPackageStatus { operation_id: operation_id.clone() },
            project.clone(), None, deadline)?;
        checked_status(response, &project, Some(&operation_id))
    }

    pub fn cancel_project_export(&mut self, project: ProjectId, operation_id: PackageOperationId) -> Result<PackageExportStatus> {
        let response = self.package_request(new_request_id(), Command::CancelProjectExport { operation_id: operation_id.clone() }, project.clone(), None)?;
        checked_status(response, &project, Some(&operation_id))
    }

    /// Explicit intentional disposal, not automatic cleanup after a download.
    pub fn release_project_export(&mut self, project: ProjectId, operation_id: PackageOperationId) -> Result<PackageExportStatus> {
        let response = self.package_request(new_request_id(), Command::ReleaseProjectExport { operation_id: operation_id.clone() }, project.clone(), None)?;
        checked_status(response, &project, Some(&operation_id))
    }

    /// Metadata only: this does not request or hydrate motion bulk data.
    pub fn project_package_revision(&mut self, project: ProjectId) -> Result<RevisionId> {
        match self.package_request(new_request_id(), Command::GetSnapshot, project.clone(), None)? {
            ResponseBody::Project(snapshot) => {
                snapshot.validate()?;
                ensure!(snapshot.project_id == project, "Package revision query returned a different project");
                Ok(snapshot.revision)
            }
            _ => bail!("Expected project metadata for package revision"),
        }
    }

    /// Streams one Ready export into a new destination. No partial data is published.
    /// A failed/cancelled download abandons its lease, never its export operation.
    pub fn download_project_package(&mut self, project: ProjectId, operation_id: PackageOperationId,
        destination: &Path, consent: &PackagePrivacyConsent, control: &PackageDownloadControl) -> Result<PublishedProjectPackage> {
        self.download_project_package_with_request_id(project, operation_id, destination, consent, control, new_request_id())
    }

    /// Preserve this Begin identity before submission. Replay it after a lost
    /// lease acknowledgement, before any chunk was accepted. After a partial
    /// download fails and its lease is abandoned, a new full download needs a
    /// fresh Begin identity; partial local files are never silently resumed.
    pub fn download_project_package_with_request_id(&mut self, project: ProjectId, operation_id: PackageOperationId,
        destination: &Path, _consent: &PackagePrivacyConsent, control: &PackageDownloadControl,
        request_id: RequestId) -> Result<PublishedProjectPackage> {
        control.check()?;
        let status = self.project_package_status(project.clone(), operation_id.clone())?;
        ensure!(status.state == PackageExportOperationState::Ready, "Package operation {} is {:?}, not Ready", operation_id, status.state);
        let artifact = status.artifact.context("Ready package has no artifact receipt")?;
        // Open and pin the user-selected directory before requesting private bytes.
        let mut staging = DestinationStage::new(destination)?;
        let _progress = control.begin(artifact.byte_len)?;
        let waiting_deadline = Instant::now().checked_add(Duration::from_millis(MAX_PACKAGE_DOWNLOAD_LIFETIME_MS))
            .context("Package verification wait deadline overflow")?;
        let mut begin_retries = 0u32;
        let (lease, started) = loop {
            control.check()?;
            ensure!(Instant::now() < waiting_deadline, "Package verification wait deadline expired; engine operation was not cancelled");
            let started = Instant::now();
            let response = match self.package_request_until(request_id.clone(), Command::BeginProjectPackageDownload {
                operation_id: operation_id.clone(), offset: 0, byte_len: artifact.byte_len,
            }, project.clone(), None, waiting_deadline) {
                Ok(response) => response,
                Err(error) if transient_package_io(&error) && begin_retries < 2 => {
                    begin_retries += 1;
                    bounded_package_pause(waiting_deadline)?;
                    continue;
                }
                Err(error) => return Err(error.context(format!("Package Begin request {} for operation {}; if its lease acknowledgement was lost, recover using this exact request ID", request_id, operation_id))),
            };
            ensure!(Instant::now() < waiting_deadline,
                "Package Begin request {} exceeded its verification wait budget; outcome may require same-ID recovery", request_id);
            match response {
                ResponseBody::PackageDownload(lease) => break (lease, started),
                ResponseBody::PackageDownloadPending(pending) => {
                    pending.validate()?;
                    ensure!(pending.operation_id == operation_id && pending.artifact_sha256 == artifact.sha256
                        && pending.total_bytes == artifact.byte_len, "Package verification response identity mismatch");
                    control.inner.verification_bytes.store(pending.completed_bytes, Ordering::Release);
                    bounded_package_pause(waiting_deadline)?;
                }
                _ => bail!("Expected package-specific download lease or verification progress"),
            }
        };
        let result = (|| -> Result<()> {
            validate_package_lease(&lease, &operation_id, &artifact, &self.package_bulk_endpoint()?)?;
            let deadline = started.checked_add(Duration::from_millis(lease.expires_after_ms))
                .context("Package deadline overflow")?;
            ensure!(deadline > Instant::now(), "Package lease expired before download");
            let mut bulk = None;
            let mut offset = 0u64;
            let mut digest = Sha256::new();
            let mut total_reconnects = 0u32;
            while offset < artifact.byte_len {
                control.check()?;
                ensure!(Instant::now() < deadline, "Package transfer absolute deadline expired");
                let byte_len = (artifact.byte_len - offset).min(MAX_ARTIFACT_CHUNK_BYTES as u64) as u32;
                let range = PackageChunkRange { offset, byte_len };
                let mut retries = 0;
                let bytes = loop {
                    if bulk.is_none() {
                        bulk = Some(self.open_package_bulk(&lease, deadline, range)?);
                    }
                    match bulk.as_mut().expect("package transport established").download(offset, byte_len) {
                        Ok(bytes) => break bytes,
                        Err(error) => {
                            let error: anyhow::Error = error.into();
                            // Scope/identity failures cannot be fixed by reconnecting.
                            if !transient_package_io(&error) || retries >= 1 || total_reconnects >= 8 {
                                return Err(error.context("Package chunk failed; no partial chunk was written"));
                            }
                            retries += 1; total_reconnects += 1; bulk = None;
                        }
                    }
                };
                ensure!(bytes.len() == byte_len as usize, "Package chunk length mismatch");
                control.check()?;
                staging.file_mut().write_all(&bytes)?;
                digest.update(&bytes);
                offset = offset.checked_add(bytes.len() as u64).context("Package offset overflow")?;
                control.inner.received.store(offset, Ordering::Release);
            }
            ensure!(offset == artifact.byte_len && staging.file_mut().metadata()?.len() == artifact.byte_len,
                "Package length differs from its immutable receipt");
            ensure!(format!("{:x}", digest.finalize()) == artifact.sha256, "Package digest mismatch; destination was not published");
            control.check()?;
            Ok(())
        })();
        let cleanup_confirmed = matches!(self.package_request(new_request_id(),
            Command::AbandonProjectPackageDownload { lease_id: lease.lease_id.clone() }, project, None),
            Ok(ResponseBody::PackageDownloadAbandoned { lease_id }) if lease_id == lease.lease_id);
        result.with_context(|| format!("Package operation {} retained for retry", operation_id))?;
        control.check()?;
        let destination = staging.publish()?;
        Ok(PublishedProjectPackage { operation_id, destination, artifact,
            verification: "full_file_length_and_sha256_not_import_qualification",
            file_and_directory_synced: true, lease_cleanup_confirmed: cleanup_confirmed, engine_release_requested: false })
    }
}

fn bounded_package_pause(deadline: Instant) -> Result<()> {
    let remaining = deadline.checked_duration_since(Instant::now()).context("Package verification wait deadline expired")?;
    std::thread::sleep(remaining.min(Duration::from_millis(100)));
    Ok(())
}

fn transient_package_io(error: &anyhow::Error) -> bool {
    // TransportError intentionally has no source() chain. Match its typed I/O
    // variant explicitly; never retry JSON, identity, framing or authority errors.
    error.chain().any(|cause| {
        let io = match cause.downcast_ref::<TransportError>() {
            Some(TransportError::Io(error)) => Some(error),
            Some(_) => None,
            None => cause.downcast_ref::<std::io::Error>(),
        };
        io.is_some_and(|error| matches!(error.kind(),
            std::io::ErrorKind::Interrupted | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::NotConnected | std::io::ErrorKind::WouldBlock))
    })
}

fn checked_status(response: ResponseBody, project: &ProjectId, operation: Option<&PackageOperationId>) -> Result<PackageExportStatus> {
    match response {
        ResponseBody::PackageExport(status) => {
            status.validate()?;
            ensure!(&status.project_id == project && operation.is_none_or(|id| id == &status.operation_id),
                "Package status project/operation identity mismatch");
            Ok(status)
        }
        _ => bail!("Expected package operation status"),
    }
}

pub(crate) fn validate_package_lease(lease: &PackageDownloadLease, operation: &PackageOperationId,
    artifact: &PackageArtifactDescriptor, expected_endpoint: &Path) -> Result<()> {
    lease.validate()?;
    ensure!(&lease.operation_id == operation && &lease.artifact == artifact,
        "Package lease differs from the pinned operation/artifact/capture identity");
    ensure!(lease.offset == 0 && lease.byte_len == artifact.byte_len && lease.next_offset == 0 && lease.replay.is_none(),
        "New package lease is not the complete requested range");
    ensure!(lease.bulk_endpoint == expected_endpoint, "Package bulk endpoint redirect rejected before authentication");
    Ok(())
}

pub(crate) fn validate_package_handshake(expected: &PackageDownloadLease, actual: &PackageDownloadLease,
    range: PackageChunkRange) -> Result<()> {
    actual.validate()?;
    ensure!(actual.lease_id == expected.lease_id && actual.engine_epoch == expected.engine_epoch
        && actual.operation_id == expected.operation_id && actual.artifact == expected.artifact
        && actual.offset == expected.offset && actual.byte_len == expected.byte_len
        && actual.bulk_endpoint == expected.bulk_endpoint && actual.expires_after_ms <= expected.expires_after_ms,
        "Package handshake changed immutable lease identity, range, endpoint or lifetime");
    actual.validate_chunk(range)?;
    Ok(())
}

/// Only the explicit allow-packaging command may read this independent proof.
pub(crate) fn allow_packaging_local(client: &mut SessionClient, project: ProjectId,
    bootstrap: &Path, _consent: &PackagePrivacyConsent) -> Result<()> {
    let proof = read_owner_proof(bootstrap.parent().context("Missing private engine directory")?)?;
    match client.package_request(new_request_id(), Command::AllowProjectPackaging { owner_proof: proof }, project, None)? {
        ResponseBody::Ack => Ok(()),
        _ => bail!("Expected packaging approval acknowledgement"),
    }
}

#[cfg(unix)]
fn read_owner_proof(directory: &Path) -> Result<String> {
    use rustix::fs::{open, openat, OFlags, Mode};
    use std::os::unix::fs::MetadataExt;
    let dir = File::from(open(directory, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?);
    let metadata = dir.metadata()?;
    ensure!(metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o077 == 0,
        "Packaging approval directory is not private to the current OS user");
    let file = File::from(openat(&dir, "package-owner.token", OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())?);
    let metadata = file.metadata()?;
    ensure!(metadata.is_file() && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o777 == 0o600 && metadata.nlink() == 1 && metadata.len() == 64,
        "Packaging approval proof must be a private, single-link 0600 regular file");
    let mut proof = String::new();
    file.take(65).read_to_string(&mut proof)?;
    ensure!(proof.len() == 64 && proof.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid packaging approval proof");
    Ok(proof)
}
#[cfg(not(unix))]
fn read_owner_proof(_: &Path) -> Result<String> {
    bail!("Host-local packaging approval is not implemented on this platform; no proof was read")
}

#[cfg(unix)]
struct DestinationStage {
    destination: PathBuf, parent_path: PathBuf, parent: File, directory: File,
    stage_name: String, destination_name: std::ffi::OsString, file: File,
    preserve: bool, file_present: bool,
}
#[cfg(unix)]
impl DestinationStage {
    fn new(destination: &Path) -> Result<Self> {
        use rustix::fs::{open, openat, mkdirat, statat, AtFlags, OFlags, Mode};
        let absolute = if destination.is_absolute() { destination.to_owned() } else { std::env::current_dir()?.join(destination) };
        let destination_name = absolute.file_name().context("Package destination must name a new file")?.to_owned();
        let parent_path = absolute.parent().context("Package destination has no parent")?.canonicalize()?;
        let parent = File::from(open(&parent_path, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?);
        match statat(&parent, &destination_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => bail!("Package destination already exists; nothing was overwritten"),
            Err(rustix::io::Errno::NOENT) => {},
            Err(error) => return Err(error.into()),
        }
        let stage_name = format!(".pulsar-package-{}", uuid::Uuid::new_v4());
        mkdirat(&parent, &stage_name, Mode::from_raw_mode(0o700))?;
        let directory = File::from(openat(&parent, &stage_name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?);
        let file = File::from(openat(&directory, "bundle",
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600))?);
        Ok(Self { destination: parent_path.join(&destination_name), parent_path, parent, directory,
            stage_name, destination_name, file, preserve: false, file_present: true })
    }
    fn file_mut(&mut self) -> &mut File { &mut self.file }
    fn parent_matches(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        let Ok(current) = fs::symlink_metadata(&self.parent_path) else { return false };
        let Ok(held) = self.parent.metadata() else { return false };
        current.is_dir() && current.dev() == held.dev() && current.ino() == held.ino()
    }
    fn failure(&self, outcome: PackagePublicationOutcome, detail: impl Into<String>) -> anyhow::Error {
        PackagePublicationError { destination: self.destination.clone(), outcome,
            recovery_staging_path: self.file_present.then(|| self.parent_path.join(&self.stage_name).join("bundle")),
            detail: detail.into() }.into()
    }
    fn publish(mut self) -> Result<PathBuf> {
        use rustix::fs::{statat, AtFlags};
        use std::os::unix::fs::MetadataExt;
        // Completed verified bytes survive ambiguous publication failures.
        self.preserve = true;
        if !self.parent_matches() {
            return Err(self.failure(PackagePublicationOutcome::NotPublished, "Selected destination directory was replaced or renamed"));
        }
        let held = self.file.metadata()?;
        let named = statat(&self.directory, "bundle", AtFlags::SYMLINK_NOFOLLOW)?;
        ensure!(held.is_file() && held.nlink() == 1 && held.dev() == named.st_dev && held.ino() == named.st_ino,
            "Private package staging path no longer identifies the verified regular file");
        self.file.sync_all().map_err(|error| self.failure(PackagePublicationOutcome::NotPublished, format!("File sync failed: {error}")))?;
        self.directory.sync_all().map_err(|error| self.failure(PackagePublicationOutcome::NotPublished, format!("Staging directory sync failed: {error}")))?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let published = rustix::fs::renameat_with(&self.directory, "bundle", &self.parent,
            &self.destination_name, rustix::fs::RenameFlags::NOREPLACE);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let published = rustix::fs::linkat(&self.directory, "bundle", &self.parent,
            &self.destination_name, AtFlags::empty());
        if let Err(error) = published {
            let outcome = if error == rustix::io::Errno::EXIST { PackagePublicationOutcome::NotPublished } else { PackagePublicationOutcome::OutcomeUnknown };
            return Err(self.failure(outcome, format!("No-clobber publication failed: {error}")));
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { self.file_present = false; }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            rustix::fs::unlinkat(&self.directory, "bundle", AtFlags::empty()).map_err(|error|
                self.failure(PackagePublicationOutcome::PublishedDurabilityUnconfirmed, format!("Published, but staging link cleanup failed: {error}")))?;
            self.file_present = false;
        }
        self.parent.sync_all().map_err(|error| self.failure(PackagePublicationOutcome::PublishedDurabilityUnconfirmed,
            format!("Destination published, but parent directory sync failed: {error}")))?;
        self.directory.sync_all().map_err(|error| self.failure(PackagePublicationOutcome::PublishedDurabilityUnconfirmed,
            format!("Destination published, but source directory sync failed: {error}")))?;
        if !self.parent_matches() {
            return Err(self.failure(PackagePublicationOutcome::OutcomeUnknown, "Publication completed in the held directory, but its user-visible path was replaced or renamed"));
        }
        let named = statat(&self.parent, &self.destination_name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| self.failure(PackagePublicationOutcome::OutcomeUnknown, format!("Published path cannot be associated: {error}")))?;
        if named.st_dev != held.dev() || named.st_ino != held.ino() {
            return Err(self.failure(PackagePublicationOutcome::OutcomeUnknown, "Published path was replaced; verified bytes must not be confused with its replacement"));
        }
        self.preserve = false;
        self.cleanup();
        self.parent.sync_all().map_err(|error| self.failure(PackagePublicationOutcome::PublishedDurabilityUnconfirmed,
            format!("Destination published, but final directory cleanup sync failed: {error}")))?;
        Ok(self.destination.clone())
    }
    fn cleanup(&mut self) {
        use rustix::fs::{statat, unlinkat, AtFlags};
        use std::os::unix::fs::MetadataExt;
        if self.file_present {
            if let (Ok(named), Ok(held)) = (statat(&self.directory, "bundle", AtFlags::SYMLINK_NOFOLLOW), self.file.metadata()) {
                if named.st_dev == held.dev() && named.st_ino == held.ino() {
                    let _ = unlinkat(&self.directory, "bundle", AtFlags::empty());
                }
            }
        }
        if let (Ok(named), Ok(held)) = (statat(&self.parent, &self.stage_name, AtFlags::SYMLINK_NOFOLLOW), self.directory.metadata()) {
            if named.st_dev == held.dev() && named.st_ino == held.ino() {
                let _ = unlinkat(&self.parent, &self.stage_name, AtFlags::REMOVEDIR);
            }
        }
    }
}
#[cfg(unix)]
impl Drop for DestinationStage { fn drop(&mut self) { if !self.preserve { self.cleanup(); } } }
#[cfg(not(unix))]
struct DestinationStage;
#[cfg(not(unix))]
impl DestinationStage {
    fn new(_: &Path) -> Result<Self> { bail!("Safe package file publication is not implemented on this platform") }
    fn file_mut(&mut self) -> &mut File { unreachable!("Unsupported platform cannot create a package stage") }
    fn publish(self) -> Result<PathBuf> { bail!("Safe package publication is unavailable") }
}


#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn artifact() -> PackageArtifactDescriptor {
        PackageArtifactDescriptor { format_version: PROJECT_PACKAGE_FORMAT_VERSION,
            sha256: "a".repeat(64), byte_len: 1024, manifest_sha256: "b".repeat(64),
            project_id: ProjectId::new("project").unwrap(), captured_revision: RevisionId::new(3),
            captured_event_cursor: 7, object_count: 1, verification: PackageReadyVerification::AllDeclaredObjectsVerified }
    }
    fn lease() -> PackageDownloadLease {
        PackageDownloadLease { lease_id: PackageDownloadId::new("lease").unwrap(), engine_epoch: "epoch".into(),
            operation_id: PackageOperationId::new("operation").unwrap(), artifact: artifact(),
            offset: 0, byte_len: 1024, next_offset: 0, replay: None, expires_after_ms: 1000,
            bulk_endpoint: PathBuf::from("/private/bulk.sock") }
    }
    fn status() -> PackageExportStatus {
        let artifact = artifact();
        PackageExportStatus { operation_id: PackageOperationId::new("operation").unwrap(),
            project_id: artifact.project_id.clone(), request_id: RequestId::new("start").unwrap(),
            requested_revision: artifact.captured_revision, state: PackageExportOperationState::Ready,
            capture: Some(artifact.capture_binding()), progress: PackageExportProgress {
                completed_objects: 1, total_objects: Some(1), completed_bytes: 1024, total_bytes: Some(1024),
            }, artifact: Some(artifact), error: None }
    }
    #[test]
    fn package_cli_requires_explicit_privacy_and_disposal_acknowledgement() {
        for args in [
            vec!["pulsar", "package-export", "p", "/tmp/export"],
            vec!["pulsar", "allow-packaging", "p"],
            vec!["pulsar", "package-download", "p", "o", "/tmp/export"],
            vec!["pulsar", "package-release", "p", "o"],
        ] { assert!(crate::cli::Cli::try_parse_from(args).is_err()); }
        for args in [
            vec!["pulsar", "package-export", "p", "/tmp/export", "--acknowledge-unencrypted-private-data", "--revision", "3"],
            vec!["pulsar", "allow-packaging", "p", "--acknowledge-unencrypted-private-data"],
            vec!["pulsar", "package-download", "p", "o", "/tmp/export", "--acknowledge-unencrypted-private-data"],
            vec!["pulsar", "package-release", "p", "o", "--acknowledge-discarding-engine-copy"],
            vec!["pulsar", "package-status", "p", "o"],
            vec!["pulsar", "package-cancel", "p", "o"],
        ] { assert!(crate::cli::Cli::try_parse_from(args).is_ok()); }
    }
    #[test]
    fn package_status_is_bound_to_requested_project_and_operation() {
        let original = status();
        checked_status(ResponseBody::PackageExport(original.clone()), &original.project_id, Some(&original.operation_id)).unwrap();
        assert!(checked_status(ResponseBody::PackageExport(original.clone()), &ProjectId::new("other").unwrap(), None).is_err());
        assert!(checked_status(ResponseBody::PackageExport(original.clone()), &original.project_id, Some(&PackageOperationId::new("other").unwrap())).is_err());
        let mut changed = original.clone(); changed.requested_revision = RevisionId::new(9);
        assert!(checked_status(ResponseBody::PackageExport(changed), &original.project_id, None).is_err());
    }
    #[test]
    fn package_lease_rejects_redirect_range_and_capture_substitution() {
        let expected = lease();
        validate_package_lease(&expected, &expected.operation_id, &expected.artifact, Path::new("/private/bulk.sock")).unwrap();
        for index in 0..5 {
            let mut altered = expected.clone();
            match index {
                0 => altered.bulk_endpoint = "/attacker/bulk.sock".into(),
                1 => altered.artifact.captured_event_cursor += 1,
                2 => altered.artifact.sha256 = "c".repeat(64),
                3 => altered.byte_len -= 1,
                _ => altered.operation_id = PackageOperationId::new("other").unwrap(),
            }
            assert!(validate_package_lease(&altered, &expected.operation_id, &expected.artifact, Path::new("/private/bulk.sock")).is_err());
        }
    }
    #[test]
    fn package_reconnect_accepts_only_exact_lost_chunk_replay_with_frozen_authority() {
        let expected = lease();
        let chunk = PackageChunkRange { offset: 0, byte_len: 128 };
        validate_package_handshake(&expected, &expected, chunk).unwrap();
        let mut advanced = expected.clone(); advanced.next_offset = 128; advanced.replay = Some(chunk);
        validate_package_handshake(&expected, &advanced, chunk).unwrap();
        assert!(validate_package_handshake(&expected, &advanced, PackageChunkRange { offset: 0, byte_len: 64 }).is_err());
        for index in 0..4 {
            let mut altered = advanced.clone();
            match index {
                0 => altered.engine_epoch = "changed".into(),
                1 => altered.expires_after_ms += 1,
                2 => altered.artifact.manifest_sha256 = "d".repeat(64),
                _ => altered.bulk_endpoint = "/attacker/bulk.sock".into(),
            }
            assert!(validate_package_handshake(&expected, &altered, chunk).is_err());
        }
    }
    #[test]
    fn retry_policy_preserves_semantic_failures_and_absolute_wait_budget() {
        assert!(transient_package_io(&std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()));
        assert!(transient_package_io(&anyhow::Error::from(TransportError::Io(std::io::ErrorKind::UnexpectedEof.into()))));
        assert!(!transient_package_io(&anyhow::Error::from(TransportError::Protocol(ProtocolError::invalid("bad request")))));
        assert!(!transient_package_io(&anyhow::Error::from(TransportError::ResponseMismatch)));
        assert!(!transient_package_io(&ProtocolError::invalid("bad request").into()));
        assert!(!transient_package_io(&std::io::Error::from(std::io::ErrorKind::InvalidData).into()));
        assert!(bounded_package_pause(Instant::now() - Duration::from_millis(1)).is_err());
    }
    #[test]
    fn public_recovery_ids_are_preserved_in_cli_arguments() {
        let parsed = crate::cli::Cli::try_parse_from(["pulsar", "package-export", "p", "/tmp/a",
            "--acknowledge-unencrypted-private-data", "--request-id", "stable-start"]).unwrap();
        assert!(matches!(parsed.command, Some(crate::cli::Commands::PackageExport { request_id: Some(id), .. }) if id == "stable-start"));
        let parsed = crate::cli::Cli::try_parse_from(["pulsar", "package-download", "p", "o", "/tmp/a",
            "--acknowledge-unencrypted-private-data", "--request-id", "stable-begin"]).unwrap();
        assert!(matches!(parsed.command, Some(crate::cli::Commands::PackageDownload { request_id: Some(id), .. }) if id == "stable-begin"));
    }
    #[test]
    fn cancellation_is_local_and_progress_cannot_be_shared_concurrently() {
        let progress = PackageDownloadControl::default();
        let clone = progress.clone();
        let guard = progress.begin(100).unwrap();
        assert!(clone.begin(100).is_err());
        clone.cancel();
        assert!(progress.check().is_err());
        assert_eq!(progress.total_bytes(), 100);
        assert_eq!(progress.received_bytes(), 0);
        drop(guard);
        assert!(progress.begin(100).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn private_staging_is_not_visible_as_destination_until_durable_publish() {
        use std::os::unix::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap(); let destination = root.path().join("result.pulsar");
        let mut stage = DestinationStage::new(&destination).unwrap();
        stage.file_mut().write_all(b"verified bytes").unwrap();
        assert!(!destination.exists());
        assert_eq!(stage.file.metadata().unwrap().mode() & 0o777, 0o600);
        assert_eq!(stage.directory.metadata().unwrap().mode() & 0o777, 0o700);
        assert_eq!(stage.publish().unwrap(), destination);
        assert_eq!(fs::read(&destination).unwrap(), b"verified bytes");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn partial_staging_drop_removes_only_owned_files() {
        let root = tempfile::tempdir().unwrap(); let destination = root.path().join("partial.pulsar");
        { let mut stage = DestinationStage::new(&destination).unwrap(); stage.file_mut().write_all(b"partial").unwrap(); }
        assert!(!destination.exists()); assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn existing_and_raced_destinations_are_never_clobbered() {
        let root = tempfile::tempdir().unwrap(); let destination = root.path().join("occupied");
        fs::write(&destination, b"original").unwrap();
        assert!(DestinationStage::new(&destination).is_err());
        fs::remove_file(&destination).unwrap();
        let mut stage = DestinationStage::new(&destination).unwrap(); stage.file_mut().write_all(b"new").unwrap();
        fs::write(&destination, b"raced original").unwrap();
        let error = stage.publish().unwrap_err();
        assert_eq!(error.downcast_ref::<PackagePublicationError>().unwrap().outcome, PackagePublicationOutcome::NotPublished);
        assert_eq!(fs::read(&destination).unwrap(), b"raced original");
    }
    #[cfg(unix)]
    #[test]
    fn symlink_destination_and_replaced_parent_do_not_redirect_publication() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("selected"); fs::create_dir(&parent).unwrap();
        let destination = parent.join("result");
        let target = root.path().join("original"); fs::write(&target, b"original").unwrap();
        symlink(&target, &destination).unwrap(); assert!(DestinationStage::new(&destination).is_err());
        fs::remove_file(&destination).unwrap();
        let mut stage = DestinationStage::new(&destination).unwrap(); stage.file_mut().write_all(b"new").unwrap();
        fs::rename(&parent, root.path().join("moved")).unwrap(); fs::create_dir(&parent).unwrap();
        let error = stage.publish().unwrap_err();
        assert_eq!(error.downcast_ref::<PackagePublicationError>().unwrap().outcome, PackagePublicationOutcome::NotPublished);
        assert!(!destination.exists()); assert_eq!(fs::read(&target).unwrap(), b"original");
    }
    #[cfg(unix)]
    #[test]
    fn replaced_staging_file_is_not_published_as_verified_bytes() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap(); let destination = root.path().join("result");
        let mut stage = DestinationStage::new(&destination).unwrap(); stage.file_mut().write_all(b"verified").unwrap();
        let path = root.path().join(&stage.stage_name).join("bundle");
        let target = root.path().join("other"); fs::write(&target, b"unverified").unwrap();
        fs::remove_file(&path).unwrap(); symlink(&target, &path).unwrap();
        assert!(stage.publish().is_err()); assert!(!destination.exists());
        assert_eq!(fs::read(&target).unwrap(), b"unverified");
    }
    #[cfg(unix)]
    #[test]
    fn owner_proof_is_private_exact_single_link_and_never_followed() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("package-owner.token");
        fs::write(&path, "a".repeat(64)).unwrap(); fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_owner_proof(root.path()).unwrap(), "a".repeat(64));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(); assert!(read_owner_proof(root.path()).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&path, root.path().join("hardlink")).unwrap(); assert!(read_owner_proof(root.path()).is_err());
        fs::remove_file(root.path().join("hardlink")).unwrap();
        fs::rename(&path, root.path().join("original")).unwrap();
        symlink(root.path().join("original"), &path).unwrap(); assert!(read_owner_proof(root.path()).is_err());
    }
}


#[cfg(all(test, unix))]
mod recovery_socket_tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    fn accept(listener: &UnixListener) -> UnixStream {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    std::thread::sleep(Duration::from_millis(1)),
                Err(error) => panic!("bounded fake control accept: {error}"),
            }
        }
    }
    fn queued(request: &Request) -> PackageExportStatus {
        PackageExportStatus {
            operation_id: PackageOperationId::new("operation").unwrap(),
            project_id: request.project.clone().unwrap(),
            request_id: request.request_id.clone(),
            requested_revision: request.expected_revision.unwrap(),
            state: PackageExportOperationState::Queued,
            capture: None, progress: PackageExportProgress::default(), artifact: None, error: None,
        }
    }
    fn send(stream: &mut UnixStream, request: &Request, result: std::result::Result<ResponseBody, ProtocolError>) {
        write_message(stream, &Response { version: PROTOCOL_VERSION, request_id: request.request_id.clone(), result }).unwrap();
    }
    #[test]
    fn lost_start_acknowledgement_can_be_replayed_with_the_callers_exact_identity() {
        let root = tempfile::tempdir().unwrap(); let endpoint = root.path().join("engine.sock");
        let listener = UnixListener::bind(&endpoint).unwrap(); listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut first = accept(&listener); let first_request: Request = read_message(&mut first).unwrap();
            drop(first); // An effect could already have committed; no response reaches the client.
            let mut second = accept(&listener); let second_request: Request = read_message(&mut second).unwrap();
            send(&mut second, &second_request, Ok(ResponseBody::PackageExport(queued(&second_request))));
            (first_request, second_request)
        });
        let mut client = SessionClient::from_credentials(&endpoint, SessionId::new("actor").unwrap(), "private-token".into()).unwrap();
        let project = ProjectId::new("project").unwrap(); let request = RequestId::new("recoverable-start").unwrap();
        let consent = PackagePrivacyConsent::acknowledge_unencrypted_private_data();
        let error = client.start_project_export_with_request_id(project.clone(), RevisionId::new(3), &consent, request.clone()).unwrap_err();
        assert!(format!("{error:#}").contains("recoverable-start"));
        let status = client.start_project_export_with_request_id(project, RevisionId::new(3), &consent, request.clone()).unwrap();
        assert_eq!(status.request_id, request);
        let (first, second) = server.join().unwrap();
        assert_eq!(first.request_id, second.request_id);
        assert_eq!(first.project, second.project);
        assert_eq!(first.expected_revision, second.expected_revision);
        assert!(matches!(first.command, Command::StartProjectExport));
        assert!(matches!(second.command, Command::StartProjectExport));
    }
    #[test]
    fn dropped_begin_ack_and_pending_poll_reuse_one_id_but_semantic_errors_stop() {
        let root = tempfile::tempdir().unwrap(); let endpoint = root.path().join("engine.sock");
        let listener = UnixListener::bind(&endpoint).unwrap(); listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = accept(&listener); let status_request: Request = read_message(&mut stream).unwrap();
            let artifact = PackageArtifactDescriptor { format_version: PROJECT_PACKAGE_FORMAT_VERSION,
                sha256: "a".repeat(64), byte_len: 1024, manifest_sha256: "b".repeat(64),
                project_id: status_request.project.clone().unwrap(), captured_revision: RevisionId::new(3),
                captured_event_cursor: 7, object_count: 1, verification: PackageReadyVerification::AllDeclaredObjectsVerified };
            let operation_id = PackageOperationId::new("operation").unwrap();
            let status = PackageExportStatus { operation_id: operation_id.clone(),
                project_id: artifact.project_id.clone(), request_id: RequestId::new("start").unwrap(),
                requested_revision: artifact.captured_revision, state: PackageExportOperationState::Ready,
                capture: Some(artifact.capture_binding()), progress: PackageExportProgress {
                    completed_objects: 1, total_objects: Some(1), completed_bytes: 1024, total_bytes: Some(1024),
                }, artifact: Some(artifact.clone()), error: None };
            send(&mut stream, &status_request, Ok(ResponseBody::PackageExport(status))); drop(stream);
            let mut stream = accept(&listener); let first: Request = read_message(&mut stream).unwrap(); drop(stream);
            let mut stream = accept(&listener); let second: Request = read_message(&mut stream).unwrap();
            send(&mut stream, &second, Ok(ResponseBody::PackageDownloadPending(PackageDownloadPending {
                operation_id, artifact_sha256: artifact.sha256,
                verification: PackageDownloadVerificationState::Verifying, completed_bytes: 512, total_bytes: 1024,
            }))); drop(stream);
            let mut stream = accept(&listener); let third: Request = read_message(&mut stream).unwrap();
            send(&mut stream, &third, Err(ProtocolError::invalid("semantic rejection after verification")));
            (first, second, third)
        });
        let mut client = SessionClient::from_credentials(&endpoint, SessionId::new("actor").unwrap(), "private-token".into()).unwrap();
        let request_id = RequestId::new("recoverable-begin").unwrap();
        let destination = root.path().join("never-published");
        let error = client.download_project_package_with_request_id(ProjectId::new("project").unwrap(),
            PackageOperationId::new("operation").unwrap(), &destination,
            &PackagePrivacyConsent::acknowledge_unencrypted_private_data(),
            &PackageDownloadControl::default(), request_id.clone()).unwrap_err();
        assert!(format!("{error:#}").contains("recoverable-begin"));
        assert!(format!("{error:#}").contains("semantic rejection"), "{error:#}; debug={error:?}");
        assert!(!destination.exists());
        let (first, second, third) = server.join().unwrap();
        for request in [first, second, third] {
            assert_eq!(request.request_id, request_id);
            assert!(matches!(request.command, Command::BeginProjectPackageDownload { offset: 0, byte_len: 1024, .. }));
        }
    }
}

#[cfg(test)]
mod progress_reuse_tests {
    use super::*;
    #[test]
    fn reused_download_control_clears_all_previous_progress() {
        let control = PackageDownloadControl::default();
        {
            let _guard = control.begin(1024).unwrap();
            control.inner.verification_bytes.store(512, Ordering::Release);
            control.inner.received.store(128, Ordering::Release);
        }
        let _guard = control.begin(2048).unwrap();
        assert_eq!(control.total_bytes(), 2048);
        assert_eq!(control.received_bytes(), 0);
        assert_eq!(control.verification_bytes(), 0, "prior export verification must not appear in a new transfer");
    }
}

