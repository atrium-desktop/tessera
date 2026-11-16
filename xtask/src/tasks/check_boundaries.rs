//! Validate every declared dependency against explicit capability boundaries
//! (ADR-0159).
//!
//! Edges come from `cargo metadata`, so renamed, platform-specific, build, and
//! dev dependencies are all seen exactly as Cargo resolves them.

use crate::workspace::{self, Package};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

/// Allowed dependencies per package, keyed by edge kind (`normal`/`build`/`dev`).
type Policy = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;

/// The compositor package is an application, not a library. Only the root
/// external test suite may depend on it, to exercise the production binaries.
const APPLICATION: &str = "tessera";
const APPLICATION_CONSUMER: &str = "tessera-tests";

fn check_package(package: &Package, policy: &Policy) -> Result<()> {
    let name = package.name.as_str();
    let rules = policy
        .get(name)
        .with_context(|| format!("unclassified package: {name}"))?;
    for dependency in &package.dependencies {
        let kind = dependency.kind();
        let target = dependency.name.as_str();
        let application_edge = target == APPLICATION && name != APPLICATION_CONSUMER;
        let allowed = rules.get(kind).is_some_and(|set| set.contains(target));
        if application_edge || !allowed {
            bail!("forbidden {kind} dependency: {name} -> {target}");
        }
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let policy_path = workspace::xtask_path("dependency-policy.toml");
    let policy: Policy = toml::from_str(
        &std::fs::read_to_string(&policy_path)
            .with_context(|| format!("reading {}", policy_path.display()))?,
    )?;
    let packages = workspace::packages()?;
    for package in &packages {
        check_package(package, &policy)?;
    }

    let members: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let declared: BTreeSet<&str> = policy.keys().map(String::as_str).collect();
    let stale: Vec<_> = declared.difference(&members).collect();
    if !stale.is_empty() {
        bail!("dependency policy lists packages that are not workspace members: {stale:?}");
    }
    println!(
        "Crate dependency boundaries: OK ({} packages, normal/build/dev/target edges)",
        members.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Dependency;

    fn package(name: &str, edges: &[(&str, Option<&str>)]) -> Package {
        Package {
            name: name.to_owned(),
            manifest_path: "Cargo.toml".into(),
            targets: Vec::new(),
            dependencies: edges
                .iter()
                .map(|(dep, kind)| Dependency {
                    name: (*dep).to_owned(),
                    kind: kind.map(str::to_owned),
                })
                .collect(),
        }
    }

    fn policy() -> Policy {
        toml::from_str(
            "[shell]\nnormal = ['types']\nbuild = []\ndev = ['test-support']\n\
             [tessera-tests]\nnormal = ['tessera']\nbuild = []\ndev = []",
        )
        .unwrap()
    }

    #[test]
    fn edges_are_checked_per_kind() {
        let policy = policy();
        assert!(check_package(&package("shell", &[("types", None)]), &policy).is_ok());
        assert!(check_package(&package("shell", &[("types", Some("build"))]), &policy).is_err());
        assert!(
            check_package(&package("shell", &[("test-support", Some("dev"))]), &policy).is_ok()
        );
        assert!(check_package(&package("shell", &[("test-support", None)]), &policy).is_err());
    }

    #[test]
    fn only_the_external_suite_may_depend_on_the_application() {
        let mut policy = policy();
        policy
            .get_mut("shell")
            .unwrap()
            .get_mut("normal")
            .unwrap()
            .insert(APPLICATION.to_owned());
        assert!(check_package(&package("shell", &[(APPLICATION, None)]), &policy).is_err());
        assert!(check_package(&package("tessera-tests", &[(APPLICATION, None)]), &policy).is_ok());
    }

    #[test]
    fn unknown_packages_fail_closed() {
        assert!(check_package(&package("unknown", &[]), &policy()).is_err());
    }
}
