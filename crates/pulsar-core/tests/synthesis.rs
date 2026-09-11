use pulsar_core::*;

fn spec(pattern: PatternKind, axis: Axis) -> PatternSpec {
    PatternSpec::new(PatternParameters {
        pattern,
        axis,
        duration: ProjectTime::from_nanos(1_000_000_000),
        frequency_hz: if pattern == PatternKind::Hold {
            0.0
        } else {
            1.0
        },
        amplitude: if pattern == PatternKind::Hold {
            0.0
        } else {
            0.2
        },
        offset: NormalizedPosition::new(0.5).unwrap(),
        phase_cycles: 0.0,
        sample_hz: 8,
        duty_cycle: 0.5,
    })
    .unwrap()
}

fn positions(program: &MotionProgram) -> Vec<f64> {
    program.tracks()[0]
        .actions()
        .iter()
        .map(|action| action.position().value())
        .collect()
}

#[test]
fn triangle_and_sine_quadrature_known_answers() {
    for pattern in [PatternKind::Sine, PatternKind::Triangle] {
        let program = synthesize_pattern(&spec(pattern, Axis::Stroke)).unwrap();
        let values = positions(&program);
        for (index, expected) in [(0, 0.5), (2, 0.7), (4, 0.5), (6, 0.3), (8, 0.5)] {
            assert!((values[index] - expected).abs() < 1e-12);
        }
        assert!(program.tracks()[0]
            .actions()
            .iter()
            .all(|action| action.evidence() == EvidenceKind::Synthesized));
    }
}

#[test]
fn pulse_preserves_high_and_low_plateaus() {
    let program = synthesize_pattern(&spec(PatternKind::Pulse, Axis::Stroke)).unwrap();
    assert_eq!(
        positions(&program),
        vec![0.7, 0.7, 0.7, 0.7, 0.3, 0.3, 0.3, 0.3, 0.7]
    );
}

#[test]
fn hold_has_no_invented_full_range_or_dense_allocation() {
    let mut params = spec(PatternKind::Hold, Axis::Stroke).parameters().clone();
    params.duration = ProjectTime::from_nanos(MAX_SYNTHESIS_DURATION_NS);
    let checked = PatternSpec::new(params).unwrap();
    assert_eq!(checked.action_count().unwrap(), 2);
    let program = synthesize_pattern(&checked).unwrap();
    assert_eq!(positions(&program), vec![0.5, 0.5]);
    assert_eq!(
        program.tracks()[0].actions()[1].time().as_nanos(),
        MAX_SYNTHESIS_DURATION_NS
    );
}

#[test]
fn exact_duration_and_phase_are_preserved() {
    let checked = parse_pattern_prompt("pattern=triangle axis=stroke duration=1.000000001s frequency=1hz amplitude=0.2 offset=0.5 phase=0.25 sample_hz=8").unwrap();
    let output = synthesize_pattern(&checked).unwrap();
    let actions = output.tracks()[0].actions();
    assert_eq!(actions.first().unwrap().position().value(), 0.7);
    assert_eq!(actions.last().unwrap().time().as_nanos(), 1_000_000_001);
    assert!(actions
        .windows(2)
        .all(|pair| pair[0].time() < pair[1].time()));
}

#[test]
fn strict_prompt_rejects_unknown_intent_duplicates_units_and_numeric_aliases() {
    let valid = "pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5";
    assert!(parse_pattern_prompt(valid).is_ok());
    for bad in [
        "make this exciting",
        "pattern=sine",
        "pattern=unknown axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5",
        "pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5 axis=yaw",
        "pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5 shell=rm",
        "pattern=sine axis=stroke duration=5 frequency=1hz amplitude=0.2 offset=0.5",
        "pattern=sine axis=stroke duration=1.0000000001s frequency=1hz amplitude=0.2 offset=0.5",
        "pattern=sine axis=stroke duration=5s frequency=NaNhz amplitude=0.2 offset=0.5",
        "pattern=sine axis=stroke duration=5s frequency=1hz amplitude=2e-1 offset=0.5",
        "pattern=hold axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5",
    ] {
        assert!(parse_pattern_prompt(bad).is_err(), "{bad}");
    }
    assert!(validate_still_image_prompt(None).is_err());
    assert!(validate_still_image_prompt(Some(" ")).is_err());
    assert!(validate_still_image_prompt(Some(valid)).is_ok());
}

