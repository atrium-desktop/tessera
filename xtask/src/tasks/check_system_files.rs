//! `system/` mirrors the installed system (ADR-0241).
//!
//! A file at `system/<path>` installs to `/usr/<path>`, except `system/etc/<path>`,
//! which installs to `/etc/<path>`. This gate fails closed when the tree, the
//! install manifest in `docs/dev/packaging.md`, and the documentation drift:
//!
//! 1. Every file under `system/` has exactly one manifest row whose destination
//!    is the one its path implies, and every `system/` row names a real file.
//! 2. Every file sits below a prefix directory (`bin`, `etc`, `lib`, …); files
//!    under `system/bin/` are executable and nothing else is.
//! 3. Every `system/<prefix dir>…` path quoted in living Markdown resolves. ADRs and the
//!    changelog are historical records and are not scanned.

use crate::workspace;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const MANIFEST: &str = "docs/dev/packaging.md";

/// Top-level directories of an install prefix (plus `etc`). Every file under
/// `system/` lives below one of them, and only `system/<one of these>` reads
/// as a reference in prose, so "system/microphone audio" is not a path.
const PREFIX_DIRS: [&str; 6] = ["bin", "etc", "lib", "libexec", "sbin", "share"];

/// Whether `relative` (a path below `system/`) starts in a prefix directory.
/// An empty path is `system/` itself.
fn in_prefix_dir(relative: &str) -> bool {
    let first = relative.split('/').next().unwrap_or_default();
    first.is_empty() || PREFIX_DIRS.contains(&first)
}

/// Installed destination implied by a path relative to `system/`.
fn destination(relative: &str) -> String {
    match relative.strip_prefix("etc/") {
        Some(rest) => format!("/etc/{rest}"),
        None => format!("/usr/{relative}"),
    }
}

/// `source -> destinations` for every manifest row whose source is in `system/`.
fn manifest_rows(markdown: &str) -> BTreeMap<String, Vec<String>> {
    let mut rows: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in markdown.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // `| source | destination | … |` splits into ["", source, destination, …].
        let (Some(source), Some(dest)) = (cells.get(1), cells.get(2)) else {
            continue;
        };
        let source = source.trim_matches('`');
        if let Some(relative) = source.strip_prefix("system/") {
            rows.entry(relative.to_owned())
                .or_default()
                .push(dest.trim_matches('`').to_owned());
        }
    }
    rows
}

/// `system/…` path tokens in Markdown, skipping placeholders such as `system/<path>`.
fn system_references(markdown: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (index, line) in markdown.lines().enumerate() {
        let mut offset = 0;
        while let Some(position) = line[offset..].find("system/") {
            let start = offset + position;
            offset = start + "system/".len();
            let preceded = line[..start].chars().next_back();
            if preceded.is_some_and(|c| c.is_alphanumeric() || matches!(c, '/' | '-' | '_' | '.')) {
                continue;
            }
            let token: String = line[start..]
                .chars()
                .take_while(|c| {
                    !c.is_whitespace() && !matches!(c, '`' | '"' | '\'' | ')' | ']' | ',')
                })
                .collect();
            let token = token.trim_end_matches(['.', ':', ';']);
            let relative = &token["system/".len()..];
            if !token.contains('<') && in_prefix_dir(relative) {
                found.push((index + 1, token.to_owned()));
            }
        }
    }
    found
}

/// Whether a referenced path exists. A `*` in the last segment is a glob that
/// must match at least one entry, so `system/share/applications/*.desktop`
/// fails once no desktop entry is left.
fn resolves(root: &Path, reference: &str) -> bool {
    let path = root.join(reference.trim_end_matches('/'));
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return false;
    };
    let Some((prefix, suffix)) = name.split_once('*') else {
        return path.exists();
    };
    let Some(Ok(entries)) = path.parent().map(fs::read_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let entry = entry.file_name().to_string_lossy().into_owned();
        entry.len() >= prefix.len() + suffix.len()
            && entry.starts_with(prefix)
            && entry.ends_with(suffix)
    })
}

