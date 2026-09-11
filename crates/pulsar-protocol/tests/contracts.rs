use pulsar_protocol::*;

fn request(command: Command) -> Request {
    Request::new(RequestId::new("request").unwrap(), command)
}

#[test]
fn wire_rejects_unknown_command_fields_and_unchecked_ids() {
    for json in [
        r#"{"version":1,"request_id":"r","session":null,"project":null,"expected_revision":null,"command":{"operation":"shell","arguments":{"command":"ls"}}}"#,
        r#"{"version":1,"request_id":"","session":null,"project":null,"expected_revision":null,"command":{"operation":"capabilities"}}"#,
        r#"{"version":1,"request_id":"r","session":null,"project":null,"expected_revision":null,"command":{"operation":"capabilities"},"actor":"admin"}"#,
    ] {
        assert!(serde_json::from_str::<Request>(json).is_err());
    }
}

#[test]
fn mutations_require_scope_and_expected_revision() {
    let edit = request(Command::BeginEditUpload {
        byte_len: 13,
        sha256: "0".repeat(64),
        label: "gesture".into(),
    });
    assert!(edit.validate().is_err());
    let scoped = edit.in_project(ProjectId::new("project").unwrap(), None);
    assert!(scoped.validate().is_err());
    assert!(Request {
        expected_revision: Some(RevisionId::new(0)),
        ..scoped
    }
    .validate()
    .is_ok());
}

#[test]
fn fingerprints_ignore_request_id_but_not_content_or_session() {
    let a = request(Command::CreateProject {
        name: "first".into(),
    });
    let b = Request {
        request_id: RequestId::new("other").unwrap(),
        ..a.clone()
    };
    assert_eq!(
        request_fingerprint(&a).unwrap(),
        request_fingerprint(&b).unwrap()
    );
    let changed = Request {
        command: Command::CreateProject {
            name: "second".into(),
        },
        ..a.clone()
    };
    assert_ne!(
        request_fingerprint(&a).unwrap(),
        request_fingerprint(&changed).unwrap()
    );
    let authenticated = a.clone().with_session(SessionId::new("session").unwrap());
    assert_ne!(
        request_fingerprint(&a).unwrap(),
        request_fingerprint(&authenticated).unwrap()
    );
    let authenticated_token = authenticated.clone().with_auth_token("secret-one".into());
    let changed_token = authenticated.with_auth_token("secret-two".into());
    assert_ne!(
        request_fingerprint(&authenticated_token).unwrap(),
        request_fingerprint(&changed_token).unwrap()
    );
}

#[test]
fn diagnostic_formatting_redacts_pairing_and_authentication_secrets() {
    let secret_request = request(Command::Pair {
        client_name: "client".into(),
        pairing_token: "pairing-secret-123".into(),
    })
    .with_auth_token("authentication-secret-456".into());
    let debug = format!("{secret_request:?}");
    assert!(!debug.contains("pairing-secret-123"));
    assert!(!debug.contains("authentication-secret-456"));
    let paired = ResponseBody::Paired {
        session: SessionId::new("public-session").unwrap(),
        auth_token: "response-secret-789".into(),
    };
    assert!(!format!("{paired:?}").contains("response-secret-789"));
}

#[test]
fn conflicting_project_scope_and_protocol_versions_rejected() {
    let open = request(Command::OpenProject {
        project_id: ProjectId::new("one").unwrap(),
    })
    .in_project(ProjectId::new("two").unwrap(), None);
    assert!(open.validate().is_err());
    let version = Request {
        version: 65535,
        ..request(Command::Capabilities)
    };
    assert_eq!(version.validate().unwrap_err().code, ErrorCode::Unsupported);
}

#[test]
fn error_serialization_keeps_unknown_outcomes_explicit() {
    let error = ProtocolError::new(
        ErrorCode::UnknownOutcome,
        "transport lost after external dispatch",
    );
    let result = Response::failure(RequestId::new("r").unwrap(), error.clone());
    let decoded: Response = serde_json::from_slice(&serde_json::to_vec(&result).unwrap()).unwrap();
    assert_eq!(decoded.result.unwrap_err(), error);
    assert!(!error.retryable);
}

