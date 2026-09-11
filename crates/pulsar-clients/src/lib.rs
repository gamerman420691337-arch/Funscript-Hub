#![forbid(unsafe_code)]
//! First-party presentation clients. All authoritative work crosses the same
//! versioned protocol; this crate cannot import the engine implementation.

pub mod cli;
mod desktop;
mod modality;
mod editor;
mod preview;
pub mod motion;
pub use motion::{ResponseBody, ProjectSnapshot, CandidateSnapshot};
pub mod session;
pub mod project_package;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser};
use cli::{Cli, Commands, ProjectCommands};
use pulsar_protocol::*;
pub use session::{EngineApi, SessionClient};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

pub use desktop::PulsarDesktop;
pub use preview::PreviewState;

/// Used by the executable composition root before daemon discovery. Help,
/// version, and completion generation never need a running engine.
pub fn command_needs_engine(args: &[OsString]) -> bool {
    match Cli::try_parse_from(args) {
        Ok(cli) => !matches!(cli.command, Some(Commands::Completions { .. })),
        Err(_) => false,
    }
}

pub fn run_gui(endpoint: PathBuf, bootstrap: PathBuf) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 540.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Pulsar Desktop",
        options,
        Box::new(move |context| Ok(Box::new(PulsarDesktop::new(context, endpoint, bootstrap)))),
    )
    .map_err(|error| anyhow::anyhow!("Desktop startup failed: {error}"))
}

