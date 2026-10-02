use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use toml::Value;

#[derive(Parser, Debug)]
pub struct OpticsArgs {
    /// Print only the release tag for CI / scripts
    #[arg(long, short)]
    pub tag_only: bool,

    /// Update all Optics dependency tags in workspace Cargo.toml to the specified tag
    #[arg(long, value_name = "TAG")]
    pub set: Option<String>,
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
                    let new_line = format!(
                        "{}{}{}",
                        &line[..tag_val_start],
                        new_tag,
                        &line[end_idx..]
                    );
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
    let toml_val: Value =
        toml::from_str(&new_content).context("validating modified Cargo.toml")?;
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
        assert!(updated.contains(r#"flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.55" }"#));
        assert!(updated.contains(r#"lens = { git = "https://github.com/ming2k/optics", tag = "v0.0.55" }"#));
        assert!(updated.contains(r#"other = "1.0""#));
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
