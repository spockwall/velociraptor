//! Operational concerns: append-only audit log, prometheus metrics,
//! periodic exchange reconciliation, and credential file permission checks.
//!
//! Each submodule is a standalone facility — none of them depend on each
//! other. They live together because they're all "process-level housekeeping"
//! rather than core order routing.

pub mod audit;
pub mod metrics;
pub mod reconcile;
pub mod secrets;

pub use audit::AuditSink;
pub use metrics::Metrics;
pub use secrets::ensure_owner_only;
