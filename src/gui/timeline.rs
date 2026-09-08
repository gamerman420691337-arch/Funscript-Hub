//! Interactive immediate-mode timeline widget for egui.

use crate::funscript::{Action, DoctorReport, Funscript};
use eframe::egui::{
    pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui,
};

use std::collections::BTreeSet;

/// Audio visualization mode on the timeline canvas
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioVizMode {
    #[default]
    Waveform,    // Standard Cyan RMS envelope
    MultiBand,   // Tri-color stacked bands: Sub-Bass (Magenta), Mid (Cyan), High (Amber)
    Spectrogram, // 16-band heat-map waterfall spectrogram (Inferno palette)
}

impl AudioVizMode {
    #[allow(dead_code)]
    pub const ALL: [AudioVizMode; 3] = [
        AudioVizMode::Waveform,
        AudioVizMode::MultiBand,
        AudioVizMode::Spectrogram,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            AudioVizMode::Waveform => "Waveform (RMS)",
            AudioVizMode::MultiBand => "Multi-Band Split",
            AudioVizMode::Spectrogram => "Waterfall Spectrogram",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            AudioVizMode::Waveform => "🎵",
            AudioVizMode::MultiBand => "🌈",
            AudioVizMode::Spectrogram => "⚡",
        }
    }
}

/// Scene or chapter bookmark timestamp with a descriptive label.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub time_ms: i64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct TimelineState {
    /// Start of visible time window in milliseconds
    pub view_start_ms: f64,
    /// Duration of visible time window in milliseconds (zoom level)
    pub view_duration_ms: f64,
    /// Current playhead position in milliseconds
    pub cursor_time_ms: i64,
    /// Index of primary / lead selected keyframe action
    pub selected_index: Option<usize>,
    /// Set of all selected keyframe action indices (multi-selection)
    pub selected_indices: BTreeSet<usize>,
    /// Starting point of active marquee selection box (if dragging)
    pub marquee_start: Option<Pos2>,
    /// Current pointer position of active marquee selection box
    pub marquee_current: Option<Pos2>,
    /// Whether a keyframe point is actively being dragged
    pub is_dragging_point: bool,
    /// Whether magnetic snapping to audio waveform transients is enabled
    pub magnetic_snapping: bool,
    /// Currently snapped acoustic transient timestamp (if active)
    pub snapped_transient_ms: Option<i64>,
    /// A/B Loop In point in milliseconds
    pub loop_in_ms: Option<i64>,
    /// A/B Loop Out point in milliseconds
    pub loop_out_ms: Option<i64>,
    /// Whether A/B looping is active
    pub loop_enabled: bool,
    /// Scene / chapter bookmarks
    pub bookmarks: Vec<Bookmark>,
    /// Audio visualization mode on the timeline canvas
    pub audio_viz_mode: AudioVizMode,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            view_start_ms: 0.0,
            view_duration_ms: 10_000.0, // Default 10 second window
            cursor_time_ms: 0,
            selected_index: None,
            selected_indices: BTreeSet::new(),
            marquee_start: None,
            marquee_current: None,
            is_dragging_point: false,
            magnetic_snapping: true,
            snapped_transient_ms: None,
            loop_in_ms: None,
            loop_out_ms: None,
            loop_enabled: false,
            bookmarks: Vec::new(),
            audio_viz_mode: AudioVizMode::default(),
        }
    }
}

impl TimelineState {
    pub fn time_to_screen_x(&self, time_ms: f64, rect: Rect) -> f32 {
        let frac = (time_ms - self.view_start_ms) / self.view_duration_ms.max(100.0);
        rect.left() + (frac as f32) * rect.width()
    }

    pub fn screen_x_to_time(&self, x: f32, rect: Rect) -> i64 {
        let frac = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        (self.view_start_ms + (frac as f64) * self.view_duration_ms).round() as i64
    }

    pub fn pos_to_screen_y(pos: i32, rect: Rect) -> f32 {
        let frac = (pos as f32) / 100.0;
        // Position 100 is top, 0 is bottom (with padding)
        let pad = 24.0;
        let inner_height = (rect.height() - pad * 2.0).max(10.0);
        rect.bottom() - pad - frac * inner_height
    }

    pub fn screen_y_to_pos(y: f32, rect: Rect) -> i32 {
        let pad = 24.0;
        let inner_height = (rect.height() - pad * 2.0).max(10.0);
        let frac = (rect.bottom() - pad - y) / inner_height;
        (frac * 100.0).round().clamp(0.0, 100.0) as i32
    }

    pub fn select_single(&mut self, idx: usize) {
        self.selected_index = Some(idx);
        self.selected_indices.clear();
        self.selected_indices.insert(idx);
    }

