//! Kinematic forward solver and 3D geometry model for OSR2 and SR6 robotic rigs.

/// Simple 3D point / vector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    #[allow(dead_code)]
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }

    #[allow(dead_code)]
    pub fn dist(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Rotate point around X axis (pitch)
    pub fn rotate_x(self, angle_rad: f32) -> Self {
        let cos = angle_rad.cos();
        let sin = angle_rad.sin();
        Self {
            x: self.x,
            y: self.y * cos - self.z * sin,
            z: self.y * sin + self.z * cos,
        }
    }

    /// Rotate point around Y axis (roll)
    pub fn rotate_y(self, angle_rad: f32) -> Self {
        let cos = angle_rad.cos();
        let sin = angle_rad.sin();
        Self {
            x: self.x * cos + self.z * sin,
            y: self.y,
            z: -self.x * sin + self.z * cos,
        }
    }

    /// Rotate point around Z axis (yaw/twist)
    pub fn rotate_z(self, angle_rad: f32) -> Self {
        let cos = angle_rad.cos();
        let sin = angle_rad.sin();
        Self {
            x: self.x * cos - self.y * sin,
            y: self.x * sin + self.y * cos,
            z: self.z,
        }
    }
}

/// Supported hardware robotic rig architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RigModel {
    #[default]
    OSR2,
    SR6,
}

impl RigModel {
    #[allow(dead_code)]
    pub const ALL: [Self; 2] = [Self::OSR2, Self::SR6];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::OSR2 => "OSR2 (3-Axis Pitch/Roll)",
            Self::SR6 => "SR6 (6-DOF Full Stewart)",
        }
    }
}

/// Solved 3D geometry of the robot mechanism ready for rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct RigGeometry {
    pub model: RigModel,
    /// Base mounting anchors / servo rotation axes
    pub base_anchors: Vec<Vec3>,
    /// Servo horn tips / elbow joints
    pub arm_joints: Vec<Vec3>,
    /// Connection joints on the receiver plate
    pub receiver_joints: Vec<Vec3>,
    /// Center of the output receiver cup
    pub receiver_center: Vec3,
    /// Orientation angles (pitch, roll, yaw) in radians
    pub rotation_rad: (f32, f32, f32),
    /// Stroke displacement in percentage [0, 100]
    pub stroke_pct: f32,
    /// True if any linkage or servo is near mechanical endstops (>92% or <8%)
    pub is_near_limit: bool,
    /// Active warnings
    pub warnings: Vec<String>,
}

/// Inputs to the kinematic solver from funscript channels [0.0, 100.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigInput {
    pub stroke: f32, // L0 (0..100)
    pub surge: f32,  // L1 (0..100, 50 neutral)
    pub sway: f32,   // L2 (0..100, 50 neutral)
    pub twist: f32,  // R0 (0..100, 50 neutral)
    pub roll: f32,   // R1 (0..100, 50 neutral)
    pub pitch: f32,  // R2 (0..100, 50 neutral)
}

impl Default for RigInput {
    fn default() -> Self {
        Self {
            stroke: 50.0,
            surge: 50.0,
            sway: 50.0,
            twist: 50.0,
            roll: 50.0,
            pitch: 50.0,
        }
    }
}

impl RigGeometry {
    /// Solve forward kinematics for the given rig model and channel inputs.
    pub fn solve(model: RigModel, input: &RigInput) -> Self {
        match model {
            RigModel::OSR2 => Self::solve_osr2(input),
            RigModel::SR6 => Self::solve_sr6(input),
        }
    }

    fn solve_osr2(input: &RigInput) -> Self {
        let stroke_norm = (input.stroke.clamp(0.0, 100.0) / 100.0) * 80.0 - 40.0; // -40 to +40mm
        let pitch_rad = ((input.pitch.clamp(0.0, 100.0) - 50.0) / 50.0) * 0.40; // +- 23 deg
        let roll_rad = ((input.roll.clamp(0.0, 100.0) - 50.0) / 50.0) * 0.35; // +- 20 deg

        let receiver_center = Vec3::new(0.0, 0.0, stroke_norm);

        // 3 Base anchors (Left, Right, Pitch)
        let base_anchors = vec![
            Vec3::new(-45.0, 0.0, -50.0), // Left servo
            Vec3::new(45.0, 0.0, -50.0),  // Right servo
            Vec3::new(0.0, -40.0, -50.0), // Pitch servo
        ];

        // 3 Receiver joints (relative to receiver center, rotated)
        let local_rcv = [
            Vec3::new(-30.0, 0.0, 0.0),
            Vec3::new(30.0, 0.0, 0.0),
            Vec3::new(0.0, -25.0, 0.0),
        ];

        let receiver_joints: Vec<Vec3> = local_rcv
            .iter()
            .map(|&p| {
                let rot = p.rotate_x(pitch_rad).rotate_y(roll_rad);
                receiver_center.add(rot)
            })
            .collect();

        // Elbow joints (interpolated 50% between base and receiver + outward offset)
        let arm_joints = vec![
            Vec3::new(-52.0, 0.0, (base_anchors[0].z + receiver_joints[0].z) * 0.5),
            Vec3::new(52.0, 0.0, (base_anchors[1].z + receiver_joints[1].z) * 0.5),
            Vec3::new(0.0, -48.0, (base_anchors[2].z + receiver_joints[2].z) * 0.5),
        ];

        let mut warnings = Vec::new();
        let is_near_limit = input.stroke > 92.0
            || input.stroke < 8.0
            || input.pitch > 90.0
            || input.pitch < 10.0
            || input.roll > 90.0
            || input.roll < 10.0;

        if is_near_limit {
            warnings.push("Travel limit warning: Approaching physical endstop".to_string());
        }

        Self {
            model: RigModel::OSR2,
            base_anchors,
            arm_joints,
            receiver_joints,
            receiver_center,
            rotation_rad: (pitch_rad, roll_rad, 0.0),
            stroke_pct: input.stroke,
            is_near_limit,
            warnings,
        }
    }

