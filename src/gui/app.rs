//! Main Funscript Hub desktop application window.

use anyhow::Context;
use crate::audio::{
    generate_multiband_companion_bundle, synthesize_spectral_actions, AudioWaveform, FrequencyBand,
    SpectralInfillConfig, SpectralInfillMode,
};
use crate::batch::queue::{BatchJob, BatchJobConfig, BatchJobStatus, BatchProgressUpdate, BatchQueue, BatchWorker};
use crate::funscript::{
    diagnose_funscript, AxisChannel, DoctorConfig, DoctorReport, Funscript, InfillPattern,
    MultiAxisScript, UndoHistory,
};
use crate::gui::rig_simulator::{show_rig_simulator, RigViewState};
use crate::gui::timeline::{show_timeline, TimelineState};
use crate::kinematics::{
    eval_scurve, smooth_funscript, DeviceProfile, DeviceType, KinematicState,
    RigInput, RigModel, SCurvePreset, ThermalModel, ThermalStatus,
};
use crate::neural::model_manager::{auto_detect_model, inspect_model, ModelInfo};
use crate::plugin::engine::PluginHost;
use crate::stash::client::{StashClient, StashConfig, StashScene};
use crate::sync::{
    format_tcode_v03, DetectedSerialPort, HandyConfig, HandyDispatcher, SerialDispatcher,
    TCodeDispatcher, VrSyncMessage, VrSyncServer,
};
use eframe::egui::{
    self, Align, Color32, Context as EguiContext, Layout, ProgressBar, RichText, ScrollArea,
    Slider, TopBottomPanel, Ui,
};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubTab {
    Studio,       // F1: Studio Editor (Video Canvas + Keyframe Inspector + Timeline + Waveform)
    Cinema,       // F2: Cinema Player (Full-window Video Canvas + Floating HUD)
    Generator,    // F3: Generator Studio
    ScriptDoctor, // F4: Script Doctor & Repair
    DeviceSync,   // F5: Device & Hardware Sync
    Automation,   // F6: Stash & Plugin Automation Ecosystem
}

pub struct VideoFrameRequest {
    pub path: PathBuf,
    pub time_ms: i64,
    pub width: u32,
    pub height: u32,
}

pub struct VideoFrameResponse {
    pub time_ms: i64,
    pub width: u32,
    pub height: u32,
    pub rgb_data: Vec<u8>,
}

pub enum WorkerMessage {
    Progress { percent: f32, status: String },
    Done(Result<MultiAxisScript, String>),
}

pub struct FunGenApp {
    /// Active interface tab
    pub active_tab: HubTab,

    /// Currently loaded funscript
    pub script: Funscript,
    /// Multi-axis script collection (6-DOF + Suction)
    pub multi_axis: MultiAxisScript,
    /// Currently selected active editing channel
    pub active_axis: AxisChannel,
    /// Whether to show faint ghost curves of inactive channels on timeline
    pub show_ghost_axes: bool,

    /// Infill generator modal and parameters
    pub infill_dialog_open: bool,
    pub infill_pattern: InfillPattern,
    pub infill_freq_hz: f32,
    pub infill_min_pos: i32,
    pub infill_max_pos: i32,
    pub infill_start_ms: i64,
    pub infill_end_ms: i64,

    /// Path to the loaded funscript file
    pub script_path: Option<PathBuf>,
    /// Interactive timeline state (zoom, pan, selection, playhead)
    pub timeline_state: TimelineState,
    /// Latest Script Doctor quality audit report
    pub doctor_report: Option<DoctorReport>,
    /// User notification or repair action message
    pub status_message: Option<(String, Instant)>,
    /// Undo/redo history engine
    pub undo_history: UndoHistory,

    // Media & Waveform
    pub audio_waveform: Option<AudioWaveform>,
    waveform_rx: Option<Receiver<AudioWaveform>>,
    pub video_texture: Option<egui::TextureHandle>,
    pub last_video_frame_ts: i64,
    pub video_req_tx: Option<Sender<VideoFrameRequest>>,
    pub video_resp_rx: Option<Receiver<VideoFrameResponse>>,
    pub video_frame_cache: VecDeque<(i64, egui::TextureHandle)>,
    pub video_pending_req_ts: Option<i64>,
    pub playback_streamer: Option<crate::video::PlaybackStreamer>,

    // Playback simulation & Synchronized Audio
    pub is_playing: bool,
    pub playback_speed: f64,
    pub last_frame_time: Option<Instant>,
    pub audio_player: crate::audio::AudioPlayer,
    pub show_tracking_overlay: bool,

    // Generator settings
    pub video_path: Option<PathBuf>,
    pub video_info: Option<String>,
    pub model_path: Option<PathBuf>,
    pub detected_model_info: Option<ModelInfo>,
    pub model_download_in_progress: bool,
    pub model_download_status: Option<String>,
    model_rx: Option<Receiver<Result<PathBuf, String>>>,
    pub target_fps: f64,
    pub neural_conf: f32,
    pub pov_mode: bool,
    pub vr_mode: bool,
    pub detrend_window: f64,
    pub norm_window: f64,
    pub use_neural: bool,
    pub generate_multi_axis: bool,

    // Background generation task state
    pub is_generating: bool,
    pub generation_progress: f32,
    pub generation_status: String,
    worker_rx: Option<Receiver<WorkerMessage>>,

    // Device, Kinematics & Physics Simulation
    pub device_profile: DeviceProfile,
    pub kinematic_state: KinematicState,
    pub thermal_model: ThermalModel,
    pub scurve_preset: SCurvePreset,

    // T-Code Hardware Protocol
    pub tcode_dispatcher: TCodeDispatcher,
    pub sync_udp_address: String,
    pub sync_udp_enabled: bool,
    pub last_tcode_string: String,

    // Direct USB Serial Hardware COM Port
    pub serial_dispatcher: SerialDispatcher,
    pub available_serial_ports: Vec<DetectedSerialPort>,
    pub selected_serial_port: String,
    pub selected_baud_rate: u32,

    // Interactive 3D Hardware Rig Simulator
    pub show_rig_simulator: bool,
    pub rig_model: RigModel,
    pub rig_view_state: RigViewState,

    // The Handy Cloud & Local Hardware Sync
    pub handy_dispatcher: HandyDispatcher,
    pub handy_config: HandyConfig,

    // VR Headset Sync Server & SBS Mode
    pub vr_server: Option<VrSyncServer>,
    pub vr_server_port: u16,
    pub vr_sbs_mode: bool,
    pub vr_ipd_offset: f32,

    // Stash Media Automation
    pub stash_config: StashConfig,
    pub stash_scenes: Vec<StashScene>,
    pub stash_connected: bool,
    pub stash_status: Option<String>,

    // Batch Queue Processing
    pub batch_queue: BatchQueue,
    pub batch_config: BatchJobConfig,
    pub batch_scan_path: String,
    pub batch_worker: BatchWorker,

    // Community Plugin & Procedural Macro SDK
    pub plugin_host: PluginHost,
    pub plugin_dialog_open: bool,
    pub selected_plugin_idx: usize,
    pub plugin_param_values: HashMap<String, f32>,

    // Audio Spectral Haptic Infill
    pub show_spectral_infill_dialog: bool,
    pub spectral_infill_config: SpectralInfillConfig,
    pub spectral_use_loop_range: bool,

    // Phase 8 Adaptive ML Pipeline State
    pub adaptive_profile: crate::neural::pipeline::AdaptiveProfile,
    pub live_branch: crate::neural::router::ExecutionBranch,
    pub live_observability: crate::neural::hypothesis::ObservabilityState,
    pub live_uncertainty: crate::neural::router::CalibratedUncertainty,

    // Help & About Modals
    pub help_dialog_open: bool,
    pub about_dialog_open: bool,
}

impl Default for FunGenApp {
    fn default() -> Self {
        let script = Funscript::new(vec![]);
        let doctor_report = Some(diagnose_funscript(&script, &DoctorConfig::default()));

        let detected_model = auto_detect_model();
        let initial_model_path = detected_model.as_ref().map(|m| m.path.clone());
        let ports = SerialDispatcher::list_ports();
        let default_port = ports.first().map(|p| p.name.clone()).unwrap_or_default();

        Self {
            active_tab: HubTab::Studio,
            script,
            multi_axis: MultiAxisScript::new(),
            active_axis: AxisChannel::Stroke,
            show_ghost_axes: true,

            infill_dialog_open: false,
            infill_pattern: InfillPattern::Sine,
            infill_freq_hz: 1.5,
            infill_min_pos: 10,
            infill_max_pos: 90,
            infill_start_ms: 0,
            infill_end_ms: 5000,

            script_path: None,
            timeline_state: TimelineState::default(),
            doctor_report,
            status_message: None,
            undo_history: UndoHistory::default(),

            audio_waveform: None,
            waveform_rx: None,
            video_texture: None,
            last_video_frame_ts: -999,
            video_req_tx: None,
            video_resp_rx: None,
            video_frame_cache: VecDeque::new(),
            video_pending_req_ts: None,

            is_playing: false,
            playback_speed: 1.0,
            last_frame_time: None,
            audio_player: crate::audio::AudioPlayer::default(),
            show_tracking_overlay: true,

            video_path: None,
            video_info: None,
            model_path: initial_model_path,
            detected_model_info: detected_model,
            model_download_in_progress: false,
            model_download_status: None,
            model_rx: None,
            target_fps: 30.0,
            neural_conf: 0.35,
            pov_mode: false,
            vr_mode: false,
            detrend_window: 2.0,
            norm_window: 3.0,
            use_neural: true,
            generate_multi_axis: true,

            is_generating: false,
            generation_progress: 0.0,
            generation_status: "Idle".to_string(),
            worker_rx: None,

            device_profile: DeviceProfile::for_device(DeviceType::TheHandy),
            kinematic_state: KinematicState::new(),
            thermal_model: ThermalModel::new(),
            scurve_preset: SCurvePreset::Balanced,

            tcode_dispatcher: TCodeDispatcher::new("127.0.0.1:8888"),
            sync_udp_address: "127.0.0.1:8888".to_string(),
            sync_udp_enabled: false,
            last_tcode_string: String::new(),

            serial_dispatcher: SerialDispatcher::new(),
            available_serial_ports: ports,
            selected_serial_port: default_port,
            selected_baud_rate: 115200,

            show_rig_simulator: false,
            rig_model: RigModel::OSR2,
            rig_view_state: RigViewState::default(),

            playback_streamer: None,
            handy_dispatcher: HandyDispatcher::new(),
            handy_config: HandyConfig::default(),

            vr_server: None,
            vr_server_port: 23443,
            vr_sbs_mode: false,
            vr_ipd_offset: 0.0,

            stash_config: StashConfig::default(),
            stash_scenes: Vec::new(),
            stash_connected: false,
            stash_status: None,

            batch_queue: BatchQueue::new(),
            batch_config: BatchJobConfig::default(),
            batch_scan_path: String::new(),
            batch_worker: BatchWorker::new(),

            plugin_host: PluginHost::new(),
            plugin_dialog_open: false,
            selected_plugin_idx: 0,
            plugin_param_values: HashMap::new(),

            show_spectral_infill_dialog: false,
            spectral_infill_config: SpectralInfillConfig::default(),
            spectral_use_loop_range: true,

            adaptive_profile: crate::neural::pipeline::AdaptiveProfile::GenericDefault,
            live_branch: crate::neural::router::ExecutionBranch::FastTracking,
            live_observability: crate::neural::hypothesis::ObservabilityState::Observed,
            live_uncertainty: crate::neural::router::CalibratedUncertainty::default(),

            help_dialog_open: false,
            about_dialog_open: false,
        }
    }
}

