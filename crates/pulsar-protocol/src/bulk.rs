//! Separate bulk-channel contracts. Framing is not authorization: the broker
//! rechecks actor/grant/epoch/range and replay bytes before every chunk effect.
use crate::{
    bounded_text, read_artifact_chunk, read_bulk_header, validate_opaque_id, write_artifact_chunk,
    write_bulk_header, DeadlineStream, ErrorCode, LocalStream, ProtocolError, SessionId,
    TransferDirection, TransferId, TransferLease, TransportError, MAX_ARTIFACT_CHUNK_BYTES,
    PROTOCOL_VERSION,
};
use interprocess::local_socket::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

pub const MAX_BULK_HEADER_BYTES: usize = 16 * 1024;
pub type BulkError = TransportError;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkHandshake {
    pub version: u16,
    pub session: SessionId,
    pub auth_token: String,
    pub lease_id: TransferId,
    pub engine_epoch: String,
}
impl std::fmt::Debug for BulkHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkHandshake")
            .field("version", &self.version)
            .field("session", &self.session)
            .field("auth_token", &"[REDACTED]")
            .field("lease_id", &self.lease_id)
            .field("engine_epoch", &self.engine_epoch)
            .finish()
    }
}
impl BulkHandshake {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::new(
                ErrorCode::Unsupported,
                "unsupported bulk protocol version",
            ));
        }
        bounded_text(&self.auth_token, 4096, "bulk authentication token")?;
        validate_opaque_id(&self.engine_epoch, "engine epoch")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum BulkOperation {
    Upload { offset: u64, byte_len: u32 },
    Download { offset: u64, byte_len: u32 },
}
impl BulkOperation {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let (offset, len) = match self {
            Self::Upload { offset, byte_len } | Self::Download { offset, byte_len } => {
                (*offset, *byte_len)
            }
        };
        if len == 0
            || len as usize > MAX_ARTIFACT_CHUNK_BYTES
            || offset.checked_add(u64::from(len)).is_none()
        {
            return Err(ProtocolError::invalid("invalid bulk chunk range"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BulkReply {
    Ready { lease: TransferLease },
    Uploaded { accepted_prefix: u64 },
    Download { offset: u64, byte_len: u32 },
    Error(ProtocolError),
}

/// One authenticated lease per connection. Each operation has one whole-call
/// deadline, including all partial header/body reads and writes. Bad framing,
/// server errors or mismatched replies poison the connection. The caller must
/// reconcile ambiguous progress with TransferStatus instead of blind replay.
/// Connection setup is separately bounded by the same timeout on Unix, through
/// the shared connector. A caller with a whole-transfer deadline must pass its
/// remaining budget to each connection or operation, not reset a transfer budget.
pub struct BulkClient {
    stream: Option<LocalStream>,
    timeout: Duration,
    lease: Option<TransferLease>,
    transfer_deadline: Option<Instant>,
}
impl BulkClient {
    /// Apply one absolute budget across connection-following handshake/chunk
    /// operations. Subsequent calls may shorten, but never extend, that budget.
    pub fn set_deadline(&mut self, deadline: Instant) -> Result<(), BulkError> {
        let deadline = self
            .transfer_deadline
            .map_or(deadline, |old| old.min(deadline));
        if deadline <= Instant::now() {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "bulk transfer deadline exceeded").into(),
            );
        }
        self.transfer_deadline = Some(deadline);
        Ok(())
    }
    fn operation_deadline(&self) -> Result<Instant, BulkError> {
        let deadline = deadline(self.timeout)?;
        let deadline = self
            .transfer_deadline
            .map_or(deadline, |end| end.min(deadline));
        if deadline <= Instant::now() {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "bulk transfer deadline exceeded").into(),
            );
        }
        Ok(deadline)
    }

