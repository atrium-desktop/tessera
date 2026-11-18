//! Graceful SIGINT/SIGTERM handling for the caster.
//!
//! A casting session owns a P2P group. Terminating the process between
//! destructors would leave that group active on the sink, so both the CLI and
//! the service translate the termination signals into the same stop flag the
//! session already checks. The session then unwinds through its group guard.

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Install handlers that set the returned flag on SIGINT or SIGTERM.
///
/// The flag is shared with [`crate::session::SessionOptions::stop`]. A second
/// call in the same process replaces the handlers and returns a fresh flag;
/// callers install once at process start.
pub fn install() -> io::Result<Arc<AtomicBool>> {
    tessera_env::signals::install_shutdown_flag()
}
