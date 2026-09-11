//! Connect deadlines are exercised against real Unix endpoints, including a
//! saturated Linux accept queue. No network service or external device is used.
#![cfg(unix)]

use pulsar_protocol::*;
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    fs, io,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use interprocess::{local_socket::ConnectOptions, ConnectWaitMode};
#[cfg(target_os = "linux")]
use std::{sync::mpsc, thread};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
const IO_TIMEOUT: Duration = Duration::from_secs(1);

struct SocketDirectory(PathBuf);
impl SocketDirectory {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "pcd-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }

    fn endpoint(&self) -> PathBuf {
        self.0.join("local.sock")
    }
}
impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.endpoint());
        let _ = fs::remove_dir(&self.0);
    }
}

struct Endpoint {
    listener: UnixListener,
    path: PathBuf,
    _directory: SocketDirectory,
}
impl Endpoint {
    fn new() -> Self {
        let directory = SocketDirectory::new();
        let path = directory.endpoint();
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        socket.bind(&SockAddr::unix(&path).unwrap()).unwrap();
        socket.listen(1).unwrap();
        let descriptor: std::os::fd::OwnedFd = socket.into();
        let listener = UnixListener::from(descriptor);
        listener.set_nonblocking(true).unwrap();
        Self {
            listener,
            path,
            _directory: directory,
        }
    }

    fn accept_connected(&self) -> UnixStream {
        // The client has already completed connect, so this never waits on an
        // absent connection. A false successful connect fails here immediately.
        let (stream, _) = self.listener.accept().unwrap();
        stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
        stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
        stream
    }
}

fn connect_client(path: &Path, timeout: Duration, bulk: bool) -> Result<(), TransportError> {
    if bulk {
        BulkClient::connect(path, timeout).map(drop)
    } else {
        LocalClient::connect(path, timeout).map(drop)
    }
}

#[test]
fn missing_endpoint_fails_immediately_for_local_and_bulk_clients() {
    let directory = SocketDirectory::new();
    let endpoint = directory.endpoint();
    for bulk in [false, true] {
        let start = Instant::now();
        let result = connect_client(&endpoint, Duration::from_secs(2), bulk);
        let elapsed = start.elapsed();
        assert!(
            matches!(result, Err(TransportError::Io(ref error)) if error.kind() == io::ErrorKind::NotFound),
            "missing endpoint returned an unexpected result: {result:?}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "missing endpoint waited for the connection timeout: {elapsed:?}"
        );
    }
}

#[test]
fn successful_local_connect_supports_a_capability_roundtrip() {
    let endpoint = Endpoint::new();
    let mut client = LocalClient::connect(&endpoint.path, IO_TIMEOUT).unwrap();
    let mut peer = endpoint.accept_connected();
    let request_id = RequestId::new("connected-capabilities").unwrap();
    let response = Response::success(
        request_id.clone(),
        ResponseBody::Capabilities(Capabilities {
            protocol_version: PROTOCOL_VERSION,
            operations: vec!["capabilities".into()],
            backends: Vec::new(),
            physical_playback: false,
        }),
    );
    // Preload the bounded reply to keep this connection test single-threaded.
    // The request is read back below to verify the other wire direction.
    write_message(&mut peer, &response).unwrap();
    let response = client
        .call(&Request::new(request_id.clone(), Command::Capabilities))
        .unwrap();
    assert!(matches!(
        response.result,
        Ok(ResponseBody::Capabilities(capabilities))
            if capabilities.protocol_version == PROTOCOL_VERSION
                && capabilities.operations == ["capabilities"]
                && !capabilities.physical_playback
    ));
    let received: Request = read_message(&mut peer).unwrap();
    assert_eq!(received.request_id, request_id);
    assert_eq!(received.version, PROTOCOL_VERSION);
    assert!(matches!(received.command, Command::Capabilities));
}

