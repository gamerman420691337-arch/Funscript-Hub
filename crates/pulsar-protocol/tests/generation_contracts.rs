use pulsar_protocol::*;

fn id() -> RequestId {
    RequestId::new("request").unwrap()
}
fn project() -> ProjectId {
    ProjectId::new("project").unwrap()
}
fn synthesis() -> GenerationInput {
    GenerationInput::Text {
        prompt: "pattern=sine axis=stroke duration=2s frequency=1hz amplitude=0.2 offset=0.5"
            .into(),
    }
}
fn flag() -> ReviewFlag {
    ReviewFlag {
        axis: Some(Axis::Stroke),
        range: TimeRange::new(ProjectTime::ZERO, ProjectTime::from_nanos(1_000_000)).unwrap(),
        reason: ReviewReason::new("missing_detection").unwrap(),
        evidence: EvidenceKind::Unavailable,
        confidence: Some(Confidence::new(0.25, None).unwrap()),
    }
}

#[test]
fn additive_mutations_require_project_and_revision() {
    for command in [
        Command::GenerateInput { input: synthesis() },
        Command::ImportFunscript {
            path: std::env::temp_dir().join("input.funscript"),
        },
        Command::MergeCandidate {
            candidate_id: CandidateId::new("candidate").unwrap(),
            axes: vec![Axis::Stroke],
            range: None,
        },
    ] {
        assert!(Request::new(id(), command.clone()).validate().is_err());
        assert!(Request::new(id(), command.clone())
            .in_project(project(), None)
            .validate()
            .is_err());
        let request = Request::new(id(), command).in_project(project(), Some(RevisionId::new(0)));
        assert!(request.validate().is_ok());
        assert!(request.command.is_mutating());
    }
    let diagnostics =
        Request::new(id(), Command::Diagnostics { candidate_id: None }).in_project(project(), None);
    assert!(diagnostics.validate().is_ok());
    assert!(!diagnostics.command.is_mutating());
}

#[test]
fn prompts_and_axes_are_bounded_and_unambiguous() {
    for prompt in [
        "".to_owned(),
        "x".repeat(MAX_PROMPT_BYTES + 1),
        "private\ncommand".to_owned(),
    ] {
        assert!(GenerationInput::Text {
            prompt: prompt.clone()
        }
        .validate()
        .is_err());
        assert!(GenerationInput::Image {
            source_version: SourceVersionId::new("source").unwrap(),
            prompt
        }
        .validate()
        .is_err());
    }
    assert!(validate_merge_axes(&[]).is_err());
    assert!(validate_merge_axes(&[Axis::Stroke, Axis::Stroke]).is_err());
    assert!(validate_merge_axes(&Axis::ALL).is_ok());
    let pattern = parse_pattern_prompt(
        "pattern=sine axis=stroke duration=2s frequency=1hz amplitude=0.2 offset=0.5",
    )
    .unwrap();
    assert!(GenerationInput::Patterns {
        patterns: vec![pattern.clone(), pattern]
    }
    .validate()
    .is_err());
    assert!(GenerationInput::Patterns { patterns: vec![] }
        .validate()
        .is_err());
}

#[test]
fn prompt_bytes_are_redacted_from_debug_and_preserved_on_wire() {
    let input = GenerationInput::Text {
        prompt: "PRIVATE_PATTERN_TEXT".into(),
    };
    assert!(!format!("{input:?}").contains("PRIVATE_PATTERN_TEXT"));
    let request = Request::new(id(), Command::GenerateInput { input });
    assert!(!format!("{request:?}").contains("PRIVATE_PATTERN_TEXT"));
    assert!(serde_json::to_string(&request)
        .unwrap()
        .contains("PRIVATE_PATTERN_TEXT"));
}

