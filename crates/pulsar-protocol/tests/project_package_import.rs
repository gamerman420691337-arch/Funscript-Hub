use pulsar_protocol::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn declaration() -> PackageUploadDeclaration {
    PackageUploadDeclaration {
        format_version: 1,
        sha256: "a".repeat(64),
        byte_len: 4096,
    }
}
fn lease() -> PackageUploadLease {
    PackageUploadLease {
        lease_id: PackageUploadId::new("upload-1").unwrap(),
        engine_epoch: "epoch-1".into(),
        operation_id: PackageImportOperationId::new("import-1").unwrap(),
        generation: 1,
        package: declaration(),
        next_offset: 0,
        replay: None,
        expires_after_ms: 10_000,
        bulk_endpoint: PathBuf::from("/tmp/bulk.sock"),
    }
}
fn handshake() -> PackageUploadHandshake {
    PackageUploadHandshake {
        version: PROTOCOL_VERSION,
        channel: PackageUploadChannel::ProjectPackageUpload,
        session: SessionId::new("session-1").unwrap(),
        auth_token: "private-secret".into(),
        lease_id: PackageUploadId::new("upload-1").unwrap(),
        engine_epoch: "epoch-1".into(),
        operation_id: PackageImportOperationId::new("import-1").unwrap(),
        generation: 1,
        package_sha256: "a".repeat(64),
    }
}
fn status() -> PackageImportStatus {
    PackageImportStatus {
        operation_id: PackageImportOperationId::new("import-1").unwrap(),
        request_id: RequestId::new("start-1").unwrap(),
        package: declaration(),
        state: PackageImportOperationState::AwaitingUpload,
        progress: PackageImportProgress {
            received_bytes: 0,
            verified_bytes: 0,
            total_bytes: 4096,
            completed_objects: 0,
            total_objects: None,
        },
        capture: None,
        receipt: None,
        error: None,
    }
}

#[test]
fn header_probe_preserves_native_and_origin_versions_without_qualifying_bytes() {
    for version in [1u16, 2] {
        let mut header = [0u8; PACKAGE_IMPORT_HEADER_BYTES];
        header[..8].copy_from_slice(b"PULSPKG\0");
        header[8..10].copy_from_slice(&version.to_le_bytes());
        header[12..20].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(package_format_version(&header).unwrap(), version);
        header[10] = 1;
        assert!(package_format_version(&header).is_err());
        header[10] = 0;
        header[8..10].copy_from_slice(&3u16.to_le_bytes());
        assert!(package_format_version(&header).is_err());
        assert!(package_format_version(&header[..51]).is_err());
    }
}

#[test]
fn clone_import_rejects_existing_project_envelopes_and_keeps_wire_version_fence() {
    let commands = vec![
        Command::StartProjectImport {
            package: declaration(),
        },
        Command::ProjectImportStatus {
            operation_id: PackageImportOperationId::new("i").unwrap(),
        },
        Command::BeginProjectPackageUpload {
            operation_id: PackageImportOperationId::new("i").unwrap(),
        },
        Command::ProjectPackageUploadStatus {
            lease_id: PackageUploadId::new("u").unwrap(),
        },
        Command::AbandonProjectPackageUpload {
            lease_id: PackageUploadId::new("u").unwrap(),
        },
        Command::SealProjectImport {
            operation_id: PackageImportOperationId::new("i").unwrap(),
            upload_generation: 1,
        },
        Command::CancelProjectImport {
            operation_id: PackageImportOperationId::new("i").unwrap(),
        },
    ];
    for command in commands {
        assert!(!command.requires_project());
        assert!(!command.requires_revision());
        let request = Request::new(RequestId::new("r").unwrap(), command);
        request.validate().unwrap();
        let mut scoped = request.clone();
        scoped.project = Some(ProjectId::new("existing").unwrap());
        assert!(scoped.validate().is_err());
        let mut revised = request.clone();
        revised.expected_revision = Some(RevisionId::new(0));
        assert!(revised.validate().is_err());
        let mut old = request;
        old.version = 1;
        assert!(old.validate().is_err());
    }
}

