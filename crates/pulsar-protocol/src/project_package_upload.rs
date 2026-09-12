//! Dedicated authenticated clone-import data lane. Upload acknowledgements
//! describe accepted current-epoch staging, not successful import publication.
use crate::{
    project_package_import_operations::import_digest, read_bulk_header, write_artifact_chunk,
    write_bulk_header, BulkError, DeadlineStream, ErrorCode, LocalStream, PackageImportOperationId,
    PackageOperationId, PackageUploadChunkReceipt, PackageUploadId, PackageUploadLease,
    ProtocolError, SessionId, TransportError, MAX_PACKAGE_UPLOAD_CHUNK_BYTES, PROTOCOL_VERSION,
};
use interprocess::local_socket::traits::Stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt, io,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageUploadChannel {
    ProjectPackageUpload,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageUploadHandshake {
    pub version: u16,
    pub channel: PackageUploadChannel,
    pub session: SessionId,
    pub auth_token: String,
    pub lease_id: PackageUploadId,
    pub engine_epoch: String,
    pub operation_id: PackageImportOperationId,
    pub generation: u64,
    pub package_sha256: String,
}
impl fmt::Debug for PackageUploadHandshake {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PackageUploadHandshake")
            .field("version", &self.version)
            .field("channel", &self.channel)
            .field("session", &self.session)
            .field("auth_token", &"<redacted>")
            .field("lease_id", &self.lease_id)
            .field("engine_epoch", &self.engine_epoch)
            .field("operation_id", &self.operation_id)
            .field("generation", &self.generation)
            .finish()
    }
}
impl PackageUploadHandshake {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                ErrorCode::Unsupported,
                "unsupported package upload protocol version",
            ));
        }
        PackageOperationId::new(self.engine_epoch.clone())?;
        import_digest(&self.package_sha256)?;
        if self.generation == 0
            || self.auth_token.is_empty()
            || self.auth_token.len() > 4096
            || self.auth_token.contains('\0')
        {
            return Err(ProtocolError::invalid(
                "invalid upload generation or authentication token",
            ));
        }
        Ok(())
    }
}

pub type PackageUploadRequest = PackageUploadChunkReceipt;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PackageUploadReply {
    Ready {
        lease: PackageUploadLease,
    },
    Uploaded {
        offset: u64,
        byte_len: u32,
        sha256: String,
        next_offset: u64,
    },
    Error(ProtocolError),
}

