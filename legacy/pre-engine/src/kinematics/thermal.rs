//! Continuous thermal load estimation and micro-cooldown compensation.
//!
//! Models motor copper loss (I²R) and mechanical dissipation to predict
//! actuator coil temperatures, inserting protective amplitude attenuation
//! when thermal thresholds are exceeded.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThermalStatus {
    Normal,
    Warm,
    Throttling,
}

impl ThermalStatus {
    #[allow(dead_code)]
    pub fn badge(&self) -> &'static str {
        match self {
            ThermalStatus::Normal => "NORMAL",
            ThermalStatus::Warm => "WARMING",
            ThermalStatus::Throttling => "COOLING_DOWN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThermalModel {
    /// Normalized thermal load in [0.0, 100.0]%
    pub thermal_load_pct: f32,
    /// Estimated temperature rise in °C above ambient (ambient = 25°C)
    pub temp_rise_c: f32,
    /// Velocity loss coefficient
    pub kv: f32,
    /// Acceleration loss coefficient
    pub ka: f32,
    /// Thermal cooling time constant in seconds (typically 30-60s)
    pub cooling_tau_sec: f32,
    /// Maximum temperature rise threshold before 100% load
    pub max_temp_rise_c: f32,
    /// Active status
    pub status: ThermalStatus,
}

impl Default for ThermalModel {
    fn default() -> Self {
        Self {
            thermal_load_pct: 0.0,
            temp_rise_c: 0.0,
            kv: 0.00015,
            ka: 0.000002,
            cooling_tau_sec: 45.0,
            max_temp_rise_c: 55.0, // 25°C ambient + 55°C rise = 80°C coil limit
            status: ThermalStatus::Normal,
        }
    }
}

impl ThermalModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset thermal state to cold ambient
    pub fn reset(&mut self) {
        self.thermal_load_pct = 0.0;
        self.temp_rise_c = 0.0;
        self.status = ThermalStatus::Normal;
    }

    /// Step simulation forward with instantaneous kinematics
    pub fn step(&mut self, dt_sec: f32, speed: f32, accel: f32) {
        if dt_sec <= 0.0 {
            return;
        }

        // Dissipated power proxy
        let power = self.kv * speed * speed + self.ka * accel * accel;

        // Exponential thermal decay: temp = temp * exp(-dt / tau) + (power / C) * dt
        let decay = (-dt_sec / self.cooling_tau_sec).exp();
        self.temp_rise_c = self.temp_rise_c * decay + power * dt_sec;
        self.temp_rise_c = self.temp_rise_c.clamp(0.0, self.max_temp_rise_c * 1.5);

        self.thermal_load_pct = (self.temp_rise_c / self.max_temp_rise_c * 100.0).clamp(0.0, 100.0);

        self.status = if self.thermal_load_pct >= 85.0 {
            ThermalStatus::Throttling
        } else if self.thermal_load_pct >= 70.0 {
            ThermalStatus::Warm
        } else {
            ThermalStatus::Normal
        };
    }

    /// Attenuate position during thermal throttling to protect physical hardware
    pub fn attenuate_position(&self, raw_pos: f32) -> f32 {
        if self.thermal_load_pct < 85.0 {
            return raw_pos;
        }

        // Scale factor shrinks stroke amplitude towards midpoint (50.0)
        // At 85% load -> factor 1.0 (no change)
        // At 100% load -> factor 0.55 (reduced to 55% amplitude)
        let excess = (self.thermal_load_pct - 85.0) / 15.0; // 0.0 to 1.0
        let attenuation = 1.0 - (excess * 0.45).clamp(0.0, 0.45);

        50.0 + (raw_pos - 50.0) * attenuation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thermal_accumulation_and_decay() {
        let mut model = ThermalModel::new();
        assert_eq!(model.status, ThermalStatus::Normal);
        assert_eq!(model.thermal_load_pct, 0.0);

        // Simulate intense high-speed motion for 20 seconds
        for _ in 0..200 {
            model.step(0.1, 450.0, 4000.0);
        }

        // Thermal load must accumulate
        assert!(model.thermal_load_pct > 30.0);

        // Simulate idle rest for 60 seconds
        let warm_load = model.thermal_load_pct;
        for _ in 0..600 {
            model.step(0.1, 0.0, 0.0);
        }

        // Thermal load must cool down
        assert!(model.thermal_load_pct < warm_load);
    }

    #[test]
    fn test_thermal_attenuation_preserves_rhythm_at_reduced_amplitude() {
        let mut model = ThermalModel::new();
        model.thermal_load_pct = 95.0; // In throttling range

        let full_stroke_top = 100.0;
        let safe_top = model.attenuate_position(full_stroke_top);
        assert!(safe_top < 100.0);
        assert!(safe_top > 50.0); // Still moves upward, just attenuated

        let full_stroke_bottom = 0.0;
        let safe_bottom = model.attenuate_position(full_stroke_bottom);
        assert!(safe_bottom > 0.0);
        assert!(safe_bottom < 50.0);
    }
}