    pub fn toggle_selection(&mut self, idx: usize) {
        if self.selected_indices.contains(&idx) {
            self.selected_indices.remove(&idx);
            if self.selected_index == Some(idx) {
                self.selected_index = self.selected_indices.iter().next().copied();
            }
        } else {
            self.selected_indices.insert(idx);
            self.selected_index = Some(idx);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected_index = None;
        self.selected_indices.clear();
        self.marquee_start = None;
        self.marquee_current = None;
    }

    pub fn select_all(&mut self, total_count: usize) {
        self.selected_indices = (0..total_count).collect();
        self.selected_index = self.selected_indices.iter().next().copied();
    }

    pub fn is_selected(&self, idx: usize) -> bool {
        self.selected_indices.contains(&idx) || self.selected_index == Some(idx)
    }

    /// Set A/B loop In point
    pub fn set_loop_in(&mut self, time_ms: i64) {
        self.loop_in_ms = Some(time_ms);
        if let Some(out) = self.loop_out_ms {
            if time_ms >= out {
                self.loop_out_ms = Some(time_ms + 1000);
            }
        }
        self.loop_enabled = true;
    }

    /// Set A/B loop Out point
    pub fn set_loop_out(&mut self, time_ms: i64) {
        self.loop_out_ms = Some(time_ms);
        if let Some(in_t) = self.loop_in_ms {
            if time_ms <= in_t {
                self.loop_in_ms = Some(time_ms.saturating_sub(1000));
            }
        }
        self.loop_enabled = true;
    }

    /// Clear A/B loop region
    pub fn clear_loop(&mut self) {
        self.loop_in_ms = None;
        self.loop_out_ms = None;
        self.loop_enabled = false;
    }

    /// Add a bookmark at the given timestamp
    pub fn add_bookmark(&mut self, time_ms: i64, name: String) {
        self.bookmarks.push(Bookmark { time_ms, name });
        self.bookmarks.sort_by_key(|b| b.time_ms);
        self.bookmarks.dedup_by_key(|b| b.time_ms);
    }

    /// Jump to the next bookmark after current playhead
    pub fn next_bookmark(&self, cur_time: i64) -> Option<i64> {
        self.bookmarks.iter().find(|b| b.time_ms > cur_time).map(|b| b.time_ms)
    }

    /// Jump to the previous bookmark before current playhead
    pub fn prev_bookmark(&self, cur_time: i64) -> Option<i64> {
        self.bookmarks.iter().rev().find(|b| b.time_ms < cur_time).map(|b| b.time_ms)
    }

    /// Cycle between Waveform, Multi-Band, and Waterfall Spectrogram visual modes
    pub fn cycle_audio_viz_mode(&mut self) {
        self.audio_viz_mode = match self.audio_viz_mode {
            AudioVizMode::Waveform => AudioVizMode::MultiBand,
            AudioVizMode::MultiBand => AudioVizMode::Spectrogram,
            AudioVizMode::Spectrogram => AudioVizMode::Waveform,
        };
    }
}

/// Render and handle interactions on the interactive timeline canvas
#[allow(clippy::too_many_arguments)]
pub fn show_timeline(
    ui: &mut Ui,
    script: &mut Funscript,
    state: &mut TimelineState,
    doctor_report: Option<&DoctorReport>,
    waveform: Option<&crate::audio::AudioWaveform>,
    ghost_axes: Option<&[(&str, &Funscript, Color32)]>,
    active_color: Color32,
    undo_history: Option<&mut crate::funscript::UndoHistory>,
) -> Response {
    let desired_size = vec2(ui.available_width(), ui.available_height().max(280.0));
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    // 1. Handle Navigation: Zoom and Pan
    let pointer = response.interact_pointer_pos();

    // Zoom via mouse scroll wheel
    let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
    if scroll_delta != 0.0 && response.hovered() {
        let zoom_factor = if scroll_delta > 0.0 { 0.85 } else { 1.18 };
        let pointer_x = pointer.map(|p| p.x).unwrap_or(rect.center().x);
        let mouse_time = state.screen_x_to_time(pointer_x, rect) as f64;

        let new_duration = (state.view_duration_ms * zoom_factor).clamp(500.0, 3_600_000.0);
        let time_frac = (mouse_time - state.view_start_ms) / state.view_duration_ms;
        state.view_start_ms = (mouse_time - time_frac * new_duration).max(0.0);
        state.view_duration_ms = new_duration;
    }

    // Pan via secondary/middle drag or Ctrl+drag
    if response.dragged_by(eframe::egui::PointerButton::Middle)
        || (response.dragged() && ui.input(|i| i.modifiers.command || i.modifiers.ctrl))
    {
        let drag_delta = response.drag_delta();
        let time_delta = -(drag_delta.x as f64) * (state.view_duration_ms / (rect.width() as f64));
        state.view_start_ms = (state.view_start_ms + time_delta).max(0.0);
    }

    // 2. Render Canvas Background & Grids
    painter.rect_filled(rect, 4.0, Color32::from_rgb(22, 24, 29));
    draw_grids(&painter, rect, state);

    // 2a. Render A/B Loop Region (if active)
    draw_loop_region(&painter, rect, state);

    // 2b. Render Audio Waveform (translucent acoustic bars under curve)
    draw_audio_waveform(&painter, rect, state, waveform);

    // 2c. Render Scene Bookmarks
    draw_bookmarks(&painter, rect, state);

    // 3. Render Action Curve, Ghost Multi-Axis Curves, & Highlights
    draw_action_curve(&painter, rect, script, state, doctor_report, ghost_axes, active_color);

    // 3b. Render Marquee Selection Box
    if let (Some(m_start), Some(m_curr)) = (state.marquee_start, state.marquee_current) {
        let m_rect = Rect::from_two_pos(m_start, m_curr);
        painter.rect_filled(m_rect, 0.0, Color32::from_rgba_unmultiplied(0, 180, 255, 40));
        painter.rect_stroke(
            m_rect,
            0.0,
            Stroke::new(1.2f32, Color32::from_rgb(0, 210, 255)),
            eframe::egui::StrokeKind::Middle,
        );
    }

    // 4. Handle Keyframe Point Selection, Dragging, Adding & Deleting (with Magnetic Snapping)
    handle_point_interactions(ui, &response, rect, script, state, waveform, undo_history);

    // 4b. Render Magnetic Transient Snap Indicator if snapped
    if let Some(snap_t) = state.snapped_transient_ms {
        let snap_x = state.time_to_screen_x(snap_t as f64, rect);
        if snap_x >= rect.left() && snap_x <= rect.right() {
            painter.line_segment(
                [pos2(snap_x, rect.top()), pos2(snap_x, rect.bottom())],
                Stroke::new(1.8f32, Color32::from_rgb(0, 240, 255)),
            );
            let badge_rect = Rect::from_min_size(pos2(snap_x - 24.0, rect.top() + 4.0), vec2(48.0, 16.0));
            painter.rect_filled(badge_rect, 3.0, Color32::from_rgba_unmultiplied(0, 25, 40, 230));
            painter.rect_stroke(badge_rect, 3.0, Stroke::new(1.0f32, Color32::from_rgb(0, 240, 255)), eframe::egui::StrokeKind::Middle);
            painter.text(
                badge_rect.center(),
                eframe::egui::Align2::CENTER_CENTER,
                "⚡ SNAP",
                eframe::egui::FontId::monospace(10.0),
                Color32::from_rgb(0, 240, 255),
            );
        }
    }

    // 5. Render Draggable Playhead Cursor
    let cursor_x = state.time_to_screen_x(state.cursor_time_ms as f64, rect);
    if cursor_x >= rect.left() && cursor_x <= rect.right() {
        painter.line_segment(
            [pos2(cursor_x, rect.top()), pos2(cursor_x, rect.bottom())],
            Stroke::new(1.5f32, Color32::from_rgb(255, 64, 129)),
        );
        // Playhead head indicator
        painter.circle_filled(pos2(cursor_x, rect.top() + 6.0), 5.0, Color32::from_rgb(255, 64, 129));
    }

    response
}

fn draw_grids(painter: &Painter, rect: Rect, state: &TimelineState) {
    // Horizontal position gridlines (0, 25, 50, 75, 100)
    for &pos in &[0, 25, 50, 75, 100] {
        let y = TimelineState::pos_to_screen_y(pos, rect);
        let color = if pos == 0 || pos == 100 {
            Color32::from_rgb(60, 65, 75)
        } else {
            Color32::from_rgb(38, 42, 50)
        };
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0f32, color));
        painter.text(
            pos2(rect.left() + 4.0, y - 8.0),
            eframe::egui::Align2::LEFT_TOP,
            format!("{}", pos),
            eframe::egui::FontId::monospace(10.0),
            Color32::from_rgb(120, 130, 145),
        );
    }

    // Vertical time gridlines
    let step_ms = calculate_time_step_ms(state.view_duration_ms);
    let first_tick = (state.view_start_ms / step_ms).floor() as i64 * step_ms as i64;
    let end_time = (state.view_start_ms + state.view_duration_ms) as i64;

    let mut t = first_tick;
    while t <= end_time {
        if t >= 0 {
            let x = state.time_to_screen_x(t as f64, rect);
            if x >= rect.left() && x <= rect.right() {
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.0f32, Color32::from_rgb(35, 38, 45)),
                );
                let time_label = format_timestamp_ms(t);
                painter.text(
                    pos2(x + 3.0, rect.bottom() - 16.0),
                    eframe::egui::Align2::LEFT_BOTTOM,
                    time_label,
                    eframe::egui::FontId::monospace(10.0),
                    Color32::from_rgb(100, 110, 125),
                );
            }
        }
        t += step_ms as i64;
    }
}

