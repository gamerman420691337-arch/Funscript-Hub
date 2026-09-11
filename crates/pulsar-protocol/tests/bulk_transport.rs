//! Bulk wire tests use real local sockets on Unix and portable in-memory codecs.
use pulsar_protocol::*;
use std::io::{self, Cursor};

fn handshake() -> BulkHandshake {
    BulkHandshake {
        version: PROTOCOL_VERSION,
        session: SessionId::new("bulk-test-session").unwrap(),
        auth_token: "private-bulk-test-token".into(),
        lease_id: TransferId::new("bulk-test-lease").unwrap(),
        engine_epoch: "test-epoch-1".into(),
    }
}

#[test]
fn handshake_debug_redacts_authentication_secret() {
    let handshake = handshake();
    let debug = format!("{handshake:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&handshake.auth_token));
    assert!(serde_json::to_string(&handshake)
        .unwrap()
        .contains(&handshake.auth_token));
}

#[test]
fn bulk_codecs_round_trip_exact_limits_and_adjacent_frames() {
    assert_eq!(MAX_BULK_HEADER_BYTES, 16 * 1024);
    assert_eq!(MAX_ARTIFACT_CHUNK_BYTES, 256 * 1024);
    let header = "h".repeat(MAX_BULK_HEADER_BYTES - 2);
    let chunk = vec![0xa5; MAX_ARTIFACT_CHUNK_BYTES];
    let mut output = Vec::new();
    write_bulk_header(&mut output, &header).unwrap();
    write_artifact_chunk(&mut output, &chunk).unwrap();
    write_bulk_header(
        &mut output,
        &BulkReply::Uploaded {
            accepted_prefix: 123,
        },
    )
    .unwrap();

    let mut input = Cursor::new(output);
    assert_eq!(read_bulk_header::<_, String>(&mut input).unwrap(), header);
    assert_eq!(read_artifact_chunk(&mut input).unwrap(), chunk);
    assert!(matches!(
        read_bulk_header::<_, BulkReply>(&mut input).unwrap(),
        BulkReply::Uploaded {
            accepted_prefix: 123
        }
    ));
    assert_eq!(input.position(), input.get_ref().len() as u64);
}

#[test]
fn invalid_bulk_lengths_reject_before_reading_or_writing_body() {
    for length in [0, MAX_BULK_HEADER_BYTES as u32 + 1, u32::MAX] {
        let mut input = Cursor::new(length.to_be_bytes());
        assert!(matches!(
            read_bulk_header::<_, serde_json::Value>(&mut input),
            Err(TransportError::InvalidLength(actual)) if actual == length
        ));
        assert_eq!(input.position(), 4);
    }
    for length in [0, MAX_ARTIFACT_CHUNK_BYTES as u32 + 1, u32::MAX] {
        let mut input = Cursor::new(length.to_be_bytes());
        assert!(matches!(
            read_artifact_chunk(&mut input),
            Err(TransportError::InvalidLength(actual)) if actual == length
        ));
        assert_eq!(input.position(), 4);
    }
    let mut output = Vec::new();
    assert!(write_bulk_header(&mut output, &"h".repeat(MAX_BULK_HEADER_BYTES - 1)).is_err());
    assert!(output.is_empty());
    for chunk in [Vec::new(), vec![0; MAX_ARTIFACT_CHUNK_BYTES + 1]] {
        assert!(write_artifact_chunk(&mut output, &chunk).is_err());
        assert!(output.is_empty());
    }
}

#[test]
fn truncated_bulk_headers_and_bodies_and_invalid_json_fail() {
    for bytes in [
        vec![],
        vec![0],
        vec![0, 0],
        vec![0, 0, 0],
        vec![0, 0, 0, 8, b'{', b'}'],
    ] {
        assert!(matches!(
            read_bulk_header::<_, serde_json::Value>(&mut Cursor::new(bytes.clone())),
            Err(TransportError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
        assert!(matches!(
            read_artifact_chunk(&mut Cursor::new(bytes)),
            Err(TransportError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
    }
    for body in [vec![0xff], b"{} {}".to_vec(), b"{".to_vec()] {
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend(body);
        assert!(matches!(
            read_bulk_header::<_, serde_json::Value>(&mut Cursor::new(frame)),
            Err(TransportError::Json(_))
        ));
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        fs,
        io::Write,
        os::unix::net::{UnixListener, UnixStream},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant},
    };

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);
    const IO_TIMEOUT: Duration = Duration::from_secs(3);

    struct SocketDirectory(PathBuf);
    impl Drop for SocketDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0.join("bulk.sock"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    fn with_peer<S, C>(peer: S, client: C)
    where
        S: FnOnce(UnixStream) + Send + 'static,
        C: FnOnce(&Path),
    {
        let directory = SocketDirectory(std::env::temp_dir().join(format!(
            "pbt-{}-{}",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir(&directory.0).unwrap();
        let endpoint = directory.0.join("bulk.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
            stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
            peer(stream);
        });
        client(&endpoint);
        server.join().expect("mock bulk peer failed");
    }

    fn lease(direction: TransferDirection) -> TransferLease {
        let request = handshake();
        TransferLease {
            lease_id: request.lease_id,
            engine_epoch: request.engine_epoch,
            project_id: ProjectId::new("bulk-test-project").unwrap(),
            direction,
            sha256: "a".repeat(64),
            total_byte_len: 16,
            offset: if direction == TransferDirection::Download {
                4
            } else {
                0
            },
            byte_len: if direction == TransferDirection::Download {
                8
            } else {
                16
            },
            accepted_prefix: 0,
            expires_after_ms: 30_000,
            bulk_endpoint: PathBuf::from("bulk.sock"),
        }
    }

    fn receive_handshake(stream: &mut UnixStream) {
        let received: BulkHandshake = read_bulk_header(stream).unwrap();
        let expected = handshake();
        assert_eq!(received.version, PROTOCOL_VERSION);
        assert_eq!(received.session, expected.session);
        assert_eq!(received.auth_token, expected.auth_token);
        assert_eq!(received.lease_id, expected.lease_id);
        assert_eq!(received.engine_epoch, expected.engine_epoch);
    }

    fn authenticate_peer(stream: &mut UnixStream, direction: TransferDirection) {
        receive_handshake(stream);
        write_bulk_header(
            stream,
            &BulkReply::Ready {
                lease: lease(direction),
            },
        )
        .unwrap();
    }

    fn expect_upload(stream: &mut UnixStream, offset: u64, bytes: &[u8]) {
        assert!(matches!(
            read_bulk_header::<_, BulkOperation>(stream).unwrap(),
            BulkOperation::Upload { offset: actual_offset, byte_len }
                if actual_offset == offset && byte_len as usize == bytes.len()
        ));
        assert_eq!(read_artifact_chunk(stream).unwrap(), bytes);
    }

    fn expect_download(stream: &mut UnixStream, offset: u64, byte_len: u32) {
        assert!(matches!(
            read_bulk_header::<_, BulkOperation>(stream).unwrap(),
            BulkOperation::Download { offset: actual_offset, byte_len: actual_len }
                if actual_offset == offset && actual_len == byte_len
        ));
    }

    fn assert_poisoned(client: &mut BulkClient) {
        assert!(matches!(
            client.upload(0, b"x"),
            Err(TransportError::ConnectionPoisoned)
        ));
        assert!(matches!(
            client.download(0, 1),
            Err(TransportError::ConnectionPoisoned)
        ));
    }

    #[test]
    fn upload_round_trip_and_acknowledged_retries_preserve_exact_prefix() {
        with_peer(
            |mut stream| {
                authenticate_peer(&mut stream, TransferDirection::Upload);
                for (offset, bytes, prefix) in [
                    (0, b"abcdefgh".as_slice(), 8),
                    (0, b"abcdefgh".as_slice(), 8),
                    (8, b"ijklmnop".as_slice(), 16),
                    (0, b"abcdefgh".as_slice(), 16),
                ] {
                    expect_upload(&mut stream, offset, bytes);
                    write_bulk_header(
                        &mut stream,
                        &BulkReply::Uploaded {
                            accepted_prefix: prefix,
                        },
                    )
                    .unwrap();
                }
            },
            |endpoint| {
                let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                assert_eq!(
                    client.handshake(&handshake()).unwrap(),
                    lease(TransferDirection::Upload)
                );
                assert_eq!(client.upload(0, b"abcdefgh").unwrap(), 8);
                assert!(matches!(
                    client.upload(9, b"x"),
                    Err(TransportError::Protocol(_))
                ));
                assert!(matches!(
                    client.upload(7, b"xy"),
                    Err(TransportError::Protocol(_))
                ));
                assert!(matches!(
                    client.download(0, 1),
                    Err(TransportError::Protocol(_))
                ));
                assert_eq!(client.upload(0, b"abcdefgh").unwrap(), 8);
                assert_eq!(client.upload(8, b"ijklmnop").unwrap(), 16);
                assert_eq!(client.upload(0, b"abcdefgh").unwrap(), 16);
            },
        );
    }

    #[test]
    fn download_round_trip_supports_nonzero_lease_offset_and_retries() {
        with_peer(
            |mut stream| {
                authenticate_peer(&mut stream, TransferDirection::Download);
                for (offset, bytes) in [
                    (4, b"efgh".as_slice()),
                    (4, b"efgh".as_slice()),
                    (8, b"ijkl".as_slice()),
                ] {
                    expect_download(&mut stream, offset, bytes.len() as u32);
                    write_bulk_header(
                        &mut stream,
                        &BulkReply::Download {
                            offset,
                            byte_len: bytes.len() as u32,
                        },
                    )
                    .unwrap();
                    write_artifact_chunk(&mut stream, bytes).unwrap();
                }
            },
            |endpoint| {
                let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                assert_eq!(
                    client.handshake(&handshake()).unwrap(),
                    lease(TransferDirection::Download)
                );
                assert!(matches!(
                    client.download(3, 1),
                    Err(TransportError::Protocol(_))
                ));
                assert!(matches!(
                    client.download(11, 2),
                    Err(TransportError::Protocol(_))
                ));
                assert!(matches!(
                    client.upload(4, b"x"),
                    Err(TransportError::Protocol(_))
                ));
                assert_eq!(client.download(4, 4).unwrap(), b"efgh");
                assert_eq!(client.download(4, 4).unwrap(), b"efgh");
                assert_eq!(client.download(8, 4).unwrap(), b"ijkl");
            },
        );
    }

    #[test]
    fn wrong_handshake_lease_epoch_or_reply_poisons_connection() {
        for case in 0..5 {
            with_peer(
                move |mut stream| {
                    receive_handshake(&mut stream);
                    let mut returned = lease(TransferDirection::Upload);
                    let reply = match case {
                        0 => {
                            returned.lease_id = TransferId::new("other-lease").unwrap();
                            BulkReply::Ready { lease: returned }
                        }
                        1 => {
                            returned.engine_epoch = "other-epoch".into();
                            BulkReply::Ready { lease: returned }
                        }
                        2 => {
                            returned.accepted_prefix = returned.byte_len + 1;
                            BulkReply::Ready { lease: returned }
                        }
                        3 => BulkReply::Uploaded { accepted_prefix: 0 },
                        _ => BulkReply::Error(ProtocolError::invalid("handshake refused")),
                    };
                    write_bulk_header(&mut stream, &reply).unwrap();
                },
                move |endpoint| {
                    let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                    let error = client.handshake(&handshake()).unwrap_err();
                    if matches!(case, 0 | 1 | 3) {
                        assert!(matches!(error, TransportError::ResponseMismatch));
                    } else {
                        assert!(matches!(error, TransportError::Protocol(_)));
                    }
                    assert_poisoned(&mut client);
                },
            );
        }
    }

    #[test]
    fn wrong_upload_acknowledgement_or_reply_poisons_connection() {
        for case in 0..4 {
            with_peer(
                move |mut stream| {
                    authenticate_peer(&mut stream, TransferDirection::Upload);
                    expect_upload(&mut stream, 0, b"abcd");
                    let reply = match case {
                        0 => BulkReply::Uploaded { accepted_prefix: 3 },
                        1 => BulkReply::Uploaded { accepted_prefix: 5 },
                        2 => BulkReply::Download {
                            offset: 0,
                            byte_len: 4,
                        },
                        _ => BulkReply::Error(ProtocolError::invalid("upload refused")),
                    };
                    write_bulk_header(&mut stream, &reply).unwrap();
                },
                move |endpoint| {
                    let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                    client.handshake(&handshake()).unwrap();
                    let error = client.upload(0, b"abcd").unwrap_err();
                    if case == 3 {
                        assert!(matches!(error, TransportError::Protocol(_)));
                    } else {
                        assert!(matches!(error, TransportError::ResponseMismatch));
                    }
                    assert_poisoned(&mut client);
                },
            );
        }
    }

    #[test]
    fn wrong_download_offset_length_or_reply_poisons_connection() {
        for case in 0..5 {
            with_peer(
                move |mut stream| {
                    authenticate_peer(&mut stream, TransferDirection::Download);
                    expect_download(&mut stream, 4, 4);
                    let reply = match case {
                        0 => BulkReply::Download {
                            offset: 5,
                            byte_len: 4,
                        },
                        1 => BulkReply::Download {
                            offset: 4,
                            byte_len: 5,
                        },
                        2 => BulkReply::Download {
                            offset: 4,
                            byte_len: 4,
                        },
                        3 => BulkReply::Uploaded { accepted_prefix: 4 },
                        _ => BulkReply::Error(ProtocolError::invalid("download refused")),
                    };
                    write_bulk_header(&mut stream, &reply).unwrap();
                    if case == 2 {
                        write_artifact_chunk(&mut stream, b"abc").unwrap();
                    }
                },
                move |endpoint| {
                    let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                    client.handshake(&handshake()).unwrap();
                    let error = client.download(4, 4).unwrap_err();
                    if case == 4 {
                        assert!(matches!(error, TransportError::Protocol(_)));
                    } else {
                        assert!(matches!(error, TransportError::ResponseMismatch));
                    }
                    assert_poisoned(&mut client);
                },
            );
        }
    }

    #[test]
    fn invalid_or_truncated_handshake_frame_poisons_connection() {
        for case in 0..5 {
            with_peer(
                move |mut stream| {
                    receive_handshake(&mut stream);
                    match case {
                        0 => stream
                            .write_all(&(MAX_BULK_HEADER_BYTES as u32 + 1).to_be_bytes())
                            .unwrap(),
                        1 => stream.write_all(&0_u32.to_be_bytes()).unwrap(),
                        2 => stream.write_all(&[0, 0]).unwrap(),
                        3 => stream.write_all(&[0, 0, 0, 8, b'{']).unwrap(),
                        _ => stream.write_all(&[0, 0, 0, 1, 0xff]).unwrap(),
                    }
                },
                move |endpoint| {
                    let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                    let error = client.handshake(&handshake()).unwrap_err();
                    match case {
                        0 | 1 => assert!(matches!(error, TransportError::InvalidLength(_))),
                        2 | 3 => assert!(matches!(
                            error,
                            TransportError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof
                        )),
                        _ => assert!(matches!(error, TransportError::Json(_))),
                    }
                    assert_poisoned(&mut client);
                },
            );
        }
    }

    #[test]
    fn oversized_or_truncated_download_chunk_poisons_connection() {
        for case in 0..4 {
            with_peer(
                move |mut stream| {
                    authenticate_peer(&mut stream, TransferDirection::Download);
                    expect_download(&mut stream, 4, 4);
                    write_bulk_header(
                        &mut stream,
                        &BulkReply::Download {
                            offset: 4,
                            byte_len: 4,
                        },
                    )
                    .unwrap();
                    match case {
                        0 => stream
                            .write_all(&(MAX_ARTIFACT_CHUNK_BYTES as u32 + 1).to_be_bytes())
                            .unwrap(),
                        1 => stream.write_all(&0_u32.to_be_bytes()).unwrap(),
                        2 => stream.write_all(&[0, 0]).unwrap(),
                        _ => stream.write_all(&[0, 0, 0, 4, b'a', b'b']).unwrap(),
                    }
                },
                move |endpoint| {
                    let mut client = BulkClient::connect(endpoint, IO_TIMEOUT).unwrap();
                    client.handshake(&handshake()).unwrap();
                    let error = client.download(4, 4).unwrap_err();
                    if case <= 1 {
                        assert!(matches!(error, TransportError::InvalidLength(_)));
                    } else {
                        assert!(matches!(
                            error,
                            TransportError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof
                        ));
                    }
                    assert_poisoned(&mut client);
                },
            );
        }
    }

    fn send_slow_header(stream: &mut UnixStream, reply: &BulkReply) {
        let mut encoded = Vec::new();
        write_bulk_header(&mut encoded, reply).unwrap();
        // Each pause is shorter than the timeout; their combined duration is
        // longer. A timeout reset on every partial read would admit this reply.
        for byte in &encoded[..4] {
            if stream.write_all(&[*byte]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = stream.write_all(&encoded[4..]);
    }

    #[test]
    fn slow_header_cannot_extend_whole_handshake_or_download_deadline() {
        let timeout = Duration::from_millis(250);
        for during_download in [false, true] {
            with_peer(
                move |mut stream| {
                    if during_download {
                        authenticate_peer(&mut stream, TransferDirection::Download);
                        expect_download(&mut stream, 4, 4);
                        send_slow_header(
                            &mut stream,
                            &BulkReply::Download {
                                offset: 4,
                                byte_len: 4,
                            },
                        );
                        let _ = write_artifact_chunk(&mut stream, b"efgh");
                    } else {
                        receive_handshake(&mut stream);
                        send_slow_header(
                            &mut stream,
                            &BulkReply::Ready {
                                lease: lease(TransferDirection::Upload),
                            },
                        );
                    }
                },
                move |endpoint| {
                    let mut client = BulkClient::connect(endpoint, timeout).unwrap();
                    if during_download {
                        client.handshake(&handshake()).unwrap();
                    }
                    let start = Instant::now();
                    let error = if during_download {
                        client.download(4, 4).unwrap_err()
                    } else {
                        client.handshake(&handshake()).unwrap_err()
                    };
                    assert!(matches!(
                        error,
                        TransportError::Io(error)
                            if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
                    ));
                    assert!(
                        start.elapsed() < Duration::from_secs(1),
                        "bulk operation exceeded its whole-call timeout allowance"
                    );
                    assert_poisoned(&mut client);
                },
            );
        }
    }
}