    fn solve_sr6(input: &RigInput) -> Self {
        let stroke_norm = (input.stroke.clamp(0.0, 100.0) / 100.0) * 80.0 - 40.0;
        let surge_norm = ((input.surge.clamp(0.0, 100.0) - 50.0) / 50.0) * 25.0; // +- 25mm fore/aft
        let sway_norm = ((input.sway.clamp(0.0, 100.0) - 50.0) / 50.0) * 25.0; // +- 25mm lateral
        let twist_rad = ((input.twist.clamp(0.0, 100.0) - 50.0) / 50.0) * 0.45; // +- 25 deg yaw
        let roll_rad = ((input.roll.clamp(0.0, 100.0) - 50.0) / 50.0) * 0.35;
        let pitch_rad = ((input.pitch.clamp(0.0, 100.0) - 50.0) / 50.0) * 0.40;

        let receiver_center = Vec3::new(sway_norm, surge_norm, stroke_norm);

        // 6 Base anchors arranged around the base circle (radius 55mm)
        let base_radius = 55.0f32;
        let base_angles = [
            -2.5f32, -0.64f32, // Front pair
            0.64f32, 2.5f32,   // Back pair
            -1.57f32, 1.57f32, // Side pair
        ];

        let base_anchors: Vec<Vec3> = base_angles
            .iter()
            .map(|&ang| Vec3::new(base_radius * ang.cos(), base_radius * ang.sin(), -55.0))
            .collect();

        // 6 Receiver joints around receiver ring (radius 32mm)
        let rcv_radius = 32.0f32;
        let rcv_angles = [
            -2.3f32, -0.84f32,
            0.84f32, 2.3f32,
            -1.57f32, 1.57f32,
        ];

        let receiver_joints: Vec<Vec3> = rcv_angles
            .iter()
            .map(|&ang| {
                let local = Vec3::new(rcv_radius * ang.cos(), rcv_radius * ang.sin(), 0.0);
                let rotated = local
                    .rotate_x(pitch_rad)
                    .rotate_y(roll_rad)
                    .rotate_z(twist_rad);
                receiver_center.add(rotated)
            })
            .collect();

        // 6 Arm linkages connecting base to receiver
        let arm_joints: Vec<Vec3> = (0..6)
            .map(|i| {
                let b = base_anchors[i];
                let r = receiver_joints[i];
                Vec3::new((b.x + r.x) * 0.5, (b.y + r.y) * 0.5, (b.z + r.z) * 0.5)
            })
            .collect();

        let mut warnings = Vec::new();
        let is_near_limit = input.stroke > 92.0
            || input.stroke < 8.0
            || input.surge > 90.0
            || input.surge < 10.0
            || input.sway > 90.0
            || input.sway < 10.0
            || input.twist > 90.0
            || input.twist < 10.0;

        if is_near_limit {
            warnings.push("6-DOF Stewart boundary: High actuator extension".to_string());
        }

        Self {
            model: RigModel::SR6,
            base_anchors,
            arm_joints,
            receiver_joints,
            receiver_center,
            rotation_rad: (pitch_rad, roll_rad, twist_rad),
            stroke_pct: input.stroke,
            is_near_limit,
            warnings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3_operations() {
        assert_eq!(Vec3::ZERO.x, 0.0);
        assert_eq!(Vec3::ZERO.y, 0.0);
        assert_eq!(Vec3::ZERO.z, 0.0);

        assert_eq!(RigModel::ALL.len(), 2);
        assert_eq!(RigModel::OSR2.display_name(), "OSR2 (3-Axis Pitch/Roll)");
        assert_eq!(RigModel::SR6.display_name(), "SR6 (6-DOF Full Stewart)");

        let v1 = Vec3::new(10.0, 0.0, 0.0);
        let v2 = Vec3::new(0.0, 10.0, 0.0);
        let dist = v1.dist(v2);
        assert!((dist - 14.142).abs() < 0.01);

        let rot = v1.rotate_z(std::f32::consts::FRAC_PI_2);
        assert!(rot.x.abs() < 1e-5);
        assert!((rot.y - 10.0).abs() < 1e-5);
    }

    #[test]
    fn test_osr2_kinematic_solve() {
        let input = RigInput {
            stroke: 100.0,
            roll: 50.0,
            pitch: 50.0,
            ..Default::default()
        };
        let geom = RigGeometry::solve(RigModel::OSR2, &input);
        assert_eq!(geom.base_anchors.len(), 3);
        assert_eq!(geom.receiver_joints.len(), 3);
        assert_eq!(geom.receiver_center.z, 40.0);
        assert!(geom.is_near_limit); // stroke = 100 is near limit
    }

    #[test]
    fn test_sr6_kinematic_solve() {
        let input = RigInput::default();
        let geom = RigGeometry::solve(RigModel::SR6, &input);
        assert_eq!(geom.base_anchors.len(), 6);
        assert_eq!(geom.receiver_joints.len(), 6);
        assert!(!geom.is_near_limit); // neutral 50% is safe
    }
}
