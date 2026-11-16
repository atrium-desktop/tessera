//! Scratch directories for tests.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub use tempfile::tempdir;

/// A temporary directory with small convenience constructors.
///
/// The directory is removed when the value drops.
pub struct TestDir {
    dir: TempDir,
}

impl TestDir {
    /// Create a new temporary directory.
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("create temporary test directory"),
        }
    }

    /// Create a new temporary directory with a descriptive prefix.
    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            dir: tempfile::Builder::new()
                .prefix(prefix)
                .tempdir()
                .expect("create temporary test directory"),
        }
    }

    /// The directory path.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A path inside the directory.
    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.dir.path().join(path)
    }

    /// Create a child directory and return its path.
    pub fn subdir(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = self.join(path);
        std::fs::create_dir_all(&path).expect("create test subdirectory");
        path
    }
}

impl Default for TestDir {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TestDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TestDir").field(&self.path()).finish()
    }
}
