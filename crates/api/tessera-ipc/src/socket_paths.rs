//! Socket naming and lifecycle hygiene for the tessera IPC contract.
//!
//! Socket *names* are protocol (ADR-0027, ADR-0147 Decision 3): every
//! process that binds or connects must derive the same path from the
//! same runtime directory, so the names live here as pure functions
//! over a caller-supplied directory. Environment resolution (where the
//! runtime directory actually is) is deliberately NOT here — that is
//! `tessera_bootstrap::runtime_dir()` (ADR-0147 Decision 2). Consumers
//! compose the two:
//!
//! ```ignore
//! let dir = tessera_bootstrap::runtime_dir()?;
//! let path = tessera_ipc::socket_paths::default_socket_path(&dir);
//! ```
//!
//! [`SocketGuard`] is the RAII unlink with identity verification: the
//! Drop path compares the file's `dev`/`ino` against the identity
//! captured at construction, so a guard never deletes a socket that a
//! successor process recreated after a crash race. It is the shared
//! replacement for the per-binary and per-server cleanup loops that
//! preceded it (ADR-0147 Decision 4).

use std::io;
use std::os::unix::fs::FileTypeExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

/// Path of the primary IPC socket under `runtime_dir` (ADR-0027).
#[must_use]
pub fn default_socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tessera.sock")
}

/// Path of the idle sidecar's control socket under `runtime_dir`.
#[must_use]
pub fn idle_control_socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tessera-idle.sock")
}

/// Base directory of the portal-adapter socket tree under `runtime_dir`.
#[must_use]
pub fn portals_base_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tessera-portals")
}

/// Captured on-disk identity of the socket file at guard construction.
#[derive(Debug)]
struct SocketIdentity {
    dev: u64,
    ino: u64,
}

impl SocketIdentity {
    fn of(path: &Path) -> io::Result<Self> {
        let metadata = path.symlink_metadata()?;
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    fn matches(&self, path: &Path) -> bool {
        path.symlink_metadata().is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.dev() == self.dev
                && metadata.ino() == self.ino
        })
    }
}

/// RAII ownership of a bound unix socket file: on drop, unlink it — but
/// only if it is still the very same file that was bound.
///
/// The identity check closes the crash-race hazard of a plain unlink:
/// if this process crashed while a successor already rebound the path,
/// the successor's socket has a fresh inode and the guard leaves it
/// alone. Constructing a guard over an existing socket removes the
/// stale file first (the pre-`bind` precondition every server wants);
/// constructing one over an absent path simply records "nothing yet".
#[derive(Debug)]
pub struct SocketGuard {
    path: PathBuf,
    identity: Option<SocketIdentity>,
}

impl SocketGuard {
    /// Take cleanup ownership of a socket path that is about to be
    /// bound. A stale file at the path is removed first; the guard
    /// records the absent identity and will not unlink anything unless
    /// the caller actually binds a socket before dropping.
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        match SocketIdentity::of(&path) {
            // The path exists: it is either our own stale socket (safe
            // to reclaim) or something unexpected. Only reclaim sockets;
            // any other file type is an error, never silently deleted.
            Ok(_identity) => {
                let is_socket = path
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_socket());
                if is_socket {
                    std::fs::remove_file(&path)?;
                    Ok(Self {
                        path,
                        identity: None,
                    })
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "socket path exists and is not a socket",
                    ))
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                path,
                identity: None,
            }),
            Err(error) => Err(error),
        }
    }

    /// Record the identity of the socket the caller just bound. Call
    /// after a successful bind; without this, Drop performs no unlink.
    pub fn observe_bound(&mut self) -> io::Result<()> {
        self.identity = Some(SocketIdentity::of(&self.path)?);
        Ok(())
    }

    /// The guarded path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Unlink now, honoring the identity check, and disarm the guard.
    pub fn release(&mut self) {
        if let Some(identity) = self.identity.take()
            && identity.matches(&self.path)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind_real_socket(path: &Path) -> std::os::unix::net::UnixListener {
        std::os::unix::net::UnixListener::bind(path).expect("bind test socket")
    }

    #[test]
    fn names_are_contract_constants() {
        let dir = Path::new("/run/user/1000");
        assert_eq!(
            default_socket_path(dir),
            PathBuf::from("/run/user/1000/tessera.sock")
        );
        assert_eq!(
            idle_control_socket_path(dir),
            PathBuf::from("/run/user/1000/tessera-idle.sock")
        );
        assert_eq!(
            portals_base_path(dir),
            PathBuf::from("/run/user/1000/tessera-portals")
        );
    }

    #[test]
    fn guard_unlinks_the_socket_it_observed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("s.sock");
        // Guard first (reclaims nothing), then bind, then observe.
        let mut guard = SocketGuard::bind(path.clone()).unwrap();
        let _listener = bind_real_socket(&path);
        guard.observe_bound().unwrap();
        assert!(path.exists());
        drop(guard);
        assert!(!path.exists(), "guard must unlink the observed socket");
    }

    #[test]
    fn guard_spares_a_successors_socket() {
        // Ownership sequence with UnixListener's free-path requirement:
        // 1. A stale socket file exists (previous crashed owner).
        // 2. The new owner's guard reclaims it and records "no identity".
        // 3. The owner binds a fresh socket and observes its identity.
        // 4. The successor unlinks and rebinds; its inode differs.
        // 5. The guard drops: identity mismatch means no unlink.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("s.sock");

        bind_real_socket(&path);
        let mut guard = SocketGuard::bind(path.clone()).unwrap();

        let _owner = bind_real_socket(&path);
        guard.observe_bound().unwrap();

        std::fs::remove_file(&path).unwrap();
        let _successor = bind_real_socket(&path);

        drop(guard);
        assert!(
            path.symlink_metadata().is_ok(),
            "guard must never delete a successor's socket"
        );
    }

    #[test]
    fn guard_removes_a_stale_socket_before_bind() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("s.sock");
        let stale = bind_real_socket(&path);
        drop(stale);
        assert!(path.symlink_metadata().is_err() || path.exists());
        // Rebind over the (possibly stale) file: bind() reclaims a
        // stale socket so UnixListener::bind can take the path.
        let mut guard = SocketGuard::bind(path.clone()).unwrap();
        let _listener = bind_real_socket(&path);
        guard.observe_bound().unwrap();
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn guard_refuses_a_non_socket_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("regular.txt");
        std::fs::write(&path, b"data").unwrap();
        let result = SocketGuard::bind(path.clone());
        assert!(result.is_err(), "must refuse to manage a regular file");
        assert!(path.exists(), "must not delete the foreign file");
    }

    #[test]
    fn unobserved_guard_never_unlinks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("s.sock");
        let guard = SocketGuard::bind(path.clone()).unwrap();
        let _listener = bind_real_socket(&path);
        // No observe_bound(): the guard never claimed this socket.
        drop(guard);
        assert!(path.exists(), "no observe_bound() means no cleanup claim");
    }

    #[test]
    fn release_is_explicit_and_disarms() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("s.sock");
        let mut guard = SocketGuard::bind(path.clone()).unwrap();
        let _listener = bind_real_socket(&path);
        guard.observe_bound().unwrap();
        guard.release();
        assert!(!path.exists());
        // Disarmed: a second release (or drop) must be a no-op.
        guard.release();
    }
}
