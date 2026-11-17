//! System network service backends and the `tessera-networkd` process
//! (ADR-0242).
//!
//! Everything that opens a system D-Bus connection lives here, never in the
//! compositor. The public surface is the backend selection and the daemon
//! entry point; the wire vocabulary is `tessera-network`.

pub mod daemon;
pub mod iwd;
pub mod networkmanager;
mod selection;

pub use selection::{DaemonBackend, open_backend};
