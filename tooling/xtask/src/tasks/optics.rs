use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value;

#[derive(Parser, Debug)]
pub struct OpticsArgs {
    /// Print only the release tag for CI / scripts
    #[arg(long, short)]
    pub tag_only: bool,

    /// Update all Optics dependency tags in workspace Cargo.toml to the specified tag
    #[arg(long, value_name = "TAG")]
    pub set: Option<String>,

    /// Verify every Optics API this workspace references exists at the pinned
    /// tag. Requires a sibling `../optics` checkout with that tag fetched.
    #[arg(long)]
    pub verify_api: bool,

    /// Tag to verify against instead of the pinned one. Lets a candidate
    /// release be checked before editing Cargo.toml.
    #[arg(long, value_name = "TAG", requires = "verify_api")]
    pub tag: Option<String>,
}

/// Optics APIs Tessera depends on that did NOT exist in every released tag.
///
/// The pin in `Cargo.toml` is a tag, not a commit, so a local development
/// checkout can satisfy a symbol the pinned tag lacks. Canonical CI resolves
/// the tag and breaks; this list makes the coupling checkable instead of
/// discovered at release time.
///
/// Each entry is `(rust path, marker).` `marker` must appear in the Optics
/// source at the pinned tag.
const OPTICS_API_DEPENDENCIES: &[(&str, &str, &str)] = &[
    (
        "lens::Ui::damage_rect",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "fn damage_rect",
    ),
    (
        "lens_damage_rect (C)",
        "libs/lens/include/lens/lens.h",
        "lens_damage_rect",
    ),
    (
        "flux effect byte budget (ADR-0205)",
        "libs/flux/src/effect/effect.c",
        "EFFECT_POOL_BUDGET_BYTES",
    ),
    (
        "lens_snapshot_submit",
        "libs/lens/include/lens/lens.h",
        "lens_snapshot_submit",
    ),
    (
        "prism_material_output (ADR-0139)",
        "libs/prism/include/prism/types.h",
        "prism_material_output",
    ),
];