pub fn run_cli(args: Vec<OsString>, endpoint: &Path, bootstrap: &Path) -> Result<()> {
    let cli = Cli::parse_from(args);
    let Some(command) = cli.command else {
        return run_gui(endpoint.to_owned(), bootstrap.to_owned());
    };
    match command {
        Commands::Gui => run_gui(endpoint.to_owned(), bootstrap.to_owned()),
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "pulsar", &mut std::io::stdout());
            Ok(())
        }
        Commands::Synthesize { prompt, image, output } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let input = match image {
                Some(path) => modality::Input::Image { path, prompt },
                None => modality::Input::Text { prompt },
            };
            modality::synthesize(&mut client, input, &output.unwrap_or_else(|| PathBuf::from("pulsar-synthetic.funscript")))
        }
        Commands::AudioMap { video, output, mode, axis, amplitude, offset } => {
            let mode = match mode.as_str() {
                "envelope" => AudioMode::Envelope, "beats" => AudioMode::Beats,
                _ => return unsupported("audio mapping mode; choose envelope or beats"),
            };
            let mapping = AudioMapping::new(modality::parse_axis(&axis)?, amplitude, NormalizedPosition::new(offset)?)?;
            let output = output.unwrap_or_else(|| video.with_extension("funscript"));
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            modality::synthesize(&mut client, modality::Input::Audio { path: video, mapping, mode }, &output)
        }
        Commands::ImportScript { script, project } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            modality::import_script(&mut client, &script, project.as_deref())
        }
        Commands::Review { project, candidate } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            modality::review(&mut client, &project, candidate.as_deref())
        }
        Commands::MergeCandidate { project, candidate, axes, start, end, output } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            modality::merge(&mut client, &project, &candidate, &axes, start, end, output.as_deref())
        }
        Commands::ExportAxis { project, path, axis } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let project = get_snapshot(&mut client, &ProjectId::new(project)?)?;
            print_response(client.execute(Command::ExportAxis { path: absolute_path(&path)?, axis: modality::parse_axis(&axis)? },
                Some(project.project_id), Some(project.revision))?)
        }
        Commands::AllowPackaging { project, acknowledge_unencrypted_private_data } => {
            anyhow::ensure!(acknowledge_unencrypted_private_data, project_package::PACKAGE_PRIVACY_WARNING);
            eprintln!("{}", project_package::PACKAGE_PRIVACY_WARNING);
            let consent = project_package::PackagePrivacyConsent::acknowledge_unencrypted_private_data();
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            project_package::allow_packaging_local(&mut client, ProjectId::new(project)?, bootstrap, &consent)?;
            println!("Packaging allowed for this authenticated owner session only.");
            Ok(())
        }
        Commands::PackageExport { project, destination, acknowledge_unencrypted_private_data, revision, request_id } => {
            anyhow::ensure!(acknowledge_unencrypted_private_data, project_package::PACKAGE_PRIVACY_WARNING);
            eprintln!("{}", project_package::PACKAGE_PRIVACY_WARNING);
            let consent = project_package::PackagePrivacyConsent::acknowledge_unencrypted_private_data();
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let project = ProjectId::new(project)?;
            let revision = match revision { Some(value) => RevisionId::new(value), None => client.project_package_revision(project.clone())? };
            let request_id = match request_id { Some(id) => RequestId::new(id)?, None => session::new_request_id() };
            eprintln!("Package Start request: {}", request_id);
            std::io::Write::flush(&mut std::io::stderr())?;
            let mut status = client.start_project_export_with_request_id(project.clone(), revision, &consent, request_id)?;
            eprintln!("Package export operation: {}", status.operation_id);
            eprintln!("Disconnect does not cancel. Use package-status, package-cancel, or package-download with this operation ID.");
            std::io::Write::flush(&mut std::io::stderr())?;
            let operation = status.operation_id.clone();
            let start_request = status.request_id.clone();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30 * 60);
            while !status.state.is_terminal() {
                anyhow::ensure!(std::time::Instant::now() < deadline,
                    "Package wait expired; operation {} is retained and was not cancelled", operation);
                let remaining = deadline.checked_duration_since(std::time::Instant::now()).context("Package wait deadline expired")?;
                std::thread::sleep(remaining.min(std::time::Duration::from_millis(100)));
                status = client.project_package_status_until(project.clone(), operation.clone(), deadline)?;
                anyhow::ensure!(status.request_id == start_request && status.requested_revision == revision,
                    "Package status changed its original request/revision identity");
            }
            anyhow::ensure!(status.state == PackageExportOperationState::Ready,
                "Package operation {} ended {:?}: {:?}", operation, status.state, status.error);
            let begin_id = session::new_request_id();
            eprintln!("Package download request: {}", begin_id);
            std::io::Write::flush(&mut std::io::stderr())?;
            let receipt = client.download_project_package_with_request_id(project, operation, &destination, &consent,
                &project_package::PackageDownloadControl::default(), begin_id)?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            Ok(())
        }
        Commands::PackageStatus { project, operation } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let status = client.project_package_status(ProjectId::new(project)?, PackageOperationId::new(operation)?)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Commands::PackageCancel { project, operation } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let status = client.cancel_project_export(ProjectId::new(project)?, PackageOperationId::new(operation)?)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Commands::PackageDownload { project, operation, destination, acknowledge_unencrypted_private_data, request_id } => {
            anyhow::ensure!(acknowledge_unencrypted_private_data, project_package::PACKAGE_PRIVACY_WARNING);
            eprintln!("{}", project_package::PACKAGE_PRIVACY_WARNING);
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let consent = project_package::PackagePrivacyConsent::acknowledge_unencrypted_private_data();
            let request_id = match request_id { Some(id) => RequestId::new(id)?, None => session::new_request_id() };
            eprintln!("Package download request: {}", request_id);
            std::io::Write::flush(&mut std::io::stderr())?;
            let receipt = client.download_project_package_with_request_id(ProjectId::new(project)?, PackageOperationId::new(operation)?,
                &destination, &consent, &project_package::PackageDownloadControl::default(), request_id)?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            Ok(())
        }
        Commands::PackageRelease { project, operation, acknowledge_discarding_engine_copy } => {
            anyhow::ensure!(acknowledge_discarding_engine_copy, "Explicit engine-copy disposal acknowledgement is required");
            eprintln!("Deleting the retained engine package copy. This does not verify importability or modify a downloaded destination.");
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let status = client.release_project_export(ProjectId::new(project)?, PackageOperationId::new(operation)?)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Commands::Capabilities => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            print_response(client.execute(Command::Capabilities, None, None)?)
        }
        Commands::Project { command } => {
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let command = match command {
                ProjectCommands::Create { name } => Command::CreateProject { name },
                ProjectCommands::Open { project_id } => Command::OpenProject {
                    project_id: ProjectId::new(project_id)?,
                },
            };
            print_response(client.execute(command, None, None)?)
        }
        Commands::Generate { video, output, model, model_class_count, conf, fps, width, height, detrend_window,
            norm_window, batch_size, vr, pov, no_balance_global, no_keyframe_reduction,
            cut_diff, cut_flow, multi_axis, profile, cuts } => {
            // No legacy option is silently ignored. Additional modes remain
            // in the grammar and migration ledger until an engine adapter owns them.
            if conf != 0.35 || fps != 30.0
                || detrend_window != 2.0 || norm_window != 3.0 || batch_size != 2000 || vr || pov
                || no_balance_global || no_keyframe_reduction || cut_diff != 30.0 || cut_flow != 8.0
                || multi_axis || cuts.is_some() {
                return unsupported("non-default legacy generation settings; use the supported default/fast stroke path until typed worker adapters are available");
            }
            if !matches!(profile.as_str(), "default" | "fast") {
                return unsupported("requested generation profile; this architecture build exposes default and fast only");
            }
            let model_input = if model.is_some() {
                let class_count = model_class_count.context("Custom models require --model-class-count; tensor layouts are never guessed")?;
                Some(ModelInputContract { width: width.unwrap_or(640), height: height.unwrap_or(640),
                    channels: 3, layout: TensorLayout::Nchw, class_count, decoder: DetectorDecoder::YoloChannelMajor })
            } else {
                if model_class_count.is_some() { bail!("--model-class-count requires --model"); }
                None
            };
            let settings = GenerationSettings { analysis_width: width.unwrap_or(320), analysis_height: height.unwrap_or(180),
                model_input, ..Default::default() };
            settings.validate()?;
            let model_path = model.map(|path| absolute_path(&path)).transpose()?;
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            let output = output.unwrap_or_else(|| video.with_extension("funscript"));
            generate_to_file(&mut client, &video, &output, &profile, settings, model_path)
        }
        Commands::Fix { .. } => unsupported("legacy script repair (B correctness adapter)"),
        Commands::Doctor { .. } => unsupported("legacy script diagnostics (B correctness adapter)"),
        Commands::Info { .. } => unsupported("standalone media probe (worker query adapter)"),
        Commands::Scan { folder, recursive, multi_axis, missing_only, json, csv, generate, model, overwrite } => {
            if multi_axis || model.is_some() || overwrite { return unsupported("multi-axis/model/overwrite library mode; those require additional engine contracts"); }
            if json && csv { bail!("Choose --json or --csv, not both"); }
            let entries = library_entries(&folder, recursive)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries.iter().filter(|entry| !missing_only || !entry.has_script).collect::<Vec<_>>())?);
            } else if csv {
                println!("video,script,has_script");
                for entry in entries.iter().filter(|entry| !missing_only || !entry.has_script) {
                    println!("\"{}\",\"{}\",{}", entry.video.display().to_string().replace('"', "\"\""),
                        entry.script.display().to_string().replace('"', "\"\""), entry.has_script);
                }
            } else {
                for entry in entries.iter().filter(|entry| !missing_only || !entry.has_script) {
                    println!("{} {}", if entry.has_script { "PRESENT" } else { "MISSING" }, entry.video.display());
                }
                println!("{} media files; {} missing stroke companions", entries.len(), entries.iter().filter(|entry| !entry.has_script).count());
            }
            if generate {
                let mut client = SessionClient::connect(endpoint, bootstrap)?;
                for entry in entries.into_iter().filter(|entry| !entry.has_script) {
                    generate_to_file(&mut client, &entry.video, &entry.script, "default", GenerationSettings::default(), None)?;
                }
            }
            Ok(())
        }
        Commands::Batch { folder, recursive, model, multi_axis, overwrite, dry_run } => {
            if model.is_some() || multi_axis || overwrite { return unsupported("batch model/multi-axis/overwrite settings; baseline stroke batching uses the shared engine path"); }
            let entries = library_entries(&folder, recursive)?;
            let pending: Vec<_> = entries.into_iter().filter(|entry| !entry.has_script).collect();
            if dry_run { return print_json(&pending); }
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            for entry in pending { generate_to_file(&mut client, &entry.video, &entry.script, "default", GenerationSettings::default(), None)?; }
            Ok(())
        }
        Commands::AudioSynth { video, output, mode, band, axis, threshold, bundle } => {
            if band != "full" || threshold != 0.35 || bundle {
                return unsupported("audio-synth currently requires --band full, threshold 0.35, and no bundle; no band filtering is substituted");
            }
            let mode = match mode.as_str() {
                "envelope" => AudioMode::Envelope, "beats" => AudioMode::Beats,
                _ => return unsupported("audio-synth mode; envelope and beats are supported"),
            };
            let mapping = AudioMapping::new(modality::parse_axis(&axis)?, 0.5, NormalizedPosition::new(0.25)?)?;
            let output = output.unwrap_or_else(|| video.with_extension("funscript"));
            eprintln!("Full-band mapping: offset 0.25, amplitude 0.5. Use audio-map for explicit range selection.");
            let mut client = SessionClient::connect(endpoint, bootstrap)?;
            modality::synthesize(&mut client, modality::Input::Audio { path: video, mapping, mode }, &output)
        }
        Commands::Model { .. } => unsupported("model registry administration (B model adapter); capabilities reports current engine backends"),
        Commands::Bench => unsupported("hardware benchmark campaign (B/C qualification adapter)"),
        Commands::Play { .. } => unsupported("physical playback; no qualified driver, stopping bound, or admission is established by this build"),
        Commands::Stash { .. } => unsupported("Stash integration (brokered external-effects adapter)"),
    }
}

