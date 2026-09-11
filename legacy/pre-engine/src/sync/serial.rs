//! Direct USB Serial COM port dispatcher for physical hardware controllers.
//!
//! Supports OSR2, SR6, DIY T-Code microcontrollers (ESP32, Teensy, Arduino, etc.)
//! via the standard T-Code v0.3 protocol.

use std::time::Duration;
use serialport::{SerialPort, SerialPortType};

/// Information about a detected serial communication port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSerialPort {
    /// System device path, e.g. `/dev/ttyUSB0` or `COM3`.
    pub name: String,
    /// Human-friendly display label with hardware identifier.
    pub label: String,
}

/// Dispatcher managing direct hardware serial communication over USB COM ports.
pub struct SerialDispatcher {
    port: Option<Box<dyn SerialPort>>,
    pub port_name: String,
    pub baud_rate: u32,
    pub is_connected: bool,
    pub last_error: Option<String>,
    pub sent_commands_count: usize,
    pub last_sent_command: Option<String>,
}

impl Default for SerialDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialDispatcher {
    /// Standard supported baud rates for T-Code microcontrollers.
    pub const SUPPORTED_BAUD_RATES: [u32; 6] = [115200, 230400, 460800, 921600, 57600, 9600];

    /// Initialize a new disconnected serial dispatcher.
    pub fn new() -> Self {
        Self {
            port: None,
            port_name: String::new(),
            baud_rate: 115200,
            is_connected: false,
            last_error: None,
            sent_commands_count: 0,
            last_sent_command: None,
        }
    }

    /// Enumerate all available hardware serial COM ports with friendly labels.
    pub fn list_ports() -> Vec<DetectedSerialPort> {
        let ports = match serialport::available_ports() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        ports
            .into_iter()
            .map(|p| {
                let label = match p.port_type {
                    SerialPortType::UsbPort(usb) => {
                        let mut desc = Vec::new();
                        if let Some(prod) = usb.product {
                            desc.push(prod);
                        } else if let Some(mfg) = usb.manufacturer {
                            desc.push(mfg);
                        }
                        if desc.is_empty() {
                            format!("{} (USB Device)", p.port_name)
                        } else {
                            format!("{} ({})", p.port_name, desc.join(" - "))
                        }
                    }
                    SerialPortType::PciPort => format!("{} (PCI Serial)", p.port_name),
                    SerialPortType::BluetoothPort => format!("{} (Bluetooth)", p.port_name),
                    SerialPortType::Unknown => p.port_name.clone(),
                };
                DetectedSerialPort {
                    name: p.port_name,
                    label,
                }
            })
            .collect()
    }

    /// Connect to a specific serial port at the given baud rate.
    pub fn connect(&mut self, port_name: &str, baud_rate: u32) -> Result<(), String> {
        self.disconnect();

        let port_builder = serialport::new(port_name, baud_rate)
            .timeout(Duration::from_millis(15));

        match port_builder.open() {
            Ok(port) => {
                self.port = Some(port);
                self.port_name = port_name.to_string();
                self.baud_rate = baud_rate;
                self.is_connected = true;
                self.last_error = None;
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Failed to open serial port {}: {}", port_name, e);
                self.last_error = Some(err_msg.clone());
                self.is_connected = false;
                Err(err_msg)
            }
        }
    }

    /// Disconnect from the active serial port.
    pub fn disconnect(&mut self) {
        self.port = None;
        self.is_connected = false;
        self.last_error = None;
    }

    /// Transmit a T-Code string directly to the connected serial hardware.
    ///
    /// Non-blocking with a short timeout. Automatically appends a newline if omitted.
    pub fn send_tcode(&mut self, command: &str) -> Result<(), String> {
        if !self.is_connected {
            return Ok(());
        }

        let port = match self.port.as_mut() {
            Some(p) => p,
            None => {
                self.is_connected = false;
                return Err("Serial port handle is not available".to_string());
            }
        };

        let mut payload = command.trim_end().to_string();
        payload.push('\n');

        match port.write_all(payload.as_bytes()) {
            Ok(()) => {
                let _ = port.flush();
                self.sent_commands_count += 1;
                self.last_sent_command = Some(command.trim().to_string());
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Serial write error: {}", e);
                self.last_error = Some(err_msg.clone());
                // If broken pipe or disconnected, auto-disconnect
                if e.kind() == std::io::ErrorKind::BrokenPipe
                    || e.kind() == std::io::ErrorKind::UnexpectedEof
                    || e.kind() == std::io::ErrorKind::NotConnected
                {
                    self.disconnect();
                }
                Err(err_msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_dispatcher_creation() {
        let dispatcher = SerialDispatcher::new();
        assert!(!dispatcher.is_connected);
        assert_eq!(dispatcher.baud_rate, 115200);
        assert_eq!(dispatcher.sent_commands_count, 0);
    }

    #[test]
    fn test_list_ports_does_not_panic() {
        // Should query available ports without panicking even in headless/CI environments
        let ports = SerialDispatcher::list_ports();
        // ports may be empty or contain virtual/hardware ports
        let _ = ports.len();
    }

    #[test]
    fn test_send_tcode_when_disconnected() {
        let mut dispatcher = SerialDispatcher::new();
        assert!(dispatcher.send_tcode("L05000").is_ok());
        assert_eq!(dispatcher.sent_commands_count, 0);
    }

    #[test]
    fn test_supported_baud_rates() {
        assert!(SerialDispatcher::SUPPORTED_BAUD_RATES.contains(&115200));
        assert!(SerialDispatcher::SUPPORTED_BAUD_RATES.contains(&230400));
    }
}
