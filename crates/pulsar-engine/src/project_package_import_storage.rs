//! Provisional portable-project import storage, never imported authority.
//!
//! Original uploaded bytes remain a separate durable recovery object. Objects
//! first live in private, engine-named staging. Full container/object integrity,
//! canonical motion, bounded origin graphs, and reconstructed ancestor hashes
//! must pass before a publication capability is constructed. Native receipt
//! semantic interpretation remains the engine's separate pre-commit gate.
//!
//! The caller owns admission, grants, operation journals, cancellation, and the
//! atomic fresh-project SQLite transaction. prepare_publication is called under
//! artifact_gate; its content holds survive publication and the SQL commit.
//! No imported path chooses any file. Publication is no-clobber and verifies
//! complete existing CAS bytes even on a hit. Failed publication may leave
//! durable, charged orphans, including the original recovery container.
//!
//! Staging Drop cleanup is best effort, not a successful-disposal claim. Every
//! regular staging/CAS file remains charged, including failed cleanup and
//! hardlinks counted conservatively in each namespace. Cleanup, hashing and
//! capability destruction run outside authority/database locks. I/O checks are
//! cooperative, not guarantees that a blocking filesystem operation terminates.

use crate::motion_artifacts::{ProgramStore,MAX_PROGRAM_BYTES};
use crate::project_package_format::{read_package,write_package,PackageCodecLimits,PACKAGE_HEADER_BYTES};
use crate::project_package_storage::{ArtifactHolds,HoldRegistry};
use pulsar_protocol::*;
use sha2::{Digest,Sha256};
use std::cell::{Cell,RefCell};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self,Read,Seek,SeekFrom,Write};
use std::path::Path;
use std::sync::{Arc,Mutex};
type Result<T> = std::result::Result<T,ProtocolError>;
const CHUNK_BYTES:usize=256*1024;
const BUFFER_BYTES:usize=64*1024;
const ORIGIN_NODES:usize=64;
const ORIGIN_BYTES:u64=256*1024*1024;
const ORIGIN_ENTRIES:usize=500_000;

fn invalid(message:&str)->ProtocolError{ProtocolError::invalid(message)}
fn exhausted(message:&str)->ProtocolError{ProtocolError::new(ErrorCode::ResourceExhausted,message)}
fn mismatch(message:&str)->ProtocolError{ProtocolError::new(ErrorCode::DependencyMismatch,message)}
fn unknown(message:&str)->ProtocolError{ProtocolError::new(ErrorCode::UnknownOutcome,message)}
fn io_error(error:io::Error)->ProtocolError{
    #[cfg(unix)]
    if matches!(error.raw_os_error(),Some(libc::ENOSPC)|Some(libc::EDQUOT)){return exhausted("import storage unavailable");}
    ProtocolError::new(ErrorCode::Unavailable,format!("import storage I/O: {error}"))
}
fn valid_sha(value:&str)->bool{
    value.len()==64&&value.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))
}
fn identity(prefix:&str,sha:&str,byte_len:u64)->Result<ArtifactIdentity>{
    if !valid_sha(sha){return Err(invalid("invalid import content digest"));}
    Ok(ArtifactIdentity{artifact_id:ArtifactId::new(format!("{prefix}.{sha}")).map_err(|_|invalid("invalid content ID"))?,sha256:sha.into(),byte_len})
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum ImportStoragePhase{Extracting,Validating,Publishing}
#[derive(Clone,Copy,Debug)]
pub struct ImportStorageProgress{
    pub phase:ImportStoragePhase,pub completed_bytes:u64,pub completed_objects:u64,
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum ImportNamespace{Motion,Source,Evidence,OriginalContainer}
#[derive(Clone,Debug)]
pub struct ImportedObjectPublication{
    pub identity:ArtifactIdentity,pub namespace:ImportNamespace,pub created:bool,
}
pub struct ImportStore{
    #[cfg(unix)]
    inner:Arc<unix::StoreInner>,
    holds:HoldRegistry,
}
pub struct ProvisionalImportUpload{
    #[cfg(unix)]
    stage:Arc<unix::Staging>,
    #[cfg(unix)]
    file:File,
    #[cfg(unix)]
    original:String,
    operation_id:PackageImportOperationId,
    declaration:PackageUploadDeclaration,
    limits:PackageCodecLimits,
    received:u64,
    poisoned:bool,
}
pub struct SealedImportContainer{
    upload:ProvisionalImportUpload,
}
pub struct VerifiedImportBytes{
    sealed:SealedImportContainer,
    manifest:PortableProjectManifest,
    container:PackageArtifactDescriptor,
    origins:BTreeMap<String,PortableProjectManifest>,
    objects:BTreeMap<String,String>,
    motions:BTreeMap<String,ProgramDescriptor>,
    manifest_name:String,
    manifest_object:ArtifactIdentity,
}
pub struct HeldImportBytes{
    verified:VerifiedImportBytes,
    holds:ArtifactHolds,
}
pub struct PublishedImportObjects{
    pub container:PackageArtifactDescriptor,
    pub manifest_object:ArtifactIdentity,
    pub publications:Vec<ImportedObjectPublication>,
    verified:VerifiedImportBytes,
    _holds:ArtifactHolds,
}
impl ProvisionalImportUpload{
    pub fn received_bytes(&self)->u64{self.received}
    pub fn operation_id(&self)->&PackageImportOperationId{&self.operation_id}
    pub fn declaration(&self)->&PackageUploadDeclaration{&self.declaration}
}
impl VerifiedImportBytes{
    pub fn manifest(&self)->&PortableProjectManifest{&self.manifest}
    pub fn container(&self)->&PackageArtifactDescriptor{&self.container}
    pub fn origins(&self)->&BTreeMap<String,PortableProjectManifest>{&self.origins}
    pub fn operation_id(&self)->&PackageImportOperationId{&self.sealed.upload.operation_id}
    pub fn motion(&self,sha:&str)->Option<&ProgramDescriptor>{self.motions.get(sha)}
    /// Extra conservative destination charge, independent of physical staging.
    /// Existing hits are still charged in this reservation estimate.
    pub fn publication_bytes(&self)->Result<u64>{
        let mut total=self.container.byte_len.checked_add(self.manifest_object.byte_len)
            .ok_or_else(||exhausted("publication accounting overflow"))?;
        for object in &self.manifest.objects{
            for _ in destinations(object){
                total=total.checked_add(object.byte_len).ok_or_else(||exhausted("publication accounting overflow"))?;
            }
        }
        Ok(total)
    }
}
impl PublishedImportObjects{
    pub fn manifest(&self)->&PortableProjectManifest{&self.verified.manifest}
    pub fn origins(&self)->&BTreeMap<String,PortableProjectManifest>{&self.verified.origins}
    pub fn motion(&self,sha:&str)->Option<&ProgramDescriptor>{self.verified.motions.get(sha)}
    pub fn operation_id(&self)->&PackageImportOperationId{self.verified.operation_id()}
}
fn destinations(object:&PackageObjectDescriptor)->Vec<ImportNamespace>{
    let mut output=vec![];
    if object.roles.contains(&PackageObjectRole::MotionProgram){output.push(ImportNamespace::Motion);}
    if object.roles.contains(&PackageObjectRole::SourceSnapshot){output.push(ImportNamespace::Source);}
    if object.roles.iter().any(|r|!matches!(r,PackageObjectRole::MotionProgram|PackageObjectRole::SourceSnapshot)){
        output.push(ImportNamespace::Evidence);
    }
    output
}
fn metadata_entries(manifest:&PortableProjectManifest)->Result<usize>{
    let mut total=1usize;
    for length in [manifest.revisions.len(),manifest.edit_states.len(),manifest.revision_lineage.len(),
        manifest.candidates.len(),manifest.authored_lineage.len(),manifest.sources.len(),
        manifest.legacy_cells.len(),manifest.generated_origins.len(),manifest.export_receipts.len(),
        manifest.objects.len(),manifest.imported_origins.len()]{
        total=total.checked_add(length).ok_or_else(||exhausted("origin entry count overflow"))?;
    }
    for origin in &manifest.imported_origins{
        for length in [origin.objects.len(),origin.revisions.len(),origin.candidates.len(),origin.sources.len(),origin.actors.len()]{
            total=total.checked_add(length).ok_or_else(||exhausted("origin map count overflow"))?;
        }
    }
    Ok(total)
}
fn motions<'a>(manifest:&'a PortableProjectManifest)->impl Iterator<Item=&'a ProgramDescriptor>{
    std::iter::once(&manifest.head.motion)
        .chain(manifest.revisions.iter().map(|r|&r.motion))
        .chain(manifest.edit_states.iter().map(|r|&r.motion))
        .chain(manifest.candidates.iter().map(|r|&r.motion))
}
struct HashSink{hash:Sha256,bytes:u64}
impl Write for HashSink{
    fn write(&mut self,bytes:&[u8])->io::Result<usize>{
        self.bytes=self.bytes.checked_add(bytes.len() as u64).ok_or_else(||io::Error::other("hash length overflow"))?;
        self.hash.update(bytes);Ok(bytes.len())
    }
    fn flush(&mut self)->io::Result<()>{Ok(())}
}
struct CheckedInput<'a,R,C,P>{
    inner:R,check:&'a RefCell<C>,progress:&'a RefCell<P>,bytes:&'a Cell<u64>,objects:&'a Cell<u64>,
    cause:&'a RefCell<Option<ProtocolError>>,phase:ImportStoragePhase,
}
impl<R:Read,C:FnMut()->Result<()>,P:FnMut(ImportStorageProgress)->Result<()>> Read for CheckedInput<'_,R,C,P>{
    fn read(&mut self,output:&mut[u8])->io::Result<usize>{
        if let Err(error)=(self.check.borrow_mut())(){*self.cause.borrow_mut()=Some(error);return Err(io::Error::other("import cancelled"));}
        let length=output.len().min(BUFFER_BYTES);
        let count=self.inner.read(&mut output[..length])?;
        if count>length{return Err(io::Error::other("reader exceeded bounded buffer"));}
        self.bytes.set(self.bytes.get().checked_add(count as u64).ok_or_else(||io::Error::other("import progress overflow"))?);
        if let Err(error)=(self.progress.borrow_mut())(ImportStorageProgress{phase:self.phase,completed_bytes:self.bytes.get(),completed_objects:self.objects.get()}){
            *self.cause.borrow_mut()=Some(error);return Err(io::Error::other("import progress cancelled"));
        }
        Ok(count)
    }
}

