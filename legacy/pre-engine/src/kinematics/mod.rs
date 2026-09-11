//! Toy Kinematic Physics Engine: Device profiles, S-Curve filtering, and thermal modeling.

pub mod device;
pub mod rig;
pub mod scurve;
pub mod thermal;

#[allow(unused_imports)]
pub use device::{DeviceProfile, DeviceType, KinematicLimits, KinematicState};
#[allow(unused_imports)]
pub use rig::{RigGeometry, RigInput, RigModel, Vec3};
#[allow(unused_imports)]
pub use scurve::{eval_scurve, quintic_smoothstep, smooth_funscript, SCurvePreset};
#[allow(unused_imports)]
pub use thermal::{ThermalModel, ThermalStatus};
