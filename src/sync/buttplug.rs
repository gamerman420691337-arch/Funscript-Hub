//! Buttplug.io v3 / Intiface Central WebSocket synchronization client.
//!
//! Enables direct streaming of funscripts and kinematic motions to hundreds of
//! commercial Bluetooth LE, USB, and Wi-Fi haptic devices supported by Intiface.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

/// Configuration for the Buttplug / Intiface client connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ButtplugConfig {
    pub server_url: String,
    pub client_name: String,
}

impl Default for ButtplugConfig {
    fn default() -> Self {
        Self {
            server_url: "ws://127.0.0.1:12345".to_string(),
            client_name: "Pulsar Kinematic Workstation".to_string(),
        }
    }
}

/// Discovered device connected via Intiface Central
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ButtplugDevice {
    pub index: u32,
    pub name: String,
    pub can_linear: bool,
    pub can_vibrate: bool,
}

/// Outgoing command for Buttplug devices
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ButtplugCommand {
    /// Move a linear stroker to a position [0.0, 1.0] over duration in ms
    Linear {
        device_index: u32,
        duration_ms: u32,
        position: f64,
    },
    /// Set vibration speed [0.0, 1.0] on a vibrating device
    Vibrate {
        device_index: u32,
        speed: f64,
    },
    /// Stop all motion on all devices
    StopAll,
}

/// Event notification emitted by the Buttplug background client
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ButtplugEvent {
    Connected { server_name: String },
    Disconnected,
    DeviceAdded(ButtplugDevice),
    DeviceRemoved { index: u32 },
    Error(String),
}

/// Message payload formatter for Buttplug.io v3 protocol
pub struct ButtplugProtocol;

impl ButtplugProtocol {
    /// Format RequestServerInfo handshake message
    pub fn format_server_info(id: u32, client_name: &str) -> String {
        serde_json::json!([
            {
                "RequestServerInfo": {
                    "Id": id,
                    "ClientName": client_name,
                    "MessageVersion": 3
                }
            }
        ])
        .to_string()
    }

    /// Format StartScanning message
    pub fn format_start_scanning(id: u32) -> String {
        serde_json::json!([
            {
                "StartScanning": {
                    "Id": id
                }
            }
        ])
        .to_string()
    }

    /// Format RequestDeviceList message
    pub fn format_request_device_list(id: u32) -> String {
        serde_json::json!([
            {
                "RequestDeviceList": {
                    "Id": id
                }
            }
        ])
        .to_string()
    }

    /// Format LinearCmd message for stroking toys
    pub fn format_linear_cmd(id: u32, device_index: u32, duration_ms: u32, position: f64) -> String {
        let clamped_pos = position.clamp(0.0, 1.0);
        serde_json::json!([
            {
                "LinearCmd": {
                    "Id": id,
                    "DeviceIndex": device_index,
                    "Vectors": [
                        {
                            "Index": 0,
                            "Duration": duration_ms,
                            "Position": clamped_pos
                        }
                    ]
                }
            }
        ])
        .to_string()
    }

    /// Format ScalarCmd / VibrateCmd message
    pub fn format_vibrate_cmd(id: u32, device_index: u32, speed: f64) -> String {
        let clamped_speed = speed.clamp(0.0, 1.0);
        serde_json::json!([
            {
                "ScalarCmd": {
                    "Id": id,
                    "DeviceIndex": device_index,
                    "Scalars": [
                        {
                            "Index": 0,
                            "Scalar": clamped_speed,
                            "ActuatorType": "Vibrate"
                        }
                    ]
                }
            }
        ])
        .to_string()
    }

    /// Parse incoming message array and extract known event types
    pub fn parse_messages(json_str: &str) -> Vec<ButtplugEvent> {
        let mut events = Vec::new();
        let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
            return events;
        };

        let Some(arr) = val.as_array() else {
            return events;
        };

        for item in arr {
            if let Some(info) = item.get("ServerInfo") {
                let name = info
                    .get("ServerName")
                    .and_then(|s| s.as_str())
                    .unwrap_or("Intiface")
                    .to_string();
                events.push(ButtplugEvent::Connected { server_name: name });
            } else if let Some(dev_list) = item.get("DeviceList") {
                if let Some(devices) = dev_list.get("Devices").and_then(|d| d.as_array()) {
                    for dev in devices {
                        if let Some(parsed) = Self::parse_device(dev) {
                            events.push(ButtplugEvent::DeviceAdded(parsed));
                        }
                    }
                }
            } else if let Some(dev) = item.get("DeviceAdded") {
                if let Some(parsed) = Self::parse_device(dev) {
                    events.push(ButtplugEvent::DeviceAdded(parsed));
                }
            } else if let Some(dev) = item.get("DeviceRemoved") {
                if let Some(idx) = dev.get("DeviceIndex").and_then(|i| i.as_u64()) {
                    events.push(ButtplugEvent::DeviceRemoved { index: idx as u32 });
                }
            } else if let Some(err) = item.get("Error") {
                let msg = err
                    .get("ErrorMessage")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                events.push(ButtplugEvent::Error(msg));
            }
        }

