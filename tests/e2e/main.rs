//! Root end-to-end suite.
//!
//! One test binary contains every end-to-end module. Tests that need a native
//! compositor stack or a compiled production binary are gated by the `native`
//! feature; the headless CLI entry test remains available in flux-free runs.

#[path = "../common/mod.rs"]
mod common;

mod cli_control;

#[cfg(feature = "native")]
mod command_entry;
#[cfg(feature = "native")]
mod tray_smoke;
