//! Backend-neutral network service contract and daemon protocol (ADR-0242).
//!
//! The compositor never opens NetworkManager or iwd. It consumes a
//! [`NetworkSnapshot`] pushed by a network service, and sends the verbs in
//! [`protocol::Request`]. The service owns the system D-Bus connection and
//! chooses its backend; this crate owns the vocabulary both sides speak.
//!
//! Deliberately absent from this crate: `zbus`, NetworkManager, iwd, socket
//! I/O, and process supervision. Those live in `tessera-network-system` (the
//! service) and `tessera` (the supervisor/client), so a pure compositor build
//! cannot link a system D-Bus client through the network path.

mod backend;
pub mod mock;
pub mod protocol;

pub use backend::{
    BackendChoice, BackendKind, NetworkBackend, NetworkError, NetworkSnapshot,
    UnavailableNetworkBackend,
};
pub use mock::MockNetworkBackend;