#[test]
fn checked_deserialization_cannot_bypass_numeric_or_action_budgets() {
    let checked = spec(PatternKind::Sine, Axis::Stroke);
    let mut wire = serde_json::to_value(&checked).unwrap();
    wire["amplitude"] = serde_json::json!(2.0);
    assert!(serde_json::from_value::<PatternSpec>(wire).is_err());
    let mut params = checked.parameters().clone();
    params.duration = ProjectTime::from_nanos(MAX_SYNTHESIS_DURATION_NS);
    params.sample_hz = 1000;
    assert!(PatternSpec::new(params).is_err());
    assert!(serde_json::from_str::<AudioEnvelopeSample>(r#"{"time":0,"level":1.1}"#).is_err());
    assert!(serde_json::from_str::<AudioMapping>(
        r#"{"axis":"stroke","amplitude":0.8,"offset":0.5}"#
    )
    .is_err());
}

#[test]
fn six_axes_share_duration_without_implicit_missing_axis_defaults() {
    let specs: Vec<_> = Axis::ALL
        .into_iter()
        .map(|axis| spec(PatternKind::Triangle, axis))
        .collect();
    let program = synthesize_patterns(&specs).unwrap();
    assert_eq!(program.tracks().len(), 6);
    assert!(synthesize_patterns(&[specs[0].clone(), specs[0].clone()]).is_err());
    let single = synthesize_pattern(&specs[0]).unwrap();
    assert_eq!(single.tracks().len(), 1);
}

#[test]
fn bounded_parameter_sweep_preserves_amplitude_and_monotonic_time() {
    for pattern in [PatternKind::Sine, PatternKind::Triangle, PatternKind::Pulse] {
        for amplitude_step in 0..=10 {
            for phase_step in 0..=7 {
                let amplitude = amplitude_step as f64 / 20.0;
                let mut params = spec(pattern, Axis::Stroke).parameters().clone();
                params.amplitude = amplitude;
                params.phase_cycles = phase_step as f64 / 8.0;
                let program = synthesize_pattern(&PatternSpec::new(params).unwrap()).unwrap();
                let actions = program.tracks()[0].actions();
                assert!(actions.iter().all(|action| action.position().value()
                    >= 0.5 - amplitude - 1e-14
                    && action.position().value() <= 0.5 + amplitude + 1e-14));
                assert!(actions
                    .windows(2)
                    .all(|pair| pair[0].time() < pair[1].time()));
            }
        }
    }
}

#[test]
fn envelope_mapping_known_answer_and_amplitude_scaling_metamorphism() {
    let samples: Vec<_> = [0.0, 0.25, 1.0, 0.0]
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            AudioEnvelopeSample::new(ProjectTime::from_nanos(i as i64 * 10), value).unwrap()
        })
        .collect();
    let mapping =
        AudioMapping::new(Axis::Stroke, 0.4, NormalizedPosition::new(0.2).unwrap()).unwrap();
    let program = synthesize_audio_envelope(&samples, &mapping).unwrap();
    for (actual, expected) in positions(&program).into_iter().zip([0.2, 0.3, 0.6, 0.2]) {
        assert!((actual - expected).abs() < 1e-12);
    }
    let half = AudioMapping::new(Axis::Stroke, 0.2, NormalizedPosition::new(0.2).unwrap()).unwrap();
    let half_program = synthesize_audio_envelope(&samples, &half).unwrap();
    for (full, half) in positions(&program)
        .into_iter()
        .zip(positions(&half_program))
    {
        assert!(((full - 0.2) * 0.5 - (half - 0.2)).abs() < 1e-12);
    }
}

#[test]
fn unavailable_audio_prefix_remains_gap_and_unordered_features_fail() {
    let sample = AudioEnvelopeSample::new(ProjectTime::from_nanos(10), 0.0).unwrap();
    let mapping =
        AudioMapping::new(Axis::Stroke, 0.3, NormalizedPosition::new(0.5).unwrap()).unwrap();
    let program = synthesize_audio_envelope(std::slice::from_ref(&sample), &mapping).unwrap();
    assert!(program.has_unresolved_gaps());
    assert_eq!(positions(&program), vec![0.5]);
    assert!(synthesize_audio_envelope(&[sample.clone(), sample], &mapping).is_err());
    assert!(synthesize_audio_envelope(&[], &mapping).is_err());
}