#[test]
fn declarations_cannot_include_paths_sql_or_authority() {
    for key in ["path", "sql", "session", "grants", "project_id", "observed"] {
        let mut value = serde_json::to_value(declaration()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert(key.into(), serde_json::json!("forged"));
        assert!(serde_json::from_value::<PackageUploadDeclaration>(value).is_err());
    }
    let mut bad = declaration();
    bad.byte_len = MAX_PACKAGE_IMPORT_BYTES + 1;
    assert!(bad.validate().is_err());
    bad = declaration();
    bad.sha256 = "G".repeat(64);
    assert!(bad.validate().is_err());
    assert!(PackageImportOperationId::new("../../foreign").is_err());
    assert!(PackageUploadId::new("").is_err());
}

#[test]
fn publication_and_terminal_status_claims_are_explicit() {
    let mut value = status();
    value.validate().unwrap();
    value.state = PackageImportOperationState::Sealing;
    assert!(value.validate().is_err());
    value.progress.received_bytes = value.package.byte_len;
    value.validate().unwrap();
    value.state = PackageImportOperationState::Completed;
    assert!(value.validate().is_err());
    value.state = PackageImportOperationState::Interrupted;
    assert!(value.validate().is_err());
    value.error = Some(ProtocolError::new(
        ErrorCode::Unavailable,
        "restart interrupted import",
    ));
    value.validate().unwrap();
    assert!(value.state.is_terminal());
    value.state = PackageImportOperationState::Validating;
    assert!(value.validate().is_err());
}

#[test]
fn upload_generation_bounds_and_exact_payload_replay_are_checked() {
    let mut value = lease();
    value.validate().unwrap();
    value.generation = 0;
    assert!(value.validate().is_err());
    value = lease();
    value.expires_after_ms = MAX_PACKAGE_UPLOAD_LIFETIME_MS + 1;
    assert!(value.validate().is_err());
    value = lease();
    let first = PackageUploadChunkReceipt {
        offset: 0,
        byte_len: 3,
        sha256: "b".repeat(64),
    };
    assert!(!value.validate_chunk(&first).unwrap());
    value.next_offset = 3;
    value.replay = Some(first.clone());
    assert!(value.validate_chunk(&first).unwrap());
    let mut conflicting = first.clone();
    conflicting.sha256 = "c".repeat(64);
    assert!(value.validate_chunk(&conflicting).is_err());
    let mut overflow = first;
    overflow.offset = u64::MAX;
    assert!(overflow.end().is_err());
}

#[test]
fn upload_cursor_scales_past_motion_replay_limits_with_constant_metadata() {
    let mut value = lease();
    value.package.byte_len = 5001 * u64::from(MAX_PACKAGE_UPLOAD_CHUNK_BYTES);
    for _ in 0..5000 {
        let chunk = PackageUploadChunkReceipt {
            offset: value.next_offset,
            byte_len: MAX_PACKAGE_UPLOAD_CHUNK_BYTES,
            sha256: "b".repeat(64),
        };
        assert!(!value.validate_chunk(&chunk).unwrap());
        value.next_offset = chunk.end().unwrap();
        value.replay = Some(chunk);
    }
    assert!(value.next_offset > 1024 * 1024 * 1024);
    assert!(serde_json::to_vec(&value).unwrap().len() < 2048);
}

#[test]
fn upload_handshake_is_tagged_strict_and_secret_redacted() {
    let value = handshake();
    value.validate().unwrap();
    assert!(!format!("{value:?}").contains("private-secret"));
    let json = serde_json::to_value(&value).unwrap();
    assert!(matches!(
        serde_json::from_value::<BulkHandshakeEnvelope>(json.clone()).unwrap(),
        BulkHandshakeEnvelope::PackageUpload(_)
    ));
    let mut wrong = json.clone();
    wrong["channel"] = serde_json::json!("project_package_download");
    assert!(serde_json::from_value::<PackageUploadHandshake>(wrong).is_err());
    let mut unknown = json;
    unknown["path"] = serde_json::json!("/private");
    assert!(serde_json::from_value::<PackageUploadHandshake>(unknown).is_err());
    let mut old = value;
    old.version = 1;
    assert!(old.validate().is_err());
}

#[cfg(unix)]
mod sockets {
    use super::*;
    use std::{
        fs,
        io::{self, Write},
        os::unix::net::{UnixListener, UnixStream},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };
    struct SocketDirectory(PathBuf);
    impl SocketDirectory {
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("pulsar-package-upload-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn endpoint(&self) -> PathBuf {
            self.0.join("bulk.sock")
        }
    }
    impl Drop for SocketDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.endpoint());
            let _ = fs::remove_dir(&self.0);
        }
    }
    fn server(
        listener: UnixListener,
        body: impl FnOnce(UnixStream) + Send + 'static,
    ) -> thread::JoinHandle<()> {
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "test accept timed out");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("test accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            body(stream);
        })
    }
    fn ready(stream: &mut UnixStream, value: PackageUploadLease) {
        let incoming: PackageUploadHandshake = read_bulk_header(stream).unwrap();
        incoming.validate().unwrap();
        write_bulk_header(stream, &PackageUploadReply::Ready { lease: value }).unwrap();
    }
    #[test]
    fn actual_upload_replay_and_local_rejection_preserve_one_connection() {
        let dir = SocketDirectory::new();
        let listener = UnixListener::bind(dir.endpoint()).unwrap();
        let worker = server(listener, |mut stream| {
            ready(&mut stream, lease());
            for (offset, expected, next_offset) in [(0, b"abc", 3), (0, b"abc", 3), (3, b"def", 6)]
            {
                let request: PackageUploadRequest = read_bulk_header(&mut stream).unwrap();
                let bytes = read_artifact_chunk(&mut stream).unwrap();
                assert_eq!(request.offset, offset);
                assert_eq!(bytes, expected);
                write_bulk_header(
                    &mut stream,
                    &PackageUploadReply::Uploaded {
                        offset,
                        byte_len: request.byte_len,
                        sha256: request.sha256,
                        next_offset,
                    },
                )
                .unwrap();
            }
        });
        let mut client =
            PackageUploadClient::connect(dir.endpoint(), Duration::from_secs(2)).unwrap();
        client.handshake(&handshake()).unwrap();
        client.upload(0, b"abc").unwrap();
        client.upload(0, b"abc").unwrap();
        assert!(client.upload(0, b"xyz").is_err());
        client.upload(3, b"def").unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn wrong_upload_generation_poisoned_before_any_payload() {
        let dir = SocketDirectory::new();
        let listener = UnixListener::bind(dir.endpoint()).unwrap();
        let worker = server(listener, |mut stream| {
            let mut wrong = lease();
            wrong.generation = 2;
            ready(&mut stream, wrong);
        });
        let mut client =
            PackageUploadClient::connect(dir.endpoint(), Duration::from_secs(2)).unwrap();
        assert!(matches!(
            client.handshake(&handshake()),
            Err(TransportError::ResponseMismatch)
        ));
        assert!(matches!(
            client.handshake(&handshake()),
            Err(TransportError::ConnectionPoisoned)
        ));
        worker.join().unwrap();
    }
    #[test]
    fn incorrect_ack_cursor_poisons_ambiguous_upload() {
        let dir = SocketDirectory::new();
        let listener = UnixListener::bind(dir.endpoint()).unwrap();
        let worker = server(listener, |mut stream| {
            ready(&mut stream, lease());
            let request: PackageUploadRequest = read_bulk_header(&mut stream).unwrap();
            read_artifact_chunk(&mut stream).unwrap();
            write_bulk_header(
                &mut stream,
                &PackageUploadReply::Uploaded {
                    offset: request.offset,
                    byte_len: request.byte_len,
                    sha256: request.sha256,
                    next_offset: 999,
                },
            )
            .unwrap();
        });
        let mut client =
            PackageUploadClient::connect(dir.endpoint(), Duration::from_secs(2)).unwrap();
        client.handshake(&handshake()).unwrap();
        assert!(matches!(
            client.upload(0, b"abc"),
            Err(TransportError::ResponseMismatch)
        ));
        assert!(matches!(
            client.upload(0, b"abc"),
            Err(TransportError::ConnectionPoisoned)
        ));
        worker.join().unwrap();
    }
    #[test]
    fn partial_ack_obeys_nonextending_absolute_deadline() {
        let dir = SocketDirectory::new();
        let listener = UnixListener::bind(dir.endpoint()).unwrap();
        let worker = server(listener, |mut stream| {
            ready(&mut stream, lease());
            let _: PackageUploadRequest = read_bulk_header(&mut stream).unwrap();
            read_artifact_chunk(&mut stream).unwrap();
            stream.write_all(&[0, 0]).unwrap();
            thread::sleep(Duration::from_millis(350));
        });
        let mut client =
            PackageUploadClient::connect(dir.endpoint(), Duration::from_secs(2)).unwrap();
        client
            .set_deadline(Instant::now() + Duration::from_millis(120))
            .unwrap();
        client
            .set_deadline(Instant::now() + Duration::from_secs(3))
            .unwrap();
        client.handshake(&handshake()).unwrap();
        let result = client.upload(0, b"abc");
        worker.join().unwrap();
        assert!(
            matches!(result, Err(TransportError::Io(error)) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock))
        );
        assert!(matches!(
            client.upload(0, b"abc"),
            Err(TransportError::ConnectionPoisoned)
        ));
    }
}
