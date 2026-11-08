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

/// One Optics symbol this workspace compiles against, and which release state
/// it is expected to be in.
struct ApiDependency {
    /// What the workspace uses it for, as a log line.
    api: &'static str,
    /// Path inside the Optics checkout.
    path: &'static str,
    /// A marker that must appear in that file.
    marker: &'static str,
    /// `Present`: the pinned tag must provide it, and its absence means the pin
    /// is too old for this workspace.
    ///
    /// `PendingRelease`: the workspace consumes it from a linked worktree and the
    /// pinned tag must **not** provide it. Two things make this state worth
    /// asserting rather than tolerating:
    ///
    /// * an entry whose tag has caught up is a stale record — the gate would go
    ///   on believing a released symbol is unreleased, and the next promotion
    ///   would not know it may drop the entry;
    /// * the reverse direction is what an under-reporting list looked like: this
    ///   check was previously green while the working tree already consumed a
    ///   half-dozen symbols `v0.0.74` never shipped, because every entry was
    ///   only ever compared in the direction that could not fail until release.
    ///
    /// A pending entry is therefore a *debt marker*: it must move to `Present`
    /// (and the pin must move with it) by the time a release is cut, and
    /// `cargo xtask optics --verify-api` fails if the tag ships one that is still
    /// marked pending.
    state: ApiState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApiState {
    /// Released: the pinned tag provides it.
    Present,
    /// Consumed from a linked worktree; the pinned tag must not provide it yet.
    PendingRelease,
}

const fn released(api: &'static str, path: &'static str, marker: &'static str) -> ApiDependency {
    ApiDependency {
        api,
        path,
        marker,
        state: ApiState::Present,
    }
}

#[allow(dead_code)] // Used by tests and by the next linked-worktree feature cycle.
const fn pending(api: &'static str, path: &'static str, marker: &'static str) -> ApiDependency {
    ApiDependency {
        api,
        path,
        marker,
        state: ApiState::PendingRelease,
    }
}

/// Every Optics symbol this workspace compiles against.
///
/// The pin in `Cargo.toml` is a tag, not a commit, so a local development
/// checkout can satisfy a symbol the pinned tag lacks. Canonical CI resolves
/// the tag and breaks; this list makes the coupling checkable instead of
/// discovered at release time. Entries stay here for as long as the workspace
/// uses the symbol — this is a dependency manifest, not a changelog.
static OPTICS_API_DEPENDENCIES: &[ApiDependency] = &[
    released(
        "lens::Ui::damage_rects",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "fn damage_rects",
    ),
    released(
        "lens_damage_rects (C)",
        "libs/lens/include/lens/lens.h",
        "lens_damage_rects",
    ),
    released(
        "flux effect byte budget (ADR-0205)",
        "libs/flux/src/effect/effect.c",
        "EFFECT_POOL_BUDGET_BYTES",
    ),
    released(
        "lens_snapshot_submit",
        "libs/lens/include/lens/lens.h",
        "lens_snapshot_submit",
    ),
    released(
        "flux_frame_materialize_image (ADR-0147)",
        "libs/flux/include/flux/core.h",
        "flux_frame_materialize_image",
    ),
    released(
        "flux_frame_plan_stats (ADR-0147)",
        "libs/flux/include/flux/core.h",
        "flux_frame_plan_stats",
    ),
    // ---- released with Optics v0.0.75 --------------------------------------
    //
    // These entries were consumed from the linked worktree before v0.0.75.
    // They stay grouped so the next promotion can compare the released surface
    // against the workspace in one place.
    released(
        "flux canvas pass damage set (ADR-0149)",
        "libs/flux/include/flux/canvas.h",
        "flux_damage damage;",
    ),
    released(
        "flux::CanvasPassOptions::damage (ADR-0149)",
        "bindings/flux-rs/crates/flux/src/lib.rs",
        "pub damage: Vec<CanvasRenderArea>",
    ),
    released(
        "flux_set_diagnostic (ADR-0153)",
        "libs/flux/include/flux/core.h",
        "flux_set_diagnostic",
    ),
    released(
        "flux::ErrorInfo::last_if_code (ADR-0153)",
        "bindings/flux-rs/crates/flux/src/lib.rs",
        "fn last_if_code",
    ),
    released(
        "lens_caret_flip_pending (ADR-0151)",
        "libs/lens/include/lens/lens.h",
        "lens_caret_flip_pending",
    ),
    released(
        "lens_caret_clock_live (ADR-0151)",
        "libs/lens/include/lens/lens.h",
        "lens_caret_clock_live",
    ),
    released(
        "lens_set_caret_blink (ADR-0151)",
        "libs/lens/include/lens/lens.h",
        "lens_set_caret_blink",
    ),
    released(
        "lens::Ui::caret_flip_pending (ADR-0151)",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "fn caret_flip_pending",
    ),
    released(
        "lens::Ui::set_caret_blink (ADR-0151)",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "fn set_caret_blink",
    ),
    released(
        "lens::DEFAULT_CARET_BLINK_PERIOD_S (ADR-0151)",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "DEFAULT_CARET_BLINK_PERIOD_S",
    ),
    released(
        "lens::Ui::focus (host-side focus transfer)",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "pub fn focus",
    ),
    released(
        "lens::Snapshot::retain (ADR-0153)",
        "bindings/lens-rs/crates/lens/src/lib.rs",
        "pub fn retain",
    ),
    released(
        "svg::Document (ADR-0222)",
        "bindings/svg-rs/crates/svg/src/lib.rs",
        "pub struct Document",
    ),
    released(
        "svg::Document::parse_bytes (ADR-0222)",
        "bindings/svg-rs/crates/svg/src/lib.rs",
        "pub fn parse_bytes",
    ),
    released(
        "svg::Document::rasterize (ADR-0222)",
        "bindings/svg-rs/crates/svg/src/lib.rs",
        "pub fn rasterize",
    ),
    released(
        "svg::Limits (ADR-0222)",
        "bindings/svg-rs/crates/svg/src/lib.rs",
        "pub struct Limits",
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
    verify_api_dependencies(OPTICS_API_DEPENDENCIES, tag, optics_dir)
}

/// Confirm every entry of `deps` exists at `tag`.
fn verify_api_dependencies(deps: &[ApiDependency], tag: &str, optics_dir: &Path) -> Result<()> {
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

    let mut unpinned = Vec::new();
    let mut stale_pending = Vec::new();
    for dep in deps {
        // `git show <tag>:<path>` reads the file as it exists at that tag, so
        // the check describes what a fresh canonical build would compile
        // against — not what the working tree happens to contain.
        let spec = format!("{tag}:{}", dep.path);
        let found = git(&["show", &spec], optics_dir)
            .map(|content| content.contains(dep.marker))
            .unwrap_or(false);
        match (dep.state, found) {
            // The pin is the source of truth for a released symbol.
            (ApiState::Present, false) => unpinned.push(format!(
                "{}  ({} has no `{}`)",
                dep.api, dep.path, dep.marker
            )),
            // A pending entry the tag already ships is a record that has come
            // true: keeping it would make the gate assert the *opposite* of
            // reality for every symbol promoted behind it.
            (ApiState::PendingRelease, true) => stale_pending.push(format!(
                "{}  ({} already provides `{}`)",
                dep.api, dep.path, dep.marker
            )),
            _ => {}
        }
    }

    let pending_count = deps
        .iter()
        .filter(|dep| dep.state == ApiState::PendingRelease)
        .count();

    if unpinned.is_empty() && stale_pending.is_empty() {
        println!(
            "Optics API: OK ({} released + {pending_count} pending-release, tag `{tag}`)",
            deps.len() - pending_count
        );
        if pending_count > 0 {
            println!(
                "  {pending_count} symbol(s) are consumed from the linked worktree and absent \
                 from `{tag}`; the next Optics release must ship them and this list must move \
                 them to `released(..)` in the same commit that bumps the pin."
            );
        }
        return Ok(());
    }

    let mut report = String::new();
    if !unpinned.is_empty() {
        report.push_str(&format!(
            "the pinned tag `{tag}` does not provide:\n  {}\n",
            unpinned.join("\n  ")
        ));
    }
    if !stale_pending.is_empty() {
        report.push_str(&format!(
            "`{tag}` already provides these, so the entries are stale:\n  {}\n",
            stale_pending.join("\n  ")
        ));
    }
    bail!(
        "Optics API drift:\n  {report}\n\
         For a missing symbol, bump the pin (`cargo xtask optics --set <newer-tag>`) or \
         land the change upstream. For a stale pending entry, move it to `released(..)` in \
         the same commit as the bump — `v{tag}` cannot both ship it and be recorded as \
         not shipping it."
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

    /// A real Optics checkout at a tag, as a throwaway git repo, so the two
    /// directions of the drift check can be exercised without the sibling
    /// repository's history.
    fn fake_optics(tag: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "test"]);
        // The test creates a lightweight tag. A global `tag.gpgSign` or
        // `commit.gpgSign` setting turns that into a signed object and needs
        // identity material the test must not depend on.
        git(&["config", "commit.gpgSign", "false"]);
        git(&["config", "tag.gpgSign", "false"]);
        git(&["config", "tag.forceSignAnnotated", "false"]);
        for (path, body) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, body).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-qm", "seed"]);
        git(&["tag", tag]);
        dir
    }

