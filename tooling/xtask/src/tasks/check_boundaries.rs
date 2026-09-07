//! Mechanical enforcement of the tiered workspace dependency law.
//!
//! Layout tiers and their dependency rules (see `docs/dev/project-layout.md`):
//!
//! ```text
//!   api      -> only api + external dependencies
//!   core     -> api
//!   chrome   -> api            (never core, never other chrome crates)
//!   services -> api + core
//!   apps     -> everything
//! ```
//!
//! The rules are structural, not enumerated: any crate placed in a tier
//! inherits that tier's rule automatically. A move between directories is
//! therefore itself an architecture decision, and this check fails the build
//! the moment an actual dependency contradicts the declared tier. It never
//! special-cases individual crates.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use toml::Value;

#[derive(Parser, Debug)]
pub struct CheckBoundariesArgs {}

/// Dependency tiers by directory under `crates/`.
const TIERS: [&str; 6] = ["api", "core", "providers", "chrome", "services", "apps"];

/// Which tiers a given tier may depend on, including its own tier.
/// `apps` depends on everything, so it has no restriction.
fn allowed_tiers(tier: &str) -> Option<&'static [&'static str]> {
    match tier {
        "api" => Some(&["api"]),
        "core" => Some(&["api", "core"]),
        "providers" => Some(&["api", "core", "providers"]),
        "chrome" => Some(&["api", "providers", "chrome"]),
        "services" => Some(&["api", "core", "providers", "services"]),
        "apps" => None,
        _ => bail_tier(tier),
    }
}

fn bail_tier(tier: &str) -> Option<&'static [&'static str]> {
    let _ = tier;
    unreachable!("TIERS and allowed_tiers must stay in sync")
}

struct Crate {
    name: String,
    tier: &'static str,
    internal_deps: Vec<String>,
}

fn discover_crates() -> Result<Vec<Crate>> {
    let mut crates = Vec::new();
    for tier in TIERS {
        let dir = Path::new("crates").join(tier);
        let entries = fs::read_dir(&dir).with_context(|| format!("reading {}/", dir.display()))?;
        for entry in entries {
            let manifest = entry?.path().join("Cargo.toml");
            if !manifest.exists() {
                continue;
            }
            let content = fs::read_to_string(&manifest)?;
            let toml: Value = toml::from_str(&content)
                .with_context(|| format!("parsing {}", manifest.display()))?;
            let name = toml
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .context("package.name missing")?
                .to_string();
            let mut internal_deps = Vec::new();
            if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
                for dep in deps.keys() {
                    if dep.starts_with("tessera-") || dep == "tessera" {
                        internal_deps.push(dep.clone());
                    }
                }
            }
            internal_deps.sort();
            crates.push(Crate {
                name,
                tier,
                internal_deps,
            });
        }
    }
    Ok(crates)
}

pub fn run_check_boundaries(_args: CheckBoundariesArgs) -> Result<()> {
    if !Path::new("Cargo.toml").exists() {
        bail!("Must run from workspace root");
    }
    let crates = discover_crates()?;
    let tier_of: BTreeMap<&str, &str> = crates.iter().map(|c| (c.name.as_str(), c.tier)).collect();

    let mut violations: Vec<String> = Vec::new();

    for krate in &crates {
        let Some(allowed) = allowed_tiers(krate.tier) else {
            continue; // apps depends on everything
        };
        for dep in &krate.internal_deps {
            let dep_tier = tier_of.get(dep.as_str()).copied().or_else(|| {
                // "tessera" is the app crate; tolerate the bare-name form.
                (dep == "tessera").then_some("apps")
            });
            match dep_tier {
                Some(t) if allowed.contains(&t) => {}
                Some(t) => violations.push(format!(
                    "{tier}/{} must not depend on {t}/{} (tier {tier} may only depend on tiers: {allowed})",
                    krate.name,
                    dep,
                    tier = krate.tier,
                    allowed = allowed.join(", "),
                )),
                None => violations.push(format!(
                    "unknown internal dependency {} -> {} (not a workspace crate?)",
                    krate.name, dep
                )),
            }
        }
    }

    if violations.is_empty() {
        println!(
            "Crate dependency boundaries: OK ({} crates across {} tiers)",
            crates.len(),
            TIERS.len()
        );
        return Ok(());
    }

    eprintln!("Architectural violations ({}):", violations.len());
    for v in &violations {
        eprintln!("  - {v}");
    }
    bail!("Crate boundary check failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_may_not_reach_core() {
        assert!(allowed_tiers("api").unwrap().contains(&"api"));
        assert!(!allowed_tiers("api").unwrap().contains(&"core"));
    }

    #[test]
    fn chrome_never_reaches_core_or_services() {
        let allowed = allowed_tiers("chrome").unwrap();
        assert!(!allowed.contains(&"core"));
        assert!(!allowed.contains(&"services"));
    }

    #[test]
    fn chrome_may_consume_providers() {
        assert!(allowed_tiers("chrome").unwrap().contains(&"providers"));
    }

    #[test]
    fn providers_never_reach_chrome_or_services() {
        let allowed = allowed_tiers("providers").unwrap();
        assert!(!allowed.contains(&"chrome"));
        assert!(!allowed.contains(&"services"));
    }

    #[test]
    fn services_reach_api_and_core_but_not_chrome() {
        let allowed = allowed_tiers("services").unwrap();
        assert!(allowed.contains(&"api") && allowed.contains(&"core"));
        assert!(!allowed.contains(&"chrome"));
    }
}
