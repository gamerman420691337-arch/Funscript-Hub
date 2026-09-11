//! 3D Anatomical Pose Estimation & 6-DOF Kinematic Coordinate Decomposition.

use serde::{Deserialize, Serialize};

/// Instantaneous 6-DOF + Suction anatomical pose estimation
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pose3D {
    /// L0: Primary Stroke penetration depth [0.0, 100.0] (100 = deep insertion)
    pub stroke: f32,
    /// L1: Surge forward/backward translation [0.0, 100.0]
    pub surge: f32,
    /// L2: Sway lateral left/right position [0.0, 100.0] (50.0 = centered)
    pub sway: f32,
    /// R1: Pitch tilt angle in vertical plane [0.0, 100.0] (50.0 = horizontal)
    pub pitch: f32,
    /// R0: Roll lateral incline [0.0, 100.0] (50.0 = neutral)
    pub roll: f32,
    /// R2: Twist axial rotational curl [0.0, 100.0] (50.0 = neutral)
    pub twist: f32,
    /// V0: Suction / contact pressure proxy [0.0, 100.0]
    pub suction: f32,
}

impl Default for Pose3D {
    fn default() -> Self {
        Self {
            stroke: 0.0,
            surge: 50.0,
            sway: 50.0,
            pitch: 50.0,
            roll: 50.0,
            twist: 50.0,
            suction: 0.0,
        }
    }
}

impl Pose3D {
    /// Calculate instantaneous 3D pose from anatomical landmarks and optical flow vorticity
    pub fn from_landmarks(
        probe_center: (f32, f32),
        probe_bbox: [f32; 4],
        target_center: (f32, f32),
        target_bbox: [f32; 4],
        vorticity: f32,
        stroke_pos: f32,
    ) -> Self {
        // 1. Stroke (L0): Distance tracker with occlusion recovery
        let stroke = stroke_pos.clamp(0.0, 100.0);

        // 2. Surge (L1): Relative depth scaling based on target bbox area
        let target_w = (target_bbox[2] - target_bbox[0]).abs();
        let target_h = (target_bbox[3] - target_bbox[1]).abs();
        let area = (target_w * target_h).sqrt();
        let surge = ((area - 0.1) / 0.5 * 100.0).clamp(0.0, 100.0);

        // 3. Sway (L2): Lateral offset dx = probe_x - target_x mapped around 50
        let dx = probe_center.0 - target_center.0;
        let sway = (50.0 + dx * 200.0).clamp(0.0, 100.0);

        // 4. Pitch (R1): Angular incline of penetration vector (probe -> target)
        let dy = probe_center.1 - target_center.1;
        let angle_rad = dy.atan2(dx);
        let angle_norm = (angle_rad / std::f32::consts::PI + 1.0) * 50.0;
        let pitch = angle_norm.clamp(0.0, 100.0);

        // 5. Roll (R0): Target bbox aspect ratio or tilt
        let aspect_ratio = if target_h > 0.01 { target_w / target_h } else { 1.0 };
        let roll = (50.0 + (aspect_ratio - 1.0) * 50.0).clamp(0.0, 100.0);

        // 6. Twist (R2): Optical flow vorticity (curl) mapped around 50
        let twist = (50.0 + vorticity * 50.0).clamp(0.0, 100.0);

        // 7. Suction (V0): Contact pressure derived from bbox overlap and stroke depth
        let x_overlap = (probe_bbox[2].min(target_bbox[2]) - probe_bbox[0].max(target_bbox[0])).max(0.0);
        let y_overlap = (probe_bbox[3].min(target_bbox[3]) - probe_bbox[1].max(target_bbox[1])).max(0.0);
        let overlap_area = x_overlap * y_overlap;
        let suction = ((overlap_area * 1000.0).min(50.0) + stroke * 0.5).clamp(0.0, 100.0);

        Self {
            stroke,
            surge,
            sway,
            pitch,
            roll,
            twist,
            suction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pose3d_defaults_and_symmetry() {
        let pose = Pose3D::default();
        assert_eq!(pose.stroke, 0.0);
        assert_eq!(pose.sway, 50.0);
        assert_eq!(pose.pitch, 50.0);
        assert_eq!(pose.twist, 50.0);

        let p_probe = (0.5, 0.5);
        let b_probe = [0.45, 0.45, 0.55, 0.55];
        let p_target = (0.5, 0.5);
        let b_target = [0.4, 0.4, 0.6, 0.6];

        let pose_calc = Pose3D::from_landmarks(p_probe, b_probe, p_target, b_target, 0.0, 80.0);
        assert_eq!(pose_calc.stroke, 80.0);
        assert_eq!(pose_calc.sway, 50.0);
        assert_eq!(pose_calc.twist, 50.0);
        assert!(pose_calc.suction > 0.0);
    }
}
