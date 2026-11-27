//! Every workspace member inherits the workspace lint table and declares every
//! dependency through `[workspace.dependencies]`, so versions, features, and
//! sources have a single owner. Every member is also named and placed by the
//! crate naming rule of ADR-0263.

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

/// The product namespace every first-party package carries (ADR-0066).
const NAMESPACE: &str = "tessera";

/// Directory that holds every package except the root members below.
const CRATES_DIR: &str = "crates";

/// Workspace members that live at the repository root rather than under
/// `crates/`, with the package name each directory must hold. `xtask` keeps
/// the Cargo-ecosystem tooling name instead of the namespace (ADR-0240).
const ROOT_MEMBERS: [(&str, &str); 2] = [("tests", "tessera-tests"), ("xtask", "xtask")];

/// Violations of the ADR-0263 naming rule for package `name` whose manifest
/// sits in `dir`, given relative to the workspace root with `/` separators.
fn naming_violations(name: &str, dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    let exempt = ROOT_MEMBERS
        .iter()
        .any(|&(_, member)| member == name && !member.starts_with(NAMESPACE));
    if !exempt {
        let role = name.strip_prefix(NAMESPACE);
        let namespaced = match role {
            Some("") => true,
            Some(rest) => rest.strip_prefix('-').is_some_and(is_kebab_role),
            None => false,
        };
        if !namespaced {
            found.push(format!(
                "package `{name}` must be `{NAMESPACE}` or `{NAMESPACE}-<role>` \
                 with a lowercase kebab-case role"
            ));
        }
    }
    if let Some(&(member_dir, _)) = ROOT_MEMBERS.iter().find(|&&(_, member)| member == name) {
        if dir != member_dir {
            found.push(format!(
                "package `{name}` lives in `{dir}`; it is a root member and \
                 belongs in `{member_dir}`"
            ));
        }
        return found;
    }
    match dir.split_once('/') {
        Some((CRATES_DIR, leaf)) if !leaf.contains('/') => {
            if leaf != name {
                found.push(format!(
                    "package `{name}` lives in `{dir}`; its directory must be \
                     `{CRATES_DIR}/{name}`"
                ));
            }
        }
        _ => match ROOT_MEMBERS
            .iter()
            .find(|&&(member_dir, _)| member_dir == dir)
        {
            Some(&(_, expected)) => found.push(format!(
                "`{dir}` is reserved for package `{expected}`, not `{name}`"
            )),
            None => found.push(format!(
                "package `{name}` lives in `{dir}`; packages live directly in \
                 `{CRATES_DIR}/<package-name>`"
            )),
        },
    }
    found
}

/// A non-empty run of lowercase ASCII alphanumeric words joined by single `-`.
fn is_kebab_role(role: &str) -> bool {
    !role.is_empty()
        && role.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

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
        let dir = workspace::display(package.dir());
        violations.extend(naming_violations(&package.name, &dir));

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
    fn accepts_namespaced_packages_in_matching_directories() {
        assert!(naming_violations("tessera", "crates/tessera").is_empty());
        assert!(naming_violations("tessera-desktop", "crates/tessera-desktop").is_empty());
        assert!(
            naming_violations(
                "tessera-wayland-protocols",
                "crates/tessera-wayland-protocols"
            )
            .is_empty()
        );
        assert!(naming_violations("tessera-tests", "tests").is_empty());
        assert!(naming_violations("xtask", "xtask").is_empty());
    }

    #[test]
    fn rejects_packages_outside_the_namespace() {
        for name in [
            "desktop",
            "tesseract",
            "tessera_desktop",
            "tessera-",
            "tessera-Desktop",
            "tessera--x",
            "tessera-x-",
        ] {
            let found = naming_violations(name, &format!("crates/{name}"));
            assert_eq!(found.len(), 1, "{name}: {found:?}");
            assert!(found[0].contains("must be `tessera`"), "{name}: {found:?}");
        }
    }

    #[test]
    fn rejects_directories_that_disagree_with_the_package() {
        let found = naming_violations("tessera-desktop", "crates/desktop");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("must be `crates/tessera-desktop`"));
    }

    #[test]
    fn rejects_nested_and_unlisted_root_placements() {
        for dir in [
            "crates/system/tessera-desktop",
            "tooling/tessera-desktop",
            "tessera-desktop",
        ] {
            let found = naming_violations("tessera-desktop", dir);
            assert_eq!(found.len(), 1, "{dir}: {found:?}");
            assert!(
                found[0].contains("packages live directly in"),
                "{dir}: {found:?}"
            );
        }
    }

    #[test]
    fn root_member_directories_are_reserved_for_their_package() {
        let found = naming_violations("tessera-e2e", "tests");
        assert_eq!(
            found,
            ["`tests` is reserved for package `tessera-tests`, not `tessera-e2e`"]
        );
        // A root member's name and its directory are reserved together.
        assert_eq!(
            naming_violations("xtask", "crates/xtask"),
            ["package `xtask` lives in `crates/xtask`; it is a root member and belongs in `xtask`"]
        );
        assert_eq!(
            naming_violations("tessera-tests", "crates/tessera-tests").len(),
            1
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