#[test]
fn successful_bulk_connect_supports_an_authenticated_handshake() {
    let endpoint = Endpoint::new();
    let mut client = BulkClient::connect(&endpoint.path, IO_TIMEOUT).unwrap();
    let mut peer = endpoint.accept_connected();
    let handshake = BulkHandshake {
        version: PROTOCOL_VERSION,
        session: SessionId::new("connected-bulk-session").unwrap(),
        auth_token: "connected-bulk-secret".into(),
        lease_id: TransferId::new("connected-bulk-lease").unwrap(),
        engine_epoch: "connected-epoch".into(),
    };
    let lease = TransferLease {
        lease_id: handshake.lease_id.clone(),
        engine_epoch: handshake.engine_epoch.clone(),
        project_id: ProjectId::new("connected-project").unwrap(),
        direction: TransferDirection::Upload,
        sha256: "a".repeat(64),
        total_byte_len: 4,
        offset: 0,
        byte_len: 4,
        accepted_prefix: 0,
        expires_after_ms: 10_000,
        bulk_endpoint: endpoint.path.clone(),
    };
    write_bulk_header(
        &mut peer,
        &BulkReply::Ready {
            lease: lease.clone(),
        },
    )
    .unwrap();
    assert_eq!(client.handshake(&handshake).unwrap(), lease);
    let received: BulkHandshake = read_bulk_header(&mut peer).unwrap();
    assert_eq!(received.version, PROTOCOL_VERSION);
    assert_eq!(received.session, handshake.session);
    assert_eq!(received.auth_token, handshake.auth_token);
    assert_eq!(received.lease_id, handshake.lease_id);
    assert_eq!(received.engine_epoch, handshake.engine_epoch);
}

#[cfg(target_os = "linux")]
fn is_backpressure(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotConnected | io::ErrorKind::WouldBlock
    )
}

#[cfg(target_os = "linux")]
#[allow(unreachable_patterns)]
fn saturate_backlog(endpoint: &Endpoint) -> Vec<LocalStream> {
    const MAX_HELD_SOCKETS: usize = 16;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut connected = Vec::new();
    while connected.len() < MAX_HELD_SOCKETS {
        assert!(
            Instant::now() < deadline,
            "failed to establish a saturated accept queue within the setup deadline"
        );
        let result = ConnectOptions::new()
            .name(endpoint_name(&endpoint.path).unwrap())
            .wait_mode(ConnectWaitMode::Deferred)
            .connect_sync();
        match result {
            Ok(stream) => {
                // Linux EAGAIN can appear as a successful but unconnected
                // interprocess stream. Such a socket cannot fill the backlog.
                let peer = match &stream {
                    LocalStream::UdSocket(socket) => socket.inner().peer_addr(),
                    _ => panic!("Linux fixture received a non-Unix local stream"),
                };
                match peer {
                    Ok(_) => connected.push(stream),
                    Err(error) if is_backpressure(error.kind()) => {
                        assert!(
                            !connected.is_empty() && connected.len() <= 2,
                            "listen(1) fixture exceeded its bounded accept queue"
                        );
                        eprintln!("proved protocol backlog: {} connected sockets, next nonblocking connection unavailable", connected.len());
                        return connected;
                    }
                    Err(error) => panic!("unexpected backlog peer-address failure: {error}"),
                }
            }
            Err(error) if is_backpressure(error.kind()) => {
                assert!(
                    !connected.is_empty() && connected.len() <= 2,
                    "listen(1) fixture exceeded its bounded accept queue"
                );
                eprintln!("proved protocol backlog: {} connected sockets, next nonblocking connection unavailable", connected.len());
                return connected;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => panic!("unexpected backlog setup failure: {error}"),
        }
    }
    panic!("accept queue did not saturate within {MAX_HELD_SOCKETS} held sockets");
}

#[cfg(target_os = "linux")]
fn connect_with_safety_release(
    endpoint: &Endpoint,
    timeout: Duration,
    bulk: bool,
) -> (Result<(), TransportError>, Duration, bool) {
    let listener = endpoint.listener.try_clone().unwrap();
    thread::scope(|scope| {
        let (finished, receive) = mpsc::channel();
        let guard = scope.spawn(move || match receive.recv_timeout(Duration::from_secs(2)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => false,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Release a regressed blocking connect, then report that the
                // guard intervened. The listener is nonblocking and full.
                let _ = listener
                    .accept()
                    .expect("saturated queue lost its pending connections");
                true
            }
        });
        let start = Instant::now();
        let result = connect_client(&endpoint.path, timeout, bulk);
        let elapsed = start.elapsed();
        let _ = finished.send(());
        let guard_released = guard.join().expect("connection safety guard panicked");
        (result, elapsed, guard_released)
    })
}

