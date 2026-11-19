//! Root end-to-end suite.
//!
//! One test binary contains every end-to-end module. Tests that need a native
//! compositor stack or a compiled production binary are gated by the `native`
//! feature.

#[path = "../common/mod.rs"]
mod common;

#[cfg(feature = "native")]
mod cast_networkmanager;
#[cfg(feature = "native")]
mod command_entry;
#[cfg(feature = "native")]
mod display_mirror;
#[cfg(feature = "native")]
mod tray_smoke;