#[test]
fn review_spans_reject_invalid_codes_times_and_calibration_claims() {
    for code in ["", "Private prompt", "UPPER", "../secret/path"] {
        assert!(ReviewReason::new(code).is_err());
    }
    assert!(serde_json::from_str::<ReviewReason>("\"not a code\"").is_err());
    assert!(validate_reviews(&[flag()]).is_ok());
    assert!(validate_reviews(&vec![flag(); MAX_REVIEW_FLAGS + 1]).is_err());
    let mut invalid = flag();
    invalid.range =
        TimeRange::new(ProjectTime::from_nanos(-1), ProjectTime::from_nanos(1)).unwrap();
    assert!(invalid.validate().is_err());
    assert!(serde_json::from_value::<ReviewFlag>(serde_json::to_value(&invalid).unwrap()).is_err());
    invalid = flag();
    invalid.confidence = Some(
        Confidence::new(
            0.9,
            Some(ArtifactId::new("unverified-calibration").unwrap()),
        )
        .unwrap(),
    );
    assert!(invalid.validate().is_err());
    assert!(serde_json::from_value::<ReviewFlag>(serde_json::to_value(invalid).unwrap()).is_err());
}

#[test]
fn candidate_descriptor_payload_defaults_review_without_inline_program() {
    let candidate = CandidateSnapshot {
        candidate_id: CandidateId::new("candidate").unwrap(),
        project_id: project(),
        base_revision: RevisionId::new(0),
        motion: MotionDescriptor {
            program: ProgramDescriptor {
                artifact_id: ArtifactId::new("artifact").unwrap(),
                sha256: "0".repeat(64),
                byte_len: 13,
                codec: MotionCodec::MotionProgramJsonV1,
                axes: vec![],
            },
            binding: MotionBinding::Candidate {
                project_id: project(),
                candidate_id: CandidateId::new("candidate").unwrap(),
                base_revision: RevisionId::new(0),
            },
        },
        job_id: None,
        review: vec![],
    };
    let mut wire = serde_json::to_value(candidate).unwrap();
    wire.as_object_mut().unwrap().remove("review");
    let decoded: CandidateSnapshot = serde_json::from_value(wire).unwrap();
    assert!(decoded.review.is_empty());
    assert!(decoded.motion.program.axes.is_empty());
}

#[test]
fn modality_unknown_fields_and_missing_image_prompt_fail_closed() {
    let mut wire = serde_json::to_value(synthesis()).unwrap();
    wire["arguments"]["shell"] = serde_json::json!("arbitrary command");
    assert!(serde_json::from_value::<GenerationInput>(wire).is_err());
    assert!(serde_json::from_str::<GenerationInput>(
        r#"{"modality":"image","arguments":{"source_version":"source"}}"#
    )
    .is_err());
}

#[test]
fn diagnostics_do_not_admit_unbounded_or_ambiguous_reports() {
    let mut report = DiagnosticsReport {
        project_id: project(),
        revision: RevisionId::new(1),
        candidate_id: None,
        base_revision: None,
        protected_regions: vec![],
        issues: vec![],
    };
    assert!(report.validate().is_ok());
    report.base_revision = Some(RevisionId::new(0));
    assert!(report.validate().is_err());
    report.base_revision = None;
    report.issues.push(DiagnosticIssue {
        code: "gap".into(),
        message: "x".repeat(513),
        axis: None,
        range: None,
    });
    assert!(report.validate().is_err());
}

#[test]
fn diagnostics_total_json_escaping_cannot_overrun_control_budget() {
    let report = DiagnosticsReport {
        project_id: project(),
        revision: RevisionId::new(0),
        candidate_id: None,
        base_revision: None,
        protected_regions: vec![],
        issues: vec![
            DiagnosticIssue {
                code: "bounded".into(),
                message: "\\".repeat(512),
                axis: None,
                range: None
            };
            MAX_DIAGNOSTIC_ISSUES
        ],
    };
    assert!(report.issues.iter().all(|issue| issue.validate().is_ok()));
    assert!(
        report.validate().is_err(),
        "individually bounded strings still require an aggregate encoded-byte limit"
    );
}

#[test]
fn all_six_checked_pattern_axes_are_admitted_once() {
    let patterns = Axis::ALL
        .iter()
        .map(|axis| {
            parse_pattern_prompt(&format!(
                "pattern=sine axis={} duration=2s frequency=1hz amplitude=0.2 offset=0.5",
                axis.as_str()
            ))
            .unwrap()
        })
        .collect();
    assert!(GenerationInput::Patterns { patterns }.validate().is_ok());
}
