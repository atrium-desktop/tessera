//! Read and move the single Optics release tag every workspace dependency on
//! `github.com/ming2k/optics` is pinned to.
//!
//! Whether the pinned tag provides every symbol the workspace uses is not
//! checked here: a build without the local `[patch]` (canonical CI) compiles
//! against the tag and is the authoritative answer.

use crate::workspace;
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeSet;
use std::fs;
use toml_edit::{DocumentMut, Item, value};

const OPTICS_GIT: &str = "github.com/ming2k/optics";

#[derive(Parser, Debug)]
pub struct Args {
    /// Print only the pinned tag (for CI and scripts).
    #[arg(long, short, conflicts_with = "set")]
    tag_only: bool,

    /// Re-pin every Optics dependency in the workspace manifest to TAG.
    #[arg(long, value_name = "TAG")]
    set: Option<String>,
}

/// `(name, tag)` for every Optics entry in `[workspace.dependencies]`.
fn optics_pins(document: &DocumentMut) -> Result<Vec<(String, Option<String>)>> {
    let dependencies = document
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table_like)
        .context("the workspace manifest has no [workspace.dependencies]")?;
    let pins: Vec<_> = dependencies
        .iter()
        .filter(|(_, spec)| {
            spec.get("git")
                .and_then(Item::as_str)
                .is_some_and(|git| git.contains(OPTICS_GIT))
        })
        .map(|(name, spec)| {
            let tag = spec.get("tag").and_then(Item::as_str).map(str::to_owned);
            (name.to_owned(), tag)
        })
        .collect();
    if pins.is_empty() {
        bail!("no Optics dependencies in [workspace.dependencies]");
    }
    Ok(pins)
}

/// The one tag all Optics dependencies share.
fn pinned_tag(document: &DocumentMut) -> Result<String> {
    let mut tags = BTreeSet::new();
    for (name, tag) in optics_pins(document)? {
        tags.insert(tag.with_context(|| format!("Optics dependency `{name}` has no git tag"))?);
    }
    match tags.len() {
        1 => Ok(tags.pop_first().expect("one tag")),
        _ => bail!("Optics dependencies pin multiple tags: {tags:?}"),
    }
}

/// Re-pin every Optics dependency to `tag`, preserving the rest of the file.
fn set_tag(document: &mut DocumentMut, tag: &str) -> Result<usize> {
    if tag.trim().is_empty() {
        bail!("the Optics tag cannot be empty");
    }
    let names: Vec<String> = optics_pins(document)?.into_iter().map(|(n, _)| n).collect();
    for name in &names {
        document["workspace"]["dependencies"][name.as_str()]["tag"] = value(tag);
    }
    Ok(names.len())
}

pub fn run(args: Args) -> Result<()> {
    let manifest = workspace::root().join("Cargo.toml");
    let text =
        fs::read_to_string(&manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let mut document: DocumentMut = text.parse().context("parsing the workspace manifest")?;

    if let Some(tag) = args.set.as_deref() {
        let count = set_tag(&mut document, tag)?;
        fs::write(&manifest, document.to_string())?;
        println!("Pinned {count} Optics dependencies to `{tag}`");
        return Ok(());
    }

    let tag = pinned_tag(&document)?;
    if args.tag_only {
        println!("{tag}");
    } else {
        let names: Vec<_> = optics_pins(&document)?
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        println!("Optics release tag: {tag} ({})", names.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    const PINNED: &str = r#"
[workspace.dependencies]
other = "1.0"
flux = { git = "https://github.com/ming2k/optics", tag = "v0.0.50" }
lens = { git = "https://github.com/ming2k/optics", tag = "v0.0.50" } # keep me
"#;

    #[test]
    fn reads_the_single_pinned_tag() {
        assert_eq!(pinned_tag(&document(PINNED)).unwrap(), "v0.0.50");
    }

    #[test]
    fn set_rewrites_only_optics_tags_and_preserves_formatting() {
        let mut doc = document(PINNED);
        assert_eq!(set_tag(&mut doc, "v0.0.55").unwrap(), 2);
        let text = doc.to_string();
        assert_eq!(text, PINNED.replace("v0.0.50", "v0.0.55"));
    }

    #[test]
    fn refuses_diverging_missing_or_absent_pins() {
        let split = r#"
[workspace.dependencies]
flux = { git = "https://github.com/ming2k/optics", tag = "v1" }
lens = { git = "https://github.com/ming2k/optics", tag = "v2" }
"#;
        let error = pinned_tag(&document(split)).unwrap_err().to_string();
        assert!(error.contains("multiple tags"), "got: {error}");

        let untagged =
            "[workspace.dependencies]\nflux = { git = \"https://github.com/ming2k/optics\" }\n";
        let error = pinned_tag(&document(untagged)).unwrap_err().to_string();
        assert!(error.contains("no git tag"), "got: {error}");

        assert!(pinned_tag(&document("[workspace.dependencies]\nother = '1'\n")).is_err());
    }

    #[test]
    fn refuses_an_empty_tag() {
        assert!(set_tag(&mut document(PINNED), "  ").is_err());
    }
}