fn draw_loop_region(painter: &Painter, rect: Rect, state: &TimelineState) {
    if !state.loop_enabled {
        return;
    }
    if let (Some(in_ms), Some(out_ms)) = (state.loop_in_ms, state.loop_out_ms) {
        if out_ms > in_ms {
            let x_in = state.time_to_screen_x(in_ms as f64, rect);
            let x_out = state.time_to_screen_x(out_ms as f64, rect);

            let visible_left = x_in.clamp(rect.left(), rect.right());
            let visible_right = x_out.clamp(rect.left(), rect.right());

            if visible_right > visible_left {
                let loop_rect = Rect::from_x_y_ranges(visible_left..=visible_right, rect.top()..=rect.bottom());
                painter.rect_filled(loop_rect, 0.0, Color32::from_rgba_unmultiplied(0, 160, 255, 35));
            }

            // Loop In boundary flag
            if x_in >= rect.left() && x_in <= rect.right() {
                painter.line_segment(
                    [pos2(x_in, rect.top()), pos2(x_in, rect.bottom())],
                    Stroke::new(1.8f32, Color32::from_rgb(0, 210, 255)),
                );
                painter.text(
                    pos2(x_in + 4.0, rect.top() + 4.0),
                    eframe::egui::Align2::LEFT_TOP,
                    "⟪ LOOP IN",
                    eframe::egui::FontId::monospace(9.5),
                    Color32::from_rgb(0, 220, 255),
                );
            }

            // Loop Out boundary flag
            if x_out >= rect.left() && x_out <= rect.right() {
                painter.line_segment(
                    [pos2(x_out, rect.top()), pos2(x_out, rect.bottom())],
                    Stroke::new(1.8f32, Color32::from_rgb(0, 210, 255)),
                );
                painter.text(
                    pos2(x_out - 4.0, rect.top() + 4.0),
                    eframe::egui::Align2::RIGHT_TOP,
                    "LOOP OUT ⟫",
                    eframe::egui::FontId::monospace(9.5),
                    Color32::from_rgb(0, 220, 255),
                );
            }
        }
    }
}

