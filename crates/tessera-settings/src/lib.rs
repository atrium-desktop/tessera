//! Tessera Settings: the standalone application that edits the compositor's
//! persistent settings.
//!
//! The library half holds everything that does not need a display server: the
//! [`module`] contract and registry, the built-in pages ([`catalog`]), and the
//! window model and layout ([`host`]). The `tessera-settings` binary wraps it
//! in a Wayland toplevel and reaches the compositor only through the IPC
//! socket — never through a Rust dependency on the compositor.

pub mod catalog;
pub mod host;
pub mod module;

mod modules;
mod widgets;

pub use catalog::builtin_modules;
pub use host::SettingsHost;

pub use tessera_desktop::app::settings_app::{APP_ID, DESKTOP_ID, EXECUTABLE};
