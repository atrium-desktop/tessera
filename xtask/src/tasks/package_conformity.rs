//! Every workspace member inherits the workspace lint table and declares every
//! dependency through `[workspace.dependencies]`, so versions, features, and
//! sources have a single owner.

use crate::error::{Context, Result, bail};
use crate::workspace;
use std::fs;
use toml::Value;

#[derive(Debug, Default)]
pub struct Args {
    /// Append `[lints] workspace = true` to members that lack it.
    pub fix: bool,
}

const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

fn inherits_lints(manifest: &Value) -> bool {
    manifest
        .get("lints")
        .and_then(|lints| lints.get("workspace"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Dependency keys, across plain and `[target.*]` tables, not declared with
/// `workspace = true`.
fn uninherited_dependencies(manifest: &Value) -> Vec<String> {
    let mut tables = vec![manifest];
    if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
        tables.extend(targets.values());
    }
    let mut found = Vec::new();
    for table in tables {
        for section in SECTIONS {
            let Some(deps) = table.get(section).and_then(Value::as_table) else {
                continue;
            };
            for (name, spec) in deps {
                if spec.get("workspace").and_then(Value::as_bool) != Some(true) {
                    found.push(format!("{section}.{name}"));
                }
            }
        }
    }
    found
}

pub fn run(args: Args) -> Result<()> {
    let packages = workspace::packages()?;
    let mut violations = Vec::new();
    for package in &packages {
        let path = &package.manifest_path;
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let manifest: Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let shown = workspace::display(path);

        if !inherits_lints(&manifest) {
            if args.fix && manifest.get("lints").is_none() {
                fs::write(path, format!("{text}\n[lints]\nworkspace = true\n"))?;
                println!("fixed: {shown} now inherits workspace lints");
            } else {
                violations.push(format!("{shown}: missing `[lints] workspace = true`"));
            }
        }
        for dependency in uninherited_dependencies(&manifest) {
            violations.push(format!(
                "{shown}: `{dependency}` must be declared in [workspace.dependencies] \
                 and inherited with `workspace = true`"
            ));
        }
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("package conformity: {violation}");
        }
        bail!("{} package conformity violation(s)", violations.len());
    }
    println!("Package conformity: OK ({} packages)", packages.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_uninherited_dependencies_in_every_table() {
        let manifest: Value = toml::from_str(
            "[dependencies]\na.workspace = true\nb = '1'\n\
             [dev-dependencies]\nc = { workspace = true, features = ['x'] }\n\
             [target.'cfg(unix)'.build-dependencies]\nd = { version = '1' }",
        )
        .unwrap();
        assert_eq!(
            uninherited_dependencies(&manifest),
            ["dependencies.b", "build-dependencies.d"]
        );
    }

    #[test]
    fn lint_inheritance_requires_an_explicit_true() {
        assert!(inherits_lints(
            &toml::from_str("[lints]\nworkspace = true").unwrap()
        ));
        assert!(!inherits_lints(
            &toml::from_str("[lints]\nworkspace = false").unwrap()
        ));
        assert!(!inherits_lints(
            &toml::from_str("[package]\nname = 'x'").unwrap()
        ));
    }
}
