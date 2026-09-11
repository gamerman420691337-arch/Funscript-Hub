//! Engine-owned portable-package content holds and immutable publication.
//!
//! Authority stays in the engine. Callers acquire/extend holds under
//! artifact_gate -> database -> brief registry locking, and check holds before
//! eviction under the same gate. A guard's Drop takes ONLY the registry.
//! Captured sources stay held until durable bundle AND READY reference commit;
//! downloads hold the bundle separately. No source is an automatic GC target.
//!
//! The store streams bounded bytes to an engine-private pending file, fsyncs,
//! publishes without replacement, and fsyncs the directory. Errors after a
//! publication or removal may have taken effect are UnknownOutcome. Complete
//! orphans count against admission and are not silently swept. Startup pending
//! cleanup requires exclusive lifecycle recovery; removal requires no SQL READY
//! references and no active read holds, proved by the caller under artifact_gate.
//!
//! Generic I/O, fsync, and callbacks can block: checks are cooperative, not hard
//! deadlines. No protection against malicious processes with the same OS user
//! is claimed. Readonly opens require a previously verified authorized READY
//! descriptor; full verification belongs outside authority/database locks.

use crate::project_package_format::{
    write_package, PackageCodecLimits, PACKAGE_HEADER_BYTES, PACKAGE_MAGIC,
};
use pulsar_core::ArtifactId;
use pulsar_protocol::{
    ArtifactIdentity, ErrorCode, PackageObjectDescriptor, PortableProjectManifest,
    ProtocolError, DEFAULT_MAX_PROJECT_PACKAGE_BYTES, MAX_PROJECT_PACKAGE_ENTRIES,
    MAX_PROJECT_PACKAGE_MANIFEST_BYTES, PROJECT_PACKAGE_FORMAT_VERSION,
};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

type Result<T> = std::result::Result<T, ProtocolError>;
const BUFFER_BYTES: usize = 64 * 1024;