#[cfg(target_os = "linux")]
#[test]
fn full_linux_backlog_bounds_local_and_bulk_connect_deadlines() {
    let timeout = Duration::from_millis(150);
    for bulk in [false, true] {
        let endpoint = Endpoint::new();
        let _held_connections = saturate_backlog(&endpoint);
        let (result, elapsed, guard_released) =
            connect_with_safety_release(&endpoint, timeout, bulk);
        assert!(!guard_released, "connect blocked until the safety release");
        assert!(
            matches!(result, Err(TransportError::Io(ref error)) if error.kind() == io::ErrorKind::TimedOut),
            "full backlog must expire with TimedOut, got {result:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(125),
            "transient backlog pressure failed without waiting for the deadline: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "connect exceeded the bounded deadline allowance: {elapsed:?}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn freeing_backlog_capacity_allows_local_and_bulk_connect_before_deadline() {
    for bulk in [false, true] {
        let endpoint = Endpoint::new();
        let _held_connections = saturate_backlog(&endpoint);
        let listener = endpoint.listener.try_clone().unwrap();
        let (result, elapsed) = thread::scope(|scope| {
            let (begin_release, begin_receive) = mpsc::channel();
            let release = scope.spawn(move || {
                begin_receive
                    .recv_timeout(IO_TIMEOUT)
                    .expect("connect start signal was not delivered");
                thread::sleep(Duration::from_millis(30));
                let _ = listener
                    .accept()
                    .expect("saturated queue had no pending connection");
            });
            let start = Instant::now();
            begin_release.send(()).unwrap();
            let result = connect_client(&endpoint.path, IO_TIMEOUT, bulk);
            let elapsed = start.elapsed();
            release.join().expect("backlog release thread panicked");
            (result, elapsed)
        });
        assert!(
            result.is_ok(),
            "connect did not retry freed capacity: {result:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(20),
            "connect reported success before the accept queue was released: {elapsed:?}"
        );
        assert!(
            elapsed < IO_TIMEOUT,
            "connect did not finish before its deadline: {elapsed:?}"
        );
    }
}

#[test]
fn bulk_absolute_deadline_survives_handshake_and_cannot_be_extended() {
    let endpoint = Endpoint::new();
    let mut client = BulkClient::connect(&endpoint.path, Duration::from_secs(2)).unwrap();
    let mut peer = endpoint.accept_connected();
    let handshake = BulkHandshake {
        version: PROTOCOL_VERSION,
        session: SessionId::new("deadline-bulk-session").unwrap(),
        auth_token: "deadline-bulk-secret".into(),
        lease_id: TransferId::new("deadline-bulk-lease").unwrap(),
        engine_epoch: "deadline-epoch".into(),
    };
    let lease = TransferLease {
        lease_id: handshake.lease_id.clone(),
        engine_epoch: handshake.engine_epoch.clone(),
        project_id: ProjectId::new("deadline-project").unwrap(),
        direction: TransferDirection::Upload,
        sha256: "a".repeat(64),
        total_byte_len: 4,
        offset: 0,
        byte_len: 4,
        accepted_prefix: 0,
        expires_after_ms: 10_000,
        bulk_endpoint: endpoint.path.clone(),
    };
    write_bulk_header(&mut peer, &BulkReply::Ready { lease }).unwrap();
    let deadline = Instant::now() + Duration::from_millis(60);
    client.set_deadline(deadline).unwrap();
    client.handshake(&handshake).unwrap();
    let received: BulkHandshake = read_bulk_header(&mut peer).unwrap();
    assert_eq!(received.lease_id, handshake.lease_id);

    // Replacing a 60ms absolute bound with a future 2s deadline must retain
    // the earlier bound across the successful handshake and the next chunk.
    client
        .set_deadline(Instant::now() + Duration::from_secs(2))
        .unwrap();
    let (result, elapsed) = std::thread::scope(|scope| {
        scope.spawn(move || {
            assert!(matches!(
                read_bulk_header::<_, BulkOperation>(&mut peer).unwrap(),
                BulkOperation::Upload {
                    offset: 0,
                    byte_len: 4
                }
            ));
            assert_eq!(read_artifact_chunk(&mut peer).unwrap(), b"abcd");
            std::thread::sleep(Duration::from_millis(160));
            let _ = write_bulk_header(&mut peer, &BulkReply::Uploaded { accepted_prefix: 4 });
        });
        let start = Instant::now();
        let result = client.upload(0, b"abcd");
        (result, start.elapsed())
    });
    assert!(
        matches!(result, Err(TransportError::Io(ref error))
            if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)),
        "absolute deadline was extended or ignored: {result:?}"
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "absolute deadline fell back to the 2s chunk timeout: {elapsed:?}"
    );
    assert!(matches!(
        client.upload(0, b"abcd"),
        Err(TransportError::ConnectionPoisoned)
    ));
}
