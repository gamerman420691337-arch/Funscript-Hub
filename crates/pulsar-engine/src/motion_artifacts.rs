//! Immutable engine-owned motion program objects.
//!
//! Content files are published and file/directory-synced before returning a
//! descriptor. The caller must commit its SQLite reference only afterwards.
//! A crash before reference commit can leave an unreferenced complete object;
//! retry verifies/reuses it. This module never garbage-collects complete objects.
//! Crashed .pending-* files are not valid objects and may be removed by exclusive
//! startup recovery after leases/jobs are reconciled. They are never opened by
//! a descriptor. Legacy migration bytes remain the caller's responsibility.
//!
//! Unix directory descriptors anchor every lookup; every path component is
//! opened with O_DIRECTORY|O_NOFOLLOW. Publication uses no-replace linkat then
//! directory fsync. Reference: Linux open(2), link(2), and fsync(2) manuals:
//! https://man7.org/linux/man-pages/man2/open.2.html
//! https://man7.org/linux/man-pages/man2/link.2.html
//! https://man7.org/linux/man-pages/man2/fsync.2.html
//!
//! Read-only file modes are defense in depth, not protection from a malicious
//! unsandboxed process with the same OS identity. Broker callers retain lease
//! authority and must not pass client-provided descriptor metadata unchecked.

use pulsar_core::{ArtifactId, Axis, MotionAction, MotionProgram, MotionTrack, TimeRange};
use pulsar_protocol::{AxisSummary, ErrorCode, MotionCodec, ProgramDescriptor, ProtocolError};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::Path;

pub const MAX_PROGRAM_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PROGRAM_ACTIONS: usize = 1_000_000;
pub const MAX_PROGRAM_GAPS: usize = 1_000_000;
pub type StoredProgram = ProgramDescriptor;
type Result<T> = std::result::Result<T, ProtocolError>;

fn bounded_message(message: impl Into<String>) -> String {
    let mut message = message.into();
    let mut end = message.len().min(1024);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message
}
fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidRequest, bounded_message(message))
}
fn exhausted(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::ResourceExhausted, bounded_message(message))
}
fn internal(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, bounded_message(message))
}
fn io_error(operation: &str, error: io::Error) -> ProtocolError {
    #[cfg(unix)]
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOSPC) | Some(libc::EDQUOT)
    ) {
        return exhausted(format!("{operation}: storage capacity unavailable"));
    }
    internal(format!("{operation}: {error}"))
}

pub struct ProgramStore {
    #[cfg(unix)]
    directory: File,
}

