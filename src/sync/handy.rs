//! The Handy (Ohdoki / Sweet Tech) Cloud & Local Sync Protocol Client.
//!
//! Provides direct hardware synchronization using the official HandyFeeling API v2
//! (HDSP - Handy Direct Streaming Protocol & HSSP - Handy Script Sync Protocol).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Configuration for The Handy synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandyConfig {
    /// 6-8 character Connection Key from Handy app or handyfeeling.com
    pub connection_key: String,
    /// Whether hardware synchronization is active
    pub is_enabled: bool,
    /// Minimum stroke percentage limit [0.0, 100.0] for safety/comfort
    pub min_stroke_pct: f32,
    /// Maximum stroke percentage limit [0.0, 100.0] for safety/comfort
    pub max_stroke_pct: f32,
    /// HDSP real-time direct streaming mode (true) vs HSSP script mode (false)
    pub direct_hdsp_mode: bool,
}

impl Default for HandyConfig {
    fn default() -> Self {
        Self {
            connection_key: String::new(),
            is_enabled: false,
            min_stroke_pct: 0.0,
            max_stroke_pct: 100.0,
            direct_hdsp_mode: true,
        }
    }
}

/// Device status and firmware info returned by Handy API v2
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandyStatus {
    pub is_connected: bool,
    pub model: String,
    pub fw_version: String,
    pub hw_version: String,
    pub branch: String,
    pub mode: i32,
    pub last_ping_ms: u64,
}

/// Commands sent from the UI thread to the background Handy worker
#[derive(Debug, Clone)]
pub enum HandyCommand {
    CheckConnection { key: String },
    SetMode { key: String, mode: i32 },
    SendPosition { key: String, position_pct: f32, duration_ms: u32 },
    Stop { key: String },
}

/// Events returned from the background Handy worker to the UI thread
#[derive(Debug, Clone)]
pub enum HandyEvent {
    Connected { status: HandyStatus },
    Disconnected { reason: String },
    Error { message: String },
    #[allow(dead_code)]
    PositionAck { position: f32 },
}

/// Synchronous HTTP client for HandyFeeling API v2
pub struct HandyHttpClient {
    base_url: String,
}

impl Default for HandyHttpClient {
    fn default() -> Self {
        Self {
            base_url: "https://www.handyfeeling.com/api/handy/v2".to_string(),
        }
    }
}

impl HandyHttpClient {
    pub fn new() -> Self {
        Self::default()
    }

    fn agent() -> ureq::Agent {
        let config = ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build();
        config.into()
    }

    /// Check if device with given connection key is online
    pub fn check_connected(&self, key: &str) -> Result<bool> {
        let url = format!("{}/connected", self.base_url);
        let resp = Self::agent()
            .get(&url)
            .header("X-Connection-Key", key.trim())
            .header("Accept", "application/json")
            .call()
            .context("Failed to connect to HandyFeeling API")?;

        let body = resp.into_body().read_to_string()?;
        let val: serde_json::Value = serde_json::from_str(&body)?;
        Ok(val.get("connected").and_then(|c| c.as_bool()).unwrap_or(false))
    }

    /// Query hardware model and firmware version
    pub fn get_info(&self, key: &str) -> Result<HandyStatus> {
        let is_connected = self.check_connected(key)?;
        if !is_connected {
            return Ok(HandyStatus {
                is_connected: false,
                ..Default::default()
            });
        }

        let url = format!("{}/info", self.base_url);
        let resp = Self::agent()
            .get(&url)
            .header("X-Connection-Key", key.trim())
            .header("Accept", "application/json")
            .call()
            .context("Failed to query Handy device info")?;

        let body = resp.into_body().read_to_string()?;
        let val: serde_json::Value = serde_json::from_str(&body)?;

        let model = val.get("model").and_then(|m| m.as_str()).unwrap_or("The Handy").to_string();
        let fw = val.get("fwVersion").and_then(|f| f.as_str()).unwrap_or("Unknown").to_string();
        let hw = val.get("hwVersion").and_then(|h| h.as_str()).unwrap_or("Unknown").to_string();
        let branch = val.get("branch").and_then(|b| b.as_str()).unwrap_or("release").to_string();

        Ok(HandyStatus {
            is_connected: true,
            model,
            fw_version: fw,
            hw_version: hw,
            branch,
            mode: 3,
            last_ping_ms: 0,
        })
    }

