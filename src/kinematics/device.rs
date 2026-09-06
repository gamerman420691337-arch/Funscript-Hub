//! Physical device kinematics modeling and servo constraint evaluation.

use crate::funscript::AxisChannel;
use std::collections::HashMap;

/// Supported physical device types
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeviceType {
    TheHandy,
    OSR2Plus,
    SR6,
    Keon,
    Custom,
}

impl DeviceType {
    pub const ALL: [DeviceType; 5] = [
        DeviceType::TheHandy,
        DeviceType::OSR2Plus,
        DeviceType::SR6,
        DeviceType::Keon,
        DeviceType::Custom,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            DeviceType::TheHandy => "The Handy (1-DOF Linear)",
            DeviceType::OSR2Plus => "OSR2+ (3-DOF Stroke, Pitch, Roll)",
            DeviceType::SR6 => "SR6 (6-DOF Stewart Platform)",
            DeviceType::Keon => "Fleshlight Keon (Reciprocating)",
            DeviceType::Custom => "Custom Device Profile",
        }
    }
}

/// Physical kinematic limit bounds
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KinematicLimits {
    /// Maximum velocity in script units/sec (0-100 scale, typically 400-500)
    pub max_speed_units_per_sec: f32,
    /// Maximum acceleration in script units/sec² (typically 3000-6000)
    pub max_accel_units_per_sec2: f32,
    /// Maximum jerk in script units/sec³ (typically 50,000-100,000)
    pub max_jerk_units_per_sec3: f32,
    /// Stroke length in millimeters (e.g. 110 mm for Handy)
    pub physical_stroke_mm: f32,
}

impl Default for KinematicLimits {
    fn default() -> Self {
        Self {
            max_speed_units_per_sec: 450.0,
            max_accel_units_per_sec2: 4500.0,
            max_jerk_units_per_sec3: 80000.0,
            physical_stroke_mm: 110.0,
        }
    }
}

/// Profile characterizing a specific hardware device
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceProfile {
    pub device_type: DeviceType,
    pub name: String,
    pub limits: KinematicLimits,
    pub supported_axes: Vec<AxisChannel>,
}

impl DeviceProfile {
    pub fn for_device(device_type: DeviceType) -> Self {
        match device_type {
            DeviceType::TheHandy => Self {
                device_type,
                name: "The Handy".to_string(),
                limits: KinematicLimits {
                    max_speed_units_per_sec: 450.0,
                    max_accel_units_per_sec2: 4000.0,
                    max_jerk_units_per_sec3: 75000.0,
                    physical_stroke_mm: 110.0,
                },
                supported_axes: vec![AxisChannel::Stroke],
            },
            DeviceType::OSR2Plus => Self {
                device_type,
                name: "OSR2+".to_string(),
                limits: KinematicLimits {
                    max_speed_units_per_sec: 500.0,
                    max_accel_units_per_sec2: 5500.0,
                    max_jerk_units_per_sec3: 90000.0,
                    physical_stroke_mm: 120.0,
                },
                supported_axes: vec![
                    AxisChannel::Stroke,
                    AxisChannel::Pitch,
                    AxisChannel::Roll,
                    AxisChannel::Suction,
                ],
            },
            DeviceType::SR6 => Self {
                device_type,
                name: "SR6".to_string(),
                limits: KinematicLimits {
                    max_speed_units_per_sec: 550.0,
                    max_accel_units_per_sec2: 6000.0,
                    max_jerk_units_per_sec3: 100000.0,
                    physical_stroke_mm: 130.0,
                },
                supported_axes: vec![
                    AxisChannel::Stroke,
                    AxisChannel::Surge,
                    AxisChannel::Sway,
                    AxisChannel::Pitch,
                    AxisChannel::Roll,
                    AxisChannel::Twist,
                    AxisChannel::Suction,
                ],
            },
            DeviceType::Keon => Self {
                device_type,
                name: "Fleshlight Keon".to_string(),
                limits: KinematicLimits {
                    max_speed_units_per_sec: 350.0,
                    max_accel_units_per_sec2: 3000.0,
                    max_jerk_units_per_sec3: 60000.0,
                    physical_stroke_mm: 95.0,
                },
                supported_axes: vec![AxisChannel::Stroke],
            },
            DeviceType::Custom => Self {
                device_type,
                name: "Custom Device".to_string(),
                limits: KinematicLimits::default(),
                supported_axes: AxisChannel::ALL.to_vec(),
            },
        }
    }
}

