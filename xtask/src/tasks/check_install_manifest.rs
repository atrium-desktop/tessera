//! Install completeness gate: every shipped binary is packaged, and every
//! packaged binary exists.
//!
//! `check-system-files` covers the `system/` tree. The product binaries are
//! shipped from `target/release/` and live outside that tree, so a new trusted
//! sibling could be added to the workspace and the installer while the
//! distribution manifest silently kept shipping a package without it. That is
//! exactly what happened to `tessera-cast`/`tessera-castd`: `scripts/install.sh`
//! installed them, the compositor supervises them, and the documented package
//! did not include them, so a packaged system lost casting with no error.
//!
//! This gate makes the three sources of truth agree:
//!
//! 1. **Workspace** — every binary target Cargo builds for a shipping package.
//! 2. **Installer** — the `BINARIES` list `scripts/install.sh` validates and
//!    installs.
//! 3. **Manifest** — the `target/release/<name>` rows of the install manifest in
//!    `docs/dev/packaging.md`.
//!
//! Every binary the workspace builds must appear in both the installer and the
//! manifest unless it is explicitly declared development-only below, and every
//! name in either list must correspond to a real binary target. Releasing is a
//! decision about *these* lists; this gate refuses to let them drift apart.

use crate::error::{Context, Result, bail};
use crate::workspace;
use std::collections::BTreeSet;
use std::fs;

const MANIFEST: &str = "docs/dev/packaging.md";
const INSTALLER: &str = "scripts/install.sh";
const PACKAGE: &str = "tessera";

/// Binary targets that are not distribution artifacts.
///
/// `xtask` is repository automation, and `tessera-lock-preview` is the
/// feature-gated contributor harness the packaging guide explicitly excludes.
const DEV_ONLY: [&str; 2] = ["xtask", "tessera-lock-preview"];

/// Binary targets `scripts/install.sh` installs, from its quoted `BINARIES`
/// list.
fn installer_binaries(source: &str) -> Result<BTreeSet<String>> {
    let start = source
        .find("BINARIES='")
        .context("scripts/install.sh has no `BINARIES='` list")?
        + "BINARIES='".len();
    let rest = &source[start..];
    let end = rest
        .find('\'')
        .context("scripts/install.sh BINARIES list is unterminated")?;
    Ok(rest[..end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// `target/release/<name>` sources in the install manifest table.
fn manifest_binaries(markdown: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for line in markdown.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let Some(source) = cells.get(1) else { continue };
        let source = source.trim_matches('`');
        if let Some(name) = source.strip_prefix("target/release/")
            && !name.is_empty()
            && !name.contains('/')
        {
            found.insert(name.to_owned());
        }
    }
    found
}

/// Binary targets Cargo builds for the shipping package(s).
///
/// `workspace::packages` resolves members, renames, and target kinds, so this
/// set cannot disagree with what `cargo build --workspace --bins` produces.
fn workspace_binaries() -> Result<BTreeSet<String>> {
    let packages = workspace::packages()?;
    let mut found = BTreeSet::new();
    for package in &packages {
        // Only the product packages ship; test support and the test entry
        // package are workspace members without installable binaries anyway.
        if package.name == "tessera-tests" || package.name == "tessera-test-support" {
            continue;
        }
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == "bin")
                && !DEV_ONLY.contains(&target.name.as_str())
            {
                found.insert(target.name.clone());
            }
        }
    }
    Ok(found)
}

pub fn run() -> Result<()> {
    let root = workspace::root();
    let manifest_path = root.join(MANIFEST);
    let manifest = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let installer_path = root.join(INSTALLER);
    let installer = fs::read_to_string(&installer_path)
        .with_context(|| format!("reading {}", installer_path.display()))?;

    let built = workspace_binaries()?;
    let installed = installer_binaries(&installer)?;
    let packaged = manifest_binaries(&manifest);
    let mut violations = Vec::new();

    for name in &built {
        if !installed.contains(name) {
            violations.push(format!("{INSTALLER}: does not install `{name}`"));
        }
        if !packaged.contains(name) {
            violations.push(format!(
                "{MANIFEST}: has no row for `target/release/{name}`"
            ));
        }
    }
    for name in &installed {
        if !built.contains(name) {
            violations.push(format!(
                "{INSTALLER}: installs `{name}`, which is not a built binary"
            ));
        }
        if !packaged.contains(name) {
            violations.push(format!(
                "{MANIFEST}: omits `{name}`, which {INSTALLER} installs"
            ));
        }
    }
    for name in &packaged {
        if !built.contains(name) {
            violations.push(format!(
                "{MANIFEST}: lists `target/release/{name}`, which is not a built binary"
            ));
        }
        if !installed.contains(name) {
            violations.push(format!(
                "{MANIFEST}: lists `{name}`, which {INSTALLER} does not install"
            ));
        }
    }

    // The packaging guide documents a package name for every artifact; catch
    // an obviously wrong package identity before it reaches a PKGBUILD.
    if !manifest.contains(&format!("`{PACKAGE}`")) {
        violations.push(format!(
            "{MANIFEST}: does not mention the `{PACKAGE}` package"
        ));
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("install manifest: {violation}");
        }
        bail!("{} install manifest violation(s)", violations.len());
    }
    println!(
        "Install manifest: OK ({} shipped binaries agree across the workspace, installer, and manifest)",
        built.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_list_is_read_as_names() {
        let script = "#!/bin/sh\nBINARIES='\ntessera\ntessera-castd\n'\n";
        assert_eq!(
            installer_binaries(script).unwrap(),
            ["tessera".to_owned(), "tessera-castd".to_owned()]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        assert!(installer_binaries("#!/bin/sh\n").is_err());
        assert!(installer_binaries("BINARIES='\ntessera\n").is_err());
    }

    #[test]
    fn only_binary_rows_of_the_manifest_are_read() {
        let markdown = "| Source | Destination |\n|---|---|\n\
                        | `target/release/tessera` | `/usr/bin/tessera` | core |\n\
                        | `system/bin/tessera-session` | `/usr/bin/tessera-session` | core |\n\
                        | `LICENSE` | `/usr/share/licenses/tessera/LICENSE` | core |\n";
        assert_eq!(
            manifest_binaries(markdown),
            ["tessera".to_owned()].into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn development_only_targets_are_named_explicitly() {
        // A new dev harness must be added here deliberately, not silently
        // shipped or silently dropped.
        assert!(DEV_ONLY.contains(&"xtask"));
        assert!(DEV_ONLY.contains(&"tessera-lock-preview"));
        assert!(!DEV_ONLY.contains(&"tessera-castd"));
    }
}
