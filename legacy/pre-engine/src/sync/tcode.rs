//! T-Code v0.3 hardware protocol formatter and UDP streaming dispatcher.

use crate::funscript::AxisChannel;
use std::collections::HashMap;
use std::net::UdpSocket;

/// Format multi-axis positions into canonical T-Code v0.3 command string.
///
/// T-Code v0.3 uses 4-digit precision (0000-9999).
/// Format: `L0{val4} L1{val4} ... I{interval_ms}\n`
pub fn format_tcode_v03(
    positions: &HashMap<AxisChannel, f32>,
    interval_ms: Option<u32>,
) -> String {
    let mut parts = Vec::new();

    // Canonical order: L0, L1, L2, R0, R1, R2, V0
    for &axis in &AxisChannel::ALL {
        if let Some(&pos) = positions.get(&axis) {
            // Map [0.0, 100.0] -> [0000, 9999]
            let val = ((pos / 100.0) * 9999.0).round().clamp(0.0, 9999.0) as u32;
            let code = match axis {
                AxisChannel::Stroke => "L0",
                AxisChannel::Surge => "L1",
                AxisChannel::Sway => "L2",
                AxisChannel::Roll => "R0",
                AxisChannel::Pitch => "R1",
                AxisChannel::Twist => "R2",
                AxisChannel::Suction => "V0",
            };
            parts.push(format!("{}{:04}", code, val));
        }
    }

    if let Some(i) = interval_ms {
        parts.push(format!("I{}", i));
    }

    let mut out = parts.join(" ");
    out.push('\n');
    out
}

/// Dispatches T-Code UDP packets to external software (MultiFunPlayer, Buttplug/Intiface, ScriptPlayer)
pub struct TCodeDispatcher {
    socket: Option<UdpSocket>,
    pub target_addr: String,
    pub is_enabled: bool,
    last_sent_string: String,
}

impl Default for TCodeDispatcher {
    fn default() -> Self {
        Self::new("127.0.0.1:8888")
    }
}

impl TCodeDispatcher {
    pub fn new(target_addr: &str) -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").ok().and_then(|s| {
            s.set_nonblocking(true).ok()?;
            Some(s)
        });

        Self {
            socket,
            target_addr: target_addr.to_string(),
            is_enabled: false,
            last_sent_string: String::new(),
        }
    }

    /// Send T-Code command string over UDP if enabled
    pub fn send(&mut self, tcode: &str) -> bool {
        if !self.is_enabled {
            return false;
        }

        if tcode == self.last_sent_string {
            return false; // Suppress duplicate unchanged packets
        }

        if let Some(ref socket) = self.socket {
            if socket.send_to(tcode.as_bytes(), &self.target_addr).is_ok() {
                self.last_sent_string = tcode.to_string();
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcode_v03_formatting() {
        let mut map = HashMap::new();
        map.insert(AxisChannel::Stroke, 50.0);
        map.insert(AxisChannel::Pitch, 100.0);
        map.insert(AxisChannel::Roll, 0.0);

        let tcode = format_tcode_v03(&map, Some(50));
        assert!(tcode.contains("L05000"));
        assert!(tcode.contains("R19999"));
        assert!(tcode.contains("R00000"));
        assert!(tcode.contains("I50\n"));
    }

    #[test]
    fn test_tcode_dispatcher_creation() {
        let dispatcher = TCodeDispatcher::new("127.0.0.1:8888");
        assert_eq!(dispatcher.target_addr, "127.0.0.1:8888");
        assert!(!dispatcher.is_enabled);
    }
}
