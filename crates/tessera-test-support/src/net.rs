//! Process-unique IPC socket paths for loopback tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique socket path under the system temporary directory.
///
/// The name combines `prefix`, the process id, and a per-process counter so
/// nextest's process-per-test model and parallel threads cannot collide.
pub fn unique_socket_path(prefix: &str) -> PathBuf {
    unique_path(prefix, "sock")
}

/// A unique path under the system temporary directory.
pub fn unique_path(prefix: &str, extension: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("{prefix}-{}-{n}.{extension}", std::process::id()));
    path
}

/// A socket path that is removed when the value drops.
#[derive(Debug)]
pub struct ScratchSocket {
    path: PathBuf,
}

impl ScratchSocket {
    /// Reserve a unique socket path. Nothing is bound yet.
    pub fn new(prefix: &str) -> Self {
        Self {
            path: unique_socket_path(prefix),
        }
    }

    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
