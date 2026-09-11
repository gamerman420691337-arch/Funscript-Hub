use pulsar_protocol::*;
use std::path::PathBuf;

fn request(axis: Axis) -> Request {
    Request::new(
        RequestId::new("axis-export").unwrap(),
        Command::ExportAxis {
            path: std::env::temp_dir().join("selected.funscript"),
            axis,
        },
    )
    .in_project(ProjectId::new("project").unwrap(), Some(RevisionId::new(7)))
}

#[test]
fn every_axis_export_requires_scope_and_pinned_revision() {
    for axis in Axis::ALL {
        let valid = request(axis);
        assert!(valid.validate().is_ok());
        assert!(valid.command.is_mutating());
        let mut missing_revision = valid.clone();
        missing_revision.expected_revision = None;
        assert!(missing_revision.validate().is_err());
        let mut missing_project = valid;
        missing_project.project = None;
        assert!(missing_project.validate().is_err());
    }
}

#[test]
fn axis_export_paths_reject_empty_nul_and_oversized_wire_values() {
    for path in [
        PathBuf::new(),
        PathBuf::from("invalid\0path"),
        PathBuf::from("x".repeat(32 * 1024 + 1)),
    ] {
        let mut invalid = request(Axis::Stroke);
        invalid.command = Command::ExportAxis {
            path,
            axis: Axis::Stroke,
        };
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn selected_axis_is_part_of_the_idempotency_fingerprint() {
    let stroke = request(Axis::Stroke);
    let yaw = request(Axis::Yaw);
    assert_ne!(
        request_fingerprint(&stroke).unwrap(),
        request_fingerprint(&yaw).unwrap()
    );
    let mut same_effect = stroke.clone();
    same_effect.request_id = RequestId::new("another-request-id").unwrap();
    assert_eq!(
        request_fingerprint(&stroke).unwrap(),
        request_fingerprint(&same_effect).unwrap()
    );
}

#[test]
fn axis_receipt_roundtrips_without_changing_legacy_export_wire_shape() {
    let path = std::env::temp_dir().join("yaw.funscript");
    let receipt = ResponseBody::AxisExported {
        path: path.clone(),
        revision: RevisionId::new(7),
        axis: Axis::Yaw,
    };
    let decoded: ResponseBody =
        serde_json::from_value(serde_json::to_value(receipt).unwrap()).unwrap();
    assert!(
        matches!(decoded, ResponseBody::AxisExported { axis: Axis::Yaw, revision, .. } if revision == RevisionId::new(7))
    );
    let legacy = ResponseBody::Exported {
        path: path.clone(),
        revision: RevisionId::new(7),
    };
    let encoded = serde_json::to_value(legacy).unwrap();
    assert_eq!(encoded["kind"], "exported");
    assert!(encoded["value"].get("axis").is_none());
    let legacy_command: Command =
        serde_json::from_value(serde_json::to_value(Command::Export { path }).unwrap()).unwrap();
    assert!(matches!(legacy_command, Command::Export { .. }));
}

#[test]
fn unknown_axis_rejected_and_private_export_path_hidden_from_debug() {
    let mut wire = serde_json::to_value(request(Axis::Stroke)).unwrap();
    wire["command"]["arguments"]["axis"] = serde_json::json!("unknown-axis");
    assert!(serde_json::from_value::<Request>(wire).is_err());
    let command = Command::ExportAxis {
        path: PathBuf::from("/PRIVATE_PROJECT/PRIVATE_MEDIA.funscript"),
        axis: Axis::Roll,
    };
    assert!(!format!("{command:?}").contains("PRIVATE"));
}
