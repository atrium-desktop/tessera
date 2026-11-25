//! In-memory bounded event ring buffer for Tessera's unified observability bus (ADR-0260).
#![forbid(unsafe_code)]

mod audit;
pub use audit::*;

mod entry;
pub use entry::{AuditEntry, AuditSnapshot};
pub mod journal;
