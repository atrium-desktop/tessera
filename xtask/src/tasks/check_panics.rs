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

use crate::workspace;
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Dev-only package whose helpers may panic: it never ships in a binary.
const TEST_SUPPORT: &str = "tessera-test-support";

#[derive(Parser, Debug)]
pub struct Args {
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

/// Production panic counts per shipping package under `crates/`.
fn measure() -> Result<BTreeMap<String, Counts>> {
    let crates = workspace::root().join("crates");
    let mut by_package = BTreeMap::new();
    for package in workspace::packages()? {
        if !package.dir().starts_with(&crates) || package.name == TEST_SUPPORT {
            continue;
        }
        let src = package.dir().join("src");
        let counts: &mut Counts = by_package.entry(package.name.clone()).or_default();
        for file in workspace::rust_files(&src)? {
            let relative = file.strip_prefix(&src).unwrap_or(&file);
            let is_test = relative.file_name().is_some_and(|n| n == "tests.rs")
                || relative.components().any(|c| c.as_os_str() == "tests");
            if is_test {
                continue;
            }
            let tokens = production_tokens(&fs::read_to_string(&file)?);
            counts.panic_sites += panic_sites(&tokens);
            counts.poison_locks += poison_locks(&tokens);
        }
    }
    Ok(by_package)
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

pub fn run(args: Args) -> Result<()> {
    let path = workspace::xtask_path("panic-baseline.toml");
    let measured = measure()?;

    let poison: usize = measured.values().map(|c| c.poison_locks).sum();
    if args.update {
        write_baseline(&path, &measured)?;
        println!(
            "Panic baseline updated: {} crates, {poison} poisonable lock(s) remaining",
            measured.len()
        );
        return Ok(());
    }

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
