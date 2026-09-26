//! Decoupled native Linux accessibility implementation for Tessera Shell (ADR-0166).

pub mod auth;
pub mod tree;

pub use auth::PeerAuthenticator;
pub use tree::ShellSemanticTree;

#[cfg(feature = "dep:zbus")]
pub mod dbus;

#[cfg(feature = "dep:zbus")]
pub use dbus::ShellA11yService;
