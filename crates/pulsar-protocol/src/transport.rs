use crate::{read_message, write_message, Request, Response, TransportError, PROTOCOL_VERSION};
use interprocess::local_socket::prelude::*;
pub use interprocess::local_socket::{
    prelude as local_socket_prelude, ConnectOptions, GenericFilePath, Listener as LocalListener,
    ListenerOptions, Name as LocalName, Stream as LocalStream,
};
pub use interprocess::ConnectWaitMode;
use std::{
    io::{self, Read, Write},
    path::Path,
    time::{Duration, Instant},
};

pub fn endpoint_name(path: &Path) -> io::Result<LocalName<'_>> {
    path.to_fs_name::<GenericFilePath>()
}

/// A client never creates/removes the server endpoint. Unix owner-only directory
/// permissions and Windows current-user pipe DACLs belong to engine startup.
/// This transport is not a defense against arbitrary code already running with
/// the owner's full OS account privileges.
pub struct LocalClient {
    stream: Option<LocalStream>,
    timeout: Duration,
}

impl LocalClient {
    pub fn connect(path: impl AsRef<Path>, timeout: Duration) -> Result<Self, TransportError> {
        if timeout.is_zero() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "zero RPC timeout").into());
        }
        let stream = connect_local(path.as_ref(), timeout)?;
        stream.set_recv_timeout(Some(timeout))?;
        stream.set_send_timeout(Some(timeout))?;
        Ok(Self {
            stream: Some(stream),
            timeout,
        })
    }

    /// One in-flight call. A framing, timeout, or identity failure poisons the
    /// connection; callers must reconnect and use the same request ID rather
    /// than silently replaying an ambiguous mutating operation with a new ID.
    pub fn call(&mut self, request: &Request) -> Result<Response, TransportError> {
        request.validate()?;
        let mut stream = self
            .stream
            .take()
            .ok_or(TransportError::ConnectionPoisoned)?;
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "RPC timeout overflow"))?;
        let mut io = DeadlineStream::new(&mut stream, deadline);
        write_message(&mut io, request)?;
        let response: Response = read_message(&mut io)?;
        if response.version != PROTOCOL_VERSION || response.request_id != request.request_id {
            return Err(TransportError::ResponseMismatch);
        }
        self.stream = Some(stream);
        Ok(response)
    }
}

/// Recomputes the remaining timeout before every partial I/O operation, so a
/// peer sending one byte at a time cannot reset a whole-message deadline.
pub struct DeadlineStream<'a> {
    stream: &'a mut LocalStream,
    deadline: Instant,
}
impl<'a> DeadlineStream<'a> {
    pub fn new(stream: &'a mut LocalStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "control message deadline exceeded")
            })
    }
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_recv_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_send_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.remaining()?;
        self.stream.flush()
    }
}

/// Bound Unix connection setup without detached work or raw descriptor access.
/// Each retry creates a fresh socket and shares one absolute deadline. The
/// dependency uses nonblocking connect/poll for ConnectWaitMode::Timeout.
///
/// Linux AF_UNIX backlog EAGAIN is not proof that a connect is progressing.
/// interprocess 2.4.4 may report writable with no SO_ERROR on an unconnected
/// socket; peer_addr prevents publishing that socket as a usable connection.
/// https://man7.org/linux/man-pages/man2/connect.2.html
/// https://docs.rs/interprocess/2.4.4/interprocess/local_socket/struct.ConnectOptions.html
///
/// Windows timeout mode rejects overloaded local pipes promptly, but cannot
/// bound remote named-pipe network activity. Remote pipes are rejected here;
/// only the local pipe namespace is an engine API endpoint.
pub fn connect_local(path: &Path, timeout: Duration) -> io::Result<LocalStream> {
    use interprocess::{local_socket::ConnectOptions, ConnectWaitMode};
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "zero connection timeout",
        ));
    }
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "connection timeout overflow")
    })?;
    #[cfg(windows)]
    validate_local_pipe_endpoint(&path.to_string_lossy())?;
    loop {
        let remaining = connect_remaining(deadline)?;
        let result = ConnectOptions::new()
            .name(endpoint_name(path)?)
            .wait_mode(ConnectWaitMode::Timeout(remaining))
            .connect_sync()
            .and_then(verify_connected);
        match result {
            Ok(stream) => {
                connect_remaining(deadline)?;
                return Ok(stream);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                        | io::ErrorKind::NotConnected
                ) => {}
            Err(error) => return Err(error),
        }
        std::thread::sleep(connect_remaining(deadline)?.min(Duration::from_millis(2)));
    }
}

fn connect_remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "local connection deadline exceeded",
            )
        })
}

fn verify_connected(stream: LocalStream) -> io::Result<LocalStream> {
    #[cfg(unix)]
    {
        match &stream {
            LocalStream::UdSocket(unix) => {
                if let Some(error) = unix.take_error()? {
                    return Err(error);
                }
                unix.inner().peer_addr()?;
            }
        }
    }
    Ok(stream)
}

#[cfg(any(windows, test))]
fn validate_local_pipe_endpoint(name: &str) -> io::Result<()> {
    let valid = name.strip_prefix(r"\\.\pipe\").is_some_and(|pipe| {
        !pipe.is_empty()
            && pipe != "."
            && pipe != ".."
            && !pipe
                .chars()
                .any(|c| c.is_control() || c == '\\' || c == '/')
    });
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "engine endpoint requires the canonical local named-pipe namespace",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    #[test]
    fn windows_endpoint_accepts_only_canonical_local_pipe_names() {
        assert!(validate_local_pipe_endpoint(r"\\.\pipe\pulsar-user-control").is_ok());
        for name in [
            r"\\server\pipe\pulsar",
            r"\\?\pipe\pulsar",
            r"\\?\UNC\server\pipe\pulsar",
            "//server/pipe/pulsar",
            "//./pipe/pulsar",
            r"C:\pipe\pulsar",
            "pulsar",
            r"\\.\pipe\",
            r"\\.\pipe\..",
            r"\\.\pipe\sub\name",
            r"\\.\pipe\sub/name",
            "\\\\.\\pipe\\pulsar\0secret",
        ] {
            assert!(
                validate_local_pipe_endpoint(name).is_err(),
                "accepted {name:?}"
            );
        }
    }
}
