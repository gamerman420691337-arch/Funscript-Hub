//! HereSphere, DeoVR, and Whirligig VR headset WebSocket synchronization server.

use serde_json::Value;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum VrSyncMessage {
    TimeUpdate { timestamp_ms: i64 },
    PlayStateChange { is_playing: bool },
    SpeedChange { speed: f64 },
    VideoPath { path: String },
}

/// Parse HereSphere and DeoVR JSON telemetry messages
pub fn parse_vr_telemetry(text: &str) -> Vec<VrSyncMessage> {
    let mut messages = Vec::new();

    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return messages;
    };

    // 1. Check HereSphere "event" style
    if let Some(event) = v.get("event").and_then(|e| e.as_str()) {
        match event {
            "time" | "currentTime" => {
                if let Some(sec) = v.get("time").and_then(|t| t.as_f64()) {
                    messages.push(VrSyncMessage::TimeUpdate {
                        timestamp_ms: (sec * 1000.0).round() as i64,
                    });
                }
            }
            "playerState" => {
                if let Some(state) = v.get("playerState").and_then(|s| s.as_i64()) {
                    messages.push(VrSyncMessage::PlayStateChange {
                        is_playing: state == 1,
                    });
                }
            }
            "playbackRate" => {
                if let Some(rate) = v.get("playbackRate").and_then(|r| r.as_f64()) {
                    messages.push(VrSyncMessage::SpeedChange { speed: rate });
                }
            }
            "path" | "file" => {
                if let Some(p) = v.get("path").and_then(|p| p.as_str()) {
                    messages.push(VrSyncMessage::VideoPath {
                        path: p.to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    // 2. Check DeoVR style: {"currentTime": 12.34, "state": "playing" / "paused"}
    if let Some(sec) = v.get("currentTime").and_then(|t| t.as_f64()) {
        messages.push(VrSyncMessage::TimeUpdate {
            timestamp_ms: (sec * 1000.0).round() as i64,
        });
    }

    if let Some(state) = v.get("state").and_then(|s| s.as_str()) {
        match state.to_lowercase().as_str() {
            "playing" | "play" => messages.push(VrSyncMessage::PlayStateChange { is_playing: true }),
            "paused" | "pause" | "stopped" => {
                messages.push(VrSyncMessage::PlayStateChange { is_playing: false })
            }
            _ => {}
        }
    }

    if let Some(p) = v.get("path").and_then(|p| p.as_str()) {
        if !messages.iter().any(|m| matches!(m, VrSyncMessage::VideoPath { .. })) {
            messages.push(VrSyncMessage::VideoPath {
                path: p.to_string(),
            });
        }
    }

    messages
}

/// Zero-config background WebSocket server for VR headsets
pub struct VrSyncServer {
    port: u16,
    running: Arc<AtomicBool>,
    server_handle: Option<JoinHandle<()>>,
    tx: Sender<VrSyncMessage>,
    rx: Receiver<VrSyncMessage>,
}

impl VrSyncServer {
    pub fn new(port: u16) -> Self {
        let (tx, rx) = channel();
        let running = Arc::new(AtomicBool::new(false));

        Self {
            port,
            running,
            server_handle: None,
            tx,
            rx,
        }
    }

    pub fn start(&mut self) -> bool {
        if self.running.load(Ordering::SeqCst) {
            return true;
        }

        let listener = match TcpListener::bind(format!("0.0.0.0:{}", self.port)) {
            Ok(l) => l,
            Err(_) => return false,
        };

        let _ = listener.set_nonblocking(true);

        self.running.store(true, Ordering::SeqCst);
        let is_running = self.running.clone();
        let tx = self.tx.clone();

        let handle = thread::spawn(move || {
            while is_running.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                        let _ = stream.set_nonblocking(false);

                        if let Ok(mut ws) = tungstenite::accept(stream) {
                            while is_running.load(Ordering::SeqCst) {
                                match ws.read() {
                                    Ok(tungstenite::Message::Text(txt)) => {
                                        let msgs = parse_vr_telemetry(&txt);
                                        for m in msgs {
                                            let _ = tx.send(m);
                                        }
                                    }
                                    Ok(tungstenite::Message::Ping(p)) => {
                                        let _ = ws.send(tungstenite::Message::Pong(p));
                                    }
                                    Ok(tungstenite::Message::Close(_)) => break,
                                    Err(tungstenite::Error::Io(ref e))
                                        if e.kind() == std::io::ErrorKind::WouldBlock
                                            || e.kind() == std::io::ErrorKind::TimedOut =>
                                    {
                                        continue;
                                    }
                                    Err(_) => break,
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });

        self.server_handle = Some(handle);
        true
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.server_handle.take() {
            let _ = h.join();
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Try receiving all pending messages from VR headsets
    pub fn try_recv(&self) -> Vec<VrSyncMessage> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            out.push(msg);
        }
        out
    }
}

impl Drop for VrSyncServer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heresphere_telemetry_json_parsing() {
        let json1 = r#"{"event":"time","time":42.5}"#;
        let msgs1 = parse_vr_telemetry(json1);
        assert_eq!(msgs1, vec![VrSyncMessage::TimeUpdate { timestamp_ms: 42500 }]);

        let json2 = r#"{"event":"playerState","playerState":1}"#;
        let msgs2 = parse_vr_telemetry(json2);
        assert_eq!(msgs2, vec![VrSyncMessage::PlayStateChange { is_playing: true }]);

        let json3 = r#"{"event":"playbackRate","playbackRate":1.25}"#;
        let msgs3 = parse_vr_telemetry(json3);
        assert_eq!(msgs3, vec![VrSyncMessage::SpeedChange { speed: 1.25 }]);
    }

    #[test]
    fn test_deovr_telemetry_json_parsing() {
        let json = r#"{"currentTime":15.2,"state":"playing","path":"C:/VR/test.mp4"}"#;
        let msgs = parse_vr_telemetry(json);
        assert!(msgs.contains(&VrSyncMessage::TimeUpdate { timestamp_ms: 15200 }));
        assert!(msgs.contains(&VrSyncMessage::PlayStateChange { is_playing: true }));
        assert!(msgs.contains(&VrSyncMessage::VideoPath { path: "C:/VR/test.mp4".to_string() }));
    }
}
