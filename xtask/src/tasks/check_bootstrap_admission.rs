//! Bootstrap admission gate (ADR-0147, Decision 1).
//!
//! An item belongs in `tessera-env` only if at least two distinct first-party
//! packages reference it from their process entry points. Entry points are
//! the binary targets Cargo reports for packages under `crates/`; each one is
//! followed through its `mod` declarations so a `main.rs` that delegates to
//! sibling modules still counts as a user of what those modules reference.
//! An item with a single consumer is the leading indicator of the
//! junk-drawer decay the gate exists to prevent: it belongs in its owner.

use crate::error::{Context, Result, bail};
use crate::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const PREFIX: &str = "tessera_env::";

/// `tessera_env::<item>` references reachable from one entry-point file.
fn bootstrap_items(entry: &Path) -> Result<BTreeSet<String>> {
    let mut items = BTreeSet::new();
    let mut queue: Vec<PathBuf> = vec![entry.to_path_buf()];
    let mut seen = BTreeSet::new();
    while let Some(path) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        for line in text.lines() {
            let declaration = line.trim();
            let declaration = declaration.strip_prefix("pub ").unwrap_or(declaration);
            if let Some(name) = declaration
                .strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
            {
                let file = dir.join(format!("{name}.rs"));
                let nested = dir.join(name).join("mod.rs");
                queue.extend([file, nested].into_iter().find(|p| p.is_file()));
                continue;
            }
            let mut rest = line;
            while let Some(position) = rest.find(PREFIX) {
                rest = &rest[position + PREFIX.len()..];
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

pub fn run() -> Result<()> {
    let crates = workspace::root().join("crates");
    let mut users: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut entry_points = 0;
    for package in workspace::packages()? {
        if !package.dir().starts_with(&crates) {
            continue;
        }
        for source in package.bin_sources() {
            entry_points += 1;
            for item in bootstrap_items(source)? {
                users.entry(item).or_default().insert(package.name.clone());
            }
        }
    }
    if entry_points == 0 {
        bail!("no process entry points found; the gate cannot run");
    }

    let violations: Vec<String> = users
        .iter()
        .filter(|(_, packages)| packages.len() < 2)
        .map(|(item, packages)| {
            let owners: Vec<_> = packages.iter().map(String::as_str).collect();
            format!(
                "tessera_env::{item} is used only by {}; single-consumer logic belongs \
                 in its owning binary",
                owners.join(", ")
            )
        })
        .collect();
    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("bootstrap admission: {violation}");
        }
        bail!("{} bootstrap admission violation(s)", violations.len());
    }
    println!(
        "Bootstrap admission: OK ({} items across {entry_points} entry points)",
        users.len()
    );
    Ok(())
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
            "use tessera_env::init;\nfn main() { init(); tessera_env::runtime_dir(); }\n",
        )
        .unwrap();
        let items = bootstrap_items(&main).unwrap();
        assert!(items.contains("init"));
        assert!(items.contains("runtime_dir"));
    }

    #[test]
    fn mod_declarations_pull_sibling_and_nested_files_into_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.rs");
        fs::write(&main, "mod runtime;\npub mod nested;\nfn main() {}\n").unwrap();
        fs::write(
            dir.path().join("runtime.rs"),
            "fn a() { tessera_env::runtime_dir(); }",
        )
        .unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(
            dir.path().join("nested/mod.rs"),
            "fn b() { tessera_env::init(); }",
        )
        .unwrap();
        let items = bootstrap_items(&main).unwrap();
        assert!(items.contains("runtime_dir"));
        assert!(items.contains("init"));
    }
}