fn draw_bookmarks(painter: &Painter, rect: Rect, state: &TimelineState) {
    for bm in &state.bookmarks {
        let x = state.time_to_screen_x(bm.time_ms as f64, rect);
        if x >= rect.left() && x <= rect.right() {
            let top_y = rect.top() + 7.0;
            let diamond_pts = [
                pos2(x, top_y - 5.0),
                pos2(x + 4.0, top_y),
                pos2(x, top_y + 5.0),
                pos2(x - 4.0, top_y),
            ];
            painter.add(Shape::convex_polygon(
                diamond_pts.to_vec(),
                Color32::from_rgb(255, 180, 50),
                Stroke::new(1.0f32, Color32::from_rgb(30, 30, 30)),
            ));

            painter.line_segment(
                [pos2(x, top_y + 5.0), pos2(x, rect.bottom())],
                Stroke::new(1.0f32, Color32::from_rgba_unmultiplied(255, 180, 50, 45)),
            );

            painter.text(
                pos2(x + 5.0, top_y - 6.0),
                eframe::egui::Align2::LEFT_TOP,
                &bm.name,
                eframe::egui::FontId::proportional(9.0),
                Color32::from_rgb(255, 200, 80),
            );
        }
    }
}

fn inferno_color(val: f32) -> Color32 {
    let v = val.clamp(0.0, 1.0);
    if v < 0.05 {
        Color32::from_rgba_unmultiplied(15, 15, 35, 30)
    } else if v < 0.30 {
        // Deep purple to violet
        let f = (v - 0.05) / 0.25;
        let r = (40.0 + f * 70.0) as u8;
        let g = (10.0 + f * 15.0) as u8;
        let b = (70.0 + f * 80.0) as u8;
        Color32::from_rgba_unmultiplied(r, g, b, 120)
    } else if v < 0.65 {
        // Violet to crimson/red-orange
        let f = (v - 0.30) / 0.35;
        let r = (110.0 + f * 110.0) as u8;
        let g = (25.0 + f * 55.0) as u8;
        let b = (150.0 - f * 110.0) as u8;
        Color32::from_rgba_unmultiplied(r, g, b, 180)
    } else if v < 0.88 {
        // Crimson to bright amber/gold
        let f = (v - 0.65) / 0.23;
        let r = (220.0 + f * 30.0) as u8;
        let g = (80.0 + f * 110.0) as u8;
        let b = (40.0 - f * 20.0) as u8;
        Color32::from_rgba_unmultiplied(r, g, b, 220)
    } else {
        // Gold to radiant white-yellow
        let f = (v - 0.88) / 0.12;
        let r = 255;
        let g = (190.0 + f * 60.0) as u8;
        let b = (20.0 + f * 180.0) as u8;
        Color32::from_rgba_unmultiplied(r, g, b, 245)
    }
}