fn summaries(program: &MotionProgram) -> Result<Vec<AxisSummary>> {
    let mut actions = 0usize;
    let mut gaps = 0usize;
    let mut axes = Vec::with_capacity(program.tracks().len());
    for track in program.tracks() {
        actions = actions
            .checked_add(track.actions().len())
            .ok_or_else(|| exhausted("action count overflow"))?;
        gaps = gaps
            .checked_add(track.gaps().len())
            .ok_or_else(|| exhausted("gap count overflow"))?;
        if actions > MAX_PROGRAM_ACTIONS || gaps > MAX_PROGRAM_GAPS {
            return Err(exhausted("motion program exceeds action or gap budget"));
        }
        axes.push(AxisSummary {
            axis: track.axis(),
            action_count: track.actions().len() as u64,
            gap_count: track.gaps().len() as u64,
        });
    }
    Ok(axes)
}
fn descriptor(hash: String, bytes: u64, axes: Vec<AxisSummary>) -> Result<StoredProgram> {
    let descriptor = ProgramDescriptor {
        artifact_id: ArtifactId::new(format!("motion.{hash}"))
            .map_err(|_| internal("invalid computed artifact identity"))?,
        sha256: hash,
        byte_len: bytes,
        codec: MotionCodec::MotionProgramJsonV1,
        axes,
    };
    descriptor.validate()?;
    Ok(descriptor)
}
fn validate_descriptor(value: &StoredProgram) -> Result<()> {
    value.validate()?;
    if value.sha256.len() != 64
        || !value
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || value.byte_len == 0
        || value.byte_len > MAX_PROGRAM_BYTES
        || value.artifact_id.as_str() != format!("motion.{}", value.sha256)
    {
        return Err(invalid(
            "motion descriptor is not a canonical bounded content identity",
        ));
    }
    Ok(())
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Component;

    pub(super) fn directory(root: &Path) -> Result<File> {
        if !root.is_absolute() {
            return Err(invalid(
                "motion store root must be engine-generated and absolute",
            ));
        }
        let components = root.components().collect::<Vec<_>>();
        if components.len() < 2
            || components
                .iter()
                .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        {
            return Err(invalid(
                "motion store root contains unsupported path components",
            ));
        }
        let mut current = File::open("/").map_err(|e| io_error("open filesystem root", e))?;
        for (index, component) in components.iter().enumerate().skip(1) {
            let Component::Normal(name) = component else {
                return Err(invalid("invalid store path"));
            };
            let name =
                CString::new(name.as_bytes()).map_err(|_| invalid("store path contains NUL"))?;
            let last = index + 1 == components.len();
            let mut fd = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 && last && io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
                let created = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) };
                if created < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
                {
                    return Err(io_error(
                        "create private motion store",
                        io::Error::last_os_error(),
                    ));
                }
                current
                    .sync_all()
                    .map_err(|e| io_error("sync motion-store parent", e))?;
                fd = unsafe {
                    libc::openat(
                        current.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
            }
            if fd < 0 {
                return Err(io_error(
                    "open no-follow motion directory",
                    io::Error::last_os_error(),
                ));
            }
            // SAFETY: successful openat returned one owned descriptor.
            current = unsafe { File::from_raw_fd(fd) };
        }
        let metadata = current
            .metadata()
            .map_err(|e| io_error("inspect motion store", e))?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o700
        {
            return Err(invalid("motion store must be current-user-owned mode 0700"));
        }
        current
            .sync_all()
            .map_err(|e| io_error("sync motion store", e))?;
        Ok(current)
    }

    pub(super) fn open(directory: &File, name: &str) -> Result<File> {
        let name = CString::new(name).map_err(|_| invalid("invalid object identity"))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io_error(
                "open immutable motion object",
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: successful openat returned one owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file
            .metadata()
            .map_err(|e| io_error("inspect motion object", e))?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o277 != 0 {
            return Err(internal(
                "motion object must be private, read-only, and regular",
            ));
        }
        // A crash after linkat but before unlinking our pending name can leave
        // two links to valid published bytes. Link count is not authority and
        // must not make that complete object unrecoverable.
        Ok(file)
    }
    pub(super) struct Pending<'a> {
        pub file: File,
        directory: &'a File,
        name: Option<CString>,
    }
    impl<'a> Pending<'a> {
        pub fn new(directory: &'a File) -> Result<Self> {
            let name = CString::new(format!(".pending-{}", uuid::Uuid::new_v4()))
                .expect("UUID has no NUL");
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(io_error(
                    "create pending motion object",
                    io::Error::last_os_error(),
                ));
            }
            Ok(Self {
                file: unsafe { File::from_raw_fd(fd) },
                directory,
                name: Some(name),
            })
        }
        pub fn publish(&mut self, hash: &str) -> Result<bool> {
            let name = self
                .name
                .as_ref()
                .ok_or_else(|| internal("pending object was already published"))?;
            let hash = CString::new(hash).map_err(|_| internal("computed digest contains NUL"))?;
            let result = unsafe {
                libc::linkat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    self.directory.as_raw_fd(),
                    hash.as_ptr(),
                    0,
                )
            };
            let created = if result == 0 {
                true
            } else {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::AlreadyExists {
                    false
                } else {
                    return Err(io_error("publish immutable motion object", error));
                }
            };
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                return Err(io_error(
                    "remove pending motion name",
                    io::Error::last_os_error(),
                ));
            }
            self.name = None;
            Ok(created)
        }
    }
    impl Drop for Pending<'_> {
        fn drop(&mut self) {
            if let Some(name) = &self.name {
                // Only our own engine-generated pending basename is removed.
                // A process crash can leave this non-object name for recovery.
                unsafe {
                    libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0);
                }
            }
        }
    }

    impl ProgramStore {
        /// Parent must already exist. Creates only the final private directory;
        /// refuses all symlink ancestors instead of canonicalizing through them.
        pub fn new(root: &Path) -> Result<Self> {
            Ok(Self {
                directory: directory(root)?,
            })
        }
        pub fn publish(&self, program: &MotionProgram) -> Result<StoredProgram> {
            self.publish_limited(program, MAX_PROGRAM_BYTES, false)
        }
        pub(super) fn publish_limited(
            &self,
            program: &MotionProgram,
            limit: u64,
            fail_before_publish: bool,
        ) -> Result<StoredProgram> {
            let axes = summaries(program)?;
            let mut pending = Pending::new(&self.directory)?;
            let (hash, bytes) = {
                let mut buffered = std::io::BufWriter::with_capacity(65536, &mut pending.file);
                let mut writer = LimitedWriter {
                    file: &mut buffered,
                    hash: Sha256::new(),
                    bytes: 0,
                    limit: limit.min(MAX_PROGRAM_BYTES),
                    exceeded: false,
                    last_os_error: None,
                };
                if let Err(error) = serde_json::to_writer(&mut writer, program) {
                    if writer.exceeded {
                        return Err(exhausted("canonical motion program exceeds byte budget"));
                    }
                    if matches!(
                        writer.last_os_error,
                        Some(libc::ENOSPC) | Some(libc::EDQUOT)
                    ) {
                        return Err(exhausted("motion publication storage capacity unavailable"));
                    }
                    return Err(internal(format!(
                        "serialize immutable motion object: {error}"
                    )));
                }
                writer
                    .flush()
                    .map_err(|error| io_error("flush immutable motion bytes", error))?;
                (format!("{:x}", writer.hash.finalize()), writer.bytes)
            };
            let descriptor = descriptor(hash, bytes, axes)?;
            pending
                .file
                .set_permissions(std::fs::Permissions::from_mode(0o400))
                .map_err(|e| io_error("seal motion object permissions", e))?;
            pending
                .file
                .sync_all()
                .map_err(|e| io_error("sync immutable motion bytes", e))?;
            if fail_before_publish {
                return Err(internal("injected failure before object publication"));
            }
            let created = pending.publish(&descriptor.sha256)?;
            if !created {
                // Never overwrite a corrupt pre-existing object, even if its
                // filename matches the digest we intended to publish.
                self.open_verified(&descriptor)?;
            }
            self.directory
                .sync_all()
                .map_err(|e| io_error("sync motion object publication", e))?;
            Ok(descriptor)
        }
        /// Legacy worker/migration bytes may use different whitespace. Decode
        /// with bounded collections, re-encode, and return the canonical digest.
        /// The returned identity never claims to hash noncanonical input bytes.
        pub fn import_canonical(&self, bytes: &[u8]) -> Result<StoredProgram> {
            let program = decode_bounded(bytes)?;
            self.publish(&program)
        }
        pub fn read(&self, descriptor: &StoredProgram) -> Result<MotionProgram> {
            let mut file = self.open_verified(descriptor)?;
            let capacity = usize::try_from(descriptor.byte_len)
                .map_err(|_| exhausted("motion object cannot fit address space"))?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(capacity)
                .map_err(|_| exhausted("motion object read allocation unavailable"))?;
            (&mut file)
                .take(descriptor.byte_len + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| io_error("read motion object", e))?;
            if bytes.len() as u64 != descriptor.byte_len
                || format!("{:x}", Sha256::digest(&bytes)) != descriptor.sha256
            {
                return Err(internal("motion object changed after verification"));
            }
            let program = decode_bounded(&bytes)?;
            let actual = summaries(&program)?;
            if actual.len() != descriptor.axes.len()
                || actual.iter().zip(&descriptor.axes).any(|(a, b)| {
                    a.axis != b.axis
                        || a.action_count != b.action_count
                        || a.gap_count != b.gap_count
                })
            {
                return Err(internal("motion descriptor metadata differs from program"));
            }
            Ok(program)
        }
        /// Verifies the entire immutable object's digest and byte count, then
        /// rewinds the same fd for lease-scoped bounded range reads. Does not
        /// repeatedly decode JSON. Descriptor must come from engine authority.
        pub fn open_verified(&self, descriptor: &StoredProgram) -> Result<File> {
            validate_descriptor(descriptor)?;
            let mut file = open(&self.directory, &descriptor.sha256)?;
            let before = file
                .metadata()
                .map_err(|e| io_error("inspect motion length", e))?;
            if before.len() != descriptor.byte_len || before.len() > MAX_PROGRAM_BYTES {
                return Err(internal(
                    "motion object length differs from descriptor or exceeds budget",
                ));
            }
            let mut hash = Sha256::new();
            let mut total = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|e| io_error("verify motion object", e))?;
                if count == 0 {
                    break;
                }
                total = total
                    .checked_add(count as u64)
                    .ok_or_else(|| exhausted("motion object read overflow"))?;
                if total > descriptor.byte_len || total > MAX_PROGRAM_BYTES {
                    return Err(internal("motion object grew during verification"));
                }
                hash.update(&buffer[..count]);
            }
            let after = file
                .metadata()
                .map_err(|e| io_error("inspect verified motion object", e))?;
            if total != descriptor.byte_len
                || format!("{:x}", hash.finalize()) != descriptor.sha256
                || before.len() != after.len()
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
                || before.ctime() != after.ctime()
                || before.ctime_nsec() != after.ctime_nsec()
            {
                return Err(internal(
                    "motion object content identity failed verification",
                ));
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|e| io_error("rewind verified motion object", e))?;
            Ok(file)
        }
    }
}

