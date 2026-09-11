use pulsar_protocol::*;
use std::io::Cursor;

fn project() -> ProjectId {
    ProjectId::new("project").unwrap()
}
fn request(command: Command) -> Request {
    Request::new(RequestId::new("request").unwrap(), command)
        .in_project(project(), Some(RevisionId::new(7)))
}
fn descriptor() -> ProgramDescriptor {
    ProgramDescriptor {
        artifact_id: ArtifactId::new("artifact").unwrap(),
        sha256: "a".repeat(64),
        byte_len: 8_000_000,
        codec: MotionCodec::MotionProgramJsonV1,
        axes: vec![AxisSummary {
            axis: Axis::Stroke,
            action_count: 54_000,
            gap_count: 0,
        }],
    }
}
fn lease() -> TransferLease {
    TransferLease {
        lease_id: TransferId::new("lease").unwrap(),
        engine_epoch: "epoch".into(),
        project_id: project(),
        direction: TransferDirection::Upload,
        sha256: "b".repeat(64),
        total_byte_len: 13,
        offset: 0,
        byte_len: 13,
        accepted_prefix: 0,
        expires_after_ms: 1000,
        bulk_endpoint: std::env::temp_dir().join("bulk.sock"),
    }
}

#[test]
fn metadata_fits_control_without_scaling_with_motion_actions() {
    let mut program = descriptor();
    program.axes = Axis::ALL
        .iter()
        .map(|axis| AxisSummary {
            axis: *axis,
            action_count: 54_000,
            gap_count: 0,
        })
        .collect();
    assert!(program.validate().is_ok());
    let snapshot = ProjectSnapshot {
        project_id: project(),
        revision: RevisionId::new(7),
        name: "Project".into(),
        motion: MotionDescriptor {
            program,
            binding: MotionBinding::ProjectRevision {
                project_id: project(),
                revision: RevisionId::new(7),
            },
        },
        sources: vec![],
    };
    snapshot.validate().unwrap();
    let mut framed = Vec::new();
    write_message(
        &mut framed,
        &Response::success(
            RequestId::new("r").unwrap(),
            ResponseBody::Project(snapshot),
        ),
    )
    .unwrap();
    assert!(framed.len() < 4096);
    assert_eq!(MAX_CONTROL_BYTES, 1024 * 1024);
    assert_eq!(MAX_ARTIFACT_CHUNK_BYTES, 256 * 1024);
}

#[test]
fn descriptors_reject_duplicate_axes_bad_hashes_and_overflowed_counts() {
    for hash in ["a".repeat(63), "A".repeat(64), "z".repeat(64)] {
        assert!(ProgramDescriptor {
            sha256: hash,
            ..descriptor()
        }
        .validate()
        .is_err());
    }
    for byte_len in [0, MAX_MOTION_BYTES + 1, u64::MAX] {
        assert!(ProgramDescriptor {
            byte_len,
            ..descriptor()
        }
        .validate()
        .is_err());
    }
    for axes in [
        vec![
            AxisSummary {
                axis: Axis::Stroke,
                action_count: 1,
                gap_count: 0
            };
            2
        ],
        vec![AxisSummary {
            axis: Axis::Stroke,
            action_count: MAX_MOTION_ACTIONS + 1,
            gap_count: 0,
        }],
        vec![AxisSummary {
            axis: Axis::Stroke,
            action_count: 0,
            gap_count: MAX_MOTION_GAPS + 1,
        }],
        vec![
            AxisSummary {
                axis: Axis::Stroke,
                action_count: u64::MAX,
                gap_count: 0,
            },
            AxisSummary {
                axis: Axis::Yaw,
                action_count: 1,
                gap_count: 0,
            },
        ],
    ] {
        assert!(ProgramDescriptor {
            axes,
            ..descriptor()
        }
        .validate()
        .is_err());
    }
}

#[test]
fn duplicate_snapshot_identity_is_not_a_trusted_binding() {
    let mut snapshot = CandidateSnapshot {
        candidate_id: CandidateId::new("candidate").unwrap(),
        project_id: project(),
        base_revision: RevisionId::new(7),
        motion: MotionDescriptor {
            program: descriptor(),
            binding: MotionBinding::Candidate {
                project_id: project(),
                candidate_id: CandidateId::new("candidate").unwrap(),
                base_revision: RevisionId::new(7),
            },
        },
        job_id: None,
        review: vec![],
    };
    snapshot.validate().unwrap();
    snapshot.project_id = ProjectId::new("another-project").unwrap();
    assert!(snapshot.validate().is_err());
    snapshot.project_id = project();
    snapshot.base_revision = RevisionId::new(8);
    assert!(snapshot.validate().is_err());
    let mut wire = serde_json::to_value(snapshot).unwrap();
    wire["program"] = serde_json::to_value(MotionProgram::default()).unwrap();
    assert!(serde_json::from_value::<CandidateSnapshot>(wire).is_err());
}

