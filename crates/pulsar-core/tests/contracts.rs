use pulsar_core::*;

fn action(time: i64, position: f64) -> MotionAction {
    MotionAction::new(
        ProjectTime::from_nanos(time),
        NormalizedPosition::new(position).unwrap(),
        EvidenceKind::Observed,
    )
    .unwrap()
}
fn program(position: f64) -> MotionProgram {
    MotionProgram::new(vec![MotionTrack::new(
        Axis::Stroke,
        vec![action(0, position), action(1_000_000_000, position)],
    )
    .unwrap()])
    .unwrap()
}
fn grant(project: &ProjectId, override_protection: bool) -> CapabilityGrant {
    let mut permissions = vec![Capability::Edit];
    if override_protection {
        permissions.push(Capability::OverrideProtection);
    }
    CapabilityGrant::new(
        GrantId::new("grant").unwrap(),
        ClientId::new("client").unwrap(),
        Some(project.clone()),
        permissions,
    )
}
fn context() -> FrameContext {
    FrameContext {
        source_version: SourceVersionId::new("source-v1").unwrap(),
        source_placement: SourcePlacementId::new("placement-1").unwrap(),
        frame: FrameId::new(12),
        transform: TransformId::new("transform-1").unwrap(),
        seek_generation: 2,
        request_generation: 3,
    }
}

#[test]
fn identifiers_reject_empty_path_unicode_and_wire_bypass() {
    for value in ["", "../path/file", "bad id", "\0", "caf\u{e9}"] {
        assert!(ProjectId::new(value).is_err());
        assert!(serde_json::from_str::<ProjectId>(&serde_json::to_string(value).unwrap()).is_err());
    }
    assert!(ProjectId::new("a".repeat(129)).is_err());
    let id = ProjectId::new("project:123_a.b-c").unwrap();
    assert_eq!(
        serde_json::from_str::<ProjectId>(&serde_json::to_string(&id).unwrap()).unwrap(),
        id
    );
}

#[test]
fn numeric_domains_do_not_wrap_or_accept_nonfinite() {
    assert!(RevisionId::new(u64::MAX).checked_next().is_err());
    assert!(ProjectTime::from_nanos(i64::MAX).checked_add(1).is_err());
    assert!(ProjectTime::from_nanos(i64::MIN)
        .checked_sub(ProjectTime::from_nanos(1))
        .is_err());
    assert!(MonotonicDeadline::from_nanos(u64::MAX)
        .checked_add(1)
        .is_err());
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
        assert!(NormalizedPosition::new(value).is_err());
    }
    assert!(serde_json::from_str::<NormalizedPosition>("1.1").is_err());
}

