//! Enforce the panic discipline of `docs/dev/error-handling.md`
//! (ADR-0195).
//!
//! Two rules over **production** code (test modules are excluded):
//!
//! 1. **No poisonable lock may panic** — `.lock()`, `.read()`, or `.write()`
//!    followed by `.unwrap()`/`.expect(`. A single panicking thread poisons the
//!    mutex and turns every later acquisition into a panic, cascading one
//!    worker's failure across the compositor. Recovery is
//!    `unwrap_or_else(|e| e.into_inner())`. This rule is a hard zero.
//! 2. **No new panic sites** — the remaining `unwrap`/`expect`/`panic!`/
//!    `unreachable!`/`todo!`/`unimplemented!` sites are grandfathered in a
//!    checked-in baseline. Counts may fall but never rise, so the debt is
//!    visible and can only shrink.
//!
//! Test code is excluded with the repository's own convention: production code
//! is everything *before* the first `#[cfg(test)]` item, and the `tests/`
//! directories and `tests.rs` modules are never production.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
pub struct CheckPanicsArgs {
    /// Rewrite the baseline from the current tree (use when deliberately
    /// lowering it, or after a change that removes panic sites).
    #[arg(long)]
    update: bool,
}

/// Production-code panic counters for one crate.
#[derive(Default)]
struct Counts {
    panic_sites: usize,
    poison_locks: usize,
}

/// Emit only production tokens: skip `#[cfg(test)]` items (brace- or
/// semicolon-terminated) so test code never counts, and collapse whitespace so
/// a lock chain split across lines still reads as one token.
///
/// Test attribute detection is brace-aware rather than "everything after the
/// first `#[cfg(test)]`", because many files gate a helper mid-file and keep
/// production code below it.
fn production_tokens(source: &str) -> String {
    let mut out = String::new();
    let mut depth: i32 = 0;
    // Depth at which a `#[cfg(test)]` item began, and whether its body brace
    // has opened yet (false while the item is a brace-less `use`/`mod`).
    let mut skip: Option<(i32, bool)> = None;
    for raw_line in source.lines() {
        if skip.is_none() && raw_line.trim_start().starts_with("#[cfg(test)]") {
            skip = Some((depth, false));
        }
        for ch in raw_line.chars() {
            match ch {
                '{' => {
                    if let Some((_, opened)) = skip.as_mut() {
                        *opened = true;
                    }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    if let Some((start, opened)) = skip
                        && opened
                        && depth == start
                    {
                        skip = None;
                    }
                }
                ';' => {
                    if let Some((_, opened)) = skip
                        && !opened
                    {
                        skip = None;
                    }
                }
                _ => {}
            }
            if skip.is_none() {
                out.push(ch);
            }
        }
        out.push(' ');
    }
    out.split_whitespace().collect()
}

fn occurrences(haystack: &str, needles: &[&str]) -> usize {
    needles.iter().map(|n| haystack.matches(n).count()).sum()
}

/// Poisonable lock acquisitions that panic on a poisoned mutex.
fn poison_locks(tokens: &str) -> usize {
    occurrences(
        tokens,
        &[
            ".lock().unwrap()",
            ".lock().expect(",
            ".read().unwrap()",
            ".read().expect(",
            ".write().unwrap()",
            ".write().expect(",
        ],
    )
}

/// Every panic-capable site, poison locks included.
fn panic_sites(tokens: &str) -> usize {
    occurrences(
        tokens,
        &[
            ".unwrap()",
            ".expect(",
            "panic!(",
            "unreachable!(",
            "todo!(",
            "unimplemented!(",
        ],
    )
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn measure(root: &Path) -> Result<BTreeMap<String, Counts>> {
    let crates = root.join("crates");
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path.join("src"), &mut files)?;
        }
    }
    let mut by_crate: BTreeMap<String, Counts> = BTreeMap::new();
    for file in files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name == "tests.rs" || file.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let relative = file.strip_prefix(&crates).unwrap_or(&file);
        let Some(crate_name) = relative.components().next() else {
            continue;
        };
        let tokens = production_tokens(&fs::read_to_string(&file)?);
        let counts = by_crate
            .entry(crate_name.as_os_str().to_string_lossy().into_owned())
            .or_default();
        counts.panic_sites += panic_sites(&tokens);
        counts.poison_locks += poison_locks(&tokens);
    }
    Ok(by_crate)
}

