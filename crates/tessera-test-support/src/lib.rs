//! Shared infrastructure for Tessera tests.
//!
//! This crate is a dev-dependency. It exists so unit tests, the root
//! integration suite, and the root end-to-end suite share one implementation
//! of environment isolation, scratch directories, socket names, polling, and
//! workspace path/binary lookup instead of copying helpers per crate.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod env;
pub mod fs;
pub mod net;
pub mod poll;
pub mod temp;
pub mod workspace;

/// Re-export so callers do not need a separate `tempfile` dependency just to
/// name the concrete directory type.
pub use tempfile::{NamedTempFile, TempDir};
