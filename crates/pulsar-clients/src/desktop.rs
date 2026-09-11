//! Pulsar's non-authoritative desktop presentation. No decoder, inference
//! runtime, database, plugin host, or physical-device driver is linked here.

use crate::{
    absolute_path,
    editor::{self, EditGesture, EditorPoint},
    preview::{self, DecodedFrame, PreviewState},
    session::{EngineApi, SessionClient},
};
use eframe::egui::{self, Color32, RichText, Stroke};
use pulsar_protocol::*;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Studio,
    Cinema,
    Generator,
    Doctor,
    Devices,
    Automation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GeneratorInputMode { Video, Text, Image, Audio, Funscript }
impl GeneratorInputMode {
    fn label(self) -> &'static str { match self {
        Self::Video => "Video motion", Self::Text => "Constrained pattern text",
        Self::Image => "Still image + pattern prompt", Self::Audio => "Full-band audio",
        Self::Funscript => "Funscript candidate import",
    } }
}

fn select_media_source(
    sources: &[pulsar_protocol::SourceSummary],
    preferred: Option<&SourceVersionId>,
) -> Option<SourceVersionId> {
    sources.iter()
        .find(|source| source.kind == pulsar_protocol::SourceKind::Media
            && preferred == Some(&source.source_version))
        .or_else(|| sources.iter().find(|source| source.kind == pulsar_protocol::SourceKind::Media))
        .map(|source| source.source_version.clone())
}

fn diagnostics_match_view(
    report: &DiagnosticsReport,
    project: Option<(&ProjectId, RevisionId)>,
    candidate: Option<&CandidateId>,
) -> bool {
    project.is_some_and(|(id, revision)| id == &report.project_id && revision == report.revision)
        && (report.candidate_id.is_none() || report.candidate_id.as_ref() == candidate)
}

fn accept_diagnostics_report(
    current: &mut Option<DiagnosticsReport>,
    report: DiagnosticsReport,
    project: Option<(&ProjectId, RevisionId)>,
    candidate: Option<&CandidateId>,
) -> bool {
    if !diagnostics_match_view(&report, project, candidate) {
        return false;
    }
    *current = Some(report);
    true
}

fn execute_work(client: &mut impl EngineApi, work: RpcWork) -> anyhow::Result<ResponseBody> {
    match work.command {
        RpcCommand::Wire(command) => client.execute(command, work.project, work.revision),
        RpcCommand::Edit(proposal) => {
            let project = work.project.ok_or_else(|| anyhow::anyhow!("Edit draft lacks project identity"))?;
            let revision = work.revision.ok_or_else(|| anyhow::anyhow!("Edit draft lacks base revision"))?;
            let candidate = client.upload_edit(project.clone(), revision, &proposal.program, &proposal.label)?;
            let candidate_id = candidate.candidate_id;
            client.execute(Command::CommitCandidate { candidate_id: candidate_id.clone() },
                Some(project), Some(revision))
                .map_err(|error| error.context(format!("Edit candidate {candidate_id} exists; commit was not confirmed. Explicit review/rebase is required before a new commit")))
        }
    }
}

enum RpcCommand { Wire(Command), Edit(EditProposal) }

struct RpcWork {
    command: RpcCommand,
    project: Option<ProjectId>,
    revision: Option<RevisionId>,
}
struct RpcReply {
    response: Result<ResponseBody, String>,
    frame: Option<Result<DecodedFrame, String>>,
}

/// This contains view state and disposable edit drafts, never authoritative
/// project mutation or device authority. Each window has an independent clock.
pub struct PulsarDesktop {
    sender: SyncSender<RpcWork>,
    receiver: Receiver<RpcReply>,
    busy: bool,
    tab: Tab,
    project: Option<ProjectSnapshot>,
    capabilities: Option<Capabilities>,
    job: Option<JobSnapshot>,
    candidate: Option<CandidateSnapshot>,
    source: Option<SourceVersionId>,
    preview: PreviewState,
    texture: Option<egui::TextureHandle>,
    preview_pending: bool,
    request_generation: u64,
    seek_generation: u64,
    playhead_seconds: f64,
    preview_playing: bool,
    last_tick: Instant,
    last_preview: Instant,
    last_job_poll: Instant,
    project_name: String,
    open_project_id: String,
    source_path: String,
    export_path: String,
    model_path: String,
    model_classes: u32,
    model_width: u32,
    model_height: u32,
    use_model: bool,
    preset: String,
    status: String,
    new_position: f64,
    gesture: Option<EditGesture>,
    timeline_axis: Axis,
    generation_mode: GeneratorInputMode,
    pattern_prompt: String,
    audio_beats: bool,
    audio_axis: Axis,
    audio_amplitude: f64,
    audio_offset: f64,
    script_import_path: String,
    diagnostics: Option<DiagnosticsReport>,
    merge_axes: Vec<Axis>,
    merge_range_enabled: bool,
    merge_start_seconds: f64,
    merge_end_seconds: f64,
}