fn baseline_path(root: &Path) -> PathBuf {
    root.join("tooling/xtask/panic-baseline.toml")
}

fn read_baseline(path: &Path) -> Result<BTreeMap<String, usize>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read baseline {} (run with --update)", path.display()))?;
    let value: toml::Value = toml::from_str(&text)?;
    let counts = value
        .get("counts")
        .and_then(toml::Value::as_table)
        .context("baseline has no [counts] table")?;
    Ok(counts
        .iter()
        .filter_map(|(k, v)| v.as_integer().map(|n| (k.clone(), n.max(0) as usize)))
        .collect())
}

fn write_baseline(path: &Path, counts: &BTreeMap<String, Counts>) -> Result<()> {
    let mut out = String::from(
        "# Production panic-site baseline (ADR-0195). Counts may fall, never rise.\n\
         # Regenerate with `cargo run -p xtask -- check-panics --update`.\n\
         # Poisonable-lock panics are a hard zero and are not listed here.\n\n[counts]\n",
    );
    for (name, c) in counts {
        out.push_str(&format!("{name} = {}\n", c.panic_sites));
    }
    fs::write(path, out)?;
    Ok(())
}

pub fn run_check_panics(args: CheckPanicsArgs) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let measured = measure(&root)?;

    let poison: usize = measured.values().map(|c| c.poison_locks).sum();
    if args.update {
        write_baseline(&baseline_path(&root), &measured)?;
        println!(
            "Panic baseline updated: {} crates, {poison} poisonable lock(s) remaining",
            measured.len()
        );
        return Ok(());
    }

    let path = baseline_path(&root);
    let baseline = read_baseline(&path)?;
    let mut violations = Vec::new();

    if poison != 0 {
        let worst: Vec<String> = measured
            .iter()
            .filter(|(_, c)| c.poison_locks > 0)
            .map(|(name, c)| format!("{name}: {}", c.poison_locks))
            .collect();
        violations.push(format!(
            "INV-ARCH-51 violation: {poison} poisonable lock(s) panic on poison ({})",
            worst.join(", ")
        ));
    }

    let mut total = 0usize;
    for (name, counts) in &measured {
        total += counts.panic_sites;
        match baseline.get(name) {
            Some(allowed) if counts.panic_sites > *allowed => violations.push(format!(
                "INV-ARCH-51 violation: {name} has {} panic site(s), baseline {allowed}; \
                 fix the site or justify raising the baseline in review",
                counts.panic_sites
            )),
            None if counts.panic_sites > 0 => violations.push(format!(
                "INV-ARCH-51 violation: new crate `{name}` has {} panic site(s) and no baseline",
                counts.panic_sites
            )),
            _ => {}
        }
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("{violation}");
        }
        bail!("{} panic-discipline violation(s)", violations.len());
    }

    let slack: usize = measured
        .iter()
        .filter_map(|(name, c)| baseline.get(name).map(|b| b.saturating_sub(c.panic_sites)))
        .sum();
    println!(
        "Panic discipline: OK (0 poisonable locks; {total} grandfathered panic site(s); \
         {slack} below baseline — lower it with --update)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_test_items_and_collapses_chains() {
        let source = "let g = m.lock()\n    .unwrap();\n#[cfg(test)]\nmod tests {\n    m.lock().unwrap();\n}\nlet h = n.lock().unwrap();";
        let tokens = production_tokens(source);
        // A mid-file `#[cfg(test)]` item is skipped, but production resumes.
        assert_eq!(poison_locks(&tokens), 2);
        assert!(!tokens.contains("modtests"));
    }

    #[test]
    fn skips_braceless_test_items() {
        let source = "#[cfg(test)]\nuse std::sync::Mutex;\nfn real() { m.lock().unwrap(); }";
        let tokens = production_tokens(source);
        assert_eq!(poison_locks(&tokens), 1);
    }

    #[test]
    fn recovering_lock_is_not_a_poison_site() {
        let tokens = "m.lock().unwrap_or_else(|e|e.into_inner())";
        assert_eq!(poison_locks(tokens), 0);
        // ...but it still counts one `.unwrap` for the coarse ratchet.
        assert_eq!(panic_sites(tokens), 0, "unwrap_or_else is not .unwrap()");
    }

    #[test]
    fn counts_every_panic_form() {
        let tokens = "x.unwrap(); y.expect(\"z\"); panic!(\"no\"); todo!(); unreachable!();";
        assert_eq!(panic_sites(tokens), 5);
    }
}
