//! Documentation integrity and audience-boundary gate.
//!
//! The taxonomy (`docs/dev/standards/taxonomy.md`) and the hyperlink standard
//! (`docs/dev/standards/style.md`) describe rules that nothing enforced: dead
//! relative links, stale kebab-case anchors, unresolved `<..._VERSION>`
//! placeholders, and user-facing pages that leak into maintainer procedures all
//! shipped because review was the only check.
//!
//! This gate makes those rules executable and fails closed on:
//!
//! 1. **Link targets** — every inline relative link resolves to a file (or
//!    directory) that exists, relative to the containing document.
//! 2. **Anchors** — a `#fragment` resolves to a heading in the target file
//!    using GitHub's kebab-case slug rules, including same-file fragments.
//! 3. **Scope firewall** — the four public quadrants
//!    (`tutorials/`, `how-to/`, `reference/`, `explanation/`) never link into
//!    `docs/dev/**`. Front-door pages (`README.md`, `CONTRIBUTING.md`,
//!    `docs/index.md`) and internal pages may.
//! 4. **Living-document placeholders** — `README.md` and the four user-facing
//!    quadrants must not contain an unresolved `<..._VERSION>` token; the
//!    pinned Optics release is concrete publishing information, not a template
//!    value. Maintainer pages under `docs/dev/` may show a release template.
//! 5. **Retired owner** — living documentation never points at a repository
//!    owner that no longer exists (`aegis-shell`).
//!
//! ADRs and `CHANGELOG.md` are historical records: their prose is never
//! rewritten, but their relative links still have to resolve and their anchors
//! still have to exist, because readers follow them.

use crate::error::{Context, Result, bail};
use crate::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

/// The public documentation quadrants that must not link into `docs/dev/`.
const PUBLIC_QUADRANTS: [&str; 4] = [
    "docs/tutorials",
    "docs/how-to",
    "docs/reference",
    "docs/explanation",
];

/// Pages allowed to link anywhere: they are the front doors, and every one of
/// them addresses contributors as well as users.
const FRONT_DOORS: [&str; 3] = ["README.md", "CONTRIBUTING.md", "docs/index.md"];

/// Repository owners that no longer exist. A live link to one is a 404.
const RETIRED_OWNERS: [&str; 1] = ["aegis-shell"];

/// One relative link found in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    line: usize,
    target: String,
}

/// GitHub's heading slug: lower-case, spaces to hyphens, punctuation dropped.
///
/// `is_alphanumeric` keeps non-Latin scripts, which is what GitHub does too.
fn slug(heading: &str) -> String {
    let mut out = String::new();
    for ch in heading.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch == ' ' {
            out.push('-');
        }
    }
    out
}

/// Every heading anchor a Markdown document exposes.
///
/// GitHub disambiguates repeated headings by appending `-1`, `-2`, … to the
/// second and later occurrence, so the anchor set has to do the same.
fn anchors(markdown: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut fenced = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&hashes) {
            continue;
        }
        let text = trimmed[hashes..].trim_start();
        if text.is_empty() {
            continue;
        }
        let base = slug(text);
        let occurrence = seen.entry(base.clone()).or_insert(0);
        found.insert(if *occurrence == 0 {
            base
        } else {
            format!("{base}-{occurrence}")
        });
        *occurrence += 1;
    }
    found
}

/// Inline links `[text](target "title")`, skipping fenced code blocks.
fn links(markdown: &str) -> Vec<Link> {
    let mut found = Vec::new();
    let mut fenced = false;
    for (index, line) in markdown.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        for target in destinations(line) {
            found.push(Link {
                line: index + 1,
                target,
            });
        }
    }
    found
}

/// Every `](…)` destination on one line.
fn destinations(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = line[cursor..].find("](") {
        let start = cursor + offset + 2;
        let rest = &line[start..];
        let mut depth = 0usize;
        let mut end = None;
        for (position, ch) in rest.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' if depth == 0 => {
                    end = Some(position);
                    break;
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let raw = rest[..end].trim();
        // `<paths with spaces>` keep their spaces; anything else stops at the
        // optional `"title"` separator.
        let destination = match raw.strip_prefix('<').and_then(|r| r.split_once('>')) {
            Some((path, _)) => path.trim(),
            None => raw.split_whitespace().next().unwrap_or_default(),
        };
        if !destination.is_empty() {
            found.push(destination.to_owned());
        }
        cursor = start + end + 1;
        if cursor >= line.len() {
            break;
        }
    }
    found
}

/// Whether a link destination is checked by this gate.
fn is_relative(target: &str) -> bool {
    !(target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("tel:"))
}

/// Whether a document belongs to one of the public quadrants.
fn is_public_quadrant(shown: &str) -> bool {
    PUBLIC_QUADRANTS
        .iter()
        .any(|prefix| shown.starts_with(prefix))
}

fn is_front_door(shown: &str) -> bool {
    FRONT_DOORS.contains(&shown)
}

/// The target file and optional fragment of one link.
fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (target, None),
    }
}