impl eframe::App for FunGenApp {
    fn update(&mut self, ctx: &EguiContext, _frame: &mut eframe::Frame) {
        // 1. Poll background messages (generation, batch worker, audio waveform, and Handy events)
        self.poll_worker();
        self.poll_batch_worker();
        self.poll_waveform();
        self.poll_vr_sync();
        self.poll_model_download();
        self.handy_dispatcher.poll_events();

        // 2. Advance playback clock if playing
        self.update_playback(ctx);

        // 3. Handle Keyboard Shortcuts
        self.handle_shortcuts(ctx);

        // 4. Update video texture frame if cursor timestamp shifted
        self.update_video_texture(ctx);

        // 5. Top Menu & Tab Navigation Bar
        TopBottomPanel::top("top_menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open Funscript... (Ctrl+O)").clicked() {
                        self.open_funscript_dialog();
                        ui.close_menu();
                    }
                    if ui.button("Save Funscript (Ctrl+S)").clicked() {
                        self.save_funscript();
                        ui.close_menu();
                    }
                    if ui.button("Save Funscript As... (Ctrl+Shift+S)").clicked() {
                        self.save_funscript_as();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Select Video...").clicked() {
                        self.select_video_dialog();
                        ui.close_menu();
                    }
                    if ui.button("Select ONNX Model...").clicked() {
                        self.select_model_dialog();
                        ui.close_menu();
                    }
                });

                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(self.undo_history.can_undo(), egui::Button::new("↶ Undo (Ctrl+Z)")).clicked() {
                        self.perform_undo();
                        ui.close_menu();
                    }
                    if ui.add_enabled(self.undo_history.can_redo(), egui::Button::new("↷ Redo (Ctrl+Y)")).clicked() {
                        self.perform_redo();
                        ui.close_menu();
                    }
                    ui.separator();
                    let has_selection = self.timeline_state.selected_index.is_some();
                    if ui.add_enabled(has_selection, egui::Button::new("🗑 Delete Selected Keyframe (Del)")).clicked() {
                        if let Some(idx) = self.timeline_state.selected_index {
                            if idx < self.script.actions.len() {
                                self.undo_history.push_snapshot(&self.script);
                                self.script.actions.remove(idx);
                                self.timeline_state.selected_index = None;
                                self.run_doctor();
                                self.set_status("Deleted selected keyframe".to_string());
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Invert Positions (0 ↔ 100)").clicked() {
                        self.undo_history.push_snapshot(&self.script);
                        self.script.invert_positions();
                        self.run_doctor();
                        self.set_status("Inverted all keyframe positions".to_string());
                        ui.close_menu();
                    }
                    ui.separator();
                    let snap_label = if self.timeline_state.magnetic_snapping {
                        "Disable Magnetic Audio Snapping (M)"
                    } else {
                        "Enable Magnetic Audio Snapping (M)"
                    };
                    if ui.button(snap_label).clicked() {
                        self.timeline_state.magnetic_snapping = !self.timeline_state.magnetic_snapping;
                        ui.close_menu();
                    }
                });

                ui.menu_button("View", |ui| {
                    if ui.button("Studio Editor (F1)").clicked() {
                        self.active_tab = HubTab::Studio;
                        ui.close_menu();
                    }
                    if ui.button("Cinema Player (F2)").clicked() {
                        self.active_tab = HubTab::Cinema;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Zoom In (Scroll Up)").clicked() {
                        self.timeline_state.view_duration_ms =
                            (self.timeline_state.view_duration_ms * 0.75).max(500.0);
                        ui.close_menu();
                    }
                    if ui.button("Zoom Out (Scroll Down)").clicked() {
                        self.timeline_state.view_duration_ms =
                            (self.timeline_state.view_duration_ms * 1.33).min(3_600_000.0);
                        ui.close_menu();
                    }
                    if ui.button("Reset Zoom (Fit Script)").clicked() {
                        self.fit_timeline();
                        ui.close_menu();
                    }
                });

                ui.menu_button("Audit", |ui| {
                    if ui.button("Run Script Doctor (F4)").clicked() {
                        self.run_doctor();
                        self.active_tab = HubTab::ScriptDoctor;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("⚡ Fix All Issues (1-Click Safe Auto-Repair)").clicked() {
                        self.undo_history.push_snapshot(&self.script);
                        let clamped = self.script.fix_speed_violations(450.0);
                        let removed_jitters = self.script.remove_micro_jitters(20, 2);
                        let initial = self.script.actions.len();
                        self.script.sanitize();
                        let deduped = initial - self.script.actions.len();
                        self.run_doctor();
                        self.set_status(format!(
                            "Safe Auto-Repair complete: clamped {} speeds, removed {} jitters, deduplicated {} points",
                            clamped, removed_jitters, deduped
                        ));
                        ui.close_menu();
                    }
                    if ui.button("Quick Fix Speed Violations (450 u/s)").clicked() {
                        self.undo_history.push_snapshot(&self.script);
                        let fixed = self.script.fix_speed_violations(450.0);
                        self.run_doctor();
                        self.set_status(format!("Clamped {} speed violations to safe limits", fixed));
                        ui.close_menu();
                    }
                    if ui.button("Remove Micro-Jitters (<20ms)").clicked() {
                        self.undo_history.push_snapshot(&self.script);
                        let removed = self.script.remove_micro_jitters(20, 2);
                        self.run_doctor();
                        self.set_status(format!("Removed {} micro-jitter keyframes", removed));
                        ui.close_menu();
                    }
                });

                ui.menu_button("Stash", |ui| {
                    if ui.button("Stash Automation Manager (F6)").clicked() {
                        self.active_tab = HubTab::Automation;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Plugins", |ui| {
                    if ui.button("Open Plugin & Macro SDK (F6)").clicked() {
                        self.active_tab = HubTab::Automation;
                        ui.close_menu();
                    }
                    if ui.button("Floating Plugin Window...").clicked() {
                        self.plugin_dialog_open = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("Keyboard & Mouse Shortcuts...").clicked() {
                        self.help_dialog_open = true;
                        ui.close_menu();
                    }
                    if ui.button("About Funscript Hub...").clicked() {
                        self.about_dialog_open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("☕ Sponsor on Buy Me a Coffee...").clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab("https://buymeacoffee.com/thesmartestgooner"));
                        ui.close_menu();
                    }
                });

                ui.separator();
                if ui.button(RichText::new("☕ Sponsor").color(Color32::from_rgb(255, 200, 50)).strong())
                    .on_hover_text("Support development on Buy Me a Coffee: https://buymeacoffee.com/thesmartestgooner")
                    .clicked()
                {
                    ctx.open_url(egui::OpenUrl::new_tab("https://buymeacoffee.com/thesmartestgooner"));
                }

                ui.separator();

                // Navigation Tabs with Function Key Hints
                ui.selectable_value(&mut self.active_tab, HubTab::Studio, "  Studio Editor (F1)  ");
                ui.selectable_value(&mut self.active_tab, HubTab::Cinema, "  Cinema Player (F2)  ");
                ui.selectable_value(&mut self.active_tab, HubTab::Generator, "  Generator (F3)  ");
                ui.selectable_value(&mut self.active_tab, HubTab::ScriptDoctor, "  Doctor & Repair (F4)  ");
                ui.selectable_value(&mut self.active_tab, HubTab::DeviceSync, "  Device & Sync (F5)  ");
                ui.selectable_value(&mut self.active_tab, HubTab::Automation, "  Automation & Plugins (F6)  ");

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.is_generating {
                        ui.spinner();
                        ui.label(RichText::new(&self.generation_status).color(Color32::from_rgb(0, 200, 255)));
                    } else if self.batch_worker.is_active() {
                        ui.spinner();
                        ui.label(RichText::new("Batch Processing Active...").color(Color32::from_rgb(0, 200, 255)));
                    } else if let Some((ref msg, instant)) = self.status_message {
                        if instant.elapsed().as_secs() < 6 {
                            ui.label(RichText::new(msg).color(Color32::from_rgb(100, 220, 150)));
                        }
                    } else if let Some(ref path) = self.script_path {
                        ui.label(
                            RichText::new(path.file_name().unwrap_or_default().to_string_lossy())
                                .color(Color32::from_rgb(180, 190, 205)),
                        );
                    }
                });
            });
        });

        // 6. Central Panel Rendered based on Active Tab
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                HubTab::Studio => self.render_studio_view(ui),
                HubTab::Cinema => self.render_cinema_view(ui),
                HubTab::Generator => self.render_generator_view(ui),
                HubTab::ScriptDoctor => self.render_doctor_view(ui),
                HubTab::DeviceSync => self.render_device_sync_view(ui),
                HubTab::Automation => self.render_automation_view(ui),
            }
        });

        // Floating dialogs & modals
        self.render_plugin_dialog(ctx);
        self.render_help_dialog(ctx);
        self.render_about_dialog(ctx);
        self.render_rig_simulator_window(ctx);
        self.render_spectral_infill_dialog(ctx);

        // Request repaint if playing, generating, or running batch worker
        if self.is_playing {
            ctx.request_repaint(); // Immediate for smooth 60 FPS playback
        } else if self.is_generating || self.batch_worker.is_active() {
            // Cap progress spinner to 30 FPS to avoid pegging CPU at uncapped FPS
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

impl FunGenApp {
    fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }

    pub fn switch_axis(&mut self, new_axis: AxisChannel) {
        if self.active_axis == new_axis {
            return;
        }
        // Commit current active script into multi_axis collection
        self.multi_axis.channels.insert(self.active_axis, self.script.clone());
        self.active_axis = new_axis;
        // Load target channel script (or blank if new)
        self.script = self.multi_axis.get_or_create(new_axis).clone();
        self.timeline_state.selected_index = None;
        self.run_doctor();
        self.set_status(format!("Switched active channel to {}", new_axis.display_name()));
    }

    pub fn toggle_playback(&mut self) {
        let next = !self.is_playing;
        self.set_playing(next);
    }

    pub fn set_playing(&mut self, playing: bool) {
        if self.is_playing == playing {
            return;
        }
        self.is_playing = playing;
        if playing {
            self.last_frame_time = Some(Instant::now());
            if let Some(ref path) = self.video_path {
                self.audio_player.play_at(path, self.timeline_state.cursor_time_ms, self.playback_speed);
            }
        } else {
            self.audio_player.pause();
        }
    }

    pub fn seek_to(&mut self, time_ms: i64) {
        self.timeline_state.cursor_time_ms = time_ms.max(0);
        if let Some(ref streamer) = self.playback_streamer {
            streamer.seek_to(self.timeline_state.cursor_time_ms);
        }
        self.audio_player.seek_to(self.timeline_state.cursor_time_ms, self.playback_speed);
    }

    pub fn render_tracking_overlay(&self, painter: &egui::Painter, video_rect: egui::Rect, cur_pos: f32) {
        use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Stroke, StrokeKind};

        let center_x = video_rect.center().x;
        // Map stroke [0, 100] to vertical motion inside a 65% height window
        let track_y = video_rect.bottom() - (cur_pos / 100.0) * (video_rect.height() * 0.65) - (video_rect.height() * 0.15);
        let track_pt = pos2(center_x, track_y);

        // 1. Central Motion Tracking Reticle & Crosshair
        let reticle_color = if cur_pos > 85.0 || cur_pos < 15.0 {
            Color32::from_rgb(255, 215, 0) // Gold at turnarounds
        } else {
            Color32::from_rgb(0, 240, 255) // Cyan in active stroke
        };

        // Outer Reticle Ring
        painter.circle_stroke(track_pt, 22.0, Stroke::new(1.2f32, Color32::from_rgba_unmultiplied(reticle_color.r(), reticle_color.g(), reticle_color.b(), 180)));
        // Inner Reticle Ring
        painter.circle_stroke(track_pt, 8.0, Stroke::new(1.5f32, reticle_color));
        // Center Target Point
        painter.circle_filled(track_pt, 3.0, Color32::WHITE);

        // Crosshairs
        painter.line_segment([pos2(track_pt.x - 32.0, track_pt.y), pos2(track_pt.x - 12.0, track_pt.y)], Stroke::new(1.2f32, reticle_color));
        painter.line_segment([pos2(track_pt.x + 12.0, track_pt.y), pos2(track_pt.x + 32.0, track_pt.y)], Stroke::new(1.2f32, reticle_color));
        painter.line_segment([pos2(track_pt.x, track_pt.y - 32.0), pos2(track_pt.x, track_pt.y - 12.0)], Stroke::new(1.2f32, reticle_color));
        painter.line_segment([pos2(track_pt.x, track_pt.y + 12.0), pos2(track_pt.x, track_pt.y + 32.0)], Stroke::new(1.2f32, reticle_color));

        // 2. Velocity / Direction Vector Arrow
        let next_pos = self.script.interpolate_position(self.timeline_state.cursor_time_ms + 60);
        let dy = (next_pos - cur_pos) * 1.8;
        if dy.abs() > 0.5 {
            let arrow_tip = pos2(track_pt.x, track_pt.y - dy * (video_rect.height() * 0.005));
            painter.line_segment([track_pt, arrow_tip], Stroke::new(2.2f32, Color32::from_rgb(255, 70, 70)));
            let head_dir = if dy > 0.0 { -1.0 } else { 1.0 };
            painter.line_segment([arrow_tip, pos2(arrow_tip.x - 5.0, arrow_tip.y + head_dir * 6.0)], Stroke::new(2.0f32, Color32::from_rgb(255, 70, 70)));
            painter.line_segment([arrow_tip, pos2(arrow_tip.x + 5.0, arrow_tip.y + head_dir * 6.0)], Stroke::new(2.0f32, Color32::from_rgb(255, 70, 70)));
        }

        // 3. Focal Analysis Region-of-Interest (ROI) Box
        let roi_w = (video_rect.width() * 0.40).min(320.0);
        let roi_h = video_rect.height() * 0.75;
        let roi_rect = Rect::from_center_size(video_rect.center(), vec2(roi_w, roi_h));
        painter.rect_stroke(roi_rect, 4.0, Stroke::new(1.0f32, Color32::from_rgba_unmultiplied(0, 200, 255, 60)), StrokeKind::Middle);
        let tick = 12.0;
        painter.line_segment([roi_rect.left_top(), pos2(roi_rect.left() + tick, roi_rect.top())], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.left_top(), pos2(roi_rect.left(), roi_rect.top() + tick)], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.right_top(), pos2(roi_rect.right() - tick, roi_rect.top())], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.right_top(), pos2(roi_rect.right(), roi_rect.top() + tick)], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.left_bottom(), pos2(roi_rect.left() + tick, roi_rect.bottom())], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.left_bottom(), pos2(roi_rect.left(), roi_rect.bottom() - tick)], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.right_bottom(), pos2(roi_rect.right() - tick, roi_rect.bottom())], Stroke::new(2.0f32, reticle_color));
        painter.line_segment([roi_rect.right_bottom(), pos2(roi_rect.right(), roi_rect.bottom() - tick)], Stroke::new(2.0f32, reticle_color));

        // 4. Live Telemetry HUD Card (Top-Right of video)
        let hud_w = 175.0;
        let hud_h = 175.0;
        let card_rect = Rect::from_min_size(pos2(video_rect.right() - hud_w - 14.0, video_rect.top() + 14.0), vec2(hud_w, hud_h));
        painter.rect_filled(card_rect, 4.0, Color32::from_rgba_unmultiplied(10, 15, 25, 210));
        painter.rect_stroke(card_rect, 4.0, Stroke::new(1.0f32, Color32::from_rgba_unmultiplied(0, 240, 255, 90)), StrokeKind::Middle);

        let positions = self.evaluate_current_positions();
        let speed_units = self.kinematic_state.instantaneous_speed;

        let mut text_y = card_rect.top() + 8.0;
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, "🎯 TRACKING TELEMETRY", FontId::monospace(10.0), Color32::from_rgb(0, 240, 255));
        text_y += 16.0;
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("PROFILE: {}", self.adaptive_profile.short_name().to_uppercase()), FontId::monospace(10.0), Color32::from_rgb(180, 220, 255));
        text_y += 15.0;
        let obs_str = match self.live_observability {
            crate::neural::hypothesis::ObservabilityState::Observed => ("OBSERVED", Color32::from_rgb(0, 255, 120)),
            crate::neural::hypothesis::ObservabilityState::Inferred => ("INFERRED", Color32::from_rgb(255, 200, 50)),
            crate::neural::hypothesis::ObservabilityState::Unresolved => ("UNRESOLVED", Color32::from_rgb(255, 80, 80)),
        };
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("STATE:   {}", obs_str.0), FontId::monospace(10.0), obs_str.1);
        text_y += 15.0;
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("UNCERT:  ±{:.3}", self.live_uncertainty.norm()), FontId::monospace(10.0), Color32::from_rgb(200, 200, 220));
        text_y += 15.0;
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("STROKE:  {:.1}%", cur_pos), FontId::monospace(11.0), Color32::WHITE);
        text_y += 15.0;
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("SPEED:   {:.0} u/s", speed_units), FontId::monospace(11.0), Color32::from_rgb(255, 200, 50));
        text_y += 15.0;
        let surge = positions.get(&AxisChannel::Surge).copied().unwrap_or(50.0);
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("SURGE:   {:.0}%", surge), FontId::monospace(10.0), Color32::from_rgb(160, 180, 205));
        text_y += 14.0;
        let sway = positions.get(&AxisChannel::Sway).copied().unwrap_or(50.0);
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("SWAY:    {:.0}%", sway), FontId::monospace(10.0), Color32::from_rgb(160, 180, 205));
        text_y += 14.0;
        let pitch = positions.get(&AxisChannel::Pitch).copied().unwrap_or(50.0);
        painter.text(pos2(card_rect.left() + 10.0, text_y), Align2::LEFT_TOP, format!("PITCH:   {:.0}%", pitch), FontId::monospace(10.0), Color32::from_rgb(160, 180, 205));

        // 5. Adaptive Fast/Slow Tracking Engine Status Badge (Top-Left of video)
        let (badge_text, badge_color) = match self.live_branch {
            crate::neural::router::ExecutionBranch::FastTracking => {
                ("⚡ FAST (TAPNext++/Flow)", Color32::from_rgb(0, 255, 120))
            }
            crate::neural::router::ExecutionBranch::SpecialistRefresh => {
                ("🧠 REFRESH (RF-DETR/YOLO)", Color32::from_rgb(0, 220, 255))
            }
            crate::neural::router::ExecutionBranch::DenseRepair => {
                ("🔍 DENSE REPAIR (CoWTracker)", Color32::from_rgb(200, 120, 255))
            }
            crate::neural::router::ExecutionBranch::SemanticReacquisition => {
                ("🌟 REACQUISITION (SAM 3.1)", Color32::from_rgb(255, 200, 50))
            }
            crate::neural::router::ExecutionBranch::UnresolvedInterval => {
                ("⚠️ UNRESOLVED INTERVAL", Color32::from_rgb(255, 80, 80))
            }
        };

        let badge_rect = Rect::from_min_size(pos2(video_rect.left() + 14.0, video_rect.top() + 14.0), vec2(205.0, 24.0));
        painter.rect_filled(badge_rect, 3.0, Color32::from_rgba_unmultiplied(10, 15, 25, 210));
        painter.rect_stroke(badge_rect, 3.0, Stroke::new(1.0f32, badge_color), StrokeKind::Middle);
        painter.text(
            badge_rect.center(),
            Align2::CENTER_CENTER,
            badge_text,
            FontId::monospace(10.0),
            badge_color,
        );
    }

    pub fn perform_undo(&mut self) {
        if self.undo_history.undo(&mut self.script) {
            self.timeline_state.selected_index = None;
            self.run_doctor();
            self.set_status("Undid last action (Ctrl+Z)".to_string());
        }
    }

    pub fn perform_redo(&mut self) {
        if self.undo_history.redo(&mut self.script) {
            self.timeline_state.selected_index = None;
            self.run_doctor();
            self.set_status("Redid action (Ctrl+Y)".to_string());
        }
    }

    pub fn delete_selected_keyframes(&mut self) {
        if self.script.actions.is_empty() {
            return;
        }
        let count = if !self.timeline_state.selected_indices.is_empty() {
            self.undo_history.push_snapshot(&self.script);
            let n = self.timeline_state.selected_indices.len();
            for &idx in self.timeline_state.selected_indices.iter().rev() {
                if idx < self.script.actions.len() {
                    self.script.actions.remove(idx);
                }
            }
            self.timeline_state.clear_selection();
            n
        } else if let Some(idx) = self.timeline_state.selected_index {
            if idx < self.script.actions.len() {
                self.undo_history.push_snapshot(&self.script);
                self.script.actions.remove(idx);
                self.timeline_state.clear_selection();
                1
            } else {
                0
            }
        } else {
            0
        };

        if count > 0 {
            self.run_doctor();
            self.set_status(format!("Deleted {} keyframe(s)", count));
        }
    }

    pub fn invert_selection(&mut self) {
        if self.script.actions.is_empty() {
            return;
        }
        self.undo_history.push_snapshot(&self.script);
        if !self.timeline_state.selected_indices.is_empty() {
            let n = self.timeline_state.selected_indices.len();
            for &idx in &self.timeline_state.selected_indices {
                if idx < self.script.actions.len() {
                    self.script.actions[idx].pos = 100 - self.script.actions[idx].pos.clamp(0, 100);
                }
            }
            self.run_doctor();
            self.set_status(format!("Inverted {} selected keyframe(s)", n));
        } else {
            self.script.invert_positions();
            self.run_doctor();
            self.set_status(format!("Inverted all {} keyframe(s)", self.script.actions.len()));
        }
    }

    pub fn scale_selection(&mut self, factor: f32) {
        if self.script.actions.is_empty() {
            return;
        }
        self.undo_history.push_snapshot(&self.script);

        let indices: Vec<usize> = if !self.timeline_state.selected_indices.is_empty() {
            self.timeline_state.selected_indices.iter().copied().collect()
        } else {
            (0..self.script.actions.len()).collect()
        };

        if indices.is_empty() {
            return;
        }

        let sum_pos: f32 = indices.iter().filter_map(|&i| self.script.actions.get(i).map(|a| a.pos as f32)).sum();
        let center = sum_pos / indices.len() as f32;

        for &i in &indices {
            if let Some(action) = self.script.actions.get_mut(i) {
                let shifted = (action.pos as f32 - center) * factor + center;
                action.pos = shifted.round().clamp(0.0, 100.0) as i32;
            }
        }

        self.run_doctor();
        self.set_status(format!("Scaled {} keyframe(s) by {:.2}x around centroid {:.0}", indices.len(), factor, center));
    }

    pub fn shift_selection_time(&mut self, delta_ms: i64) {
        if self.script.actions.is_empty() || delta_ms == 0 {
            return;
        }
        self.undo_history.push_snapshot(&self.script);

        let indices: Vec<usize> = if !self.timeline_state.selected_indices.is_empty() {
            self.timeline_state.selected_indices.iter().copied().collect()
        } else {
            (0..self.script.actions.len()).collect()
        };

        let mut target_ats = Vec::new();
        for &i in &indices {
            if let Some(action) = self.script.actions.get_mut(i) {
                action.at = (action.at + delta_ms).max(0);
                target_ats.push(action.at);
            }
        }

        self.script.sanitize();

        if !self.timeline_state.selected_indices.is_empty() {
            self.timeline_state.selected_indices = self.script
                .actions
                .iter()
                .enumerate()
                .filter_map(|(i, a)| if target_ats.contains(&a.at) { Some(i) } else { None })
                .collect();
            self.timeline_state.selected_index = self.timeline_state.selected_indices.iter().next().copied();
        }

        self.run_doctor();
        let sign = if delta_ms > 0 { "+" } else { "" };
        self.set_status(format!("Shifted {} keyframe(s) by {}{}ms", indices.len(), sign, delta_ms));
    }

    pub fn snap_selected_to_beats(&mut self) {
        if self.script.actions.is_empty() {
            return;
        }
        let waveform = match &self.audio_waveform {
            Some(wf) if !wf.is_empty() => wf,
            _ => {
                self.set_status("Beat snap: No audio track loaded or extracted".to_string());
                return;
            }
        };

        self.undo_history.push_snapshot(&self.script);

        let indices: Vec<usize> = if !self.timeline_state.selected_indices.is_empty() {
            self.timeline_state.selected_indices.iter().copied().collect()
        } else {
            (0..self.script.actions.len()).collect()
        };

        let mut snapped_count = 0;
        let mut target_ats = Vec::new();

        for &i in &indices {
            if let Some(action) = self.script.actions.get_mut(i) {
                // Search radius ±85ms for acoustic transient
                if let Some(beat_ms) = waveform.find_nearest_transient(action.at, 85) {
                    if beat_ms != action.at {
                        action.at = beat_ms;
                        snapped_count += 1;
                    }
                }
                target_ats.push(action.at);
            }
        }

        self.script.sanitize();

        if !self.timeline_state.selected_indices.is_empty() {
            self.timeline_state.selected_indices = self.script
                .actions
                .iter()
                .enumerate()
                .filter_map(|(i, a)| if target_ats.contains(&a.at) { Some(i) } else { None })
                .collect();
            self.timeline_state.selected_index = self.timeline_state.selected_indices.iter().next().copied();
        }

        self.run_doctor();
        self.set_status(format!("Snapped {} keyframe(s) to audio rhythm transients", snapped_count));
    }

    pub fn current_rig_input(&self) -> RigInput {
        let positions = self.evaluate_current_positions();
        RigInput {
            stroke: *positions.get(&AxisChannel::Stroke).unwrap_or(&50.0),
            surge: *positions.get(&AxisChannel::Surge).unwrap_or(&50.0),
            sway: *positions.get(&AxisChannel::Sway).unwrap_or(&50.0),
            twist: *positions.get(&AxisChannel::Twist).unwrap_or(&50.0),
            roll: *positions.get(&AxisChannel::Roll).unwrap_or(&50.0),
            pitch: *positions.get(&AxisChannel::Pitch).unwrap_or(&50.0),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &EguiContext) {
        // Global Undo / Redo shortcuts
        if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::Z) && !i.modifiers.shift) {
            self.perform_undo();
        }
        if ctx.input(|i| {
            let ctrl = i.modifiers.command || i.modifiers.ctrl;
            (ctrl && i.key_pressed(egui::Key::Y)) || (ctrl && i.modifiers.shift && i.key_pressed(egui::Key::Z))
        }) {
            self.perform_redo();
        }

        // Global File shortcuts (Save / Save As / Open)
        if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::S) && !i.modifiers.shift) {
            self.save_funscript();
        }
        if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::S) && i.modifiers.shift) {
            self.save_funscript_as();
        }
        if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::O)) {
            self.open_funscript_dialog();
        }

        // Select All (Ctrl+A)
        if ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::A)) {
            self.timeline_state.select_all(self.script.actions.len());
            self.set_status(format!("Selected all {} keyframes", self.timeline_state.selected_indices.len()));
        }

        // Clear Selection (Escape)
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.timeline_state.clear_selection();
            self.set_status("Cleared selection".to_string());
        }

        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.active_tab = HubTab::Studio;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.active_tab = HubTab::Cinema;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.active_tab = HubTab::Generator;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F4)) {
            self.active_tab = HubTab::ScriptDoctor;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.active_tab = HubTab::DeviceSync;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F6)) {
            self.active_tab = HubTab::Automation;
        }

        // Editing, playback, frame stepping, and axis channel hotkeys (when not typing in an input field)
        if !ctx.wants_keyboard_input() {
            // Delete / Backspace: delete selected keyframe(s)
            if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
                self.delete_selected_keyframes();
            }

            // Up / Down Arrow: nudge selected keyframe(s) position (Shift: by 5, normal: by 1)
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowDown)) {
                let is_up = ctx.input(|i| i.key_pressed(egui::Key::ArrowUp));
                let step = if ctx.input(|i| i.modifiers.shift) { 5 } else { 1 };
                let indices: Vec<usize> = if !self.timeline_state.selected_indices.is_empty() {
                    self.timeline_state.selected_indices.iter().copied().collect()
                } else if let Some(idx) = self.timeline_state.selected_index {
                    vec![idx]
                } else {
                    Vec::new()
                };

                if !indices.is_empty() {
                    self.undo_history.push_snapshot(&self.script);
                    for &i in &indices {
                        if let Some(action) = self.script.actions.get_mut(i) {
                            if is_up {
                                action.pos = (action.pos + step).min(100);
                            } else {
                                action.pos = (action.pos - step).max(0);
                            }
                        }
                    }
                    self.run_doctor();
                    let dir_str = if is_up { "Up" } else { "Down" };
                    self.set_status(format!("Nudged {} keyframe(s) {} (±{})", indices.len(), dir_str, step));
                }
            }

            // Alt+M: Snap selected keyframes to audio rhythm transients
            if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::M)) {
                self.snap_selected_to_beats();
            } else if ctx.input(|i| i.key_pressed(egui::Key::M)) {
                // M: Toggle magnetic audio snapping
                self.timeline_state.magnetic_snapping = !self.timeline_state.magnetic_snapping;
                let s = if self.timeline_state.magnetic_snapping { "enabled" } else { "disabled" };
                self.set_status(format!("Magnetic audio snapping {}", s));
            }

            // Alt+V: Cycle Audio Visualization Mode (Waveform -> Multi-Band -> Spectrogram)
            if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::V)) {
                self.timeline_state.cycle_audio_viz_mode();
                self.set_status(format!(
                    "Audio Visualization: {} {}",
                    self.timeline_state.audio_viz_mode.icon(),
                    self.timeline_state.audio_viz_mode.display_name()
                ));
            }

            // Alt+I: Open / Toggle Audio Spectral Haptic Infill Modal
            if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::I)) {
                self.show_spectral_infill_dialog = !self.show_spectral_infill_dialog;
            }

            // B: Drop Bookmark Marker
            if ctx.input(|i| i.key_pressed(egui::Key::B) && !i.modifiers.ctrl && !i.modifiers.command && !i.modifiers.alt) {
                let cur = self.timeline_state.cursor_time_ms;
                let b_idx = self.timeline_state.bookmarks.len() + 1;
                self.timeline_state.add_bookmark(cur, format!("Marker {}", b_idx));
                self.set_status(format!("Dropped bookmark pin #{} at {}ms", b_idx, cur));
            }

            // [ / ]: A/B Looper markers (Shift + [ / ]: Playback speed adjustment)
            if ctx.input(|i| i.key_pressed(egui::Key::OpenBracket)) {
                if ctx.input(|i| i.modifiers.shift) {
                    self.playback_speed = (self.playback_speed - 0.25).max(0.25);
                    if self.is_playing {
                        if let Some(ref path) = self.video_path {
                            self.audio_player.play_at(path, self.timeline_state.cursor_time_ms, self.playback_speed);
                        }
                    }
                    self.set_status(format!("Playback speed: {:.2}x", self.playback_speed));
                } else {
                    let cur = self.timeline_state.cursor_time_ms;
                    self.timeline_state.set_loop_in(cur);
                    self.set_status(format!("Set Loop In marker at {}ms", cur));
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::CloseBracket)) {
                if ctx.input(|i| i.modifiers.shift) {
                    self.playback_speed = (self.playback_speed + 0.25).min(3.0);
                    if self.is_playing {
                        if let Some(ref path) = self.video_path {
                            self.audio_player.play_at(path, self.timeline_state.cursor_time_ms, self.playback_speed);
                        }
                    }
                    self.set_status(format!("Playback speed: {:.2}x", self.playback_speed));
                } else {
                    let cur = self.timeline_state.cursor_time_ms;
                    self.timeline_state.set_loop_out(cur);
                    self.set_status(format!("Set Loop Out marker at {}ms", cur));
                }
            }

            // \: Clear A/B Section Loop
            if ctx.input(|i| i.key_pressed(egui::Key::Backslash)) {
                self.timeline_state.clear_loop();
                self.set_status("Cleared A/B section loop".to_string());
            }

            // Hotkeys 1-7: Switch active multi-axis channel
            if ctx.input(|i| i.key_pressed(egui::Key::Num1)) { self.switch_axis(AxisChannel::Stroke); }
            if ctx.input(|i| i.key_pressed(egui::Key::Num2)) { self.switch_axis(AxisChannel::Surge); }
            if ctx.input(|i| i.key_pressed(egui::Key::Num3)) { self.switch_axis(AxisChannel::Sway); }
            if ctx.input(|i| i.key_pressed(egui::Key::Num4)) { self.switch_axis(AxisChannel::Pitch); }
            if ctx.input(|i| i.key_pressed(egui::Key::Num5)) { self.switch_axis(AxisChannel::Roll); }
            if ctx.input(|i| i.key_pressed(egui::Key::Num6)) { self.switch_axis(AxisChannel::Twist); }
            if ctx.input(|i| i.key_pressed(egui::Key::Num7)) { self.switch_axis(AxisChannel::Suction); }

            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle_playback();
            }

            // Frame-accurate stepping based on video FPS (Shift: 1 second jump, Alt: Bookmark jump)
            let frame_step_ms = (1000.0 / self.target_fps.max(1.0)).round() as i64;
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                if ctx.input(|i| i.modifiers.alt) {
                    if let Some(bm_ms) = self.timeline_state.prev_bookmark(self.timeline_state.cursor_time_ms) {
                        self.seek_to(bm_ms);
                        self.set_status(format!("Jumped to bookmark at {}ms", bm_ms));
                    }
                } else {
                    let step = if ctx.input(|i| i.modifiers.shift) { 1000 } else { frame_step_ms.max(1) };
                    self.seek_to((self.timeline_state.cursor_time_ms - step).max(0));
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                if ctx.input(|i| i.modifiers.alt) {
                    if let Some(bm_ms) = self.timeline_state.next_bookmark(self.timeline_state.cursor_time_ms) {
                        self.seek_to(bm_ms);
                        self.set_status(format!("Jumped to bookmark at {}ms", bm_ms));
                    }
                } else {
                    let step = if ctx.input(|i| i.modifiers.shift) { 1000 } else { frame_step_ms.max(1) };
                    let dur = self.script.actions.last().map(|a| a.at).unwrap_or(0);
                    self.seek_to((self.timeline_state.cursor_time_ms + step).min(dur.max(step)));
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
                self.seek_to(0);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::End)) {
                let dur = self.script.actions.last().map(|a| a.at).unwrap_or(0);
                self.seek_to(dur);
            }
        }
    }

    fn ensure_video_worker(&mut self) -> Sender<VideoFrameRequest> {
        if let Some(ref tx) = self.video_req_tx {
            return tx.clone();
        }
        let (req_tx, req_rx) = channel::<VideoFrameRequest>();
        let (resp_tx, resp_rx) = channel::<VideoFrameResponse>();

        thread::spawn(move || {
            while let Ok(mut req) = req_rx.recv() {
                // Drain newer requests to avoid backlog
                while let Ok(newer) = req_rx.try_recv() {
                    req = newer;
                }
                if let Ok(raw_rgb) = crate::video::extract_frame_at(&req.path, req.time_ms, req.width, req.height) {
                    let _ = resp_tx.send(VideoFrameResponse {
                        time_ms: req.time_ms,
                        width: req.width,
                        height: req.height,
                        rgb_data: raw_rgb,
                    });
                }
            }
        });

        self.video_req_tx = Some(req_tx.clone());
        self.video_resp_rx = Some(resp_rx);
        req_tx
    }

    fn poll_video_frames(&mut self, ctx: &EguiContext) {
        if let Some(ref rx) = self.video_resp_rx {
            let mut got_frame = false;
            while let Ok(resp) = rx.try_recv() {
                let color_image = egui::ColorImage::from_rgb(
                    [resp.width as usize, resp.height as usize],
                    &resp.rgb_data,
                );
                let texture = ctx.load_texture(
                    "current_video_frame",
                    color_image,
                    egui::TextureOptions::LINEAR,
                );
                if self.video_frame_cache.len() >= 24 {
                    self.video_frame_cache.pop_front();
                }
                self.video_frame_cache.push_back((resp.time_ms, texture.clone()));
                self.video_texture = Some(texture);
                self.last_video_frame_ts = resp.time_ms;
                self.video_pending_req_ts = None;
                got_frame = true;
            }
            if got_frame {
                ctx.request_repaint();
            }
        }
    }

    fn update_video_texture(&mut self, ctx: &EguiContext) {
        self.poll_video_frames(ctx);

        let video_path_ref = match self.video_path.as_ref() {
            Some(p) => p,
            None => return,
        };

        let cur_time = self.timeline_state.cursor_time_ms;
        // Don't re-extract if timestamp hasn't changed significantly (within 20ms)
        if (cur_time - self.last_video_frame_ts).abs() < 20 && self.video_texture.is_some() {
            return;
        }

        let w = 480u32;
        let h = 270u32;

        // High-performance streaming path: query buffered background PlaybackStreamer (zero process spawns)
        if self.playback_streamer.is_none() {
            let fps = self.target_fps.max(12.0);
            self.playback_streamer = Some(crate::video::PlaybackStreamer::new(
                video_path_ref,
                w,
                h,
                fps,
                cur_time,
            ));
        }

        if let Some(ref streamer) = self.playback_streamer {
            if let Some((ts, rgb_data)) = streamer.get_frame(cur_time) {
                let color_image = egui::ColorImage::from_rgb([w as usize, h as usize], &rgb_data);
                // Reuse single persistent GPU texture handle — eliminates per-frame allocation/destruction
                if let Some(ref mut tex) = self.video_texture {
                    tex.set(color_image, egui::TextureOptions::LINEAR);
                } else {
                    self.video_texture = Some(ctx.load_texture(
                        "current_video_frame",
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.last_video_frame_ts = ts;
                self.video_pending_req_ts = None;
                ctx.request_repaint();
                return;
            }
        }

        // Avoid spamming duplicate requests if one is already pending for this timestamp
        if let Some(pending_ts) = self.video_pending_req_ts {
            if (cur_time - pending_ts).abs() < 35 {
                return;
            }
        }

        // Borrow path for fallback worker request
        let video_path = video_path_ref.clone();
        let tx = self.ensure_video_worker();
        let _ = tx.send(VideoFrameRequest {
            path: video_path,
            time_ms: cur_time,
            width: w,
            height: h,
        });
        self.video_pending_req_ts = Some(cur_time);
    }

    fn poll_waveform(&mut self) {
        if let Some(ref rx) = self.waveform_rx {
            if let Ok(wf) = rx.try_recv() {
                self.audio_waveform = Some(wf);
                self.waveform_rx = None;
                self.set_status("Audio waveform extracted and loaded".to_string());
            }
        }
    }

    fn poll_vr_sync(&mut self) {
        let msgs = if let Some(ref vr) = self.vr_server {
            vr.try_recv()
        } else {
            Vec::new()
        };

        for msg in msgs {
            match msg {
                VrSyncMessage::TimeUpdate { timestamp_ms } => {
                    self.timeline_state.cursor_time_ms = timestamp_ms;
                }
                VrSyncMessage::PlayStateChange { is_playing } => {
                    self.is_playing = is_playing;
                }
                VrSyncMessage::SpeedChange { speed } => {
                    self.playback_speed = speed.clamp(0.1, 5.0);
                }
                VrSyncMessage::VideoPath { path } => {
                    self.set_status(format!("VR Headset requested video: {}", path));
                }
            }
        }
    }

    pub fn evaluate_current_positions(&self) -> HashMap<AxisChannel, f32> {
        let cur_ms = self.timeline_state.cursor_time_ms;
        let blend_ms = self.scurve_preset.blend_radius_ms();
        let mut positions = HashMap::new();

        // 1. Evaluate active axis directly without per-frame cloning
        if !self.script.actions.is_empty() {
            let raw_pos = if blend_ms > 0 {
                eval_scurve(&self.script.actions, cur_ms, blend_ms)
            } else {
                self.script.interpolate_position(cur_ms)
            };
            let final_pos = if self.active_axis == AxisChannel::Stroke {
                self.thermal_model.attenuate_position(raw_pos)
            } else {
                raw_pos
            };
            positions.insert(self.active_axis, final_pos);
        }

        // 2. Evaluate companion channels
        for (&axis, script) in &self.multi_axis.channels {
            if axis == self.active_axis || script.actions.is_empty() {
                continue;
            }
            let raw_pos = if blend_ms > 0 {
                eval_scurve(&script.actions, cur_ms, blend_ms)
            } else {
                script.interpolate_position(cur_ms)
            };
            let final_pos = if axis == AxisChannel::Stroke {
                self.thermal_model.attenuate_position(raw_pos)
            } else {
                raw_pos
            };
            positions.insert(axis, final_pos);
        }

        if positions.is_empty() {
            positions.insert(AxisChannel::Stroke, 50.0);
        }

        positions
    }

    fn update_playback(&mut self, ctx: &EguiContext) {
        if self.is_playing {
            let now = Instant::now();
            let mut dt_secs = 0.033;
            if let Some(last) = self.last_frame_time {
                dt_secs = now.duration_since(last).as_secs_f64();
                let advance_ms = (dt_secs * 1000.0 * self.playback_speed).round() as i64;
                self.timeline_state.cursor_time_ms += advance_ms;

                // A/B Section Looping or full script loop
                let mut looped = false;
                if self.timeline_state.loop_enabled {
                    if let (Some(in_ms), Some(out_ms)) = (self.timeline_state.loop_in_ms, self.timeline_state.loop_out_ms) {
                        if out_ms > in_ms && self.timeline_state.cursor_time_ms >= out_ms {
                            self.timeline_state.cursor_time_ms = in_ms;
                            self.audio_player.seek_to(in_ms, self.playback_speed);
                            looped = true;
                        }
                    }
                }

                if !looped {
                    let duration_ms = self.script.actions.last().map(|a| a.at).unwrap_or(0);
                    if duration_ms > 0 && self.timeline_state.cursor_time_ms > duration_ms {
                        self.timeline_state.cursor_time_ms = 0; // Loop playback
                        if let Some(ref path) = self.video_path {
                            self.audio_player.play_at(path, 0, self.playback_speed);
                        }
                    }
                }

                // Auto-scroll timeline view if cursor passes the visible window edge
                let view_end = self.timeline_state.view_start_ms + self.timeline_state.view_duration_ms;
                if (self.timeline_state.cursor_time_ms as f64) > view_end - self.timeline_state.view_duration_ms * 0.15 {
                    self.timeline_state.view_start_ms = (self.timeline_state.cursor_time_ms as f64)
                        - self.timeline_state.view_duration_ms * 0.15;
                }
            }
            self.last_frame_time = Some(now);

            // Hardware Kinematics & T-Code Streaming
            let positions = self.evaluate_current_positions();
            let limits = &self.device_profile.limits;
            self.kinematic_state.update(&positions, self.timeline_state.cursor_time_ms, limits);
            self.thermal_model.step(dt_secs as f32, self.kinematic_state.instantaneous_speed, self.kinematic_state.instantaneous_accel);

            let interval_ms = (dt_secs * 1000.0).round().max(10.0) as u32;
            self.last_tcode_string = format_tcode_v03(&positions, Some(interval_ms));
            if self.sync_udp_enabled {
                self.tcode_dispatcher.send(&self.last_tcode_string);
            }
            if self.serial_dispatcher.is_connected {
                let _ = self.serial_dispatcher.send_tcode(&self.last_tcode_string);
            }

            // The Handy Hardware Streaming
            if self.handy_config.is_enabled {
                let stroke_pos = *positions.get(&AxisChannel::Stroke).unwrap_or(&50.0);
                self.handy_dispatcher.stream_position(
                    &self.handy_config.connection_key,
                    stroke_pos,
                    &self.handy_config,
                    interval_ms.max(50),
                );
            }

            ctx.request_repaint();
        } else {
            self.last_frame_time = None;
        }
    }

    fn poll_model_download(&mut self) {
        if let Some(ref rx) = self.model_rx {
            if let Ok(res) = rx.try_recv() {
                self.model_download_in_progress = false;
                match res {
                    Ok(path) => {
                        let info = inspect_model(&path);
                        self.detected_model_info = info;
                        self.model_path = Some(path.clone());
                        self.model_download_status = Some(format!("Model installed: {}", path.display()));
                        self.set_status(format!("Installed AI model to {}", path.display()));
                    }
                    Err(e) => {
                        self.model_download_status = Some(format!("Download failed: {}", e));
                        self.set_status(format!("Model download error: {}", e));
                    }
                }
                self.model_rx = None;
            }
        }
    }

    fn poll_worker(&mut self) {
        let mut messages = Vec::new();
        if let Some(ref rx) = self.worker_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }

        for msg in messages {
            match msg {
                WorkerMessage::Progress { percent, status } => {
                    self.generation_progress = percent;
                    self.generation_status = status;
                }
                WorkerMessage::Done(result) => {
                    self.is_generating = false;
                    self.worker_rx = None;
                    match result {
                        Ok(bundle) => {
                            if let Some(stroke) = bundle.channels.get(&AxisChannel::Stroke) {
                                self.script = stroke.clone();
                            }
                            self.multi_axis = bundle;
                            self.active_axis = AxisChannel::Stroke;
                            self.run_doctor();
                            self.fit_timeline();
                            let count = self.multi_axis.channels.len();
                            self.generation_status = format!("Generation Complete! ({} axes generated)", count);
                            self.set_status(format!("Generated {} axes! Loaded into Studio Editor.", count));
                            self.active_tab = HubTab::Studio;
                        }
                        Err(e) => {
                            self.generation_status = format!("Error: {e}");
                        }
                    }
                }
            }
        }
    }

    fn poll_batch_worker(&mut self) {
        let mut updates = Vec::new();
        while let Ok(msg) = self.batch_worker.rx.try_recv() {
            updates.push(msg);
        }

        for update in updates {
            match update {
                BatchProgressUpdate::JobStarted { id } => {
                    if let Some(j) = self.batch_queue.jobs.iter_mut().find(|j| j.id == id) {
                        j.status = BatchJobStatus::Processing { progress: 0.0, current_frame: 0 };
                    }
                }
                BatchProgressUpdate::JobProgress { id, progress, frame } => {
                    if let Some(j) = self.batch_queue.jobs.iter_mut().find(|j| j.id == id) {
                        j.status = BatchJobStatus::Processing { progress, current_frame: frame };
                    }
                }
                BatchProgressUpdate::JobCompleted { id, actions_count, elapsed_secs } => {
                    if let Some(j) = self.batch_queue.jobs.iter_mut().find(|j| j.id == id) {
                        j.status = BatchJobStatus::Completed { actions_count, elapsed_secs };
                    }
                }
                BatchProgressUpdate::JobFailed { id, error } => {
                    if let Some(j) = self.batch_queue.jobs.iter_mut().find(|j| j.id == id) {
                        j.status = BatchJobStatus::Failed { error };
                    }
                }
                BatchProgressUpdate::AllJobsCompleted => {
                    self.set_status("Batch Queue execution complete!".to_string());
                }
            }
        }
    }

    // =========================================================================
    // View 1: Studio Editor (F1)
    // =========================================================================
    fn render_studio_view(&mut self, ui: &mut Ui) {
        let duration_ms = self.script.actions.last().map(|a| a.at).unwrap_or(0);

        // Top Section: Embedded Synchronized Video + Inspector Sidebar
        let top_height = 240.0;
        ui.allocate_ui(egui::vec2(ui.available_width(), top_height), |ui| {
            ui.horizontal(|ui| {
                // Embedded Video Canvas
                let video_w = 426.0;
                let video_h = 240.0;
                ui.group(|ui| {
                    ui.set_width(video_w);
                    ui.set_height(video_h);

                    if let Some(ref tex) = self.video_texture {
                        ui.image((tex.id(), egui::vec2(video_w - 8.0, video_h - 26.0)));
                    } else if let Some(ref p) = self.video_path {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.label(RichText::new(p.file_name().unwrap_or_default().to_string_lossy()).strong());
                            ui.label(RichText::new("Loading video frames...").size(11.0).color(Color32::GRAY));
                        });
                    } else {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.label(RichText::new("No Video Loaded").color(Color32::GRAY));
                            if ui.button("Select Video...").clicked() {
                                self.select_video_dialog();
                            }
                        });
                    }

                    // Video caption overlay
                    ui.horizontal(|ui| {
                        let cur_time_str = format_ms(self.timeline_state.cursor_time_ms);
                        ui.monospace(RichText::new(format!("Frame Time: {cur_time_str}")).size(10.0));
                    });
                });

                ui.separator();

                // Keyframe Inspector & Quick Health
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 8.0);
                    self.render_selected_action_editor(ui);
                    ui.separator();
                    self.render_quick_doctor_summary(ui);
                });
            });
        });

        ui.add_space(4.0);
        ui.separator();

        // Bottom Section: Playback Controller + Timeline Canvas & Audio Waveform
        ui.horizontal(|ui| {
            let play_label = if self.is_playing { "⏸ Pause (Space)" } else { "▶ Play (Space)" };
            if ui.button(play_label).clicked() {
                self.toggle_playback();
            }

            let frame_step_ms = (1000.0 / self.target_fps.max(1.0)).round() as i64;
            if ui.button("⏮ Start").clicked() {
                self.seek_to(0);
            }
            if ui.button("◀ 1F").clicked() {
                self.seek_to((self.timeline_state.cursor_time_ms - frame_step_ms).max(0));
            }
            if ui.button("1F ▶").clicked() {
                self.seek_to((self.timeline_state.cursor_time_ms + frame_step_ms).min(duration_ms.max(frame_step_ms)));
            }
            if ui.button("⏭ End").clicked() {
                self.seek_to(duration_ms);
            }

            ui.separator();
            if ui.add_enabled(self.undo_history.can_undo(), egui::Button::new("↶ Undo")).clicked() {
                self.perform_undo();
            }
            if ui.add_enabled(self.undo_history.can_redo(), egui::Button::new("↷ Redo")).clicked() {
                self.perform_redo();
            }

            ui.separator();

            // Playback speed selector
            ui.label("Speed:");
            let old_speed = self.playback_speed;
            ui.selectable_value(&mut self.playback_speed, 0.5, "0.5x");
            ui.selectable_value(&mut self.playback_speed, 1.0, "1.0x");
            ui.selectable_value(&mut self.playback_speed, 1.5, "1.5x");
            ui.selectable_value(&mut self.playback_speed, 2.0, "2.0x");
            if (self.playback_speed - old_speed).abs() > 0.01 && self.is_playing {
                if let Some(ref path) = self.video_path {
                    self.audio_player.play_at(path, self.timeline_state.cursor_time_ms, self.playback_speed);
                }
            }

            // Audio Mute & Volume Controls
            ui.separator();
            let mute_icon = if self.audio_player.is_muted { "🔇" } else { "🔊" };
            if ui.button(mute_icon).on_hover_text("Toggle Audio Mute").clicked() {
                let new_muted = !self.audio_player.is_muted;
                self.audio_player.set_muted(new_muted, self.playback_speed);
            }
            let mut vol = self.audio_player.volume;
            if ui.add_sized(egui::vec2(55.0, 16.0), Slider::new(&mut vol, 0.0..=1.0).show_value(false)).changed() {
                self.audio_player.set_volume(vol, self.playback_speed);
            }

            ui.separator();

            // Formatted time display
            let cur_time_str = format_ms(self.timeline_state.cursor_time_ms);
            let dur_time_str = format_ms(duration_ms);
            ui.monospace(format!("{cur_time_str} / {dur_time_str}"));

            // Current interpolated position
            let cur_pos = self.script.interpolate_position(self.timeline_state.cursor_time_ms);
            ui.colored_label(Color32::from_rgb(0, 200, 255), format!("Pos: {cur_pos:.0}%"));

            ui.separator();

            // Zoom presets
            if ui.button("Fit All").clicked() {
                self.fit_timeline();
            }
            if ui.button("10s").clicked() {
                self.timeline_state.view_duration_ms = 10_000.0;
            }
            if ui.button("30s").clicked() {
                self.timeline_state.view_duration_ms = 30_000.0;
            }
        });

        // Time scrubber slider
        if duration_ms > 0 {
            let mut scrub_time = self.timeline_state.cursor_time_ms;
            if ui.add(
                Slider::new(&mut scrub_time, 0..=duration_ms)
                    .show_value(false)
                    .trailing_fill(true),
            ).changed() {
                self.seek_to(scrub_time);
            }
        }

        // Multi-Axis Channel Ribbon & Snapping Controls
        ui.horizontal(|ui| {
            ui.label(RichText::new("Axis (1-7):").size(11.0).strong());
            for axis in AxisChannel::ALL {
                let is_active = axis == self.active_axis;
                let count = if is_active {
                    self.script.actions.len()
                } else {
                    self.multi_axis.get(axis).map(|s| s.actions.len()).unwrap_or(0)
                };
                let (r, g, b) = axis.color_rgb();
                let color = Color32::from_rgb(r, g, b);

                let label_text = format!("{} ({})", axis.tcode_axis(), count);
                let btn = if is_active {
                    egui::Button::new(RichText::new(format!("● {label_text}")).color(color).strong())
                } else {
                    egui::Button::new(RichText::new(format!("○ {label_text}")).color(Color32::from_rgb(140, 150, 165)))
                };

                if ui.add(btn).clicked() {
                    self.switch_axis(axis);
                }
            }

            ui.separator();
            ui.checkbox(&mut self.show_ghost_axes, "Ghost Curves");
            ui.checkbox(&mut self.timeline_state.magnetic_snapping, "🧲 Audio Snap (±35ms)");

            ui.separator();
            if ui.button("🌊 Infill Generator...").clicked() {
                self.infill_start_ms = self.timeline_state.view_start_ms as i64;
                self.infill_end_ms = (self.timeline_state.view_start_ms + self.timeline_state.view_duration_ms) as i64;
                self.infill_dialog_open = true;
            }
            if ui.button("🧩 Plugin Macros...").clicked() {
                self.plugin_dialog_open = true;
            }
        });

        // Procedural Waveform Infill Generator Modal
        if self.infill_dialog_open {
            let mut open = self.infill_dialog_open;
            let mut do_generate = false;

            egui::Window::new("Procedural Waveform Infill Generator")
                .open(&mut open)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.heading(format!("Target Axis: {}", self.active_axis.display_name()));
                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("Pattern Shape:");
                        ui.selectable_value(&mut self.infill_pattern, InfillPattern::Sine, "Sine");
                        ui.selectable_value(&mut self.infill_pattern, InfillPattern::Triangle, "Triangle");
                        ui.selectable_value(&mut self.infill_pattern, InfillPattern::Sawtooth, "Sawtooth");
                    });

                    ui.horizontal(|ui| {
                        ui.label("Frequency (Hz):");
                        ui.add(Slider::new(&mut self.infill_freq_hz, 0.2..=10.0).suffix(" Hz"));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Min Pos:");
                        ui.add(Slider::new(&mut self.infill_min_pos, 0..=100));
                        ui.label("Max Pos:");
                        ui.add(Slider::new(&mut self.infill_max_pos, 0..=100));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Start (ms):");
                        ui.add(egui::DragValue::new(&mut self.infill_start_ms).speed(100.0));
                        ui.label("End (ms):");
                        ui.add(egui::DragValue::new(&mut self.infill_end_ms).speed(100.0));
                    });

                    ui.horizontal(|ui| {
                        if ui.button("Viewport Window").clicked() {
                            self.infill_start_ms = self.timeline_state.view_start_ms as i64;
                            self.infill_end_ms = (self.timeline_state.view_start_ms + self.timeline_state.view_duration_ms) as i64;
                        }
                        if ui.button("Cursor + 5s").clicked() {
                            self.infill_start_ms = self.timeline_state.cursor_time_ms;
                            self.infill_end_ms = self.timeline_state.cursor_time_ms + 5000;
                        }
                    });

                    ui.separator();
                    if ui.button(RichText::new("⚡ Generate Infill Waveform").strong()).clicked() {
                        do_generate = true;
                    }
                });

            self.infill_dialog_open = open;

            if do_generate {
                self.undo_history.push_snapshot(&self.script);
                self.script.infill_pattern(
                    self.infill_start_ms,
                    self.infill_end_ms,
                    self.infill_min_pos,
                    self.infill_max_pos,
                    self.infill_freq_hz,
                    self.infill_pattern,
                );
                self.run_doctor();
                self.set_status(format!(
                    "Generated {:?} infill on {} ({}ms - {}ms)",
                    self.infill_pattern,
                    self.active_axis.display_name(),
                    self.infill_start_ms,
                    self.infill_end_ms
                ));
                self.infill_dialog_open = false;
            }
        }

        ui.add_space(2.0);

        // Studio Selection & Transform Ribbon
        ui.horizontal(|ui| {
            let sel_count = if !self.timeline_state.selected_indices.is_empty() {
                self.timeline_state.selected_indices.len()
            } else if self.timeline_state.selected_index.is_some() {
                1
            } else {
                0
            };

            ui.label(
                RichText::new(format!("Selection: {} pts", sel_count))
                    .strong()
                    .color(if sel_count > 0 { Color32::from_rgb(255, 220, 50) } else { Color32::from_rgb(140, 150, 165) }),
            );

            if ui.button("Select All (Ctrl+A)").clicked() {
                self.timeline_state.select_all(self.script.actions.len());
                self.set_status(format!("Selected all {} keyframes", self.timeline_state.selected_indices.len()));
            }

            if sel_count > 0 && ui.button("Clear (Esc)").clicked() {
                self.timeline_state.clear_selection();
            }

            ui.separator();

            if ui.button("Invert ↕").on_hover_text("Invert positions (pos -> 100 - pos) of selection or all").clicked() {
                self.invert_selection();
            }

            if ui.button("0.8x Scale").on_hover_text("Scale range down by 0.8x around centroid").clicked() {
                self.scale_selection(0.8);
            }

            if ui.button("1.2x Scale").on_hover_text("Scale range up by 1.2x around centroid").clicked() {
                self.scale_selection(1.2);
            }

            ui.separator();

            if ui.button("⏪ -100ms").on_hover_text("Shift selected keyframes earlier by 100ms").clicked() {
                self.shift_selection_time(-100);
            }

            if ui.button("+100ms ⏩").on_hover_text("Shift selected keyframes later by 100ms").clicked() {
                self.shift_selection_time(100);
            }

            if sel_count > 0 {
                ui.separator();
                if ui.button(RichText::new("🗑 Delete (Del)").color(Color32::from_rgb(255, 100, 100))).clicked() {
                    self.delete_selected_keyframes();
                }
            }

            ui.separator();

            if ui.button("🎵 Snap Beat (Alt+M)")
                .on_hover_text("Snap selected keyframe timestamps directly to nearest acoustic beat transients")
                .clicked()
            {
                self.snap_selected_to_beats();
            }

            ui.separator();

            let in_lbl = if let Some(t) = self.timeline_state.loop_in_ms {
                format!("⟪ In: {}ms ([)", t)
            } else {
                "⟪ Set In ([)".to_string()
            };
            if ui.button(in_lbl).on_hover_text("Set A/B Loop In point at current playhead").clicked() {
                let cur = self.timeline_state.cursor_time_ms;
                self.timeline_state.set_loop_in(cur);
                self.set_status(format!("Set Loop In marker at {}ms", cur));
            }

            let out_lbl = if let Some(t) = self.timeline_state.loop_out_ms {
                format!("Out: {}ms (]) ⟫", t)
            } else {
                "Set Out (]) ⟫".to_string()
            };
            if ui.button(out_lbl).on_hover_text("Set A/B Loop Out point at current playhead").clicked() {
                let cur = self.timeline_state.cursor_time_ms;
                self.timeline_state.set_loop_out(cur);
                self.set_status(format!("Set Loop Out marker at {}ms", cur));
            }

            if self.timeline_state.loop_enabled {
                if ui.button(RichText::new("✕ Clear Loop (\\)").color(Color32::from_rgb(0, 200, 255))).clicked() {
                    self.timeline_state.clear_loop();
                    self.set_status("Cleared A/B loop".to_string());
                }
            }

            if ui.button("🔖 Bookmark (B)").on_hover_text("Drop a bookmark marker pin at current playhead (Alt+Left / Alt+Right to jump)").clicked() {
                let cur = self.timeline_state.cursor_time_ms;
                let b_idx = self.timeline_state.bookmarks.len() + 1;
                self.timeline_state.add_bookmark(cur, format!("Marker {}", b_idx));
                self.set_status(format!("Added bookmark marker #{} at {}ms", b_idx, cur));
            }

            ui.separator();

            let rig_text = if self.show_rig_simulator { "🤖 3D Rig: ON" } else { "🤖 3D Rig: OFF" };
            if ui.selectable_label(self.show_rig_simulator, rig_text)
                .on_hover_text("Toggle interactive 3D OSR2 / SR6 robot rig simulator")
                .clicked()
            {
                self.show_rig_simulator = !self.show_rig_simulator;
            }

            ui.separator();

            let viz_lbl = format!(
                "{} {} (Alt+V)",
                self.timeline_state.audio_viz_mode.icon(),
                self.timeline_state.audio_viz_mode.display_name()
            );
            if ui.button(viz_lbl)
                .on_hover_text("Cycle Audio Visualization: Waveform (RMS), Multi-Band Split, or Waterfall Spectrogram (Alt+V)")
                .clicked()
            {
                self.timeline_state.cycle_audio_viz_mode();
                self.set_status(format!(
                    "Audio Visualization: {} {}",
                    self.timeline_state.audio_viz_mode.icon(),
                    self.timeline_state.audio_viz_mode.display_name()
                ));
            }

            if ui.button(RichText::new("⚡ Spectral Infill (Alt+I)").color(Color32::from_rgb(255, 215, 0)))
                .on_hover_text("Audio-reactive multi-band frequency infill & companion generation (Alt+I)")
                .clicked()
            {
                self.show_spectral_infill_dialog = true;
            }
        });

        // Timeline Canvas with Audio Waveform and Ghost Axis Overlays
        ui.label(
            RichText::new("Timeline Curve & Audio Waveform | Left Drag: Keyframe | Shift+Drag: Marquee Box | Double-Click: Add | Scroll: Zoom | Middle/Ctrl+Drag: Pan | Arrows: Step Frame")
                .size(11.0)
                .color(Color32::from_rgb(130, 140, 155)),
        );

        let ghost_data: Vec<(&str, &Funscript, Color32)> = if self.show_ghost_axes {
            AxisChannel::ALL
                .iter()
                .filter(|&&a| a != self.active_axis)
                .filter_map(|&a| {
                    self.multi_axis.get(a).and_then(|s| {
                        if !s.actions.is_empty() {
                            let (r, g, b) = a.color_rgb();
                            Some((a.display_name(), s, Color32::from_rgb(r, g, b)))
                        } else {
                            None
                        }
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        let (ar, ag, ab) = self.active_axis.color_rgb();
        let active_color = Color32::from_rgb(ar, ag, ab);

        show_timeline(
            ui,
            &mut self.script,
            &mut self.timeline_state,
            self.doctor_report.as_ref(),
            self.audio_waveform.as_ref(),
            if ghost_data.is_empty() { None } else { Some(&ghost_data) },
            active_color,
            Some(&mut self.undo_history),
        );
    }

    // =========================================================================
    // View 2: Cinema Player (F2)
    // =========================================================================
    fn render_cinema_view(&mut self, ui: &mut Ui) {
        let avail_rect = ui.available_rect_before_wrap();
        let cur_pos = self.script.interpolate_position(self.timeline_state.cursor_time_ms);

        // 1. Full-Window Video Canvas (Mono or VR 180 SBS Stereo)
        if let Some(ref tex) = self.video_texture {
            let img_h = (avail_rect.height() - 70.0).max(200.0);

            if self.vr_sbs_mode {
                // VR 180 SBS Stereoscopic Mode: Dual Viewports Side-by-Side
                let half_w = (avail_rect.width() / 2.0) - 6.0;
                let top_y = avail_rect.top() + 4.0;

                // Left Eye Viewport
                let left_rect = egui::Rect::from_min_size(
                    egui::pos2(avail_rect.left() + 2.0, top_y),
                    egui::vec2(half_w, img_h),
                );
                let left_uv_min = egui::pos2((0.0 + self.vr_ipd_offset).clamp(0.0, 0.4), 0.0);
                let left_uv_max = egui::pos2((0.5 + self.vr_ipd_offset).clamp(0.1, 0.5), 1.0);
                ui.painter().image(
                    tex.id(),
                    left_rect,
                    egui::Rect::from_min_max(left_uv_min, left_uv_max),
                    Color32::WHITE,
                );

                // Right Eye Viewport
                let right_rect = egui::Rect::from_min_size(
                    egui::pos2(avail_rect.left() + half_w + 10.0, top_y),
                    egui::vec2(half_w, img_h),
                );
                let right_uv_min = egui::pos2((0.5 - self.vr_ipd_offset).clamp(0.5, 0.9), 0.0);
                let right_uv_max = egui::pos2((1.0 - self.vr_ipd_offset).clamp(0.6, 1.0), 1.0);
                ui.painter().image(
                    tex.id(),
                    right_rect,
                    egui::Rect::from_min_max(right_uv_min, right_uv_max),
                    Color32::WHITE,
                );

                // Eye labels
                ui.painter().text(
                    left_rect.left_top() + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    "LEFT EYE",
                    egui::FontId::monospace(12.0),
                    Color32::from_rgba_unmultiplied(200, 220, 255, 180),
                );
                ui.painter().text(
                    right_rect.left_top() + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    "RIGHT EYE",
                    egui::FontId::monospace(12.0),
                    Color32::from_rgba_unmultiplied(200, 220, 255, 180),
                );
            } else {
                // Standard Cinema Monoscopic Canvas
                let img_w = (img_h * (16.0 / 9.0)).min(avail_rect.width());
                let center_x = avail_rect.center().x;
                let video_rect = egui::Rect::from_center_size(
                    egui::pos2(center_x, avail_rect.top() + img_h / 2.0),
                    egui::vec2(img_w, img_h),
                );
                ui.painter().image(
                    tex.id(),
                    video_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );

                if self.show_tracking_overlay {
                    self.render_tracking_overlay(ui.painter(), video_rect, cur_pos);
                }
            }
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(avail_rect.height() * 0.35);
                ui.heading("Cinema Player Mode");
                ui.label("No video loaded for playback.");
                if ui.button("Select Video...").clicked() {
                    self.select_video_dialog();
                }
            });
        }

        // 2. Floating Bottom HUD (Auto-Hiding Controls & Live Stroke Position)
        let hud_rect = egui::Rect::from_min_max(
            egui::pos2(avail_rect.left(), avail_rect.bottom() - 65.0),
            avail_rect.max,
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(hud_rect), |ui| {
            ui.painter().rect_filled(ui.available_rect_before_wrap(), 4.0, Color32::from_rgba_unmultiplied(18, 20, 26, 230));
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                let play_label = if self.is_playing { "⏸ Pause" } else { "▶ Play" };
                if ui.button(play_label).clicked() {
                    self.toggle_playback();
                }
                if ui.button("⏮").clicked() {
                    self.seek_to(0);
                }

                let cur_time_str = format_ms(self.timeline_state.cursor_time_ms);
                let dur_ms = self.script.actions.last().map(|a| a.at).unwrap_or(0);
                let dur_str = format_ms(dur_ms);
                ui.monospace(format!("{cur_time_str} / {dur_str}"));

                // Playback speed selector
                ui.separator();
                let old_speed = self.playback_speed;
                ui.selectable_value(&mut self.playback_speed, 0.5, "0.5x");
                ui.selectable_value(&mut self.playback_speed, 1.0, "1.0x");
                ui.selectable_value(&mut self.playback_speed, 1.5, "1.5x");
                ui.selectable_value(&mut self.playback_speed, 2.0, "2.0x");
                if (self.playback_speed - old_speed).abs() > 0.01 && self.is_playing {
                    if let Some(ref path) = self.video_path {
                        self.audio_player.play_at(path, self.timeline_state.cursor_time_ms, self.playback_speed);
                    }
                }

                // Audio Mute & Volume Controls
                ui.separator();
                let mute_icon = if self.audio_player.is_muted { "🔇" } else { "🔊" };
                if ui.button(mute_icon).on_hover_text("Toggle Audio Mute").clicked() {
                    let new_muted = !self.audio_player.is_muted;
                    self.audio_player.set_muted(new_muted, self.playback_speed);
                }
                let mut vol = self.audio_player.volume;
                if ui.add_sized(egui::vec2(60.0, 16.0), Slider::new(&mut vol, 0.0..=1.0).show_value(false)).changed() {
                    self.audio_player.set_volume(vol, self.playback_speed);
                }

                // Tracking HUD & 3D Rig Toggles
                ui.separator();
                ui.checkbox(&mut self.show_tracking_overlay, "🎯 Tracking HUD");
                ui.checkbox(&mut self.show_rig_simulator, "🤖 3D Rig");
                if ui.button(format!("{} Viz", self.timeline_state.audio_viz_mode.icon()))
                    .on_hover_text("Cycle Audio Visualization: Waveform, Multi-Band, or Spectrogram (Alt+V)")
                    .clicked()
                {
                    self.timeline_state.cycle_audio_viz_mode();
                }

                // Live Haptic Gauge Bar
                ui.separator();
                ui.label(RichText::new("Haptic Stroke:").size(11.0));
                let (gauge_rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(gauge_rect, 2.0, Color32::from_rgb(30, 35, 45));
                let fill_w = (cur_pos / 100.0) * gauge_rect.width();
                let fill_rect = egui::Rect::from_min_max(gauge_rect.min, egui::pos2(gauge_rect.left() + fill_w, gauge_rect.bottom()));
                ui.painter().rect_filled(fill_rect, 2.0, Color32::from_rgb(0, 200, 255));
                ui.strong(format!("{:.0}%", cur_pos));

                // VR SBS Controls
                if self.vr_sbs_mode {
                    ui.separator();
                    ui.label("IPD:");
                    ui.add(Slider::new(&mut self.vr_ipd_offset, -0.05..=0.05).step_by(0.005));
                }

                // Telemetry & Studio button
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Exit to Studio (F1)").clicked() {
                        self.active_tab = HubTab::Studio;
                    }
                    ui.monospace(format!("T-Code: L0{:03}", (cur_pos * 10.0).round() as i32));
                });
            });

            // Scrubber slider
            let duration_ms = self.script.actions.last().map(|a| a.at).unwrap_or(0);
            if duration_ms > 0 {
                let mut scrub_time = self.timeline_state.cursor_time_ms;
                if ui.add(
                    Slider::new(&mut scrub_time, 0..=duration_ms)
                        .show_value(false)
                        .trailing_fill(true),
                ).changed() {
                    self.seek_to(scrub_time);
                }
            }
        });
    }

    fn render_selected_action_editor(&mut self, ui: &mut Ui) {
        ui.heading("Keyframe & Channel Inspector");
        if let Some(idx) = self.timeline_state.selected_index {
            if idx < self.script.actions.len() {
                let action_at = self.script.actions[idx].at;
                let mut jump_clicked = false;
                let mut delete_clicked = false;

                ui.horizontal(|ui| {
                    ui.label("Time (ms):");
                    ui.add(egui::DragValue::new(&mut self.script.actions[idx].at).speed(10.0));
                });

                ui.horizontal(|ui| {
                    ui.label("Position:");
                    ui.add(egui::Slider::new(&mut self.script.actions[idx].pos, 0..=100));
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Jump Playhead").clicked() {
                        jump_clicked = true;
                    }
                    if ui.button("🗑 Delete Point").clicked() {
                        delete_clicked = true;
                    }
                });

                if jump_clicked {
                    self.timeline_state.cursor_time_ms = action_at;
                }
                if delete_clicked {
                    self.undo_history.push_snapshot(&self.script);
                    self.script.actions.remove(idx);
                    self.timeline_state.selected_index = None;
                    self.run_doctor();
                    self.set_status("Deleted keyframe".to_string());
                }
            }
        } else {
            ui.label(RichText::new("Click a keyframe node on the timeline to edit.").color(Color32::GRAY));
        }

        ui.add_space(4.0);
        ui.separator();
        ui.label(RichText::new("Channel Transformations:").size(11.0).strong());
        ui.horizontal(|ui| {
            if ui.button("Invert (100-P)").clicked() {
                self.undo_history.push_snapshot(&self.script);
                self.script.invert_positions();
                self.run_doctor();
                self.set_status("Inverted channel positions".to_string());
            }
            if ui.button("Scale 1.1x").clicked() {
                self.undo_history.push_snapshot(&self.script);
                self.script.scale_amplitude(1.1, 50);
                self.run_doctor();
                self.set_status("Scaled amplitude up 10%".to_string());
            }
            if ui.button("Scale 0.9x").clicked() {
                self.undo_history.push_snapshot(&self.script);
                self.script.scale_amplitude(0.9, 50);
                self.run_doctor();
                self.set_status("Scaled amplitude down 10%".to_string());
            }
        });
    }

    fn render_quick_doctor_summary(&mut self, ui: &mut Ui) {
        ui.heading("Doctor Health");
        if let Some(ref rep) = self.doctor_report {
            let (status_color, status_text) = if rep.speed_violations_count == 0 && rep.timing_inversions_count == 0 {
                (Color32::from_rgb(80, 220, 120), "PERFECT")
            } else if rep.speed_violations_count > 0 {
                (Color32::from_rgb(255, 80, 80), "SPEED VIOLATIONS")
            } else {
                (Color32::from_rgb(255, 200, 60), "WARNINGS DETECTED")
            };

            ui.colored_label(status_color, format!("Status: {status_text}"));
            ui.label(format!("Actions: {}", rep.total_actions));
            ui.label(format!("Max Speed: {:.0} u/s", rep.max_speed_units_per_sec));
            ui.label(format!("Violations: {}", rep.speed_violations_count));

            ui.add_space(4.0);
            if ui.button("Open Full Doctor & Repair (F4)").clicked() {
                self.active_tab = HubTab::ScriptDoctor;
            }
        }
    }

    // =========================================================================
    // View 3: Generator Studio (F3)
    // =========================================================================
    fn render_generator_view(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Funscript Generator Studio");
            ui.label(
                RichText::new("Generate hardware-ready funscripts from video using AI Neural Anatomical Tracking or Optical Flow.")
                    .color(Color32::from_rgb(160, 170, 185)),
            );
            ui.add_space(8.0);

            // Mode Selector
            ui.group(|ui| {
                ui.heading("1. Tracking Engine");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.use_neural, true, "AI Neural YOLOv12 Tracking (Recommended)");
                    ui.radio_value(&mut self.use_neural, false, "Dense Optical Flow (Classical)");
                });
                if self.use_neural {
                    ui.label(
                        RichText::new("Uses Ultralytics YOLOv12 via ONNX Runtime to detect 10 anatomical classes and calculate penetration depth with hybrid flow fallback.")
                            .size(11.0)
                            .color(Color32::GRAY),
                    );
                } else {
                    ui.label(
                        RichText::new("Uses Rayon multi-threaded Lucas-Kanade dense optical flow and 2D velocity divergence to track motion centers.")
                            .size(11.0)
                            .color(Color32::GRAY),
                    );
                }

                ui.add_space(6.0);
                ui.label(RichText::new("Adaptive Fast/Slow Operating Profile:").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.adaptive_profile, crate::neural::pipeline::AdaptiveProfile::Economy, "⚡ Economy");
                    ui.selectable_value(&mut self.adaptive_profile, crate::neural::pipeline::AdaptiveProfile::BalancedSpecialist, "🎯 Specialist");
                    ui.selectable_value(&mut self.adaptive_profile, crate::neural::pipeline::AdaptiveProfile::GenericDefault, "🌟 Default");
                    ui.selectable_value(&mut self.adaptive_profile, crate::neural::pipeline::AdaptiveProfile::DenseOffline, "🔍 Dense");
                    ui.selectable_value(&mut self.adaptive_profile, crate::neural::pipeline::AdaptiveProfile::GeometryHeavy3D, "🌐 3D-Geom");
                });
                let profile_desc = match self.adaptive_profile {
                    crate::neural::pipeline::AdaptiveProfile::Economy => "Economy: Separable LK Flow + YOLO26-N keyframe refresh (90-250+ FPS, lowest latency)",
                    crate::neural::pipeline::AdaptiveProfile::BalancedSpecialist => "Specialist: TAPNext++ point tracking + RF-DETR-Seg direct instance mask (60-120 FPS)",
                    crate::neural::pipeline::AdaptiveProfile::GenericDefault => "Default: SAM 3.1 open-vocab initialization + TAPNext++ continuous tracking (30-60 FPS)",
                    crate::neural::pipeline::AdaptiveProfile::DenseOffline => "Dense: Generic default + CoWTracker repair on uncertain intervals + bidirectional reconciliation",
                    crate::neural::pipeline::AdaptiveProfile::GeometryHeavy3D => "3D-Geom: Dense offline + 3D depth ray triangulation resolving perspective scale ambiguity",
                };
                ui.label(RichText::new(profile_desc).size(11.0).color(Color32::from_rgb(130, 200, 255)));
            });

            ui.add_space(8.0);

            // Input Video Section
            ui.group(|ui| {
                ui.heading("2. Input Video");
                ui.horizontal(|ui| {
                    if ui.button("Browse Video...").clicked() {
                        self.select_video_dialog();
                    }
                    if let Some(ref p) = self.video_path {
                        ui.strong(p.file_name().unwrap_or_default().to_string_lossy());
                    } else {
                        ui.label(RichText::new("No video selected").color(Color32::GRAY));
                    }
                });

                if let Some(ref info) = self.video_info {
                    ui.monospace(info);
                }
            });

            ui.add_space(8.0);

            // Model / Neural Settings (if neural enabled)
            if self.use_neural {
                ui.group(|ui| {
                    ui.heading("3. Neural Model Configuration (YOLO POV Pose)");
                    ui.label("High-speed ONNX Runtime model detecting anatomical landmarks (shaft, head, hands, POV context).");
                    ui.add_space(4.0);

                    // Model Status Badge & Path Info
                    if let Some(ref m) = self.model_path {
                        let filename = m.file_name().unwrap_or_default().to_string_lossy();
                        let size_str = if let Ok(meta) = std::fs::metadata(m) {
                            format!("{:.1} MB", meta.len() as f64 / 1_000_000.0)
                        } else {
                            "Unknown size".to_string()
                        };
                        ui.horizontal(|ui| {
                            ui.colored_label(Color32::from_rgb(60, 220, 100), "● ACTIVE MODEL:");
                            ui.strong(format!("{} ({})", filename, size_str));
                        });
                        ui.label(RichText::new(m.display().to_string()).size(10.5).color(Color32::from_rgb(160, 175, 195)));
                    } else {
                        ui.horizontal(|ui| {
                            ui.colored_label(Color32::from_rgb(255, 80, 80), "○ NO MODEL LOADED:");
                            ui.label("Click Auto-Detect or Download Official Model to install.");
                        });
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("⚡ Auto-Detect Local Models").clicked() {
                            self.auto_detect_model_action();
                        }

                        if self.model_download_in_progress {
                            ui.add(egui::Spinner::new());
                            ui.label(RichText::new("Downloading model...").color(Color32::GOLD));
                        } else {
                            let dl_btn = if self.model_path.is_some() { "⬇ Re-download Official Model" } else { "⬇ Download Official Model" };
                            if ui.button(dl_btn).clicked() {
                                self.download_model_action();
                            }
                        }

                        if ui.button("📁 Custom ONNX File...").clicked() {
                            self.select_model_dialog();
                        }
                    });

                    if let Some(ref status) = self.model_download_status {
                        ui.add_space(2.0);
                        ui.label(RichText::new(status).size(11.0).color(Color32::from_rgb(130, 200, 255)));
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Detection Confidence Threshold:");
                        ui.add(Slider::new(&mut self.neural_conf, 0.1..=0.9).step_by(0.05));
                    });
                });
                ui.add_space(8.0);
            }

            // Signal & Processing Settings
            ui.group(|ui| {
                ui.heading(if self.use_neural { "4. Processing Settings" } else { "3. Processing Settings" });
                ui.horizontal(|ui| {
                    ui.label("Target Frame Rate (FPS):");
                    ui.add(Slider::new(&mut self.target_fps, 15.0..=60.0).step_by(1.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Detrend Window (seconds):");
                    ui.add(Slider::new(&mut self.detrend_window, 1.0..=5.0).step_by(0.5));
                });
                ui.horizontal(|ui| {
                    ui.label("Normalization Window (seconds):");
                    ui.add(Slider::new(&mut self.norm_window, 1.0..=6.0).step_by(0.5));
                });

                ui.add_space(4.0);
                ui.checkbox(&mut self.pov_mode, "POV Mode (Fixed bottom-center interaction anchor)");
                ui.checkbox(&mut self.vr_mode, "VR 180 Stereo Mode (Crop bottom-left quadrant)");
                ui.checkbox(&mut self.generate_multi_axis, "Generate 6-DOF Multi-Axis Bundle (L0, L1, L2, R1, R0, R2, V0)");
            });

            ui.add_space(12.0);

            // Run Button & Progress Section
            if self.is_generating {
                ui.add(ProgressBar::new(self.generation_progress).text(&self.generation_status));
            } else {
                let can_run = self.video_path.is_some() && (!self.use_neural || self.model_path.is_some());
                if ui.add_enabled(can_run, egui::Button::new(RichText::new("Start Generation").size(16.0))).clicked() {
                    self.start_generation_task();
                }
            }
        });
    }

    // =========================================================================
    // View 4: Script Doctor & Repair (F4)
    // =========================================================================
    fn render_doctor_view(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Script Doctor Quality & Repair Center");
            ui.label("Audit script safety against motor limits, detect timing inversions, and apply one-click batch repairs.");
            ui.add_space(8.0);

            if let Some(rep) = self.doctor_report.clone() {
                let is_perfect = rep.speed_violations_count == 0 && rep.timing_inversions_count == 0;
                let (badge_color, badge_text) = if is_perfect {
                    (Color32::from_rgb(60, 200, 100), "PERFECT - Hardware Certified Safe")
                } else if rep.speed_violations_count > 0 {
                    (Color32::from_rgb(255, 70, 70), "WARNING - Motor Speed Violations Detected")
                } else {
                    (Color32::from_rgb(255, 180, 50), "ATTENTION - Timing Inversions or Anomalies")
                };

                ui.group(|ui| {
                    ui.colored_label(badge_color, RichText::new(badge_text).size(15.0).strong());
                    ui.add_space(6.0);

                    ui.columns(4, |cols| {
                        cols[0].label("Total Actions:");
                        cols[0].strong(format!("{}", rep.total_actions));

                        cols[1].label("Duration:");
                        cols[1].strong(format!("{:.1}s", rep.duration_ms as f64 / 1000.0));

                        cols[2].label("Average Speed:");
                        cols[2].strong(format!("{:.1} units/s", rep.avg_speed_units_per_sec));

                        cols[3].label("Maximum Speed:");
                        let speed_color = if rep.max_speed_units_per_sec > 450.0 {
                            Color32::from_rgb(255, 80, 80)
                        } else {
                            Color32::from_rgb(100, 220, 100)
                        };
                        cols[3].colored_label(speed_color, format!("{:.1} units/s", rep.max_speed_units_per_sec));
                    });

                    ui.add_space(4.0);
                    ui.columns(3, |cols| {
                        cols[0].label("Speed Violations (>450 u/s):");
                        cols[0].colored_label(
                            if rep.speed_violations_count > 0 { Color32::from_rgb(255, 80, 80) } else { Color32::GREEN },
                            format!("{}", rep.speed_violations_count),
                        );

                        cols[1].label("Timing Inversions:");
                        cols[1].colored_label(
                            if rep.timing_inversions_count > 0 { Color32::from_rgb(255, 80, 80) } else { Color32::GREEN },
                            format!("{}", rep.timing_inversions_count),
                        );

                        cols[2].label("Micro-Jitters (<20ms):");
                        cols[2].label(format!("{}", rep.micro_jitter_count));
                    });
                });

                ui.add_space(10.0);

                // Quick Repair Tools
                ui.group(|ui| {
                    ui.heading("Automated Repair Tools");
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("⚡ Fix All Issues (1-Click Safe Auto-Repair)").strong().color(Color32::from_rgb(0, 240, 255))).clicked() {
                            self.undo_history.push_snapshot(&self.script);
                            let clamped = self.script.fix_speed_violations(450.0);
                            let removed_jitters = self.script.remove_micro_jitters(20, 2);
                            let initial = self.script.actions.len();
                            self.script.sanitize();
                            let deduped = initial - self.script.actions.len();
                            self.run_doctor();
                            self.set_status(format!(
                                "Safe Auto-Repair complete: clamped {} speeds, removed {} jitters, deduplicated {} points",
                                clamped, removed_jitters, deduped
                            ));
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Clamp Speed Violations (450 u/s)").clicked() {
                            self.undo_history.push_snapshot(&self.script);
                            let fixed = self.script.fix_speed_violations(450.0);
                            self.run_doctor();
                            self.set_status(format!("Clamped {} speed violations to safe limits", fixed));
                        }

                        if ui.button("🧹 Remove Micro-Jitters").clicked() {
                            self.undo_history.push_snapshot(&self.script);
                            let removed = self.script.remove_micro_jitters(20, 2);
                            self.run_doctor();
                            self.set_status(format!("Removed {} micro-jitter keyframes", removed));
                        }

                        if ui.button("✨ Sort & Deduplicate").clicked() {
                            self.undo_history.push_snapshot(&self.script);
                            let initial = self.script.actions.len();
                            self.script.sanitize();
                            let removed = initial - self.script.actions.len();
                            self.run_doctor();
                            self.set_status(format!("Deduplicated {} redundant actions", removed));
                        }
                    });
                });

                ui.add_space(10.0);

                // Detailed Issue Log Table
                ui.heading("Detected Issues List");
                if rep.issues.is_empty() {
                    ui.label(RichText::new("Zero issues detected. This funscript is optimal.").color(Color32::GREEN));
                } else {
                    for (i, issue) in rep.issues.iter().take(25).enumerate() {
                        ui.horizontal(|ui| {
                            let color = match issue.severity {
                                crate::funscript::IssueSeverity::Error => Color32::from_rgb(255, 80, 80),
                                crate::funscript::IssueSeverity::Warning => Color32::from_rgb(255, 180, 50),
                                crate::funscript::IssueSeverity::Info => Color32::from_rgb(120, 180, 255),
                            };
                            ui.colored_label(color, format!("[{:?}]", issue.severity));
                            ui.monospace(format!("{:>7} ms:", issue.timestamp_ms));
                            ui.label(&issue.description);
                            if ui.button("Jump").clicked() {
                                self.timeline_state.cursor_time_ms = issue.timestamp_ms;
                                self.timeline_state.view_start_ms = (issue.timestamp_ms as f64 - self.timeline_state.view_duration_ms / 2.0).max(0.0);
                                self.active_tab = HubTab::Studio;
                            }
                        });
                        if i < rep.issues.len() - 1 {
                            ui.separator();
                        }
                    }
                }
            } else {
                ui.label("No script loaded. Open a funscript to audit.");
            }
        });
    }

    // =========================================================================
    // View 5: Device & Sync (F5)
    // =========================================================================
    fn render_device_sync_view(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Hardware Toy Kinematics & VR Headset Sync");
            ui.label("Calibrated motor dynamics models, jerk-limited S-curve smoothing, thermal wattage protection, and HereSphere/DeoVR zero-config sync.");
            ui.add_space(8.0);

            // 1. Device Profile & S-Curve Configuration Bar
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong("Device Profile:");
                    let mut cur_type = self.device_profile.device_type;
                    egui::ComboBox::from_id_salt("device_profile_combo")
                        .selected_text(cur_type.display_name())
                        .show_ui(ui, |ui| {
                            for dt in DeviceType::ALL {
                                if ui.selectable_value(&mut cur_type, dt, dt.display_name()).clicked() {
                                    self.device_profile = DeviceProfile::for_device(dt);
                                    self.set_status(format!("Active hardware profile: {}", dt.display_name()));
                                }
                            }
                        });

                    ui.separator();
                    ui.strong("S-Curve Smoothing:");
                    egui::ComboBox::from_id_salt("scurve_preset_combo")
                        .selected_text(self.scurve_preset.display_name())
                        .show_ui(ui, |ui| {
                            for p in SCurvePreset::ALL {
                                ui.selectable_value(&mut self.scurve_preset, p, p.display_name());
                            }
                        });

                    ui.separator();
                    if ui.button("⚡ Apply S-Curve to Script (Permanent)").clicked() {
                        let blend_ms = self.scurve_preset.blend_radius_ms();
                        if blend_ms > 0 {
                            let prev_len = self.script.actions.len();
                            self.script = smooth_funscript(&self.script, blend_ms);
                            self.run_doctor();
                            self.set_status(format!(
                                "Applied {} S-curve smoothing (densified from {} to {} actions)",
                                self.scurve_preset.display_name(),
                                prev_len,
                                self.script.actions.len()
                            ));
                        } else {
                            self.set_status("S-curve filter is set to Off (raw linear)".to_string());
                        }
                    }
                });
            });

            ui.add_space(10.0);

            let positions = self.evaluate_current_positions();
            let cur_pos = *positions.get(&AxisChannel::Stroke).unwrap_or(&50.0);

            // 2. Hardware Telemetry & Physics Gauges (3 Cards)
            ui.columns(3, |columns| {
                // Column 1: Multi-Axis Position Gauges
                columns[0].group(|ui| {
                    ui.set_min_height(260.0);
                    ui.heading("Actuator Positions");
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        // Vertical Stroke Meter
                        let gauge_h = 160.0;
                        let gauge_w = 30.0;
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(gauge_w, gauge_h), egui::Sense::hover());
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(rect, 4.0, Color32::from_rgb(30, 35, 45));

                        let fill_h = (cur_pos / 100.0) * gauge_h;
                        let fill_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left(), rect.bottom() - fill_h),
                            rect.max,
                        );
                        painter.rect_filled(fill_rect, 4.0, Color32::from_rgb(0, 200, 255));

                        ui.vertical(|ui| {
                            ui.strong(format!("L0 Stroke: {:.1}%", cur_pos));
                            ui.add_space(6.0);

                            // Other channels
                            for &ch in &[
                                AxisChannel::Surge,
                                AxisChannel::Sway,
                                AxisChannel::Pitch,
                                AxisChannel::Roll,
                                AxisChannel::Twist,
                                AxisChannel::Suction,
                            ] {
                                let val = *positions.get(&ch).unwrap_or(&50.0);
                                let (r, g, b) = ch.color_rgb();
                                ui.horizontal(|ui| {
                                    ui.colored_label(Color32::from_rgb(r, g, b), format!("{:>2}:", ch.tcode_axis()));
                                    ui.monospace(format!("{:>5.1}%", val));
                                });
                            }
                        });
                    });
                });

                // Column 2: Velocity & Acceleration Kinematics
                columns[1].group(|ui| {
                    ui.set_min_height(260.0);
                    ui.heading("Actuator Kinematics");
                    ui.add_space(4.0);

                    let speed = self.kinematic_state.instantaneous_speed;
                    let speed_ratio = self.kinematic_state.speed_ratio;
                    let max_speed = self.device_profile.limits.max_speed_units_per_sec;
                    let mm_speed = speed * (self.device_profile.limits.physical_stroke_mm / 100.0);

                    ui.label(RichText::new("Velocity:").strong());
                    ui.horizontal(|ui| {
                        ui.label(format!("{:.1} units/s ({:.0} mm/s)", speed, mm_speed));
                    });

                    let speed_color = if speed_ratio > 1.0 {
                        Color32::from_rgb(255, 70, 70)
                    } else if speed_ratio > 0.85 {
                        Color32::from_rgb(255, 180, 50)
                    } else {
                        Color32::from_rgb(80, 220, 120)
                    };

                    let speed_pb = ProgressBar::new(speed_ratio.min(1.0))
                        .fill(speed_color)
                        .text(format!("{:.0}% of limit ({:.0} u/s)", speed_ratio * 100.0, max_speed));
                    ui.add(speed_pb);

                    ui.add_space(8.0);
                    let accel = self.kinematic_state.instantaneous_accel;
                    let accel_ratio = self.kinematic_state.accel_ratio;
                    let max_accel = self.device_profile.limits.max_accel_units_per_sec2;

                    ui.label(RichText::new("Acceleration:").strong());
                    ui.label(format!("{:.0} units/s²", accel));

                    let accel_pb = ProgressBar::new(accel_ratio.min(1.0))
                        .fill(Color32::from_rgb(140, 180, 255))
                        .text(format!("{:.0}% of limit ({:.0} u/s²)", accel_ratio * 100.0, max_accel));
                    ui.add(accel_pb);

                    ui.add_space(8.0);
                    if self.kinematic_state.is_speed_warning {
                        ui.colored_label(Color32::from_rgb(255, 80, 80), "⚠ SPEED LIMIT EXCEEDED");
                    } else {
                        ui.colored_label(Color32::from_rgb(100, 220, 150), "✓ Safe Motor Dynamics");
                    }
                });

                // Column 3: Thermal Load & Motor Heat Simulation
                columns[2].group(|ui| {
                    ui.set_min_height(260.0);
                    ui.heading("Motor Thermal Engine");
                    ui.add_space(4.0);

                    let load = self.thermal_model.thermal_load_pct;
                    let status = self.thermal_model.status;

                    let (status_color, status_text) = match status {
                        ThermalStatus::Normal => (Color32::from_rgb(80, 220, 120), "NORMAL"),
                        ThermalStatus::Warm => (Color32::from_rgb(255, 180, 50), "WARMING"),
                        ThermalStatus::Throttling => (Color32::from_rgb(255, 70, 70), "THROTTLING"),
                    };

                    ui.horizontal(|ui| {
                        ui.strong("Status:");
                        ui.colored_label(status_color, status_text);
                    });

                    ui.add_space(4.0);
                    let thermal_pb = ProgressBar::new(load / 100.0)
                        .fill(status_color)
                        .text(format!("{:.1}% Thermal Load", load));
                    ui.add(thermal_pb);

                    ui.add_space(6.0);
                    let temp_rise = self.thermal_model.temp_rise_c;
                    let est_temp = 25.0 + temp_rise;
                    ui.label(format!("Temp Rise:   +{:.1} °C above ambient", temp_rise));
                    ui.label(format!("Estimated:   {:.1} °C coil temp (limit: 80°C)", est_temp));

                    ui.add_space(6.0);
                    if status == ThermalStatus::Throttling {
                        ui.colored_label(Color32::from_rgb(255, 120, 120), "Active Micro-Cooldown Protection");
                        ui.label("Automatically attenuating amplitude to shed coil heat without stopping playback.");
                    } else {
                        ui.label("Coil temperature within safe bounds.");
                    }

                    ui.add_space(8.0);
                    if ui.button("Reset Thermal Model (Cold)").clicked() {
                        self.thermal_model.reset();
                        self.set_status("Reset thermal dissipation accumulator".to_string());
                    }
                });
            });

            ui.add_space(10.0);

            // 3. Interactive 3D Mechanism Kinematic Simulator (OSR2 / SR6)
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.heading("Interactive 3D Robotic Rig Simulator");
                    ui.separator();
                    ui.radio_value(&mut self.rig_model, RigModel::OSR2, "OSR2 (3-DOF)");
                    ui.radio_value(&mut self.rig_model, RigModel::SR6, "SR6 (6-DOF)");
                    ui.separator();
                    if ui.button("↺ Reset Camera Orbit").clicked() {
                        self.rig_view_state = RigViewState::default();
                    }
                });
                ui.label("Real-time forward kinematic linkage simulation with hardware endstop limits. Drag to rotate 3D mechanism.");
                ui.add_space(4.0);

                let sim_w = ui.available_width().min(600.0);
                let rig_size = egui::vec2(sim_w, 220.0);
                let input = self.current_rig_input();
                show_rig_simulator(ui, self.rig_model, &input, &mut self.rig_view_state, rig_size);
            });

            ui.add_space(10.0);

            // 4. Physical Hardware Controllers (Serial COM Port & UDP Network)
            ui.columns(2, |columns| {
                // Column 1: Direct USB Serial Port Hardware (OSR2, SR6, T-Code)
                columns[0].group(|ui| {
                    ui.set_min_height(250.0);
                    ui.heading("USB Serial Hardware (OSR2 / SR6 / T-Code)");
                    ui.label("Direct ultra-low latency serial streaming to physical OSR2, SR6, or DIY ESP32/Teensy T-Code devices.");
                    ui.add_space(4.0);

                    // Port Selection
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Serial Port:").strong());
                        let display_label = if self.available_serial_ports.is_empty() {
                            "(No serial ports detected)".to_string()
                        } else {
                            self.available_serial_ports
                                .iter()
                                .find(|p| p.name == self.selected_serial_port)
                                .map(|p| p.label.clone())
                                .unwrap_or_else(|| self.selected_serial_port.clone())
                        };

                        egui::ComboBox::from_id_salt("serial_port_combo")
                            .selected_text(display_label)
                            .show_ui(ui, |ui| {
                                for p in &self.available_serial_ports {
                                    ui.selectable_value(&mut self.selected_serial_port, p.name.clone(), &p.label);
                                }
                            });

                        if ui.button("🔄 Refresh").clicked() {
                            self.available_serial_ports = SerialDispatcher::list_ports();
                            if !self.available_serial_ports.is_empty() && self.selected_serial_port.is_empty() {
                                self.selected_serial_port = self.available_serial_ports[0].name.clone();
                            }
                            self.set_status(format!("Found {} serial communication port(s)", self.available_serial_ports.len()));
                        }
                    });

                    // Baud Rate Selection
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Baud Rate:").strong());
                        egui::ComboBox::from_id_salt("serial_baud_combo")
                            .selected_text(format!("{} baud", self.selected_baud_rate))
                            .show_ui(ui, |ui| {
                                for &baud in &SerialDispatcher::SUPPORTED_BAUD_RATES {
                                    let tag = match baud {
                                        115200 => " (OSR2 / SR6 Standard)",
                                        230400 => " (High Speed)",
                                        921600 => " (Ultra-Fast ESP32)",
                                        _ => "",
                                    };
                                    ui.selectable_value(&mut self.selected_baud_rate, baud, format!("{} baud{}", baud, tag));
                                }
                            });
                    });

                    ui.add_space(4.0);

                    // Connect / Disconnect Buttons & Live Status
                    ui.horizontal(|ui| {
                        let is_conn = self.serial_dispatcher.is_connected;
                        let btn_text = if is_conn { "Disconnect Serial Port" } else { "Connect Serial Port" };
                        let btn_enabled = !self.selected_serial_port.is_empty();

                        if ui.add_enabled(btn_enabled, egui::Button::new(btn_text)).clicked() {
                            if is_conn {
                                self.serial_dispatcher.disconnect();
                                self.set_status("Disconnected USB serial port".to_string());
                            } else {
                                match self.serial_dispatcher.connect(&self.selected_serial_port, self.selected_baud_rate) {
                                    Ok(()) => {
                                        self.set_status(format!(
                                            "Connected to {} @ {} baud",
                                            self.selected_serial_port, self.selected_baud_rate
                                        ));
                                    }
                                    Err(e) => {
                                        self.set_status(format!("Serial connection error: {}", e));
                                    }
                                }
                            }
                        }

                        if self.serial_dispatcher.is_connected {
                            ui.colored_label(
                                Color32::GREEN,
                                format!(
                                    "● Connected ({} pkts)",
                                    self.serial_dispatcher.sent_commands_count
                                ),
                            );
                        } else {
                            ui.colored_label(Color32::GRAY, "○ Disconnected");
                        }
                    });

                    if let Some(ref err) = self.serial_dispatcher.last_error {
                        ui.colored_label(Color32::from_rgb(255, 90, 90), format!("⚠ {err}"));
                    }

                    ui.add_space(6.0);
                    ui.label(RichText::new("Live Outgoing COM Serial Monitor:").size(11.0).strong());
                    let mon = self
                        .serial_dispatcher
                        .last_sent_command
                        .as_deref()
                        .unwrap_or("Waiting for active playback...");
                    ui.monospace(format!(">>> {mon}"));
                });

                // Column 2: T-Code v0.3 UDP Broadcast
                columns[1].group(|ui| {
                    ui.set_min_height(250.0);
                    ui.heading("T-Code v0.3 UDP Broadcast");
                    ui.label("Streams real-time multi-axis T-Code commands to MultiFunPlayer, Intiface, or physical ESP32/Arduino WiFi controllers.");
                    ui.add_space(4.0);

                    // Quick Endpoint Presets
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Endpoint Presets:").strong());
                        if ui.button("MultiFunPlayer").clicked() {
                            self.sync_udp_address = "127.0.0.1:8888".to_string();
                        }
                        if ui.button("Intiface").clicked() {
                            self.sync_udp_address = "127.0.0.1:12345".to_string();
                        }
                        if ui.button("Local LAN").clicked() {
                            self.sync_udp_address = "192.168.1.50:8888".to_string();
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Target UDP Host:Port:");
                        ui.text_edit_singleline(&mut self.sync_udp_address);
                    });

                    ui.horizontal(|ui| {
                        let btn_label = if self.sync_udp_enabled { "Disconnect UDP Broadcast" } else { "Connect UDP Broadcast" };
                        if ui.button(btn_label).clicked() {
                            self.sync_udp_enabled = !self.sync_udp_enabled;
                            self.tcode_dispatcher.target_addr = self.sync_udp_address.clone();
                            self.tcode_dispatcher.is_enabled = self.sync_udp_enabled;
                            self.set_status(if self.sync_udp_enabled {
                                format!("Streaming T-Code v0.3 to UDP {}", self.sync_udp_address)
                            } else {
                                "UDP Broadcast disconnected".to_string()
                            });
                        }

                        if self.sync_udp_enabled {
                            ui.colored_label(Color32::GREEN, "● Active Streaming");
                        } else {
                            ui.colored_label(Color32::GRAY, "○ Standby");
                        }
                    });

                    ui.add_space(6.0);
                    ui.label(RichText::new("Live Protocol Stream Output:").size(11.0).strong());
                    let display_cmd = if self.last_tcode_string.is_empty() {
                        "L05000 I100\n".to_string()
                    } else {
                        self.last_tcode_string.clone()
                    };
                    ui.monospace(format!(">>> {display_cmd}"));
                });
            });

            ui.add_space(10.0);

            // 4. VR Headset Sync Server (HereSphere & DeoVR)
            ui.group(|ui| {
                ui.heading("VR Headset Sync Server");
                ui.label("Zero-config local WebSocket server for HereSphere, DeoVR, and Whirligig VR headsets.");
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("WebSocket Port:");
                    ui.add(egui::DragValue::new(&mut self.vr_server_port).range(1024..=65535));
                });

                let is_running = self.vr_server.as_ref().map(|s| s.is_running()).unwrap_or(false);

                ui.horizontal(|ui| {
                    let btn_label = if is_running { "Stop VR Sync Server" } else { "Start VR Sync Server" };
                    if ui.button(btn_label).clicked() {
                        if is_running {
                            if let Some(mut s) = self.vr_server.take() {
                                s.stop();
                            }
                            self.set_status("VR WebSocket Sync Server stopped".to_string());
                        } else {
                            let mut s = VrSyncServer::new(self.vr_server_port);
                            if s.start() {
                                self.set_status(format!("VR Sync Server listening on port {}", self.vr_server_port));
                                self.vr_server = Some(s);
                            } else {
                                self.set_status(format!("Failed to bind port {}", self.vr_server_port));
                            }
                        }
                    }

                    if is_running {
                        ui.colored_label(Color32::GREEN, format!("● LISTENING (ws://0.0.0.0:{})", self.vr_server_port));
                    } else {
                        ui.colored_label(Color32::GRAY, "○ Server Stopped");
                    }
                });

                ui.add_space(6.0);
                ui.label(RichText::new("How to connect:").size(11.0).strong());
                ui.label(RichText::new("1. In HereSphere or DeoVR on your Quest/Pico/PCVR headset, enable External Sync / WebSocket.\n2. Point it to this PC's local IP address and configured port.\n3. Funscript Hub will automatically lock playhead time, scrubbing, and pause states with zero latency.").size(10.5).color(Color32::from_rgb(170, 180, 195)));
            });

            ui.add_space(10.0);

            // 4. The Handy (Ohdoki / Sweet Tech) Hardware Cloud & Local Sync
            ui.group(|ui| {
                ui.heading("The Handy — Hardware Cloud & Local HDSP Sync");
                ui.label("Direct physical toy control via official HandyFeeling API v2 (HDSP real-time streaming & HSSP script mode).");
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Connection Key:").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.handy_config.connection_key)
                            .hint_text("e.g. 6-8 char key from Handy app or handyfeeling.com")
                            .desired_width(240.0)
                    );

                    if self.handy_dispatcher.is_connecting {
                        ui.colored_label(Color32::from_rgb(255, 200, 50), "⏳ Connecting...");
                    } else if self.handy_dispatcher.status.is_connected {
                        if ui.button("Disconnect").clicked() {
                            self.handy_dispatcher.disconnect(&self.handy_config.connection_key);
                            self.set_status("The Handy disconnected".to_string());
                        }
                        ui.colored_label(Color32::GREEN, format!(
                            "● Connected ({} | FW: {} | HW: {} | {}ms ping)",
                            self.handy_dispatcher.status.model,
                            self.handy_dispatcher.status.fw_version,
                            self.handy_dispatcher.status.hw_version,
                            self.handy_dispatcher.status.last_ping_ms
                        ));
                    } else {
                        if ui.button("Connect & Verify").clicked() {
                            self.handy_dispatcher.connect(&self.handy_config.connection_key);
                            self.set_status("Connecting to The Handy...".to_string());
                        }
                        ui.colored_label(Color32::GRAY, "○ Disconnected");
                    }

                    if let Some(ref err) = self.handy_dispatcher.last_error {
                        ui.colored_label(Color32::from_rgb(255, 100, 100), format!("⚠ {err}"));
                    }
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.handy_config.is_enabled, RichText::new("Enable Hardware Streaming").strong());
                    ui.separator();

                    ui.label("Protocol Mode:");
                    let mode_label = if self.handy_config.direct_hdsp_mode {
                        "HDSP (High-Definition Streaming Protocol)"
                    } else {
                        "HSSP (Handy Script Sync Protocol)"
                    };
                    let prev_mode = self.handy_config.direct_hdsp_mode;
                    egui::ComboBox::from_id_salt("handy_mode_combo")
                        .selected_text(mode_label)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.handy_config.direct_hdsp_mode, true, "HDSP (High-Definition Streaming Protocol)");
                            ui.selectable_value(&mut self.handy_config.direct_hdsp_mode, false, "HSSP (Handy Script Sync Protocol)");
                        });
                    if prev_mode != self.handy_config.direct_hdsp_mode && self.handy_dispatcher.status.is_connected {
                        let mode_num = if self.handy_config.direct_hdsp_mode { 3 } else { 2 };
                        self.handy_dispatcher.set_mode(&self.handy_config.connection_key, mode_num);
                    }
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stroke Range Limits:").strong());
                    ui.add(Slider::new(&mut self.handy_config.min_stroke_pct, 0.0..=80.0).text("Min Stroke %"));
                    ui.add(Slider::new(&mut self.handy_config.max_stroke_pct, 20.0..=100.0).text("Max Stroke %"));

                    // Ensure min < max
                    if self.handy_config.min_stroke_pct >= self.handy_config.max_stroke_pct - 5.0 {
                        self.handy_config.min_stroke_pct = (self.handy_config.max_stroke_pct - 5.0).max(0.0);
                    }

                    ui.separator();
                    ui.label("Presets:");
                    if ui.button("Full (0-100%)").clicked() {
                        self.handy_config.min_stroke_pct = 0.0;
                        self.handy_config.max_stroke_pct = 100.0;
                    }
                    if ui.button("Deep (0-60%)").clicked() {
                        self.handy_config.min_stroke_pct = 0.0;
                        self.handy_config.max_stroke_pct = 60.0;
                    }
                    if ui.button("Tip (40-100%)").clicked() {
                        self.handy_config.min_stroke_pct = 40.0;
                        self.handy_config.max_stroke_pct = 100.0;
                    }
                    if ui.button("Comfort (20-80%)").clicked() {
                        self.handy_config.min_stroke_pct = 20.0;
                        self.handy_config.max_stroke_pct = 80.0;
                    }
                });

                if self.handy_config.is_enabled && !self.handy_dispatcher.status.is_connected {
                    ui.add_space(4.0);
                    ui.colored_label(Color32::from_rgb(255, 180, 50), "ℹ Hardware streaming is enabled. Enter your Connection Key and click 'Connect & Verify' to lock sync.");
                }
            });
        });
    }

    // =========================================================================
    // Generator Worker Launcher
    // =========================================================================
    fn start_generation_task(&mut self) {
        let video = match self.video_path.clone() {
            Some(v) => v,
            None => return,
        };
        let model = if self.use_neural { self.model_path.clone() } else { None };
        let fps = self.target_fps;
        let pov = self.pov_mode;
        let vr = self.vr_mode;
        let conf = self.neural_conf;
        let _detrend_win = self.detrend_window;
        let _norm_win = self.norm_window;
        let multi_axis = self.generate_multi_axis;

        let (tx, rx): (Sender<WorkerMessage>, Receiver<WorkerMessage>) = channel();
        self.worker_rx = Some(rx);
        self.is_generating = true;
        self.generation_progress = 0.0;
        self.generation_status = "Initializing generator...".to_string();

        thread::spawn(move || {
            let width = if model.is_some() { 640 } else { 256 };
            let height = if model.is_some() { 640 } else { 256 };
            let est_total = crate::video::probe_video(&video)
                .ok()
                .map(|m| (m.duration_secs * fps).round().max(1.0) as u64);

            let res = (|| -> anyhow::Result<MultiAxisScript> {
                let stream_cfg = crate::video::StreamConfig {
                    target_width: width,
                    target_height: height,
                    target_fps: fps,
                    vr_mode: vr,
                    is_rgb: model.is_some(),
                };

                let mut stream = crate::video::FrameStreamReader::new(&video, &stream_cfg)?;
                let mut all_samples = Vec::new();
                let mut surge_samples = Vec::new();
                let mut sway_samples = Vec::new();
                let mut pitch_samples = Vec::new();
                let mut roll_samples = Vec::new();
                let mut neural_poses: Vec<(crate::neural::Pose3D, i64)> = Vec::new();

                let first = stream.next_frame()?.context("Zero video frames")?;
                let mut last_raw = first.0;

                let mut detector = if let Some(ref m) = model {
                    Some(crate::neural::NeuralDetector::new(m, conf)?)
                } else {
                    None
                };
                let mut tracker = if model.is_some() {
                    Some(crate::neural::tracker::AnatomicalTracker::new())
                } else {
                    None
                };

                let mut flow_ctx = crate::tracking::FlowScratchContext::new(width as usize, height as usize);
                let mut frame_count = 0;
                while let Some((curr_raw, ts)) = stream.next_frame()? {
                    frame_count += 1;
                    if frame_count % 30 == 0 {
                        let pct = est_total
                            .map(|tot| (frame_count as f32 / tot as f32).min(0.99))
                            .unwrap_or(0.5);
                        let _ = tx.send(WorkerMessage::Progress {
                            percent: pct,
                            status: format!("Processing frame {}", frame_count),
                        });
                    }

                    let (last_gray, curr_gray) = if stream_cfg.is_rgb {
                        (
                            crate::video::rgb_to_gray(&last_raw),
                            crate::video::rgb_to_gray(&curr_raw),
                        )
                    } else {
                        (last_raw.clone(), curr_raw.clone())
                    };

                    let is_cut_diff = crate::tracking::detect_cut_photometric(&last_gray, &curr_gray, 30.0);
                    let flow = crate::tracking::compute_dense_flow(&last_gray, &curr_gray, width as usize, height as usize, &mut flow_ctx);
                    let is_cut = is_cut_diff || (flow.mean_magnitude() > 8.0);

                    let center = if pov {
                        ((width / 2) as f32, (height - 1) as f32)
                    } else {
                        let (cx, cy, _) = crate::tracking::find_divergence_center(&flow);
                        (cx, cy)
                    };

                    let motion_dot = crate::tracking::project_adaptive_motion(&flow, center, is_cut, pov, true);
                    all_samples.push((motion_dot, is_cut, ts));

                    if multi_axis {
                        let (surge, sway, pitch, roll) = crate::tracking::project_companion_motion(&flow, center, is_cut, pov, true);
                        surge_samples.push((surge, is_cut, ts));
                        sway_samples.push((sway, is_cut, ts));
                        pitch_samples.push((pitch, is_cut, ts));
                        roll_samples.push((roll, is_cut, ts));
                    }

                    if let (Some(ref mut det), Some(ref mut trk)) = (&mut detector, &mut tracker) {
                        let detections = det.detect(&curr_raw, width as usize, height as usize)?;
                        let (pose, _) = trk.update_pose(&detections, Some(&flow));
                        neural_poses.push((pose, ts));
                    }

                    last_raw = curr_raw;
                }

                let mut bundle = MultiAxisScript::new();

                if model.is_some() && !neural_poses.is_empty() {
                    let timestamps: Vec<i64> = neural_poses.iter().map(|(_, ts)| *ts).collect();
                    let stroke_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.stroke).collect();
                    let mut stroke_script = Funscript::new(crate::signal::extract_actions_full_range(&stroke_values, &timestamps, 3.5, true));
                    stroke_script.sanitize();
                    bundle.channels.insert(AxisChannel::Stroke, stroke_script);

                    if multi_axis {
                        let surge_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.surge).collect();
                        let mut surge_script = Funscript::new(crate::signal::extract_actions(&surge_values, &timestamps, true));
                        surge_script.sanitize();
                        bundle.channels.insert(AxisChannel::Surge, surge_script);

                        let sway_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.sway).collect();
                        let mut sway_script = Funscript::new(crate::signal::extract_actions(&sway_values, &timestamps, true));
                        sway_script.sanitize();
                        bundle.channels.insert(AxisChannel::Sway, sway_script);

                        let pitch_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.pitch).collect();
                        let mut pitch_script = Funscript::new(crate::signal::extract_actions(&pitch_values, &timestamps, true));
                        pitch_script.sanitize();
                        bundle.channels.insert(AxisChannel::Pitch, pitch_script);

                        let roll_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.roll).collect();
                        let mut roll_script = Funscript::new(crate::signal::extract_actions(&roll_values, &timestamps, true));
                        roll_script.sanitize();
                        bundle.channels.insert(AxisChannel::Roll, roll_script);

                        let twist_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.twist).collect();
                        let mut twist_script = Funscript::new(crate::signal::extract_actions(&twist_values, &timestamps, true));
                        twist_script.sanitize();
                        bundle.channels.insert(AxisChannel::Twist, twist_script);

                        let suction_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.suction).collect();
                        let mut suction_script = Funscript::new(crate::signal::extract_actions(&suction_values, &timestamps, true));
                        suction_script.sanitize();
                        bundle.channels.insert(AxisChannel::Suction, suction_script);
                    }
                } else {
                    let (cum, ts) = crate::signal::integrate_flow(&all_samples);
                    let mut stroke_script = Funscript::new(crate::signal::extract_actions_full_range(&cum, &ts, 0.8, true));
                    stroke_script.sanitize();
                    bundle.channels.insert(AxisChannel::Stroke, stroke_script);

                    if multi_axis {
                        let (surge_cum, _) = crate::signal::integrate_flow(&surge_samples);
                        let surge_norm = crate::signal::detrend_and_normalize(&surge_cum, fps, 2.0, 3.0, 50.0);
                        let mut surge_script = Funscript::new(crate::signal::extract_actions(&surge_norm, &ts, true));
                        surge_script.sanitize();
                        bundle.channels.insert(AxisChannel::Surge, surge_script);

                        let (sway_cum, _) = crate::signal::integrate_flow(&sway_samples);
                        let sway_norm = crate::signal::detrend_and_normalize(&sway_cum, fps, 2.0, 3.0, 50.0);
                        let mut sway_script = Funscript::new(crate::signal::extract_actions(&sway_norm, &ts, true));
                        sway_script.sanitize();
                        bundle.channels.insert(AxisChannel::Sway, sway_script);

                        let (pitch_cum, _) = crate::signal::integrate_flow(&pitch_samples);
                        let pitch_norm = crate::signal::detrend_and_normalize(&pitch_cum, fps, 2.0, 3.0, 50.0);
                        let mut pitch_script = Funscript::new(crate::signal::extract_actions(&pitch_norm, &ts, true));
                        pitch_script.sanitize();
                        bundle.channels.insert(AxisChannel::Pitch, pitch_script);

                        let (roll_cum, _) = crate::signal::integrate_flow(&roll_samples);
                        let roll_norm = crate::signal::detrend_and_normalize(&roll_cum, fps, 2.0, 3.0, 50.0);
                        let mut roll_script = Funscript::new(crate::signal::extract_actions(&roll_norm, &ts, true));
                        roll_script.sanitize();
                        bundle.channels.insert(AxisChannel::Roll, roll_script);
                    }
                }

                Ok(bundle)
            })();

            let _ = tx.send(WorkerMessage::Done(res.map_err(|e| e.to_string())));
        });
    }

    fn open_funscript_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Funscript", &["funscript", "json"])
            .pick_file()
        {
            if let Ok(bundle) = MultiAxisScript::load_bundle(&path) {
                self.multi_axis = bundle;
                self.active_axis = AxisChannel::Stroke;
                self.script = self.multi_axis.get_or_create(AxisChannel::Stroke).clone();
                self.undo_history.clear();
                self.script_path = Some(path.clone());
                self.run_doctor();
                self.fit_timeline();
                let active_count = self
                    .multi_axis
                    .channels
                    .values()
                    .filter(|s| !s.actions.is_empty())
                    .count();
                self.set_status(format!(
                    "Loaded multi-axis bundle ({} active channels) from {}",
                    active_count,
                    path.display()
                ));
            }
        }
    }

    fn save_funscript(&mut self) {
        self.multi_axis.channels.insert(self.active_axis, self.script.clone());
        if let Some(ref path) = self.script_path {
            if self.multi_axis.save_bundle(path).is_ok() {
                self.set_status(format!("Saved multi-axis bundle for {}", path.display()));
            }
        } else {
            self.save_funscript_as();
        }
    }

    fn save_funscript_as(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Funscript", &["funscript"])
            .save_file()
        {
            self.multi_axis.channels.insert(self.active_axis, self.script.clone());
            if self.multi_axis.save_bundle(&path).is_ok() {
                self.script_path = Some(path.clone());
                self.set_status(format!("Saved multi-axis bundle to {}", path.display()));
            }
        }
    }

    fn select_video_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Video Files", &["mp4", "mkv", "mov", "avi", "webm"])
            .pick_file()
        {
            let fps = if let Ok(meta) = crate::video::probe_video(&path) {
                self.video_info = Some(format!(
                    "{}x{}, {:.2} fps, {:.1}s ({} frames)",
                    meta.width, meta.height, meta.fps, meta.duration_secs, meta.total_frames
                ));
                self.target_fps = meta.fps;
                meta.fps
            } else {
                30.0
            };
            self.video_path = Some(path.clone());
            self.playback_streamer = Some(crate::video::PlaybackStreamer::new(
                &path,
                480,
                270,
                fps,
                0,
            ));

            // Extract audio waveform in background thread
            let (tx, rx) = channel();
            self.waveform_rx = Some(rx);
            let path_for_audio = path.clone();
            thread::spawn(move || {
                let wf = crate::audio::extract_audio_waveform(&path_for_audio).unwrap_or_default();
                let _ = tx.send(wf);
            });

            // Reset video frame cache
            self.last_video_frame_ts = -999;
            self.video_texture = None;
            self.video_frame_cache.clear();
            self.video_pending_req_ts = None;
            self.set_status(format!("Selected video: {}", path.file_name().unwrap_or_default().to_string_lossy()));
        }
    }

    fn select_model_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ONNX Models", &["onnx"])
            .pick_file()
        {
            let info = inspect_model(&path);
            self.detected_model_info = info;
            self.model_path = Some(path.clone());
            self.set_status(format!("Selected model: {}", path.file_name().unwrap_or_default().to_string_lossy()));
        }
    }

    fn auto_detect_model_action(&mut self) {
        if let Some(info) = auto_detect_model() {
            let msg = format!("Located AI model: {} ({:.1} MB)", info.filename, info.size_bytes as f64 / 1_000_000.0);
            self.model_path = Some(info.path.clone());
            self.detected_model_info = Some(info);
            self.set_status(msg);
        } else {
            self.set_status("No local ONNX model detected. Click 'Download Official Model' to fetch it.".to_string());
        }
    }

    fn download_model_action(&mut self) {
        if self.model_download_in_progress {
            return;
        }
        self.model_download_in_progress = true;
        self.model_download_status = Some("Installing / Downloading model...".to_string());
        let (tx, rx) = channel();
        self.model_rx = Some(rx);

        thread::spawn(move || {
            let res = crate::neural::model_manager::install_or_download_default_model(|_downloaded, _total| {});
            let _ = tx.send(res);
        });
    }

    fn fit_timeline(&mut self) {
        if let Some(last) = self.script.actions.last() {
            self.timeline_state.view_start_ms = 0.0;
            self.timeline_state.view_duration_ms = (last.at as f64).max(5000.0);
        }
    }

    fn run_doctor(&mut self) {
        self.doctor_report = Some(diagnose_funscript(&self.script, &DoctorConfig::default()));
    }

    fn apply_current_plugin(&mut self, selection_only: bool) {
        let plugins = self.plugin_host.list_plugins();
        let Some(plugin_meta) = plugins.get(self.selected_plugin_idx) else {
            return;
        };
        let plugin = &self.plugin_host.plugins[self.selected_plugin_idx];
        let mut params = HashMap::new();
        for p in plugin.parameters() {
            let val = self.plugin_param_values.get(&p.key).copied().unwrap_or(p.default);
            params.insert(p.key, val);
        }

        if selection_only {
            let start_ms = self.timeline_state.view_start_ms as i64;
            let end_ms = (self.timeline_state.view_start_ms + self.timeline_state.view_duration_ms) as i64;

            let mut in_range = Vec::new();
            let mut before = Vec::new();
            let mut after = Vec::new();

            for a in &self.script.actions {
                if a.at < start_ms {
                    before.push(*a);
                } else if a.at > end_ms {
                    after.push(*a);
                } else {
                    in_range.push(*a);
                }
            }

            if in_range.is_empty() {
                self.set_status("No keyframes inside current viewport range to transform.".to_string());
                return;
            }

            match self.plugin_host.apply_by_id(&plugin_meta.id, &in_range, &params) {
                Ok(transformed) => {
                    self.undo_history.push_snapshot(&self.script);
                    let mut combined = before;
                    combined.extend(transformed);
                    combined.extend(after);
                    self.script.actions = combined;
                    self.script.sanitize();
                    self.run_doctor();
                    self.set_status(format!("Applied plugin '{}' to viewport range ({}ms - {}ms)", plugin_meta.name, start_ms, end_ms));
                }
                Err(e) => {
                    self.set_status(format!("Plugin execution failed: {e}"));
                }
            }
        } else {
            match self.plugin_host.apply_by_id(&plugin_meta.id, &self.script.actions, &params) {
                Ok(transformed) => {
                    self.undo_history.push_snapshot(&self.script);
                    self.script.actions = transformed;
                    self.script.sanitize();
                    self.run_doctor();
                    self.set_status(format!("Applied plugin '{}' across {} actions", plugin_meta.name, self.script.actions.len()));
                }
                Err(e) => {
                    self.set_status(format!("Plugin execution failed: {e}"));
                }
            }
        }
    }

    fn render_plugin_dialog(&mut self, ctx: &EguiContext) {
        if !self.plugin_dialog_open {
            return;
        }

        let mut open = self.plugin_dialog_open;
        let mut apply_entire = false;
        let mut apply_selection = false;

        egui::Window::new("Community Plugin & Macro SDK")
            .open(&mut open)
            .resizable(true)
            .default_width(450.0)
            .show(ctx, |ui| {
                ui.heading("Procedural Macro Transformers");
                ui.label(
                    RichText::new("Execute non-linear mathematical curve filters and stochastic infills on keyframe curves.")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.separator();

                let plugins = self.plugin_host.list_plugins();
                if plugins.is_empty() {
                    ui.label("No plugins loaded.");
                    return;
                }

                ui.horizontal(|ui| {
                    ui.label("Select Plugin:");
                    egui::ComboBox::from_id_salt("plugin_select_dialog")
                        .selected_text(
                            plugins
                                .get(self.selected_plugin_idx)
                                .map(|p| p.name.as_str())
                                .unwrap_or("Select..."),
                        )
                        .show_ui(ui, |ui| {
                            for (idx, p) in plugins.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_plugin_idx, idx, &p.name);
                            }
                        });
                });

                if let Some(plugin_meta) = plugins.get(self.selected_plugin_idx) {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&plugin_meta.description).italics());
                    ui.monospace(format!("Author: {} | Version: {}", plugin_meta.author, plugin_meta.version));
                    ui.separator();

                    ui.heading("Parameters");
                    let params = self.plugin_host.plugins[self.selected_plugin_idx].parameters();
                    for p in params {
                        let val = self.plugin_param_values.entry(p.key.clone()).or_insert(p.default);
                        ui.horizontal(|ui| {
                            ui.label(format!("{}:", p.label));
                            ui.add(Slider::new(val, p.min..=p.max).step_by(0.1));
                        });
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("⚡ Apply to Entire Script").strong()).clicked() {
                            apply_entire = true;
                        }
                        if ui.button("Apply to Viewport Range").clicked() {
                            apply_selection = true;
                        }
                    });
                }
            });

        self.plugin_dialog_open = open;

        if apply_entire {
            self.apply_current_plugin(false);
        } else if apply_selection {
            self.apply_current_plugin(true);
        }
    }

    fn render_help_dialog(&mut self, ctx: &EguiContext) {
        if !self.help_dialog_open {
            return;
        }

        let mut open = self.help_dialog_open;
        egui::Window::new("⌨ Keyboard & Mouse Shortcuts")
            .open(&mut open)
            .resizable(true)
            .default_width(540.0)
            .show(ctx, |ui| {
                ui.heading("Funscript Hub Controls & Shortcuts");
                ui.separator();

                ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
                    egui::Grid::new("shortcuts_grid")
                        .striped(true)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            ui.strong("Shortcut");
                            ui.strong("Action");
                            ui.strong("Context");
                            ui.end_row();

                            ui.monospace("Ctrl + Z");
                            ui.label("Undo last editing change");
                            ui.label("Studio / Global");
                            ui.end_row();

                            ui.monospace("Ctrl + Y / Ctrl+Shift+Z");
                            ui.label("Redo last undone change");
                            ui.label("Studio / Global");
                            ui.end_row();

                            ui.monospace("Ctrl + S");
                            ui.label("Save funscript bundle");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Ctrl + Shift + S");
                            ui.label("Save funscript as new file");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Ctrl + O");
                            ui.label("Open funscript file / bundle");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Space");
                            ui.label("Play / Pause playback");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Delete / Backspace");
                            ui.label("Delete currently selected keyframe");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("Up / Down Arrow");
                            ui.label("Nudge selected keyframe position (+/- 1)");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("Shift + Up / Down");
                            ui.label("Nudge selected keyframe position (+/- 5)");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("Left / Right Arrow");
                            ui.label("Step time cursor (-33ms / +33ms)");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Shift + Left / Right");
                            ui.label("Step time cursor (-1000ms / +1000ms)");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Home / End");
                            ui.label("Jump to start (0ms) / end of script");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("M");
                            ui.label("Toggle magnetic audio transient snapping");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("Alt + M");
                            ui.label("Snap selected keyframes to nearest audio beat transients");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("[");
                            ui.label("Set A/B Loop In point at current playhead");
                            ui.label("Global / Studio");
                            ui.end_row();

                            ui.monospace("]");
                            ui.label("Set A/B Loop Out point at current playhead");
                            ui.label("Global / Studio");
                            ui.end_row();

                            ui.monospace("\\");
                            ui.label("Clear active A/B section loop");
                            ui.label("Global / Studio");
                            ui.end_row();

                            ui.monospace("Shift + [ / ]");
                            ui.label("Decrease / Increase playback speed (0.25x)");
                            ui.label("Global / Cinema");
                            ui.end_row();

                            ui.monospace("B");
                            ui.label("Drop bookmark marker pin at current playhead");
                            ui.label("Global / Studio");
                            ui.end_row();

                            ui.monospace("Alt + Left / Right");
                            ui.label("Jump playhead to Previous / Next bookmark");
                            ui.label("Global / Studio");
                            ui.end_row();

                            ui.monospace("Alt + V");
                            ui.label("Cycle Audio Viz mode (Waveform / Multi-Band / Spectrogram)");
                            ui.label("Studio / Cinema");
                            ui.end_row();

                            ui.monospace("Alt + I");
                            ui.label("Open Audio Spectral Haptic Infill Synthesizer");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("Shift + Drag Canvas");
                            ui.label("Marquee box multi-select keyframes");
                            ui.label("Timeline");
                            ui.end_row();

                            ui.monospace("1 - 7");
                            ui.label("Switch 6-DOF channel (Stroke, Surge, Sway...)");
                            ui.label("Studio Editor");
                            ui.end_row();

                            ui.monospace("F1 - F6");
                            ui.label("Switch View (Studio, Cinema, Generator...)");
                            ui.label("Global");
                            ui.end_row();

                            ui.monospace("Double-Click Canvas");
                            ui.label("Insert keyframe at mouse position");
                            ui.label("Timeline");
                            ui.end_row();

                            ui.monospace("Left-Click Canvas");
                            ui.label("Seek playhead to clicked timestamp");
                            ui.label("Timeline");
                            ui.end_row();

                            ui.monospace("Left-Click Drag Canvas");
                            ui.label("Scrub playhead smoothly across time");
                            ui.label("Timeline");
                            ui.end_row();

                            ui.monospace("Right-Click Keyframe");
                            ui.label("Delete hovered keyframe");
                            ui.label("Timeline");
                            ui.end_row();

                            ui.monospace("Middle-Click / Ctrl-Drag");
                            ui.label("Pan timeline viewport horizontally");
                            ui.label("Timeline");
                            ui.end_row();

                            ui.monospace("Scroll Wheel");
                            ui.label("Zoom timeline in / out");
                            ui.label("Timeline");
                            ui.end_row();
                        });
                });
            });

        self.help_dialog_open = open;
    }

    fn render_about_dialog(&mut self, ctx: &EguiContext) {
        if !self.about_dialog_open {
            return;
        }

        let mut open = self.about_dialog_open;
        egui::Window::new("ℹ About Funscript Hub")
            .open(&mut open)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Funscript Hub (fs-hub)");
                    ui.label(RichText::new("v0.1.0 • Pure Rust SOTA Composed Monolith").color(Color32::from_rgb(0, 200, 255)));
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                ui.label("Funscript Hub is the open-source, native-first funscript generation, precision editing, and haptic synchronization suite.");
                ui.add_space(6.0);

                ui.group(|ui| {
                    ui.strong("Core Architectural Highlights:");
                    ui.label("• Pure Rust Native Engine (Zero external C bindings)");
                    ui.label("• Real-time Lucas-Kanade dense optical flow with Rayon parallelization");
                    ui.label("• AI Neural YOLOv12 / SAM 2 tracking with ONNX Runtime");
                    ui.label("• 6-DOF Multi-Axis Timeline & Jerk-Limited S-Curve Kinematics");
                    ui.label("• Hardware thermal dissipation wattage protection");
                    ui.label("• Stash GraphQL media server sync & headless batch queue");
                    ui.label("• T-Code v0.3 protocol dispatcher & DeoVR/HereSphere telemetry");
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Support Ongoing Development:");
                    ui.hyperlink_to(
                        RichText::new("☕ Buy Me a Coffee").color(Color32::from_rgb(255, 200, 50)).strong(),
                        "https://buymeacoffee.com/thesmartestgooner",
                    );
                });
                ui.add_space(4.0);
                ui.separator();
                ui.label(RichText::new("Cleanroom Open Source • MIT / Apache-2.0 License").weak().size(11.0));
            });

        self.about_dialog_open = open;
    }

    fn render_rig_simulator_window(&mut self, ctx: &EguiContext) {
        if !self.show_rig_simulator {
            return;
        }

        let mut open = self.show_rig_simulator;
        egui::Window::new("🤖 3D Hardware Rig Simulator (OSR2 / SR6)")
            .open(&mut open)
            .default_size([280.0, 260.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.rig_model, RigModel::OSR2, "OSR2 (3-DOF)");
                    ui.radio_value(&mut self.rig_model, RigModel::SR6, "SR6 (6-DOF)");
                    ui.separator();
                    if ui.button("↺ Reset").on_hover_text("Reset Camera Orbit").clicked() {
                        self.rig_view_state = RigViewState::default();
                    }
                });
                let avail = ui.available_size();
                let input = self.current_rig_input();
                show_rig_simulator(ui, self.rig_model, &input, &mut self.rig_view_state, avail);
            });
        self.show_rig_simulator = open;
    }

    fn render_spectral_infill_dialog(&mut self, ctx: &EguiContext) {
        if !self.show_spectral_infill_dialog {
            return;
        }

        let mut open = self.show_spectral_infill_dialog;
        let mut do_apply = false;
        let mut do_auto_bundle = false;

        egui::Window::new("⚡ Audio Spectral Haptic Infill Synthesizer")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Couples audio frequency bands to 6-DOF haptic hardware axes using Biquad filtering & STFT.")
                        .size(11.0)
                        .color(Color32::from_rgb(140, 155, 175)),
                );
                ui.separator();

                // 1. Source Band Selection
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Frequency Band:").strong());
                    for band in &FrequencyBand::ALL {
                        ui.selectable_value(
                            &mut self.spectral_infill_config.band,
                            *band,
                            band.short_name(),
                        );
                    }
                });
                ui.label(
                    RichText::new(format!("Active Band: {}", self.spectral_infill_config.band.display_name()))
                        .size(11.0)
                        .color(Color32::from_rgb(0, 200, 255)),
                );

                ui.add_space(4.0);

                // 2. Target Axis Selection
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Target Axis:").strong());
                    egui::ComboBox::from_id_salt("spectral_target_axis")
                        .selected_text(self.spectral_infill_config.target_axis.display_name())
                        .show_ui(ui, |ui| {
                            for axis in &AxisChannel::ALL {
                                ui.selectable_value(
                                    &mut self.spectral_infill_config.target_axis,
                                    *axis,
                                    axis.display_name(),
                                );
                            }
                        });
                });

                ui.add_space(4.0);

                // 3. Coupling Mode Selection
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Coupling Mode:").strong());
                    egui::ComboBox::from_id_salt("spectral_infill_mode")
                        .selected_text(self.spectral_infill_config.mode.display_name())
                        .show_ui(ui, |ui| {
                            for mode in &SpectralInfillMode::ALL {
                                ui.selectable_value(
                                    &mut self.spectral_infill_config.mode,
                                    *mode,
                                    mode.display_name(),
                                );
                            }
                        });
                });

                ui.separator();

                // 4. Sliders & Tuning Parameters
                ui.horizontal(|ui| {
                    ui.label("Sensitivity Threshold:");
                    ui.add(Slider::new(&mut self.spectral_infill_config.threshold, 0.05..=0.90).step_by(0.01));
                });

                ui.horizontal(|ui| {
                    ui.label("Position Range:");
                    ui.add(Slider::new(&mut self.spectral_infill_config.min_pos, 0..=100).text("Min"));
                    ui.add(Slider::new(&mut self.spectral_infill_config.max_pos, 0..=100).text("Max"));
                });

                match self.spectral_infill_config.mode {
                    SpectralInfillMode::RhythmicBeats => {
                        ui.horizontal(|ui| {
                            ui.label("Min Beat Interval:");
                            ui.add(Slider::new(&mut self.spectral_infill_config.min_interval_ms, 50..=500).suffix(" ms"));
                        });
                    }
                    SpectralInfillMode::ModulatedOscillation => {
                        ui.horizontal(|ui| {
                            ui.label("Oscillation Freq:");
                            ui.add(Slider::new(&mut self.spectral_infill_config.modulation_freq_hz, 1.0..=20.0).suffix(" Hz"));
                        });
                    }
                    SpectralInfillMode::EnvelopeFollower => {}
                }

                ui.separator();

                // 5. Scope Selection (Entire Media vs A/B Loop)
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Scope:").strong());
                    let has_loop = self.timeline_state.loop_in_ms.is_some() && self.timeline_state.loop_out_ms.is_some();
                    if has_loop {
                        let l_in = self.timeline_state.loop_in_ms.unwrap();
                        let l_out = self.timeline_state.loop_out_ms.unwrap();
                        ui.checkbox(&mut self.spectral_use_loop_range, format!("A/B Loop [{}ms - {}ms]", l_in, l_out));
                    } else {
                        ui.label("Entire Media (Set [ and ] loop markers to isolate a section)");
                    }
                });

                ui.separator();

                // 6. Action buttons
                ui.horizontal(|ui| {
                    let has_audio = self.audio_waveform.as_ref().map(|w| !w.is_empty()).unwrap_or(false);
                    if ui.add_enabled(has_audio, egui::Button::new(RichText::new("⚡ Synthesize Axis").strong().color(Color32::from_rgb(255, 215, 0)))).clicked() {
                        do_apply = true;
                    }

                    if ui.add_enabled(has_audio, egui::Button::new(RichText::new("📦 Auto-Generate 6-DOF Bundle").color(Color32::from_rgb(0, 210, 255)))).clicked() {
                        do_auto_bundle = true;
                    }
                });

                if self.audio_waveform.is_none() {
                    ui.colored_label(Color32::from_rgb(255, 120, 80), "⚠ Load a video or audio file first to extract spectral waveform.");
                }
            });

        self.show_spectral_infill_dialog = open;

        if do_apply {
            if let Some(ref wf) = self.audio_waveform {
                let (start_ms, end_ms) = if self.spectral_use_loop_range && self.timeline_state.loop_in_ms.is_some() && self.timeline_state.loop_out_ms.is_some() {
                    (self.timeline_state.loop_in_ms.unwrap(), self.timeline_state.loop_out_ms.unwrap())
                } else {
                    let dur = (wf.duration_secs * 1000.0).round() as i64;
                    (0, dur.max(self.script.actions.last().map(|a| a.at).unwrap_or(10_000)))
                };

                let empty_spec = crate::audio::SpectralAnalysis::default();
                let spec_ref = wf.spectral.as_ref().unwrap_or(&empty_spec);

                let generated_actions = synthesize_spectral_actions(
                    spec_ref,
                    &wf.peaks,
                    wf.bins_per_sec,
                    &self.spectral_infill_config,
                    start_ms,
                    end_ms,
                );

                if !generated_actions.is_empty() {
                    let target_axis = self.spectral_infill_config.target_axis;
                    let target_script = self.multi_axis.get_or_create(target_axis);

                    if target_axis == self.active_axis {
                        self.undo_history.push_snapshot(&self.script);
                        self.script.actions.retain(|a| a.at < start_ms || a.at > end_ms);
                        self.script.actions.extend(generated_actions.clone());
                        self.script.sanitize();
                        *target_script = self.script.clone();
                    } else {
                        target_script.actions.retain(|a| a.at < start_ms || a.at > end_ms);
                        target_script.actions.extend(generated_actions.clone());
                        target_script.sanitize();
                    }

                    self.run_doctor();
                    self.set_status(format!(
                        "⚡ Synthesized {} actions for {} ({}ms - {}ms)",
                        generated_actions.len(),
                        target_axis.display_name(),
                        start_ms,
                        end_ms
                    ));
                    self.show_spectral_infill_dialog = false;
                } else {
                    self.set_status("No actions synthesized; try lowering the sensitivity threshold slider.".to_string());
                }
            }
        }

        if do_auto_bundle {
            if let Some(ref wf) = self.audio_waveform {
                let duration_ms = (wf.duration_secs * 1000.0).round() as i64;
                let empty_spec = crate::audio::SpectralAnalysis::default();
                let spec_ref = wf.spectral.as_ref().unwrap_or(&empty_spec);

                let bundle = generate_multiband_companion_bundle(
                    spec_ref,
                    &wf.peaks,
                    wf.bins_per_sec,
                    duration_ms,
                );

                self.undo_history.push_snapshot(&self.script);
                if let Some(stroke) = bundle.channels.get(&AxisChannel::Stroke) {
                    self.script = stroke.clone();
                }
                self.multi_axis = bundle;
                self.run_doctor();
                self.set_status(format!(
                    "⚡ Auto-generated full multi-band companion bundle ({} channels) from audio spectrum!",
                    self.multi_axis.channels.len()
                ));
                self.show_spectral_infill_dialog = false;
            }
        }
    }

    fn render_automation_view(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Funscript Hub: Automation, Stash & Community Plugin Ecosystem");
            ui.label(
                RichText::new("Centralized control center for media server sync, high-throughput batch generation queues, and procedural macro plugins.")
                    .color(Color32::from_rgb(160, 170, 185)),
            );
            ui.add_space(8.0);

            // Section 1: Stash Media Server Integration
            ui.group(|ui| {
                ui.heading("1. Stash Media Server Integration (GraphQL)");
                ui.label(
                    RichText::new("Connect to your self-hosted Stash instance (localhost:9999) to detect scenes lacking funscripts, batch-generate them, and trigger metadata scans.")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("Endpoint:");
                    ui.text_edit_singleline(&mut self.stash_config.endpoint);
                    ui.label("API Key:");
                    let mut api_key_str = self.stash_config.api_key.clone().unwrap_or_default();
                    if ui.text_edit_singleline(&mut api_key_str).changed() {
                        self.stash_config.api_key = if api_key_str.trim().is_empty() {
                            None
                        } else {
                            Some(api_key_str)
                        };
                    }
                    if ui.button("🔌 Test Connection").clicked() {
                        let client = StashClient::new(self.stash_config.clone());
                        match client.test_connection() {
                            Ok(ver) => {
                                self.stash_connected = true;
                                self.stash_status = Some(format!("Connected: Stash v{}", ver));
                            }
                            Err(e) => {
                                self.stash_connected = false;
                                self.stash_status = Some(format!("Connection Failed: {}", e));
                            }
                        }
                    }
                    if ui.button("🔍 Find Scenes Missing Scripts").clicked() {
                        let client = StashClient::new(self.stash_config.clone());
                        match client.find_scenes_missing_scripts(50) {
                            Ok(scenes) => {
                                let count = scenes.len();
                                self.stash_scenes = scenes;
                                self.stash_status = Some(format!("Found {} scenes missing funscripts", count));
                            }
                            Err(e) => {
                                self.stash_status = Some(format!("Query failed: {}", e));
                            }
                        }
                    }
                });

                if let Some(ref status) = self.stash_status {
                    ui.label(RichText::new(status).color(if self.stash_connected { Color32::GREEN } else { Color32::LIGHT_BLUE }));
                }

                if !self.stash_scenes.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Discovered Scenes Requiring Funscripts ({} total):", self.stash_scenes.len())).strong());
                        if ui.button("⚡ Enqueue All to Batch Queue").clicked() {
                            let mut added = 0;
                            for sc in &self.stash_scenes {
                                if let Some(f) = sc.files.first() {
                                    let path = PathBuf::from(&f.path);
                                    let out = path.with_extension("funscript");
                                    if !self.batch_queue.jobs.iter().any(|j| j.video_path == path) {
                                        self.batch_queue.jobs.push(BatchJob {
                                            id: self.batch_queue.next_job_id,
                                            video_path: path,
                                            output_path: out,
                                            status: BatchJobStatus::Queued,
                                        });
                                        self.batch_queue.next_job_id += 1;
                                        added += 1;
                                    }
                                }
                            }
                            self.set_status(format!("Enqueued {} scenes from Stash into Batch Queue!", added));
                        }
                    });

                    egui::Frame::NONE
                        .fill(Color32::from_rgb(18, 20, 26))
                        .show(ui, |ui| {
                            ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                                for sc in &self.stash_scenes {
                                    ui.horizontal(|ui| {
                                        ui.monospace(format!("[ID: {}]", sc.id));
                                        let title = sc.title.as_deref().unwrap_or("<Untitled>");
                                        ui.label(title);
                                        if let Some(f) = sc.files.first() {
                                            ui.label(RichText::new(&f.path).color(Color32::GRAY).size(10.0));
                                        }
                                    });
                                }
                            });
                        });
                }
            });

            ui.add_space(8.0);

            // Section 2: Headless Multi-threaded Batch Processing Queue
            ui.group(|ui| {
                ui.heading("2. High-Throughput Batch Processing Queue");
                ui.label(
                    RichText::new("Scan folders recursively for videos, automatically enqueue missing scripts, and process in background thread with zero UI lag.")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("Scan Directory:");
                    ui.text_edit_singleline(&mut self.batch_scan_path);
                    if ui.button("Browse...").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.batch_scan_path = dir.to_string_lossy().to_string();
                        }
                    }
                    if ui.button("📂 Scan & Enqueue").clicked() {
                        let p = PathBuf::from(&self.batch_scan_path);
                        if p.is_dir() {
                            let count = self.batch_queue.scan_directory(&p, true, &self.batch_config);
                            self.set_status(format!("Enqueued {} videos for processing.", count));
                        } else {
                            self.set_status("Specified path is not a valid directory.".to_string());
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.batch_config.multi_axis, "Generate 6-DOF Companion Bundle (.surge, .sway, etc.)");
                    ui.checkbox(&mut self.batch_config.overwrite, "Overwrite Existing Scripts");
                });

                ui.add_space(4.0);
                let (total, queued, processing, completed, failed, _skipped) = self.batch_queue.stats();
                ui.horizontal(|ui| {
                    ui.monospace(format!("Jobs: {} Total | {} Queued | {} Running | {} Done | {} Failed", total, queued, processing, completed, failed));
                    if self.batch_worker.is_active() {
                        if ui.button("⏸ Stop Worker").clicked() {
                            self.batch_worker.stop();
                            self.set_status("Batch worker stopped.".to_string());
                        }
                    } else if !self.batch_queue.jobs.is_empty()
                        && ui.button(RichText::new("▶ Start Batch Processing").strong()).clicked()
                    {
                        let jobs = self.batch_queue.jobs.clone();
                        let cfg = self.batch_config.clone();
                        self.batch_worker.start(jobs, cfg);
                        self.set_status("Batch processing worker started.".to_string());
                    }
                    if ui.button("Clear Queue").clicked() {
                        self.batch_queue.clear();
                    }
                });

                if !self.batch_queue.jobs.is_empty() {
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(18, 20, 26))
                        .show(ui, |ui| {
                            ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                                for job in &self.batch_queue.jobs {
                                    ui.horizontal(|ui| {
                                        let status_text = match &job.status {
                                            BatchJobStatus::Queued => RichText::new("QUEUED").color(Color32::GRAY),
                                            BatchJobStatus::Processing { progress, .. } => {
                                                RichText::new(format!("RUNNING ({:.0}%)", progress * 100.0)).color(Color32::from_rgb(0, 200, 255))
                                            }
                                            BatchJobStatus::Completed { actions_count, .. } => {
                                                RichText::new(format!("DONE ({} pts)", actions_count)).color(Color32::GREEN)
                                            }
                                            BatchJobStatus::Failed { error } => {
                                                RichText::new(format!("ERR: {}", error)).color(Color32::RED)
                                            }
                                            BatchJobStatus::Skipped { reason } => {
                                                RichText::new(format!("SKIP: {}", reason)).color(Color32::YELLOW)
                                            }
                                        };
                                        ui.monospace(format!("[#{:>2}]", job.id));
                                        ui.label(status_text);
                                        ui.label(job.video_path.file_name().unwrap_or_default().to_string_lossy());
                                    });
                                }
                            });
                        });
                }
            });

            ui.add_space(8.0);

            // Section 3: Community Plugin & Procedural Macro SDK
            ui.group(|ui| {
                ui.heading("3. Community Plugin & Procedural Macro SDK");
                ui.label(
                    RichText::new("Extensible procedural macro interface for transforming keyframe actions using pure mathematical signal filters.")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(4.0);

                let plugins = self.plugin_host.list_plugins();
                ui.horizontal(|ui| {
                    ui.label("Select Macro Plugin:");
                    egui::ComboBox::from_id_salt("plugin_select_tab")
                        .selected_text(
                            plugins
                                .get(self.selected_plugin_idx)
                                .map(|p| p.name.as_str())
                                .unwrap_or("Select..."),
                        )
                        .show_ui(ui, |ui| {
                            for (idx, p) in plugins.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_plugin_idx, idx, &p.name);
                            }
                        });
                });

                if let Some(meta) = plugins.get(self.selected_plugin_idx) {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&meta.description).italics());
                    ui.monospace(format!("Author: {} | Version: {}", meta.author, meta.version));
                    ui.add_space(4.0);

                    let params = self.plugin_host.plugins[self.selected_plugin_idx].parameters();
                    for p in params {
                        let val = self.plugin_param_values.entry(p.key.clone()).or_insert(p.default);
                        ui.horizontal(|ui| {
                            ui.label(format!("{}:", p.label));
                            ui.add(Slider::new(val, p.min..=p.max).step_by(0.1));
                        });
                    }

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("⚡ Apply to Current Script").strong()).clicked() {
                            self.apply_current_plugin(false);
                        }
                        if ui.button("Apply to Viewport Range").clicked() {
                            self.apply_current_plugin(true);
                        }
                        if ui.button("Open as Floating Dialog").clicked() {
                            self.plugin_dialog_open = true;
                        }
                    });
                }
            });
        });
    }
}

fn format_ms(ms: i64) -> String {
    let total_secs = ms.max(0) / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    let millis = ms.max(0) % 1000;
    format!("{:02}:{:02}.{:03}", mins, secs, millis)
}