fn unsupported<T>(operation: &str) -> Result<T> {
    Err(ProtocolError::unsupported(operation).into())
}

fn print_response(response: ResponseBody) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

pub fn create_project(api: &mut impl EngineApi, name: String) -> Result<ProjectSnapshot> {
    match api.execute(Command::CreateProject { name }, None, None)? {
        ResponseBody::Project(project) => Ok(project),
        _ => bail!("Engine returned an unexpected create-project response"),
    }
}

pub fn get_snapshot(api: &mut impl EngineApi, project: &ProjectId) -> Result<ProjectSnapshot> {
    match api.execute(Command::GetSnapshot, Some(project.clone()), None)? {
        ResponseBody::Project(snapshot) => Ok(snapshot),
        _ => bail!("Engine returned an unexpected project response"),
    }
}

fn generate_to_file(
    api: &mut impl EngineApi,
    video: &Path,
    output: &Path,
    preset: &str,
    settings: GenerationSettings,
    model_path: Option<PathBuf>,
) -> Result<()> {
    let name = video
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let project = create_project(
        api,
        if name.is_empty() {
            "Generated motion".into()
        } else {
            name
        },
    )?;
    eprintln!(
        "Project: {}. Generation output remains unqualified until benchmark evidence exists.",
        project.project_id
    );
    let source_version = match api.execute(
        Command::ImportSource {
            path: absolute_path(video)?,
        },
        Some(project.project_id.clone()),
        Some(project.revision),
    )? {
        ResponseBody::Source { source_version } => source_version,
        _ => bail!("Engine returned an unexpected source-import response"),
    };
    let project = get_snapshot(api, &project.project_id)?;
    let job = match api.execute(
        Command::Generate {
            source_version,
            preset: preset.to_owned(),
            settings: Some(settings),
            model_path,
        },
        Some(project.project_id.clone()),
        Some(project.revision),
    )? {
        ResponseBody::Job(job) => job,
        _ => bail!("Engine returned an unexpected generation response"),
    };
    let candidate = modality::await_candidate(api, &project, job)?;
    modality::commit_and_export(api, &project, candidate, output, Axis::Stroke)
}