impl PulsarDesktop {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        endpoint: PathBuf,
        bootstrap: PathBuf,
    ) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(17, 21, 29);
        visuals.window_fill = Color32::from_rgb(21, 27, 37);
        visuals.selection.bg_fill = Color32::from_rgb(0, 99, 113);
        visuals.selection.stroke.color = Color32::from_rgb(69, 222, 230);
        context.egui_ctx.set_visuals(visuals);
        let (sender, requests) = mpsc::sync_channel::<RpcWork>(8);
        let (replies, receiver) = mpsc::sync_channel(8);
        let repaint = context.egui_ctx.clone();
        std::thread::spawn(move || {
            let mut client: Option<SessionClient> = None;
            while let Ok(work) = requests.recv() {
                let response = (|| -> anyhow::Result<ResponseBody> {
                    if client.is_none() {
                        client = Some(SessionClient::connect(&endpoint, &bootstrap)?);
                    }
                    execute_work(client.as_mut().expect("client established"), work)
                })()
                .map_err(|error| error.to_string());
                let frame = match &response {
                    Ok(ResponseBody::Preview(result)) => result.frame.as_ref().map(|frame| {
                        preview::decode_frame(frame).map_err(|error| error.to_string())
                    }),
                    _ => None,
                };
                if replies.send(RpcReply { response, frame }).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });
        let mut app = Self {
            sender,
            receiver,
            busy: false,
            tab: Tab::Studio,
            project: None,
            capabilities: None,
            job: None,
            candidate: None,
            source: None,
            preview: PreviewState::default(),
            texture: None,
            preview_pending: false,
            request_generation: 0,
            seek_generation: 0,
            playhead_seconds: 0.0,
            preview_playing: false,
            last_tick: Instant::now(),
            last_preview: Instant::now(),
            last_job_poll: Instant::now(),
            project_name: "Untitled motion".into(),
            open_project_id: String::new(),
            source_path: String::new(),
            export_path: String::new(),
            model_path: String::new(),
            model_classes: 1,
            model_width: 640,
            model_height: 640,
            use_model: false,
            preset: "default".into(),
            status: "Connecting to the per-user engine...".into(),
            new_position: 0.5,
            gesture: None,
            timeline_axis: Axis::Stroke,
            generation_mode: GeneratorInputMode::Video,
            pattern_prompt: "pattern=sine axis=stroke duration=5s frequency=1hz amplitude=0.2 offset=0.5".into(),
            audio_beats: false, audio_axis: Axis::Stroke, audio_amplitude: 0.5, audio_offset: 0.25,
            script_import_path: String::new(), diagnostics: None,
            merge_axes: vec![Axis::Stroke], merge_range_enabled: false, merge_start_seconds: 0.0, merge_end_seconds: 5.0,
        };
        app.send(Command::Capabilities, None, None);
        app
    }

    fn send(&mut self, command: Command, project: Option<ProjectId>, revision: Option<RevisionId>) -> bool {
        self.enqueue(RpcCommand::Wire(command), project, revision)
    }

    fn send_edit(&mut self, proposal: EditProposal, project: ProjectId, revision: RevisionId) -> bool {
        self.enqueue(RpcCommand::Edit(proposal), Some(project), Some(revision))
    }

    fn enqueue(
        &mut self,
        command: RpcCommand,
        project: Option<ProjectId>,
        revision: Option<RevisionId>,
    ) -> bool {
        if self.busy {
            self.status = "An engine request is pending; no duplicate was submitted.".into();
            return false;
        }
        match self.sender.try_send(RpcWork {
            command,
            project,
            revision,
        }) {
            Ok(()) => {
                self.busy = true;
                true
            }
            Err(error) => {
                self.status = format!("Client request queue unavailable: {error}");
                false
            }
        }
    }

    fn project_command(&mut self, command: Command, revision_required: bool) -> bool {
        let Some(project) = &self.project else {
            self.status = "Create or open a project first.".into();
            return false;
        };
        self.send(
            command,
            Some(project.project_id.clone()),
            revision_required.then_some(project.revision),
        )
    }

    fn poll(&mut self, context: &egui::Context) {
        while let Ok(reply) = self.receiver.try_recv() {
            self.busy = false;
            match reply.response {
                Err(error) => {
                    self.status = error;
                }
                Ok(ResponseBody::Capabilities(capabilities)) => {
                    self.status =
                        "Engine connected. Qualification status is shown explicitly.".into();
                    self.capabilities = Some(capabilities);
                }
                Ok(ResponseBody::Project(project)) => {
                    let prior_source = self.source.clone();
                    let changed_project = self
                        .project
                        .as_ref()
                        .is_none_or(|old| old.project_id != project.project_id);
                    self.status = format!(
                        "Durable project revision {} acknowledged.",
                        project.revision.value()
                    );
                    self.open_project_id = project.project_id.to_string();
                    if changed_project {
                        self.source = select_media_source(&project.sources, None);
                        self.candidate = None;
                        self.job = None;
                        self.gesture = None;
                        self.seek(0.0);
                    } else if self.source.is_none() {
                        self.source = select_media_source(&project.sources, None);
                    }
                    self.diagnostics = None;
                    self.source = select_media_source(&project.sources, self.source.as_ref());
                    self.project = Some(project);
                    self.project_command(Command::Diagnostics { candidate_id: None }, false);
                    if changed_project || prior_source != self.source {
                        self.seek(0.0);
                    }
                    if self.preview.displayed().is_none() {
                        self.preview_pending = self.has_media_source();
                    }
                    if !self.has_media_source() {
                        self.preview_playing = false;
                        self.preview_pending = false;
                        self.preview.clear();
                        self.texture = None;
                    }
                }
                Ok(ResponseBody::Source { source_version }) => {
                    self.source = Some(source_version);
                    self.seek(0.0);
                    self.status = "Immutable source imported. Refreshing project revision.".into();
                    self.project_command(Command::GetSnapshot, false);
                }
                Ok(ResponseBody::Job(job)) => {
                    self.status = job
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("Job {}: {}", job.job_id, job.state));
                    self.job = Some(job);
                }
                Ok(ResponseBody::Candidate(candidate)) => {
                    self.status =
                        "Candidate is available for review; project remains unchanged.".into();
                    self.diagnostics = None;
                    self.candidate = Some(candidate);
                }
                Ok(ResponseBody::Preview(result)) => {
                    if self.preview.accept(result) {
                        match reply.frame {
                            Some(Ok(frame)) => {
                                self.texture = Some(context.load_texture(
                                    "pulsar-source-frame",
                                    egui::ColorImage::from_rgba_unmultiplied(
                                        [frame.width, frame.height],
                                        &frame.rgba,
                                    ),
                                    egui::TextureOptions::LINEAR,
                                ))
                            }
                            Some(Err(error)) => {
                                self.texture = None;
                                self.preview.clear();
                                self.status = error;
                            }
                            None => self.texture = None,
                        }
                    }
                }
                Ok(ResponseBody::Diagnostics(report)) => {
                    let accepted = accept_diagnostics_report(
                        &mut self.diagnostics,
                        report,
                        self.project.as_ref().map(|project| (&project.project_id, project.revision)),
                        self.candidate.as_ref().map(|candidate| &candidate.candidate_id),
                    );
                    if accepted {
                        let report = self.diagnostics.as_ref().expect("accepted diagnostics are retained");
                        self.status = format!("Read-only diagnostics: {} issue(s) at revision {}.",
                            report.issues.len(), report.revision.value());
                    } else {
                        self.status = "Discarded diagnostics for a stale project, revision, or candidate.".into();
                    }
                }
                Ok(ResponseBody::AxisExported { path, revision, axis }) => {
                    self.status = format!("Exported {:?} revision {}: {}", axis, revision.value(), path.display());
                }
                Ok(ResponseBody::Exported { path, revision }) => {
                    self.status =
                        format!("Exported revision {}: {}", revision.value(), path.display());
                }
                Ok(_) => {
                    self.status = "Engine request acknowledged.".into();
                }
            }
        }
    }

    fn seek(&mut self, seconds: f64) {
        self.playhead_seconds = seconds.clamp(0.0, 86_400.0);
        self.seek_generation = self.seek_generation.saturating_add(1);
        self.request_generation = self.request_generation.saturating_add(1);
        self.texture = None;
        self.preview.clear();
        self.preview_pending = self.has_media_source();
    }

    fn model_contract(&self) -> Option<ModelInputContract> {
        self.use_model.then_some(ModelInputContract {
            width: self.model_width,
            height: self.model_height,
            channels: 3,
            layout: TensorLayout::Nchw,
            class_count: self.model_classes,
            decoder: DetectorDecoder::YoloChannelMajor,
        })
    }

    fn selected_model(&self) -> anyhow::Result<Option<PathBuf>> {
        if !self.use_model {
            return Ok(None);
        }
        if self.model_path.trim().is_empty() {
            anyhow::bail!("Select a model path or disable neural analysis");
        }
        Ok(Some(absolute_path(std::path::Path::new(&self.model_path))?))
    }

    fn request_preview(&mut self) {
        if !self.has_media_source() {
            self.preview_pending = false;
            self.preview_playing = false;
            self.preview.clear();
            self.texture = None;
            return;
        }
        if self.busy {
            return;
        }
        let (Some(source), Some(project)) = (self.source.clone(), self.project.as_ref()) else {
            return;
        };
        let context = FrameContext {
            source_version: source.clone(),
            source_placement: SourcePlacementId::new(format!("primary:{source}"))
                .expect("bounded source identifier"),
            // For time-based seeks, the engine replaces this placeholder with
            // the decoder's actual ordinal. It is never shown as a source frame.
            frame: FrameId::new(0),
            transform: TransformId::new("source-identity").unwrap(),
            seek_generation: self.seek_generation,
            request_generation: self.request_generation,
        };
        let source_time = match SourceTimestamp::new(
            (self.playhead_seconds * 1_000_000_000.0).round() as i64,
            1_000_000_000,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.status = error.to_string();
                self.preview_pending = false;
                return;
            }
        };
        let model_path = match self.selected_model() {
            Ok(value) => value,
            Err(error) => {
                self.status = error.to_string();
                self.preview_pending = false;
                return;
            }
        };
        let project_id = project.project_id.clone();
        let model_input = self.model_contract();
        self.preview.begin(context.clone(), source_time.clone());
        self.preview_pending = false;
        self.last_preview = Instant::now();
        self.send(
            Command::PreviewFrame {
                context,
                source_time,
                model_path,
                model_input,
            },
            Some(project_id),
            None,
        );
    }

    fn generator(&mut self, ui: &mut egui::Ui) {
        ui.heading("Generator");
        egui::ComboBox::from_label("Input modality")
            .selected_text(self.generation_mode.label()).show_ui(ui, |ui| {
                for mode in [GeneratorInputMode::Video, GeneratorInputMode::Text, GeneratorInputMode::Image,
                    GeneratorInputMode::Audio, GeneratorInputMode::Funscript] {
                    ui.selectable_value(&mut self.generation_mode, mode, mode.label());
                }
            });
        ui.label("Immutable input -> isolated worker -> review candidate -> atomic commit.");
        match self.generation_mode {
            GeneratorInputMode::Video => self.video_generator(ui),
            GeneratorInputMode::Text | GeneratorInputMode::Image => {
                ui.label("Strict pattern syntax");
                ui.text_edit_multiline(&mut self.pattern_prompt);
                ui.small("Required: pattern, axis, duration, frequency, amplitude, offset. Patterns: sine, triangle, hold, pulse.");
                ui.colored_label(Color32::YELLOW, "Deterministic preset syntax, not free-form language understanding or a local AI assistant.");
                if self.generation_mode == GeneratorInputMode::Image {
                    ui.label("Import/select a still image in Project. Prompt supplies motion; image content is not claimed to infer motion.");
                }
                let enabled = !self.busy && self.project.is_some()
                    && (self.generation_mode == GeneratorInputMode::Text || self.has_media_source());
                if ui.add_enabled(enabled, egui::Button::new("Synthesize pattern candidate")).clicked() {
                    if self.pattern_prompt.trim().is_empty() {
                        self.status = "A nonempty constrained pattern prompt is required.".into();
                    } else {
                        let input = if self.generation_mode == GeneratorInputMode::Text {
                            GenerationInput::Text { prompt: self.pattern_prompt.clone() }
                        } else {
                            GenerationInput::Image { source_version: self.source.clone().unwrap(), prompt: self.pattern_prompt.clone() }
                        };
                        self.project_command(Command::GenerateInput { input }, true);
                    }
                }
            }
            GeneratorInputMode::Audio => {
                ui.label("Full-band audio mapping");
                ui.checkbox(&mut self.audio_beats, "Amplitude-onset pulses (not musical beat tracking)");
                ui.small("Unchecked: energy envelope. Onset mode: fixed 0.35 upward RMS threshold.");
                egui::ComboBox::from_label("Output axis").selected_text(format!("{:?}", self.audio_axis))
                    .show_ui(ui, |ui| {
                        for axis in [Axis::Stroke, Axis::Sway, Axis::Surge, Axis::Roll, Axis::Pitch, Axis::Yaw] {
                            ui.selectable_value(&mut self.audio_axis, axis, format!("{axis:?}"));
                        }
                    });
                ui.add(egui::Slider::new(&mut self.audio_amplitude, 0.0..=1.0).text("Amplitude"));
                ui.add(egui::Slider::new(&mut self.audio_offset, 0.0..=1.0).text("Offset"));
                ui.small("Position = offset + amplitude * level. Offset + amplitude must not exceed 1.");
                if ui.add_enabled(!self.busy && self.project.is_some() && self.has_media_source(),
                    egui::Button::new("Generate audio candidate")).clicked() {
                    let mapping = NormalizedPosition::new(self.audio_offset)
                        .and_then(|offset| AudioMapping::new(self.audio_axis, self.audio_amplitude, offset));
                    match mapping {
                        Ok(mapping) => {
                            let input = GenerationInput::Audio { source_version: self.source.clone().unwrap(), mapping,
                                mode: if self.audio_beats { AudioMode::Beats } else { AudioMode::Envelope } };
                            self.project_command(Command::GenerateInput { input }, true);
                        }
                        Err(error) => self.status = error.to_string(),
                    }
                }
                ui.colored_label(Color32::YELLOW, "No frequency-band separation or qualified musical beat tracker is claimed.");
            }
            GeneratorInputMode::Funscript => {
                ui.label("Funscript path"); ui.text_edit_singleline(&mut self.script_import_path);
                ui.label("Import proposes an engine-owned candidate. Original script and committed timeline remain unchanged.");
                if ui.add_enabled(!self.busy && self.project.is_some(), egui::Button::new("Import script candidate")).clicked() {
                    if self.script_import_path.trim().is_empty() { self.status = "Enter a funscript path.".into(); }
                    else {
                        match absolute_path(std::path::Path::new(&self.script_import_path)) {
                            Ok(path) => { self.project_command(Command::ImportFunscript { path }, true); }
                            Err(error) => self.status = error.to_string(),
                        }
                    }
                }
            }
        }
        if let Some(job) = self.job.clone() {
            ui.separator(); ui.label(format!("Job {} / attempt {}", job.job_id, job.attempt_id));
            ui.label(format!("State: {}", job.state));
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.busy, egui::Button::new("Refresh job")).clicked() {
                    self.project_command(Command::JobStatus { job_id: job.job_id.clone() }, false);
                }
                if ui.add_enabled(!self.busy && job.candidate_id.is_none(), egui::Button::new("Cancel job")).clicked() {
                    self.project_command(Command::CancelJob { job_id: job.job_id.clone() }, false);
                }
                if let Some(candidate_id) = job.candidate_id {
                    if ui.add_enabled(!self.busy, egui::Button::new("Review candidate")).clicked() {
                        self.project_command(Command::GetCandidate { candidate_id }, false);
                    }
                }
            });
        }
        if let Some(candidate) = self.candidate.clone() {
            ui.separator();
            let stale = self.project.as_ref().is_some_and(|project| project.revision != candidate.base_revision);
            self.candidate_controls(ui, &candidate, stale);
        }
        ui.separator();
        ui.label("Free-form local AI, visual-semantic image interpretation, trained multimodal inference and physical qualification remain explicit implementation/research gaps.");
    }

    fn video_generator(&mut self, ui: &mut egui::Ui) {
        egui::ComboBox::from_label("Stroke preset").selected_text(&self.preset).show_ui(ui, |ui| {
            ui.selectable_value(&mut self.preset, "default".into(), "Default");
            ui.selectable_value(&mut self.preset, "fast".into(), "Fast");
        });
        if ui.checkbox(&mut self.use_model, "Use explicitly declared custom ONNX model (unqualified)").changed() {
            self.seek(self.playhead_seconds);
        }
        if self.use_model {
            ui.label("Model path"); ui.text_edit_singleline(&mut self.model_path);
            ui.horizontal(|ui| {
                ui.label("Input"); ui.add(egui::DragValue::new(&mut self.model_width).range(1..=4096));
                ui.add(egui::DragValue::new(&mut self.model_height).range(1..=4096));
                ui.label("Classes"); ui.add(egui::DragValue::new(&mut self.model_classes).range(1..=65536));
            });
            ui.label("Declared ABI: RGB / NCHW / YOLO channel-major. No format guessing.");
            ui.colored_label(Color32::YELLOW, "Custom model output is unqualified. Runtime must be installed and pinned by the engine.");
        }
        if ui.add_enabled(!self.busy && self.has_media_source(), egui::Button::new("Generate stroke candidate")).clicked() {
            match self.selected_model() {
                Ok(model_path) => {
                    let settings = GenerationSettings { model_input: self.model_contract(), ..Default::default() };
                    self.project_command(Command::Generate { source_version: self.source.clone().unwrap(),
                        preset: self.preset.clone(), settings: Some(settings), model_path }, true);
                }
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    fn candidate_controls(&mut self, ui: &mut egui::Ui, candidate: &CandidateSnapshot, stale: bool) {
        ui.label(format!("Candidate {} / base revision {}", candidate.candidate_id, candidate.base_revision.value()));
        ui.label("Amber curve is proposed motion; review bands identify localized uncertainty. It is not committed.");
        egui::ScrollArea::vertical().max_height(140.0).id_salt("candidate-review-flags").show(ui, |ui| {
            for flag in candidate.review.iter().take(256) {
                ui.label(format!("{} / {:?} / {:.3}..{:.3}s / {:?}",
                    flag.reason.as_str(), flag.axis, flag.range.start().as_nanos() as f64 / 1e9,
                    flag.range.end().as_nanos() as f64 / 1e9, flag.evidence));
                if let Some(confidence) = &flag.confidence {
                    ui.small(format!("Confidence {:.3} ({:?})", confidence.value(), confidence.calibration()));
                }
            }
            if candidate.review.len() > 256 { ui.label(format!("{} additional flags retained by the engine.", candidate.review.len() - 256)); }
            if candidate.review.is_empty() { ui.label("No flags supplied. This is not a guarantee of correct motion."); }
        });
        if ui.add_enabled(!self.busy, egui::Button::new("Inspect candidate and protected regions")).clicked() {
            self.project_command(Command::Diagnostics { candidate_id: Some(candidate.candidate_id.clone()) }, false);
        }
        if stale {
            ui.colored_label(Color32::YELLOW, "Project changed. Explicit rebase and fresh review are required.");
            if ui.add_enabled(!self.busy, egui::Button::new("Explicitly rebase candidate")).clicked() {
                self.project_command(Command::RebaseCandidate { candidate_id: candidate.candidate_id.clone() }, true);
            }
        }
        ui.horizontal_wrapped(|ui| {
            for axis in [Axis::Stroke, Axis::Sway, Axis::Surge, Axis::Roll, Axis::Pitch, Axis::Yaw] {
                let mut selected = self.merge_axes.contains(&axis);
                if ui.checkbox(&mut selected, format!("{axis:?}")).changed() {
                    if selected { self.merge_axes.push(axis); } else { self.merge_axes.retain(|value| *value != axis); }
                }
            }
        });
        ui.checkbox(&mut self.merge_range_enabled, "Merge only a project-time range");
        if self.merge_range_enabled {
            ui.horizontal(|ui| {
                ui.label("Start"); ui.add(egui::DragValue::new(&mut self.merge_start_seconds).range(0.0..=86_400.0).suffix(" s"));
                ui.label("End"); ui.add(egui::DragValue::new(&mut self.merge_end_seconds).range(0.0..=86_400.0).suffix(" s"));
            });
        }
        if ui.add_enabled(!self.busy && !stale && !self.merge_axes.is_empty(), egui::Button::new("Merge selected axes / preserve protection")).clicked() {
            let range = if self.merge_range_enabled {
                crate::modality::range(Some(self.merge_start_seconds), Some(self.merge_end_seconds))
            } else { Ok(None) };
            match range {
                Ok(range) => { self.project_command(Command::MergeCandidate {
                    candidate_id: candidate.candidate_id.clone(), axes: self.merge_axes.clone(), range }, true); }
                Err(error) => self.status = error.to_string(),
            }
        }
        if ui.add_enabled(!self.busy && !stale, egui::Button::new("Commit whole reviewed candidate")).clicked() {
            self.project_command(Command::CommitCandidate { candidate_id: candidate.candidate_id.clone() }, true);
        }
        if ui.button("Dismiss candidate preview").clicked() { self.candidate = None; self.diagnostics = None; }
        self.diagnostics_view(ui);
    }

    fn doctor(&mut self, ui: &mut egui::Ui) {
        ui.heading("Evidence and protection review");
        ui.label("Read-only engine diagnostics. No edits, automatic repairs, or physical qualification.");
        ui.label("Legacy speed/jitter doctor and repair options are separate pending adapters.");
        if ui.add_enabled(!self.busy && self.project.is_some(), egui::Button::new("Inspect committed revision")).clicked() {
            self.project_command(Command::Diagnostics { candidate_id: None }, false);
        }
        self.diagnostics_view(ui);
    }

    fn diagnostics_current(&self) -> Option<&DiagnosticsReport> {
        self.diagnostics.as_ref().filter(|report| diagnostics_match_view(
            report,
            self.project.as_ref().map(|project| (&project.project_id, project.revision)),
            self.candidate.as_ref().map(|candidate| &candidate.candidate_id),
        ))
    }

    fn diagnostics_view(&self, ui: &mut egui::Ui) {
        let Some(report) = self.diagnostics_current() else { return };
        if report.candidate_id.is_some() && self.candidate.as_ref().map(|candidate| &candidate.candidate_id) != report.candidate_id.as_ref() {
            ui.label("Diagnostics refer to a different candidate; request fresh diagnostics."); return;
        }
        ui.separator();
        ui.label(format!("Diagnostics revision {} / {} protected regions", report.revision.value(), report.protected_regions.len()));
        for region in &report.protected_regions {
            ui.label(format!("Protected {:?}: {:.3}..{:.3}s", region.axis,
                region.range.start().as_nanos() as f64 / 1e9, region.range.end().as_nanos() as f64 / 1e9));
        }
        for issue in report.issues.iter().take(256) {
            let span = issue.range.as_ref().map(|range| format!("{:.3}..{:.3}s",
                range.start().as_nanos() as f64 / 1e9, range.end().as_nanos() as f64 / 1e9)).unwrap_or_default();
            ui.label(format!("{} {:?} {}: {}", issue.code, issue.axis, span, issue.message));
        }
        if report.issues.is_empty() { ui.label("No issues reported by these checks. Neural accuracy and safety remain unqualified."); }
    }

    fn paint_review_spans(&self, painter: &egui::Painter, rect: egui::Rect, duration: f64) {
        let paint = |range: &TimeRange, color: Color32| {
            let start = (range.start().as_nanos() as f64 / 1e9 / duration).clamp(0.0, 1.0) as f32;
            let end = (range.end().as_nanos() as f64 / 1e9 / duration).clamp(0.0, 1.0) as f32;
            if end > start {
                painter.rect_filled(egui::Rect::from_min_max(
                    egui::pos2(rect.left() + start * rect.width(), rect.top()),
                    egui::pos2(rect.left() + end * rect.width(), rect.bottom())), 0.0, color);
            }
        };
        if let Some(candidate) = &self.candidate {
            for flag in &candidate.review {
                if flag.axis.is_none() || flag.axis == Some(self.timeline_axis) {
                    paint(&flag.range, Color32::from_rgba_unmultiplied(235, 159, 43, 30));
                }
            }
        }
        if let Some(report) = self.diagnostics_current() {
            for region in &report.protected_regions {
                if region.axis.is_none() || region.axis == Some(self.timeline_axis) {
                    paint(&region.range, Color32::from_rgba_unmultiplied(158, 169, 188, 60));
                }
            }
        }
    }

    fn has_media_source(&self) -> bool {
        self.project.as_ref().is_some_and(|project| project.sources.iter().any(|source|
            source.kind == pulsar_protocol::SourceKind::Media
                && self.source.as_ref() == Some(&source.source_version)))
    }

    fn project_panel(&mut self, ui: &mut egui::Ui) {
        if let Some((count, revision)) = self.diagnostics_current()
            .map(|report| (report.issues.len(), report.revision.value())) {
            ui.heading("Review status");
            if count > 0 {
                ui.colored_label(Color32::YELLOW, format!("{count} diagnostic issue(s) at revision {revision}"));
            } else {
                ui.label(format!("No issues from current checks at revision {revision}."));
                ui.small("This is not accuracy or safety qualification.");
            }
            if ui.button("Open read-only diagnostics").clicked() {
                self.tab = Tab::Doctor;
            }
            ui.separator();
        }
        ui.heading("Project");
        ui.text_edit_singleline(&mut self.project_name);
        if ui
            .add_enabled(!self.busy, egui::Button::new("Create project"))
            .clicked()
        {
            self.send(
                Command::CreateProject {
                    name: self.project_name.clone(),
                },
                None,
                None,
            );
        }
        ui.label("Existing project ID");
        ui.text_edit_singleline(&mut self.open_project_id);
        if ui
            .add_enabled(!self.busy, egui::Button::new("Open project"))
            .clicked()
        {
            match ProjectId::new(self.open_project_id.trim()) {
                Ok(project_id) => {
                    self.send(Command::OpenProject { project_id }, None, None);
                }
                Err(error) => self.status = error.to_string(),
            }
        }
        if let Some(project) = &self.project {
            ui.separator();
            ui.label(RichText::new(&project.name).strong());
            ui.label(format!("Revision {}", project.revision.value()));
            ui.label(format!("{} motion axes", project.program.tracks().len()));
            let sources = project.sources.clone();
            for source in sources {
                if source.kind != pulsar_protocol::SourceKind::Media {
                    let kind = match source.kind {
                        pulsar_protocol::SourceKind::GenerationInput => "generation input",
                        pulsar_protocol::SourceKind::Funscript => "funscript",
                        pulsar_protocol::SourceKind::Media => unreachable!(),
                    };
                    ui.add_enabled(false, egui::Label::new(format!("{} [{}; not previewable]", source.label, kind)));
                    continue;
                }
                if ui
                    .selectable_label(
                        self.source.as_ref() == Some(&source.source_version),
                        &source.label,
                    )
                    .clicked()
                {
                    self.source = Some(source.source_version);
                    self.seek(0.0);
                }
            }
        }
        ui.separator();
        ui.label("Media path");
        ui.text_edit_singleline(&mut self.source_path);
        if ui
            .add_enabled(
                !self.busy && self.project.is_some(),
                egui::Button::new("Import immutable source"),
            )
            .clicked()
        {
            if self.source_path.trim().is_empty() {
                self.status = "Enter a source path.".into();
            } else {
                match absolute_path(std::path::Path::new(&self.source_path)) {
                    Ok(path) => {
                        self.project_command(Command::ImportSource { path }, true);
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
        }
        ui.separator();
        ui.label(format!("Export {:?} .funscript path", self.timeline_axis));
        ui.text_edit_singleline(&mut self.export_path);
        if ui
            .add_enabled(
                !self.busy && self.project.is_some(),
                egui::Button::new("Export committed revision"),
            )
            .clicked()
        {
            if self.export_path.trim().is_empty() {
                self.status = "Enter an export path.".into();
            } else {
                match absolute_path(std::path::Path::new(&self.export_path)) {
                    Ok(path) => {
                        self.project_command(Command::ExportAxis { path, axis: self.timeline_axis }, true);
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("Undo"),
                )
                .clicked()
            {
                self.project_command(Command::Undo, true);
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("Redo"),
                )
                .clicked()
            {
                self.project_command(Command::Redo, true);
            }
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("Refresh"),
                )
                .clicked()
            {
                self.project_command(Command::GetSnapshot, false);
            }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        if !self.has_media_source() {
            ui.label("No previewable media source. Generated patterns and scripts remain available on the timeline.");
        }
        ui.add_enabled_ui(self.has_media_source(), |ui| {

        ui.horizontal(|ui| {
            if ui
                .button(if self.preview_playing {
                    "Pause preview"
                } else {
                    "Play preview"
                })
                .clicked()
            {
                self.preview_playing = !self.preview_playing;
                self.last_tick = Instant::now();
            }
            let response = ui.add(
                egui::DragValue::new(&mut self.playhead_seconds)
                    .speed(0.04)
                    .range(0.0..=86_400.0)
                    .suffix(" s"),
            );
            if response.changed() {
                self.seek(self.playhead_seconds);
            }
            if ui
                .add_enabled(
                    !self.busy && self.has_media_source(),
                    egui::Button::new("Analyze displayed time"),
                )
                .clicked()
            {
                self.seek(self.playhead_seconds);
            }
        });
        ui.small("Independent preview only. No audio synchronization or physical commands. Source identity projection; VR reprojection unavailable.");
        let available = ui.available_size();
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(available.x.max(1.0), (available.y - 70.0).max(80.0)),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_rgb(8, 11, 17));
        if let Some(texture) = &self.texture {
            let image_size = texture.size_vec2();
            let scale = (rect.width() / image_size.x).min(rect.height() / image_size.y);
            let image_rect = egui::Rect::from_center_size(rect.center(), image_size * scale);
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if let Some(result) = self.preview.displayed() {
                for observation in &result.observations {
                    if !observation.matches_frame(&result.context) {
                        continue;
                    }
                    let bounds = observation.bounds();
                    // The worker publishes normalized source-plane coordinates.
                    // Unmapped model/crop/viewport pixels are never guessed.
                    if bounds.space() != CoordinateSpace::ProjectionNormalized {
                        continue;
                    }
                    let min = egui::pos2(
                        image_rect.left() + bounds.x_min() as f32 * image_rect.width(),
                        image_rect.top() + bounds.y_min() as f32 * image_rect.height(),
                    );
                    let max = egui::pos2(
                        image_rect.left() + bounds.x_max() as f32 * image_rect.width(),
                        image_rect.top() + bounds.y_max() as f32 * image_rect.height(),
                    );
                    let color = if observation.evidence() == EvidenceKind::Observed {
                        Color32::from_rgb(61, 223, 229)
                    } else {
                        Color32::from_rgb(255, 188, 79)
                    };
                    painter.rect_stroke(
                        egui::Rect::from_min_max(min, max),
                        1.0,
                        Stroke::new(2.0_f32, color),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        min,
                        egui::Align2::LEFT_BOTTOM,
                        format!("{:?}", observation.evidence()),
                        egui::FontId::monospace(12.0),
                        color,
                    );
                }
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.busy && self.has_media_source() {
                    "Waiting for a matching source frame"
                } else {
                    "Import media, then request a preview"
                },
                egui::FontId::proportional(18.0),
                Color32::from_rgb(151, 166, 185),
            );
        }
        if let Some(result) = self.preview.displayed() {
            ui.label(format!(
                "Decoded source frame {} / source time {:?}",
                result.source_frame_index, result.source_time
            ));
            match &result.analysis {
                AnalysisStatus::Available => {
                    ui.label(format!("{} matching observations. Empty means no detection, not a fixed analysis box.", result.observations.len()));
                }
                AnalysisStatus::Pending => {
                    ui.label("Analysis pending. No stale evidence shown.");
                }
                AnalysisStatus::Unavailable { reason } => {
                    ui.colored_label(Color32::YELLOW, format!("Analysis unavailable: {reason}"));
                }
            }
        }
            });
    }

    fn timeline_duration_seconds(&self) -> f64 {
        self.project.iter().map(|project| &project.program)
            .chain(self.candidate.iter().map(|candidate| &candidate.program))
            .flat_map(|program| program.tracks()).flat_map(|track| track.actions())
            .map(|action| action.time().as_nanos() as f64 / 1e9)
            .fold(10.0, f64::max).max(self.playhead_seconds + 1.0)
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Timeline axis");
            for axis in [Axis::Stroke, Axis::Sway, Axis::Surge, Axis::Roll, Axis::Pitch, Axis::Yaw] {
                if ui.selectable_value(&mut self.timeline_axis, axis, format!("{axis:?}")).changed() {
                    self.gesture = None;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{:?} / NEUTRAL MASTER TIMELINE", self.timeline_axis)).strong());
            ui.add(egui::Slider::new(&mut self.new_position, 0.0..=1.0).text("New position"));
            if ui
                .add_enabled(
                    !self.busy && self.project.is_some(),
                    egui::Button::new("Add at playhead"),
                )
                .clicked()
            {
                if let Some(project) = &self.project {
                    let mut points = editor::axis_points(&project.program, self.timeline_axis);
                    points.push(EditorPoint {
                        nanos: (self.playhead_seconds * 1_000_000_000.0).round() as i64,
                        position: self.new_position,
                    });
                    match editor::replace_axis(&project.program, self.timeline_axis, &points) {
                        Ok(program) => {
                            self.send_edit(
                                EditProposal { program, label: format!("Add {:?} keyframe", self.timeline_axis) },
                                project.project_id.clone(), project.revision,
                            );
                        }
                        Err(error) => self.status = error.to_string(),
                    }
                }
            }
        });
        let committed = self
            .project
            .as_ref()
            .map(|project| editor::axis_points(&project.program, self.timeline_axis))
            .unwrap_or_default();
        let points = self
            .gesture
            .as_ref()
            .map(|gesture| gesture.points.clone())
            .unwrap_or_else(|| committed.clone());
        let candidate = self
            .candidate
            .as_ref()
            .map(|candidate| editor::axis_points(&candidate.program, self.timeline_axis))
            .unwrap_or_default();
        let duration = self.timeline_duration_seconds();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 138.0),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::from_rgb(10, 15, 23));
        self.paint_review_spans(&painter, rect, duration);
        for index in 0..=4 {
            let y = rect.top() + rect.height() * index as f32 / 4.0;
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                Stroke::new(1.0_f32, Color32::from_rgb(30, 42, 56)),
            );
        }
        let to_screen = |point: &EditorPoint| {
            egui::pos2(
                rect.left() + (point.nanos as f64 / 1e9 / duration) as f32 * rect.width(),
                rect.bottom() - point.position as f32 * rect.height(),
            )
        };
        let committed_gaps = self.project.as_ref().and_then(|project| project.program.track(self.timeline_axis))
            .map(|track| track.gaps()).unwrap_or_default();
        let candidate_gaps = self.candidate.as_ref().and_then(|candidate| candidate.program.track(self.timeline_axis))
            .map(|track| track.gaps()).unwrap_or_default();
        for (line, gaps, color) in [
            (&candidate, candidate_gaps, Color32::from_rgb(221, 160, 54)),
            (&points, committed_gaps, Color32::from_rgb(57, 214, 227)),
        ] {
            for gap in gaps {
                let left = (gap.start().as_nanos() as f64 / 1e9 / duration).clamp(0.0, 1.0) as f32;
                let right = (gap.end().as_nanos() as f64 / 1e9 / duration).clamp(0.0, 1.0) as f32;
                painter.rect_filled(egui::Rect::from_min_max(
                    egui::pos2(rect.left() + left * rect.width(), rect.top()),
                    egui::pos2(rect.left() + right * rect.width(), rect.bottom())),
                    0.0, Color32::from_rgba_unmultiplied(190, 80, 96, 38));
            }
            editor::available_segments(line, gaps, |segment| {
                painter.line_segment([to_screen(&segment[0]), to_screen(&segment[1])], Stroke::new(1.7_f32, color));
            });
            for point in line {
                if editor::point_is_available(point.nanos, gaps) {
                    painter.circle_filled(to_screen(point), 3.3, color);
                }
            }
        }
        if !committed_gaps.is_empty() || !candidate_gaps.is_empty() {
            painter.text(rect.left_top() + egui::vec2(5.0, 4.0), egui::Align2::LEFT_TOP,
                "Shaded gaps: motion unavailable; curves are clipped, not filled.",
                egui::FontId::proportional(11.0), Color32::from_rgb(240, 173, 185));
        }
        let cursor_x = rect.left() + (self.playhead_seconds / duration) as f32 * rect.width();
        painter.line_segment(
            [
                egui::pos2(cursor_x, rect.top()),
                egui::pos2(cursor_x, rect.bottom()),
            ],
            Stroke::new(1.0_f32, Color32::WHITE),
        );
        if response.drag_started() && !self.busy {
            if let (Some(pointer), Some(project)) =
                (response.interact_pointer_pos(), self.project.as_ref())
            {
                let nearest = committed
                    .iter()
                    .enumerate()
                    .map(|(i, point)| (i, pointer.distance(to_screen(point))))
                    .filter(|(_, distance)| *distance <= 14.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((selected, _)) = nearest {
                    self.gesture = Some(EditGesture {
                        project_id: project.project_id.clone(),
                        base_revision: project.revision,
                        base_program: project.program.clone(),
                        axis: self.timeline_axis,
                        points: committed,
                        selected,
                    });
                }
            }
        }
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let seconds =
                    ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * duration;
                if let Some(gesture) = &mut self.gesture {
                    gesture.points[gesture.selected] = EditorPoint {
                        nanos: (seconds * 1e9).round() as i64,
                        position: ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0)
                            as f64,
                    };
                } else {
                    self.seek(seconds);
                }
            }
        }
        if response.drag_stopped() {
            if let Some(gesture) = self.gesture.take() {
                match gesture.finish() {
                    Ok((command, project, revision)) => {
                        self.send_edit(command, project, revision);
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
        }
        if response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.seek(
                    ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * duration,
                );
            }
        }
        ui.small("Drag a point: one atomic edit on release. Click elsewhere: independent preview seek. White = playhead; cyan = committed/draft gesture; amber = candidate.");
    }

    fn capability_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Capabilities and qualification");
        if let Some(capabilities) = &self.capabilities {
            ui.label(format!("Protocol {}", capabilities.protocol_version));
            for backend in &capabilities.backends {
                ui.label(format!(
                    "{}: {} / {:?}",
                    backend.name,
                    if backend.available {
                        "available"
                    } else {
                        "unavailable"
                    },
                    backend.qualification
                ));
                if let Some(reason) = &backend.reason {
                    ui.small(reason);
                }
            }
            ui.separator();
            ui.label("Engine operations");
            for operation in &capabilities.operations {
                ui.monospace(operation);
            }
        }
        ui.separator();
        ui.colored_label(Color32::YELLOW, "Physical playback is disabled here. Preview motion is not device admission or physical stopping evidence.");
        ui.add_enabled(
            false,
            egui::Button::new("Arm physical playback: qualification required"),
        );
        ui.label("Legacy script doctor/repair, spectral synthesis, hardware dashboards, Stash and extensions remain visible migration gaps, not replacement behavior.");
        ui.add_enabled(
            false,
            egui::Button::new("Local assistant: scoped engine adapter pending"),
        );
        if ui
            .add_enabled(!self.busy, egui::Button::new("Refresh capabilities"))
            .clicked()
        {
            self.send(Command::Capabilities, None, None);
        }
    }
}

impl eframe::App for PulsarDesktop {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(context);
        let now = Instant::now();
        if self.preview_playing {
            let seconds = self.playhead_seconds + now.duration_since(self.last_tick).as_secs_f64();
            if now.duration_since(self.last_preview) >= Duration::from_millis(150) {
                self.seek(seconds);
            } else {
                self.playhead_seconds = seconds;
            }
            context.request_repaint_after(Duration::from_millis(30));
        }
        self.last_tick = now;
        egui::TopBottomPanel::top("pulsar-header").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("PULSAR")
                        .size(24.0)
                        .strong()
                        .color(Color32::from_rgb(62, 217, 226)),
                );
                ui.label("Local motion workstation");
                if self.busy {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("Development build / unqualified output");
                });
            });
        });
        egui::TopBottomPanel::bottom("pulsar-status").show(context, |ui| {
            ui.label(&self.status);
        });
        egui::SidePanel::left("pulsar-project")
            .default_width(245.0)
            .min_width(200.0)
            .resizable(true)
            .show(context, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (tab, label) in [
                            (Tab::Studio, "Studio"),
                            (Tab::Cinema, "Cinema"),
                            (Tab::Generator, "Generator"),
                            (Tab::Doctor, "Doctor"),
                            (Tab::Devices, "Devices"),
                            (Tab::Automation, "Automation"),
                        ] {
                            ui.selectable_value(&mut self.tab, tab, label);
                        }
                    });
                    ui.separator();
                    self.project_panel(ui);
                });
            });
        egui::TopBottomPanel::bottom("pulsar-timeline")
            .resizable(true)
            .default_height(205.0)
            .show(context, |ui| self.timeline(ui));
        egui::CentralPanel::default().show(context, |ui| match self.tab {
            Tab::Studio | Tab::Cinema => self.canvas(ui),
            Tab::Generator => {
                egui::ScrollArea::vertical().show(ui, |ui| self.generator(ui));
            }
            Tab::Doctor => { egui::ScrollArea::vertical().show(ui, |ui| self.doctor(ui)); }
            _ => {
                egui::ScrollArea::vertical().show(ui, |ui| self.capability_panel(ui));
            }
        });
        if !self.busy && self.preview_pending {
            self.request_preview();
        }
        if !self.busy && now.duration_since(self.last_job_poll) >= Duration::from_millis(750) {
            if let Some(job) = self.job.as_ref() {
                if job.candidate_id.is_none()
                    && job.error.is_none()
                    && !matches!(
                        job.state.to_ascii_lowercase().as_str(),
                        "cancelled" | "canceled" | "failed" | "interrupted"
                    )
                {
                    let command = Command::JobStatus {
                        job_id: job.job_id.clone(),
                    };
                    self.last_job_poll = now;
                    self.project_command(command, false);
                }
            }
        }
        if self.busy || self.job.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod source_kind_tests {
    use super::select_media_source;
    use pulsar_protocol::{SourceKind, SourceSummary, SourceVersionId};

    fn source(id: &str, label: &str, kind: SourceKind) -> SourceSummary {
        SourceSummary { source_version: SourceVersionId::new(id).unwrap(), label: label.into(), kind }
    }

    #[test]
    fn synthetic_and_script_sources_never_become_media_from_their_labels() {
        let sources = vec![
            source("text", "movie.mp4", SourceKind::GenerationInput),
            source("script", "image.png", SourceKind::Funscript),
        ];
        assert_eq!(select_media_source(&sources, None), None);
        for source in &sources {
            assert_eq!(select_media_source(&sources, Some(&source.source_version)), None);
        }
    }

    #[test]
    fn media_selection_skips_nonmedia_and_preserves_an_existing_media_choice() {
        let sources = vec![
            source("text", "Generation input", SourceKind::GenerationInput),
            source("media-one", "script.funscript", SourceKind::Media),
            source("script", "movie.mp4", SourceKind::Funscript),
            source("media-two", "Generation input", SourceKind::Media),
        ];
        assert_eq!(select_media_source(&sources, None), Some(sources[1].source_version.clone()));
        assert_eq!(select_media_source(&sources, Some(&sources[3].source_version)),
            Some(sources[3].source_version.clone()));
        assert_eq!(select_media_source(&sources, Some(&sources[0].source_version)),
            Some(sources[1].source_version.clone()));
    }

    #[test]
    fn removed_or_reclassified_media_is_not_retained_after_snapshot_refresh() {
        let selected = SourceVersionId::new("old-media").unwrap();
        assert_eq!(select_media_source(&[], Some(&selected)), None);
        let sources = vec![source("old-media", "movie.mp4", SourceKind::GenerationInput)];
        assert_eq!(select_media_source(&sources, Some(&selected)), None);
    }
}

