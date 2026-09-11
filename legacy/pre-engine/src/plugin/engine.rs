//! Extensible procedural plugin engine and community macro SDK.

use crate::funscript::Action;
use anyhow::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginParam {
    pub key: String,
    pub label: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub current: f32,
}

pub trait FunscriptPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn parameters(&self) -> Vec<PluginParam>;
    fn apply(&self, actions: &[Action], params: &HashMap<String, f32>) -> Result<Vec<Action>>;
}

// ============================================================================
// Built-in Reference Plugin 1: Exponential Climax Ramp (Power Curve Distortion)
// ============================================================================
pub struct ExponentialRampPlugin;

impl FunscriptPlugin for ExponentialRampPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: "exponential_ramp".to_string(),
            name: "Exponential Climax Ramp".to_string(),
            author: "Pulsar Team".to_string(),
            version: "1.0.0".to_string(),
            description: "Applies non-linear power-law curve (p^gamma) for crescendo buildups.".to_string(),
        }
    }

    fn parameters(&self) -> Vec<PluginParam> {
        vec![
            PluginParam {
                key: "gamma".to_string(),
                label: "Curve Exponent (Gamma)".to_string(),
                min: 0.2,
                max: 3.0,
                default: 1.5,
                current: 1.5,
            },
        ]
    }

    fn apply(&self, actions: &[Action], params: &HashMap<String, f32>) -> Result<Vec<Action>> {
        let gamma = *params.get("gamma").unwrap_or(&1.5);
        let mut out = Vec::with_capacity(actions.len());

        for a in actions {
            let norm = (a.pos as f32 / 100.0).clamp(0.0, 1.0);
            let transformed = norm.powf(gamma);
            let new_pos = (transformed * 100.0).round().clamp(0.0, 100.0) as i32;
            out.push(Action { at: a.at, pos: new_pos });
        }

        Ok(out)
    }
}

// ============================================================================
// Built-in Reference Plugin 2: Beat Pulser (Rhythmic Micro-Harmonics)
// ============================================================================
pub struct BeatPulserPlugin;

impl FunscriptPlugin for BeatPulserPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: "beat_pulser".to_string(),
            name: "Harmonic Beat Pulser".to_string(),
            author: "Pulsar Team".to_string(),
            version: "1.0.0".to_string(),
            description: "Superimposes high-frequency micro-pulses over primary strokes.".to_string(),
        }
    }

    fn parameters(&self) -> Vec<PluginParam> {
        vec![
            PluginParam {
                key: "freq_hz".to_string(),
                label: "Pulse Frequency (Hz)".to_string(),
                min: 1.0,
                max: 12.0,
                default: 4.0,
                current: 4.0,
            },
            PluginParam {
                key: "amplitude".to_string(),
                label: "Pulse Amplitude (0-25)".to_string(),
                min: 1.0,
                max: 25.0,
                default: 8.0,
                current: 8.0,
            },
        ]
    }

    fn apply(&self, actions: &[Action], params: &HashMap<String, f32>) -> Result<Vec<Action>> {
        let freq = *params.get("freq_hz").unwrap_or(&4.0);
        let amp = *params.get("amplitude").unwrap_or(&8.0);
        let mut out = Vec::with_capacity(actions.len());

        for a in actions {
            let t_sec = a.at as f32 / 1000.0;
            let pulse = (2.0 * std::f32::consts::PI * freq * t_sec).sin() * amp;
            let new_pos = (a.pos as f32 + pulse).round().clamp(0.0, 100.0) as i32;
            out.push(Action { at: a.at, pos: new_pos });
        }

        Ok(out)
    }
}

// ============================================================================
// Built-in Reference Plugin 3: Chaos Jitter Injector
// ============================================================================
pub struct ChaosJitterPlugin;

impl FunscriptPlugin for ChaosJitterPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: "chaos_jitter".to_string(),
            name: "Organic Chaos Jitter".to_string(),
            author: "Pulsar Team".to_string(),
            version: "1.0.0".to_string(),
            description: "Injects organic stochastic variations to mimic human tremor.".to_string(),
        }
    }

    fn parameters(&self) -> Vec<PluginParam> {
        vec![
            PluginParam {
                key: "intensity".to_string(),
                label: "Jitter Intensity".to_string(),
                min: 1.0,
                max: 15.0,
                default: 5.0,
                current: 5.0,
            },
        ]
    }

    fn apply(&self, actions: &[Action], params: &HashMap<String, f32>) -> Result<Vec<Action>> {
        let intensity = *params.get("intensity").unwrap_or(&5.0);
        let mut out = Vec::with_capacity(actions.len());

        for (i, a) in actions.iter().enumerate() {
            // Pseudorandom deterministic hash based on timestamp and index
            let pseudo_rand = ((i as f32 * 12.9898 + a.at as f32 * 78.233).sin() * 43_758.547_f32).fract();
            let jitter = (pseudo_rand * 2.0 - 1.0) * intensity;
            let new_pos = (a.pos as f32 + jitter).round().clamp(0.0, 100.0) as i32;
            out.push(Action { at: a.at, pos: new_pos });
        }

        Ok(out)
    }
}