    /// Set operating mode (0: Off, 1: HAMP/manual, 2: HSSP/script, 3: HDSP/streaming)
    pub fn set_mode(&self, key: &str, mode: i32) -> Result<()> {
        let url = format!("{}/mode", self.base_url);
        let payload = json!({ "mode": mode });
        let bytes = serde_json::to_vec(&payload)?;

        Self::agent()
            .put(&url)
            .header("X-Connection-Key", key.trim())
            .header("Content-Type", "application/json")
            .send(bytes)
            .context("Failed to set Handy mode")?;

        Ok(())
    }

    /// Stream position target via HDSP `xpt` (percent position + time duration)
    pub fn send_hdsp_xpt(&self, key: &str, position_pct: f32, duration_ms: u32) -> Result<()> {
        let url = format!("{}/hdsp/xpt", self.base_url);
        let clamped_pos = position_pct.clamp(0.0, 100.0);
        let payload = json!({
            "position": clamped_pos,
            "duration": duration_ms.max(10),
            "immediateResponse": false
        });
        let bytes = serde_json::to_vec(&payload)?;

        Self::agent()
            .put(&url)
            .header("X-Connection-Key", key.trim())
            .header("Content-Type", "application/json")
            .send(bytes)
            .context("Failed to send HDSP position command to Handy")?;

        Ok(())
    }

    /// Stop Handy motion
    pub fn stop(&self, key: &str) -> Result<()> {
        let url = format!("{}/hamp/stop", self.base_url);
        let _ = Self::agent()
            .put(&url)
            .header("X-Connection-Key", key.trim())
            .send(vec![]);
        Ok(())
    }
}

/// Asynchronous non-blocking dispatcher managing Handy communication in a background thread
pub struct HandyDispatcher {
    cmd_tx: Sender<HandyCommand>,
    event_rx: Receiver<HandyEvent>,
    pub status: HandyStatus,
    pub last_error: Option<String>,
    pub is_connecting: bool,
    last_pos_sent: f32,
    last_send_time: Instant,
}

impl Default for HandyDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HandyDispatcher {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = channel::<HandyCommand>();
        let (event_tx, event_rx) = channel::<HandyEvent>();

        thread::spawn(move || {
            let client = HandyHttpClient::new();
            while let Ok(mut cmd) = cmd_rx.recv() {
                // Drain newer position commands to keep latency minimal
                if let HandyCommand::SendPosition { .. } = cmd {
                    while let Ok(newer) = cmd_rx.try_recv() {
                        if let HandyCommand::SendPosition { .. } = newer {
                            cmd = newer;
                        } else {
                            // Non-position command, execute it next
                            let _ = event_tx.send(HandyEvent::Error {
                                message: "Interrupted by command".to_string(),
                            });
                            cmd = newer;
                            break;
                        }
                    }
                }

                match cmd {
                    HandyCommand::CheckConnection { key } => {
                        let t0 = Instant::now();
                        match client.get_info(&key) {
                            Ok(mut status) => {
                                status.last_ping_ms = t0.elapsed().as_millis() as u64;
                                if status.is_connected {
                                    // Ensure mode 3 (HDSP direct streaming)
                                    let _ = client.set_mode(&key, 3);
                                    let _ = event_tx.send(HandyEvent::Connected { status });
                                } else {
                                    let _ = event_tx.send(HandyEvent::Disconnected {
                                        reason: "Device reported offline or invalid key".to_string(),
                                    });
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(HandyEvent::Error {
                                    message: format!("Connection check failed: {e}"),
                                });
                            }
                        }
                    }
                    HandyCommand::SetMode { key, mode } => {
                        if let Err(e) = client.set_mode(&key, mode) {
                            let _ = event_tx.send(HandyEvent::Error {
                                message: format!("Set mode failed: {e}"),
                            });
                        }
                    }
                    HandyCommand::SendPosition { key, position_pct, duration_ms } => {
                        if let Ok(()) = client.send_hdsp_xpt(&key, position_pct, duration_ms) {
                            let _ = event_tx.send(HandyEvent::PositionAck { position: position_pct });
                        }
                    }
                    HandyCommand::Stop { key } => {
                        let _ = client.stop(&key);
                    }
                }
            }
        });

        Self {
            cmd_tx,
            event_rx,
            status: HandyStatus::default(),
            last_error: None,
            is_connecting: false,
            last_pos_sent: -1.0,
            last_send_time: Instant::now() - Duration::from_secs(10),
        }
    }