fn git(args: &[&str], repo: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Confirm every entry of [`OPTICS_API_DEPENDENCIES`] exists at `tag`.
fn verify_optics_api(tag: &str, optics_dir: &Path) -> Result<()> {
    if !optics_dir.join(".git").exists() {
        bail!(
            "no Optics checkout at {}; run this from a linked worktree (see .cargo/optics-local.toml)",
            optics_dir.display()
        );
    }
    // The tag must resolve locally. A missing tag is the common failure: the
    // checkout has the commit but nobody fetched the ref.
    if git(
        &["rev-parse", "--verify", &format!("{tag}^{{commit}}")],
        optics_dir,
    )
    .is_err()
    {
        bail!(
            "Optics tag `{tag}` is not present in {}; fetch it before verifying",
            optics_dir.display()
        );
    }

    let mut missing = Vec::new();
    for (api, path, marker) in OPTICS_API_DEPENDENCIES {
        // `git show <tag>:<path>` reads the file as it exists at that tag, so
        // the check describes what a fresh canonical build would compile
        // against — not what the working tree happens to contain.
        let spec = format!("{tag}:{path}");
        let found = git(&["show", &spec], optics_dir)
            .map(|content| content.contains(marker))
            .unwrap_or(false);
        if !found {
            missing.push(format!("{api}  ({path} has no `{marker}`)"));
        }
    }

    if missing.is_empty() {
        println!(
            "Optics API: OK ({} dependency/dependencies present at tag `{tag}`)",
            OPTICS_API_DEPENDENCIES.len()
        );
        return Ok(());
    }
    bail!(
        "Optics API drift: the pinned tag `{tag}` does not provide:\n  {}\n\n\
         Bump the pin (`cargo xtask optics --set <newer-tag>`) or land the change \
         upstream before release.",
        missing.join("\n  ")
    );
}

/// The single Optics release tag every workspace dependency is pinned to.
pub fn optics_tag_in_content(content: &str) -> Result<String> {
    let toml: Value = toml::from_str(content).context("parsing Cargo.toml")?;
    let deps = toml
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
        .context("No [workspace.dependencies] found in Cargo.toml")?;
    let mut tags = HashSet::new();
    for (name, dep) in deps {
        let Some(table) = dep.as_table() else {
            continue;
        };
        let is_optics = table
            .get("git")
            .and_then(|g| g.as_str())
            .is_some_and(|g| g.contains("github.com/ming2k/optics"));
        if !is_optics {
            continue;
        }
        match table.get("tag").and_then(|t| t.as_str()) {
            Some(tag) => {
                tags.insert(tag.to_string());
            }
            None => bail!("Optics dependency `{name}` is missing a git tag"),
        }
    }
    match tags.len() {
        0 => bail!("No Optics dependencies found in workspace.dependencies"),
        1 => Ok(tags.into_iter().next().expect("len checked")),
        _ => bail!("Optics dependencies pin multiple tags: {tags:?}"),
    }
}

pub fn update_optics_tags_in_content(content: &str, new_tag: &str) -> Result<(String, usize)> {
    if new_tag.trim().is_empty() {
        bail!("New Optics tag cannot be empty");
    }
    let mut updated_lines = Vec::new();
    let mut count = 0;

    for line in content.lines() {
        if line.contains("github.com/ming2k/optics") && line.contains("tag = \"") {
            // Replace tag = "..." with tag = "{new_tag}"
            if let Some(start_idx) = line.find("tag = \"") {
                let tag_val_start = start_idx + "tag = \"".len();
                if let Some(end_quote) = line[tag_val_start..].find('"') {
                    let end_idx = tag_val_start + end_quote;
                    let new_line =
                        format!("{}{}{}", &line[..tag_val_start], new_tag, &line[end_idx..]);
                    updated_lines.push(new_line);
                    count += 1;
                    continue;
                }
            }
        }
        updated_lines.push(line.to_string());
    }

    if count == 0 {
        bail!("No Optics dependencies with 'tag = \"...\"' found in Cargo.toml to update");
    }

    let mut new_content = updated_lines.join("\n");
    if content.ends_with('\n') {
        new_content.push('\n');
    }

    // Validate TOML syntax
    let toml_val: Value = toml::from_str(&new_content).context("validating modified Cargo.toml")?;
    let workspace_deps = toml_val
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
        .context("No [workspace.dependencies] found in modified Cargo.toml")?;

    for (name, dep) in workspace_deps {
        if let Some(table) = dep.as_table() {
            let is_optics = table
                .get("git")
                .and_then(|g| g.as_str())
                .is_some_and(|g| g.contains("github.com/ming2k/optics"));
            if is_optics {
                let tag = table.get("tag").and_then(|t| t.as_str());
                if tag != Some(new_tag) {
                    bail!(
                        "Failed to verify updated tag for '{}': expected '{}', found '{:?}'",
                        name,
                        new_tag,
                        tag
                    );
                }
            }
        }
    }

    Ok((new_content, count))
}

pub fn run_optics(args: OpticsArgs) -> Result<()> {
    let manifest_path = Path::new("Cargo.toml");

    if args.verify_api {
        let tag = match args.tag.clone() {
            Some(tag) => tag,
            None => {
                let content = fs::read_to_string(manifest_path).context("reading Cargo.toml")?;
                optics_tag_in_content(&content)?
            }
        };
        // Sibling checkout by convention; overridable for other layouts.
        let optics_dir = std::env::var_os("OPTICS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("../optics"));
        return verify_optics_api(&tag, &optics_dir);
    }

    if let Some(new_tag) = args.set.as_deref() {
        let content = fs::read_to_string(manifest_path).context("reading Cargo.toml")?;
        let (new_content, count) = update_optics_tags_in_content(&content, new_tag)?;
        fs::write(manifest_path, new_content).context("writing updated Cargo.toml")?;
        println!(
            "Successfully updated {} Optics dependencies to tag '{}' in Cargo.toml",
            count, new_tag
        );
        return Ok(());
    }
    let content = fs::read_to_string(manifest_path).context("reading Cargo.toml")?;
    let toml: Value = toml::from_str(&content).context("parsing Cargo.toml")?;

    let workspace_deps = toml
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table())
        .context("No [workspace.dependencies] found in Cargo.toml")?;

    let mut tags = HashSet::new();
    let mut optics_pkgs = Vec::new();

    for (name, dep) in workspace_deps {
        if let Some(table) = dep.as_table() {
            let is_optics = table
                .get("git")
                .and_then(|g| g.as_str())
                .is_some_and(|g| g.contains("github.com/ming2k/optics"));

            if is_optics {
                if let Some(tag) = table.get("tag").and_then(|t| t.as_str()) {
                    tags.insert(tag.to_string());
                    optics_pkgs.push((name.clone(), tag.to_string()));
                } else {
                    bail!("Optics dependency '{}' is missing a git tag", name);
                }
            }
        }
    }

    if optics_pkgs.is_empty() {
        bail!("No Optics dependencies found in workspace.dependencies");
    }

    if tags.len() > 1 {
        bail!("Multiple differing Optics tags found: {:?}", tags);
    }

    let resolved_tag = tags.into_iter().next().unwrap();

    if args.tag_only {
        println!("{}", resolved_tag);
    } else {
        println!(
            "Optics release tag: {} (across {} crates: {})",
            resolved_tag,
            optics_pkgs.len(),
            optics_pkgs
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_all_optics_tags_in_manifest() {
        let sample = r#"
[workspace.dependencies]
other = "1.0"
flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.50" }
lens = { git = "https://github.com/ming2k/optics", tag = "v0.0.50" }
"#;
        let (updated, count) =
            update_optics_tags_in_content(sample, "v0.0.55").expect("update failed");
        assert_eq!(count, 2);
        assert!(
            updated.contains(
                r#"flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.55" }"#
            )
        );
        assert!(
            updated.contains(
                r#"lens = { git = "https://github.com/ming2k/optics", tag = "v0.0.55" }"#
            )
        );
        assert!(updated.contains(r#"other = "1.0""#));
    }

    #[test]
    fn extracts_the_single_pinned_optics_tag() {
        let sample = r#"
[workspace.dependencies]
other = "1.0"
flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.67" }
lens = { git = "https://github.com/ming2k/optics", tag = "v0.0.67" }
"#;
        assert_eq!(optics_tag_in_content(sample).unwrap(), "v0.0.67");
    }

    #[test]
    fn refuses_a_workspace_pinning_two_optics_tags() {
        let sample = r#"
[workspace.dependencies]
flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.67" }
lens = { git = "https://github.com/ming2k/optics", tag = "v0.0.68" }
"#;
        let error = optics_tag_in_content(sample).unwrap_err().to_string();
        assert!(error.contains("multiple tags"), "got: {error}");
    }

    #[test]
    fn refuses_optics_without_a_tag() {
        let sample = r#"
[workspace.dependencies]
flux = { git = "https://github.com/ming2k/optics" }
"#;
        let error = optics_tag_in_content(sample).unwrap_err().to_string();
        assert!(error.contains("missing a git tag"), "got: {error}");
    }

    #[test]
    fn refuses_a_workspace_with_no_optics_at_all() {
        let sample = r#"
[workspace.dependencies]
other = "1.0"
"#;
        assert!(optics_tag_in_content(sample).is_err());
    }

    /// The guard exists because a local checkout can satisfy a symbol the
    /// pinned tag lacks, and only a canonical build notices. Assert it reports
    /// the drift rather than passing silently.
    #[test]
    fn api_verification_rejects_a_missing_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let error = verify_optics_api("v0.0.67", temp.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("no Optics checkout"), "got: {error}");
    }

    #[test]
    fn rejects_empty_optics_tag() {
        let sample = r#"
[workspace.dependencies]
flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.50" }
"#;
        assert!(update_optics_tags_in_content(sample, "   ").is_err());
    }
}