pub(crate) fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    Ok(std::env::current_dir()
        .context("Resolve client path")?
        .join(path))
}

#[derive(serde::Serialize)]
struct LibraryEntry {
    video: PathBuf,
    script: PathBuf,
    has_script: bool,
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Discovery is client input selection, not generation or project persistence.
/// Do not traverse symlinks or an unbounded directory tree.
fn library_entries(folder: &Path, recursive: bool) -> Result<Vec<LibraryEntry>> {
    let root = absolute_path(folder)?;
    let mut directories = vec![(root, 0usize)];
    let mut entries = Vec::new();
    let mut visited = 0usize;
    while let Some((directory, depth)) = directories.pop() {
        if depth > 64 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "library depth exceeds 64",
            )
            .into());
        }
        for entry in std::fs::read_dir(&directory)
            .with_context(|| format!("Read library directory {}", directory.display()))?
        {
            let entry = entry?;
            visited += 1;
            if visited > 100_000 {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "library scan exceeds 100000 entries",
                )
                .into());
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() && recursive {
                directories.push((entry.path(), depth + 1));
                continue;
            }
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if kind.is_file()
                && matches!(
                    extension.as_str(),
                    "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg"
                )
            {
                let script = path.with_extension("funscript");
                entries.push(LibraryEntry {
                    has_script: script.is_file(),
                    video: path,
                    script,
                });
            }
        }
    }
    entries.sort_by(|a, b| a.video.cmp(&b.video));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn informational_commands_do_not_start_engine() {
        assert!(!command_needs_engine(&args(&["pulsar", "--help"])));
        assert!(!command_needs_engine(&args(&["pulsar", "--version"])));
        assert!(!command_needs_engine(&args(&[
            "pulsar",
            "completions",
            "bash"
        ])));
        assert!(command_needs_engine(&args(&["pulsar"])));
        assert!(command_needs_engine(&args(&[
            "pulsar",
            "generate",
            "video.mp4"
        ])));
    }

    #[test]
    fn unsupported_legacy_capability_keeps_typed_failure() {
        let error = unsupported::<()>("legacy test mode").unwrap_err();
        assert_eq!(
            error.downcast_ref::<ProtocolError>().unwrap().code,
            ErrorCode::Unsupported
        );
    }

    #[test]
    fn cli_and_batch_generation_use_engine_commits_and_pin_candidate_revision() {
        struct FakeEngine {
            responses: std::collections::VecDeque<ResponseBody>,
            requests: Vec<(Command, Option<ProjectId>, Option<RevisionId>)>,
        }
        impl EngineApi for FakeEngine {
            fn execute(
                &mut self,
                command: Command,
                project: Option<ProjectId>,
                revision: Option<RevisionId>,
            ) -> Result<ResponseBody> {
                self.requests.push((command, project, revision));
                self.responses
                    .pop_front()
                    .context("unexpected extra request")
            }
        }
        let project_id = ProjectId::new("project").unwrap();
        let snapshot = |revision| ProjectSnapshot {
            project_id: project_id.clone(),
            revision: RevisionId::new(revision),
            name: "fixture".into(),
            motion: crate::motion::fixture_descriptor(&MotionProgram::default(),
                MotionBinding::ProjectRevision { project_id: project_id.clone(), revision: RevisionId::new(revision) }),
            program: MotionProgram::default().into(),
            sources: vec![],
        };
        let candidate_id = CandidateId::new("candidate").unwrap();
        let output = std::env::temp_dir().join("pulsar-test-output.funscript");
        let mut fake = FakeEngine {
            requests: vec![],
            responses: [
                ResponseBody::Project(snapshot(0)),
                ResponseBody::Source {
                    source_version: SourceVersionId::new("source").unwrap(),
                },
                ResponseBody::Project(snapshot(1)),
                ResponseBody::Job(JobSnapshot {
                    job_id: JobId::new("job").unwrap(),
                    attempt_id: AttemptId::new("attempt").unwrap(),
                    project_id: project_id.clone(),
                    base_revision: RevisionId::new(1),
                    state: "completed".into(),
                    candidate_id: Some(candidate_id),
                    error: None,
                }),
                ResponseBody::Candidate(CandidateSnapshot {
                    candidate_id: CandidateId::new("candidate").unwrap(), project_id: project_id.clone(),
                    base_revision: RevisionId::new(1),
                    motion: crate::motion::fixture_descriptor(&MotionProgram::default(), MotionBinding::Candidate {
                        project_id: project_id.clone(), candidate_id: CandidateId::new("candidate").unwrap(),
                        base_revision: RevisionId::new(1) }),
                    program: MotionProgram::default().into(),
                    job_id: Some(JobId::new("job").unwrap()), review: vec![],
                }),
                ResponseBody::Project(snapshot(2)),
                ResponseBody::AxisExported {
                    path: output.clone(),
                    revision: RevisionId::new(2),
                    axis: Axis::Stroke,
                },
            ]
            .into_iter()
            .collect(),
        };
        generate_to_file(
            &mut fake,
            Path::new("fixture.mp4"),
            &output,
            "default",
            GenerationSettings::default(),
            None,
        )
        .unwrap();
        assert_eq!(fake.requests.len(), 7);
        assert!(matches!(fake.requests[1].0, Command::ImportSource { .. }));
        assert!(matches!(fake.requests[3].0, Command::Generate { .. }));
        assert!(matches!(
            fake.requests[5].0,
            Command::CommitCandidate { .. }
        ));
        assert_eq!(fake.requests[5].2.unwrap().value(), 1);
        assert!(matches!(fake.requests[6].0, Command::ExportAxis { axis: Axis::Stroke, .. }));
        assert_eq!(fake.requests[6].2.unwrap().value(), 2);
    }
}