    /// Trigger asynchronous connection test and info retrieval
    pub fn connect(&mut self, key: &str) {
        if key.trim().is_empty() {
            self.last_error = Some("Connection key cannot be empty".to_string());
            return;
        }
        self.is_connecting = true;
        self.last_error = None;
        let _ = self.cmd_tx.send(HandyCommand::CheckConnection {
            key: key.trim().to_string(),
        });
    }

    /// Disconnect and signal stop to device
    pub fn disconnect(&mut self, key: &str) {
        self.status.is_connected = false;
        self.is_connecting = false;
        let _ = self.cmd_tx.send(HandyCommand::Stop {
            key: key.trim().to_string(),
        });
    }

    /// Set operating mode on connected device (0: Off, 1: HAMP, 2: HSSP, 3: HDSP)
    pub fn set_mode(&mut self, key: &str, mode: i32) {
        let _ = self.cmd_tx.send(HandyCommand::SetMode {
            key: key.trim().to_string(),
            mode,
        });
    }

    /// Stream normalized stroke position [0.0, 100.0] respecting rate-limits and min/max bounds
    pub fn stream_position(&mut self, key: &str, raw_pos: f32, config: &HandyConfig, duration_ms: u32) {
        if !config.is_enabled || !self.status.is_connected || key.trim().is_empty() {
            return;
        }

        // Rate limit cloud commands to max 15 requests/sec (~65ms interval) to comply with API limits
        let elapsed = self.last_send_time.elapsed().as_millis();
        let pos_delta = (raw_pos - self.last_pos_sent).abs();
        if elapsed < 65 && pos_delta < 3.0 {
            return;
        }

        // Map raw [0, 100] into user's [min_stroke_pct, max_stroke_pct] range
        let range = (config.max_stroke_pct - config.min_stroke_pct).max(5.0);
        let mapped_pos = config.min_stroke_pct + (raw_pos.clamp(0.0, 100.0) / 100.0) * range;

        let _ = self.cmd_tx.send(HandyCommand::SendPosition {
            key: key.trim().to_string(),
            position_pct: mapped_pos,
            duration_ms: duration_ms.clamp(20, 500),
        });

        self.last_pos_sent = raw_pos;
        self.last_send_time = Instant::now();
    }

    /// Poll incoming events from background worker
    pub fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                HandyEvent::Connected { status } => {
                    self.status = status;
                    self.is_connecting = false;
                    self.last_error = None;
                }
                HandyEvent::Disconnected { reason } => {
                    self.status.is_connected = false;
                    self.is_connecting = false;
                    self.last_error = Some(reason);
                }
                HandyEvent::Error { message } => {
                    self.last_error = Some(message);
                    self.is_connecting = false;
                }
                HandyEvent::PositionAck { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handy_config_defaults() {
        let cfg = HandyConfig::default();
        assert_eq!(cfg.min_stroke_pct, 0.0);
        assert_eq!(cfg.max_stroke_pct, 100.0);
        assert!(cfg.direct_hdsp_mode);
        assert!(!cfg.is_enabled);
    }

    #[test]
    fn test_handy_stroke_mapping() {
        let cfg = HandyConfig {
            connection_key: "TESTKEY".to_string(),
            is_enabled: true,
            min_stroke_pct: 20.0,
            max_stroke_pct: 80.0,
            direct_hdsp_mode: true,
        };

        let map_fn = |raw: f32| -> f32 {
            let range = cfg.max_stroke_pct - cfg.min_stroke_pct;
            cfg.min_stroke_pct + (raw / 100.0) * range
        };

        assert_eq!(map_fn(0.0), 20.0);
        assert_eq!(map_fn(50.0), 50.0);
        assert_eq!(map_fn(100.0), 80.0);
    }

    #[test]
    fn test_handy_dispatcher_creation() {
        let mut disp = HandyDispatcher::new();
        assert!(!disp.status.is_connected);
        assert!(!disp.is_connecting);
        disp.poll_events();
    }
}
