//! A read-only CLI must not hang or replace an engine when its accept queue is full.
#![cfg(target_os = "linux")]

use pulsar_protocol::{
    endpoint_name, read_message, write_message, ConnectOptions, ConnectWaitMode, ErrorCode,
    LocalStream, ProtocolError, Request, Response,
};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    ffi::OsString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    _listener: Option<UnixListener>,
    root: PathBuf,
    state: PathBuf,
    endpoint: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pcli-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        let state = root.join("state");
        DirBuilder::new().mode(0o700).create(&state).unwrap();
        let endpoint = state.join("engine.sock");
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        socket.bind(&SockAddr::unix(&endpoint).unwrap()).unwrap();
        socket.listen(1).unwrap();
        let descriptor: std::os::fd::OwnedFd = socket.into();
        let listener = UnixListener::from(descriptor);
        listener.set_nonblocking(true).unwrap();
        Self {
            _listener: Some(listener),
            root,
            state,
            endpoint,
        }
    }

    fn saturate_accept_queue(&self) -> Vec<LocalStream> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut held = Vec::new();
        while held.len() < 16 {
            assert!(
                Instant::now() < deadline,
                "accept-queue fixture setup deadline"
            );
            let stream = ConnectOptions::new()
                .name(endpoint_name(&self.endpoint).unwrap())
                .wait_mode(ConnectWaitMode::Deferred)
                .connect_sync()
                .unwrap();
            let peer = match &stream {
                LocalStream::UdSocket(socket) => socket.inner().peer_addr(),
            };
            match peer {
                Ok(_) => held.push(stream),
                Err(error) if error.kind() == io::ErrorKind::NotConnected => {
                    // A real unconnected nonblocking socket, not a scheduling
                    // timeout, proves the listener cannot accept this connect.
                    assert!(
                        !held.is_empty() && held.len() <= 2,
                        "listen(1) fixture exceeded its bounded accept queue"
                    );
                    eprintln!("proved saturated queue: {} connected sockets, next nonblocking socket NotConnected", held.len());
                    return held;
                }
                Err(error) => panic!("unexpected backlog result: {error}"),
            }
        }
        panic!("accept queue did not saturate within fixture resource bound");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // The root was created exclusively by this test. No external state
        // directory, device, or unrelated process is touched during cleanup.
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct OwnedChild(Child);
impl OwnedChild {
    fn wait_until(&mut self, deadline: Instant) -> io::Result<Option<ExitStatus>> {
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                let _ = self.0.kill();
                self.0.wait()?;
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn capabilities_cli_rejects_busy_engine_without_replacing_it_or_hanging() {
    let fixture = Fixture::new();
    let before = fs::symlink_metadata(&fixture.endpoint).unwrap();
    assert!(before.file_type().is_socket());
    let _held_connections = fixture.saturate_accept_queue();

    // A regular file cannot leave a capture reader blocked by inherited pipe
    // handles. It is outside the engine state directory inspected below.
    let stderr_path = fixture.root.join("cli.stderr");
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stderr_path)
        .unwrap();
    let start = Instant::now();
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_pulsar"))
            .arg("capabilities")
            .env("PULSAR_STATE_DIR", &fixture.state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap(),
    );
    let status = child.wait_until(start + Duration::from_secs(2)).unwrap();
    let mut stderr = String::new();
    File::open(&stderr_path)
        .unwrap()
        .take(64 * 1024)
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(
        status.is_some(),
        "CLI blocked on the full engine accept queue for more than 2s; child killed and reaped; stderr: {stderr}"
    );
    assert!(
        !status.unwrap().success(),
        "CLI unexpectedly accepted the unresponsive engine; stderr: {stderr}"
    );
    let diagnostic = stderr.to_ascii_lowercase();
    assert!(
        diagnostic.contains("busy") || diagnostic.contains("unresponsive"),
        "CLI did not identify a busy or unresponsive engine: {stderr}"
    );

    let after = fs::symlink_metadata(&fixture.endpoint).unwrap();
    assert!(after.file_type().is_socket());
    assert_eq!(
        (after.dev(), after.ino()),
        (before.dev(), before.ino()),
        "CLI replaced the existing engine socket"
    );
    let mut entries = fs::read_dir(&fixture.state)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        vec![OsString::from("engine.sock")],
        "read-only CLI created replacement engine, project, or pairing state"
    );
}

#[test]
fn launcher_waits_for_a_known_owner_before_its_listener_appears() {
    let mut fixture = Fixture::new();
    drop(fixture._listener.take());
    fs::remove_file(&fixture.endpoint).unwrap();
    let lock_path = fixture.state.join("engine.lock");
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock_path)
        .unwrap();
    owner.try_lock().unwrap();
    let config = pulsar_engine::EngineConfig::new(
        fixture.state.clone(),
        PathBuf::from(env!("CARGO_BIN_EXE_pulsar")),
    );
    assert!(pulsar_engine::instance_lock_is_held(&config).unwrap());
    let lock_before = fs::metadata(&lock_path).unwrap();
    let mut token = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(fixture.state.join("pairing.token"))
        .unwrap();
    token.write_all(b"fixture-bootstrap-token").unwrap();
    drop(token);
    let stderr_path = fixture.root.join("starting.stderr");
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stderr_path)
        .unwrap();
    let endpoint = fixture.endpoint.clone();
    let (status, elapsed, negotiated) = thread::scope(|scope| {
        let server = scope.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let listener = UnixListener::bind(&endpoint).unwrap();
            fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(250)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_millis(250)))
                            .unwrap();
                        // Readiness probes connect then close without a frame.
                        if let Ok(request) = read_message::<_, Request>(&mut stream) {
                            write_message(
                                &mut stream,
                                &Response::failure(
                                    request.request_id,
                                    ProtocolError::new(
                                        ErrorCode::Unsupported,
                                        "fixture existing engine unsupported",
                                    ),
                                ),
                            )
                            .unwrap();
                            return true;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
            false
        });
        let start = Instant::now();
        let mut child = OwnedChild(
            Command::new(env!("CARGO_BIN_EXE_pulsar"))
                .arg("capabilities")
                .env("PULSAR_STATE_DIR", &fixture.state)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
        let status = child.wait_until(start + Duration::from_secs(2)).unwrap();
        let elapsed = start.elapsed();
        (status, elapsed, server.join().unwrap())
    });
    let stderr = fs::read_to_string(stderr_path).unwrap();
    assert!(
        status.is_some(),
        "launcher hung while another owner initialized: {stderr}"
    );
    assert!(
        !status.unwrap().success(),
        "fixture deliberately advertises incompatibility"
    );
    assert!(negotiated && stderr.contains("fixture existing engine unsupported"),
        "launcher did not wait for the existing owner to reach negotiation; elapsed={elapsed:?}; stderr={stderr}");
    assert!(elapsed >= Duration::from_millis(250) && elapsed < Duration::from_secs(2));
    let lock_after = fs::metadata(lock_path).unwrap();
    assert_eq!(
        (lock_before.dev(), lock_before.ino(), lock_before.mode()),
        (lock_after.dev(), lock_after.ino(), lock_after.mode())
    );
    assert!(pulsar_engine::instance_lock_is_held(&config).unwrap());
    let mut entries = fs::read_dir(&fixture.state)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            OsString::from("engine.lock"),
            OsString::from("engine.sock"),
            OsString::from("pairing.token")
        ]
    );
}
