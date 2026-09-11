//! Headless batch processing and directory queue orchestration.

pub mod detector;
pub mod queue;

#[allow(unused_imports)]
pub use detector::{scan_library, EnqueueFilter, LibraryAuditReport, MediaAuditItem, ScriptCoverage};
#[allow(unused_imports)]
pub use queue::{BatchJob, BatchJobConfig, BatchJobStatus, BatchProgressUpdate, BatchQueue, BatchWorker};