#[cfg(not(unix))]
impl ProgramStore {
    pub fn new(_root: &Path) -> Result<Self> {
        Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "secure immutable motion storage is not qualified on this platform",
        ))
    }
    pub fn publish(&self, _program: &MotionProgram) -> Result<StoredProgram> {
        Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "secure immutable motion storage unavailable",
        ))
    }
    pub fn import_canonical(&self, _bytes: &[u8]) -> Result<StoredProgram> {
        Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "secure immutable motion storage unavailable",
        ))
    }
    pub fn read(&self, _descriptor: &StoredProgram) -> Result<MotionProgram> {
        Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "secure immutable motion storage unavailable",
        ))
    }
    pub fn open_verified(&self, _descriptor: &StoredProgram) -> Result<File> {
        Err(ProtocolError::new(
            ErrorCode::Unsupported,
            "secure immutable motion storage unavailable",
        ))
    }
}

struct LimitedWriter<'a> {
    file: &'a mut dyn Write,
    hash: Sha256,
    bytes: u64,
    limit: u64,
    exceeded: bool,
    last_os_error: Option<i32>,
}
impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self.bytes.checked_add(bytes.len() as u64);
        if !next.is_some_and(|count| count <= self.limit) {
            self.exceeded = true;
            return Err(io::Error::other("motion byte budget exceeded"));
        }
        let count = match self.file.write(bytes) {
            Ok(count) => count,
            Err(error) => {
                self.last_os_error = error.raw_os_error();
                return Err(error);
            }
        };
        self.hash.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Default)]
