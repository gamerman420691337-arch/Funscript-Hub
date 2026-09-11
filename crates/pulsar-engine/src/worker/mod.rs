//! Native-worker entry point and media-to-candidate adapter.
//!
//! The engine owns admission, confinement, cancellation, project transactions,
//! and artifact publication. This module executes one immutable attempt, writes
//! only new files in its engine-approved directory, and never commits a project.

mod artifacts;
mod audio;
mod detector;
#[cfg(test)]
mod good_tests;
mod inputs;
#[cfg(test)]
mod integration_tests;
mod media;
mod tracking;

pub use artifacts::{discover_tools, pin_artifact};
pub type WorkerError = anyhow::Error;

use anyhow::{bail, Context, Result};
use pulsar_protocol::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::{Duration, Instant};

const MAX_SAMPLED_FRAMES: usize = 1_000_000;

/// One framed request and one framed response. Native stdout is never inherited.
/// Parent-enforced limits include the time spent reading an incomplete request.
pub fn run_worker() -> Result<()> {
    let request: WorkerRequest = read_message(&mut std::io::stdin().lock())?;
    let result = execute_manifest(&request);
    let mut stdout = std::io::stdout().lock();
    write_message(&mut stdout, &result)?;
    // Files live only in the worker's private, quota-bounded tmpfs. The engine
    // validates this bounded egress and writes its own durable attempt copies.
    // No native worker receives a writable host project/artifact directory.
    if let Ok(output) = &result.result {
        let references: Vec<&ArtifactReference> = match output {
            WorkerOutput::Generated {
                program, receipt, ..
            } => vec![program, receipt],
            WorkerOutput::Preview(preview) => {
                preview.frame.iter().map(|frame| &frame.artifact).collect()
            }
        };
        let mut total = 0u64;
        let mut buffer = vec![0u8; MAX_ARTIFACT_CHUNK_BYTES];
        for artifact in references {
            total = total
                .checked_add(artifact.identity.byte_len)
                .context("bulk artifact length overflow")?;
            if total > request.budget.output_bytes {
                bail!("bulk artifact egress exceeds attempt admission");
            }
            let mut file = std::fs::File::open(&artifact.path)?;
            if file.metadata()?.len() != artifact.identity.byte_len {
                bail!("artifact changed before egress");
            }
            let mut remaining = artifact.identity.byte_len;
            while remaining > 0 {
                let length = usize::try_from(remaining.min(MAX_ARTIFACT_CHUNK_BYTES as u64))?;
                file.read_exact(&mut buffer[..length])?;
                write_artifact_chunk(&mut stdout, &buffer[..length])?;
                remaining -= length as u64;
            }
        }
    }
    Ok(())
}

/// Execute only after engine confinement has been established. This function
/// does not claim that its caller is sandboxed; public client requests cannot
/// call it or choose this manifest's resource paths.
pub fn execute_manifest(request: &WorkerRequest) -> WorkerResult {
    let result = execute(request).map_err(|error| {
        error
            .downcast_ref::<ProtocolError>()
            .cloned()
            .unwrap_or_else(|| {
                ProtocolError::new(
                    ErrorCode::Unavailable,
                    format!("worker attempt failed: {error:#}"),
                )
            })
    });
    WorkerResult {
        version: PROTOCOL_VERSION,
        attempt_id: request.attempt_id.clone(),
        job_id: request.job_id.clone(),
        result,
    }
}

fn execute(request: &WorkerRequest) -> Result<WorkerOutput> {
    request.validate()?;
    if request.budget.wall_time_ms > 24 * 60 * 60 * 1000 {
        bail!("worker deadline must be at most one day");
    }
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(request.budget.wall_time_ms))
        .context("worker deadline overflow")?;
    let directory = request
        .output_dir
        .canonicalize()
        .context("approved attempt directory is absent")?;
    if directory != request.output_dir || !directory.is_dir() {
        bail!("attempt directory must be a canonical existing directory");
    }
    validate_dependencies(request, deadline)?;
    let result = if let WorkerOperation::GenerateInput { input } = &request.operation {
        inputs::generate(request, input, deadline)?
    } else {
        let index = media::probe(&request.tools.ffprobe.path, &request.source.path, deadline)?;
        match &request.operation {
            WorkerOperation::Generate {
                preset,
                settings,
                model,
            } => generate(request, &index, preset, settings, model.as_ref(), deadline)?,
            WorkerOperation::Preview {
                context,
                source_time,
                max_width,
                max_height,
                model,
                model_input,
            } => preview(
                request,
                &index,
                context,
                *source_time,
                *max_width,
                *max_height,
                model.as_ref(),
                model_input.as_ref(),
                deadline,
            )?,
            WorkerOperation::GenerateInput { .. } => unreachable!("handled before media probing"),
        }
    };
    // Read-only mounts are not immutable copies of mutable backing files.
    // This detects changed inputs and invalidates the result; it does not undo
    // native effects. The launcher therefore also confines effects and snapshots
    // mutable custom model/runtime bytes before calling this worker.
    validate_dependencies(request, deadline)?;
    Ok(result)
}

