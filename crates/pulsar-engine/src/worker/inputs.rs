//! Bounded effect adapter for explicit synthetic and audio-derived candidates.
//! No prompt is a shell command; no modality can bypass immutable input checks.

use super::*;

const AUDIO_SAMPLE_RATE: u32 = 16_000;
const AUDIO_WINDOW_SAMPLES: u32 = 160;

pub(super) fn generate(
    request: &WorkerRequest,
    input: &GenerationInput,
    deadline: Instant,
) -> Result<WorkerOutput> {
    input.validate()?;
    let mut notes = Vec::new();
    let (program, recipe, details) = match input {
        GenerationInput::Text { prompt } => {
            check_declared_source(request, input)?;
            let spec = parse_pattern_prompt(prompt)?;
            notes.push("constrained_pattern_parser_not_local_ai");
            (
                synthesize_pattern(&spec)?,
                "explicit-text-pattern-v1",
                serde_json::json!({"language": "strict-pattern-parameters", "natural_language_inference": false}),
            )
        }
        GenerationInput::Patterns { patterns } => {
            check_declared_source(request, input)?;
            (
                synthesize_patterns(patterns)?,
                "explicit-six-axis-pattern-v1",
                serde_json::json!({"observed_motion": false}),
            )
        }
        GenerationInput::Image { prompt, .. } => {
            let spec = validate_still_image_prompt(Some(prompt))?;
            let index = media::probe(&request.tools.ffprobe.path, &request.source.path, deadline)?;
            if index.pts.len() != 1 {
                return Err(ProtocolError::invalid(
                    "image input requires exactly one decoded still frame",
                )
                .into());
            }
            let (width, height) = media::fit_dimensions(index.width, index.height, 64, 64)?;
            let _pixels = media::extract(
                &request.tools.ffmpeg.path,
                &request.source.path,
                0,
                width,
                height,
                request.budget.cpu_threads,
                deadline,
            )?;
            notes.push("image_not_semantically_conditioning_motion");
            notes.push("constrained_pattern_parser_not_local_ai");
            (
                synthesize_pattern(&spec)?,
                "validated-still-plus-explicit-pattern-v1",
                serde_json::json!({
                    "image_dimensions": [index.width, index.height], "image_semantic_inference": false,
                    "conditioning": "prompt pattern only; image bytes decoded and bound to lineage"
                }),
            )
        }
        GenerationInput::Audio { mapping, mode, .. } => {
            let decoded = super::audio::decode(request, deadline)?;
            let features =
                extract_audio_features(&decoded.samples, AUDIO_SAMPLE_RATE, AUDIO_WINDOW_SAMPLES)?;
            notes.push("audio_mapping_uncalibrated");
            match mode {
                AudioMode::Envelope => (
                    synthesize_audio_envelope(&features.envelope, mapping)?,
                    "full-band-rms-envelope-v1",
                    serde_json::json!({"sample_rate": AUDIO_SAMPLE_RATE, "window_samples": AUDIO_WINDOW_SAMPLES,
                        "channels": 1, "band": "full", "normalization": "none", "resampled_sample_count": decoded.samples.len(), "source_timeline": decoded.timeline}),
                ),
                AudioMode::Beats => {
                    // This mode generates pulses at RMS threshold onsets, not a
                    // claim of musical beat tracking. Unsupported edge/overlap
                    // pulses are omitted and reported, never retimed.
                    let rise = 50_000_000u64;
                    let fall = 100_000_000u64;
                    let mut selected = Vec::new();
                    let mut omitted = 0usize;
                    for onset in &features.amplitude_onsets {
                        let t = onset.as_nanos();
                        let fits = t >= rise as i64
                            && t.checked_add(fall as i64)
                                .is_some_and(|end| end <= features.duration.as_nanos())
                            && selected.last().is_none_or(|last: &ProjectTime| {
                                t - last.as_nanos() >= (rise + fall) as i64
                            });
                        if fits {
                            selected.push(*onset);
                        } else {
                            omitted += 1;
                        }
                    }
                    notes.push("amplitude_onsets_not_musical_beat_tracking");
                    if omitted > 0 {
                        notes.push("unsupported_onset_pulses_omitted");
                    }
                    (
                        synthesize_beats(
                            &selected,
                            features.duration,
                            &BeatMapping::new(
                                mapping.axis(),
                                mapping.amplitude(),
                                mapping.offset(),
                                rise,
                                fall,
                            )?,
                        )?,
                        "full-band-rms-onset-pulses-v1",
                        serde_json::json!({"sample_rate": AUDIO_SAMPLE_RATE, "window_samples": AUDIO_WINDOW_SAMPLES,
                            "onset_threshold": AUDIO_ONSET_THRESHOLD, "rise_ns": rise, "fall_ns": fall,
                            "raw_onsets": features.amplitude_onsets.len(), "retained_onsets": selected.len(),
                            "omitted_edge_or_overlapping_onsets": omitted, "musical_beat_tracking": false,
                            "band": "full", "normalization": "none", "resampled_sample_count": decoded.samples.len(), "source_timeline": decoded.timeline}),
                    )
                }
            }
        }
    };
    if Instant::now() >= deadline {
        bail!("synthetic generation deadline exceeded");
    }
    let duration = program
        .tracks()
        .iter()
        .flat_map(|track| track.actions())
        .map(|action| action.time())
        .max()
        .unwrap_or(ProjectTime::ZERO);
    let mut spans = ReviewSpans::new();
    for note in notes {
        record_review(
            &mut spans,
            note.to_owned(),
            ProjectTime::ZERO,
            duration.checked_add(1)?,
            EvidenceKind::Synthesized,
        )?;
    }
    let review = finish_reviews(spans, None)?;
    let program_artifact = artifacts::write_json(
        &request.output_dir,
        "candidate.json",
        &program,
        request.budget.output_bytes,
    )?;
    let mut lineage = vec![request.source.identity.clone()];
    for dependency in request.dependencies.iter().chain([
        &request.tools.ffmpeg.identity,
        &request.tools.ffprobe.identity,
    ]) {
        if !lineage.contains(dependency) {
            lineage.push(dependency.clone());
        }
    }
    let configuration = serde_json::to_vec(input)?;
    let receipt = serde_json::json!({
        "schema_version": 2, "recipe": recipe, "qualification": "unqualified",
        "project_id": request.project_id, "base_revision": request.base_revision,
        "job_id": request.job_id, "attempt_id": request.attempt_id,
        "source_version": request.source.source_version, "program": program_artifact.identity,
        "configuration_sha256": format!("{:x}", Sha256::digest(configuration)),
        "dependencies": lineage, "details": details, "review": review,
        "evidence": "synthesized", "confidence_calibration": "none",
        "claim": "Deterministic synthetic mapping, not neural, creative-quality, performance or device qualification"
    });
    let remaining = request
        .budget
        .output_bytes
        .checked_sub(program_artifact.identity.byte_len)
        .context("candidate artifact budget exhausted")?;
    let receipt = artifacts::write_json(&request.output_dir, "lineage.json", &receipt, remaining)?;
    Ok(WorkerOutput::Generated {
        program: program_artifact,
        receipt,
        lineage,
        review,
    })
}

fn check_declared_source(request: &WorkerRequest, input: &GenerationInput) -> Result<()> {
    let expected = serde_json::to_vec(input)?;
    if expected.len() as u64 > request.budget.memory_bytes / 16 {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "synthetic input exceeds memory admission",
        )
        .into());
    }
    let file = std::fs::File::open(&request.source.path)?;
    let mut actual = Vec::new();
    file.take(expected.len() as u64 + 1)
        .read_to_end(&mut actual)?;
    if actual != expected {
        return Err(ProtocolError::new(
            ErrorCode::DependencyMismatch,
            "synthetic input differs from immutable source snapshot",
        )
        .into());
    }
    Ok(())
}