#[cfg(test)]
mod diagnostics_freshness_tests {
    use super::accept_diagnostics_report;
    use pulsar_protocol::{CandidateId, DiagnosticsReport, ProjectId, RevisionId};

    fn report(project: &str, revision: u64, candidate: Option<&str>) -> DiagnosticsReport {
        DiagnosticsReport {
            project_id: ProjectId::new(project).unwrap(),
            revision: RevisionId::new(revision),
            candidate_id: candidate.map(|id| CandidateId::new(id).unwrap()),
            base_revision: candidate.map(|_| RevisionId::new(revision)),
            protected_regions: Vec::new(),
            issues: Vec::new(),
        }
    }

    #[test]
    fn late_old_project_and_revision_reports_do_not_replace_current_diagnostics() {
        let project = ProjectId::new("current-project").unwrap();
        let view = Some((&project, RevisionId::new(7)));
        let mut current = None;
        assert!(accept_diagnostics_report(&mut current, report("current-project", 7, None), view, None));
        assert!(!accept_diagnostics_report(&mut current, report("old-project", 7, None), view, None));
        assert!(!accept_diagnostics_report(&mut current, report("current-project", 6, None), view, None));
        let retained = current.unwrap();
        assert_eq!(retained.project_id, project);
        assert_eq!(retained.revision, RevisionId::new(7));
    }

