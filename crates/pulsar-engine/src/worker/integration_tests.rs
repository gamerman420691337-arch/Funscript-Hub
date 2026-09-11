//! Real decoder/process fixtures. These tests qualify interface behavior only,
//! not a detector's real-media quality or a production sandbox configuration.

use super::*;
use std::path::Path;
use std::process::Command;

fn fixture() -> (tempfile::TempDir, WorkerTools, SourceArtifact) {
    let directory = tempfile::tempdir().unwrap();
    let tools =
        discover_tools().expect("architecture media tests require declared ffmpeg/ffprobe tools");
    let raw = directory.path().join("pixels.rgb");
    let mut bytes = Vec::new();
    for frame in 0..8usize {
        for y in 0..64usize {
            for x in 0..64usize {
                let shifted_y = (y + 64 - frame) % 64;
                let value =
                    ((x * 31 + shifted_y * 17 + (x / 7) * (shifted_y / 5) * 13) % 256) as u8;
                bytes.extend_from_slice(&[value, value, value]);
            }
        }
    }
    std::fs::write(&raw, bytes).unwrap();
    let video = directory.path().join("moving.mkv");
    let mut command = Command::new(&tools.ffmpeg.path);
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            "64x64",
            "-framerate",
            "8",
            "-i",
        ])
        .arg(&raw)
        .args(["-frames:v", "8", "-threads", "1", "-c:v", "ffv1"])
        .arg(&video);
    media::bounded_output(command, 1024, Instant::now() + Duration::from_secs(20)).unwrap();
    let pinned = pin_artifact(&video).unwrap();
    let source = SourceArtifact {
        source_version: SourceVersionId::new("fixture-source").unwrap(),
        path: pinned.path,
        identity: pinned.identity,
    };
    (directory, tools, source)
}

fn manifest(
    directory: &Path,
    tools: WorkerTools,
    source: SourceArtifact,
    operation: WorkerOperation,
) -> WorkerRequest {
    let output_dir = directory.join("attempt");
    std::fs::create_dir(&output_dir).unwrap();
    WorkerRequest {
        version: PROTOCOL_VERSION,
        attempt_id: AttemptId::new("fixture-attempt").unwrap(),
        job_id: JobId::new("fixture-job").unwrap(),
        project_id: ProjectId::new("fixture-project").unwrap(),
        base_revision: RevisionId::new(0),
        source,
        output_dir: output_dir.canonicalize().unwrap(),
        operation,
        budget: ResourceBudget {
            memory_bytes: 512 * 1024 * 1024,
            output_bytes: 16 * 1024 * 1024,
            wall_time_ms: 30_000,
            cpu_threads: 1,
        },
        dependencies: Vec::new(),
        tools,
    }
}

fn context(source: &SourceArtifact) -> FrameContext {
    FrameContext {
        source_version: source.source_version.clone(),
        source_placement: SourcePlacementId::new("primary-fixture-source").unwrap(),
        frame: FrameId::new(999),
        transform: TransformId::new("source-identity").unwrap(),
        seek_generation: 7,
        request_generation: 12,
    }
}