fn draw_audio_waveform(
    painter: &Painter,
    rect: Rect,
    state: &TimelineState,
    waveform: Option<&crate::audio::AudioWaveform>,
) {
    let wf = match waveform {
        Some(w) if !w.is_empty() => w,
        _ => return,
    };

    let base_y = TimelineState::pos_to_screen_y(0, rect);
    let max_height = (rect.height() * 0.38).min(110.0);

    match state.audio_viz_mode {
        AudioVizMode::Waveform => {
            let bar_color = Color32::from_rgba_unmultiplied(0, 180, 240, 65);
            let mut mesh = Mesh::default();
            let mut x = rect.left();
            let mut transients = Vec::new();

            while x <= rect.right() {
                let time_ms = state.screen_x_to_time(x, rect);
                let peak = wf.get_peak_at(time_ms);
                if peak > 0.02 {
                    let bar_h = peak * max_height;
                    let bar_rect = Rect::from_min_max(pos2(x - 0.9, base_y - bar_h), pos2(x + 0.9, base_y));
                    mesh.add_colored_rect(bar_rect, bar_color);

                    if peak >= 0.60 {
                        transients.push(pos2(x, base_y - bar_h - 2.0));
                    }
                }
                x += 2.0;
            }
            if !mesh.is_empty() {
                painter.add(Shape::Mesh(mesh.into()));
            }

            for p in transients {
                painter.circle_filled(p, 1.8, Color32::from_rgb(255, 205, 70));
            }
        }
        AudioVizMode::MultiBand => {
            let sub_color = Color32::from_rgba_unmultiplied(225, 45, 125, 80);  // Deep Magenta
            let mid_color = Color32::from_rgba_unmultiplied(0, 215, 215, 70);   // Cyan/Teal
            let high_color = Color32::from_rgba_unmultiplied(255, 195, 40, 90); // Gold/Amber

            let mut mesh = Mesh::default();
            let mut x = rect.left();
            let mut kicks = Vec::new();

            while x <= rect.right() {
                let time_ms = state.screen_x_to_time(x, rect);
                let sub = wf.get_sub_bass_at(time_ms);
                let mid = wf.get_mid_at(time_ms);
                let high = wf.get_high_at(time_ms);

                if sub > 0.02 {
                    let h_sub = sub * max_height;
                    let r_sub = Rect::from_min_max(pos2(x - 0.9, base_y - h_sub), pos2(x + 0.9, base_y));
                    mesh.add_colored_rect(r_sub, sub_color);
                    if sub >= 0.55 {
                        kicks.push(pos2(x, base_y - h_sub - 2.5));
                    }
                }
                if mid > 0.03 {
                    let h_mid = mid * max_height * 0.85;
                    let r_mid = Rect::from_min_max(pos2(x - 0.9, base_y - h_mid), pos2(x + 0.9, base_y));
                    mesh.add_colored_rect(r_mid, mid_color);
                }
                if high > 0.03 {
                    let h_high = high * max_height * 0.65;
                    let r_high = Rect::from_min_max(pos2(x - 0.9, base_y - h_high), pos2(x + 0.9, base_y));
                    mesh.add_colored_rect(r_high, high_color);
                }
                x += 2.0;
            }
            if !mesh.is_empty() {
                painter.add(Shape::Mesh(mesh.into()));
            }

            for p in kicks {
                painter.circle_filled(p, 2.0, Color32::from_rgb(255, 60, 160));
            }
        }
        AudioVizMode::Spectrogram => {
            let mut mesh = Mesh::default();
            let mut x = rect.left();
            let cell_h = (max_height / 16.0).max(1.5);

            while x <= rect.right() {
                let time_ms = state.screen_x_to_time(x, rect);
                if let Some(spec) = wf.get_spectrogram_at(time_ms) {
                    for b in 0..16 {
                        let val = spec[b];
                        if val > 0.04 {
                            let color = inferno_color(val);
                            let y_top = base_y - (b + 1) as f32 * cell_h;
                            let y_bot = base_y - b as f32 * cell_h;
                            let cell_rect = Rect::from_min_max(pos2(x - 1.2, y_top), pos2(x + 1.2, y_bot));
                            mesh.add_colored_rect(cell_rect, color);
                        }
                    }
                }
                x += 2.5;
            }
            if !mesh.is_empty() {
                painter.add(Shape::Mesh(mesh.into()));
            }
        }
    }
}

