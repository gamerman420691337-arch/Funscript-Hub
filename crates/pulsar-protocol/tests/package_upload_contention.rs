use pulsar_protocol::{
    read_bulk_header, write_bulk_header, ErrorCode, PackageUploadReply, ProtocolError,
    PROTOCOL_VERSION,
};
use std::io::Cursor;

fn contention(retryable: bool) -> ProtocolError {
    let mut error = ProtocolError::new(
        ErrorCode::PackageUploadConnectionBusy,
        "the bound upload lease already has an active connection",
    );
    error.retryable = retryable;
    error
}

#[test]
fn upload_connection_contention_has_a_distinct_typed_wire_code() {
    let reply = PackageUploadReply::Error(contention(true));
    let value = serde_json::to_value(&reply).unwrap();
    assert_eq!(value["status"], "error");
    assert_eq!(value["payload"]["code"], "package_upload_connection_busy");
    assert_eq!(value["payload"]["retryable"], true);
    assert_eq!(PROTOCOL_VERSION, 2);
    let mut encoded = Vec::new();
    write_bulk_header(&mut encoded, &reply).unwrap();
    let decoded: PackageUploadReply = read_bulk_header(&mut Cursor::new(encoded)).unwrap();
    match decoded {
        PackageUploadReply::Error(error) => {
            assert_eq!(error.code, ErrorCode::PackageUploadConnectionBusy);
            assert!(error.retryable);
        }
        _ => panic!("contention must remain a rejected handshake, not a successful lease"),
    }
}

#[test]
fn contention_without_retry_authorization_is_not_promoted() {
    let value = serde_json::to_value(PackageUploadReply::Error(contention(false))).unwrap();
    let decoded: PackageUploadReply = serde_json::from_value(value).unwrap();
    match decoded {
        PackageUploadReply::Error(error) => {
            assert_eq!(error.code, ErrorCode::PackageUploadConnectionBusy);
            assert!(!error.retryable);
        }
        _ => panic!("rejection must not become a successful lease"),
    }
}

#[test]
fn generic_resource_errors_and_unknown_codes_do_not_become_upload_contention() {
    let mut generic = ProtocolError::new(ErrorCode::ResourceExhausted, "storage admission blocked");
    generic.retryable = true;
    let value = serde_json::to_value(PackageUploadReply::Error(generic)).unwrap();
    assert_eq!(value["payload"]["code"], "resource_exhausted");
    let decoded: PackageUploadReply = serde_json::from_value(value).unwrap();
    match decoded {
        PackageUploadReply::Error(error) => {
            assert_ne!(error.code, ErrorCode::PackageUploadConnectionBusy)
        }
        _ => panic!("resource rejection must not become a successful lease"),
    }
    let mut unknown = serde_json::to_value(PackageUploadReply::Error(contention(true))).unwrap();
    unknown["payload"]["code"] = serde_json::json!("package_upload_connection_busy_future");
    assert!(serde_json::from_value::<PackageUploadReply>(unknown).is_err());
}

