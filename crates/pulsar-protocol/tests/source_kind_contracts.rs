use pulsar_protocol::*;

#[test]
fn legacy_source_summary_defaults_only_to_media_import_purpose() {
    let source: SourceSummary = serde_json::from_value(serde_json::json!({
        "source_version": "legacy-source", "label": "legacy input"
    }))
    .unwrap();
    assert_eq!(source.kind, SourceKind::Media);
    assert_eq!(source.source_version.as_str(), "legacy-source");
}

#[test]
fn source_kind_roundtrips_independently_of_filename_or_label() {
    for (kind, wire_kind) in [
        (SourceKind::Media, "media"),
        (SourceKind::GenerationInput, "generation_input"),
        (SourceKind::Funscript, "funscript"),
    ] {
        let source = SourceSummary {
            source_version: SourceVersionId::new("same-opaque-source").unwrap(),
            label: "deliberately-misleading.mp4".into(),
            kind,
        };
        let encoded = serde_json::to_value(source).unwrap();
        assert_eq!(encoded["kind"], wire_kind);
        let decoded: SourceSummary = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.kind, kind);
    }
}

#[test]
fn callers_cannot_choose_source_kind_through_import_commands() {
    for operation in ["import_source", "import_funscript"] {
        let command = serde_json::json!({
            "operation": operation,
            "arguments": { "path": "source", "kind": "media" }
        });
        assert!(serde_json::from_value::<Command>(command).is_err());
    }
}

#[test]
fn unknown_source_kind_is_not_silently_treated_as_previewable_media() {
    assert!(serde_json::from_value::<SourceSummary>(serde_json::json!({
        "source_version": "source", "label": "input", "kind": "unknown_purpose"
    }))
    .is_err());
}