#[test]
fn actual_decode_preview_binds_frame_pts_and_reports_missing_model() {
    let (directory, tools, source) = fixture();
    let requested = context(&source);
    let manifest = manifest(
        directory.path(),
        tools,
        source,
        WorkerOperation::Preview {
            context: requested.clone(),
            source_time: SourceTimestamp::new(3, 8).unwrap(),
            max_width: 48,
            max_height: 48,
            model: None,
            model_input: None,
        },
    );
    let output = execute_manifest(&manifest).result.unwrap();
    let WorkerOutput::Preview(result) = output else {
        panic!("expected preview");
    };
    assert!(result.matches_seek_request(&requested, &SourceTimestamp::new(3, 8).unwrap()));
    assert!(!result.matches_seek_request(&requested, &SourceTimestamp::new(4, 8).unwrap()));
    assert_eq!(result.context.frame, FrameId::new(3));
    assert_eq!(result.source_frame_index, 3);
    assert_eq!(
        result.source_time.numerator() * 8,
        i64::from(result.source_time.denominator()) * 3
    );
    assert!(result.observations.is_empty());
    assert!(matches!(
        result.analysis,
        AnalysisStatus::Unavailable { .. }
    ));
    let frame = result.frame.unwrap();
    frame.validate(manifest.budget.output_bytes).unwrap();
    let pixels = std::fs::read(&frame.artifact.path).unwrap();
    assert_eq!(pixels.len(), 48 * 48 * 3);
    assert!(pixels.windows(2).any(|window| window[0] != window[1]));
    artifacts::verify_artifact(
        &frame.artifact.path,
        &frame.artifact.identity,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
}

#[test]
fn actual_pixel_generation_returns_checked_candidate_and_lineage() {
    let (directory, tools, source) = fixture();
    let manifest = manifest(
        directory.path(),
        tools,
        source,
        WorkerOperation::Generate {
            preset: "fast".into(),
            settings: GenerationSettings {
                analysis_width: 64,
                analysis_height: 64,
                ..Default::default()
            },
            model: None,
        },
    );
    let output = execute_manifest(&manifest).result.unwrap();
    let WorkerOutput::Generated {
        program,
        receipt,
        lineage,
        ..
    } = output
    else {
        panic!("expected candidate");
    };
    let program: MotionProgram =
        serde_json::from_slice(&std::fs::read(&program.path).unwrap()).unwrap();
    let track = program.track(Axis::Stroke).unwrap();
    assert!(
        track.actions().len() >= 2,
        "moving textured source must yield actual motion evidence"
    );
    assert!(track
        .actions()
        .iter()
        .any(|action| action.evidence() == EvidenceKind::Inferred));
    assert!(track
        .actions()
        .iter()
        .all(|action| action.evidence() != EvidenceKind::Observed));
    assert!(track
        .actions()
        .iter()
        .all(|action| action.position().value() > 0.0 && action.position().value() < 1.0));
    assert!(lineage.contains(&manifest.source.identity));
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt.path).unwrap()).unwrap();
    assert_eq!(receipt["samples"].as_array().unwrap().len(), 8);
    assert_eq!(receipt["qualification"], "unqualified");
    assert_eq!(receipt["source_origin_pts"], 0);
}

#[test]
fn source_byte_replacement_rejected_before_native_execution() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    std::fs::write(&path, b"original").unwrap();
    let reference = pin_artifact(&path).unwrap();
    let source = SourceArtifact {
        source_version: SourceVersionId::new("source").unwrap(),
        path: reference.path.clone(),
        identity: reference.identity.clone(),
    };
    let tools = WorkerTools {
        ffmpeg: reference.clone(),
        ffprobe: reference,
        onnx_runtime: None,
    };
    let manifest = manifest(
        directory.path(),
        tools,
        source,
        WorkerOperation::Generate {
            preset: "default".into(),
            settings: Default::default(),
            model: None,
        },
    );
    std::fs::write(&path, b"replaced").unwrap();
    assert_eq!(
        execute_manifest(&manifest).result.unwrap_err().code,
        ErrorCode::DependencyMismatch
    );
}

#[test]
fn artifact_budget_failure_is_typed_resource_exhaustion() {
    let (directory, tools, source) = fixture();
    let request_context = context(&source);
    let mut manifest = manifest(
        directory.path(),
        tools,
        source,
        WorkerOperation::Preview {
            context: request_context,
            source_time: SourceTimestamp::new(0, 1).unwrap(),
            max_width: 48,
            max_height: 48,
            model: None,
            model_input: None,
        },
    );
    manifest.budget.output_bytes = 8;
    assert_eq!(
        execute_manifest(&manifest).result.unwrap_err().code,
        ErrorCode::ResourceExhausted
    );
}