fn validate_dependencies(request: &WorkerRequest, deadline: Instant) -> Result<()> {
    artifacts::verify_artifact(&request.source.path, &request.source.identity, deadline)?;
    for tool in [&request.tools.ffmpeg, &request.tools.ffprobe] {
        artifacts::verify_artifact(&tool.path, &tool.identity, deadline)?;
    }
    if let Some(runtime) = &request.tools.onnx_runtime {
        artifacts::verify_artifact(&runtime.path, &runtime.identity, deadline)?;
    }
    let model = match &request.operation {
        WorkerOperation::Generate { model, .. } | WorkerOperation::Preview { model, .. } => {
            model.as_ref()
        }
        WorkerOperation::GenerateInput { .. } => None,
    };
    if let Some(model) = model {
        artifacts::verify_artifact(&model.path, &model.identity, deadline)?;
    }
    Ok(())
}

fn create_detector(
    request: &WorkerRequest,
    model: Option<&SourceArtifact>,
    contract: Option<&ModelInputContract>,
) -> Result<Option<detector::Detector>> {
    match (model, contract) {
        (None, None) => Ok(None),
        (Some(model), Some(contract)) => {
            let runtime = request.tools.onnx_runtime.as_ref().ok_or_else(||
                ProtocolError::new(ErrorCode::Unavailable, "pinned local ONNX runtime unavailable; no download or implicit backend fallback"))?;
            detector::Detector::new(
                &model.path,
                &runtime.path,
                contract,
                request.budget.cpu_threads,
            )
            .map(Some)
        }
        _ => Err(ProtocolError::invalid(
            "model and declared decoder contract must be supplied together",
        )
        .into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn preview(
    request: &WorkerRequest,
    index: &media::MediaIndex,
    context: &FrameContext,
    requested_time: SourceTimestamp,
    max_width: u32,
    max_height: u32,
    model: Option<&SourceArtifact>,
    contract: Option<&ModelInputContract>,
    deadline: Instant,
) -> Result<WorkerOutput> {
    if context.source_version != request.source.source_version {
        return Err(ProtocolError::invalid(
            "preview context does not name the input source version",
        )
        .into());
    }
    if context.transform.as_str() != "source-identity" {
        return Err(ProtocolError::unsupported(
            "preview projection without a declared transform adapter",
        )
        .into());
    }
    let requested = i128::from(requested_time.numerator()) * i128::from(index.time_base_den);
    let frame_index = index
        .pts
        .partition_point(|pts| {
            i128::from(*pts)
                * i128::from(index.time_base_num)
                * i128::from(requested_time.denominator())
                <= requested
        })
        .saturating_sub(1);
    let (width, height) = media::fit_dimensions(index.width, index.height, max_width, max_height)?;
    let frame_bytes = media::frame_bytes(width, height)? as u64;
    if frame_bytes > request.budget.memory_bytes / 8 || frame_bytes > request.budget.output_bytes {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "preview buffer exceeds memory admission",
        )
        .into());
    }
    let rgb = media::extract(
        &request.tools.ffmpeg.path,
        &request.source.path,
        frame_index,
        width,
        height,
        request.budget.cpu_threads,
        deadline,
    )?;
    let actual_context = FrameContext {
        frame: FrameId::new(frame_index as u64),
        ..context.clone()
    };
    let mut detector = create_detector(request, model, contract)?;
    let (observations, analysis) = if let Some(detector) = detector.as_mut() {
        let raw = detector.detect(&rgb, width, height)?;
        let observations = raw
            .into_iter()
            .map(|raw| {
                DetectionObservation::new(
                    actual_context.clone(),
                    CoordinateBox::new(
                        CoordinateSpace::ProjectionNormalized,
                        raw.bbox[0] as f64,
                        raw.bbox[1] as f64,
                        raw.bbox[2] as f64,
                        raw.bbox[3] as f64,
                    )?,
                    None,
                    EvidenceKind::Observed,
                    Confidence::new(raw.score as f64, None)?,
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // Empty real inference is absence, not pending work or a fabricated ROI.
        (observations, AnalysisStatus::Available)
    } else {
        (
            Vec::new(),
            AnalysisStatus::Unavailable {
                reason:
                    "No model selected. Preview pixels are decoded; no detector observations exist."
                        .into(),
            },
        )
    };
    let artifact = artifacts::write_bytes(
        &request.output_dir,
        "preview.rgb",
        &rgb,
        request.budget.output_bytes,
    )?;
    let frame = FrameArtifact {
        artifact,
        width,
        height,
        stride_bytes: u64::from(width) * 3,
        format: PixelFormat::Rgb8,
    };
    frame.validate(request.budget.output_bytes)?;
    Ok(WorkerOutput::Preview(PreviewResult {
        requested_source_time: match &request.operation {
            WorkerOperation::Preview { source_time, .. } => *source_time,
            _ => unreachable!("preview operation"),
        },
        context: actual_context,
        observations,
        frame: Some(frame),
        analysis,
        source_time: exact_source_time(index, frame_index)?,
        source_frame_index: frame_index as u64,
    }))
}

#[derive(Serialize)]
struct SampleLineage {
    source_frame_index: u64,
    source_pts: i64,
    project_time_ns: i64,
    quantization_remainder: u32,
    quantization_denominator: u32,
    evidence: EvidenceKind,
    observed_boxes: usize,
    selected_class: Option<u32>,
    selected_track: Option<EntityTrackId>,
}

fn generate(
    request: &WorkerRequest,
    index: &media::MediaIndex,
    preset: &str,
    settings: &GenerationSettings,
    model: Option<&SourceArtifact>,
    deadline: Instant,
) -> Result<WorkerOutput> {
    if !matches!(preset, "default" | "fast" | "economy") {
        return Err(ProtocolError::unsupported(format!("generation preset {preset}")).into());
    }
    if settings.vr_mode || settings.axis != Axis::Stroke {
        return Err(
            ProtocolError::unsupported("generation projection or non-stroke recipe").into(),
        );
    }
    let (width, height) = media::fit_dimensions(
        index.width,
        index.height,
        settings.analysis_width,
        settings.analysis_height,
    )?;
    let bytes = media::frame_bytes(width, height)? as u64;
    let selected_count = index
        .pts
        .len()
        .div_ceil(settings.source_sample_stride as usize);
    if selected_count > MAX_SAMPLED_FRAMES
        || bytes > request.budget.memory_bytes / 16
        || selected_count as u64 > request.budget.memory_bytes / 1024
    {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "generation buffers exceed immutable attempt admission",
        )
        .into());
    }
    let mut decoder = media::Decoder::new(
        &request.tools.ffmpeg.path,
        &request.source.path,
        width,
        height,
        settings.source_sample_stride,
        request.budget.cpu_threads,
        deadline,
    )?;
    let mut detector = create_detector(request, model, settings.model_input.as_ref())?;
    let mut previous_gray: Option<Vec<u8>> = None;
    let mut tracker = tracking::StrokeTracker::new(&request.attempt_id)?;
    let mut review_spans = std::collections::BTreeMap::new();
    let mut position = NormalizedPosition::new(0.5)?;
    let mut actions = Vec::new();
    let mut gaps = Vec::new();
    let mut gap_start: Option<ProjectTime> = None;
    let mut samples = Vec::with_capacity(selected_count);
    let mut last_time = ProjectTime::ZERO;
    for frame_index in (0..index.pts.len()).step_by(settings.source_sample_stride as usize) {
        if Instant::now() >= deadline {
            bail!("generation deadline exceeded");
        }
        let rgb = decoder
            .next()?
            .context("decoder ended before the indexed source frame")?;
        let gray = grayscale(&rgb);
        let quantized = project_time(index, frame_index)?.quantize_nanoseconds()?;
        let time = quantized.time;
        let is_cut = previous_gray
            .as_ref()
            .is_some_and(|previous| photometric_cut(previous, &gray));
        let mut observed_boxes = 0;
        let mut selected_class = None;
        let mut selected_track = None;
        let next_frame = frame_index.saturating_add(settings.source_sample_stride as usize);
        let end = if next_frame < index.pts.len() {
            project_time(index, next_frame)?
                .quantize_nanoseconds()?
                .time
        } else {
            time.checked_add(1)?
        };
        let mut reasons = Vec::new();
        let mut evidence = EvidenceKind::Unavailable;
        if let Some(detector) = detector.as_mut() {
            let detections = detector.detect(&rgb, width, height)?;
            let frame = tracker.update(
                FrameContext {
                    source_version: request.source.source_version.clone(),
                    source_placement: SourcePlacementId::new("worker-primary-source")?,
                    frame: FrameId::new(frame_index as u64),
                    transform: TransformId::new("source-identity")?,
                    seek_generation: 0,
                    request_generation: frame_index as u64,
                },
                &gray,
                width,
                height,
                &detections,
                is_cut,
            )?;
            observed_boxes = frame.observed_boxes.len();
            selected_class = frame.selected_class;
            selected_track = frame.selected_track;
            reasons = frame.reviews;
            if let Some(value) = frame.position {
                position = value;
                evidence = frame.evidence;
            }
        } else if let Some(previous) = previous_gray.as_ref() {
            if !is_cut {
                let estimate = estimate_vertical_translation(previous, &gray, width, height, 8)?;
                if estimate.has_evidence {
                    position =
                        integrate_vertical_motion(position, estimate.displacement_pixels, height)?;
                    evidence = EvidenceKind::Inferred;
                }
            }
        } else {
            // An explicit neutral starting anchor, not an observed stroke.
            evidence = EvidenceKind::Synthesized;
        }
        if model.is_none() {
            reasons.push("global_motion_without_target_identity".to_owned());
        }
        if is_cut {
            reasons.push("scene_cut".to_owned());
        }
        if evidence == EvidenceKind::Unavailable {
            reasons.push("missing_motion_evidence".to_owned());
        }
        for reason in reasons {
            record_review(&mut review_spans, reason, time, end, evidence)?;
        }
        if evidence == EvidenceKind::Unavailable {
            gap_start.get_or_insert(time);
        } else {
            if let Some(start) = gap_start.take() {
                if start < time {
                    gaps.push(TimeRange::new(start, time)?);
                }
            }
            actions.push(MotionAction::new(time, position, evidence)?);
        }
        samples.push(SampleLineage {
            source_frame_index: frame_index as u64,
            source_pts: index.pts[frame_index],
            project_time_ns: time.as_nanos(),
            quantization_remainder: quantized.remainder_numerator,
            quantization_denominator: quantized.denominator,
            evidence,
            observed_boxes,
            selected_class,
            selected_track,
        });
        last_time = time;
        previous_gray = Some(gray);
    }
    if decoder.next()?.is_some() {
        bail!("decoder produced frames absent from its source index");
    }
    if let Some(start) = gap_start {
        gaps.push(TimeRange::new(start, last_time.checked_add(1)?)?);
    }
    let program = MotionProgram::new(vec![MotionTrack::with_gaps(Axis::Stroke, actions, gaps)?])?;
    let review = finish_reviews(review_spans, Some(Axis::Stroke))?;
    let program_artifact = artifacts::write_json(
        &request.output_dir,
        "candidate.json",
        &program,
        request.budget.output_bytes,
    )?;
    let mut lineage = vec![
        request.source.identity.clone(),
        request.tools.ffmpeg.identity.clone(),
        request.tools.ffprobe.identity.clone(),
    ];
    if let Some(model) = model {
        lineage.push(model.identity.clone());
    }
    if let Some(runtime) = &request.tools.onnx_runtime {
        lineage.push(runtime.identity.clone());
    }
    lineage.extend(
        request
            .dependencies
            .iter()
            .filter(|item| !lineage.contains(item))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let configuration = serde_json::to_vec(&(preset, settings))?;
    let config_sha256 = format!("{:x}", Sha256::digest(&configuration));
    let receipt = serde_json::json!({
        "schema_version": 1, "recipe": if model.is_some() { "tracked-relative-image-flow-stroke-v2" } else { "vertical-translation-stroke-v1" },
        "qualification": "unqualified", "project_id": request.project_id, "base_revision": request.base_revision,
        "job_id": request.job_id, "attempt_id": request.attempt_id, "source_version": request.source.source_version,
        "program": program_artifact.identity, "configuration_sha256": config_sha256, "configuration": settings,
        "preset": preset, "dependencies": lineage, "coordinate_projection": "source-identity",
        "source_dimensions": [index.width, index.height], "analysis_dimensions": [width, height],
        "source_time_base": {"numerator": index.time_base_num, "denominator": index.time_base_den},
        "source_origin_pts": index.pts[0], "project_time_mapping": "source PTS minus first PTS, floor nanoseconds; exact remainder retained",
        "confidence_calibration": "uncalibrated", "target_selection": if model.is_some() {
            "stable observed entity identity; target/background independently checked image flow; neutral synthesized anchors; missing support remains gaps"
        } else { "global source-pixel vertical translation; not anatomical target identification" },
        "samples": samples, "review": review, "review_aggregation": "same-reason covering spans; may include intervening supported frames; uncalibrated", "unresolved_gaps": program.track(Axis::Stroke).map(|track| track.gaps()),
        "claim": "Unqualified best-effort generation; not a stroke-quality, speed, model, or hardware qualification"
    });
    let remaining = request
        .budget
        .output_bytes
        .checked_sub(program_artifact.identity.byte_len)
        .context("attempt artifact budget exceeded")?;
    let receipt = artifacts::write_json(&request.output_dir, "lineage.json", &receipt, remaining)?;
    Ok(WorkerOutput::Generated {
        program: program_artifact,
        receipt,
        lineage,
        review,
    })
}

type ReviewSpans = std::collections::BTreeMap<String, (ProjectTime, ProjectTime, EvidenceKind)>;

fn record_review(
    spans: &mut ReviewSpans,
    code: String,
    start: ProjectTime,
    end: ProjectTime,
    evidence: EvidenceKind,
) -> Result<()> {
    ReviewReason::new(code.clone())?;
    if end <= start {
        bail!("review interval lost timestamp precision");
    }
    spans
        .entry(code)
        .and_modify(|value| {
            value.0 = value.0.min(start);
            value.1 = value.1.max(end);
            if value.2 != evidence {
                value.2 = EvidenceKind::Inferred;
            }
        })
        .or_insert((start, end, evidence));
    if spans.len() > MAX_REVIEW_FLAGS {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "review reason budget exhausted",
        )
        .into());
    }
    Ok(())
}

fn finish_reviews(spans: ReviewSpans, axis: Option<Axis>) -> Result<Vec<ReviewFlag>> {
    let review = spans
        .into_iter()
        .map(|(code, (start, end, evidence))| {
            Ok(ReviewFlag {
                axis,
                range: TimeRange::new(start, end)?,
                reason: ReviewReason::new(code)?,
                evidence,
                confidence: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    validate_reviews(&review)?;
    Ok(review)
}

fn exact_source_time(index: &media::MediaIndex, frame: usize) -> Result<SourceTimestamp> {
    SourceTimestamp::new(
        index.pts[frame]
            .checked_mul(i64::from(index.time_base_num))
            .context("source rational timestamp overflow")?,
        index.time_base_den,
    )
    .map_err(Into::into)
}

fn project_time(index: &media::MediaIndex, frame: usize) -> Result<SourceTimestamp> {
    let delta = index.pts[frame]
        .checked_sub(index.pts[0])
        .context("source PTS difference overflow")?;
    SourceTimestamp::new(
        delta
            .checked_mul(i64::from(index.time_base_num))
            .context("source/project timestamp mapping overflow")?,
        index.time_base_den,
    )
    .map_err(Into::into)
}

fn grayscale(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks_exact(3)
        .map(|pixel| {
            ((u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29) >> 8)
                as u8
        })
        .collect()
}

fn photometric_cut(previous: &[u8], current: &[u8]) -> bool {
    previous.len() != current.len()
        || previous.is_empty()
        || previous
            .iter()
            .zip(current)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>() as f64
            / previous.len() as f64
            > 65.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_source_pts_and_project_origin_stay_distinct() {
        let index = media::MediaIndex {
            width: 32,
            height: 32,
            time_base_num: 1,
            time_base_den: 90_000,
            pts: vec![9_000, 12_003, 15_006],
        };
        assert_eq!(exact_source_time(&index, 1).unwrap().numerator(), 12_003);
        let project = project_time(&index, 1)
            .unwrap()
            .quantize_nanoseconds()
            .unwrap();
        assert_eq!(project.time.as_nanos(), 33_366_666);
        assert_ne!(project.remainder_numerator, 0);
    }

    #[test]
    fn grayscale_and_cut_are_real_pixel_functions() {
        assert_eq!(grayscale(&[255, 255, 255, 0, 0, 0]), [255, 0]);
        assert!(!photometric_cut(&[128; 9], &[128; 9]));
        assert!(photometric_cut(&[0; 9], &[255; 9]));
    }
}
