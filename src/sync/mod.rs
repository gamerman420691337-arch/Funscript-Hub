//! Hardware and VR headset synchronization modules (T-Code v0.3 & WebSocket).

pub mod buttplug;
pub mod handy;
pub mod serial;
pub mod tcode;
pub mod vr;

#[allow(unused_imports)]
pub use buttplug::{ButtplugCommand, ButtplugConfig, ButtplugDevice, ButtplugDispatcher, ButtplugEvent};
#[allow(unused_imports)]
pub use handy::{HandyCommand, HandyConfig, HandyDispatcher, HandyEvent, HandyStatus};
#[allow(unused_imports)]
pub use serial::{DetectedSerialPort, SerialDispatcher};
#[allow(unused_imports)]
pub use tcode::{format_tcode_v03, TCodeDispatcher};
#[allow(unused_imports)]
pub use vr::{parse_vr_telemetry, VrSyncMessage, VrSyncServer};
