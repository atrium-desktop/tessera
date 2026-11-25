//! Workspace paths and built-binary lookup for external test suites.

use std::path::{Path, PathBuf};

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

/// Directory containing built artifacts for the current test profile.
///
/// `TESSERA_TARGET_DIR` overrides the lookup. Otherwise the directory is
/// derived from the running test executable, which Cargo always places in
/// `<target-dir>/[<triple>/]<profile>/deps/`. That path already accounts for
/// `CARGO_TARGET_DIR`, `--profile` and `--target`, so `CARGO_TARGET_DIR` is
/// deliberately not read here: it names only the root, and joining a binary
/// name to it misses the profile directory the binaries are built into.
pub fn target_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TESSERA_TARGET_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().expect("current test executable path");
    profile_dir_for_executable(&exe)
}

/// The profile artifact directory for a Cargo-built executable path: the
/// parent of the nearest `deps` ancestor, or the executable's own directory.
fn profile_dir_for_executable(exe: &Path) -> PathBuf {
    let mut ancestor = exe;
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

/// Path to an already-built workspace binary.
///
/// Nextest does not set `CARGO_BIN_EXE_*` for binaries that belong to another
/// package, so the root external-test crate resolves the artifact from the
/// target directory. Building here is deliberately not supported: a test runs
/// while the test runner holds Cargo's target-directory lock, so a nested
/// `cargo build` blocks on that lock until the test times out and looks like a
/// product hang. `.config/nextest.toml` builds the siblings once per run with a
/// setup script, and this lookup fails with the exact remediation when that
/// did not happen.
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

    let package = package_for_binary(name).unwrap_or("<unknown package>");
    panic!(
        "workspace binary `{name}` is missing from {}.\n\
         The root suites spawn production binaries and never build them: this \
         test already holds Cargo's target-directory lock, so a nested build \
         would deadlock until the test timeout.\n\
         Run `cargo build --locked -p {package} --bin {name}` first, or run the \
         suites through `cargo nextest run`, whose setup script builds every \
         workspace binary once before the tests start.",
        target.display()
    );
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
        "tessera-mcp" => "tessera-mcp",
        "tessera-lock" | "tessera-lock-preview" => "tessera-lock",
        "tessera-settings" | "tessera-settings-preview" => "tessera-settings",
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
    fn profile_dir_is_the_parent_of_deps_under_a_custom_target_dir() {
        // `CARGO_TARGET_DIR=/build/custom cargo nextest run` puts the test
        // executable here; sibling binaries live in `/build/custom/debug/`.
        let exe = Path::new("/build/custom/debug/deps/e2e-0123456789abcdef");
        assert_eq!(
            profile_dir_for_executable(exe),
            PathBuf::from("/build/custom/debug")
        );
    }

    #[test]
    fn profile_dir_keeps_the_target_triple_and_named_profile() {
        let exe = Path::new("/w/target/x86_64-unknown-linux-gnu/release-fast/deps/e2e-0123");
        assert_eq!(
            profile_dir_for_executable(exe),
            PathBuf::from("/w/target/x86_64-unknown-linux-gnu/release-fast")
        );
    }

    #[test]
    fn profile_dir_without_deps_is_the_executable_directory() {
        let exe = Path::new("/w/target/debug/tessera");
        assert_eq!(
            profile_dir_for_executable(exe),
            PathBuf::from("/w/target/debug")
        );
    }
}