pub struct PackageUploadClient {
    stream: Option<LocalStream>,
    timeout: Duration,
    deadline: Option<Instant>,
    lease: Option<PackageUploadLease>,
}
impl PackageUploadClient {
    pub fn connect(path: impl AsRef<Path>, timeout: Duration) -> Result<Self, BulkError> {
        if timeout.is_zero() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "zero package upload timeout").into(),
            );
        }
        let stream = crate::transport::connect_local(path.as_ref(), timeout)?;
        stream.set_recv_timeout(Some(timeout))?;
        stream.set_send_timeout(Some(timeout))?;
        Ok(Self {
            stream: Some(stream),
            timeout,
            deadline: None,
            lease: None,
        })
    }
    /// Bound connection setup without permanently shortening later RPCs.
    pub fn connect_until(
        path: impl AsRef<Path>,
        ordinary_timeout: Duration,
        connection_deadline: Instant,
    ) -> Result<Self, BulkError> {
        if ordinary_timeout.is_zero() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "zero package upload timeout").into(),
            );
        }
        let remaining = connection_deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "package upload connection deadline expired",
                )
            })?;
        let mut client = Self::connect(path, ordinary_timeout.min(remaining))?;
        if Instant::now() >= connection_deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "package upload connection deadline expired",
            )
            .into());
        }
        client.timeout = ordinary_timeout;
        Ok(client)
    }

    pub fn set_deadline(&mut self, deadline: Instant) -> Result<(), BulkError> {
        let deadline = self.deadline.map_or(deadline, |old| old.min(deadline));
        self.deadline = Some(deadline);
        if deadline <= Instant::now() {
            self.stream = None;
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "package upload deadline expired").into(),
            );
        }
        Ok(())
    }
    fn operation_deadline(&self) -> Result<Instant, BulkError> {
        let relative = Instant::now().checked_add(self.timeout).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "package upload timeout overflow",
            )
        })?;
        let deadline = self
            .deadline
            .map_or(relative, |absolute| relative.min(absolute));
        if deadline <= Instant::now() {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "package upload deadline expired").into(),
            );
        }
        Ok(deadline)
    }
    pub fn handshake(
        &mut self,
        request: &PackageUploadHandshake,
    ) -> Result<PackageUploadLease, BulkError> {
        self.handshake_with_deadline(request, None)
    }

    /// Bound only this handshake. Successful admission retains the original
    /// transfer deadline and ordinary per-operation timeout for later chunks.
    pub fn handshake_until(
        &mut self,
        request: &PackageUploadHandshake,
        handshake_deadline: Instant,
    ) -> Result<PackageUploadLease, BulkError> {
        self.handshake_with_deadline(request, Some(handshake_deadline))
    }

    fn handshake_with_deadline(
        &mut self,
        request: &PackageUploadHandshake,
        handshake_deadline: Option<Instant>,
    ) -> Result<PackageUploadLease, BulkError> {
        request.validate()?;
        if self.lease.is_some() {
            return Err(ProtocolError::invalid("upload connection is already bound").into());
        }
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let ordinary_deadline = self.operation_deadline()?;
        let deadline =
            handshake_deadline.map_or(ordinary_deadline, |bound| bound.min(ordinary_deadline));
        let mut io = DeadlineStream::new(&mut stream, deadline);
        write_bulk_header(&mut io, request)?;
        let reply: PackageUploadReply = read_bulk_header(&mut io)?;
        let lease = match reply {
            PackageUploadReply::Ready { lease } => lease,
            PackageUploadReply::Error(error) => return Err(error.into()),
            _ => return Err(TransportError::ResponseMismatch),
        };
        lease.validate()?;
        if lease.lease_id != request.lease_id
            || lease.engine_epoch != request.engine_epoch
            || lease.operation_id != request.operation_id
            || lease.generation != request.generation
            || lease.package.sha256 != request.package_sha256
        {
            return Err(TransportError::ResponseMismatch);
        }
        let expiry = Instant::now()
            .checked_add(Duration::from_millis(lease.expires_after_ms))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "upload lease lifetime overflow",
                )
            })?;
        self.set_deadline(expiry)?;
        self.lease = Some(lease.clone());
        self.stream = Some(stream);
        Ok(lease)
    }
    pub fn upload(
        &mut self,
        offset: u64,
        bytes: &[u8],
    ) -> Result<PackageUploadChunkReceipt, BulkError> {
        if bytes.is_empty() || bytes.len() > MAX_PACKAGE_UPLOAD_CHUNK_BYTES as usize {
            return Err(ProtocolError::invalid("upload chunk exceeds its bound").into());
        }
        let request = PackageUploadChunkReceipt {
            offset,
            byte_len: bytes.len() as u32,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        };
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| ProtocolError::invalid("upload handshake required"))?;
        let replay = lease.validate_chunk(&request)?;
        let expected_next = if replay {
            lease.next_offset
        } else {
            request.end()?
        };
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let mut io = DeadlineStream::new(&mut stream, self.operation_deadline()?);
        write_bulk_header(&mut io, &request)?;
        write_artifact_chunk(&mut io, bytes)?;
        let reply: PackageUploadReply = read_bulk_header(&mut io)?;
        match reply {
            PackageUploadReply::Uploaded {
                offset,
                byte_len,
                sha256,
                next_offset,
            } if offset == request.offset
                && byte_len == request.byte_len
                && sha256 == request.sha256
                && next_offset == expected_next =>
            {
                ()
            }
            PackageUploadReply::Error(error) => return Err(error.into()),
            _ => return Err(TransportError::ResponseMismatch),
        }
        let lease = self.lease.as_mut().expect("validated upload lease");
        lease.next_offset = expected_next;
        lease.replay = Some(request.clone());
        self.stream = Some(stream);
        Ok(request)
    }
}
