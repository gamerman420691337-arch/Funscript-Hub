//! Interactive 3D vector-rendered hardware simulator widget for OSR2 & SR6 robotic rigs.

use crate::kinematics::rig::{RigGeometry, RigInput, RigModel, Vec3};
use eframe::egui::{
    pos2, vec2, Color32, Pos2, Rect, Response, Sense, Stroke, Ui,
};

/// 3D Camera / Viewport state for the rig simulator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigViewState {
    pub yaw_rad: f32,
    pub pitch_rad: f32,
    pub distance: f32,
    pub fov: f32,
}

impl Default for RigViewState {
    fn default() -> Self {
        Self {
            yaw_rad: -0.65,   // ~-37 degrees isometric
            pitch_rad: 0.40,  // ~23 degrees elevation
            distance: 220.0,
            fov: 180.0,
        }
    }
}

impl RigViewState {
    /// Project a 3D world coordinate into a 2D screen coordinate within the designated rect.
    pub fn project(&self, point: Vec3, center: Pos2, scale: f32) -> (Pos2, f32) {
        // Rotate around origin by yaw then pitch
        let p_yaw = point.rotate_z(self.yaw_rad);
        let p_cam = p_yaw.rotate_x(self.pitch_rad);

        // Simple perspective projection
        let z_depth = (self.distance + p_cam.y).max(10.0);
        let factor = (self.fov / z_depth) * scale;

        let screen_x = center.x + p_cam.x * factor;
        // Invert Y because screen Y goes downwards
        let screen_y = center.y - p_cam.z * factor;

        (pos2(screen_x, screen_y), z_depth)
    }
}

/// Renders the interactive 3D robot rig simulator into the UI.
pub fn show_rig_simulator(
    ui: &mut Ui,
    model: RigModel,
    input: &RigInput,
    view_state: &mut RigViewState,
    size: eframe::egui::Vec2,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::drag());

    // Dragging mouse rotates yaw and pitch
    if response.dragged() {
        let delta = response.drag_delta();
        view_state.yaw_rad += delta.x * 0.01;
        view_state.pitch_rad = (view_state.pitch_rad - delta.y * 0.01).clamp(-1.2, 1.2);
    }

    let painter = ui.painter_at(rect);
    let center = rect.center();
    let scale = (rect.width().min(rect.height()) / 180.0).max(0.6);

    // Dark technical background card with subtle grid
    painter.rect_filled(rect, 6.0, Color32::from_rgb(18, 22, 28));
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0f32, Color32::from_rgb(45, 55, 70)), eframe::egui::StrokeKind::Middle);

    // Solve kinematics
    let geom = RigGeometry::solve(model, input);

    // Color definitions
    let base_color = Color32::from_rgb(90, 105, 125);
    let arm_color = if geom.is_near_limit {
        Color32::from_rgb(255, 80, 80)
    } else {
        Color32::from_rgb(0, 200, 255)
    };
    let joint_color = Color32::from_rgb(255, 200, 50);
    let rcv_color = if geom.is_near_limit {
        Color32::from_rgb(255, 100, 100)
    } else {
        Color32::from_rgb(50, 220, 140)
    };

    // 1. Draw Base Platform
    let mut base_screen_pts = Vec::new();
    for &b in &geom.base_anchors {
        let (s_pos, _) = view_state.project(b, center, scale);
        base_screen_pts.push(s_pos);
        // Base anchor dots
        painter.circle_filled(s_pos, 4.0 * scale, base_color);
    }

    // Connect base anchors into closed ring/triangle
    if base_screen_pts.len() >= 3 {
        for i in 0..base_screen_pts.len() {
            let next_i = (i + 1) % base_screen_pts.len();
            painter.line_segment(
                [base_screen_pts[i], base_screen_pts[next_i]],
                Stroke::new(1.5f32 * scale, base_color),
            );
        }
    }

    // 2. Draw Linkages & Servo Horns
    for i in 0..geom.base_anchors.len() {
        let b = geom.base_anchors[i];
        let j = geom.arm_joints[i];
        let r = geom.receiver_joints[i];

        let (s_b, _) = view_state.project(b, center, scale);
        let (s_j, _) = view_state.project(j, center, scale);
        let (s_r, _) = view_state.project(r, center, scale);

        // Lower arm (base -> elbow)
        painter.line_segment([s_b, s_j], Stroke::new(2.5f32 * scale, arm_color));
        // Elbow joint pin
        painter.circle_filled(s_j, 3.5 * scale, joint_color);
        // Upper pushrod linkage (elbow -> receiver)
        painter.line_segment([s_j, s_r], Stroke::new(1.8f32 * scale, Color32::from_rgb(220, 230, 245)));
        // Receiver connection joint
        painter.circle_filled(s_r, 3.0 * scale, joint_color);
    }

    // 3. Draw Output Receiver Ring & Orientation Vector
    let mut rcv_screen_pts = Vec::new();
    for &r in &geom.receiver_joints {
        let (s_r, _) = view_state.project(r, center, scale);
        rcv_screen_pts.push(s_r);
    }

    // Connect receiver perimeter
    if rcv_screen_pts.len() >= 3 {
        for i in 0..rcv_screen_pts.len() {
            let next_i = (i + 1) % rcv_screen_pts.len();
            painter.line_segment(
                [rcv_screen_pts[i], rcv_screen_pts[next_i]],
                Stroke::new(2.5f32 * scale, rcv_color),
            );
        }
    }

    // Output receiver center & normal thrust vector
    let (s_rcv_center, _) = view_state.project(geom.receiver_center, center, scale);
    painter.circle_filled(s_rcv_center, 5.0 * scale, rcv_color);

    let tip = geom.receiver_center.add(Vec3::new(0.0, 0.0, 25.0));
    let (s_tip, _) = view_state.project(tip, center, scale);
    painter.line_segment([s_rcv_center, s_tip], Stroke::new(2.0f32 * scale, Color32::WHITE));

    // 4. On-Screen Telemetry Overlay
    let text_x = rect.left() + 8.0;
    let mut text_y = rect.top() + 8.0;

    let title = format!("{} 3D Sim", model.display_name());
    painter.text(
        pos2(text_x, text_y),
        eframe::egui::Align2::LEFT_TOP,
        title,
        eframe::egui::FontId::proportional(11.0),
        Color32::from_rgb(200, 215, 235),
    );
    text_y += 14.0;

    let status_str = format!("L0 Stroke: {:.0}%", geom.stroke_pct);
    let status_color = if geom.is_near_limit {
        Color32::from_rgb(255, 90, 90)
    } else {
        Color32::from_rgb(60, 220, 100)
    };

    painter.text(
        pos2(text_x, text_y),
        eframe::egui::Align2::LEFT_TOP,
        status_str,
        eframe::egui::FontId::proportional(10.0),
        status_color,
    );

    if geom.is_near_limit {
        text_y += 13.0;
        painter.text(
            pos2(text_x, text_y),
            eframe::egui::Align2::LEFT_TOP,
            "⚠ Endstop Limit",
            eframe::egui::FontId::proportional(9.5),
            Color32::from_rgb(255, 80, 80),
        );
    }

    // Reset view button on bottom-right of viewport
    let reset_rect = Rect::from_min_size(
        pos2(rect.right() - 44.0, rect.bottom() - 20.0),
        vec2(40.0, 16.0),
    );
    if ui.put(reset_rect, eframe::egui::Button::new("Reset").small()).clicked() {
        *view_state = RigViewState::default();
    }

    response
}