#[test]
fn beats_preserve_peak_times_pauses_and_shallow_amplitude() {
    let mapping = BeatMapping::new(
        Axis::Stroke,
        0.1,
        NormalizedPosition::new(0.4).unwrap(),
        10,
        20,
    )
    .unwrap();
    let program = synthesize_beats(
        &[ProjectTime::from_nanos(20), ProjectTime::from_nanos(80)],
        ProjectTime::from_nanos(100),
        &mapping,
    )
    .unwrap();
    let actual: Vec<_> = program.tracks()[0]
        .actions()
        .iter()
        .map(|a| (a.time().as_nanos(), a.position().value()))
        .collect();
    assert_eq!(
        actual,
        vec![
            (0, 0.4),
            (10, 0.4),
            (20, 0.5),
            (40, 0.4),
            (70, 0.4),
            (80, 0.5),
            (100, 0.4)
        ]
    );
    assert!(synthesize_beats(
        &[ProjectTime::from_nanos(20), ProjectTime::from_nanos(30)],
        ProjectTime::from_nanos(100),
        &mapping
    )
    .is_err());
    assert!(synthesize_beats(
        &[ProjectTime::from_nanos(5)],
        ProjectTime::from_nanos(100),
        &mapping
    )
    .is_err());
    assert!(synthesize_beats(
        &[ProjectTime::from_nanos(90)],
        ProjectTime::from_nanos(100),
        &mapping
    )
    .is_err());
}

#[test]
fn empty_beat_sequence_is_an_explicit_stationary_hold() {
    let mapping = BeatMapping::new(
        Axis::Stroke,
        0.2,
        NormalizedPosition::new(0.3).unwrap(),
        10,
        10,
    )
    .unwrap();
    let program = synthesize_beats(&[], ProjectTime::from_nanos(100), &mapping).unwrap();
    assert_eq!(positions(&program), vec![0.3, 0.3]);
}

#[test]
fn rms_features_and_onset_threshold_have_known_answers() {
    assert_eq!(AUDIO_ONSET_THRESHOLD, 0.35);
    let features = extract_audio_features(&[0.34, 0.36, 0.37, 0.1, 0.36], 100, 1).unwrap();
    assert_eq!(
        features.amplitude_onsets,
        vec![
            ProjectTime::from_nanos(10_000_000),
            ProjectTime::from_nanos(40_000_000)
        ]
    );
    assert_eq!(features.duration, ProjectTime::from_nanos(50_000_000));
    assert_eq!(features.envelope.len(), 6);
    let rms = extract_audio_features(&[0.5, -0.5, 0.5, -0.5], 4, 4).unwrap();
    assert_eq!(rms.envelope[0].level(), 0.5);
    assert_eq!(rms.envelope[1].level(), 0.5);
    assert_eq!(rms.amplitude_onsets, vec![ProjectTime::ZERO]);
}

#[test]
fn audio_sample_time_is_integer_quantized_and_final_endpoint_exact() {
    let features = extract_audio_features(&[0.0, 0.5, 0.0], 3000, 1).unwrap();
    let times: Vec<_> = features
        .envelope
        .iter()
        .map(|sample| sample.time().as_nanos())
        .collect();
    assert_eq!(times, vec![0, 333_333, 666_666, 1_000_000]);
    assert_eq!(
        features.amplitude_onsets,
        vec![ProjectTime::from_nanos(333_333)]
    );
}

#[test]
fn quiet_pcm_and_amplitude_scaling_do_not_invent_full_range_motion() {
    let quiet = extract_audio_features(&[0.01; 100], 100, 10).unwrap();
    assert!(quiet.amplitude_onsets.is_empty());
    assert!(quiet
        .envelope
        .iter()
        .all(|sample| (sample.level() - 0.01).abs() < 1e-8));
    let mapping =
        AudioMapping::new(Axis::Stroke, 0.2, NormalizedPosition::new(0.5).unwrap()).unwrap();
    let motion = synthesize_audio_envelope(&quiet.envelope, &mapping).unwrap();
    assert!(positions(&motion)
        .iter()
        .all(|position| (*position - 0.502).abs() < 1e-8));
    let full = extract_audio_features(&[0.2, -0.2, 0.4, -0.4], 4, 2).unwrap();
    let half = extract_audio_features(&[0.1, -0.1, 0.2, -0.2], 4, 2).unwrap();
    for (full, half) in full.envelope.iter().zip(&half.envelope) {
        assert_eq!(full.time(), half.time());
        assert!((full.level() * 0.5 - half.level()).abs() < 1e-12);
    }
}

#[test]
fn malformed_pcm_and_feature_resource_inputs_fail_closed() {
    for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.01, -1.01] {
        assert!(extract_audio_features(&[sample], 100, 1).is_err());
    }
    assert!(extract_audio_features(&[], 100, 1).is_err());
    assert!(extract_audio_features(&[0.0], 0, 1).is_err());
    assert!(extract_audio_features(&[0.0], 192001, 1).is_err());
    assert!(extract_audio_features(&[0.0], 100, 0).is_err());
    assert!(extract_audio_features(&[0.0], 100, 101).is_err());
    let features = extract_audio_features(&[0.0; 10], 100, 3).unwrap();
    assert!(features.envelope.iter().all(|sample| sample.level() == 0.0));
    assert!(features.amplitude_onsets.is_empty());
}