/// Instantaneous physics state tracked during playback simulation
#[derive(Debug, Clone)]
pub struct KinematicState {
    pub timestamp_ms: i64,
    pub positions: HashMap<AxisChannel, f32>,
    pub velocities: HashMap<AxisChannel, f32>,
    pub accelerations: HashMap<AxisChannel, f32>,
    pub instantaneous_speed: f32,
    pub instantaneous_accel: f32,
    pub speed_ratio: f32,
    pub accel_ratio: f32,
    pub is_speed_warning: bool,
    pub is_accel_warning: bool,
}

impl Default for KinematicState {
    fn default() -> Self {
        Self {
            timestamp_ms: 0,
            positions: HashMap::new(),
            velocities: HashMap::new(),
            accelerations: HashMap::new(),
            instantaneous_speed: 0.0,
            instantaneous_accel: 0.0,
            speed_ratio: 0.0,
            accel_ratio: 0.0,
            is_speed_warning: false,
            is_accel_warning: false,
        }
    }
}

impl KinematicState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update kinematics state with current positions and elapsed time delta
    pub fn update(
        &mut self,
        new_positions: &HashMap<AxisChannel, f32>,
        timestamp_ms: i64,
        limits: &KinematicLimits,
    ) {
        let dt_sec = ((timestamp_ms - self.timestamp_ms).abs() as f32 / 1000.0).max(0.001);
        self.timestamp_ms = timestamp_ms;

        let mut max_speed = 0.0f32;
        let mut max_accel = 0.0f32;

        for (&axis, &new_pos) in new_positions {
            let prev_pos = *self.positions.get(&axis).unwrap_or(&new_pos);
            let prev_vel = *self.velocities.get(&axis).unwrap_or(&0.0);

            // Raw velocity in units/sec
            let raw_vel = (new_pos - prev_pos) / dt_sec;
            // Low-pass exponential filter on velocity to suppress discrete time jitter
            let filtered_vel = prev_vel * 0.3 + raw_vel * 0.7;

            // Acceleration in units/sec²
            let prev_accel = *self.accelerations.get(&axis).unwrap_or(&0.0);
            let raw_accel = (filtered_vel - prev_vel) / dt_sec;
            let filtered_accel = prev_accel * 0.4 + raw_accel * 0.6;

            self.positions.insert(axis, new_pos);
            self.velocities.insert(axis, filtered_vel);
            self.accelerations.insert(axis, filtered_accel);

            if filtered_vel.abs() > max_speed {
                max_speed = filtered_vel.abs();
            }
            if filtered_accel.abs() > max_accel {
                max_accel = filtered_accel.abs();
            }
        }

        self.instantaneous_speed = max_speed;
        self.instantaneous_accel = max_accel;
        self.speed_ratio = (max_speed / limits.max_speed_units_per_sec).clamp(0.0, 2.0);
        self.accel_ratio = (max_accel / limits.max_accel_units_per_sec2).clamp(0.0, 2.0);

        self.is_speed_warning = self.speed_ratio > 0.9;
        self.is_accel_warning = self.accel_ratio > 0.9;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_profile_limits() {
        let handy = DeviceProfile::for_device(DeviceType::TheHandy);
        assert_eq!(handy.limits.physical_stroke_mm, 110.0);
        assert_eq!(handy.supported_axes, vec![AxisChannel::Stroke]);

        let sr6 = DeviceProfile::for_device(DeviceType::SR6);
        assert_eq!(sr6.supported_axes.len(), 7);
        assert!(sr6.limits.max_speed_units_per_sec > handy.limits.max_speed_units_per_sec);
    }

    #[test]
    fn test_kinematic_state_numerical_derivatives() {
        let mut state = KinematicState::new();
        let limits = KinematicLimits::default();

        let mut p0 = HashMap::new();
        p0.insert(AxisChannel::Stroke, 10.0);
        state.update(&p0, 0, &limits);
        assert_eq!(state.instantaneous_speed, 0.0);

        // Move 40 units in 100ms = 400 units/s
        let mut p1 = HashMap::new();
        p1.insert(AxisChannel::Stroke, 50.0);
        state.update(&p1, 100, &limits);

        // Filtered velocity will be close to 400 * 0.7 = 280
        assert!(state.instantaneous_speed > 200.0);
        assert!(!state.is_speed_warning);

        // Move 90 units in 50ms = 1800 units/s (extreme violation)
        let mut p2 = HashMap::new();
        p2.insert(AxisChannel::Stroke, 90.0);
        state.update(&p2, 150, &limits);
        assert!(state.is_speed_warning);
    }
}