/// Host manager maintaining registered community plugins
pub struct PluginHost {
    pub plugins: Vec<Box<dyn FunscriptPlugin>>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self {
            plugins: vec![
                Box::new(ExponentialRampPlugin),
                Box::new(BeatPulserPlugin),
                Box::new(ChaosJitterPlugin),
            ],
        }
    }
}

impl PluginHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        self.plugins.iter().map(|p| p.metadata()).collect()
    }

    #[allow(dead_code)]
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    #[allow(dead_code)]
    pub fn register<P: FunscriptPlugin + 'static>(&mut self, plugin: P) {
        self.plugins.push(Box::new(plugin));
    }

    #[allow(dead_code)]
    pub fn get_plugin(&self, id: &str) -> Option<&dyn FunscriptPlugin> {
        self.plugins.iter().find(|p| p.metadata().id == id).map(|b| &**b)
    }

    pub fn apply_by_id(&self, id: &str, actions: &[Action], params: &HashMap<String, f32>) -> Result<Vec<Action>> {
        for p in &self.plugins {
            if p.metadata().id == id {
                return p.apply(actions, params);
            }
        }
        anyhow::bail!("Plugin '{}' not found in host registry", id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_filter_transformations() {
        let host = PluginHost::new();
        let plugins = host.list_plugins();
        assert_eq!(plugins.len(), 3);

        let actions = vec![
            Action { at: 0, pos: 10 },
            Action { at: 200, pos: 50 },
            Action { at: 400, pos: 90 },
        ];

        // 1. Test Exponential Ramp
        let mut params = HashMap::new();
        params.insert("gamma".to_string(), 2.0);
        let ramped = host.apply_by_id("exponential_ramp", &actions, &params).unwrap();
        assert_eq!(ramped.len(), 3);
        // (50/100)^2 * 100 = 25
        assert_eq!(ramped[1].pos, 25);

        // 2. Test Beat Pulser
        let mut pulse_params = HashMap::new();
        pulse_params.insert("freq_hz".to_string(), 5.0);
        pulse_params.insert("amplitude".to_string(), 10.0);
        let pulsed = host.apply_by_id("beat_pulser", &actions, &pulse_params).unwrap();
        assert_eq!(pulsed.len(), 3);
        for a in &pulsed {
            assert!(a.pos >= 0 && a.pos <= 100);
        }

        // 3. Test Chaos Jitter
        let mut jitter_params = HashMap::new();
        jitter_params.insert("intensity".to_string(), 4.0);
        let jittered = host.apply_by_id("chaos_jitter", &actions, &jitter_params).unwrap();
        assert_eq!(jittered.len(), 3);
        for a in &jittered {
            assert!(a.pos >= 0 && a.pos <= 100);
        }

        // 4. Test Dynamic Registration & Lookup
        struct CustomInvertPlugin;
        impl FunscriptPlugin for CustomInvertPlugin {
            fn metadata(&self) -> PluginMetadata {
                PluginMetadata {
                    id: "custom_invert".into(),
                    name: "Custom Inverter".into(),
                    author: "Tester".into(),
                    version: "0.1.0".into(),
                    description: "Inverts script coordinates".into(),
                }
            }
            fn parameters(&self) -> Vec<PluginParam> { vec![] }
            fn apply(&self, actions: &[Action], _params: &HashMap<String, f32>) -> Result<Vec<Action>> {
                Ok(actions.iter().map(|a| Action { at: a.at, pos: 100 - a.pos }).collect())
            }
        }

        let mut mutable_host = PluginHost::new();
        assert_eq!(mutable_host.plugin_count(), 3);
        mutable_host.register(CustomInvertPlugin);
        assert_eq!(mutable_host.plugin_count(), 4);
        assert!(mutable_host.get_plugin("custom_invert").is_some());
        let inverted = mutable_host.apply_by_id("custom_invert", &actions, &HashMap::new()).unwrap();
        assert_eq!(inverted[0].pos, 90);
        assert_eq!(inverted[1].pos, 50);
        assert_eq!(inverted[2].pos, 10);
    }
}
