//! Bounded, uncompressed portable-project container codec.
//!
//! This module never authorizes, resolves filesystem paths, publishes objects,
//! executes historical records, or imports project authority. Decode sinks must
//! be provisional staging; a later error invalidates every earlier sink.
//!
//! Format v1: magic[8], version:u16 LE, flags:u16 LE (zero), manifest length:u64
//! LE, SHA-256(manifest)[32], canonical manifest JSON, then exact object bytes
//! in the manifest's digest order. There are no entry paths or executable hooks.
//!
//! Generic Read/Write and callbacks may block, including the final EOF probe.
//! The caller owns absolute I/O deadlines and cancellation; bounded byte counts
//! do not establish bounded wall-clock time or transfer authorization.

use pulsar_protocol::{
    ErrorCode, PackageObjectDescriptor, PortableProjectManifest, ProtocolError,
    DEFAULT_MAX_PROJECT_PACKAGE_BYTES, MAX_PROJECT_PACKAGE_MANIFEST_BYTES,
    PROJECT_PACKAGE_FORMAT_VERSION,
};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

pub const PACKAGE_MAGIC: [u8; 8] = *b"PULSPKG\0";
pub const PACKAGE_HEADER_BYTES: usize = 52;
const BUFFER_BYTES: usize = 64 * 1024;
type Result<T> = std::result::Result<T, ProtocolError>;

/// Content integrity only, not an authenticity or qualification assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDigest {
    pub sha256: String,
    pub byte_len: u64,
}

#[derive(Clone, Debug)]
pub struct DecodedPackage {
    pub manifest: PortableProjectManifest,
    pub digest: PackageDigest,
}

#[derive(Clone, Copy, Debug)]
pub struct PackageCodecLimits {
    /// Total container bytes, including its header and metadata.
    pub max_package_bytes: u64,
}
impl Default for PackageCodecLimits {
    fn default() -> Self {
        Self {
            max_package_bytes: DEFAULT_MAX_PROJECT_PACKAGE_BYTES,
        }
    }
}

fn exhausted(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ResourceExhausted, message)
}
fn mismatch(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::DependencyMismatch, message)
}
fn io_error(error: io::Error) -> ProtocolError {
    ProtocolError::new(ErrorCode::Unavailable, format!("package stream I/O: {error}"))
}
fn exact(reader: &mut impl Read, bytes: &mut [u8]) -> Result<()> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            ProtocolError::invalid("truncated package")
        } else {
            io_error(error)
        }
    })
}
fn read_some(reader: &mut impl Read, bytes: &mut [u8]) -> Result<usize> {
    loop {
        match reader.read(bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
            Ok(count) if count > bytes.len() => {
                return Err(ProtocolError::invalid("reader violated its byte bound"))
            }
            Ok(count) => return Ok(count),
        }
    }
}
fn flush_stream(writer: &mut impl Write) -> Result<()> {
    loop {
        match writer.flush() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
            Ok(()) => return Ok(()),
        }
    }
}
fn total_length(
    manifest: &PortableProjectManifest,
    manifest_len: usize,
    limits: PackageCodecLimits,
) -> Result<u64> {
    let mut total = (PACKAGE_HEADER_BYTES as u64)
        .checked_add(manifest_len as u64)
        .ok_or_else(|| exhausted("package length overflow"))?;
    for object in &manifest.objects {
        total = total
            .checked_add(object.byte_len)
            .ok_or_else(|| exhausted("package object length overflow"))?;
    }
    if total > limits.max_package_bytes {
        return Err(exhausted("package exceeds admitted total byte limit"));
    }
    Ok(total)
}