#[cfg(unix)]
mod deadlines {
    use super::*;
    use pulsar_protocol::{
        read_artifact_chunk, PackageImportOperationId, PackageUploadChannel, PackageUploadClient,
        PackageUploadDeclaration, PackageUploadHandshake, PackageUploadId, PackageUploadLease,
        PackageUploadRequest, SessionId, TransportError,
    };
    use std::{
        fs, io,
        os::unix::net::{UnixListener, UnixStream},
        path::PathBuf,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    struct SocketDirectory(PathBuf);
    impl SocketDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pulsar-upload-budget-{}-{nonce}",
                std::process::id()
            ));
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
    fn request() -> PackageUploadHandshake {
        PackageUploadHandshake {
            version: PROTOCOL_VERSION,
            channel: PackageUploadChannel::ProjectPackageUpload,
            session: SessionId::new("s").unwrap(),
            auth_token: "secret".into(),
            lease_id: PackageUploadId::new("u").unwrap(),
            engine_epoch: "epoch".into(),
            operation_id: PackageImportOperationId::new("i").unwrap(),
            generation: 1,
            package_sha256: "a".repeat(64),
        }
    }
    fn lease() -> PackageUploadLease {
        PackageUploadLease {
            lease_id: PackageUploadId::new("u").unwrap(),
            engine_epoch: "epoch".into(),
            operation_id: PackageImportOperationId::new("i").unwrap(),
            generation: 1,
            package: PackageUploadDeclaration {
                format_version: 1,
                sha256: "a".repeat(64),
                byte_len: 4096,
            },
            next_offset: 0,
            replay: None,
            expires_after_ms: 10_000,
            bulk_endpoint: "/tmp/bulk.sock".into(),
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
                        assert!(Instant::now() < deadline, "test accept deadline expired");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("test accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            body(stream);
        })
    }
    #[test]
    fn expired_connection_budget_is_rejected_before_endpoint_lookup() {
        let dir = SocketDirectory::new();
        let result = PackageUploadClient::connect_until(
            dir.endpoint(),
            Duration::from_secs(2),
            Instant::now() - Duration::from_millis(1),
        );
        assert!(
            matches!(result,Err(TransportError::Io(error))if error.kind()==io::ErrorKind::TimedOut)
        );
    }
    #[test]
    fn short_handshake_budget_still_poisons_on_timeout() {
        let dir = SocketDirectory::new();
        let listener = UnixListener::bind(dir.endpoint()).unwrap();
        let worker = server(listener, |mut stream| {
            let _: PackageUploadHandshake = read_bulk_header(&mut stream).unwrap();
            thread::sleep(Duration::from_millis(400));
            let _ = write_bulk_header(&mut stream, &PackageUploadReply::Ready { lease: lease() });
        });
        let mut client =
            PackageUploadClient::connect(dir.endpoint(), Duration::from_secs(2)).unwrap();
        client
            .set_deadline(Instant::now() + Duration::from_secs(5))
            .unwrap();
        let result =
            client.handshake_until(&request(), Instant::now() + Duration::from_millis(150));
        worker.join().unwrap();
        assert!(
            matches!(result,Err(TransportError::Io(error))if matches!(error.kind(),io::ErrorKind::TimedOut|io::ErrorKind::WouldBlock))
        );
        assert!(matches!(
            client.handshake(&request()),
            Err(TransportError::ConnectionPoisoned)
        ));
    }
    #[test]
    fn successful_short_admission_keeps_original_transfer_and_chunk_budgets() {
        let dir = SocketDirectory::new();
        let listener = UnixListener::bind(dir.endpoint()).unwrap();
        let worker = server(listener, |mut stream| {
            let _: PackageUploadHandshake = read_bulk_header(&mut stream).unwrap();
            write_bulk_header(&mut stream, &PackageUploadReply::Ready { lease: lease() }).unwrap();
            let chunk: PackageUploadRequest = read_bulk_header(&mut stream).unwrap();
            assert_eq!(read_artifact_chunk(&mut stream).unwrap(), b"abc");
            // Longer than the admission budget, shorter than the ordinary RPC bound.
            thread::sleep(Duration::from_millis(700));
            write_bulk_header(
                &mut stream,
                &PackageUploadReply::Uploaded {
                    offset: chunk.offset,
                    byte_len: chunk.byte_len,
                    sha256: chunk.sha256,
                    next_offset: 3,
                },
            )
            .unwrap();
        });
        let admission = Instant::now() + Duration::from_millis(500);
        let mut client =
            PackageUploadClient::connect_until(dir.endpoint(), Duration::from_secs(2), admission)
                .unwrap();
        client
            .set_deadline(Instant::now() + Duration::from_secs(5))
            .unwrap();
        client.handshake_until(&request(), admission).unwrap();
        // The next operation starts only after the successful admission budget expired.
        let delay =
            admission.saturating_duration_since(Instant::now()) + Duration::from_millis(100);
        thread::sleep(delay);
        let result = client.upload(0, b"abc");
        worker.join().unwrap();
        assert!(
            result.is_ok(),
            "temporary admission budget leaked into transfer: {result:?}"
        );
    }
}