#[test]
fn preview_adopts_actual_frame_but_rejects_stale_seek_or_transform() {
    let requested = FrameContext {
        source_version: SourceVersionId::new("source").unwrap(),
        source_placement: SourcePlacementId::new("placement").unwrap(),
        frame: FrameId::new(100),
        transform: TransformId::new("transform").unwrap(),
        seek_generation: 7,
        request_generation: 8,
    };
    let result = PreviewResult {
        context: FrameContext {
            frame: FrameId::new(101),
            ..requested.clone()
        },
        requested_source_time: SourceTimestamp::new(338, 100).unwrap(),
        source_time: SourceTimestamp::new(337, 100).unwrap(),
        source_frame_index: 101,
        observations: Vec::new(),
        frame: None,
        analysis: AnalysisStatus::Unavailable {
            reason: "model absent".into(),
        },
    };
    assert!(!result.matches_request(&requested));
    assert!(result.matches_request(&result.context));
    assert!(result.matches_seek_request(&requested, &SourceTimestamp::new(338, 100).unwrap()));
    assert!(!result.matches_seek_request(&requested, &SourceTimestamp::new(339, 100).unwrap()));
    assert!(!result.matches_seek_request(
        &FrameContext {
            seek_generation: 9,
            ..requested.clone()
        },
        &result.requested_source_time
    ));
    assert!(!result.matches_request(&FrameContext {
        seek_generation: 9,
        ..requested.clone()
    }));
    assert!(!result.matches_request(&FrameContext {
        transform: TransformId::new("other").unwrap(),
        ..requested
    }));
    assert!(!PreviewResult {
        source_frame_index: 102,
        ..result
    }
    .matches_request(&FrameContext {
        source_version: SourceVersionId::new("source").unwrap(),
        source_placement: SourcePlacementId::new("placement").unwrap(),
        frame: FrameId::new(100),
        transform: TransformId::new("transform").unwrap(),
        seek_generation: 7,
        request_generation: 8,
    }));
}

#[cfg(unix)]
mod local {
    use super::*;
    use std::{
        io::Write,
        os::unix::net::UnixListener,
        path::PathBuf,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    struct SocketPath(PathBuf);
    impl SocketPath {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "pulsar-protocol-{}-{nonce}-{label}.sock",
                std::process::id()
            )))
        }
    }
    impl Drop for SocketPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn mismatched_response_poisoned_not_reused() {
        let path = SocketPath::new("identity");
        let listener = UnixListener::bind(&path.0).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let _: Request = read_message(&mut stream).unwrap();
            write_message(
                &mut stream,
                &Response::success(RequestId::new("wrong").unwrap(), ResponseBody::Ack),
            )
            .unwrap();
        });
        let mut client = LocalClient::connect(&path.0, Duration::from_secs(2)).unwrap();
        assert!(matches!(
            client.call(&request(Command::Capabilities)),
            Err(TransportError::ResponseMismatch)
        ));
        assert!(matches!(
            client.call(&request(Command::Capabilities)),
            Err(TransportError::ConnectionPoisoned)
        ));
        server.join().unwrap();
    }

    #[test]
    fn whole_rpc_deadline_resists_slow_header() {
        let path = SocketPath::new("deadline");
        let listener = UnixListener::bind(&path.0).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let _: Request = read_message(&mut stream).unwrap();
            for byte in 10_u32.to_be_bytes() {
                thread::sleep(Duration::from_millis(35));
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
            }
        });
        let mut client = LocalClient::connect(&path.0, Duration::from_millis(60)).unwrap();
        assert!(matches!(
            client.call(&request(Command::Capabilities)),
            Err(TransportError::Io(_))
        ));
        server.join().unwrap();
    }
}