fn draw_action_curve(
    painter: &Painter,
    rect: Rect,
    script: &Funscript,
    state: &TimelineState,
    _doctor_report: Option<&DoctorReport>,
    ghost_axes: Option<&[(&str, &Funscript, Color32)]>,
    active_color: Color32,
) {
    let view_start = state.view_start_ms as i64;
    let view_end = (state.view_start_ms + state.view_duration_ms) as i64;

    // 1. Draw Inactive Ghost Axis Curves if enabled (culled to visible range)
    if let Some(ghosts) = ghost_axes {
        for (_name, ghost_script, col) in ghosts {
            if ghost_script.actions.len() < 2 {
                continue;
            }
            let start_k = ghost_script.actions.partition_point(|a| a.at < view_start).saturating_sub(1);
            let end_k = (ghost_script.actions.partition_point(|a| a.at <= view_end) + 1).min(ghost_script.actions.len());
            let ghost_slice = &ghost_script.actions[start_k..end_k];
            if ghost_slice.len() < 2 {
                continue;
            }

            let ghost_color = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 65);
            let mut prev_pt: Option<Pos2> = None;

            for a in ghost_slice {
                let x = state.time_to_screen_x(a.at as f64, rect);
                let y = TimelineState::pos_to_screen_y(a.pos, rect);
                let curr_pt = pos2(x, y);

                if let Some(p0) = prev_pt {
                    if !((p0.x < rect.left() && curr_pt.x < rect.left()) || (p0.x > rect.right() && curr_pt.x > rect.right())) {
                        painter.line_segment([p0, curr_pt], Stroke::new(1.2f32, ghost_color));
                    }
                }
                prev_pt = Some(curr_pt);
            }
        }
    }

    // 2. Draw Active Channel Curve (culled to visible slice with zero heap allocations)
    if script.actions.is_empty() {
        return;
    }

    let actions = &script.actions;
    let start_idx = actions.partition_point(|a| a.at < view_start).saturating_sub(1);
    let end_idx = (actions.partition_point(|a| a.at <= view_end) + 1).min(actions.len());
    if start_idx >= end_idx {
        return;
    }
    let visible_slice = &actions[start_idx..end_idx];

    // Draw connecting line segments with speed violation detection directly on visible slice
    for j in 1..visible_slice.len() {
        let a0 = &visible_slice[j - 1];
        let a1 = &visible_slice[j];
        let p0 = pos2(state.time_to_screen_x(a0.at as f64, rect), TimelineState::pos_to_screen_y(a0.pos, rect));
        let p1 = pos2(state.time_to_screen_x(a1.at as f64, rect), TimelineState::pos_to_screen_y(a1.pos, rect));

        if (p0.x < rect.left() && p1.x < rect.left()) || (p0.x > rect.right() && p1.x > rect.right()) {
            continue;
        }

        let dt = (a1.at - a0.at) as f64 / 1000.0;
        let speed = if dt > 0.0 { (a1.pos - a0.pos).abs() as f64 / dt } else { 0.0 };

        let stroke_color = if speed > 450.0 {
            Color32::from_rgb(255, 50, 50)
        } else if speed > 350.0 {
            Color32::from_rgb(255, 180, 50)
        } else {
            active_color
        };

        painter.line_segment([p0, p1], Stroke::new(2.0f32, stroke_color));
    }

    // Draw keyframe nodes on visible slice
    for (rel_i, a) in visible_slice.iter().enumerate() {
        let actual_idx = start_idx + rel_i;
        let x = state.time_to_screen_x(a.at as f64, rect);
        if x < rect.left() - 10.0 || x > rect.right() + 10.0 {
            continue;
        }
        let y = TimelineState::pos_to_screen_y(a.pos, rect);
        let p = pos2(x, y);

        let is_selected = state.is_selected(actual_idx);
        let radius = if is_selected { 5.5 } else { 3.5 };
        let fill = if is_selected {
            Color32::from_rgb(255, 220, 50)
        } else {
            Color32::from_rgb(255, 255, 255)
        };

        painter.circle_filled(p, radius, fill);
        if is_selected {
            painter.circle_stroke(p, radius + 2.0, Stroke::new(1.5f32, Color32::from_rgb(255, 180, 0)));
        }
    }
}