    pub fn connect(path: impl AsRef<Path>, timeout: Duration) -> Result<Self, BulkError> {
        if timeout.is_zero() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "zero bulk timeout").into());
        }
        let stream = crate::transport::connect_local(path.as_ref(), timeout)?;
        stream.set_recv_timeout(Some(timeout))?;
        stream.set_send_timeout(Some(timeout))?;
        Ok(Self {
            stream: Some(stream),
            timeout,
            lease: None,
            transfer_deadline: None,
        })
    }
    pub fn handshake(&mut self, handshake: &BulkHandshake) -> Result<TransferLease, BulkError> {
        handshake.validate()?;
        if self.lease.is_some() {
            return Err(ProtocolError::invalid("bulk connection already authenticated").into());
        }
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let mut io = DeadlineStream::new(&mut stream, self.operation_deadline()?);
        write_bulk_header(&mut io, handshake)?;
        let reply: BulkReply = read_bulk_header(&mut io)?;
        let lease = match reply {
            BulkReply::Ready { lease } => lease,
            BulkReply::Error(e) => return Err(e.into()),
            _ => return Err(TransportError::ResponseMismatch),
        };
        lease.validate()?;
        if lease.lease_id != handshake.lease_id || lease.engine_epoch != handshake.engine_epoch {
            return Err(TransportError::ResponseMismatch);
        }
        self.lease = Some(lease.clone());
        self.stream = Some(stream);
        Ok(lease)
    }
    pub fn upload(&mut self, offset: u64, bytes: &[u8]) -> Result<u64, BulkError> {
        let byte_len = u32::try_from(bytes.len())
            .map_err(|_| ProtocolError::invalid("bulk chunk length overflow"))?;
        let operation = BulkOperation::Upload { offset, byte_len };
        operation.validate()?;
        let lease = self.check_range(TransferDirection::Upload, offset, byte_len)?;
        let end = offset + u64::from(byte_len);
        // An exact retry may cover an already-acknowledged prefix, but a new
        // chunk cannot jump ahead. The engine compares retry bytes, not only offsets.
        if offset > lease.accepted_prefix
            || (offset < lease.accepted_prefix && end > lease.accepted_prefix)
        {
            return Err(ProtocolError::invalid(
                "upload offset is not a sequential chunk or acknowledged retry",
            )
            .into());
        }
        let expected_prefix = lease.accepted_prefix.max(end);
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let mut io = DeadlineStream::new(&mut stream, self.operation_deadline()?);
        write_bulk_header(&mut io, &operation)?;
        write_artifact_chunk(&mut io, bytes)?;
        let reply: BulkReply = read_bulk_header(&mut io)?;
        let accepted_prefix = match reply {
            BulkReply::Uploaded { accepted_prefix } => accepted_prefix,
            BulkReply::Error(e) => return Err(e.into()),
            _ => return Err(TransportError::ResponseMismatch),
        };
        if accepted_prefix != expected_prefix {
            return Err(TransportError::ResponseMismatch);
        }
        if let Some(lease) = self.lease.as_mut() {
            lease.accepted_prefix = accepted_prefix;
        }
        self.stream = Some(stream);
        Ok(accepted_prefix)
    }
    pub fn download(&mut self, offset: u64, byte_len: u32) -> Result<Vec<u8>, BulkError> {
        let operation = BulkOperation::Download { offset, byte_len };
        operation.validate()?;
        self.check_range(TransferDirection::Download, offset, byte_len)?;
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let mut io = DeadlineStream::new(&mut stream, self.operation_deadline()?);
        write_bulk_header(&mut io, &operation)?;
        let reply: BulkReply = read_bulk_header(&mut io)?;
        match reply {
            BulkReply::Download {
                offset: actual_offset,
                byte_len: actual_len,
            } if actual_offset == offset && actual_len == byte_len => {}
            BulkReply::Error(e) => return Err(e.into()),
            _ => return Err(TransportError::ResponseMismatch),
        }
        let bytes = read_artifact_chunk(&mut io)?;
        if bytes.len() != byte_len as usize {
            return Err(TransportError::ResponseMismatch);
        }
        if let Some(lease) = self.lease.as_mut() {
            if offset == lease.offset + lease.accepted_prefix {
                lease.accepted_prefix += u64::from(byte_len);
            }
        }
        self.stream = Some(stream);
        Ok(bytes)
    }
    fn check_range(
        &self,
        direction: TransferDirection,
        offset: u64,
        byte_len: u32,
    ) -> Result<&TransferLease, BulkError> {
        if self.stream.is_none() {
            return Err(TransportError::ConnectionPoisoned);
        }
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| ProtocolError::invalid("bulk handshake required"))?;
        let end = offset
            .checked_add(u64::from(byte_len))
            .ok_or_else(|| ProtocolError::invalid("bulk range overflow"))?;
        if lease.direction != direction
            || offset < lease.offset
            || end > lease.offset + lease.byte_len
        {
            return Err(ProtocolError::invalid("chunk outside lease direction or range").into());
        }
        Ok(lease)
    }
}
fn deadline(timeout: Duration) -> Result<Instant, BulkError> {
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bulk deadline overflow").into())
}