        events
    }

    fn parse_device(dev: &serde_json::Value) -> Option<ButtplugDevice> {
        let index = dev.get("DeviceIndex")?.as_u64()? as u32;
        let name = dev.get("DeviceName")?.as_str()?.to_string();

        let mut can_linear = false;
        let mut can_vibrate = false;

        if let Some(messages) = dev.get("DeviceMessages") {
            if messages.get("LinearCmd").is_some() {
                can_linear = true;
            }
            if messages.get("ScalarCmd").is_some() || messages.get("VibrateCmd").is_some() {
                can_vibrate = true;
            }
        }

        Some(ButtplugDevice {
            index,
            name,
            can_linear,
            can_vibrate,
        })
    }
}

/// Thread-safe dispatcher and client for streaming to Intiface Central
pub struct ButtplugDispatcher {
    pub is_connected: Arc<AtomicBool>,
    pub known_devices: Arc<std::sync::Mutex<HashMap<u32, ButtplugDevice>>>,
    cmd_tx: Option<Sender<ButtplugCommand>>,
    event_rx: Receiver<ButtplugEvent>,
    worker_handle: Option<JoinHandle<()>>,
    msg_counter: Arc<AtomicU32>,
}

impl ButtplugDispatcher {
    pub fn new() -> Self {
        let (_event_tx, event_rx) = channel();
        Self {
            is_connected: Arc::new(AtomicBool::new(false)),
            known_devices: Arc::new(std::sync::Mutex::new(HashMap::new())),
            cmd_tx: None,
            event_rx,
            worker_handle: None,
            msg_counter: Arc::new(AtomicU32::new(1)),
        }
    }

    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst)
    }

    /// Try to receive pending events from the background thread
    #[allow(dead_code)]
    pub fn poll_events(&self) -> Vec<ButtplugEvent> {
        let mut events = Vec::new();
        while let Ok(evt) = self.event_rx.try_recv() {
            events.push(evt);
        }
        events
    }

    /// Connect to Intiface Central WebSocket server and begin discovery
    pub fn connect_server(&mut self, config: ButtplugConfig) -> Result<()> {
        self.disconnect();

        let (cmd_tx, cmd_rx) = channel::<ButtplugCommand>();
        let (event_tx, event_rx) = channel::<ButtplugEvent>();

        self.cmd_tx = Some(cmd_tx);
        self.event_rx = event_rx;

        let is_connected = self.is_connected.clone();
        let known_devices = self.known_devices.clone();
        let msg_counter = self.msg_counter.clone();

        let handle = thread::spawn(move || {
            let socket = match connect(&config.server_url) {
                Ok((s, _)) => s,
                Err(e) => {
                    let _ = event_tx.send(ButtplugEvent::Error(format!("Connection failed: {e}")));
                    return;
                }
            };

            let mut ws: WebSocket<MaybeTlsStream<TcpStream>> = socket;
            is_connected.store(true, Ordering::SeqCst);

            // Handshake 1: RequestServerInfo
            let id = msg_counter.fetch_add(1, Ordering::SeqCst);
            let info_msg = ButtplugProtocol::format_server_info(id, &config.client_name);
            if ws.send(Message::Text(info_msg.into())).is_err() {
                is_connected.store(false, Ordering::SeqCst);
                let _ = event_tx.send(ButtplugEvent::Disconnected);
                return;
            }

            // Handshake 2: RequestDeviceList & StartScanning
            let id2 = msg_counter.fetch_add(1, Ordering::SeqCst);
            let dev_req = ButtplugProtocol::format_request_device_list(id2);
            let _ = ws.send(Message::Text(dev_req.into()));

            let id3 = msg_counter.fetch_add(1, Ordering::SeqCst);
            let scan_req = ButtplugProtocol::format_start_scanning(id3);
            let _ = ws.send(Message::Text(scan_req.into()));

            // Set read non-blocking timeout if stream allows
            let _ = match ws.get_mut() {
                MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(Duration::from_millis(50))),
                _ => Ok(()),
            };

            while is_connected.load(Ordering::SeqCst) {
                // Check outgoing commands
                while let Ok(cmd) = cmd_rx.try_recv() {
                    let next_id = msg_counter.fetch_add(1, Ordering::SeqCst);
                    let wire_msg = match cmd {
                        ButtplugCommand::Linear {
                            device_index,
                            duration_ms,
                            position,
                        } => Some(ButtplugProtocol::format_linear_cmd(
                            next_id,
                            device_index,
                            duration_ms,
                            position,
                        )),
                        ButtplugCommand::Vibrate {
                            device_index,
                            speed,
                        } => Some(ButtplugProtocol::format_vibrate_cmd(next_id, device_index, speed)),
                        ButtplugCommand::StopAll => {
                            let mut dev_msgs = Vec::new();
                            if let Ok(devs) = known_devices.lock() {
                                for dev in devs.values() {
                                    if dev.can_linear {
                                        dev_msgs.push(ButtplugProtocol::format_linear_cmd(
                                            next_id, dev.index, 500, 0.0,
                                        ));
                                    } else if dev.can_vibrate {
                                        dev_msgs.push(ButtplugProtocol::format_vibrate_cmd(
                                            next_id, dev.index, 0.0,
                                        ));
                                    }
                                }
                            }
                            dev_msgs.first().cloned()
                        }
                    };

                    if let Some(payload) = wire_msg {
                        if ws.send(Message::Text(payload.into())).is_err() {
                            break;
                        }
                    }
                }

                // Read incoming messages
                match ws.read() {
                    Ok(Message::Text(txt)) => {
                        let events = ButtplugProtocol::parse_messages(&txt);
                        for evt in events {
                            match &evt {
                                ButtplugEvent::DeviceAdded(dev) => {
                                    if let Ok(mut devs) = known_devices.lock() {
                                        devs.insert(dev.index, dev.clone());
                                    }
                                }
                                ButtplugEvent::DeviceRemoved { index } => {
                                    if let Ok(mut devs) = known_devices.lock() {
                                        devs.remove(index);
                                    }
                                }
                                _ => {}
                            }
                            let _ = event_tx.send(evt);
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        // Timeout: loop back to process outgoing commands
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }

            is_connected.store(false, Ordering::SeqCst);
            let _ = event_tx.send(ButtplugEvent::Disconnected);
        });

        self.worker_handle = Some(handle);
        Ok(())
    }

    /// Dispatch linear stroke command to all known linear devices
    pub fn send_stroke(&self, position: f64, duration_ms: u32) {
        if let Some(tx) = &self.cmd_tx {
            if let Ok(devs) = self.known_devices.lock() {
                for dev in devs.values() {
                    if dev.can_linear {
                        let _ = tx.send(ButtplugCommand::Linear {
                            device_index: dev.index,
                            duration_ms,
                            position,
                        });
                    }
                }
            }
        }
    }

    /// Dispatch vibration speed to all known vibrating devices
    pub fn send_vibrate(&self, speed: f64) {
        if let Some(tx) = &self.cmd_tx {
            if let Ok(devs) = self.known_devices.lock() {
                for dev in devs.values() {
                    if dev.can_vibrate {
                        let _ = tx.send(ButtplugCommand::Vibrate {
                            device_index: dev.index,
                            speed,
                        });
                    }
                }
            }
        }
    }

    /// Disconnect from server and terminate worker thread
    pub fn disconnect(&mut self) {
        self.is_connected.store(false, Ordering::SeqCst);
        self.cmd_tx = None;
        if let Some(h) = self.worker_handle.take() {
            let _ = h.join();
        }
        if let Ok(mut devs) = self.known_devices.lock() {
            devs.clear();
        }
    }
}

impl Default for ButtplugDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buttplug_handshake_serialization() {
        let msg = ButtplugProtocol::format_server_info(1, "Pulsar Test");
        assert!(msg.contains("RequestServerInfo"));
        assert!(msg.contains("Pulsar Test"));
        assert!(msg.contains("\"MessageVersion\":3"));
    }

    #[test]
    fn test_buttplug_linear_cmd_generation() {
        let msg = ButtplugProtocol::format_linear_cmd(42, 0, 150, 0.75);
        assert!(msg.contains("LinearCmd"));
        assert!(msg.contains("\"DeviceIndex\":0"));
        assert!(msg.contains("\"Duration\":150"));
        assert!(msg.contains("\"Position\":0.75"));
    }

    #[test]
    fn test_buttplug_vibrate_cmd_generation() {
        let msg = ButtplugProtocol::format_vibrate_cmd(99, 1, 0.65);
        assert!(msg.contains("ScalarCmd"));
        assert!(msg.contains("\"DeviceIndex\":1"));
        assert!(msg.contains("\"Scalar\":0.65"));
        assert!(msg.contains("\"ActuatorType\":\"Vibrate\""));
    }

    #[test]
    fn test_buttplug_parse_server_info_and_devices() {
        let raw = r#"[
            {
                "ServerInfo": {
                    "Id": 1,
                    "ServerName": "Intiface Engine",
                    "MessageVersion": 3,
                    "MaxPingTime": 0
                }
            },
            {
                "DeviceAdded": {
                    "Id": 0,
                    "DeviceIndex": 3,
                    "DeviceName": "The Handy",
                    "DeviceMessages": {
                        "LinearCmd": [{"StepCount": 100}]
                    }
                }
            }
        ]"#;

        let events = ButtplugProtocol::parse_messages(raw);
        assert_eq!(events.len(), 2);

        if let ButtplugEvent::Connected { server_name } = &events[0] {
            assert_eq!(server_name, "Intiface Engine");
        } else {
            panic!("Expected Connected event");
        }

        if let ButtplugEvent::DeviceAdded(dev) = &events[1] {
            assert_eq!(dev.index, 3);
            assert_eq!(dev.name, "The Handy");
            assert!(dev.can_linear);
            assert!(!dev.can_vibrate);
        } else {
            panic!("Expected DeviceAdded event");
        }
    }
}