#[cfg(unix)]
mod unix{
    use super::*;
    use crate::project_package_storage::unix as anchored;
    use std::ffi::CString;
    use std::os::fd::{AsRawFd,FromRawFd};
    use std::os::unix::fs::{MetadataExt,PermissionsExt};

    pub(super) struct StoreInner{
        staging:Arc<File>,motion:File,sources:File,evidence:File,containers:File,
    }
    pub(super) struct Staging{
        store:Arc<StoreInner>,names:Mutex<Vec<String>>,
    }
    impl Staging{
        fn create(&self,kind:&str)->Result<(String,File)>{
            let name=format!(".import-{kind}-{}",uuid::Uuid::new_v4());
            let c=CString::new(name.clone()).expect("internal UUID basename");
            // SAFETY: generated basename and anchored private directory FD.
            let fd=unsafe{libc::openat(self.store.staging.as_raw_fd(),c.as_ptr(),
                libc::O_RDWR|libc::O_CREAT|libc::O_EXCL|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600)};
            if fd<0{return Err(io_error(io::Error::last_os_error()));}
            let file=unsafe{File::from_raw_fd(fd)};
            let mut names=self.names.lock().map_err(|_|invalid("staging registry poisoned"))?;
            if names.len()>MAX_PROJECT_PACKAGE_ENTRIES+ORIGIN_NODES+4{return Err(exhausted("staging entry budget exceeded"));}
            names.push(name.clone());Ok((name,file))
        }
        fn open(&self,name:&str)->Result<File>{
            let name=CString::new(name).map_err(|_|invalid("invalid engine staging name"))?;
            let file=anchored::open(&self.store.staging,&name).map_err(io_error)?;
            anchored::metadata(&file,true)?;Ok(file)
        }
    }
    impl Drop for Staging{
        fn drop(&mut self){
            // Only generated names are eligible; failed cleanup remains charged.
            let Ok(names)=self.names.get_mut() else{return;};
            for name in names{
                if let Ok(name)=CString::new(name.as_str()){let _=anchored::unlink(&self.store.staging,&name);}
            }
        }
    }
    struct ObjectSink<'a>{
        file:File,objects:&'a Cell<u64>,completed:bool,
    }
    impl Write for ObjectSink<'_>{
        fn write(&mut self,bytes:&[u8])->io::Result<usize>{self.file.write(bytes)}
        fn flush(&mut self)->io::Result<()>{
            self.file.flush()?;
            self.file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
            self.file.sync_all()?;
            if !self.completed{self.objects.set(self.objects.get()+1);self.completed=true;}
            Ok(())
        }
    }
    fn checked_identity(file:&mut File,sha:&str,length:u64,mut check:impl FnMut()->Result<()>,mut progress:impl FnMut(u64)->Result<()>)->Result<()>{
        if !valid_sha(sha)||length>DEFAULT_MAX_PROJECT_PACKAGE_BYTES{return Err(invalid("invalid bounded identity"));}
        if file.metadata().map_err(io_error)?.len()!=length{return Err(mismatch("import file length mismatch"));}
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let mut hash=Sha256::new();let mut actual=0u64;let mut buffer=[0u8;BUFFER_BYTES];
        progress(0)?;
        loop{
            check()?;
            let n=match file.read(&mut buffer){Ok(n)=>n,Err(e)if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(io_error(e))};
            if n==0{break;}
            actual=actual.checked_add(n as u64).ok_or_else(||exhausted("import length overflow"))?;
            if actual>length{return Err(mismatch("import file grew beyond its identity"));}
            hash.update(&buffer[..n]);progress(actual)?;
        }
        if actual!=length||format!("{:x}",hash.finalize())!=sha{return Err(mismatch("import content checksum mismatch"));}
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;Ok(())
    }
    fn bounded_bytes(mut file:File,object:&PackageObjectDescriptor,limit:u64,check:&mut impl FnMut()->Result<()>)->Result<Vec<u8>>{
        if object.byte_len>limit{return Err(exhausted("structured import object exceeds byte budget"));}
        checked_identity(&mut file,&object.sha256,object.byte_len,&mut *check,|_|Ok(()))?;
        let mut bytes=Vec::new();
        bytes.try_reserve_exact(object.byte_len as usize).map_err(|_|exhausted("import structured allocation unavailable"))?;
        (&mut file).take(object.byte_len+1).read_to_end(&mut bytes).map_err(io_error)?;
        if bytes.len() as u64!=object.byte_len||format!("{:x}",Sha256::digest(&bytes))!=object.sha256{
            return Err(mismatch("structured import object changed"));
        }
        Ok(bytes)
    }
    impl ProvisionalImportUpload{
        /// Bounded immutable-prefix replay, without a per-chunk replay map.
        pub fn append(&mut self,offset:u64,bytes:&[u8])->Result<u64>{
            if self.poisoned{return Err(invalid("upload is terminal after a storage failure"));}
            if bytes.is_empty()||bytes.len()>CHUNK_BYTES{return Err(invalid("upload chunk exceeds bound"));}
            let end=offset.checked_add(bytes.len() as u64).ok_or_else(||exhausted("upload offset overflow"))?;
            if end>self.declaration.byte_len||offset>self.received{return Err(invalid("upload has a gap or exceeds declared length"));}
            if offset<self.received{
                if end>self.received{return Err(invalid("upload overlaps the committed prefix boundary"));}
                self.file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
                let mut buffer=[0u8;BUFFER_BYTES];
                let mut compared=0usize;
                while compared<bytes.len(){
                    let n=(bytes.len()-compared).min(buffer.len());
                    self.file.read_exact(&mut buffer[..n]).map_err(io_error)?;
                    if buffer[..n]!=bytes[compared..compared+n]{return Err(mismatch("upload replay changed bytes"));}
                    compared+=n;
                }
                return Ok(self.received);
            }
            self.file.seek(SeekFrom::Start(self.received)).map_err(io_error)?;
            match self.file.write_all(bytes){
                Ok(())=>{self.received=end;Ok(self.received)},
                Err(error)=>{self.poisoned=true;Err(io_error(error))}
            }
        }
    }
    impl ImportStore{
        pub fn new(root:&Path,holds:HoldRegistry)->Result<Self>{
            let inner=StoreInner{
                staging:Arc::new(anchored::directory(&root.join("project-import-staging"))?),
                motion:anchored::directory(&root.join("motion-objects"))?,
                sources:anchored::directory(&root.join("snapshots"))?,
                evidence:anchored::directory(&root.join("imported-project-evidence"))?,
                containers:anchored::directory(&root.join("imported-project-containers"))?,
            };
            let dev=inner.staging.metadata().map_err(io_error)?.dev();
            for directory in [&inner.motion,&inner.sources,&inner.evidence,&inner.containers]{
                if directory.metadata().map_err(io_error)?.dev()!=dev{return Err(ProtocolError::new(ErrorCode::Unsupported,"import namespaces must share an admitted filesystem"));}
            }
            Ok(Self{inner:Arc::new(inner),holds})
        }
        pub fn begin_upload(&self,operation_id:PackageImportOperationId,declaration:PackageUploadDeclaration,limits:PackageCodecLimits)->Result<ProvisionalImportUpload>{
            declaration.validate()?;
            if declaration.byte_len>limits.max_package_bytes||declaration.byte_len<=PACKAGE_HEADER_BYTES as u64{
                return Err(exhausted("upload exceeds admitted package bounds"));
            }
            let stage=Arc::new(Staging{store:self.inner.clone(),names:Mutex::new(vec![])});
            let(original,file)=stage.create("container")?;
            Ok(ProvisionalImportUpload{stage,file,original,operation_id,declaration,limits,received:0,poisoned:false})
        }
        fn require_store(&self,stage:&Staging)->Result<()>{
            if !Arc::ptr_eq(&self.inner,&stage.store){return Err(invalid("import capability belongs to another store"));}
            Ok(())
        }
        pub fn seal_upload<C:FnMut()->Result<()>,P:FnMut(u64)->Result<()>>(&self,mut upload:ProvisionalImportUpload,mut check:C,progress:P)->Result<SealedImportContainer>{
            self.require_store(&upload.stage)?;
            if upload.poisoned||upload.received!=upload.declaration.byte_len{return Err(invalid("upload is incomplete or failed"));}
            checked_identity(&mut upload.file,&upload.declaration.sha256,upload.declaration.byte_len,&mut check,progress)?;
            check()?;
            upload.file.set_permissions(std::fs::Permissions::from_mode(0o400)).map_err(io_error)?;
            upload.file.sync_all().map_err(io_error)?;
            Ok(SealedImportContainer{upload})
        }
        pub fn verify_and_stage<C:FnMut()->Result<()>,P:FnMut(ImportStorageProgress)->Result<()>>(&self,mut sealed:SealedImportContainer,check:C,progress:P)->Result<VerifiedImportBytes>{
            self.require_store(&sealed.upload.stage)?;
            let check=RefCell::new(check);let progress=RefCell::new(progress);
            let bytes=Cell::new(0u64);let completed=Cell::new(0u64);let cause=RefCell::new(None);
            let mut objects=BTreeMap::new();
            sealed.upload.file.seek(SeekFrom::Start(0)).map_err(io_error)?;
            let decoded={
                let mut input=CheckedInput{inner:&mut sealed.upload.file,check:&check,progress:&progress,bytes:&bytes,objects:&completed,cause:&cause,phase:ImportStoragePhase::Extracting};
                read_package(&mut input,|descriptor|{
                    let(name,file)=sealed.upload.stage.create("object")?;
                    if objects.insert(descriptor.sha256.clone(),name).is_some(){return Err(invalid("duplicate staged content digest"));}
                    Ok(ObjectSink{file,objects:&completed,completed:false})
                },sealed.upload.limits).map_err(|e|cause.borrow_mut().take().unwrap_or(e))?
            };
            if decoded.digest.sha256!=sealed.upload.declaration.sha256||decoded.digest.byte_len!=sealed.upload.declaration.byte_len
                ||decoded.manifest.format_version!=sealed.upload.declaration.format_version{
                return Err(mismatch("uploaded declaration differs from decoded container"));
            }
            let canonical=decoded.manifest.canonical_bytes()?;
            let manifest_object=identity("imported-manifest",&format!("{:x}",Sha256::digest(&canonical)),canonical.len() as u64)?;
            let container=PackageArtifactDescriptor{
                format_version:decoded.manifest.format_version,sha256:decoded.digest.sha256,byte_len:decoded.digest.byte_len,
                manifest_sha256:manifest_object.sha256.clone(),project_id:decoded.manifest.origin_project_id.clone(),
                captured_revision:decoded.manifest.captured_revision,captured_event_cursor:decoded.manifest.captured_event_cursor,
                object_count:decoded.manifest.objects.len().try_into().map_err(|_|exhausted("object count overflow"))?,
                verification:PackageReadyVerification::AllDeclaredObjectsVerified,
            };
            let mut origins=BTreeMap::new();
            let mut origin_bytes=0u64;let mut entries=metadata_entries(&decoded.manifest)?;
            for object in &decoded.manifest.objects{
                if !object.roles.contains(&PackageObjectRole::ImportedManifest){continue;}
                if origins.len()==ORIGIN_NODES{return Err(exhausted("origin manifest node budget exceeded"));}
                origin_bytes=origin_bytes.checked_add(object.byte_len).ok_or_else(||exhausted("origin byte overflow"))?;
                if origin_bytes>ORIGIN_BYTES{return Err(exhausted("aggregate origin manifest byte budget exceeded"));}
                let data=bounded_bytes(sealed.upload.stage.open(&objects[&object.sha256])?,object,MAX_PROJECT_PACKAGE_MANIFEST_BYTES as u64,&mut *check.borrow_mut())?;
                let original:PortableProjectManifest=serde_json::from_slice(&data).map_err(|_|invalid("invalid origin manifest"))?;
                if original.canonical_bytes()?!=data{return Err(invalid("origin manifest is not canonical"));}
                entries=entries.checked_add(metadata_entries(&original)?).ok_or_else(||exhausted("origin entry overflow"))?;
                if entries>ORIGIN_ENTRIES{return Err(exhausted("aggregate origin entry budget exceeded"));}
                origins.insert(object.sha256.clone(),original);
            }
            crate::project_package_import_plan::validate_import_origins(&decoded.manifest,&container,&origins)?;
            // Reconstruct each ancestor flat container from its exact canonical
            // manifest and exact staged object bytes, never nested container blobs.
            let mut reconstructed=BTreeMap::new();
            for manifest in std::iter::once(&decoded.manifest).chain(origins.values()){
                for origin in &manifest.imported_origins{
                    if !reconstructed.contains_key(&origin.manifest.sha256){
                        let original=origins.get(&origin.manifest.sha256).ok_or_else(||mismatch("missing original manifest"))?;
                        let mut sink=HashSink{hash:Sha256::new(),bytes:0};
                        let digest=write_package(original,&mut sink,|object|{
                            let name=objects.get(&object.sha256).ok_or_else(||mismatch("missing original payload"))?;
                            let file=sealed.upload.stage.open(name)?;
                            Ok(CheckedInput{inner:file,check:&check,progress:&progress,bytes:&bytes,objects:&completed,cause:&cause,phase:ImportStoragePhase::Validating})
                        },PackageCodecLimits::default()).map_err(|e|cause.borrow_mut().take().unwrap_or(e))?;
                        reconstructed.insert(origin.manifest.sha256.clone(),digest);
                    }
                    let digest=&reconstructed[&origin.manifest.sha256];
                    // Check every edge, even when this namespace was already
                    // hashed: a second edge must not claim another container.
                    if digest.sha256!=origin.container.sha256||digest.byte_len!=origin.container.byte_len{
                        return Err(mismatch("ancestor flat-container identity failed reconstruction"));
                    }
                }
            }
            let mut expected=BTreeMap::<String,ProgramDescriptor>::new();
            for manifest in std::iter::once(&decoded.manifest).chain(origins.values()){
                for motion in motions(manifest){
                    if let Some(previous)=expected.insert(motion.sha256.clone(),motion.clone()){
                        if previous!=*motion{return Err(mismatch("motion metadata aliases disagree"));}
                    }
                }
            }
            let mut validated=BTreeMap::new();
            for object in &decoded.manifest.objects{
                if !object.roles.contains(&PackageObjectRole::MotionProgram){continue;}
                let data=bounded_bytes(sealed.upload.stage.open(&objects[&object.sha256])?,object,MAX_PROGRAM_BYTES,&mut *check.borrow_mut())?;
                let actual=ProgramStore::validate_canonical(&data)?;
                if actual.sha256!=object.sha256||actual.byte_len!=object.byte_len
                    ||expected.get(&object.sha256)!=Some(&actual){
                    return Err(mismatch("motion payload differs from its checked metadata"));
                }
                validated.insert(object.sha256.clone(),actual);
            }
            if validated.len()!=expected.len(){return Err(mismatch("not all motion references have validated payloads"));}
            (check.borrow_mut())()?;
            let(manifest_name,mut manifest_file)=sealed.upload.stage.create("manifest")?;
            manifest_file.write_all(&canonical).map_err(io_error)?;
            manifest_file.set_permissions(std::fs::Permissions::from_mode(0o400)).map_err(io_error)?;
            manifest_file.sync_all().map_err(io_error)?;
            (progress.borrow_mut())(ImportStorageProgress{phase:ImportStoragePhase::Validating,completed_bytes:bytes.get(),completed_objects:completed.get()})?;
            Ok(VerifiedImportBytes{sealed,manifest:decoded.manifest,container,origins,objects,motions:validated,manifest_name,manifest_object})
        }
        pub fn open_staged_object(&self,verified:&VerifiedImportBytes,object:&PackageObjectDescriptor)->Result<File>{
            self.require_store(&verified.sealed.upload.stage)?;
            if verified.manifest.objects.iter().find(|o|o.sha256==object.sha256)!=Some(object){
                return Err(invalid("object is outside verified import catalog"));
            }
            let name=verified.objects.get(&object.sha256).ok_or_else(||mismatch("missing staged object"))?;
            let mut file=verified.sealed.upload.stage.open(name)?;
            checked_identity(&mut file,&object.sha256,object.byte_len,||Ok(()),|_|Ok(()))?;
            Ok(file)
        }
        /// MUST run under artifact_gate after additional publication admission.
        /// No filesystem I/O is performed while acquiring these content holds.
        pub fn prepare_publication(&self,verified:VerifiedImportBytes)->Result<HeldImportBytes>{
            self.require_store(&verified.sealed.upload.stage)?;
            let mut identities=verified.manifest.objects.iter().map(|o|identity("imported",&o.sha256,o.byte_len)).collect::<Result<Vec<_>>>()?;
            identities.push(identity("package",&verified.container.sha256,verified.container.byte_len)?);
            identities.push(verified.manifest_object.clone());
            let holds=self.holds.acquire(&identities)?;
            Ok(HeldImportBytes{verified,holds})
        }
        pub fn publish_objects<C:FnMut()->Result<()>,P:FnMut(ImportStorageProgress)->Result<()>>(&self,held:HeldImportBytes,mut check:C,mut progress:P)->Result<PublishedImportObjects>{
            self.require_store(&held.verified.sealed.upload.stage)?;
            let mut publications=vec![];let mut complete=0u64;let mut bytes=0u64;
            let result=(||{
                let value=&held.verified;
                let container=identity("package",&value.container.sha256,value.container.byte_len)?;
                publications.push(self.publish_one(&value.sealed.upload.stage,&value.sealed.upload.original,&container,ImportNamespace::OriginalContainer,&mut check)?);
                complete+=1;bytes=bytes.checked_add(container.byte_len).ok_or_else(||exhausted("publication progress overflow"))?;
                progress(ImportStorageProgress{phase:ImportStoragePhase::Publishing,completed_bytes:bytes,completed_objects:complete})?;
                publications.push(self.publish_one(&value.sealed.upload.stage,&value.manifest_name,&value.manifest_object,ImportNamespace::Evidence,&mut check)?);
                complete+=1;bytes=bytes.checked_add(value.manifest_object.byte_len).ok_or_else(||exhausted("publication progress overflow"))?;
                for object in &value.manifest.objects{
                    let name=&value.objects[&object.sha256];
                    for destination in destinations(object){
                        let id=identity("imported",&object.sha256,object.byte_len)?;
                        publications.push(self.publish_one(&value.sealed.upload.stage,name,&id,destination,&mut check)?);
                        complete+=1;bytes=bytes.checked_add(object.byte_len).ok_or_else(||exhausted("publication progress overflow"))?;
                        progress(ImportStorageProgress{phase:ImportStoragePhase::Publishing,completed_bytes:bytes,completed_objects:complete})?;
                    }
                }
                Ok(())
            })();
            if let Err(error)=result{
                if publications.iter().any(|p|p.created){return Err(unknown("import publication may have left durable recovery objects; reconcile before cleanup"));}
                return Err(error);
            }
            Ok(PublishedImportObjects{container:held.verified.container.clone(),manifest_object:held.verified.manifest_object.clone(),publications,verified:held.verified,_holds:held.holds})
        }
        fn destination(&self,namespace:ImportNamespace)->&File{
            match namespace{ImportNamespace::Motion=>&self.inner.motion,ImportNamespace::Source=>&self.inner.sources,
                ImportNamespace::Evidence=>&self.inner.evidence,ImportNamespace::OriginalContainer=>&self.inner.containers}
        }
        fn publish_one(&self,stage:&Staging,name:&str,value:&ArtifactIdentity,namespace:ImportNamespace,check:&mut impl FnMut()->Result<()>)->Result<ImportedObjectPublication>{
            let mut staged=stage.open(name)?;
            checked_identity(&mut staged,&value.sha256,value.byte_len,&mut *check,|_|Ok(()))?;
            staged.sync_all().map_err(io_error)?;check()?;
            let name=CString::new(name).map_err(|_|invalid("invalid engine staging name"))?;
            let digest=anchored::basename(&value.sha256)?;
            let destination=self.destination(namespace);
            // SAFETY: both directory handles are anchored; both names are generated.
            let result=unsafe{libc::linkat(self.inner.staging.as_raw_fd(),name.as_ptr(),destination.as_raw_fd(),digest.as_ptr(),0)};
            let created=if result==0{true}else{
                let error=io::Error::last_os_error();
                if error.kind()!=io::ErrorKind::AlreadyExists{return Err(io_error(error));}false
            };
            let mut existing=anchored::open(destination,&digest).map_err(|e|if created{unknown("published import reopen outcome uncertain")}else{io_error(e)})?;
            anchored::metadata(&existing,true).map_err(|e|if created{unknown("published import metadata outcome uncertain")}else{e})?;
            if let Err(error)=checked_identity(&mut existing,&value.sha256,value.byte_len,&mut *check,|_|Ok(())){
                return Err(if created{unknown("published import verification outcome uncertain")}else{error});
            }
            existing.sync_all().map_err(|_|unknown("import object file sync outcome uncertain"))?;
            destination.sync_all().map_err(|_|unknown("import object directory sync outcome uncertain"))?;
            Ok(ImportedObjectPublication{identity:value.clone(),namespace,created})
        }

        /// Engine-authored, inert identity-map receipt, never arbitrary paths.
        /// Caller admits bytes and holds its computed identity under artifact_gate.
        pub fn publish_evidence(&self,bytes:&[u8])->Result<ArtifactIdentity>{
            if bytes.len()>MAX_PROJECT_PACKAGE_MANIFEST_BYTES{return Err(exhausted("import identity-map receipt exceeds byte bound"));}
            let value=identity("imported",&format!("{:x}",Sha256::digest(bytes)),bytes.len() as u64)?;
            let stage=Staging{store:self.inner.clone(),names:Mutex::new(vec![])};
            let(name,mut file)=stage.create("receipt")?;
            for chunk in bytes.chunks(BUFFER_BYTES){file.write_all(chunk).map_err(io_error)?;}
            file.set_permissions(std::fs::Permissions::from_mode(0o400)).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            self.publish_one(&stage,&name,&value,ImportNamespace::Evidence,&mut ||Ok(()))?;
            Ok(value)
        }

        /// Includes every regular file in every import destination/staging
        /// namespace, including existing unrelated motions, sources and orphans.
        /// Hardlinks and active reservations may deliberately be double-counted.
        pub fn allocated_bytes(&self)->Result<u64>{
            let mut bytes=0u64;
            for directory in [&*self.inner.staging,&self.inner.motion,&self.inner.sources,&self.inner.evidence,&self.inner.containers]{
                anchored::visit(directory,|name|{
                    let file=anchored::open(directory,name).map_err(io_error)?;
                    let length=anchored::metadata(&file,false)?.len();
                    bytes=bytes.checked_add(length).ok_or_else(||exhausted("import allocation count overflow"))?;
                    Ok(())
                })?;
            }
            Ok(bytes)
        }
        /// Exclusive recovery only, after operation/lease reconciliation. Does
        /// not remove any completed source, motion, evidence or recovery CAS.
        pub fn cleanup_staging(&self)->Result<u64>{
            let mut removed=0u64;
            let outcome=anchored::visit(&self.inner.staging,|name|{
                let text=name.to_str().map_err(|_|invalid("unexpected import staging basename"))?;
                let suffix=text.strip_prefix(".import-").ok_or_else(||invalid("unexpected staging entry"))?;
                let (_,uuid)=suffix.split_once('-').ok_or_else(||invalid("invalid staging entry"))?;
                if uuid::Uuid::parse_str(uuid).map_err(|_|invalid("invalid staging UUID"))?.to_string()!=uuid{
                    return Err(invalid("noncanonical staging UUID"));
                }
                let file=anchored::open(&self.inner.staging,name).map_err(io_error)?;
                anchored::metadata(&file,false)?;
                anchored::unlink(&self.inner.staging,name).map_err(io_error)?;
                removed+=1;Ok(())
            });
            self.inner.staging.sync_all().map_err(|_|unknown("import staging cleanup durability uncertain"))?;
            match outcome{Err(_)if removed>0=>Err(unknown("import staging cleanup partially changed files")),Err(e)=>Err(e),Ok(())=>Ok(removed)}
        }
    }
}
#[cfg(not(unix))]
impl ImportStore{
    pub fn new(_root:&Path,_holds:HoldRegistry)->Result<Self>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable on this platform"))}
}

