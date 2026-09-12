//! Package-only authenticated downloads sharing framing, not motion lease state.
use crate::*;
use interprocess::local_socket::traits::Stream as _;
use serde::{Deserialize, Serialize};
use std::{
    fmt, io,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageBulkChannel {
    ProjectPackageDownload,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageBulkHandshake {
    pub version: u16,
    pub channel: PackageBulkChannel,
    pub session: SessionId,
    pub auth_token: String,
    pub lease_id: PackageDownloadId,
    pub engine_epoch: String,
    pub operation_id: PackageOperationId,
    pub artifact_sha256: String,
}
impl fmt::Debug for PackageBulkHandshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackageBulkHandshake")
            .field("version", &self.version)
            .field("channel", &self.channel)
            .field("session", &self.session)
            .field("auth_token", &"[REDACTED]")
            .field("lease_id", &self.lease_id)
            .field("engine_epoch", &self.engine_epoch)
            .field("operation_id", &self.operation_id)
            .field("artifact_sha256", &self.artifact_sha256)
            .finish()
    }
}
impl PackageBulkHandshake {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::unsupported(
                "unsupported package bulk protocol",
            ));
        }
        crate::project_package_operations::bounded_text(&self.auth_token, 4096, true)?;
        crate::project_package_operations::bounded_text(&self.engine_epoch, 128, true)?;
        validate_sha256(&self.artifact_sha256)?;
        if serde_json::to_vec(self)
            .map_err(|_| ProtocolError::invalid("invalid package handshake"))?
            .len()
            > 16 * 1024
        {
            return Err(ProtocolError::invalid(
                "package handshake exceeds bounded header",
            ));
        }
        Ok(())
    }
}
/// The package marker is mandatory. Existing motion handshakes remain unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BulkHandshakeEnvelope {
    PackageUpload(crate::PackageUploadHandshake),
    Package(PackageBulkHandshake),
    Motion(BulkHandshake),
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageBulkRequest {
    pub offset: u64,
    pub byte_len: u32,
}
impl PackageBulkRequest {
    pub fn range(self) -> PackageChunkRange {
        PackageChunkRange {
            offset: self.offset,
            byte_len: self.byte_len,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PackageBulkReply {
    Ready {
        lease: PackageDownloadLease,
    },
    Download {
        offset: u64,
        byte_len: u32,
        next_offset: u64,
    },
    Error(ProtocolError),
}

/// One authenticated package lease per connection. Remote/protocol/I/O failures
/// poison the stream; invalid local ranges do not send bytes or advance cursors.
pub struct PackageBulkClient {
    stream: Option<LocalStream>,
    timeout: Duration,
    lease: Option<PackageDownloadLease>,
    deadline: Option<Instant>,
}
impl PackageBulkClient {
    pub fn connect(path: impl AsRef<Path>, timeout: Duration) -> Result<Self, BulkError> {
        let stream = crate::transport::connect_local(path.as_ref(), timeout)?;
        stream.set_recv_timeout(Some(timeout))?;
        stream.set_send_timeout(Some(timeout))?;
        Ok(Self {
            stream: Some(stream),
            timeout,
            lease: None,
            deadline: None,
        })
    }
    pub fn set_deadline(&mut self, deadline: Instant) -> Result<(), BulkError> {
        if deadline <= Instant::now() {
            self.stream = None;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "package transfer deadline expired",
            )
            .into());
        }
        self.deadline = Some(self.deadline.map_or(deadline, |old| old.min(deadline)));
        Ok(())
    }
    fn operation_deadline(&self) -> Result<Instant, BulkError> {
        let now = Instant::now();
        let deadline = now.checked_add(self.timeout).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "package timeout overflow")
        })?;
        let deadline = self
            .deadline
            .map_or(deadline, |overall| overall.min(deadline));
        if deadline <= now {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "package transfer deadline expired",
            )
            .into());
        }
        Ok(deadline)
    }
    pub fn handshake(
        &mut self,
        request: &PackageBulkHandshake,
    ) -> Result<PackageDownloadLease, BulkError> {
        request.validate()?;
        if self.lease.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "package connection is already authenticated",
            )
            .into());
        }
        let deadline = self.operation_deadline()?;
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let mut io = DeadlineStream::new(&mut stream, deadline);
        write_bulk_header(&mut io, request)?;
        let reply: PackageBulkReply = read_bulk_header(&mut io)?;
        let lease = match reply {
            PackageBulkReply::Ready { lease } => lease,
            PackageBulkReply::Error(error) => return Err(error.into()),
            _ => return Err(TransportError::ResponseMismatch),
        };
        lease.validate()?;
        if lease.lease_id != request.lease_id
            || lease.engine_epoch != request.engine_epoch
            || lease.operation_id != request.operation_id
            || lease.artifact.sha256 != request.artifact_sha256
        {
            return Err(TransportError::ResponseMismatch);
        }
        let expires = Instant::now()
            .checked_add(Duration::from_millis(lease.expires_after_ms))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "package lease lifetime overflow",
                )
            })?;
        self.deadline = Some(self.deadline.map_or(expires, |old| old.min(expires)));
        self.stream = Some(stream);
        self.lease = Some(lease.clone());
        Ok(lease)
    }
    pub fn download(&mut self, offset: u64, byte_len: u32) -> Result<Vec<u8>, BulkError> {
        let lease = self.lease.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "package handshake required")
        })?;
        let range = PackageChunkRange { offset, byte_len };
        let replay = lease.validate_chunk(range)?;
        let expected_next = if replay {
            lease.next_offset
        } else {
            range.end()?
        };
        let deadline = self.operation_deadline()?;
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let mut io = DeadlineStream::new(&mut stream, deadline);
        write_bulk_header(&mut io, &PackageBulkRequest { offset, byte_len })?;
        match read_bulk_header::<_, PackageBulkReply>(&mut io)? {
            PackageBulkReply::Download {
                offset: actual,
                byte_len: actual_len,
                next_offset,
            } if actual == offset && actual_len == byte_len && next_offset == expected_next => {}
            PackageBulkReply::Error(error) => return Err(error.into()),
            _ => return Err(TransportError::ResponseMismatch),
        }
        let bytes = read_artifact_chunk(&mut io)?;
        if bytes.len() != byte_len as usize {
            return Err(TransportError::ResponseMismatch);
        }
        let lease = self.lease.as_mut().expect("authenticated package lease");
        if !replay {
            lease.next_offset = expected_next;
            lease.replay = Some(range);
        }
        self.stream = Some(stream);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn handshake() -> PackageBulkHandshake {
        PackageBulkHandshake {
            version: PROTOCOL_VERSION,
            channel: PackageBulkChannel::ProjectPackageDownload,
            session: SessionId::new("s").unwrap(),
            auth_token: "private-auth".into(),
            lease_id: PackageDownloadId::new("l").unwrap(),
            engine_epoch: "epoch".into(),
            operation_id: PackageOperationId::new("o").unwrap(),
            artifact_sha256: "a".repeat(64),
        }
    }
    #[test]
    fn package_handshake_is_tagged_scoped_and_secret_redacted() {
        let h = handshake();
        h.validate().unwrap();
        assert!(!format!("{h:?}").contains("private-auth"));
        let bytes = serde_json::to_vec(&h).unwrap();
        assert!(matches!(
            serde_json::from_slice::<BulkHandshakeEnvelope>(&bytes).unwrap(),
            BulkHandshakeEnvelope::Package(_)
        ));
        assert!(serde_json::from_slice::<BulkHandshake>(&bytes).is_err());
    }
    #[test]
    fn malformed_old_version_unknown_fields_and_fake_channel_reject() {
        let mut h = handshake();
        h.version = 1;
        assert!(h.validate().is_err());
        for key in ["path", "upload", "project_memory"] {
            let mut value = serde_json::to_value(handshake()).unwrap();
            value[key] = serde_json::json!("bad");
            assert!(serde_json::from_value::<BulkHandshakeEnvelope>(value).is_err());
        }
        let mut value = serde_json::to_value(handshake()).unwrap();
        value["channel"] = serde_json::json!("motion");
        assert!(serde_json::from_value::<BulkHandshakeEnvelope>(value).is_err());
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    use super::*;
    use std::{
        fs,
        os::unix::net::{UnixListener, UnixStream},
        path::PathBuf,
        thread,
        time::SystemTime,
    };
    struct SocketDir(PathBuf);
    impl Drop for SocketDir {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0.join("bulk.sock"));
            let _ = fs::remove_dir(&self.0);
        }
    }
    fn fixture() -> (SocketDir, UnixListener, PackageDownloadLease) {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pulsar-package-bulk-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("bulk.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let artifact = PackageArtifactDescriptor {
            format_version: 1,
            sha256: "a".repeat(64),
            byte_len: 1024,
            manifest_sha256: "b".repeat(64),
            project_id: ProjectId::new("p").unwrap(),
            captured_revision: RevisionId::new(0),
            captured_event_cursor: 0,
            object_count: 1,
            verification: PackageReadyVerification::AllDeclaredObjectsVerified,
        };
        let lease = PackageDownloadLease {
            lease_id: PackageDownloadId::new("l").unwrap(),
            engine_epoch: "e".into(),
            operation_id: PackageOperationId::new("o").unwrap(),
            artifact,
            offset: 0,
            byte_len: 4,
            next_offset: 0,
            replay: None,
            expires_after_ms: 2000,
            bulk_endpoint: path,
        };
        (SocketDir(dir), listener, lease)
    }
    fn hello(lease: &PackageDownloadLease) -> PackageBulkHandshake {
        PackageBulkHandshake {
            version: PROTOCOL_VERSION,
            channel: PackageBulkChannel::ProjectPackageDownload,
            session: SessionId::new("s").unwrap(),
            auth_token: "test-auth".into(),
            lease_id: lease.lease_id.clone(),
            engine_epoch: lease.engine_epoch.clone(),
            operation_id: lease.operation_id.clone(),
            artifact_sha256: lease.artifact.sha256.clone(),
        }
    }
    fn server_hello(listener: UnixListener, lease: PackageDownloadLease) -> UnixStream {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request: PackageBulkHandshake = read_bulk_header(&mut stream).unwrap();
        request.validate().unwrap();
        write_bulk_header(&mut stream, &PackageBulkReply::Ready { lease }).unwrap();
        stream
    }
    #[test]
    fn socket_roundtrip_exact_replay_and_local_rejection_preserve_connection() {
        let (_dir, listener, lease) = fixture();
        let server_lease = lease.clone();
        let server = thread::spawn(move || {
            let mut stream = server_hello(listener, server_lease);
            for (offset, next, data) in [(0, 2, b"ab"), (0, 2, b"ab"), (2, 4, b"cd"), (2, 4, b"cd")]
            {
                let request: PackageBulkRequest = read_bulk_header(&mut stream).unwrap();
                assert_eq!(request.offset, offset);
                assert_eq!(request.byte_len, 2);
                write_bulk_header(
                    &mut stream,
                    &PackageBulkReply::Download {
                        offset,
                        byte_len: 2,
                        next_offset: next,
                    },
                )
                .unwrap();
                write_artifact_chunk(&mut stream, data).unwrap();
            }
        });
        let mut client =
            PackageBulkClient::connect(&lease.bulk_endpoint, Duration::from_secs(2)).unwrap();
        client.handshake(&hello(&lease)).unwrap();
        assert_eq!(client.download(0, 2).unwrap(), b"ab");
        assert_eq!(client.download(0, 2).unwrap(), b"ab");
        assert!(client.download(3, 2).is_err());
        assert_eq!(client.download(2, 2).unwrap(), b"cd");
        assert_eq!(client.download(2, 2).unwrap(), b"cd");
        drop(client);
        server.join().unwrap();
    }
    #[test]
    fn wrong_response_cursor_poisons_socket_before_body() {
        let (_dir, listener, lease) = fixture();
        let server_lease = lease.clone();
        let server = thread::spawn(move || {
            let mut stream = server_hello(listener, server_lease);
            let _: PackageBulkRequest = read_bulk_header(&mut stream).unwrap();
            write_bulk_header(
                &mut stream,
                &PackageBulkReply::Download {
                    offset: 0,
                    byte_len: 2,
                    next_offset: 99,
                },
            )
            .unwrap();
        });
        let mut client =
            PackageBulkClient::connect(&lease.bulk_endpoint, Duration::from_secs(2)).unwrap();
        client.handshake(&hello(&lease)).unwrap();
        assert!(matches!(
            client.download(0, 2),
            Err(TransportError::ResponseMismatch)
        ));
        assert!(matches!(
            client.download(0, 2),
            Err(TransportError::ConnectionPoisoned)
        ));
        server.join().unwrap();
    }
    #[test]
    fn truncated_body_poisons_and_absolute_deadline_cannot_be_extended() {
        let (_dir, listener, lease) = fixture();
        let server_lease = lease.clone();
        let server = thread::spawn(move || {
            let mut stream = server_hello(listener, server_lease);
            let _: PackageBulkRequest = read_bulk_header(&mut stream).unwrap();
            write_bulk_header(
                &mut stream,
                &PackageBulkReply::Download {
                    offset: 0,
                    byte_len: 2,
                    next_offset: 2,
                },
            )
            .unwrap();
            use std::io::Write;
            stream.write_all(&[0, 0, 0, 2, b'a']).unwrap();
        });
        let mut client =
            PackageBulkClient::connect(&lease.bulk_endpoint, Duration::from_secs(2)).unwrap();
        client.handshake(&hello(&lease)).unwrap();
        assert!(client.download(0, 2).is_err());
        assert!(matches!(
            client.download(0, 2),
            Err(TransportError::ConnectionPoisoned)
        ));
        server.join().unwrap();
        let (_dir, listener, lease) = fixture();
        let server_lease = lease.clone();
        let server = thread::spawn(move || {
            let mut stream = server_hello(listener, server_lease);
            let _: PackageBulkRequest = read_bulk_header(&mut stream).unwrap();
            use std::io::Write;
            stream.write_all(&[0]).unwrap();
            thread::sleep(Duration::from_millis(150));
        });
        let mut client =
            PackageBulkClient::connect(&lease.bulk_endpoint, Duration::from_secs(2)).unwrap();
        client.handshake(&hello(&lease)).unwrap();
        let start = Instant::now();
        client
            .set_deadline(start + Duration::from_millis(40))
            .unwrap();
        client.set_deadline(start + Duration::from_secs(5)).unwrap();
        assert!(client.download(0, 2).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(client.stream.is_none());
        server.join().unwrap();
    }
}