fn handle_point_interactions(
    ui: &mut Ui,
    response: &Response,
    rect: Rect,
    script: &mut Funscript,
    state: &mut TimelineState,
    waveform: Option<&crate::audio::AudioWaveform>,
    mut undo_history: Option<&mut crate::funscript::UndoHistory>,
) {
    let pointer = response.interact_pointer_pos();
    let shift_held = ui.input(|i| i.modifiers.shift);
    let ctrl_held = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);

    // 1. Hit-test keyframe points for selection and dragging using binary search time radius
    if let Some(pos) = pointer {
        let click_time = state.screen_x_to_time(pos.x, rect);
        let time_radius = ((14.0 / rect.width().max(1.0)) as f64 * state.view_duration_ms).max(20.0) as i64;
        let search_start = script.actions.partition_point(|a| a.at < click_time - time_radius);
        let search_end = (script.actions.partition_point(|a| a.at <= click_time + time_radius) + 1).min(script.actions.len());

        let mut hit: Option<usize> = None;
        for i in search_start..search_end {
            let a = &script.actions[i];
            let px = state.time_to_screen_x(a.at as f64, rect);
            let py = TimelineState::pos_to_screen_y(a.pos, rect);
            let dist = ((px - pos.x).powi(2) + (py - pos.y).powi(2)).sqrt();
            if dist < 12.0 {
                hit = Some(i);
                break;
            }
        }

        if response.clicked() {
            if let Some(idx) = hit {
                if shift_held {
                    state.toggle_selection(idx);
                } else {
                    state.select_single(idx);
                }
                state.cursor_time_ms = script.actions[idx].at;
            } else if !shift_held {
                state.clear_selection();
                state.cursor_time_ms = click_time.max(0);
            }
        }

        // Double click: insert new keyframe (with magnetic audio snapping if enabled)
        if response.double_clicked() {
            let mut t = state.screen_x_to_time(pos.x, rect);
            let p = TimelineState::screen_y_to_pos(pos.y, rect);

            if state.magnetic_snapping {
                if let Some(wf) = waveform {
                    if let Some(snap) = wf.find_nearest_transient(t, 35) {
                        t = snap;
                    }
                }
            }

            if let Some(ref mut uh) = undo_history {
                uh.push_snapshot(script);
            }

            script.actions.push(Action { at: t, pos: p });
            script.sanitize();

            if let Some(new_idx) = script.actions.iter().position(|a| a.at == t) {
                state.select_single(new_idx);
            }
            state.cursor_time_ms = t;
        }

        // Drag start & continuous drag handling
        if response.drag_started_by(eframe::egui::PointerButton::Primary) && !ctrl_held {
            if let Some(idx) = hit {
                if !state.is_selected(idx) {
                    if shift_held {
                        state.toggle_selection(idx);
                    } else {
                        state.select_single(idx);
                    }
                }
                state.is_dragging_point = true;
                state.marquee_start = None;
                state.marquee_current = None;
                if let Some(ref mut uh) = undo_history {
                    uh.push_snapshot(script);
                }
            } else if shift_held {
                state.marquee_start = Some(pos);
                state.marquee_current = Some(pos);
            }
        }

        if response.dragged_by(eframe::egui::PointerButton::Primary) && !ctrl_held {
            if state.is_dragging_point {
                if let Some(lead_idx) = state.selected_index {
                    if lead_idx < script.actions.len() {
                        let mut new_t = state.screen_x_to_time(pos.x, rect);
                        let new_p = TimelineState::screen_y_to_pos(pos.y, rect);

                        if state.magnetic_snapping {
                            if let Some(wf) = waveform {
                                if let Some(snap) = wf.find_nearest_transient(new_t, 35) {
                                    new_t = snap;
                                    state.snapped_transient_ms = Some(snap);
                                } else {
                                    state.snapped_transient_ms = None;
                                }
                            }
                        } else {
                            state.snapped_transient_ms = None;
                        }

                        let dt = new_t - script.actions[lead_idx].at;
                        let dp = new_p - script.actions[lead_idx].pos;

                        if dt != 0 || dp != 0 {
                            for &idx in &state.selected_indices {
                                if idx < script.actions.len() {
                                    script.actions[idx].at = (script.actions[idx].at + dt).max(0);
                                    script.actions[idx].pos = (script.actions[idx].pos + dp).clamp(0, 100);
                                }
                            }
                            state.cursor_time_ms = new_t;
                        }
                    }
                }
            } else if let Some(m_start) = state.marquee_start {
                state.marquee_current = Some(pos);
                let m_rect = Rect::from_two_pos(m_start, pos);

                let view_start = state.view_start_ms as i64;
                let view_end = (state.view_start_ms + state.view_duration_ms) as i64;
                let start_idx = script.actions.partition_point(|a| a.at < view_start).saturating_sub(1);
                let end_idx = (script.actions.partition_point(|a| a.at <= view_end) + 1).min(script.actions.len());

                for i in start_idx..end_idx {
                    let a = &script.actions[i];
                    let kx = state.time_to_screen_x(a.at as f64, rect);
                    let ky = TimelineState::pos_to_screen_y(a.pos, rect);
                    if m_rect.contains(pos2(kx, ky)) {
                        state.selected_indices.insert(i);
                        if state.selected_index.is_none() {
                            state.selected_index = Some(i);
                        }
                    }
                }
            } else if state.selected_indices.is_empty() {
                // Dragging empty canvas without Shift: scrub cursor time
                state.cursor_time_ms = state.screen_x_to_time(pos.x, rect).max(0);
            }
        }

        // Drag release
        if response.drag_stopped() {
            if state.is_dragging_point {
                state.is_dragging_point = false;
                state.snapped_transient_ms = None;

                let selected_ats: Vec<i64> = state
                    .selected_indices
                    .iter()
                    .filter_map(|&i| script.actions.get(i).map(|a| a.at))
                    .collect();
                script.sanitize();
                state.selected_indices = script
                    .actions
                    .iter()
                    .enumerate()
                    .filter_map(|(i, a)| if selected_ats.contains(&a.at) { Some(i) } else { None })
                    .collect();
                state.selected_index = state.selected_indices.iter().next().copied();
            }

            if state.marquee_start.is_some() {
                state.marquee_start = None;
                state.marquee_current = None;
                state.selected_index = state.selected_indices.iter().next().copied();
            }
        }

        // Right-click: Delete hovered keyframe or selection
        if response.secondary_clicked() {
            if let Some(idx) = hit {
                if let Some(ref mut uh) = undo_history {
                    uh.push_snapshot(script);
                }
                if state.selected_indices.contains(&idx) && state.selected_indices.len() > 1 {
                    for &i in state.selected_indices.iter().rev() {
                        if i < script.actions.len() {
                            script.actions.remove(i);
                        }
                    }
                    state.clear_selection();
                } else {
                    script.actions.remove(idx);
                    state.clear_selection();
                }
            }
        }
    }
}

fn calculate_time_step_ms(duration_ms: f64) -> f64 {
    let raw_step = duration_ms / 8.0;
    if raw_step <= 200.0 {
        100.0
    } else if raw_step <= 500.0 {
        500.0
    } else if raw_step <= 1000.0 {
        1000.0
    } else if raw_step <= 5000.0 {
        5000.0
    } else if raw_step <= 10000.0 {
        10000.0
    } else if raw_step <= 30000.0 {
        30000.0
    } else {
        60000.0
    }
}