struct HashReader<'a, R> {
    inner: &'a mut R,
    hash: Sha256,
    bytes: u64,
}
impl<R: Read> Read for HashReader<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(out)?;
        if count > out.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid read count"));
        }
        self.bytes = self.bytes.checked_add(count as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "package read count overflow")
        })?;
        self.hash.update(&out[..count]);
        Ok(count)
    }
}
impl<R> HashReader<'_, R> {
    fn finish(self) -> PackageDigest {
        PackageDigest {
            sha256: format!("{:x}", self.hash.finalize()),
            byte_len: self.bytes,
        }
    }
}
struct HashWriter<'a, W> {
    inner: &'a mut W,
    hash: Sha256,
    bytes: u64,
}
impl<W: Write> Write for HashWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        if count > bytes.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid write count"));
        }
        self.bytes = self.bytes.checked_add(count as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "package write count overflow")
        })?;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl<W> HashWriter<'_, W> {
    fn finish(self) -> PackageDigest {
        PackageDigest {
            sha256: format!("{:x}", self.hash.finalize()),
            byte_len: self.bytes,
        }
    }
}

/// Copies exactly one declared object, checking every byte without buffering it.
/// It does not read past the object: the decoder shares a stream with later objects.
fn stream_object(
    input: &mut impl Read,
    output: &mut impl Write,
    descriptor: &PackageObjectDescriptor,
) -> Result<()> {
    let mut remaining = descriptor.byte_len;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; BUFFER_BYTES];
    while remaining != 0 {
        let length = remaining.min(BUFFER_BYTES as u64) as usize;
        let read = read_some(input, &mut buffer[..length])?;
        if read == 0 {
            return Err(mismatch("object ended before its declared byte length"));
        }
        output.write_all(&buffer[..read]).map_err(io_error)?;
        hash.update(&buffer[..read]);
        remaining -= read as u64;
    }
    if format!("{:x}", hash.finalize()) != descriptor.sha256 {
        return Err(mismatch("object SHA-256 differs from its descriptor"));
    }
    Ok(())
}

/// Validates metadata before writing and streams each object exactly once.
/// On any error, output is incomplete/untrusted and must not be published.
/// The caller supplies verified engine-owned inputs, not paths from a package.
pub fn write_package<W, R, F>(
    manifest: &PortableProjectManifest,
    output: &mut W,
    mut open_object: F,
    limits: PackageCodecLimits,
) -> Result<PackageDigest>
where
    W: Write,
    R: Read,
    F: FnMut(&PackageObjectDescriptor) -> Result<R>,
{
    if limits.max_package_bytes < PACKAGE_HEADER_BYTES as u64 {
        return Err(exhausted("package budget cannot contain a header"));
    }
    let bytes = manifest.canonical_bytes()?;
    if bytes.len() > MAX_PROJECT_PACKAGE_MANIFEST_BYTES {
        return Err(exhausted("package manifest exceeds its byte limit"));
    }
    let expected = total_length(manifest, bytes.len(), limits)?;
    let mut header = [0u8; PACKAGE_HEADER_BYTES];
    header[..8].copy_from_slice(&PACKAGE_MAGIC);
    header[8..10].copy_from_slice(&PROJECT_PACKAGE_FORMAT_VERSION.to_le_bytes());
    header[12..20].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
    header[20..52].copy_from_slice(&Sha256::digest(&bytes));
    let mut output = HashWriter {
        inner: output,
        hash: Sha256::new(),
        bytes: 0,
    };
    output.write_all(&header).map_err(io_error)?;
    output.write_all(&bytes).map_err(io_error)?;
    for descriptor in &manifest.objects {
        let mut input = open_object(descriptor)?;
        stream_object(&mut input, &mut output, descriptor)?;
        if read_some(&mut input, &mut [0u8; 1])? != 0 {
            return Err(mismatch("object exceeds its declared byte length"));
        }
    }
    flush_stream(&mut output)?;
    let digest = output.finish();
    if digest.byte_len != expected {
        return Err(mismatch("written package length differs from metadata"));
    }
    Ok(digest)
}