#[test]
fn all_transfers_are_project_scoped_and_upload_pins_a_revision() {
    let id = TransferId::new("lease").unwrap();
    for command in [
        Command::BeginEditUpload {
            byte_len: 13,
            sha256: "a".repeat(64),
            label: "gesture".into(),
        },
        Command::FinishEditUpload {
            lease_id: id.clone(),
        },
        Command::TransferStatus {
            lease_id: id.clone(),
        },
        Command::AbandonTransfer { lease_id: id },
        Command::BeginMotionDownload {
            locator: MotionLocator::ProjectRevision {
                revision: RevisionId::new(7),
            },
            offset: 0,
            byte_len: 13,
        },
    ] {
        let scoped = request(command);
        scoped.validate().unwrap();
        let mut unscoped = scoped.clone();
        unscoped.project = None;
        assert!(unscoped.validate().is_err());
        if matches!(scoped.command, Command::BeginEditUpload { .. }) {
            let mut unpinned = scoped;
            unpinned.expected_revision = None;
            assert!(unpinned.validate().is_err());
        }
    }
    assert!(!Command::TransferStatus {
        lease_id: TransferId::new("lease").unwrap()
    }
    .is_mutating());
}

#[test]
fn v1_removed_commands_get_typed_upgrade_before_command_parsing() {
    let legacy = serde_json::json!({"version":1,"request_id":"old-client","session":"session","auth_token":"secret",
        "project":"project","expected_revision":7,"command":{"operation":"apply_edit","arguments":{"program":{"tracks":[]},"label":"old gesture"}}});
    let mut bytes = Vec::new();
    write_message(&mut bytes, &legacy).unwrap();
    match read_request(&mut Cursor::new(bytes)).unwrap() {
        RequestFrame::Rejected { request_id, error } => {
            assert_eq!(request_id.as_str(), "old-client");
            assert_eq!(error.code, ErrorCode::Unsupported);
        }
        RequestFrame::Current(_) => panic!("old client must not reach authority"),
    }
    let mut current = legacy;
    current["version"] = serde_json::json!(PROTOCOL_VERSION);
    assert!(serde_json::from_value::<Request>(current).is_err());
}

#[test]
fn current_structural_rejection_preserves_request_identity() {
    let unscoped = Request::new(
        RequestId::new("missing-scope").unwrap(),
        Command::TransferStatus {
            lease_id: TransferId::new("lease").unwrap(),
        },
    );
    let mut bytes = Vec::new();
    write_message(&mut bytes, &unscoped).unwrap();
    assert!(matches!(
        read_request(&mut Cursor::new(bytes)).unwrap(),
        RequestFrame::Rejected {
            error: ProtocolError {
                code: ErrorCode::InvalidRequest,
                ..
            },
            ..
        }
    ));
}

#[test]
fn upload_control_cannot_smuggle_motion_or_provenance() {
    for field in [
        "program",
        "evidence",
        "lineage",
        "qualification",
        "worker",
        "model",
    ] {
        let mut value = serde_json::to_value(request(Command::BeginEditUpload {
            byte_len: 13,
            sha256: "a".repeat(64),
            label: "gesture".into(),
        }))
        .unwrap();
        value["command"]["arguments"][field] = serde_json::json!("forged");
        assert!(serde_json::from_value::<Request>(value).is_err());
    }
    for invalid in ["", "../lease", "lease\n", "x y"] {
        assert!(TransferId::new(invalid).is_err());
        assert!(serde_json::from_value::<TransferId>(serde_json::json!(invalid)).is_err());
    }
}

#[test]
fn lease_and_range_bounds_do_not_wrap_or_launder_partial_uploads() {
    lease().validate().unwrap();
    for invalid in [
        TransferLease {
            accepted_prefix: 14,
            ..lease()
        },
        TransferLease {
            expires_after_ms: 0,
            ..lease()
        },
        TransferLease {
            offset: 1,
            byte_len: 12,
            ..lease()
        },
        TransferLease {
            offset: u64::MAX,
            ..lease()
        },
        TransferLease {
            engine_epoch: "../epoch".into(),
            ..lease()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
    for (offset, byte_len) in [(u64::MAX, 1), (0, 0), (MAX_MOTION_BYTES, 1)] {
        assert!(request(Command::BeginMotionDownload {
            locator: MotionLocator::ProjectRevision {
                revision: RevisionId::new(7)
            },
            offset,
            byte_len
        })
        .validate()
        .is_err());
    }
}

#[test]
fn replay_fingerprint_binds_content_lease_and_project() {
    let original = request(Command::FinishEditUpload {
        lease_id: TransferId::new("first").unwrap(),
    });
    let mut changed = original.clone();
    changed.command = Command::FinishEditUpload {
        lease_id: TransferId::new("second").unwrap(),
    };
    assert_ne!(
        request_fingerprint(&original).unwrap(),
        request_fingerprint(&changed).unwrap()
    );
    changed = original.clone();
    changed.project = Some(ProjectId::new("other").unwrap());
    assert_ne!(
        request_fingerprint(&original).unwrap(),
        request_fingerprint(&changed).unwrap()
    );
    changed = original.clone();
    changed.request_id = RequestId::new("new-id").unwrap();
    assert_eq!(
        request_fingerprint(&original).unwrap(),
        request_fingerprint(&changed).unwrap()
    );
}