pub fn run() -> Result<()> {
    let root = workspace::root();
    let tree = root.join("system");
    let files = workspace::files(&tree, |_| true)?;
    if files.is_empty() {
        bail!("system/ is empty or missing");
    }
    let manifest_path = root.join(MANIFEST);
    let manifest = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut rows = manifest_rows(&manifest);
    let mut violations = Vec::new();

    for file in &files {
        let relative = file
            .strip_prefix(&tree)
            .expect("walked under system/")
            .to_string_lossy()
            .replace('\\', "/");
        if !in_prefix_dir(&relative) {
            violations.push(format!(
                "system/{relative} is outside the prefix directories {PREFIX_DIRS:?}"
            ));
        }
        let expected = destination(&relative);
        match rows.remove(&relative).as_deref() {
            None => violations.push(format!("system/{relative} has no row in {MANIFEST}")),
            Some([dest]) if *dest == expected => {}
            Some([dest]) => violations.push(format!(
                "{MANIFEST}: system/{relative} installs to `{dest}`; its path implies `{expected}`"
            )),
            Some(_) => violations.push(format!(
                "{MANIFEST}: system/{relative} is listed more than once"
            )),
        }

        let mode = fs::metadata(file)?.permissions().mode();
        let executable = mode & 0o111 != 0;
        match (relative.starts_with("bin/"), executable) {
            (true, false) => violations.push(format!("system/{relative} must be executable")),
            (false, true) => violations.push(format!("system/{relative} must not be executable")),
            _ => {}
        }
    }
    for relative in rows.keys() {
        violations.push(format!(
            "{MANIFEST}: row names system/{relative}, which does not exist"
        ));
    }

    let docs = workspace::files(&root, |path| path.extension().is_some_and(|e| e == "md"))?;
    for doc in docs {
        let shown = workspace::display(&doc);
        if shown.starts_with("docs/adr/") || shown == "CHANGELOG.md" {
            continue;
        }
        for (line, reference) in system_references(&fs::read_to_string(&doc)?) {
            if !resolves(&root, &reference) {
                violations.push(format!("{shown}:{line}: `{reference}` does not exist"));
            }
        }
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("system files: {violation}");
        }
        bail!("{} system file violation(s)", violations.len());
    }
    println!(
        "System files: OK ({} files, manifest and documentation in sync)",
        files.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_mirror_the_prefix_except_etc() {
        assert_eq!(
            destination("bin/tessera-session"),
            "/usr/bin/tessera-session"
        );
        assert_eq!(
            destination("lib/systemd/user/tessera.service"),
            "/usr/lib/systemd/user/tessera.service"
        );
        assert_eq!(
            destination("etc/pam.d/tessera-lock"),
            "/etc/pam.d/tessera-lock"
        );
    }

    #[test]
    fn manifest_rows_keep_only_system_sources_and_duplicates() {
        let rows = manifest_rows(
            "| Source | Destination |\n|---|---|\n\
             | `target/release/tessera` | `/usr/bin/tessera` | core |\n\
             | `system/bin/a` | `/usr/bin/a` | core |\n\
             | `system/bin/a` | `/usr/bin/b` | core |\n",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["bin/a"], ["/usr/bin/a", "/usr/bin/b"]);
    }

    #[test]
    fn globs_must_match_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("apps")).unwrap();
        fs::write(dir.path().join("apps/a.desktop"), "").unwrap();
        assert!(resolves(dir.path(), "apps/*.desktop"));
        assert!(resolves(dir.path(), "apps/*"));
        assert!(resolves(dir.path(), "apps/"));
        assert!(!resolves(dir.path(), "apps/*.svg"));
        assert!(!resolves(dir.path(), "missing/*"));
    }

    #[test]
    fn references_skip_placeholders_and_foreign_prefixes() {
        let found = system_references(
            "install `system/etc/pam.d/x` and system/share/*.\n\
             `system/<path>` maps; target/system/y and my-system/z are not ours;\n\
             share system/microphone audio.",
        );
        let tokens: Vec<_> = found.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(tokens, ["system/etc/pam.d/x", "system/share/*"]);
    }
}
