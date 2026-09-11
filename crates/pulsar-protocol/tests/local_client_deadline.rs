#![cfg(unix)]

use pulsar_protocol::{
    read_message, write_message, Command, LocalClient, Request, RequestId, Response, ResponseBody,
    TransportError, PROTOCOL_VERSION,
};
use std::{
    fs, io,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct SocketDirectory(PathBuf);

impl SocketDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pulsar-local-deadline-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn endpoint(&self) -> PathBuf {
        self.0.join("engine.sock")
    }
}

impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.endpoint());
        let _ = fs::remove_dir(&self.0);
    }
}

fn request() -> Request {
    Request::new(
        RequestId::new("deadline-request").unwrap(),
        Command::Capabilities,
    )
}

fn accept_before(listener: &UnixListener, deadline: Instant) -> UnixStream {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "test listener accept deadline expired"
                );
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("test listener accept failed: {error}"),
        }
    }
}

fn serve_ack(listener: UnixListener, delay: Duration) -> thread::JoinHandle<()> {
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let mut stream = accept_before(&listener, Instant::now() + Duration::from_secs(3));
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let incoming: Request = read_message(&mut stream).unwrap();
        thread::sleep(delay);
        let reply = Response {
            version: PROTOCOL_VERSION,
            request_id: incoming.request_id,
            result: Ok(ResponseBody::Ack),
        };
        // A timed-out client is expected to close its owned socket.
        let _ = write_message(&mut stream, &reply);
    })
}

fn assert_timeout(result: &Result<Response, TransportError>) {
    assert!(
        matches!(result, Err(TransportError::Io(error)) if matches!(
            error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        )),
        "expected bounded transport timeout, got {result:?}"
    );
}

#[test]
fn ordinary_control_call_without_absolute_deadline_remains_usable() {
    let directory = SocketDirectory::new("ordinary");
    let listener = UnixListener::bind(directory.endpoint()).unwrap();
    let server = serve_ack(listener, Duration::from_millis(20));
    let mut client = LocalClient::connect(directory.endpoint(), Duration::from_secs(2)).unwrap();
    let result = client.call(&request());
    server.join().unwrap();
    assert!(matches!(result.unwrap().result, Ok(ResponseBody::Ack)));
}

#[test]
fn absolute_deadline_can_shorten_but_never_extend() {
    let directory = SocketDirectory::new("shorten");
    let listener = UnixListener::bind(directory.endpoint()).unwrap();
    let server = serve_ack(listener, Duration::from_millis(650));
    let mut client = LocalClient::connect(directory.endpoint(), Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    client.set_deadline(start + Duration::from_secs(2)).unwrap();
    client
        .set_deadline(start + Duration::from_millis(250))
        .unwrap();
    client.set_deadline(start + Duration::from_secs(3)).unwrap();
    let result = client.call(&request());
    let elapsed = start.elapsed();
    let next = client.call(&request());
    server.join().unwrap();
    assert_timeout(&result);
    assert!(
        elapsed < Duration::from_millis(600),
        "short bound was extended: {elapsed:?}"
    );
    assert!(matches!(next, Err(TransportError::ConnectionPoisoned)));
}

#[test]
fn expired_absolute_deadline_cannot_be_revived() {
    let directory = SocketDirectory::new("expired");
    let _listener = UnixListener::bind(directory.endpoint()).unwrap();
    let mut client = LocalClient::connect(directory.endpoint(), Duration::from_secs(2)).unwrap();
    let expired = Instant::now() - Duration::from_millis(1);
    assert!(
        matches!(client.set_deadline(expired), Err(TransportError::Io(error))
        if error.kind() == io::ErrorKind::TimedOut)
    );
    assert!(
        matches!(client.set_deadline(Instant::now() + Duration::from_secs(5)),
        Err(TransportError::Io(error)) if error.kind() == io::ErrorKind::TimedOut)
    );
    assert!(matches!(
        client.call(&request()),
        Err(TransportError::ConnectionPoisoned)
    ));
}

#[test]
fn longer_absolute_deadline_does_not_extend_per_call_timeout() {
    let directory = SocketDirectory::new("per-call");
    let listener = UnixListener::bind(directory.endpoint()).unwrap();
    let server = serve_ack(listener, Duration::from_millis(500));
    let mut client =
        LocalClient::connect(directory.endpoint(), Duration::from_millis(150)).unwrap();
    client
        .set_deadline(Instant::now() + Duration::from_secs(2))
        .unwrap();
    let start = Instant::now();
    let result = client.call(&request());
    let elapsed = start.elapsed();
    server.join().unwrap();
    assert_timeout(&result);
    assert!(
        elapsed < Duration::from_millis(450),
        "per-call bound was extended: {elapsed:?}"
    );
}

#[cfg(target_os = "linux")]
fn small_backlog_listener(path: &Path) -> UnixListener {
    use std::os::fd::OwnedFd;
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
    socket
        .bind(&socket2::SockAddr::unix(path).unwrap())
        .unwrap();
    socket.listen(1).unwrap();
    let fd: OwnedFd = socket.into();
    UnixListener::from(fd)
}

#[cfg(target_os = "linux")]
#[test]
fn connection_setup_and_call_share_one_absolute_budget() {
    let directory = SocketDirectory::new("connect-call");
    let endpoint = directory.endpoint();
    let listener = small_backlog_listener(&endpoint);
    let fillers = [
        UnixStream::connect(&endpoint).unwrap(),
        UnixStream::connect(&endpoint).unwrap(),
    ];
    let probe = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
    probe.set_nonblocking(true).unwrap();
    let error = probe
        .connect(&socket2::SockAddr::unix(&endpoint).unwrap())
        .unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(11),
        "Linux backlog must be positively saturated"
    );
    drop(probe);
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        let accept_deadline = Instant::now() + Duration::from_secs(3);
        drop(accept_before(&listener, accept_deadline));
        drop(accept_before(&listener, accept_deadline));
        let mut stream = accept_before(&listener, accept_deadline);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let incoming: Request = read_message(&mut stream).unwrap();
        thread::sleep(Duration::from_millis(1_000));
        let reply = Response {
            version: PROTOCOL_VERSION,
            request_id: incoming.request_id,
            result: Ok(ResponseBody::Ack),
        };
        let _ = write_message(&mut stream, &reply);
    });
    let start = Instant::now();
    let deadline = start + Duration::from_millis(1_200);
    let mut client =
        LocalClient::connect(&endpoint, deadline.duration_since(Instant::now())).unwrap();
    let connect_elapsed = start.elapsed();
    client.set_deadline(deadline).unwrap();
    let result = client.call(&request());
    let total_elapsed = start.elapsed();
    server.join().unwrap();
    drop(fillers);
    assert!(
        connect_elapsed >= Duration::from_millis(300),
        "fixture did not delay connection: {connect_elapsed:?}"
    );
    assert_timeout(&result);
    assert!(
        total_elapsed < Duration::from_millis(1_380),
        "connection budget was reset before RPC: {total_elapsed:?}"
    );
}