fn invalid(message: &str) -> ProtocolError { ProtocolError::invalid(message) }
fn exhausted(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ResourceExhausted, message)
}
fn mismatch(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::DependencyMismatch, message)
}
fn internal(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, message)
}
fn unknown(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::UnknownOutcome, message)
}
fn io_error(error: io::Error) -> ProtocolError {
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(libc::ENOSPC) | Some(libc::EDQUOT)) {
        return exhausted("package storage capacity unavailable");
    }
    ProtocolError::new(ErrorCode::Unavailable, format!("package storage I/O: {error}"))
}
fn valid_sha(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone)]
pub struct HoldRegistry {
    inner: Arc<Mutex<BTreeMap<String, HeldContent>>>,
    max_entries: usize,
}
#[derive(Clone, Copy)]
struct HeldContent { byte_len: u64, count: usize }
impl Default for HoldRegistry {
    fn default() -> Self {
        Self { inner: Arc::new(Mutex::new(BTreeMap::new())), max_entries: MAX_PROJECT_PACKAGE_ENTRIES }
    }
}
pub struct ArtifactHolds {
    registry: HoldRegistry,
    identities: BTreeMap<String, u64>,
}
impl HoldRegistry {
    /// Caller serializes this acquisition with matching eviction using artifact_gate.
    pub fn acquire(&self, identities: &[ArtifactIdentity]) -> Result<ArtifactHolds> {
        let mut holds = ArtifactHolds { registry: self.clone(), identities: BTreeMap::new() };
        holds.extend(identities)?;
        Ok(holds)
    }
    /// Failure is not absence: callers must not evict when the registry is poisoned.
    pub fn contains(&self, sha256: &str) -> Result<bool> {
        if !valid_sha(sha256) { return Err(invalid("invalid retention digest")); }
        let held = self.inner.lock().map_err(|_| internal("retention registry poisoned"))?;
        Ok(held.contains_key(sha256))
    }
}
impl ArtifactHolds {
    /// Atomic, deduplicated extension. ArtifactId aliases do not duplicate a hold.
    pub fn extend(&mut self, identities: &[ArtifactIdentity]) -> Result<()> {
        if identities.len() > self.registry.max_entries {
            return Err(exhausted("retention request exceeds entry budget"));
        }
        let mut additions = BTreeMap::new();
        for identity in identities {
            if !valid_sha(&identity.sha256) || identity.byte_len > DEFAULT_MAX_PROJECT_PACKAGE_BYTES {
                return Err(invalid("invalid bounded retained content identity"));
            }
            if let Some(length) = self.identities.get(&identity.sha256) {
                if *length != identity.byte_len { return Err(mismatch("retained digest has conflicting length")); }
                continue;
            }
            if additions.insert(identity.sha256.clone(), identity.byte_len)
                .is_some_and(|length| length != identity.byte_len) {
                return Err(mismatch("retention request has conflicting lengths"));
            }
        }
        let mut held = self.registry.inner.lock().map_err(|_| internal("retention registry poisoned"))?;
        let mut new_keys = 0usize;
        for (sha, length) in &additions {
            if let Some(existing) = held.get(sha) {
                if existing.byte_len != *length { return Err(mismatch("retained digest has conflicting length")); }
                if existing.count == usize::MAX { return Err(exhausted("retention reference count overflow")); }
            } else { new_keys += 1; }
        }
        if held.len().checked_add(new_keys).is_none_or(|count| count > self.registry.max_entries) {
            return Err(exhausted("aggregate retention entry budget exceeded"));
        }
        for (sha, length) in additions {
            held.entry(sha.clone()).and_modify(|entry| entry.count += 1)
                .or_insert(HeldContent { byte_len: length, count: 1 });
            self.identities.insert(sha, length);
        }
        Ok(())
    }
}
impl Drop for ArtifactHolds {
    fn drop(&mut self) {
        // Fail closed on poison: conservative retention survives until recovery.
        let Ok(mut held) = self.registry.inner.lock() else { return; };
        for sha in self.identities.keys() {
            if let Some(entry) = held.get_mut(sha) {
                entry.count -= 1;
                if entry.count == 0 { held.remove(sha); }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoredPackage {
    pub identity: ArtifactIdentity,
    pub manifest_sha256: String,
}
#[derive(Clone, Debug)]
pub struct PackageEstimate {
    pub byte_len: u64,
    pub manifest_sha256: String,
}
pub struct PackageStore {
    #[cfg(unix)]
    directory: File,
    #[cfg(test)]
    fault: Mutex<Option<PublicationFault>>,
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PublicationFault { BeforePublish, AfterPublish, BeforeDirectorySync, BeforeRemovalSync, FailPendingUnlink, FailCleanupAfterFirstRemoval }

fn validate_stored(value: &StoredPackage) -> Result<()> {
    if !valid_sha(&value.identity.sha256) || !valid_sha(&value.manifest_sha256)
        || value.identity.byte_len < PACKAGE_HEADER_BYTES as u64
        || value.identity.byte_len > DEFAULT_MAX_PROJECT_PACKAGE_BYTES
        || value.identity.artifact_id.as_str() != format!("package.{}", value.identity.sha256) {
        return Err(invalid("noncanonical package content descriptor"));
    }
    Ok(())
}
impl PackageStore {
    pub fn estimate(manifest: &PortableProjectManifest) -> Result<PackageEstimate> {
        let bytes = manifest.canonical_bytes()?;
        let mut byte_len = (PACKAGE_HEADER_BYTES as u64).checked_add(bytes.len() as u64)
            .ok_or_else(|| exhausted("package length overflow"))?;
        for object in &manifest.objects {
            byte_len = byte_len.checked_add(object.byte_len).ok_or_else(|| exhausted("package length overflow"))?;
        }
        if byte_len > DEFAULT_MAX_PROJECT_PACKAGE_BYTES { return Err(exhausted("package exceeds store byte bound")); }
        Ok(PackageEstimate { byte_len, manifest_sha256: format!("{:x}", Sha256::digest(&bytes)) })
    }
}

struct Callbacks<C, P> {
    check: RefCell<C>,
    progress: RefCell<P>,
    bytes: Cell<u64>,
    objects: Cell<u64>,
    error: RefCell<Option<ProtocolError>>,
}
impl<C: FnMut() -> Result<()>, P: FnMut(u64,u64) -> Result<()>> Callbacks<C,P> {
    fn check(&self) -> io::Result<()> {
        let result = (self.check.borrow_mut())();
        self.map(result)
    }
    fn progress(&self) -> io::Result<()> {
        let result = (self.progress.borrow_mut())(self.objects.get(), self.bytes.get());
        self.map(result)
    }
    fn map(&self, result: Result<()>) -> io::Result<()> {
        result.map_err(|error| {
            if self.error.borrow().is_none() { *self.error.borrow_mut() = Some(error); }
            io::Error::other("package callback interrupted operation")
        })
    }
    fn fail(&self, error: io::Error) -> io::Error {
        if error.kind() != io::ErrorKind::Interrupted && self.error.borrow().is_none() {
            *self.error.borrow_mut() = Some(io_error(io::Error::new(error.kind(), error.to_string())));
            #[cfg(unix)]
            if matches!(error.raw_os_error(), Some(libc::ENOSPC) | Some(libc::EDQUOT)) {
                *self.error.borrow_mut() = Some(exhausted("package storage capacity unavailable"));
            }
        }
        error
    }
    fn finish_error(&self, fallback: ProtocolError) -> ProtocolError {
        self.error.borrow_mut().take().unwrap_or(fallback)
    }
}
struct CheckedWriter<'a,C,P> { file: &'a mut File, callbacks: &'a Callbacks<C,P>, limit:u64 }
impl<C:FnMut()->Result<()>,P:FnMut(u64,u64)->Result<()>> Write for CheckedWriter<'_,C,P> {
    fn write(&mut self, bytes:&[u8])->io::Result<usize> {
        self.callbacks.check()?;
        let length=bytes.len().min(BUFFER_BYTES);
        if self.callbacks.bytes.get().checked_add(length as u64).is_none_or(|n|n>self.limit) {
            return self.callbacks.map(Err(exhausted("package writer exceeded admitted bytes"))).map(|_|0);
        }
        let count=self.file.write(&bytes[..length]).map_err(|e|self.callbacks.fail(e))?;
        self.callbacks.bytes.set(self.callbacks.bytes.get()+count as u64);
        self.callbacks.progress()?;
        Ok(count)
    }
    fn flush(&mut self)->io::Result<()> {
        self.callbacks.check()?;
        self.file.flush().map_err(|e|self.callbacks.fail(e))
    }
}
struct CheckedReader<'a,R,C,P> {
    inner:R, callbacks:&'a Callbacks<C,P>, expected:u64, bytes:u64, completed:bool,
}
impl<R:Read,C:FnMut()->Result<()>,P:FnMut(u64,u64)->Result<()>> Read for CheckedReader<'_,R,C,P> {
    fn read(&mut self, out:&mut [u8])->io::Result<usize> {
        self.callbacks.check()?;
        let length=out.len().min(BUFFER_BYTES);
        let count=self.inner.read(&mut out[..length]).map_err(|e|self.callbacks.fail(e))?;
        if count>length { return Err(io::Error::other("object reader violated bounded read")); }
        self.bytes=self.bytes.checked_add(count as u64).ok_or_else(||io::Error::other("object read overflow"))?;
        // The codec's post-hash EOF probe reaches this after validating digest.
        if length!=0 && count==0 && self.bytes==self.expected && !self.completed {
            self.completed=true;
            self.callbacks.objects.set(self.callbacks.objects.get()+1);
            self.callbacks.progress()?;
        }
        Ok(count)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{CStr,CString};
    use std::os::fd::{AsRawFd,FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt,PermissionsExt};
    use std::path::Component;

    fn directory(root:&Path)->Result<File> {
        if !root.is_absolute() { return Err(invalid("package store root must be absolute")); }
        let components=root.components().collect::<Vec<_>>();
        if components.len()<2 || components.iter().any(|c| !matches!(c,Component::RootDir|Component::Normal(_))) {
            return Err(invalid("unsupported package root components"));
        }
        let mut current=File::open("/").map_err(io_error)?;
        for (index,component) in components.iter().enumerate().skip(1) {
            let Component::Normal(name)=component else { return Err(invalid("invalid package path")); };
            let name=CString::new(name.as_bytes()).map_err(|_|invalid("package path contains NUL"))?;
            // SAFETY: live directory descriptor and NUL-terminated single component.
            let mut fd=unsafe{libc::openat(current.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
            if fd<0 && index+1==components.len() && io::Error::last_os_error().kind()==io::ErrorKind::NotFound {
                // SAFETY: as above; creates only the final engine-controlled component.
                if unsafe{libc::mkdirat(current.as_raw_fd(),name.as_ptr(),0o700)}<0
                    && io::Error::last_os_error().kind()!=io::ErrorKind::AlreadyExists { return Err(io_error(io::Error::last_os_error())); }
                current.sync_all().map_err(io_error)?;
                fd=unsafe{libc::openat(current.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
            }
            if fd<0 { return Err(io_error(io::Error::last_os_error())); }
            // SAFETY: openat returned this uniquely owned descriptor.
            current=unsafe{File::from_raw_fd(fd)};
        }
        let meta=current.metadata().map_err(io_error)?;
        if !meta.is_dir() || meta.uid()!=unsafe{libc::geteuid()} || meta.mode()&0o777!=0o700 {
            return Err(invalid("package store must be current-user-owned mode0700"));
        }
        current.sync_all().map_err(io_error)?;
        Ok(current)
    }
    fn open(directory:&File,name:&CStr)->io::Result<File> {
        // SAFETY: valid directory handle and an engine-generated basename.
        let fd=unsafe{libc::openat(directory.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_NOFOLLOW|libc::O_NONBLOCK|libc::O_CLOEXEC)};
        if fd<0 { return Err(io::Error::last_os_error()); }
        Ok(unsafe{File::from_raw_fd(fd)})
    }
    fn metadata(file:&File,readonly:bool)->Result<std::fs::Metadata> {
        let meta=file.metadata().map_err(io_error)?;
        if !meta.is_file() || meta.uid()!=unsafe{libc::geteuid()}
            || meta.mode()&0o077!=0 || (readonly && meta.mode()&0o222!=0) {
            return Err(invalid("package file must be private, regular, and immutable when complete"));
        }
        Ok(meta)
    }
    fn basename(sha:&str)->Result<CString> {
        if !valid_sha(sha) { return Err(invalid("invalid package digest basename")); }
        CString::new(sha).map_err(|_|invalid("invalid package digest"))
    }
    fn unlink(directory:&File,name:&CStr)->io::Result<()> {
        // SAFETY: only engine-owned basenames relative to the anchored directory.
        if unsafe{libc::unlinkat(directory.as_raw_fd(),name.as_ptr(),0)}<0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
    struct Pending<'a> {file:File,directory:&'a File,name:Option<CString>,#[cfg(test)]fail_unlink:bool}
    impl<'a> Pending<'a> {
        fn new(directory:&'a File)->Result<Self> {
            let name=CString::new(format!(".pending-{}",uuid::Uuid::new_v4())).expect("UUID");
            let fd=unsafe{libc::openat(directory.as_raw_fd(),name.as_ptr(),libc::O_RDWR|libc::O_CREAT|libc::O_EXCL|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600)};
            if fd<0 { return Err(io_error(io::Error::last_os_error())); }
            Ok(Self{file:unsafe{File::from_raw_fd(fd)},directory,name:Some(name),#[cfg(test)]fail_unlink:false})
        }
        fn link(&self,sha:&str)->Result<bool> {
            let destination=basename(sha)?;
            let source=self.name.as_ref().ok_or_else(||internal("missing pending name"))?;
            let result=unsafe{libc::linkat(self.directory.as_raw_fd(),source.as_ptr(),self.directory.as_raw_fd(),destination.as_ptr(),0)};
            if result==0 { return Ok(true); }
            let error=io::Error::last_os_error();
            if error.kind()==io::ErrorKind::AlreadyExists { Ok(false) } else { Err(io_error(error)) }
        }
        fn clear(&mut self)->Result<()> {
            if let Some(name)=&self.name { unlink(self.directory,name).map_err(io_error)?; }
            self.name=None;
            Ok(())
        }
    }
    impl Drop for Pending<'_> {
        fn drop(&mut self) {
            if let Some(name)=&self.name {
                // Deterministic fault injection for an OS unlink refusal.
                #[cfg(test)]
                if self.fail_unlink { let _=Err::<(),_>(io::Error::from(io::ErrorKind::PermissionDenied)); return; }
                let _=unlink(self.directory,name);
            }
        }
    }
    // A fresh open-file description is essential: dup(directory) would share the
    // readdir cursor and could make subsequent disk-admission counts underflow.
    #[cfg(any(target_os="linux",target_os="macos"))]
    fn visit(directory:&File,mut visitor:impl FnMut(&CStr)->Result<()>)->Result<()> {
        let dot=c".";
        let fd=unsafe{libc::openat(directory.as_raw_fd(),dot.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
        if fd<0 { return Err(io_error(io::Error::last_os_error())); }
        let stream=unsafe{libc::fdopendir(fd)};
        if stream.is_null() {
            let error=io::Error::last_os_error();
            unsafe{libc::close(fd)};
            return Err(io_error(error));
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream { fn drop(&mut self) { unsafe{libc::closedir(self.0)}; } }
        let stream=Stream(stream);
        loop {
            #[cfg(target_os="linux")]
            let errno=unsafe{libc::__errno_location()};
            #[cfg(target_os="macos")]
            let errno=unsafe{libc::__error()};
            unsafe{*errno=0};
            let entry=unsafe{libc::readdir(stream.0)};
            if entry.is_null() {
                if unsafe{*errno}!=0 { return Err(io_error(io::Error::last_os_error())); }
                return Ok(());
            }
            // SAFETY: readdir's d_name is NUL-terminated and remains live until
            // the next readdir call. The visitor cannot retain its borrowed name.
            let name=unsafe{CStr::from_ptr((*entry).d_name.as_ptr())};
            if name.to_bytes()!=b"." && name.to_bytes()!=b".." { visitor(name)?; }
        }
    }
    #[cfg(not(any(target_os="linux",target_os="macos")))]
    fn visit(_directory:&File,_visitor:impl FnMut(&CStr)->Result<()>)->Result<()> {
        Err(ProtocolError::new(ErrorCode::Unsupported,"anchored package enumeration unavailable on this platform"))
    }
    fn pending_name(name:&str)->bool {
        name.strip_prefix(".pending-").and_then(|id|uuid::Uuid::parse_str(id).ok())
            .is_some_and(|id|name==format!(".pending-{id}"))
    }

    impl PackageStore {
        pub fn new(root:&Path)->Result<Self> {
            Ok(Self{directory:directory(root)?,#[cfg(test)]fault:Mutex::new(None)})
        }
        pub fn materialize<R:Read,F:FnMut(&PackageObjectDescriptor)->Result<R>,C:FnMut()->Result<()>,P:FnMut(u64,u64)->Result<()>>(
            &self,manifest:&PortableProjectManifest,mut open_object:F,limits:PackageCodecLimits,check:C,progress:P,
        )->Result<StoredPackage> {
            let estimate=Self::estimate(manifest)?;
            if estimate.byte_len>limits.max_package_bytes { return Err(exhausted("package exceeds admitted byte budget")); }
            let callbacks=Callbacks{check:RefCell::new(check),progress:RefCell::new(progress),bytes:Cell::new(0),objects:Cell::new(0),error:RefCell::new(None)};
            callbacks.check().and_then(|_|callbacks.progress()).map_err(|e|callbacks.finish_error(io_error(e)))?;
            let mut pending=Pending::new(&self.directory)?;
            #[cfg(test)]
            { pending.fail_unlink=*self.fault.lock().expect("fault mutex")==Some(PublicationFault::FailPendingUnlink); }
            let digest={
                let mut writer=CheckedWriter{file:&mut pending.file,callbacks:&callbacks,limit:estimate.byte_len};
                write_package(manifest,&mut writer,|descriptor|{
                    callbacks.check().map_err(|e|callbacks.finish_error(io_error(e)))?;
                    let input=open_object(descriptor)?;
                    Ok(CheckedReader{inner:input,callbacks:&callbacks,expected:descriptor.byte_len,bytes:0,completed:false})
                },limits).map_err(|error|callbacks.finish_error(error))?
            };
            if digest.byte_len!=estimate.byte_len || callbacks.objects.get()!=manifest.objects.len() as u64 {
                return Err(mismatch("materialized package differs from admitted snapshot"));
            }
            callbacks.check().map_err(|e|callbacks.finish_error(io_error(e)))?;
            pending.file.set_permissions(std::fs::Permissions::from_mode(0o400)).map_err(io_error)?;
            pending.file.sync_all().map_err(io_error)?;
            #[cfg(test)]
            if *self.fault.lock().expect("fault mutex")==Some(PublicationFault::BeforePublish) { return Err(io_error(io::Error::other("injected prepublication failure"))); }
            callbacks.check().map_err(|e|callbacks.finish_error(io_error(e)))?;
            let value=StoredPackage {
                identity:ArtifactIdentity {
                    artifact_id:ArtifactId::new(format!("package.{}",digest.sha256)).map_err(|_|internal("computed package ID invalid"))?,
                    sha256:digest.sha256,byte_len:digest.byte_len,
                },
                manifest_sha256:estimate.manifest_sha256,
            };
            let created=pending.link(&value.identity.sha256)?;
            // The public name may now exist. Never report subsequent errors as
            // proving no effect; leave complete orphan bytes for reconciliation.
            #[cfg(test)]
            if *self.fault.lock().expect("fault mutex")==Some(PublicationFault::AfterPublish) { return Err(unknown("injected failure after package publication")); }
            if !created {
                self.open_verified(&value,||{
                    callbacks.check().map_err(|e|callbacks.finish_error(io_error(e)))
                }, |_|Ok(()))?.sync_all().map_err(|_|unknown("existing package sync outcome uncertain"))?;
            }
            pending.clear().map_err(|_|unknown("package published but pending-name cleanup uncertain"))?;
            #[cfg(test)]
            if *self.fault.lock().expect("fault mutex")==Some(PublicationFault::BeforeDirectorySync) { return Err(unknown("injected failure before package directory sync")); }
            self.directory.sync_all().map_err(|_|unknown("package publication directory sync outcome uncertain"))?;
            Ok(value)
        }

        /// Fast authorized READY reopen, not a fresh integrity verification.
        pub fn open_readonly(&self,value:&StoredPackage)->Result<File> {
            validate_stored(value)?;
            let mut file=open(&self.directory,&basename(&value.identity.sha256)?).map_err(io_error)?;
            if metadata(&file,true)?.len()!=value.identity.byte_len { return Err(mismatch("package file length differs from receipt")); }
            let mut header=[0u8;PACKAGE_HEADER_BYTES];
            file.read_exact(&mut header).map_err(io_error)?;
            let manifest_len=u64::from_le_bytes(header[12..20].try_into().expect("header slice"));
            if header[..8]!=PACKAGE_MAGIC || header[8..10]!=PROJECT_PACKAGE_FORMAT_VERSION.to_le_bytes()
                || header[10..12]!=[0,0] || manifest_len>MAX_PROJECT_PACKAGE_MANIFEST_BYTES as u64
                || (PACKAGE_HEADER_BYTES as u64).checked_add(manifest_len).is_none_or(|n|n>value.identity.byte_len)
                || hex_digest(&header[20..52])!=value.manifest_sha256 {
                return Err(mismatch("package header differs from verified receipt"));
            }
            file.seek(SeekFrom::Start(0)).map_err(io_error)?;
            Ok(file)
        }
        pub fn open_verified<C:FnMut()->Result<()>,P:FnMut(u64)->Result<()>>(&self,value:&StoredPackage,mut check:C,mut progress:P)->Result<File> {
            check()?;
            progress(0)?;
            let mut file=self.open_readonly(value)?;
            let mut hash=Sha256::new();
            let mut count=0u64;
            let mut buffer=[0u8;BUFFER_BYTES];
            loop {
                check()?;
                let length=match file.read(&mut buffer) {
                    Ok(length)=>length,Err(error) if error.kind()==io::ErrorKind::Interrupted=>continue,
                    Err(error)=>return Err(io_error(error)),
                };
                if length==0 { break; }
                count=count.checked_add(length as u64).ok_or_else(||exhausted("package verification length overflow"))?;
                if count>value.identity.byte_len { return Err(mismatch("package grew beyond its receipt")); }
                hash.update(&buffer[..length]);
                progress(count)?;
            }
            if count!=value.identity.byte_len || format!("{:x}",hash.finalize())!=value.identity.sha256 {
                return Err(mismatch("package content checksum differs from receipt"));
            }
            file.seek(SeekFrom::Start(0)).map_err(io_error)?;
            Ok(file)
        }
        /// Includes complete orphans AND all private pending bytes. Live staging
        /// deliberately also retains its full-size reservation, so admission
        /// can double-count it conservatively. Failed Drop cleanup must never
        /// leave uncharged staging after a reservation ends. One extra FD only.
        pub fn allocated_bytes(&self)->Result<u64> {
            let mut bytes=0u64;
            visit(&self.directory,|name|{
                let text=name.to_str().map_err(|_|invalid("non-UTF8 package store entry"))?;
                if pending_name(text) {
                    let file=open(&self.directory,name).map_err(io_error)?;
                    let length=metadata(&file,false)?.len();
                    bytes=bytes.checked_add(length).ok_or_else(||exhausted("allocated pending byte count overflow"))?;
                    return Ok(());
                }
                if !valid_sha(text) { return Err(invalid("unexpected package store entry")); }
                let file=open(&self.directory,name).map_err(io_error)?;
                let length=metadata(&file,true)?.len();
                if length<PACKAGE_HEADER_BYTES as u64 || length>DEFAULT_MAX_PROJECT_PACKAGE_BYTES {
                    return Err(mismatch("complete package object exceeds size bounds"));
                }
                bytes=bytes.checked_add(length).ok_or_else(||exhausted("allocated package byte count overflow"))?;
                Ok(())
            })?;
            Ok(bytes)
        }
        /// Exclusive startup recovery only; no live materializer may be active.
        /// Complete digest-named objects are never removed by this operation.
        pub fn cleanup_pending(&self)->Result<u64> {
            let mut removed=0u64;
            let outcome=visit(&self.directory,|name|{
                let text=name.to_str().map_err(|_|invalid("non-UTF8 package store entry"))?;
                if !pending_name(text) { return Ok(()); }
                let file=open(&self.directory,name).map_err(io_error)?;
                metadata(&file,false)?;
                unlink(&self.directory,name).map_err(io_error)?;
                removed+=1;
                #[cfg(test)]
                if *self.fault.lock().expect("fault mutex")==Some(PublicationFault::FailCleanupAfterFirstRemoval) { return Err(io_error(io::Error::other("injected failure after earlier pending unlink"))); }
                Ok(())
            });
            // A later enumeration/open failure must not skip durability of
            // earlier removals. Even a missing-file retry syncs the directory.
            self.directory.sync_all().map_err(|_|unknown("pending cleanup directory sync outcome uncertain"))?;
            match outcome {
                Err(_) if removed>0=>Err(unknown("pending cleanup partially removed files before failure")),
                Err(error)=>Err(error),
                Ok(())=>Ok(removed),
            }
        }
        /// Caller must prove zero READY references and zero active read holds
        /// while excluding lease admission/eviction with artifact_gate.
        pub fn remove(&self,value:&StoredPackage)->Result<()> {
            validate_stored(value)?;
            let name=basename(&value.identity.sha256)?;
            match open(&self.directory,&name) {
                Ok(file)=>{
                    if metadata(&file,true)?.len()!=value.identity.byte_len { return Err(mismatch("removed package length differs from receipt")); }
                    unlink(&self.directory,&name).map_err(io_error)?;
                }
                Err(error) if error.kind()==io::ErrorKind::NotFound=>{},
                Err(error)=>return Err(io_error(error)),
            }
            #[cfg(test)]
            if *self.fault.lock().expect("fault mutex")==Some(PublicationFault::BeforeRemovalSync) { return Err(unknown("injected failure before removal directory sync")); }
            // Also sync a missing retry after an earlier uncertain unlink.
            self.directory.sync_all().map_err(|_|unknown("package removal directory sync outcome uncertain"))
        }
    }
}
fn hex_digest(bytes:&[u8])->String {
    const HEX:&[u8;16]=b"0123456789abcdef";
    let mut output=String::with_capacity(bytes.len()*2);
    for byte in bytes { output.push(HEX[(byte>>4) as usize] as char); output.push(HEX[(byte&15) as usize] as char); }
    output
}

#[cfg(not(unix))]
impl PackageStore {
    pub fn new(_root:&Path)->Result<Self>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package storage is unavailable on this platform"))}
    pub fn materialize<R:Read,F:FnMut(&PackageObjectDescriptor)->Result<R>,C:FnMut()->Result<()>,P:FnMut(u64,u64)->Result<()>>(&self,_manifest:&PortableProjectManifest,_open:F,_limits:PackageCodecLimits,_check:C,_progress:P)->Result<StoredPackage>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package publication unavailable"))}
    pub fn open_verified<C:FnMut()->Result<()>,P:FnMut(u64)->Result<()>>(&self,_value:&StoredPackage,_check:C,_progress:P)->Result<File>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package storage unavailable"))}
    pub fn open_readonly(&self,_value:&StoredPackage)->Result<File>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package storage unavailable"))}
    pub fn allocated_bytes(&self)->Result<u64>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package storage unavailable"))}
    pub fn remove(&self,_value:&StoredPackage)->Result<()>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package storage unavailable"))}
    pub fn cleanup_pending(&self)->Result<u64>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure package storage unavailable"))}
}

#[cfg(all(test,unix))]
mod tests {
 use super::*;
 use pulsar_protocol::*;
 use std::io::{Cursor,Read};
    const EMPTY_BYTES: &[u8] = br#"{"tracks":[]}"#;
    const EMPTY_SHA: &str = "ac11c4570c1e4918245c0ca34c2b951091b867a5aef252556ae323d06df61cf8";

    fn fixture() -> PortableProjectManifest {
        let motion = ProgramDescriptor {
            artifact_id: ArtifactId::new(format!("motion.{EMPTY_SHA}")).unwrap(),
            sha256: EMPTY_SHA.into(), byte_len: 13,
            codec: MotionCodec::MotionProgramJsonV1, axes: vec![],
        };
        let mut manifest = PortableProjectManifest {
            format_version: 1, origin_project_id: ProjectId::new("p").unwrap(),
            captured_revision: RevisionId::new(0), captured_event_cursor: 0,
            completeness: PackageCompleteness::default(), counts: PackageRecordCounts::default(),
            head: PackageProjectHead { name: "P".into(), revision: RevisionId::new(0),
                history_cursor: 0, motion: motion.clone(), protected: vec![] },
            revisions: vec![PackageRevision { revision: RevisionId::new(0), actor: "actor".into(),
                kind: "create".into(), label: "".into(), motion: motion.clone(), protected: vec![] }],
            edit_states: vec![PackageEditState { position: 0, motion, protected: vec![], source_revision: None }],
            revision_lineage: vec![], candidates: vec![], authored_lineage: vec![],
            sources: vec![], legacy_cells: vec![], generated_origins: vec![], export_receipts: vec![],
            objects: vec![PackageObjectDescriptor { sha256: EMPTY_SHA.into(), byte_len: 13,
                roles: vec![PackageObjectRole::MotionProgram] }],
        };
        manifest.counts = manifest.record_counts().unwrap();
        manifest
    }


 fn identity(sha:&str,len:u64,alias:&str)->ArtifactIdentity {
  ArtifactIdentity{artifact_id:ArtifactId::new(alias).unwrap(),sha256:sha.into(),byte_len:len}
 }
 fn store()->(tempfile::TempDir,PackageStore){
  let root=tempfile::tempdir().unwrap();
  let store=PackageStore::new(&root.path().join("project-packages")).unwrap();
  (root,store)
 }
 fn publish(store:&PackageStore)->Result<StoredPackage>{
  store.materialize(&fixture(),|_|Ok(Cursor::new(EMPTY_BYTES)),PackageCodecLimits::default(),||Ok(()),|_,_|Ok(()))
 }
 #[test]
 fn content_holds_survive_alias_guard_release(){
  let registry=HoldRegistry::default();
  let first=registry.acquire(&[identity(EMPTY_SHA,13,"a"),identity(EMPTY_SHA,13,"alias")]).unwrap();
  let second=registry.acquire(&[identity(EMPTY_SHA,13,"b")]).unwrap();
  assert!(registry.contains(EMPTY_SHA).unwrap());
  drop(first);
  assert!(registry.contains(EMPTY_SHA).unwrap());
  drop(second);
  assert!(!registry.contains(EMPTY_SHA).unwrap());
 }
 #[test]
 fn published_package_matches_independent_golden_identity(){
  let (_root,store)=store();
  let value=publish(&store).unwrap();
  assert_eq!(value.identity.byte_len,1652);
  assert_eq!(value.identity.sha256,"1e9f93581dd9e32be46d2dc982f776b803eb2f83d560036ec2e82b083a6508a8");
  assert_eq!(value.identity.artifact_id.as_str(),format!("package.{}",value.identity.sha256));
  assert_eq!(value.manifest_sha256,"ce4f3273baf8b2bfb457e878ab78322b8c6cae3e9f2625f2c699ca0883fc4ac6");
  let mut bytes=vec![];
  store.open_verified(&value,||Ok(()),|_|Ok(())).unwrap().read_to_end(&mut bytes).unwrap();
  assert_eq!(&bytes[bytes.len()-13..],EMPTY_BYTES);
  assert_eq!(store.allocated_bytes().unwrap(),1652);
 }

 #[test]
 fn retention_extend_is_atomic_on_conflict_and_capacity(){
  let registry=HoldRegistry{max_entries:2,..HoldRegistry::default()};
  let a=identity(EMPTY_SHA,13,"a");
  let b=identity(&"b".repeat(64),7,"b");
  let c=identity(&"c".repeat(64),8,"c");
  let mut guard=registry.acquire(&[a.clone()]).unwrap();
  let bad=identity(EMPTY_SHA,99,"wrong-length");
  assert_eq!(guard.extend(&[b.clone(),bad]).unwrap_err().code,ErrorCode::DependencyMismatch);
  assert!(!registry.contains(&b.sha256).unwrap());
  guard.extend(&[a,b.clone()]).unwrap();
  assert!(registry.contains(&b.sha256).unwrap());
  assert_eq!(guard.extend(&[c.clone()]).unwrap_err().code,ErrorCode::ResourceExhausted);
  assert!(!registry.contains(&c.sha256).unwrap());
  drop(guard);
  assert!(!registry.contains(EMPTY_SHA).unwrap());
  assert!(!registry.contains(&b.sha256).unwrap());
 }
 #[test]
 fn independent_registry_guards_are_send_and_sync(){
  fn assert_send_sync<T:Send+Sync>(){}
  assert_send_sync::<HoldRegistry>();
  assert_send_sync::<PackageStore>();
  let registry=HoldRegistry::default();
  let held=registry.acquire(&[identity(EMPTY_SHA,13,"a")]).unwrap();
  let cloned=registry.clone();
  std::thread::spawn(move||{
   assert!(cloned.contains(EMPTY_SHA).unwrap());
   drop(held);
  }).join().unwrap();
  assert!(!registry.contains(EMPTY_SHA).unwrap());
 }
 #[test]
 fn duplicate_publication_and_repeated_accounting_are_stable(){
  use std::os::unix::fs::MetadataExt;
  let(root,store)=store();
  let first=publish(&store).unwrap();
  let path=root.path().join("project-packages").join(&first.identity.sha256);
  let inode=std::fs::metadata(&path).unwrap().ino();
  let second=publish(&store).unwrap();
  assert_eq!(first.identity,second.identity);
  assert_eq!(std::fs::metadata(path).unwrap().ino(),inode);
  for _ in 0..3 { assert_eq!(store.allocated_bytes().unwrap(),1652); }
 }
 #[test]
 fn admission_fails_before_source_open_or_staging(){
  let(root,store)=store();
  let error=store.materialize(&fixture(),|_|->Result<Cursor<&[u8]>>{panic!("must not open")},
   PackageCodecLimits{max_package_bytes:1651},||Ok(()),|_,_|Ok(())).unwrap_err();
  assert_eq!(error.code,ErrorCode::ResourceExhausted);
  assert_eq!(std::fs::read_dir(root.path().join("project-packages")).unwrap().count(),0);
 }
 #[test]
 fn callback_cancellation_preserves_error_and_removes_provisional_bytes(){
  let(root,store)=store();
  let error=store.materialize(&fixture(),|_|Ok(Cursor::new(EMPTY_BYTES)),PackageCodecLimits::default(),
   ||Ok(()),|_,bytes|if bytes>=52{Err(ProtocolError::new(ErrorCode::Forbidden,"revoked"))}else{Ok(())}).unwrap_err();
  assert_eq!(error.code,ErrorCode::Forbidden);
  assert_eq!(store.allocated_bytes().unwrap(),0);
  assert_eq!(std::fs::read_dir(root.path().join("project-packages")).unwrap().count(),0);
 }
 #[test]
 fn exact_object_corruption_and_length_fail_without_publication(){
  let(root,store)=store();
  for bytes in [b"{\"tracks\":[]} ".as_slice(),b"{\"tracks\":[1]".as_slice(),b"{".as_slice()] {
   assert!(store.materialize(&fixture(),|_|Ok(Cursor::new(bytes)),PackageCodecLimits::default(),||Ok(()),|_,_|Ok(())).is_err());
   assert_eq!(store.allocated_bytes().unwrap(),0);
   assert_eq!(std::fs::read_dir(root.path().join("project-packages")).unwrap().count(),0);
  }
 }
 #[test]
 fn progress_counts_header_metadata_and_verified_object_completion(){
  let(_root,store)=store();
  let mut progress=vec![];
  let value=store.materialize(&fixture(),|_|Ok(Cursor::new(EMPTY_BYTES)),PackageCodecLimits::default(),
   ||Ok(()),|objects,bytes|{progress.push((objects,bytes));Ok(())}).unwrap();
  assert_eq!(progress.first(),Some(&(0,0)));
  assert_eq!(progress.last(),Some(&(1,value.identity.byte_len)));
  assert!(progress.windows(2).all(|w|w[0].0<=w[1].0 && w[0].1<=w[1].1));
  assert!(progress.iter().any(|&(objects,bytes)|objects==0&&bytes>=52));
  let mut verified=vec![];
  store.open_verified(&value,||Ok(()),|bytes|{verified.push(bytes);Ok(())}).unwrap();
  assert_eq!(verified.first(),Some(&0));
  assert_eq!(verified.last(),Some(&1652));
  let error=store.open_verified(&value,||Ok(()),|bytes|if bytes>0{
    Err(ProtocolError::new(ErrorCode::Forbidden,"revoked"))
   }else{Ok(())}).unwrap_err();
  assert_eq!(error.code,ErrorCode::Forbidden);
 }
 #[test]
 fn publication_faults_distinguish_no_effect_from_unknown_outcome(){
  for fault in [PublicationFault::BeforePublish,PublicationFault::AfterPublish,PublicationFault::BeforeDirectorySync]{
   let(root,store)=store();
   *store.fault.lock().unwrap()=Some(fault);
   let error=publish(&store).unwrap_err();
   if fault==PublicationFault::BeforePublish {
    assert_eq!(error.code,ErrorCode::Unavailable);
    assert_eq!(store.allocated_bytes().unwrap(),0);
   }else{
    assert_eq!(error.code,ErrorCode::UnknownOutcome);
    assert_eq!(store.allocated_bytes().unwrap(),1652,"complete orphan is still charged");
   }
   *store.fault.lock().unwrap()=None;
   let value=publish(&store).unwrap();
   assert_eq!(store.allocated_bytes().unwrap(),value.identity.byte_len);
   assert_eq!(std::fs::read_dir(root.path().join("project-packages")).unwrap().count(),1);
  }
 }
 #[test]
 fn corrupt_existing_object_is_not_overwritten_or_trusted(){
  use std::os::unix::fs::PermissionsExt;
  let(root,store)=store();
  let value=publish(&store).unwrap();
  let path=root.path().join("project-packages").join(&value.identity.sha256);
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).unwrap();
  let mut bytes=std::fs::read(&path).unwrap();
  *bytes.last_mut().unwrap()=b'!';
  std::fs::write(&path,&bytes).unwrap();
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o400)).unwrap();
  assert_eq!(store.open_verified(&value,||Ok(()),|_|Ok(())).unwrap_err().code,ErrorCode::DependencyMismatch);
  assert_eq!(publish(&store).unwrap_err().code,ErrorCode::DependencyMismatch);
  assert_eq!(std::fs::read(path).unwrap(),bytes);
 }
 #[test]
 fn path_symlinks_nonregular_files_and_descriptor_aliases_fail_closed(){
  use std::os::unix::fs::{symlink,PermissionsExt};
  let(root,store)=store();
  let value=publish(&store).unwrap();
  let alias=root.path().join("alias");
  symlink(root.path().join("project-packages"),&alias).unwrap();
  assert!(PackageStore::new(&alias).is_err());
  let mut bad=value.clone();
  bad.identity.sha256="../escape".into();
  assert!(store.open_readonly(&bad).is_err());
  let path=root.path().join("project-packages").join(&value.identity.sha256);
  std::fs::remove_file(&path).unwrap();
  symlink(root.path().join("outside"),&path).unwrap();
  assert!(store.open_readonly(&value).is_err());
  assert!(store.remove(&value).is_err());
  std::fs::remove_file(&path).unwrap();
  std::fs::create_dir(&path).unwrap();
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o400)).unwrap();
  assert!(store.open_readonly(&value).is_err());
 }
 #[test]
 fn anchored_directory_survives_path_replacement(){
  use std::os::unix::fs::symlink;
  let(root,store)=store();
  let original=root.path().join("project-packages");
  let moved=root.path().join("retained");
  let outside=root.path().join("outside");
  std::fs::rename(&original,&moved).unwrap();
  std::fs::create_dir(&outside).unwrap();
  symlink(&outside,&original).unwrap();
  let value=publish(&store).unwrap();
  assert!(moved.join(&value.identity.sha256).is_file());
  assert_eq!(std::fs::read_dir(outside).unwrap().count(),0);
  assert_eq!(store.allocated_bytes().unwrap(),1652);
 }
 #[test]
 fn startup_cleanup_only_removes_private_pending_and_keeps_orphans(){
  use std::os::unix::fs::PermissionsExt;
  let(root,store)=store();
  let value=publish(&store).unwrap();
  let pending=root.path().join("project-packages").join(format!(".pending-{}",uuid::Uuid::new_v4()));
  std::fs::write(&pending,b"crash").unwrap();
  std::fs::set_permissions(&pending,std::fs::Permissions::from_mode(0o600)).unwrap();
  assert_eq!(store.allocated_bytes().unwrap(),1657,"pending bytes remain conservatively charged");
  assert_eq!(store.cleanup_pending().unwrap(),1);
  assert_eq!(store.cleanup_pending().unwrap(),0);
  store.open_verified(&value,||Ok(()),|_|Ok(())).unwrap();
  assert_eq!(store.allocated_bytes().unwrap(),1652);
 }
 #[test]
 fn explicit_disposal_is_idempotent_and_uncertain_sync_is_recoverable(){
  let(_root,store)=store();
  let value=publish(&store).unwrap();
  *store.fault.lock().unwrap()=Some(PublicationFault::BeforeRemovalSync);
  assert_eq!(store.remove(&value).unwrap_err().code,ErrorCode::UnknownOutcome);
  *store.fault.lock().unwrap()=None;
  store.remove(&value).unwrap();
  store.remove(&value).unwrap();
  assert_eq!(store.allocated_bytes().unwrap(),0);
 }


 #[test]
 fn failed_staging_unlink_remains_charged_after_reservation_release(){
  let(root,store)=store();
  *store.fault.lock().unwrap()=Some(PublicationFault::FailPendingUnlink);
  let error=store.materialize(&fixture(),|_|Ok(Cursor::new(EMPTY_BYTES)),PackageCodecLimits::default(),
   ||Ok(()),|_,bytes|if bytes>=52{Err(ProtocolError::new(ErrorCode::Forbidden,"revoked"))}else{Ok(())}).unwrap_err();
  assert_eq!(error.code,ErrorCode::Forbidden);
  let files=std::fs::read_dir(root.path().join("project-packages")).unwrap().collect::<std::io::Result<Vec<_>>>().unwrap();
  assert_eq!(files.len(),1,"fault leaves the cancelled operation's staging file");
  let remaining=files[0].metadata().unwrap().len();
  assert_eq!(remaining,52);
  assert_eq!(store.allocated_bytes().unwrap(),remaining,"reservation is gone: surviving staging must remain charged");
 }


 #[test]
 fn partial_pending_cleanup_failure_reports_uncertain_effect(){
  use std::os::unix::fs::PermissionsExt;
  let(root,store)=store();
  let pending=root.path().join("project-packages").join(format!(".pending-{}",uuid::Uuid::new_v4()));
  std::fs::write(&pending,b"partial cleanup").unwrap();
  std::fs::set_permissions(&pending,std::fs::Permissions::from_mode(0o600)).unwrap();
  *store.fault.lock().unwrap()=Some(PublicationFault::FailCleanupAfterFirstRemoval);
  let error=store.cleanup_pending().unwrap_err();
  assert!(!pending.exists(),"earlier unlink really occurred");
  assert_eq!(error.code,ErrorCode::UnknownOutcome,"a later cleanup error must not imply no removal effect");
  *store.fault.lock().unwrap()=None;
  assert_eq!(store.cleanup_pending().unwrap(),0,"retry syncs even after earlier unlink");
 }

}