    /// The direction the old `missing` list could not express: an entry marked
    /// `PendingRelease` that the tag already ships means the *record* is stale,
    /// not that the pin is behind. Both failures are `bail!`; a check that only
    /// ever looked one way silently accepted a workspace consuming a dozen
    /// unreleased symbols and reported "OK" for months.
    #[test]
    fn api_verification_reports_a_pending_entry_the_tag_already_ships() {
        let dir = fake_optics(
            "v0.0.1",
            &[("libs/lens/include/lens/lens.h", "lens_caret_flip_pending")],
        );
        let deps = [pending(
            "lens_caret_flip_pending (ADR-0151)",
            "libs/lens/include/lens/lens.h",
            "lens_caret_flip_pending",
        )];
        let error = verify_api_dependencies(&deps, "v0.0.1", dir.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("already provides"), "got: {error}");
        assert!(
            error.contains("move it to `released(..)`"),
            "the error must name the fix, got: {error}"
        );
    }

    /// And the direction it *did* express: a `Present` entry the tag lacks
    /// means the pin is too old for this workspace.
    #[test]
    fn api_verification_reports_a_released_entry_the_tag_lacks() {
        let dir = fake_optics("v0.0.1", &[("libs/flux/include/flux/core.h", "unrelated")]);
        let deps = [released(
            "lens::Ui::damage_rects",
            "libs/flux/include/flux/core.h",
            "fn damage_rects",
        )];
        let error = verify_api_dependencies(&deps, "v0.0.1", dir.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not provide"), "got: {error}");
        assert!(
            error.contains("lens::Ui::damage_rects"),
            "the report must name which symbol is missing, got: {error}"
        );
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
