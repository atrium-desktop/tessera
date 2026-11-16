//! Filesystem helpers for fixtures.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Create every parent of `path` and write `contents` to it.
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// Write an executable shell script at `path`.
pub fn write_executable(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    write(path, contents)?;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
}

/// Create a directory and all missing parents.
pub fn mkdir(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(path)
}
