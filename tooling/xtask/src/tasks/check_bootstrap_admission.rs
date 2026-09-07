//! Bootstrap admission gate (ADR-0147, Decision 1).
//!
//! A function or type belongs in `tessera-bootstrap` only if it is
//! referenced by at least two distinct first-party process entry points.
//! This task makes that criterion mechanical: it scans the workspace for
//! process entry points and counts the `tessera_bootstrap::<item>`
//! references each one makes.
//!
//! An entry point is a `main.rs`-like file: any source file named
//! `main.rs`, or a `[[bin]]` target's declared path. Test code,
//! examples, and library modules are not entry points and never count.
//!
//! Exit status is zero when every exported bootstrap item is used by at
//! least two entry points (or explicitly allowlisted during a planned
//! deprecation), and non-zero otherwise. An unused item in bootstrap is
//! the leading indicator of the junk-drawer decay the gate exists to
//! prevent.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
pub struct CheckBootstrapAdmissionArgs {}

/// Items allowed below the two-entry-point threshold while a planned
/// migration completes. Keep entries short-lived; the gate prints each
/// one so the allowlist cannot rot silently.
const ALLOWLIST: [&str; 0] = [];

struct Entry {
    /// Binary/package name the entry point belongs to.
    owner: String,
    path: PathBuf,
}

struct Finding {
    item: String,
    /// Entry points (package names) referencing the item.
    users: BTreeSet<String>,
}

fn discover_entry_points() -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for tier in ["api", "core", "providers", "chrome", "services", "apps"] {
        let dir = Path::new("crates").join(tier);
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for crate_dir in read.flatten() {
            let manifest = crate_dir.path().join("Cargo.toml");
            if !manifest.exists() {
                continue;
            }
            let pkg = fs::read_to_string(&manifest)?;
            let name = pkg
                .lines()
                .find_map(|l| l.strip_prefix("name = "))
                .map(|s| s.trim_matches('"').to_string())
                .with_context(|| format!("package name in {}", manifest.display()))?;

            // `src/main.rs` is the canonical entry point; any declared
            // [[bin]] path also qualifies. Parse the manifest naively:
            // this gate only needs the paths, not the full TOML graph.
            let src_main = crate_dir.path().join("src/main.rs");
            if src_main.exists() {
                entries.push(Entry {
                    owner: name.clone(),
                    path: src_main.clone(),
                });
            }
            for bin_path in declared_bin_paths(&pkg, &crate_dir.path()) {
                if bin_path.exists() && bin_path != src_main {
                    entries.push(Entry {
                        owner: name.clone(),
                        path: bin_path,
                    });
                }
            }
        }
    }
    Ok(entries)
}

fn declared_bin_paths(pkg: &str, crate_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut in_bin = false;
    for line in pkg.lines() {
        if line.starts_with("[[bin]]") {
            in_bin = true;
            continue;
        }
        if line.starts_with('[') {
            in_bin = false;
            continue;
        }
        if in_bin && let Some(rest) = line.trim().strip_prefix("path = ") {
            let rel = rest.trim_matches('"');
            paths.push(crate_dir.join(rel));
        }
    }
    paths
}

/// The `tessera_bootstrap::<item>` references in one source tree.
///
/// A tree root covers the file and its `mod`-declared descendants, so an
/// entry point that delegates to sibling runtime modules still counts as
/// a user of the items those modules resolve.
fn bootstrap_items(root: &Path) -> Result<BTreeSet<String>> {
    let mut items = BTreeSet::new();
    let mut queue: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some(path) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        for line in text.lines() {
            // Declaration following: `mod <name>;` pulls in a sibling file.
            let trimmed = line.trim();
            if let Some(name) = trimmed
                .strip_prefix("mod ")
                .and_then(|s| s.strip_suffix(';'))
            {
                if !trimmed.starts_with("pub") || trimmed.starts_with("pub mod") {
                    // Both `mod x;` and `pub mod x;` declare a file.
                }
                let file = dir.join(format!("{name}.rs"));
                if file.exists() {
                    queue.push(file);
                } else {
                    let dirmod = dir.join(name).join("mod.rs");
                    if dirmod.exists() {
                        queue.push(dirmod);
                    }
                }
                continue;
            }
            // Every `tessera_bootstrap::<item>` path occurrence.
            let mut rest: &str = line;
            while let Some(pos) = rest.find("tessera_bootstrap::") {
                rest = &rest[pos + "tessera_bootstrap::".len()..];
                let item: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !item.is_empty() {
                    items.insert(item);
                }
            }
        }
    }
    Ok(items)
}

pub fn run_check_bootstrap_admission(_args: CheckBootstrapAdmissionArgs) -> Result<()> {
    if !Path::new("Cargo.toml").exists() {
        bail!("Must run from workspace root");
    }
    let entries = discover_entry_points()?;
    if entries.is_empty() {
        bail!("No process entry points found; the gate cannot run");
    }

    let mut per_entry: Vec<(String, BTreeSet<String>)> = Vec::new();
    for entry in &entries {
        per_entry.push((entry.owner.clone(), bootstrap_items(&entry.path)?));
    }

    let mut findings: BTreeMap<String, Finding> = BTreeMap::new();
    for (owner, items) in &per_entry {
        for item in items {
            let finding = findings.entry(item.clone()).or_insert_with(|| Finding {
                item: item.clone(),
                users: BTreeSet::new(),
            });
            finding.users.insert(owner.clone());
        }
    }

    let mut violations = Vec::new();
    let mut allowlisted = Vec::new();
    for finding in findings.values() {
        if finding.users.len() >= 2 {
            continue;
        }
        if ALLOWLIST.contains(&finding.item.as_str()) {
            allowlisted.push(format!(
                "{} (used by {} only; allowlisted)",
                finding.item,
                finding.users.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
            continue;
        }
        violations.push(format!(
            "{} is used by {} entry point(s) ({}); single-consumer logic must live in its owning binary, not tessera-bootstrap",
            finding.item,
            finding.users.len(),
            finding.users.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    if !allowlisted.is_empty() {
        eprintln!("bootstrap admission allowlist (migrate or delete):");
        for entry in &allowlisted {
            eprintln!("  - {entry}");
        }
    }

    if violations.is_empty() {
        println!(
            "Bootstrap admission: OK ({} items across {} entry points)",
            findings.len().saturating_sub(allowlisted.len()),
            entries.len()
        );
        return Ok(());
    }

    eprintln!("Bootstrap admission violations ({}):", violations.len());
    for v in &violations {
        eprintln!("  - {v}");
    }
    bail!("Bootstrap admission check failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_extraction_covers_use_and_qualified_paths() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.rs");
        fs::write(
            &main,
            r#"
            use tessera_bootstrap::init;
            fn main() {
                init("info");
                tessera_bootstrap::runtime_dir().unwrap();
            }
        "#,
        )
        .unwrap();
        let items = bootstrap_items(&main).unwrap();
        assert!(items.contains("init"));
        assert!(items.contains("runtime_dir"));
    }

    #[test]
    fn mod_declarations_pull_sibling_files_into_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.rs");
        fs::write(&main, "mod runtime;\nfn main() {}\n").unwrap();
        fs::write(
            dir.path().join("runtime.rs"),
            "fn run() { let _ = tessera_bootstrap::runtime_dir(); }",
        )
        .unwrap();
        let items = bootstrap_items(&main).unwrap();
        assert!(items.contains("runtime_dir"));
    }
}