#[test]
fn rational_timestamps_preserve_quantization_and_negative_direction() {
    let positive = RationalTimestamp::new(1, 3)
        .unwrap()
        .quantize_nanoseconds()
        .unwrap();
    assert_eq!(positive.time.as_nanos(), 333_333_333);
    assert_eq!(positive.remainder_numerator, 1);
    let negative = RationalTimestamp::new(-1, 3)
        .unwrap()
        .quantize_nanoseconds()
        .unwrap();
    assert_eq!(negative.time.as_nanos(), -333_333_334);
    assert_eq!(negative.remainder_numerator, 2);
    assert!(RationalTimestamp::new(i64::MAX, 1)
        .unwrap()
        .quantize_nanoseconds()
        .is_err());
    assert!(
        serde_json::from_str::<RationalTimestamp>(r#"{"numerator":1,"denominator":0}"#).is_err()
    );
    assert!(serde_json::from_str::<TimeRange>(r#"{"start":2,"end":1}"#).is_err());
}

#[test]
fn box_clipping_preserves_center_and_rejects_bad_geometry() {
    let bounds = CoordinateBox::new(CoordinateSpace::SourcePixels, -10.0, 0.0, 30.0, 40.0).unwrap();
    let clipped = bounds.clipped_to(20.0, 20.0).unwrap().unwrap();
    assert_eq!(clipped.center(), (10.0, 10.0));
    assert_eq!(clipped.width(), 20.0);
    assert!(CoordinateBox::new(CoordinateSpace::SourcePixels, 2.0, 0.0, 1.0, 1.0).is_err());
    assert!(
        CoordinateBox::new(CoordinateSpace::SourcePixels, -f64::MAX, 0.0, f64::MAX, 1.0).is_err()
    );
    assert!(CoordinateBox::new(CoordinateSpace::ProjectionNormalized, 0.0, 0.0, 2.0, 1.0).is_err());
    assert!(serde_json::from_str::<CoordinateBox>(
        r#"{"space":"source_pixels","x_min":5,"y_min":0,"x_max":1,"y_max":1}"#
    )
    .is_err());
}

#[test]
fn every_frame_identity_dimension_fences_stale_observations() {
    let original = context();
    let observation = DetectionObservation::new(
        original.clone(),
        CoordinateBox::new(CoordinateSpace::SourcePixels, 0.0, 0.0, 10.0, 10.0).unwrap(),
        None,
        EvidenceKind::Observed,
        Confidence::new(0.9, None).unwrap(),
    )
    .unwrap();
    assert!(observation.matches_frame(&original));
    for field in 0..6 {
        let mut changed = original.clone();
        match field {
            0 => changed.source_version = SourceVersionId::new("other").unwrap(),
            1 => changed.source_placement = SourcePlacementId::new("other").unwrap(),
            2 => changed.frame = FrameId::new(13),
            3 => changed.transform = TransformId::new("other").unwrap(),
            4 => changed.seek_generation += 1,
            _ => changed.request_generation += 1,
        }
        assert!(!observation.matches_frame(&changed));
    }
    let wire = serde_json::to_string(&observation)
        .unwrap()
        .replace("\"observed\"", "\"unavailable\"");
    assert!(serde_json::from_str::<DetectionObservation>(&wire).is_err());
}

#[test]
fn motion_keeps_shallow_plateaus_without_range_expansion() {
    let original = program(0.37);
    let decoded: MotionProgram =
        serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(original, decoded);
    assert_eq!(decoded.tracks()[0].actions()[1].position().value(), 0.37);
    assert!(decoded.track(Axis::Yaw).is_none());
}

#[test]
fn malformed_motion_cannot_cross_constructor_or_serde() {
    assert!(MotionTrack::new(Axis::Stroke, vec![action(1, 0.2), action(1, 0.3)]).is_err());
    assert!(MotionTrack::new(Axis::Stroke, vec![action(2, 0.2), action(1, 0.3)]).is_err());
    let track = MotionTrack::new(Axis::Stroke, vec![]).unwrap();
    assert!(MotionProgram::new(vec![track.clone(), track]).is_err());
    assert!(MotionAction::new(
        ProjectTime::from_nanos(-1),
        NormalizedPosition::new(0.5).unwrap(),
        EvidenceKind::Observed
    )
    .is_err());
    assert!(MotionAction::new(
        ProjectTime::ZERO,
        NormalizedPosition::new(0.5).unwrap(),
        EvidenceKind::Unavailable
    )
    .is_err());
    assert!(serde_json::from_str::<MotionProgram>(r#"{"tracks":[{"name":"stroke","axis":"stroke","actions":[{"time":5,"position":0.1,"evidence":"observed"},{"time":1,"position":0.2,"evidence":"observed"}]}]}"#).is_err());
}

#[test]
fn project_edits_are_atomic_revision_checked_and_undo_creates_new_revision() {
    let id = ProjectId::new("project").unwrap();
    let authority = grant(&id, false);
    let mut state = ProjectState::new(id, program(0.2));
    state
        .apply_edit(
            RevisionId::new(0),
            &authority,
            ProjectEdit::ReplaceProgram(program(0.3)),
        )
        .unwrap();
    let before = state.snapshot().clone();
    assert!(state
        .apply_edit(
            RevisionId::new(0),
            &authority,
            ProjectEdit::ReplaceProgram(program(0.4))
        )
        .is_err());
    assert_eq!(*state.snapshot(), before);
    let receipt = state.undo(RevisionId::new(1), &authority).unwrap();
    assert_eq!(receipt.revision, RevisionId::new(2));
    assert_eq!(receipt.actor, ClientId::new("client").unwrap());
    assert_eq!(state.snapshot().program, program(0.2));
    state.redo(RevisionId::new(2), &authority).unwrap();
    assert_eq!(state.snapshot().revision, RevisionId::new(3));
}

#[test]
fn protection_and_revocation_are_rechecked_for_edits_and_undo() {
    let id = ProjectId::new("project").unwrap();
    let privileged = grant(&id, true);
    let mut ordinary = grant(&id, false);
    let mut state = ProjectState::new(id, program(0.2));
    state
        .apply_edit(
            RevisionId::new(0),
            &privileged,
            ProjectEdit::SetProtection(vec![ProtectedRegion {
                axis: Some(Axis::Stroke),
                range: TimeRange::new(ProjectTime::ZERO, ProjectTime::from_nanos(2_000_000_000))
                    .unwrap(),
            }]),
        )
        .unwrap();
    let before = state.snapshot().clone();
    assert_eq!(
        state
            .apply_edit(
                RevisionId::new(1),
                &ordinary,
                ProjectEdit::ReplaceProgram(program(0.4))
            )
            .unwrap_err(),
        CoreError::ProtectedRegion
    );
    assert!(state.undo(RevisionId::new(1), &ordinary).is_err());
    assert_eq!(*state.snapshot(), before);
    ordinary.revoke();
    assert!(state
        .apply_edit(
            RevisionId::new(1),
            &ordinary,
            ProjectEdit::ReplaceProgram(program(0.2))
        )
        .is_err());
}

#[test]
fn project_scope_and_stale_candidate_do_not_leak_authority() {
    let project = ProjectId::new("project").unwrap();
    let authority = grant(&project, false);
    let wrong = grant(&ProjectId::new("other").unwrap(), true);
    let mut state = ProjectState::new(project.clone(), program(0.2));
    assert!(state
        .apply_edit(
            RevisionId::new(0),
            &wrong,
            ProjectEdit::ReplaceProgram(program(0.4))
        )
        .is_err());
    let candidate = MotionCandidate {
        id: CandidateId::new("candidate").unwrap(),
        project,
        base_revision: RevisionId::new(0),
        program: program(0.7),
        job: JobId::new("job").unwrap(),
        attempt: AttemptId::new("attempt").unwrap(),
    };
    state
        .apply_edit(
            RevisionId::new(0),
            &authority,
            ProjectEdit::ReplaceProgram(program(0.3)),
        )
        .unwrap();
    assert!(state
        .commit_candidate(RevisionId::new(1), &authority, &candidate)
        .is_err());
    assert_eq!(state.snapshot().program, program(0.3));
}

#[test]
fn overflow_rejects_whole_project_transition() {
    let id = ProjectId::new("project").unwrap();
    let authority = grant(&id, true);
    let snapshot = ProjectSnapshot {
        project: id,
        revision: RevisionId::new(u64::MAX),
        program: program(0.2),
        protected_regions: vec![],
    };
    let mut state = ProjectState::from_snapshot(snapshot.clone());
    assert!(state
        .apply_edit(
            RevisionId::new(u64::MAX),
            &authority,
            ProjectEdit::ReplaceProgram(program(0.3))
        )
        .is_err());
    assert_eq!(*state.snapshot(), snapshot);
}

#[test]
fn cancellation_fences_late_attempts_and_results() {
    let attempt = AttemptId::new("attempt").unwrap();
    let mut job = JobLifecycle::new(
        JobId::new("job").unwrap(),
        attempt.clone(),
        GrantId::new("grant").unwrap(),
        RevisionId::new(0),
    );
    job.apply(&attempt, JobEvent::Start).unwrap();
    job.apply(&attempt, JobEvent::RevokeAuthority).unwrap();
    assert!(job.apply(&attempt, JobEvent::WorkerCompleted).is_err());
    assert_eq!(job.state(), JobState::CancelRequested);
    assert!(job
        .apply(
            &AttemptId::new("old-attempt").unwrap(),
            JobEvent::AcknowledgeCancel
        )
        .is_err());
    job.apply(&attempt, JobEvent::AcknowledgeCancel).unwrap();
    assert_eq!(job.state(), JobState::Cancelled);
    assert!(JobState::Completed.transition(JobEvent::Start).is_err());
}

fn qualified_session(headless: bool) -> (DeviceSession, ClientId, DeviceSessionId) {
    let device = DeviceId::new("device").unwrap();
    let session_id = DeviceSessionId::new("session").unwrap();
    let controller = ClientId::new("controller").unwrap();
    let config = DeviceConfiguration {
        device: device.clone(),
        firmware: ArtifactId::new("firmware").unwrap(),
        driver: ArtifactId::new("driver").unwrap(),
        transport: ArtifactId::new("transport").unwrap(),
        profile: ArtifactId::new("profile").unwrap(),
    };
    let qualification = DeviceQualification::new(
        config.clone(),
        ArtifactId::new("record").unwrap(),
        QualificationAuthority::LocalHumanAcceptance,
        Some(100),
    )
    .unwrap();
    let approval = if headless {
        Some(HeadlessApproval::new(&qualification).unwrap())
    } else {
        None
    };
    let admission = PlaybackAdmission {
        project: ProjectId::new("project").unwrap(),
        revision: RevisionId::new(2),
        configuration: config,
        program_artifact: ArtifactId::new("program").unwrap(),
    };
    let mut session = DeviceSession::new(device, session_id.clone());
    session
        .arm(
            controller.clone(),
            admission,
            &qualification,
            approval.as_ref(),
        )
        .unwrap();
    (session, controller, session_id)
}

#[test]
fn stop_request_does_not_prove_physical_stopping() {
    let (mut session, controller, session_id) = qualified_session(false);
    session.start(&controller, &session_id).unwrap();
    session.controller_lost();
    assert_eq!(session.state(), DeviceState::StopRequested);
    assert!(session
        .confirm_stopped(
            &session_id,
            StopEvidence::QualifiedBoundElapsed { elapsed_ns: 99 }
        )
        .is_err());
    session.mark_stop_unconfirmed().unwrap();
    assert_eq!(session.state(), DeviceState::StopUnconfirmed);
    session
        .confirm_stopped(
            &session_id,
            StopEvidence::QualifiedBoundElapsed { elapsed_ns: 100 },
        )
        .unwrap();
    assert_eq!(session.state(), DeviceState::Disarmed);
    assert!(session.start(&controller, &session_id).is_err());
}

#[test]
fn explicit_gaps_survive_wire_and_reject_interpolated_observations() {
    let gap = TimeRange::new(ProjectTime::from_nanos(1), ProjectTime::from_nanos(10)).unwrap();
    let track = MotionTrack::with_gaps(
        Axis::Stroke,
        vec![action(0, 0.5), action(10, 0.6)],
        vec![gap],
    )
    .unwrap();
    let program = MotionProgram::new(vec![track]).unwrap();
    let restored: MotionProgram =
        serde_json::from_str(&serde_json::to_string(&program).unwrap()).unwrap();
    assert!(restored.has_unresolved_gaps());
    assert_eq!(restored.tracks()[0].gaps(), &[gap]);
    assert!(MotionTrack::with_gaps(Axis::Stroke, vec![action(5, 0.5)], vec![gap]).is_err());
    assert!(MotionTrack::with_gaps(Axis::Stroke, vec![], vec![gap, gap]).is_err());
}

#[test]
fn reconnect_preserves_stop_uncertainty_and_blocks_rearm() {
    let (mut session, controller, session_id) = qualified_session(false);
    let admission = session.admission().unwrap().clone();
    let qualification = DeviceQualification::new(
        admission.configuration.clone(),
        ArtifactId::new("record").unwrap(),
        QualificationAuthority::OfficialRecord,
        Some(100),
    )
    .unwrap();
    session.start(&controller, &session_id).unwrap();
    let new_session = DeviceSessionId::new("replacement-session").unwrap();
    session.reconnect(new_session.clone()).unwrap();
    assert!(session.physical_stop_pending());
    assert!(session
        .arm(controller.clone(), admission.clone(), &qualification, None)
        .is_err());
    assert!(session
        .confirm_stopped(
            &new_session,
            StopEvidence::QualifiedBoundElapsed { elapsed_ns: 100 }
        )
        .is_err());
    session
        .confirm_stopped(&new_session, StopEvidence::DeviceObservedStopped)
        .unwrap();
    assert!(!session.physical_stop_pending());
    session
        .arm(controller, admission, &qualification, None)
        .unwrap();
}

#[test]
fn headless_disconnect_differs_from_revocation_and_reconnect_is_disarmed() {
    let (mut session, controller, session_id) = qualified_session(true);
    session.start(&controller, &session_id).unwrap();
    session.controller_lost();
    assert_eq!(session.state(), DeviceState::Playing);
    session.revoke_controller();
    assert_eq!(session.state(), DeviceState::StopRequested);
    assert!(session.reconnect(session_id.clone()).is_err());
    session
        .reconnect(DeviceSessionId::new("new-session").unwrap())
        .unwrap();
    assert_eq!(session.state(), DeviceState::Disarmed);
    assert!(session.start(&controller, &session_id).is_err());
}

#[test]
fn device_configuration_and_stopping_qualification_fail_closed() {
    let (session, _, _) = qualified_session(false);
    let mut config = session.admission().unwrap().configuration.clone();
    let qualification = DeviceQualification::new(
        config.clone(),
        ArtifactId::new("record").unwrap(),
        QualificationAuthority::OfficialRecord,
        None,
    )
    .unwrap();
    assert!(HeadlessApproval::new(&qualification).is_err());
    config.driver = ArtifactId::new("changed-driver").unwrap();
    let mut fresh = DeviceSession::new(
        config.device.clone(),
        DeviceSessionId::new("fresh").unwrap(),
    );
    let admission = PlaybackAdmission {
        project: ProjectId::new("project").unwrap(),
        revision: RevisionId::new(0),
        configuration: config,
        program_artifact: ArtifactId::new("program").unwrap(),
    };
    assert_eq!(
        fresh
            .arm(
                ClientId::new("client").unwrap(),
                admission,
                &qualification,
                None
            )
            .unwrap_err(),
        CoreError::MissingQualification
    );
    assert_eq!(fresh.state(), DeviceState::Disarmed);
}