struct DecodeBudget {
    actions: usize,
    gaps: usize,
    exceeded: bool,
}
fn decode_bounded(bytes: &[u8]) -> Result<MotionProgram> {
    if bytes.len() as u64 > MAX_PROGRAM_BYTES {
        return Err(exhausted("motion JSON exceeds byte budget"));
    }
    let mut budget = DecodeBudget::default();
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let result = ProgramSeed(&mut budget).deserialize(&mut deserializer);
    if budget.exceeded {
        return Err(exhausted("motion JSON exceeds axis, action, or gap budget"));
    }
    let program =
        result.map_err(|error| invalid(format!("invalid checked motion program: {error}")))?;
    deserializer
        .end()
        .map_err(|error| invalid(format!("trailing motion program data: {error}")))?;
    Ok(program)
}
struct ProgramSeed<'a>(&'a mut DecodeBudget);
impl<'de> DeserializeSeed<'de> for ProgramSeed<'_> {
    type Value = MotionProgram;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for ProgramSeed<'_> {
    type Value = MotionProgram;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a checked motion program")
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut tracks = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "tracks" if tracks.is_none() => {
                    tracks = Some(map.next_value_seed(TracksSeed(self.0))?)
                }
                "tracks" => return Err(de::Error::duplicate_field("tracks")),
                _ => return Err(de::Error::unknown_field(&key, &["tracks"])),
            }
        }
        MotionProgram::new(tracks.ok_or_else(|| de::Error::missing_field("tracks"))?)
            .map_err(de::Error::custom)
    }
}
struct TracksSeed<'a>(&'a mut DecodeBudget);
impl<'de> DeserializeSeed<'de> for TracksSeed<'_> {
    type Value = Vec<MotionTrack>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for TracksSeed<'_> {
    type Value = Vec<MotionTrack>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("at most six motion tracks")
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut tracks = Vec::new();
        loop {
            if tracks.len() == 6 {
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    self.0.exceeded = true;
                    return Err(de::Error::custom("axis budget exceeded"));
                }
                break;
            }
            match sequence.next_element_seed(TrackSeed(self.0))? {
                Some(track) => tracks.push(track),
                None => break,
            }
        }
        Ok(tracks)
    }
}
struct TrackSeed<'a>(&'a mut DecodeBudget);
impl<'de> DeserializeSeed<'de> for TrackSeed<'_> {
    type Value = MotionTrack;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for TrackSeed<'_> {
    type Value = MotionTrack;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a checked motion track")
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let (mut name, mut axis, mut actions, mut gaps) =
            (None::<String>, None::<Axis>, None, None);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" if name.is_none() => name = Some(map.next_value()?),
                "axis" if axis.is_none() => axis = Some(map.next_value()?),
                "actions" if actions.is_none() => {
                    actions = Some(map.next_value_seed(ItemsSeed::<MotionAction> {
                        count: &mut self.0.actions,
                        exceeded: &mut self.0.exceeded,
                        limit: MAX_PROGRAM_ACTIONS,
                        marker: PhantomData,
                    })?)
                }
                "gaps" if gaps.is_none() => {
                    gaps = Some(map.next_value_seed(ItemsSeed::<TimeRange> {
                        count: &mut self.0.gaps,
                        exceeded: &mut self.0.exceeded,
                        limit: MAX_PROGRAM_GAPS,
                        marker: PhantomData,
                    })?)
                }
                "name" | "axis" | "actions" | "gaps" => {
                    return Err(de::Error::custom("duplicate motion track field"))
                }
                _ => {
                    return Err(de::Error::unknown_field(
                        &key,
                        &["name", "axis", "actions", "gaps"],
                    ))
                }
            }
        }
        MotionTrack::with_name_and_gaps(
            name.ok_or_else(|| de::Error::missing_field("name"))?,
            axis.ok_or_else(|| de::Error::missing_field("axis"))?,
            actions.ok_or_else(|| de::Error::missing_field("actions"))?,
            gaps.unwrap_or_default(),
        )
        .map_err(de::Error::custom)
    }
}
struct ItemsSeed<'a, T> {
    count: &'a mut usize,
    exceeded: &'a mut bool,
    limit: usize,
    marker: PhantomData<T>,
}
impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for ItemsSeed<'_, T> {
    type Value = Vec<T>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de, T: Deserialize<'de>> Visitor<'de> for ItemsSeed<'_, T> {
    type Value = Vec<T>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a bounded motion collection")
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        loop {
            if *self.count == self.limit {
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    *self.exceeded = true;
                    return Err(de::Error::custom("motion item budget exceeded"));
                }
                break;
            }
            match sequence.next_element()? {
                Some(item) => {
                    *self.count += 1;
                    items.push(item);
                }
                None => break,
            }
        }
        Ok(items)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use pulsar_core::{EvidenceKind, NormalizedPosition, ProjectTime};
    use std::fs;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

    fn program(axes: usize, actions: usize) -> MotionProgram {
        MotionProgram::new(
            Axis::ALL[..axes]
                .iter()
                .map(|&axis| {
                    MotionTrack::new(
                        axis,
                        (0..actions)
                            .map(|index| {
                                MotionAction::new(
                                    ProjectTime::from_nanos(index as i64 * 1_000_000),
                                    NormalizedPosition::new((index % 101) as f64 / 100.0).unwrap(),
                                    EvidenceKind::Inferred,
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
    fn store() -> (tempfile::TempDir, ProgramStore) {
        let root = tempfile::tempdir().unwrap();
        let store = ProgramStore::new(&root.path().join("motion-objects")).unwrap();
        (root, store)
    }
    #[test]
    fn idempotent_publication_preserves_inode_and_roundtrips() {
        let (root, store) = store();
        let input = program(1, 100);
        let first = store.publish(&input).unwrap();
        let path = root.path().join("motion-objects").join(&first.sha256);
        let inode = fs::metadata(&path).unwrap().ino();
        let second = store.publish(&input).unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.artifact_id, second.artifact_id);
        assert_eq!(first.byte_len, second.byte_len);
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(store.read(&first).unwrap(), input);
        assert_eq!(
            fs::read_dir(root.path().join("motion-objects"))
                .unwrap()
                .count(),
            1
        );
        let mut file = store.open_verified(&first).unwrap();
        let mut first_byte = [0];
        file.read_exact(&mut first_byte).unwrap();
        assert_eq!(first_byte, [b'{']);
    }
    #[test]
    fn corrupted_digest_length_or_metadata_never_reads_as_valid() {
        let (root, store) = store();
        let descriptor = store.publish(&program(1, 20)).unwrap();
        let mut wrong = descriptor.clone();
        wrong.byte_len += 1;
        assert!(store.read(&wrong).is_err());
        wrong = descriptor.clone();
        wrong.axes[0].action_count += 1;
        assert!(store.read(&wrong).is_err());
        let path = root.path().join("motion-objects").join(&descriptor.sha256);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] = b'[';
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(store.read(&descriptor).is_err());
        assert!(store.publish(&program(1, 20)).is_err());
    }
    #[test]
    fn failure_before_publication_leaves_no_reachable_partial_object() {
        let (root, store) = store();
        assert!(store
            .publish_limited(&program(1, 20), MAX_PROGRAM_BYTES, true)
            .is_err());
        assert_eq!(
            fs::read_dir(root.path().join("motion-objects"))
                .unwrap()
                .count(),
            0
        );
        assert!(store.publish_limited(&program(1, 20), 20, false).is_err());
        assert_eq!(
            fs::read_dir(root.path().join("motion-objects"))
                .unwrap()
                .count(),
            0
        );
        assert!(store.publish(&program(1, 20)).is_ok());
    }
    #[test]
    fn unreferenced_complete_object_survives_restart_without_database_commit() {
        let (root, store) = store();
        let descriptor = store.publish(&program(1, 20)).unwrap();
        drop(store);
        let reopened = ProgramStore::new(&root.path().join("motion-objects")).unwrap();
        assert_eq!(reopened.read(&descriptor).unwrap(), program(1, 20));
    }
    #[test]
    fn canonical_import_returns_digest_of_canonical_bytes_not_input_format() {
        let (_root, store) = store();
        let input = program(1, 3);
        let pretty = serde_json::to_vec_pretty(&input).unwrap();
        let imported = store.import_canonical(&pretty).unwrap();
        assert_eq!(imported.sha256, store.publish(&input).unwrap().sha256);
        assert_ne!(imported.sha256, format!("{:x}", Sha256::digest(&pretty)));
    }
    #[test]
    fn malformed_program_and_byte_or_action_excess_are_rejected() {
        let (_root, store) = store();
        assert!(store
            .import_canonical(br#"{"tracks":[],"tracks":[]}"#)
            .is_err());
        assert!(store
            .import_canonical(br#"{"tracks":[]} trailing"#)
            .is_err());
        assert!(store
            .import_canonical(&vec![b' '; MAX_PROGRAM_BYTES as usize + 1])
            .is_err());
        let excessive = program(1, MAX_PROGRAM_ACTIONS + 1);
        assert!(store.publish(&excessive).is_err());
        let mut budget = DecodeBudget {
            actions: MAX_PROGRAM_ACTIONS,
            gaps: 0,
            exceeded: false,
        };
        let mut deserializer = serde_json::Deserializer::from_str(
            r#"{"tracks":[{"name":"stroke","axis":"stroke","actions":[{"time":0,"position":0.5,"evidence":"inferred"}]}]}"#,
        );
        assert!(ProgramSeed(&mut budget)
            .deserialize(&mut deserializer)
            .is_err());
        assert!(budget.exceeded);
    }
    #[test]
    fn caller_paths_symlinks_and_nonregular_objects_are_rejected() {
        let (root, store) = store();
        let descriptor = store.publish(&program(1, 2)).unwrap();
        let path = root.path().join("motion-objects").join(&descriptor.sha256);
        let outside = root.path().join("outside");
        fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(store.open_verified(&descriptor).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store.open_verified(&descriptor).is_err());
        let mut traversal = descriptor.clone();
        traversal.sha256 = "../outside".into();
        assert!(store.open_verified(&traversal).is_err());
        let alias = root.path().join("alias");
        symlink(root.path().join("motion-objects"), &alias).unwrap();
        assert!(ProgramStore::new(&alias).is_err());
        assert!(ProgramStore::new(&alias.join("child")).is_err());
        assert!(ProgramStore::new(&root.path().join("motion-objects/../other")).is_err());
    }
    #[test]
    fn refuses_public_store_and_writable_objects() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("motion-objects");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ProgramStore::new(&directory).is_err());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let store = ProgramStore::new(&directory).unwrap();
        let descriptor = store.publish(&program(1, 2)).unwrap();
        fs::set_permissions(
            directory.join(&descriptor.sha256),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(store.open_verified(&descriptor).is_err());
    }
    #[test]
    fn production_sized_stroke_and_six_axes_fit_beyond_control_message_limits() {
        let (_root, store) = store();
        for axes in [1, 6] {
            let input = program(axes, 54_000);
            let descriptor = store.publish(&input).unwrap();
            assert!(descriptor.byte_len > 512 * 1024);
            assert!(descriptor.byte_len < MAX_PROGRAM_BYTES);
            assert_eq!(
                descriptor
                    .axes
                    .iter()
                    .map(|axis| axis.action_count)
                    .sum::<u64>(),
                axes as u64 * 54_000
            );
            assert_eq!(store.read(&descriptor).unwrap(), input);
            println!(
                "motion sizing axes={axes} actions={} canonical_bytes={}",
                axes * 54_000,
                descriptor.byte_len
            );
        }
    }
    #[test]
    fn empty_initial_project_has_stable_valid_content_descriptor() {
        let (_root, store) = store();
        let empty = MotionProgram::default();
        let first = store.publish(&empty).unwrap();
        let second = store.publish(&empty).unwrap();
        assert_eq!(first.artifact_id, second.artifact_id);
        assert!(first.axes.is_empty());
        assert_eq!(store.read(&first).unwrap(), empty);
    }

    #[test]
    fn crash_after_final_link_before_pending_unlink_remains_recoverable() {
        let (root, store) = store();
        let input = program(1, 20);
        let descriptor = store.publish(&input).unwrap();
        let directory = root.path().join("motion-objects");
        fs::hard_link(
            directory.join(&descriptor.sha256),
            directory.join(".pending-crash-fixture"),
        )
        .unwrap();
        drop(store);
        let recovered = ProgramStore::new(&directory).unwrap();
        assert_eq!(recovered.read(&descriptor).unwrap(), input);
        assert_eq!(
            recovered.publish(&input).unwrap().artifact_id,
            descriptor.artifact_id
        );
    }

    #[test]
    fn oversized_sparse_file_and_fifo_are_rejected_before_buffer_allocation() {
        let (root, store) = store();
        let descriptor = store.publish(&program(1, 20)).unwrap();
        let path = root.path().join("motion-objects").join(&descriptor.sha256);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_PROGRAM_BYTES + 1)
            .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(store.open_verified(&descriptor).is_err());
        fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o400) }, 0);
        assert!(store.open_verified(&descriptor).is_err());
    }

    #[test]
    fn malformed_field_names_cannot_expand_control_errors_without_bound() {
        let (_root, store) = store();
        let bytes = format!("{{\"tracks\":[],\"{}\":0}}", "x".repeat(8192));
        let error = store.import_canonical(bytes.as_bytes()).unwrap_err();
        assert!(error.message.len() <= 1024);
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}