#[cfg(not(unix))]
impl ProvisionalImportUpload{
    pub fn append(&mut self,_offset:u64,_bytes:&[u8])->Result<u64>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
}
#[cfg(not(unix))]
impl ImportStore{
    pub fn begin_upload(&self,_id:PackageImportOperationId,_declaration:PackageUploadDeclaration,_limits:PackageCodecLimits)->Result<ProvisionalImportUpload>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn seal_upload<C:FnMut()->Result<()>,P:FnMut(u64)->Result<()>>(&self,_upload:ProvisionalImportUpload,_check:C,_progress:P)->Result<SealedImportContainer>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn verify_and_stage<C:FnMut()->Result<()>,P:FnMut(ImportStorageProgress)->Result<()>>(&self,_sealed:SealedImportContainer,_check:C,_progress:P)->Result<VerifiedImportBytes>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn open_staged_object(&self,_verified:&VerifiedImportBytes,_object:&PackageObjectDescriptor)->Result<File>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn prepare_publication(&self,_verified:VerifiedImportBytes)->Result<HeldImportBytes>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn publish_objects<C:FnMut()->Result<()>,P:FnMut(ImportStorageProgress)->Result<()>>(&self,_held:HeldImportBytes,_check:C,_progress:P)->Result<PublishedImportObjects>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn publish_evidence(&self,_bytes:&[u8])->Result<ArtifactIdentity>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn allocated_bytes(&self)->Result<u64>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
    pub fn cleanup_staging(&self)->Result<u64>{Err(ProtocolError::new(ErrorCode::Unsupported,"secure import storage unavailable"))}
}

