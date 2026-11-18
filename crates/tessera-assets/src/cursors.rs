//! Bundled cursor themes embedded directly into the binary.

use std::path::Path;

/// A statically embedded directory of `(relative_path, bytes)` sorted by path.
#[derive(Clone, Copy)]
pub struct EmbeddedDir(&'static [(&'static str, &'static [u8])]);

impl EmbeddedDir {
    /// Look up a file by its relative path inside the embedded directory.
    #[must_use]
    pub fn get_file(&self, path: impl AsRef<Path>) -> Option<&'static [u8]> {
        let needle = path.as_ref().to_str()?;
        self.0
            .binary_search_by_key(&needle, |(k, _)| *k)
            .ok()
            .map(|idx| self.0[idx].1)
    }
}

include!(concat!(env!("OUT_DIR"), "/embedded_cursors.rs"));