pub fn run() -> Result<()> {
    let root = workspace::root();
    let documents = workspace::files(&root, |path| path.extension().is_some_and(|e| e == "md"))?;
    let mut violations = Vec::new();

    // Anchors per file, computed once: a link may target a document that this
    // pass has not reached yet.
    let mut anchor_cache: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for document in &documents {
        let text = fs::read_to_string(document)
            .with_context(|| format!("reading {}", document.display()))?;
        anchor_cache.insert(document.clone(), anchors(&text));
    }

    for document in &documents {
        let shown = workspace::display(document);
        let text = fs::read_to_string(document)?;
        let public = is_public_quadrant(&shown);
        let front_door = is_front_door(&shown);

        for link in links(&text) {
            let target = link.target.as_str();
            if !is_relative(target) {
                continue;
            }
            // Templates and ADR standards use `NNNN-slug.md` placeholders.
            if target.contains("NNNN") || target.contains('<') {
                continue;
            }
            let (path, fragment) = split_target(target);
            let resolved = if path.is_empty() {
                document.clone()
            } else {
                let joined = document
                    .parent()
                    .expect("a Markdown file always has a parent")
                    .join(path);
                joined.canonicalize().unwrap_or(joined)
            };
            // Every destination stays inside the workspace: a relative link
            // that only resolves on the author's machine is dead for everyone
            // else. Sibling Optics ADRs are cited as absolute URLs pinned to
            // the release tag (docs/dev/standards/style.md).
            let root_text = root.to_string_lossy();
            let resolved_text = resolved.to_string_lossy();
            if !resolved_text.starts_with(root_text.as_ref()) {
                violations.push(format!(
                    "{shown}:{}: `{target}` escapes the repository",
                    link.line
                ));
                continue;
            }
            if !resolved.exists() {
                violations.push(format!(
                    "{shown}:{}: `{target}` does not resolve",
                    link.line
                ));
                continue;
            }
            if let Some(fragment) = fragment
                && !fragment.is_empty()
            {
                let known = match anchor_cache.get(&resolved) {
                    Some(known) => known,
                    None => {
                        // A target outside the walk (for example a generated
                        // file): compute it now rather than guessing.
                        let entry = anchors(&fs::read_to_string(&resolved)?);
                        anchor_cache.insert(resolved.clone(), entry);
                        &anchor_cache[&resolved]
                    }
                };
                if !known.contains(fragment) {
                    violations.push(format!(
                        "{shown}:{}: `#{fragment}` is not a heading in {}",
                        link.line,
                        workspace::display(&resolved)
                    ));
                }
            }
            if public && !front_door {
                let shown_target = workspace::display(&resolved);
                if shown_target.starts_with("docs/dev/") {
                    violations.push(format!(
                        "{shown}:{}: public documentation links into internal `{shown_target}`",
                        link.line
                    ));
                }
            }
        }

        for owner in RETIRED_OWNERS {
            let needle = format!("github.com/{owner}/");
            if text.contains(&needle) {
                violations.push(format!("{shown}: links to the retired owner `{owner}`"));
            }
        }

        // Publishing information is concrete. A maintainer page may show a
        // template on purpose, but a page a user follows to build the project
        // must not.
        if shown == "README.md" || public {
            for token in placeholder_versions(&text) {
                violations.push(format!(
                    "{shown}: unresolved version placeholder `{token}`; state the pinned release"
                ));
            }
        }
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("docs: {violation}");
        }
        bail!("{} documentation violation(s)", violations.len());
    }
    println!(
        "Documentation: OK ({} documents, links and anchors resolve, scope firewall holds)",
        documents.len()
    );
    Ok(())
}

/// `<OPTICS_VERSION>`-shaped tokens that should have been replaced.
fn placeholder_versions(markdown: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = markdown;
    while let Some(start) = rest.find('<') {
        let after = &rest[start..];
        let Some(end) = after.find('>') else { break };
        let token = &after[..=end];
        if token.ends_with("_VERSION>") && token.starts_with('<') {
            found.push(token.to_owned());
        }
        rest = &after[end..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_github_kebab_case() {
        assert_eq!(slug("Build & Run Guide"), "build--run-guide");
        assert_eq!(
            slug("5. Automated Verification & Testing"),
            "5-automated-verification--testing"
        );
        assert_eq!(
            slug("2. Scenario 0: Native-Free Rapid Logic Verification (Zero C Dependencies)"),
            "2-scenario-0-native-free-rapid-logic-verification-zero-c-dependencies"
        );
        assert_eq!(slug("`system/` Mirrors"), "system-mirrors");
    }

    #[test]
    fn anchors_ignore_fenced_code() {
        let text = "# Title\n## Real Heading\n```text\n# not a heading\n```\n";
        let found = anchors(text);
        assert!(found.contains("title"));
        assert!(found.contains("real-heading"));
        assert!(!found.contains("not-a-heading"));
    }

    #[test]
    fn links_skip_code_and_read_titles() {
        let text = "[a](one.md) [b](two.md \"title\") [c](<spaced path.md>)\n\
                    ```\n[f](ignored.md)\n```\n";
        let found = links(text);
        assert_eq!(
            found.iter().map(|l| l.target.as_str()).collect::<Vec<_>>(),
            ["one.md", "two.md", "spaced path.md"]
        );
        assert_eq!(found[0].line, 1);
    }

    #[test]
    fn scope_and_relative_rules_are_explicit() {
        assert!(is_public_quadrant("docs/reference/config.md"));
        assert!(!is_public_quadrant("docs/dev/workflow.md"));
        assert!(is_front_door("docs/index.md"));
        assert!(!is_relative("https://example.com"));
        assert!(is_relative("../adr/0001-x.md"));
        assert_eq!(split_target("a.md#b"), ("a.md", Some("b")));
        assert_eq!(split_target("#b"), ("", Some("b")));
    }

    #[test]
    fn placeholders_are_bounded_tokens() {
        assert_eq!(
            placeholder_versions("`v<OPTICS_VERSION>` and <TESSERA_VERSION> but <b>"),
            ["<OPTICS_VERSION>", "<TESSERA_VERSION>"]
        );
    }
}
