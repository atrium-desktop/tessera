//! Shared support for the root external test suites.
//!
//! This module deliberately contains no test functions. It re-exports the
//! `tessera-test-support` infrastructure so the integration and end-to-end
//! entries share one vocabulary for scratch directories, environment guards,
//! socket names, polling, workspace paths, and built binaries.

#![allow(dead_code, unused_imports)]

pub use tessera_test_support::env;
pub use tessera_test_support::fs;
pub use tessera_test_support::net;
pub use tessera_test_support::poll;
pub use tessera_test_support::temp;
pub use tessera_test_support::workspace;

#[cfg(feature = "native")]
pub mod fake_networkmanager;

/// Shorthand for the shared binary lookup used by end-to-end entry tests.
pub fn binary(name: &str) -> std::path::PathBuf {
    workspace::binary(name)
}

/// Shorthand for the workspace root.
pub fn workspace_root() -> std::path::PathBuf {
    workspace::workspace_root()
}