/// Parses and validates an entire package, streaming object bytes to private sinks.
/// Sinks can receive bytes before a later failure; they remain provisional until
/// this function returns success. The caller owns cleanup, fsync and publication.
pub fn read_package<R, W, F>(
    input: &mut R,
    mut create_staging: F,
    limits: PackageCodecLimits,
) -> Result<DecodedPackage>
where
    R: Read,
    W: Write,
    F: FnMut(&PackageObjectDescriptor) -> Result<W>,
{
    if limits.max_package_bytes < PACKAGE_HEADER_BYTES as u64 {
        return Err(exhausted("package budget cannot contain a header"));
    }
    let mut input = HashReader {
        inner: input,
        hash: Sha256::new(),
        bytes: 0,
    };
    let mut header = [0u8; PACKAGE_HEADER_BYTES];
    exact(&mut input, &mut header)?;
    if header[..8] != PACKAGE_MAGIC {
        return Err(ProtocolError::invalid("invalid package magic"));
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != PROJECT_PACKAGE_FORMAT_VERSION {
        return Err(ProtocolError::unsupported("portable project format version"));
    }
    if header[10..12] != [0u8; 2] {
        return Err(ProtocolError::unsupported("portable project format flags"));
    }
    let declared = u64::from_le_bytes(header[12..20].try_into().expect("fixed header field"));
    if declared > MAX_PROJECT_PACKAGE_MANIFEST_BYTES as u64
        || declared.checked_add(PACKAGE_HEADER_BYTES as u64)
            .is_none_or(|total| total > limits.max_package_bytes)
    {
        return Err(exhausted("manifest length exceeds admitted package bounds"));
    }
    let length = usize::try_from(declared)
        .map_err(|_| exhausted("manifest length exceeds addressable memory"))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length)
        .map_err(|_| exhausted("manifest allocation could not be admitted"))?;
    bytes.resize(length, 0);
    exact(&mut input, &mut bytes)?;
    if Sha256::digest(&bytes)[..] != header[20..52] {
        return Err(mismatch("manifest SHA-256 mismatch"));
    }
    let manifest: PortableProjectManifest = serde_json::from_slice(&bytes)
        .map_err(|_| ProtocolError::invalid("invalid typed package manifest"))?;
    let canonical = manifest.canonical_bytes()?;
    if canonical != bytes {
        return Err(ProtocolError::invalid("package manifest JSON is not canonical"));
    }
    let expected = total_length(&manifest, bytes.len(), limits)?;
    for descriptor in &manifest.objects {
        let mut staging = create_staging(descriptor)?;
        stream_object(&mut input, &mut staging, descriptor)?;
        flush_stream(&mut staging)?;
    }
    if read_some(&mut input, &mut [0u8; 1])? != 0 {
        return Err(ProtocolError::invalid("trailing bytes after declared package objects"));
    }
    let digest = input.finish();
    if digest.byte_len != expected {
        return Err(mismatch("read package length differs from metadata"));
    }
    Ok(DecodedPackage { manifest, digest })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_protocol::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

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

    #[derive(Clone, Default)]
    struct SharedSink(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    fn encoded() -> Vec<u8> {
        let mut bytes = vec![];
        write_package(&fixture(), &mut bytes, |_| Ok(Cursor::new(EMPTY_BYTES)),
            PackageCodecLimits::default()).unwrap();
        bytes
    }

    #[test]
    fn minimal_project_roundtrip_preserves_exact_object_once() {
        let manifest = fixture();
        let mut bytes = vec![];
        let mut opened = 0;
        let written = write_package(&manifest, &mut bytes, |_| {
            opened += 1;
            Ok(Cursor::new(EMPTY_BYTES))
        }, PackageCodecLimits::default()).unwrap();
        assert_eq!(opened, 1, "three logical motion references share one payload");
        // Independently computed by Node crypto from a handwritten canonical
        // manifest and the specified wire header, not generated by this codec.
        assert_eq!(manifest.canonical_bytes().unwrap().len(), 1587);
        assert_eq!(written.byte_len, 1652);
        assert_eq!(written.sha256, "1e9f93581dd9e32be46d2dc982f776b803eb2f83d560036ec2e82b083a6508a8");
        assert_eq!(bytes.len(), PACKAGE_HEADER_BYTES + manifest.canonical_bytes().unwrap().len() + 13);
        assert_eq!(&bytes[..12], b"PULSPKG\0\x01\x00\x00\x00");
        assert_eq!(&bytes[bytes.len()-13..], EMPTY_BYTES);
        let sink = SharedSink::default();
        let decoded = read_package(&mut Cursor::new(&bytes), |_| Ok(sink.clone()),
            PackageCodecLimits::default()).unwrap();
        assert_eq!(decoded.digest, written);
        assert_eq!(&*sink.0.lock().unwrap(), EMPTY_BYTES);
        assert_eq!(decoded.manifest.canonical_bytes().unwrap(), manifest.canonical_bytes().unwrap());
    }
    fn decode_discard(bytes: &[u8]) -> Result<DecodedPackage> {
        read_package(&mut Cursor::new(bytes), |_| Ok(io::sink()), PackageCodecLimits::default())
    }

    // Builds hostile fixtures directly from the wire specification. This is not
    // an expected-value oracle; the independent golden digest above is that oracle.
    fn frame_raw_json(json: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"PULSPKG\0\x01\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&(json.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&Sha256::digest(json));
        bytes.extend_from_slice(json);
        bytes.extend_from_slice(payload);
        bytes
    }

    struct ShortReads {
        input: Cursor<Vec<u8>>,
        calls: usize,
    }
    impl Read for ShortReads {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls % 3 == 1 { return Err(io::ErrorKind::Interrupted.into()); }
            let length = bytes.len().min(2);
            self.input.read(&mut bytes[..length])
        }
    }
    struct ShortWrites {
        sink: SharedSink,
        calls: usize,
        flushes: usize,
    }
    impl Write for ShortWrites {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls % 3 == 1 { return Err(io::ErrorKind::Interrupted.into()); }
            self.sink.write(&bytes[..bytes.len().min(2)])
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.flushes == 1 { return Err(io::ErrorKind::Interrupted.into()); }
            Ok(())
        }
    }

    #[test]
    fn short_and_interrupted_io_preserves_the_known_package() {
        let bytes = SharedSink::default();
        let mut output = ShortWrites { sink: bytes.clone(), calls: 0, flushes: 0 };
        let written = write_package(&fixture(), &mut output, |_| Ok(ShortReads {
            input: Cursor::new(EMPTY_BYTES.to_vec()), calls: 0,
        }), PackageCodecLimits::default()).unwrap();
        assert_eq!(written.sha256, "1e9f93581dd9e32be46d2dc982f776b803eb2f83d560036ec2e82b083a6508a8");
        let collected = SharedSink::default();
        let mut input = ShortReads { input: Cursor::new(bytes.0.lock().unwrap().clone()), calls: 0 };
        let decoded = read_package(&mut input, |_| Ok(ShortWrites {
            sink: collected.clone(), calls: 0, flushes: 0,
        }), PackageCodecLimits::default()).unwrap();
        assert_eq!(decoded.digest, written);
        assert_eq!(&*collected.0.lock().unwrap(), EMPTY_BYTES);
    }

    #[test]
    fn every_prefix_truncation_is_rejected() {
        let bytes = encoded();
        for length in 0..bytes.len() {
            assert!(decode_discard(&bytes[..length]).is_err(), "accepted truncated prefix {length}");
        }
    }

    #[test]
    fn trailing_bytes_unknown_version_flags_and_magic_are_rejected() {
        let mut bytes = encoded(); bytes.push(0);
        assert!(decode_discard(&bytes).is_err());
        let mut bytes = encoded(); bytes[8] = 2;
        assert_eq!(decode_discard(&bytes).unwrap_err().code, ErrorCode::Unsupported);
        let mut bytes = encoded(); bytes[10] = 1;
        assert_eq!(decode_discard(&bytes).unwrap_err().code, ErrorCode::Unsupported);
        let mut bytes = encoded(); bytes[0] = b'X';
        assert_eq!(decode_discard(&bytes).unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn oversized_declared_metadata_is_rejected_before_payload_read_or_staging() {
        struct HeaderOnly { bytes: Cursor<Vec<u8>> }
        impl Read for HeaderOnly {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                assert!(self.bytes.position() < PACKAGE_HEADER_BYTES as u64,
                    "oversized metadata must not be read or allocated");
                self.bytes.read(out)
            }
        }
        let mut header = encoded()[..PACKAGE_HEADER_BYTES].to_vec();
        header[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        let result = read_package(&mut HeaderOnly { bytes: Cursor::new(header) },
            |_| -> Result<io::Sink> { panic!("must not stage oversized input") },
            PackageCodecLimits { max_package_bytes: u64::MAX });
        assert_eq!(result.unwrap_err().code, ErrorCode::ResourceExhausted);
    }

    #[test]
    fn complete_package_budget_is_checked_before_object_effects() {
        let bytes = encoded();
        let limits = PackageCodecLimits { max_package_bytes: bytes.len() as u64 - 1 };
        let read = read_package(&mut Cursor::new(&bytes),
            |_| -> Result<io::Sink> { panic!("must not stage an over-budget object") }, limits);
        assert_eq!(read.unwrap_err().code, ErrorCode::ResourceExhausted);
        let mut output = vec![];
        let write = write_package(&fixture(), &mut output,
            |_| -> Result<Cursor<&[u8]>> { panic!("must not open an over-budget object") }, limits);
        assert_eq!(write.unwrap_err().code, ErrorCode::ResourceExhausted);
        assert!(output.is_empty());
    }

    #[test]
    fn manifest_and_object_corruption_cannot_return_success() {
        let mut bytes = encoded(); bytes[PACKAGE_HEADER_BYTES + 20] ^= 1;
        assert_eq!(decode_discard(&bytes).unwrap_err().code, ErrorCode::DependencyMismatch);
        let mut bytes = encoded(); let last = bytes.len()-1; bytes[last] ^= 1;
        assert_eq!(decode_discard(&bytes).unwrap_err().code, ErrorCode::DependencyMismatch);
    }

    #[test]
    fn noncanonical_json_and_unknown_authority_or_path_fields_fail_before_staging() {
        let pretty = serde_json::to_vec_pretty(&fixture()).unwrap();
        assert_eq!(decode_discard(&frame_raw_json(&pretty, EMPTY_BYTES)).unwrap_err().code,
            ErrorCode::InvalidRequest);
        for field in ["sql", "grants", "sessions", "script", "destination_path"] {
            let mut value = serde_json::to_value(fixture()).unwrap();
            value[field] = serde_json::json!("untrusted");
            let bytes = frame_raw_json(&serde_json::to_vec(&value).unwrap(), EMPTY_BYTES);
            let result = read_package(&mut Cursor::new(bytes),
                |_| -> Result<io::Sink> { panic!("must not stage an authority-bearing manifest") },
                PackageCodecLimits::default());
            assert!(result.is_err(), "accepted field {field}");
        }
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["objects"][0]["path"] = serde_json::json!("../../outside");
        assert!(decode_discard(&frame_raw_json(&serde_json::to_vec(&value).unwrap(), EMPTY_BYTES)).is_err());
    }

    #[test]
    fn duplicate_and_unreferenced_objects_are_rejected_before_object_open() {
        let mut duplicate = fixture();
        duplicate.objects.push(duplicate.objects[0].clone());
        duplicate.counts = duplicate.record_counts().unwrap();
        let mut unreferenced = fixture();
        unreferenced.objects.push(PackageObjectDescriptor {
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            byte_len: 0, roles: vec![PackageObjectRole::SourceSnapshot],
        });
        unreferenced.counts = unreferenced.record_counts().unwrap();
        for manifest in [duplicate, unreferenced] {
            let mut output = vec![];
            assert!(write_package(&manifest, &mut output,
                |_| -> Result<Cursor<&[u8]>> { panic!("invalid catalog must not open objects") },
                PackageCodecLimits::default()).is_err());
            assert!(output.is_empty());
            let raw = serde_json::to_vec(&manifest).unwrap();
            assert!(read_package(&mut Cursor::new(frame_raw_json(&raw, EMPTY_BYTES)),
                |_| -> Result<io::Sink> { panic!("invalid catalog must not stage objects") },
                PackageCodecLimits::default()).is_err());
        }
    }

    #[test]
    fn encoder_rejects_short_long_and_changed_source_objects() {
        for source in [
            EMPTY_BYTES[..12].to_vec(),
            [EMPTY_BYTES, b"extra"].concat(),
            b"{\"tracks\":[ ]}".to_vec(),
        ] {
            let result = write_package(&fixture(), &mut io::sink(), |_| Ok(Cursor::new(source.clone())),
                PackageCodecLimits::default());
            assert_eq!(result.unwrap_err().code, ErrorCode::DependencyMismatch);
        }
    }

    struct FailingSink { remaining: usize, fail_flush: bool }
    impl Write for FailingSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 { return Err(io::Error::other("injected write failure")); }
            let length = self.remaining.min(bytes.len());
            self.remaining -= length;
            Ok(length)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush { Err(io::Error::other("injected flush failure")) } else { Ok(()) }
        }
    }

    #[test]
    fn output_or_staging_write_and_flush_failures_never_succeed() {
        for (remaining, fail_flush) in [(4, false), (usize::MAX, true)] {
            let result = write_package(&fixture(), &mut FailingSink { remaining, fail_flush },
                |_| Ok(Cursor::new(EMPTY_BYTES)), PackageCodecLimits::default());
            assert_eq!(result.unwrap_err().code, ErrorCode::Unavailable);
            let result = read_package(&mut Cursor::new(encoded()),
                |_| Ok(FailingSink { remaining, fail_flush }), PackageCodecLimits::default());
            assert_eq!(result.unwrap_err().code, ErrorCode::Unavailable);
        }
    }

    #[test]
    fn late_failure_leaves_only_provisional_bytes_and_no_decoded_package() {
        let mut bytes = encoded(); bytes.push(0);
        let sink = SharedSink::default();
        let result = read_package(&mut Cursor::new(bytes), |_| Ok(sink.clone()),
            PackageCodecLimits::default());
        assert!(result.is_err());
        assert_eq!(&*sink.0.lock().unwrap(), EMPTY_BYTES,
            "earlier sink bytes are not evidence of full package success");
    }

    #[test]
    fn large_exact_legacy_bytes_stream_with_fixed_object_buffer() {
        struct BoundedRead { input: Cursor<Vec<u8>> }
        impl Read for BoundedRead {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                assert!(bytes.len() <= 64 * 1024, "object read request exceeded fixed buffer");
                self.input.read(bytes)
            }
        }
        let mut raw = vec![b' '; 3 * 64 * 1024 + 1];
        raw.extend_from_slice(EMPTY_BYTES);
        let sha256 = format!("{:x}", Sha256::digest(&raw));
        let mut manifest = fixture();
        manifest.legacy_cells.push(PackageLegacyCell {
            owner: PackageLegacyOwner::Projects, row_key: "p".into(),
            object: PackageObjectRef { sha256: sha256.clone(), byte_len: raw.len() as u64 },
        });
        manifest.objects.push(PackageObjectDescriptor {
            sha256: sha256.clone(), byte_len: raw.len() as u64,
            roles: vec![PackageObjectRole::LegacyMotionBytes],
        });
        manifest.objects.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        manifest.counts = manifest.record_counts().unwrap();
        let mut output = vec![];
        let written = write_package(&manifest, &mut output, |object| Ok(BoundedRead {
            input: Cursor::new(if object.sha256 == sha256 { raw.clone() } else { EMPTY_BYTES.to_vec() }),
        }), PackageCodecLimits::default()).unwrap();
        let saved = SharedSink::default();
        let decoded = read_package(&mut BoundedRead { input: Cursor::new(output) },
            |object| Ok(if object.sha256 == sha256 { saved.clone() } else { SharedSink::default() }),
            PackageCodecLimits::default()).unwrap();
        assert_eq!(decoded.digest, written);
        assert_eq!(*saved.0.lock().unwrap(), raw, "archival whitespace is exact evidence");
    }

}
