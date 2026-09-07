//! Interactive immediate-mode timeline widget for egui.

use crate::funscript::{Action, DoctorReport, Funscript};
use eframe::egui::{
    pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui,
};

#[derive(Debug, Clone)]
pub struct TimelineState {
    /// Start of visible time window in milliseconds
    pub view_start_ms: f64,
    /// Duration of visible time window in milliseconds (zoom level)
    pub view_duration_ms: f64,
    /// Current playhead position in milliseconds
    pub cursor_time_ms: i64,
    /// Index of currently selected keyframe action
    pub selected_index: Option<usize>,
    /// Whether a keyframe point is actively being dragged
    pub is_dragging_point: bool,
    /// Whether magnetic snapping to audio waveform transients is enabled
    pub magnetic_snapping: bool,
    /// Currently snapped acoustic transient timestamp (if active)
    pub snapped_transient_ms: Option<i64>,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            view_start_ms: 0.0,
            view_duration_ms: 10_000.0, // Default 10 second window
            cursor_time_ms: 0,
            selected_index: None,
            is_dragging_point: false,
            magnetic_snapping: true,
            snapped_transient_ms: None,
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

    // 2b. Render Audio Waveform (translucent acoustic bars under curve)
    draw_audio_waveform(&painter, rect, state, waveform);

    // 3. Render Action Curve, Ghost Multi-Axis Curves, & Highlights
    draw_action_curve(&painter, rect, script, state, doctor_report, ghost_axes, active_color);

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
    let max_height = (rect.height() * 0.35).min(100.0);
    let bar_color = Color32::from_rgba_unmultiplied(0, 180, 240, 65);

    let mut mesh = Mesh::default();
    let mut x = rect.left();
    while x <= rect.right() {
        let time_ms = state.screen_x_to_time(x, rect);
        let peak = wf.get_peak_at(time_ms);
        if peak > 0.02 {
            let bar_h = peak * max_height;
            let bar_rect = Rect::from_min_max(pos2(x - 0.9, base_y - bar_h), pos2(x + 0.9, base_y));
            mesh.add_colored_rect(bar_rect, bar_color);
        }
        x += 2.0;
    }
    if !mesh.is_empty() {
        painter.add(Shape::Mesh(mesh.into()));
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

        let is_selected = state.selected_index == Some(actual_idx);
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
    _ui: &mut Ui,
    response: &Response,
    rect: Rect,
    script: &mut Funscript,
    state: &mut TimelineState,
    waveform: Option<&crate::audio::AudioWaveform>,
    mut undo_history: Option<&mut crate::funscript::UndoHistory>,
) {
    let pointer = response.interact_pointer_pos();

    // 1. Hit-test keyframe points for selection and dragging using binary search time radius
    if let Some(pos) = pointer {
        if response.clicked() {
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

            state.selected_index = hit;
            if let Some(idx) = hit {
                state.cursor_time_ms = script.actions[idx].at;
            } else {
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

            // Find new index
            state.selected_index = script.actions.iter().position(|a| a.at == t);
            state.cursor_time_ms = t;
        }

        // Dragging a selected keyframe (with magnetic audio snapping if enabled)
        if response.dragged_by(eframe::egui::PointerButton::Primary) {
            if let Some(idx) = state.selected_index {
                if idx < script.actions.len() {
                    if !state.is_dragging_point {
                        if let Some(ref mut uh) = undo_history {
                            uh.push_snapshot(script);
                        }
                        state.is_dragging_point = true;
                    }

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

                    script.actions[idx].at = new_t;
                    script.actions[idx].pos = new_p;
                    state.cursor_time_ms = new_t;
                }
            }
        } else {
            if state.is_dragging_point {
                state.is_dragging_point = false;
                state.snapped_transient_ms = None;
                script.sanitize();
            }
        }

        // Scrub cursor time when primary-dragging empty timeline canvas (without pan modifier)
        if response.dragged_by(eframe::egui::PointerButton::Primary)
            && state.selected_index.is_none()
            && !_ui.input(|i| i.modifiers.command || i.modifiers.ctrl)
        {
            state.cursor_time_ms = state.screen_x_to_time(pos.x, rect).max(0);
        }

        // Right-click: Delete hovered keyframe
        if response.secondary_clicked() {
            let click_time = state.screen_x_to_time(pos.x, rect);
            let time_radius = ((14.0 / rect.width().max(1.0)) as f64 * state.view_duration_ms).max(20.0) as i64;
            let search_start = script.actions.partition_point(|a| a.at < click_time - time_radius);
            let search_end = (script.actions.partition_point(|a| a.at <= click_time + time_radius) + 1).min(script.actions.len());

            let mut delete_idx: Option<usize> = None;
            for i in search_start..search_end {
                let a = &script.actions[i];
                let px = state.time_to_screen_x(a.at as f64, rect);
                let py = TimelineState::pos_to_screen_y(a.pos, rect);
                let dist = ((px - pos.x).powi(2) + (py - pos.y).powi(2)).sqrt();
                if dist < 12.0 {
                    delete_idx = Some(i);
                    break;
                }
            }
            if let Some(idx) = delete_idx {
                if let Some(ref mut uh) = undo_history {
                    uh.push_snapshot(script);
                }
                script.actions.remove(idx);
                state.selected_index = None;
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
    }
}
