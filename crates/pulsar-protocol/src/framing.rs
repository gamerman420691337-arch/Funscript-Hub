use crate::{MAX_ARTIFACT_CHUNK_BYTES, MAX_BULK_HEADER_BYTES, MAX_CONTROL_BYTES};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, Read, Write};

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Json(serde_json::Error),
    Protocol(crate::ProtocolError),
    InvalidLength(u32),
    ResponseMismatch,
    ConnectionPoisoned,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "transport I/O: {e}"),
            Self::Json(e) => write!(f, "invalid control JSON: {e}"),
            Self::Protocol(e) => write!(f, "{e}"),
            Self::InvalidLength(n) => write!(f, "invalid frame length: {n}"),
            Self::ResponseMismatch => write!(f, "response version or request identity mismatch"),
            Self::ConnectionPoisoned => write!(f, "connection unusable after a transport failure"),
        }
    }
}
impl std::error::Error for TransportError {}
impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for TransportError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<crate::ProtocolError> for TransportError {
    fn from(e: crate::ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

/// Four-byte unsigned big-endian length followed by exactly that many UTF-8
/// JSON bytes. Reject the length before allocating. The connection owner must
/// enforce a whole-frame deadline; this generic codec cannot interrupt Read.
pub fn read_message<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, TransportError> {
    let bytes = read_frame(reader, MAX_CONTROL_BYTES)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn write_message<W: Write, T: Serialize>(
    writer: &mut W,
    message: &T,
) -> Result<(), TransportError> {
    let mut buffer = BoundedBuffer {
        bytes: Vec::new(),
        limit: MAX_CONTROL_BYTES,
    };
    serde_json::to_writer(&mut buffer, message)?;
    write_frame(writer, &buffer.bytes, MAX_CONTROL_BYTES)
}

fn read_frame<R: Read>(reader: &mut R, limit: usize) -> Result<Vec<u8>, TransportError> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header);
    if size == 0 || size as usize > limit {
        return Err(TransportError::InvalidLength(size));
    }
    let mut bytes = vec![0; size as usize];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_frame<W: Write>(writer: &mut W, bytes: &[u8], limit: usize) -> Result<(), TransportError> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(TransportError::InvalidLength(
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}

/// Dedicated bulk channel codec, never the control connection. The broker must
/// enforce descriptor length, aggregate budget, grant, offset and digest.
pub fn read_artifact_chunk<R: Read>(reader: &mut R) -> Result<Vec<u8>, TransportError> {
    read_frame(reader, MAX_ARTIFACT_CHUNK_BYTES)
}
pub fn write_artifact_chunk<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<(), TransportError> {
    write_frame(writer, bytes, MAX_ARTIFACT_CHUNK_BYTES)
}

struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encoded frame exceeds byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Bulk metadata uses a smaller independent cap; raw chunks follow separately.
pub fn read_bulk_header<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, TransportError> {
    Ok(serde_json::from_slice(&read_frame(
        reader,
        MAX_BULK_HEADER_BYTES,
    )?)?)
}
pub fn write_bulk_header<W: Write, T: Serialize>(
    writer: &mut W,
    header: &T,
) -> Result<(), TransportError> {
    let mut buffer = BoundedBuffer {
        bytes: Vec::new(),
        limit: MAX_BULK_HEADER_BYTES,
    };
    serde_json::to_writer(&mut buffer, header)?;
    write_frame(writer, &buffer.bytes, MAX_BULK_HEADER_BYTES)
}

pub enum RequestFrame {
    Current(crate::Request),
    Rejected {
        request_id: crate::RequestId,
        error: crate::ProtocolError,
    },
}
/// Read the bounded envelope version before the version-specific command shape.
/// This preserves a typed upgrade error even for removed v1 inline edit commands.
pub fn read_request<R: Read>(reader: &mut R) -> Result<RequestFrame, TransportError> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        version: u16,
        request_id: crate::RequestId,
    }
    let bytes = read_frame(reader, MAX_CONTROL_BYTES)?;
    let envelope: Envelope = serde_json::from_slice(&bytes)?;
    if envelope.version != crate::PROTOCOL_VERSION {
        return Ok(RequestFrame::Rejected {
            request_id: envelope.request_id,
            error: crate::ProtocolError::new(
                crate::ErrorCode::Unsupported,
                "wire version 2 required; upgrade this client before retrying",
            ),
        });
    }
    let request: crate::Request = serde_json::from_slice(&bytes)?;
    if let Err(error) = request.validate() {
        return Ok(RequestFrame::Rejected {
            request_id: request.request_id,
            error,
        });
    }
    Ok(RequestFrame::Current(request))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Request, RequestId};
    use std::io::Cursor;

    #[test]
    fn control_round_trip_and_adjacent_frames() {
        let request = Request::new(RequestId::new("one").unwrap(), Command::Capabilities);
        let mut bytes = Vec::new();
        write_message(&mut bytes, &request).unwrap();
        write_message(&mut bytes, &request).unwrap();
        let mut reader = Cursor::new(bytes);
        let a: Request = read_message(&mut reader).unwrap();
        let b: Request = read_message(&mut reader).unwrap();
        assert_eq!(a.request_id, b.request_id);
        assert_eq!(reader.position(), reader.get_ref().len() as u64);
    }

    #[test]
    fn oversized_and_zero_header_rejected_without_body() {
        for size in [0, MAX_CONTROL_BYTES as u32 + 1, u32::MAX] {
            let result = read_message::<_, serde_json::Value>(&mut Cursor::new(size.to_be_bytes()));
            assert!(matches!(result, Err(TransportError::InvalidLength(n)) if n == size));
        }
    }

    #[test]
    fn truncated_invalid_utf8_and_trailing_json_rejected() {
        for bytes in [
            vec![0, 0, 0, 3, b'{'],
            vec![0, 0, 0, 1, 255],
            [vec![0, 0, 0, 5], b"{} {}".to_vec()].concat(),
        ] {
            assert!(read_message::<_, serde_json::Value>(&mut Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn sender_rejects_oversized_without_writing_header() {
        let mut output = Vec::new();
        assert!(write_message(&mut output, &"x".repeat(MAX_CONTROL_BYTES)).is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn partial_reads_and_writes_supported() {
        struct OneByte(Cursor<Vec<u8>>);
        impl Read for OneByte {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let len = out.len().min(1);
                self.0.read(&mut out[..len])
            }
        }
        impl Write for OneByte {
            fn write(&mut self, input: &[u8]) -> io::Result<usize> {
                self.0.write(&input[..input.len().min(1)])
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut stream = OneByte(Cursor::new(Vec::new()));
        write_message(&mut stream, &vec![1, 2, 3]).unwrap();
        stream.0.set_position(0);
        assert_eq!(
            read_message::<_, Vec<u32>>(&mut stream).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn bulk_limit_is_independent() {
        let bytes = vec![42; MAX_ARTIFACT_CHUNK_BYTES];
        let mut framed = Vec::new();
        write_artifact_chunk(&mut framed, &bytes).unwrap();
        assert_eq!(
            read_artifact_chunk(&mut Cursor::new(framed)).unwrap(),
            bytes
        );
        assert!(
            write_artifact_chunk(&mut Vec::new(), &vec![0; MAX_ARTIFACT_CHUNK_BYTES + 1]).is_err()
        );
    }
}