fn format_timestamp_ms(ms: i64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    let remainder_ms = (ms % 1000).abs();
    format!("{:02}:{:02}.{:03}", mins, secs, remainder_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeline_coord_roundtrip() {
        let state = TimelineState {
            view_start_ms: 1000.0,
            view_duration_ms: 5000.0,
            ..Default::default()
        };
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 300.0));

        // Center of viewport (3500 ms) should map to center of rect (500 px)
        let sx = state.time_to_screen_x(3500.0, rect);
        assert!((sx - 500.0).abs() < 1.0);

        let t_roundtrip = state.screen_x_to_time(sx, rect);
        assert!((t_roundtrip - 3500).abs() <= 1);
    }

    #[test]
    fn test_pos_to_screen_y_clamping() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 300.0));
        let y_100 = TimelineState::pos_to_screen_y(100, rect);
        let y_0 = TimelineState::pos_to_screen_y(0, rect);
        assert!(y_100 < y_0, "Position 100 must be higher (lower Y in screen coords) than position 0");

        let pos_roundtrip = TimelineState::screen_y_to_pos(y_100, rect);
        assert_eq!(pos_roundtrip, 100);
    }

    #[test]
    fn test_timeline_state_defaults() {
        let state = TimelineState::default();
        assert!(state.magnetic_snapping);
        assert_eq!(state.snapped_transient_ms, None);
        assert!(state.selected_indices.is_empty());
        assert_eq!(state.selected_index, None);
    }

    #[test]
    fn test_timeline_state_multi_selection() {
        let mut state = TimelineState::default();
        state.select_single(3);
        assert_eq!(state.selected_index, Some(3));
        assert!(state.is_selected(3));
        assert!(!state.is_selected(4));
        assert_eq!(state.selected_indices.len(), 1);

        state.toggle_selection(5);
        assert!(state.is_selected(3));
        assert!(state.is_selected(5));
        assert_eq!(state.selected_indices.len(), 2);

        state.toggle_selection(3);
        assert!(!state.is_selected(3));
        assert!(state.is_selected(5));
        assert_eq!(state.selected_index, Some(5));

        state.select_all(10);
        assert_eq!(state.selected_indices.len(), 10);
        for i in 0..10 {
            assert!(state.is_selected(i));
        }

        state.clear_selection();
        assert!(state.selected_indices.is_empty());
        assert_eq!(state.selected_index, None);
    }

    #[test]
    fn test_timeline_loop_and_bookmarks() {
        let mut state = TimelineState::default();
        assert!(!state.loop_enabled);
        assert_eq!(state.loop_in_ms, None);
        assert_eq!(state.loop_out_ms, None);

        state.set_loop_in(1000);
        assert_eq!(state.loop_in_ms, Some(1000));
        assert!(state.loop_enabled);

        state.set_loop_out(3500);
        assert_eq!(state.loop_out_ms, Some(3500));
        assert!(state.loop_enabled);

        // Setting in > out auto-adjusts (4000 + 1000 = 5000)
        state.set_loop_in(4000);
        assert_eq!(state.loop_in_ms, Some(4000));
        assert_eq!(state.loop_out_ms, Some(5000));

        state.clear_loop();
        assert!(!state.loop_enabled);
        assert_eq!(state.loop_in_ms, None);
        assert_eq!(state.loop_out_ms, None);

        // Bookmarks
        state.add_bookmark(1500, "Drop 1".into());
        state.add_bookmark(500, "Intro".into());
        state.add_bookmark(3000, "Climax".into());

        // Should be ordered by time_ms: 500, 1500, 3000
        assert_eq!(state.bookmarks[0].time_ms, 500);
        assert_eq!(state.bookmarks[1].time_ms, 1500);
        assert_eq!(state.bookmarks[2].time_ms, 3000);

        assert_eq!(state.next_bookmark(0), Some(500));
        assert_eq!(state.next_bookmark(500), Some(1500));
        assert_eq!(state.next_bookmark(2000), Some(3000));
        assert_eq!(state.next_bookmark(3500), None);

        assert_eq!(state.prev_bookmark(4000), Some(3000));
        assert_eq!(state.prev_bookmark(3000), Some(1500));
        assert_eq!(state.prev_bookmark(1000), Some(500));
        assert_eq!(state.prev_bookmark(500), None);
    }

    #[test]
    fn test_audio_viz_mode_lifecycle() {
        let mut state = TimelineState::default();
        assert_eq!(state.audio_viz_mode, AudioVizMode::Waveform);
        assert_eq!(AudioVizMode::ALL.len(), 3);

        state.cycle_audio_viz_mode();
        assert_eq!(state.audio_viz_mode, AudioVizMode::MultiBand);
        assert_eq!(state.audio_viz_mode.display_name(), "Multi-Band Split");
        assert_eq!(state.audio_viz_mode.icon(), "🌈");

        state.cycle_audio_viz_mode();
        assert_eq!(state.audio_viz_mode, AudioVizMode::Spectrogram);
        assert_eq!(state.audio_viz_mode.display_name(), "Waterfall Spectrogram");
        assert_eq!(state.audio_viz_mode.icon(), "⚡");

        state.cycle_audio_viz_mode();
        assert_eq!(state.audio_viz_mode, AudioVizMode::Waveform);
        assert_eq!(state.audio_viz_mode.display_name(), "Waveform (RMS)");
        assert_eq!(state.audio_viz_mode.icon(), "🎵");
    }
}

