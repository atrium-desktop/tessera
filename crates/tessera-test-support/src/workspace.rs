//! Workspace paths and built-binary lookup for external test suites.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Absolute path to the workspace root.
///
/// This crate is compiled from `crates/tessera-test-support`, so the manifest
/// directory is stable even when a test binary runs from a different current
/// directory.
pub fn workspace_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

/// Absolute path to a crate directory such as `crates/tessera-ipc`.
pub fn crate_dir(name: &str) -> PathBuf {
    workspace_root().join("crates").join(name)
}

/// Absolute path to a file in the repository, relative to the workspace root.
pub fn workspace_path(relative: impl AsRef<Path>) -> PathBuf {
    workspace_root().join(relative)
}

/// Absolute path to a test fixture in the root `tests/fixtures` tree.
pub fn fixture(relative: impl AsRef<Path>) -> PathBuf {
    workspace_root().join("tests/fixtures").join(relative)
}

/// Directory containing built artifacts for the current test profile.
pub fn target_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TESSERA_TARGET_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            dir
        } else {
            workspace_root().join(dir)
        };
    }
    let exe = std::env::current_exe().expect("current test executable path");
    let mut ancestor = exe.as_path();
    while let Some(parent) = ancestor.parent() {
        if parent.file_name().is_some_and(|name| name == "deps") {
            return parent.parent().unwrap_or(parent).to_path_buf();
        }
        ancestor = parent;
    }
    exe.parent()
        .expect("test executable directory")
        .to_path_buf()
}

/// Path to a workspace binary, building it on first use when necessary.
///
/// Nextest does not set `CARGO_BIN_EXE_*` for binaries that belong to another
/// package, so the root external-test crate resolves the artifact from the
/// target directory and asks Cargo to build it once when missing. The build is
/// an ordinary `cargo build`; no xtask is involved in running tests.
pub fn binary(name: &str) -> PathBuf {
    if let Some(path) = env_binary(name) {
        return path;
    }

    let target = target_dir();
    let candidate = target.join(name);
    if candidate.is_file() {
        return candidate;
    }
    let release = target.join("release").join(name);
    if release.is_file() {
        return release;
    }

    let package = package_for_binary(name)
        .unwrap_or_else(|| panic!("no package mapping for test binary {name}"));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--locked", "-p", package, "--bin", name])
        .current_dir(workspace_root())
        .status()
        .unwrap_or_else(|error| panic!("build {name}: {error}"));
    assert!(
        status.success(),
        "cargo build -p {package} --bin {name} failed"
    );

    assert!(
        candidate.is_file(),
        "expected {name} at {} after cargo build",
        candidate.display()
    );
    candidate
}

fn env_binary(name: &str) -> Option<PathBuf> {
    let exact = [
        format!("NEXTEST_BIN_EXE_{name}"),
        format!("CARGO_BIN_EXE_{name}"),
    ];
    let underscored = [
        format!("NEXTEST_BIN_EXE_{}", name.replace('-', "_")),
        format!("CARGO_BIN_EXE_{}", name.replace('-', "_")),
    ];
    exact
        .into_iter()
        .chain(underscored)
        .find_map(|key| std::env::var_os(key).map(PathBuf::from))
        .filter(|path| path.is_file())
}

fn package_for_binary(name: &str) -> Option<&'static str> {
    Some(match name {
        "tessera" => "tessera",
        "tessera-ctl" => "tessera-cli",
        "tessera-mcp" => "tessera-mcp",
        "tessera-lock" | "tessera-lock-preview" => "tessera-lock",
        "tessera-idle" => "tessera-idle",
        "tessera-cast" | "tessera-castd" => "tessera-cast",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_contains_the_workspace_manifest() {
        assert!(workspace_root().join("Cargo.toml").is_file());
    }

    #[test]
    fn fixture_path_is_rooted_at_the_external_test_tree() {
        assert_eq!(
            fixture("wayland/clipboard_probe.c"),
            workspace_root().join("tests/fixtures/wayland/clipboard_probe.c")
        );
    }
}
