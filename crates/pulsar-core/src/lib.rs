//! Checked domain values and deterministic state transitions.
//!
//! This crate does not discover hardware, read files, spawn workers, grant
//! authority, or acknowledge durability. The engine supplies validated facts
//! and performs effects only after these transitions succeed. These kernels
//! are not a claim of physical-device or neural-model qualification.
#![forbid(unsafe_code)]

mod authority;
mod edit_values;
mod device;
mod dsp;
mod geometry;
mod ids;
mod jobs;
mod motion;
pub mod perception;
mod synthesis;
pub use synthesis::*;
mod editing;
pub use editing::*;
mod project;
mod time;

pub use authority::*;
pub use edit_values::*;
pub use device::*;
pub use dsp::*;
pub use geometry::*;
pub use ids::*;
pub use jobs::*;
pub use motion::*;
pub use project::*;
pub use time::*;

#[cfg(kani)]
mod proofs;

/// Errors are domain failures, not opaque runtime or transport failures.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("identifier must contain 1..=128 ASCII letters, digits, '.', '_', ':', or '-'")]
    InvalidIdentifier,
    #[error("checked arithmetic overflow")]
    Overflow,
    #[error("rational timestamp denominator must be nonzero")]
    InvalidTimebase,
    #[error("value must be finite")]
    NonFinite,
    #[error("normalized value must be in [0, 1]")]
    OutOfRange,
    #[error("invalid coordinate geometry: {0}")]
    InvalidGeometry(String),
    #[error("invalid motion: {0}")]
    InvalidMotion(String),
    #[error("permission denied")]
    PermissionDenied,
    #[error("revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("edit intersects a protected region")]
    ProtectedRegion,
    #[error("invalid state transition: {0}")]
    InvalidTransition(String),
    #[error("candidate belongs to a different project")]
    CandidateMismatch,
    #[error("no undo or redo entry exists")]
    EmptyHistory,
    #[error("qualification does not match the exact device configuration")]
    MissingQualification,
    #[error("device session or controller does not match")]
    DeviceAuthorityMismatch,
    #[error("headless playback requires an approved, qualified stopping bound")]
    MissingStoppingBound,
}
