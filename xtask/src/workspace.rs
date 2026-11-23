//! Workspace facts shared by every task.
//!
//! Package discovery goes through `cargo metadata`, never through directory
//! globs or hand-parsed manifests: Cargo resolves members, renames, targets,
//! and platform-specific edges, so a check cannot disagree with the build.

use crate::error::{Context, Result, bail};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Absolute path to the workspace root (the parent of this package).
pub fn root() -> PathBuf {
    let xtask = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

/// Absolute path to a file owned by the xtask package itself.
pub fn xtask_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

/// One workspace member, as Cargo resolves it.
#[derive(Debug, Deserialize)]
pub struct Package {
    pub name: String,
    pub manifest_path: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub targets: Vec<Target>,
}

impl Package {
    /// Directory holding the package's `Cargo.toml`.
    pub fn dir(&self) -> &Path {
        self.manifest_path
            .parent()
            .expect("a manifest path always has a parent")
    }

    /// Entry-point source files of every binary target.
    pub fn bin_sources(&self) -> impl Iterator<Item = &Path> {
        self.targets
            .iter()
            .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
            .map(|target| target.src_path.as_path())
    }
}

/// One declared dependency edge. `name` is the real package name even when
/// the manifest renames it, so an alias cannot hide an edge.
#[derive(Debug, Deserialize)]
pub struct Dependency {
    pub name: String,
    /// `None` for a normal edge, otherwise `"dev"` or `"build"`.
    pub kind: Option<String>,
}

impl Dependency {
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or("normal")
    }
}

#[derive(Debug, Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: PathBuf,
}

/// Every workspace member, sorted by name.
pub fn packages() -> Result<Vec<Package>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root())
        .output()
        .context("running `cargo metadata`")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("parsing `cargo metadata` output")?;
    metadata.packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(metadata.packages)
}

/// Every `.rs` file under `dir`, sorted, recursing into subdirectories.
pub fn rust_files(dir: &Path) -> Result<Vec<PathBuf>> {
    files(dir, |path| path.extension().is_some_and(|ext| ext == "rs"))
}

/// Every file under `dir` accepted by `keep`, sorted. Hidden directories and
/// Cargo's `target/` are never entered.
pub fn files(dir: &Path, keep: impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !name.starts_with('.') && name != "target" {
                    walk(&path, keep, out)?;
                }
            } else if keep(&path) {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    walk(dir, &keep, &mut found)?;
    found.sort();
    Ok(found)
}

/// `path` relative to the workspace root, with `/` separators, for messages.
pub fn display(path: &Path) -> String {
    let root = root();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
