//! Validate every declared dependency against explicit capability boundaries.
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Parser, Debug)]
pub struct CheckBoundariesArgs {}

type Policy = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;

fn check_manifest(manifest: &toml::Value, workspace: &toml::Value, policy: &Policy) -> Result<()> {
    let name = manifest["package"]["name"]
        .as_str()
        .context("package.name")?;
    let rules = policy
        .get(name)
        .with_context(|| format!("unclassified package: {name}"))?;
    let mut tables = vec![manifest];
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        tables.extend(targets.values());
    }
    for table in tables {
        for (section, kind) in [
            ("dependencies", "normal"),
            ("build-dependencies", "build"),
            ("dev-dependencies", "dev"),
        ] {
            let Some(deps) = table.get(section).and_then(toml::Value::as_table) else {
                continue;
            };
            for (alias, value) in deps {
                let value = if value.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    workspace
                        .get("dependencies")
                        .and_then(|deps| deps.get(alias))
                        .with_context(|| format!("missing workspace dependency {alias}"))?
                } else {
                    value
                };
                let dependency = value
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(alias);
                if dependency == "tessera"
                    || !rules
                        .get(kind)
                        .is_some_and(|allowed| allowed.contains(dependency))
                {
                    bail!("forbidden {kind} dependency: {name} -> {dependency}");
                }
            }
        }
    }
    Ok(())
}

pub fn run_check_boundaries(_args: CheckBoundariesArgs) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let policy: Policy = toml::from_str(include_str!("../../dependency-policy.toml"))?;
    let mut checked = BTreeSet::new();
    // Workspace members are deliberately flat and explicit in ADR-0159.
    let members = workspace["workspace"]["members"]
        .as_array()
        .context("workspace.members")?;
    for member in members {
        let member = member.as_str().context("member path")?;
        if !matches!(member, "crates/*" | "tooling/*") {
            bail!("unclassified workspace member pattern: {member}");
        }
        for entry in fs::read_dir(root.join(member.trim_end_matches("/*")))? {
            let path = entry?.path().join("Cargo.toml");
            if !path.exists() {
                continue;
            }
            let manifest: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
            check_manifest(&manifest, &workspace["workspace"], &policy)
                .with_context(|| path.display().to_string())?;
            checked.insert(
                manifest["package"]["name"]
                    .as_str()
                    .context("package.name")?
                    .to_owned(),
            );
        }
    }
    let declared: BTreeSet<_> = policy.keys().cloned().collect();
    if checked != declared {
        bail!("dependency policy contains missing or unclassified packages");
    }
    println!(
        "Crate dependency boundaries: OK ({} packages, normal/build/dev/target edges)",
        checked.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(body: &str) -> Result<()> {
        let manifest = toml::from_str(&format!("[package]\nname = 'shell'\n{body}"))?;
        let policy =
            toml::from_str("[shell]\nnormal = ['types']\nbuild = []\ndev = ['test-support']")?;
        let workspace =
            toml::from_str("[dependencies]\nrenamed = { package = 'tessera', path = '.' }")?;
        check_manifest(&manifest, &workspace, &policy)
    }
    #[test]
    fn conditional_and_build_edges_cannot_bypass_policy() {
        assert!(check("[target.'cfg(unix)'.dependencies]\ntessera = '1'").is_err());
        assert!(check("[build-dependencies]\ntypes = '1'").is_err());
    }
    #[test]
    fn aliases_cannot_hide_the_application() {
        assert!(check("[dependencies]\nrenamed.workspace = true").is_err());
        assert!(check("[dependencies]\nrenamed = { package = 'tessera', version = '1' }").is_err());
    }
    #[test]
    fn test_only_allowance_does_not_grant_production_access() {
        assert!(check("[dev-dependencies]\ntest-support = '1'").is_ok());
        assert!(check("[dependencies]\ntest-support = '1'").is_err());
        assert!(check("[dependencies]\ntypes = '1'").is_ok());
    }
    #[test]
    fn unknown_packages_fail_closed() {
        let manifest = toml::from_str("[package]\nname = 'unknown'").unwrap();
        assert!(
            check_manifest(
                &manifest,
                &toml::Value::Table(Default::default()),
                &Policy::new()
            )
            .is_err()
        );
    }
}