    #[test]
    fn candidate_diagnostics_require_the_visible_candidate_identity() {
        let project = ProjectId::new("project").unwrap();
        let candidate = CandidateId::new("visible-candidate").unwrap();
        let view = Some((&project, RevisionId::new(7)));
        let mut current = None;
        assert!(!accept_diagnostics_report(&mut current, report("project", 7, Some("old-candidate")), view, Some(&candidate)));
        assert!(!accept_diagnostics_report(&mut current, report("project", 7, Some("visible-candidate")), view, None));
        assert!(accept_diagnostics_report(&mut current, report("project", 7, Some("visible-candidate")), view, Some(&candidate)));
        assert_eq!(current.as_ref().unwrap().candidate_id.as_ref(), Some(&candidate));
    }

    #[test]
    fn diagnostics_for_an_unopened_project_are_never_accepted() {
        let mut current = None;
        assert!(!accept_diagnostics_report(&mut current, report("project", 7, None), None, None));
        assert!(current.is_none());
    }
}

use crate::motion::{ResponseBody, ProjectSnapshot, CandidateSnapshot, EditProposal};

#[cfg(test)]
mod edit_transport_tests {
    use super::*;
    use crate::motion::{fixture_descriptor, EditProposal};

    struct EditApi { calls: Vec<&'static str>, conflict: bool }
    impl EngineApi for EditApi {
        fn upload_edit(&mut self, project: ProjectId, revision: RevisionId,
            program: &MotionProgram, _: &str) -> anyhow::Result<CandidateSnapshot> {
            assert_eq!(project.as_str(), "project"); assert_eq!(revision.value(), 4);
            self.calls.push("upload");
            let candidate_id = CandidateId::new("edit-candidate").unwrap();
            Ok(CandidateSnapshot { candidate_id: candidate_id.clone(), project_id: project.clone(),
                base_revision: revision, motion: fixture_descriptor(program, MotionBinding::Candidate {
                    project_id: project, candidate_id, base_revision: revision }),
                program: program.clone().into(), job_id: None, review: Vec::new() })
        }
        fn execute(&mut self, command: Command, project: Option<ProjectId>,
            revision: Option<RevisionId>) -> anyhow::Result<ResponseBody> {
            assert!(matches!(command, Command::CommitCandidate { candidate_id } if candidate_id.as_str() == "edit-candidate"));
            assert_eq!(project.unwrap().as_str(), "project"); assert_eq!(revision, Some(RevisionId::new(4)));
            self.calls.push("commit");
            if self.conflict { Err(ProtocolError::new(ErrorCode::RevisionConflict, "stale base").into()) }
            else { Ok(ResponseBody::Ack) }
        }
    }
    fn draft() -> RpcWork {
        RpcWork { command: RpcCommand::Edit(EditProposal { program: MotionProgram::default(), label: "Edit".into() }),
            project: Some(ProjectId::new("project").unwrap()), revision: Some(RevisionId::new(4)) }
    }
    #[test]
    fn gui_draft_uses_shared_upload_then_normal_pinned_candidate_commit() {
        let mut api = EditApi { calls: Vec::new(), conflict: false };
        assert!(matches!(execute_work(&mut api, draft()).unwrap(), ResponseBody::Ack));
        assert_eq!(api.calls, vec!["upload", "commit"]);
    }
    #[test]
    fn stale_gui_commit_retains_candidate_identity_and_is_never_replayed() {
        let mut api = EditApi { calls: Vec::new(), conflict: true };
        let error = execute_work(&mut api, draft()).unwrap_err();
        assert!(error.to_string().contains("edit-candidate"));
        assert_eq!(error.downcast_ref::<ProtocolError>().unwrap().code, ErrorCode::RevisionConflict);
        assert_eq!(api.calls, vec!["upload", "commit"]);
    }
}