#[cfg(all(test,unix))]
mod tests {
 use super::*;
 use crate::project_package_format::{write_package,PackageCodecLimits};
 use crate::motion_artifacts::ProgramStore;
 use std::io::Cursor;
 use sha2::{Digest,Sha256};
    const EMPTY_BYTES: &[u8] = br#"{"tracks":[]}"#;
    const EMPTY_SHA: &str = "ac11c4570c1e4918245c0ca34c2b951091b867a5aef252556ae323d06df61cf8";

    fn fixture() -> PortableProjectManifest {
        let motion = ProgramDescriptor {
            artifact_id: ArtifactId::new(format!("motion.{EMPTY_SHA}")).unwrap(),
            sha256: EMPTY_SHA.into(), byte_len: 13,
            codec: MotionCodec::MotionProgramJsonV1, axes: vec![],
        };
        let mut manifest = PortableProjectManifest {
            imported_origins: vec![], format_version: 1, origin_project_id: ProjectId::new("p").unwrap(),
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


 fn encoded()->Vec<u8>{
  let mut bytes=vec![];
  write_package(&fixture(),&mut bytes,|_|Ok(Cursor::new(EMPTY_BYTES)),PackageCodecLimits::default()).unwrap();
  bytes
 }
 #[test]
 fn uploaded_exact_original_can_be_sealed_without_path_authority(){
  let root=tempfile::tempdir().unwrap();
  let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();
  let declaration=PackageUploadDeclaration{format_version:1,sha256:format!("{:x}",Sha256::digest(&bytes)),byte_len:bytes.len() as u64};
  let mut upload=store.begin_upload(PackageImportOperationId::new("local-import").unwrap(),declaration,PackageCodecLimits::default()).unwrap();
  assert_eq!(upload.append(0,&bytes).unwrap(),bytes.len() as u64);
  store.seal_upload(upload,||Ok(()),|_|Ok(())).unwrap();
 }
 #[test]
 fn pure_motion_validator_rejects_normalization_before_publication(){
  assert_eq!(ProgramStore::validate_canonical(EMPTY_BYTES).unwrap().sha256,EMPTY_SHA);
  assert_eq!(ProgramStore::validate_canonical(br#"{"tracks": []}"#).unwrap_err().code,ErrorCode::InvalidRequest);
 }

 fn declaration(bytes:&[u8])->PackageUploadDeclaration{
  PackageUploadDeclaration{format_version:1,sha256:format!("{:x}",Sha256::digest(bytes)),byte_len:bytes.len() as u64}
 }
 fn uploaded(store:&ImportStore,bytes:&[u8])->ProvisionalImportUpload{
  let mut upload=store.begin_upload(PackageImportOperationId::new("local-import").unwrap(),declaration(bytes),PackageCodecLimits::default()).unwrap();
  for(offset,chunk)in bytes.chunks(CHUNK_BYTES).enumerate(){upload.append((offset*CHUNK_BYTES) as u64,chunk).unwrap();}
  upload
 }
 fn verified(store:&ImportStore,bytes:&[u8])->VerifiedImportBytes{
  let sealed=store.seal_upload(uploaded(store,bytes),||Ok(()),|_|Ok(())).unwrap();
  store.verify_and_stage(sealed,||Ok(()),|_|Ok(())).unwrap()
 }
 fn publish(store:&ImportStore,bytes:&[u8])->PublishedImportObjects{
  let value=verified(store,bytes);
  let held=store.prepare_publication(value).unwrap();
  store.publish_objects(held,||Ok(()),|_|Ok(())).unwrap()
 }
 fn encoding_with_motion(data:&[u8])->Vec<u8>{
  let mut manifest=fixture();
  let sha=format!("{:x}",Sha256::digest(data));
  let mut motion=manifest.head.motion.clone();
  motion.sha256=sha.clone();motion.byte_len=data.len() as u64;
  motion.artifact_id=ArtifactId::new(format!("motion.{sha}")).unwrap();
  manifest.head.motion=motion.clone();manifest.revisions[0].motion=motion.clone();manifest.edit_states[0].motion=motion;
  manifest.objects[0].sha256=sha;manifest.objects[0].byte_len=data.len() as u64;
  let mut bytes=vec![];
  write_package(&manifest,&mut bytes,|_|Ok(Cursor::new(data)),PackageCodecLimits::default()).unwrap();
  bytes
 }
 #[test]
 fn full_import_retains_original_bytes_and_holds_until_reference_commit(){
  let root=tempfile::tempdir().unwrap();let holds=HoldRegistry::default();
  let store=ImportStore::new(root.path(),holds.clone()).unwrap();
  let bytes=encoded();let value=publish(&store,&bytes);
  assert_eq!(std::fs::read(root.path().join("imported-project-containers").join(&value.container.sha256)).unwrap(),bytes);
  assert_eq!(std::fs::read(root.path().join("motion-objects").join(EMPTY_SHA)).unwrap(),EMPTY_BYTES);
  assert_eq!(std::fs::read(root.path().join("imported-project-evidence").join(&value.manifest_object.sha256)).unwrap(),fixture().canonical_bytes().unwrap());
  assert!(holds.contains(EMPTY_SHA).unwrap());assert!(holds.contains(&value.container.sha256).unwrap());
  let descriptor=value.motion(EMPTY_SHA).unwrap();
  ProgramStore::new(&root.path().join("motion-objects")).unwrap().read(descriptor).unwrap();
  drop(value);
  assert!(!holds.contains(EMPTY_SHA).unwrap());
  assert_eq!(std::fs::read_dir(root.path().join("project-import-staging")).unwrap().count(),0);
  assert_eq!(store.allocated_bytes().unwrap(),1652+1587+13);
 }
 #[test]
 fn upload_replays_are_exact_bounded_and_cannot_make_holes(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();let mut upload=store.begin_upload(PackageImportOperationId::new("op").unwrap(),declaration(&bytes),PackageCodecLimits::default()).unwrap();
  assert!(upload.append(1,&bytes[..1]).is_err());
  assert_eq!(upload.append(0,&bytes[..100]).unwrap(),100);
  assert_eq!(upload.append(0,&bytes[..100]).unwrap(),100);
  assert_eq!(upload.append(0,b"wrong").unwrap_err().code,ErrorCode::DependencyMismatch);
  assert!(upload.append(90,&bytes[90..110]).is_err());
  assert!(upload.append(u64::MAX,b"x").is_err());
  upload.append(100,&bytes[100..]).unwrap();
  store.seal_upload(upload,||Ok(()),|_|Ok(())).unwrap();
 }
 #[test]
 fn seal_rejects_partial_and_wrong_declared_container_hash(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();
  let mut upload=store.begin_upload(PackageImportOperationId::new("op").unwrap(),declaration(&bytes),PackageCodecLimits::default()).unwrap();
  upload.append(0,&bytes[..52]).unwrap();
  assert!(store.seal_upload(upload,||Ok(()),|_|Ok(())).is_err());
  let mut wrong=declaration(&bytes);wrong.sha256="f".repeat(64);
  let mut upload=store.begin_upload(PackageImportOperationId::new("op").unwrap(),wrong,PackageCodecLimits::default()).unwrap();
  upload.append(0,&bytes).unwrap();
  assert_eq!(store.seal_upload(upload,||Ok(()),|_|Ok(())).err().unwrap().code,ErrorCode::DependencyMismatch);
  assert_eq!(store.allocated_bytes().unwrap(),0);
 }
 #[test]
 fn noncanonical_motion_is_rejected_before_any_cas_publication(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoding_with_motion(br#"{"tracks": []}"#);
  let sealed=store.seal_upload(uploaded(&store,&bytes),||Ok(()),|_|Ok(())).unwrap();
  assert_eq!(store.verify_and_stage(sealed,||Ok(()),|_|Ok(())).err().unwrap().code,ErrorCode::InvalidRequest);
  assert_eq!(store.allocated_bytes().unwrap(),0);
 }
 #[test]
 fn canonical_motion_must_match_declared_axis_metadata(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let mut manifest=fixture();
  let mut motion=manifest.head.motion.clone();
  motion.axes=vec![AxisSummary{axis:Axis::ALL[0],action_count:0,gap_count:0}];
  manifest.head.motion=motion.clone();manifest.revisions[0].motion=motion.clone();manifest.edit_states[0].motion=motion;
  let mut bytes=vec![];
  write_package(&manifest,&mut bytes,|_|Ok(Cursor::new(EMPTY_BYTES)),PackageCodecLimits::default()).unwrap();
  let sealed=store.seal_upload(uploaded(&store,&bytes),||Ok(()),|_|Ok(())).unwrap();
  assert_eq!(store.verify_and_stage(sealed,||Ok(()),|_|Ok(())).err().unwrap().code,ErrorCode::DependencyMismatch);
  assert_eq!(store.allocated_bytes().unwrap(),0);
 }
 #[test]
 fn whole_container_digest_does_not_replace_strict_eof_validation(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let original=encoded();
  for bytes in [original[..original.len()-1].to_vec(),[original.as_slice(),b"x"].concat()]{
   let sealed=store.seal_upload(uploaded(&store,&bytes),||Ok(()),|_|Ok(())).unwrap();
   assert!(store.verify_and_stage(sealed,||Ok(()),|_|Ok(())).is_err());
   assert_eq!(store.allocated_bytes().unwrap(),0);
  }
 }
 #[test]
 fn cancellation_before_publication_does_not_leave_visible_objects(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();let sealed=store.seal_upload(uploaded(&store,&bytes),||Ok(()),|_|Ok(())).unwrap();
  let error=store.verify_and_stage(sealed,||Ok(()),|p|if p.completed_bytes>52{Err(ProtocolError::new(ErrorCode::Forbidden,"revoked"))}else{Ok(())}).err().unwrap();
  assert_eq!(error.code,ErrorCode::Forbidden);
  assert_eq!(store.allocated_bytes().unwrap(),0);
 }
 #[test]
 fn cancellation_after_first_publication_retains_recovery_as_unknown_outcome(){
  let root=tempfile::tempdir().unwrap();let holds=HoldRegistry::default();let store=ImportStore::new(root.path(),holds.clone()).unwrap();
  let bytes=encoded();let value=verified(&store,&bytes);let sha=value.container.sha256.clone();
  let held=store.prepare_publication(value).unwrap();
  let error=store.publish_objects(held,||Ok(()),|_|Err(ProtocolError::new(ErrorCode::Forbidden,"revoked"))).err().unwrap();
  assert_eq!(error.code,ErrorCode::UnknownOutcome);
  assert_eq!(std::fs::read(root.path().join("imported-project-containers").join(&sha)).unwrap(),bytes);
  assert_eq!(store.allocated_bytes().unwrap(),bytes.len() as u64);
  assert!(!holds.contains(&sha).unwrap());
 }
 #[test]
 fn duplicate_publication_fully_verifies_shared_cas_and_never_overwrites_corruption(){
  use std::os::unix::fs::{MetadataExt,PermissionsExt};
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();drop(publish(&store,&bytes));
  let path=root.path().join("motion-objects").join(EMPTY_SHA);
  let inode=std::fs::metadata(&path).unwrap().ino();
  drop(publish(&store,&bytes));
  assert_eq!(std::fs::metadata(&path).unwrap().ino(),inode);
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).unwrap();
  let bad=b"XXXXXXXXXXXXX";std::fs::write(&path,bad).unwrap();
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o400)).unwrap();
  let held=store.prepare_publication(verified(&store,&bytes)).unwrap();
  assert_eq!(store.publish_objects(held,||Ok(()),|_|Ok(())).err().unwrap().code,ErrorCode::DependencyMismatch);
  assert_eq!(std::fs::read(path).unwrap(),bad);
 }
 #[test]
 fn original_container_cas_hit_is_also_fully_verified(){
  use std::os::unix::fs::PermissionsExt;
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();let first=publish(&store,&bytes);let sha=first.container.sha256.clone();drop(first);
  let path=root.path().join("imported-project-containers").join(sha);
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).unwrap();
  let mut bad=bytes.clone();*bad.last_mut().unwrap()=b'!';std::fs::write(&path,&bad).unwrap();
  std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o400)).unwrap();
  let held=store.prepare_publication(verified(&store,&bytes)).unwrap();
  assert_eq!(store.publish_objects(held,||Ok(()),|_|Ok(())).err().unwrap().code,ErrorCode::DependencyMismatch);
  assert_eq!(std::fs::read(path).unwrap(),bad);
 }
 #[test]
 fn source_bytes_are_published_to_snapshot_namespace_without_original_paths(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let source=b"exact original source bytes";let sha=format!("{:x}",Sha256::digest(source));
  let mut manifest=fixture();
  manifest.sources.push(PackageSource{source_version:SourceVersionId::new("source").unwrap(),kind:SourceKind::Media,label:"source".into(),
   identity:identity("source",&sha,source.len() as u64).unwrap(),pinned:false,evicted:false});
  manifest.objects.push(PackageObjectDescriptor{sha256:sha.clone(),byte_len:source.len() as u64,roles:vec![PackageObjectRole::SourceSnapshot]});
  manifest.objects.sort_by(|a,b|a.sha256.cmp(&b.sha256));manifest.counts=manifest.record_counts().unwrap();
  let mut bytes=vec![];
  write_package(&manifest,&mut bytes,|o|Ok(Cursor::new(if o.sha256==sha{source.as_slice()}else{EMPTY_BYTES})),PackageCodecLimits::default()).unwrap();
  let value=publish(&store,&bytes);
  assert_eq!(std::fs::read(root.path().join("snapshots").join(&sha)).unwrap(),source);
  assert!(value.publications.iter().any(|p|p.namespace==ImportNamespace::Source&&p.identity.sha256==sha));
 }
 #[test]
 fn accounted_staging_and_all_cas_orphans_survive_until_explicit_recovery(){
  use std::os::unix::fs::PermissionsExt;
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();drop(publish(&store,&bytes));let before=store.allocated_bytes().unwrap();
  let pending=root.path().join("project-import-staging").join(format!(".import-object-{}",uuid::Uuid::new_v4()));
  std::fs::write(&pending,b"abandoned").unwrap();std::fs::set_permissions(&pending,std::fs::Permissions::from_mode(0o600)).unwrap();
  assert_eq!(store.allocated_bytes().unwrap(),before+9);
  assert_eq!(store.cleanup_staging().unwrap(),1);
  assert_eq!(store.allocated_bytes().unwrap(),before);
  assert_eq!(store.cleanup_staging().unwrap(),0);
 }
 #[test]
 fn symlink_roots_and_cross_store_capabilities_are_rejected(){
  use std::os::unix::fs::symlink;
  let root=tempfile::tempdir().unwrap();let alias=root.path().join("alias");symlink(root.path(),&alias).unwrap();
  assert!(ImportStore::new(&alias,HoldRegistry::default()).is_err());
  let first=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let second=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=encoded();let upload=uploaded(&first,&bytes);
  assert!(second.seal_upload(upload,||Ok(()),|_|Ok(())).is_err());
 }
 #[test]
 fn identity_map_receipts_use_inert_content_addressed_evidence(){
  let root=tempfile::tempdir().unwrap();let store=ImportStore::new(root.path(),HoldRegistry::default()).unwrap();
  let bytes=br#"{"origin":"archival-only","local":"fresh-project"}"#;
  let first=store.publish_evidence(bytes).unwrap();let second=store.publish_evidence(bytes).unwrap();
  assert_eq!(first,second);
  assert_eq!(std::fs::read(root.path().join("imported-project-evidence").join(&first.sha256)).unwrap(),bytes);
 }
 #[test]
 fn import_capabilities_can_move_to_bounded_background_workers(){
  fn send_sync<T:Send+Sync>(){}
  send_sync::<ImportStore>();send_sync::<ProvisionalImportUpload>();send_sync::<VerifiedImportBytes>();send_sync::<HeldImportBytes>();
 }

}

