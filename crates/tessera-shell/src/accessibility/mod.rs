//! Decoupled native Linux accessibility implementation for Tessera Shell (ADR-0166).

pub mod auth;
pub mod tree;

pub use auth::PeerAuthenticator;
pub use tree::ShellSemanticTree;

#[cfg(feature = "chrome-accessibility")]
pub mod dbus;

#[cfg(feature = "chrome-accessibility")]
pub use dbus::ShellA11yService;
