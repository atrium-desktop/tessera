//! Enforce the ADR registry invariants of `docs/dev/standards/adr.md` §5.
//!
//! The record corpus is append-only: identities, numbering, and the index are
//! contracts, and drift in any of them silently corrupts the project's
//! decision lineage. ADR-0080 once shipped twice — two files, two index rows —
//! because nothing checked the registry mechanically. This gate makes the
//! documented rules executable and fails closed on:
//!
//! 1. **Numbering** — `docs/adr/NNNN-slug.md` where `NNNN` is unique, and the
//!    frontmatter `id` agrees with the filename number. Gaps are permitted (a
//!    retired number is never reused) but duplicates are not.
//! 2. **Frontmatter** — `id`, `title`, `status`, and `date` are present, and
//!    `status` is one of the lifecycle states.
//! 3. **Supersession** — `status: superseded` names a `superseded_by`, and every
//!    `superseded_by` points at a record that exists.
//! 4. **Index integrity** — every record is registered in `index.md`, every row
//!    resolves to a file, and rows ascend monotonically by number.
//! 5. **Lineage links** — every `NNNN-slug.md` link in the corpus resolves to an
//!    existing record, so `Amends:`/`superseded_by` references cannot rot.
//! 6. **Status agreement** — the record carries exactly one body `- Status:`
//!    line, and it and the index status cell both open with the capitalized
//!    form of the frontmatter status; a superseded record names its successor
//!    in both, and only a superseded record may carry `superseded_by`.
//! 7. **Reciprocal lineage** — every `Amends:`/`Extends:`/`Supersedes:` header
//!    link is answered by an `Amended by`/`Extended by`/`Superseded …` header
//!    line in the target that links back (standards §3).

use crate::error::{Context, Result, bail};
use crate::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const STATUSES: &[&str] = &["draft", "accepted", "superseded", "rejected", "deprecated"];

/// The capitalized word a body `- Status:` line and an index cell open with.
fn status_word(status: &str) -> Option<&'static str> {
    Some(match status {
        "draft" => "Draft",
        "accepted" => "Accepted",
        "superseded" => "Superseded",
        "rejected" => "Rejected",
        "deprecated" => "Deprecated",
        _ => return None,
    })
}

/// The leading run of ASCII letters: `"Superseded by [..]"` -> `"Superseded"`.
fn leading_word(text: &str) -> &str {
    let end = text
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    &text[..end]
}

/// The record header: everything before the first `## ` section.
fn header(source: &str) -> &str {
    let end = source
        .match_indices("\n## ")
        .next()
        .map_or(source.len(), |(at, _)| at);
    &source[..end]
}

/// Header bullets with their two-space continuation lines folded in.
fn bullets(header: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for line in header.lines() {
        if line.starts_with("- ") {
            out.push(line.to_owned());
            open = true;
        } else if open && line.starts_with("  ") {
            let last = out.last_mut().expect("open bullet");
            last.push(' ');
            last.push_str(line.trim_start());
        } else {
            open = false;
        }
    }
    out
}

/// `NNNN` numbers of every `](NNNN-slug.md)` link in `text`.
fn linked_numbers(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        let target = rest.split(')').next().unwrap_or_default();
        if target.contains("://") {
            continue;
        }
        if let Some(base) = target.rsplit('/').next()
            && let Some(number) = parse_number(base)
        {
            out.push(number);
        }
    }
    out
}

/// Whether `bullet` is a back-link naming record `number`.
fn is_back_link(bullet: &str, number: &str) -> bool {
    let lower = bullet.to_ascii_lowercase();
    ["amended", "extended", "superseded"]
        .iter()
        .any(|verb| lower.contains(verb))
        && (linked_numbers(bullet).contains(&number) || bullet.contains(&format!("ADR-{number}")))
}

struct Record {
    file: String,
    number: String,
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    date: Option<String>,
    superseded_by: Option<String>,
    source: String,
}

/// Split `---\nkey: value\n---` frontmatter into its raw key/value lines.
fn frontmatter(source: &str) -> Vec<(&str, &str)> {
    let mut lines = source.lines();
    if lines.next() != Some("---") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            out.push((key.trim(), value.trim()));
        }
    }
    out
}

fn field(fm: &[(&str, &str)], key: &str) -> Option<String> {
    fm.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.trim_matches('"').to_owned())
        .filter(|v| !v.is_empty() && v != "null")
}

/// `NNNN` number from `NNNN-slug.md`.
fn parse_number(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".md")?;
    let (number, _) = stem.split_once('-')?;
    if number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit()) {
        Some(number)
    } else {
        None
    }
}

fn load_records(dir: &Path) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(number) = parse_number(name) else {
            continue;
        };
        let source = fs::read_to_string(&path)?;
        let fm = frontmatter(&source);
        records.push(Record {
            file: name.to_owned(),
            number: number.to_owned(),
            id: field(&fm, "id"),
            title: field(&fm, "title"),
            status: field(&fm, "status"),
            date: field(&fm, "date"),
            superseded_by: field(&fm, "superseded_by"),
            source,
        });
    }
    Ok(records)
}

pub fn run() -> Result<()> {
    let dir = workspace::root().join("docs/adr");
    let records = load_records(&dir)?;
    if records.is_empty() {
        bail!("no ADR records found in {}", dir.display());
    }

    let mut violations = Vec::new();
    let mut by_number: BTreeMap<&str, &Record> = BTreeMap::new();
    let mut files: BTreeSet<&str> = BTreeSet::new();

    for record in &records {
        files.insert(record.file.as_str());
        if let Some(previous) = by_number.insert(&record.number, record) {
            violations.push(format!(
                "duplicate number {}: {} and {}",
                record.number, previous.file, record.file
            ));
        }
        let Some(id) = &record.id else {
            violations.push(format!("{}: missing frontmatter `id`", record.file));
            continue;
        };
        let expected = format!("ADR-{}", record.number);
        if *id != expected {
            violations.push(format!(
                "{}: frontmatter id `{id}` does not match filename number ({expected})",
                record.file
            ));
        }
        for (key, value) in [
            ("title", &record.title),
            ("status", &record.status),
            ("date", &record.date),
        ] {
            if value.is_none() {
                violations.push(format!("{}: missing frontmatter `{key}`", record.file));
            }
        }
        match record.status.as_deref() {
            Some(status) if !STATUSES.contains(&status) => violations.push(format!(
                "{}: status `{status}` is not one of {STATUSES:?}",
                record.file
            )),
            Some("superseded") if record.superseded_by.is_none() => violations.push(format!(
                "{}: status superseded requires `superseded_by`",
                record.file
            )),
            Some(status) if status != "superseded" && record.superseded_by.is_some() => {
                violations.push(format!(
                    "{}: `superseded_by` is set but status is `{status}`",
                    record.file
                ));
            }
            _ => {}
        }
        if let Some(word) = record.status.as_deref().and_then(status_word) {
            let lines: Vec<String> = bullets(header(&record.source))
                .into_iter()
                .filter(|b| b.starts_with("- Status:"))
                .collect();
            match lines.as_slice() {
                [line] => {
                    let body = line["- Status:".len()..].trim_start();
                    if leading_word(body) != word {
                        violations.push(format!(
                            "{}: body status `{}` does not match frontmatter `{}` (expected `{word}`)",
                            record.file,
                            leading_word(body),
                            record.status.as_deref().unwrap_or_default()
                        ));
                    }
                    if let Some(target) = &record.superseded_by
                        && !line.contains(&format!("]({}-", target.trim_start_matches("ADR-")))
                    {
                        violations.push(format!(
                            "{}: body status line does not link its successor {target}",
                            record.file
                        ));
                    }
                }
                [] => violations.push(format!("{}: missing body `- Status:` line", record.file)),
                _ => violations.push(format!(
                    "{}: {} body `- Status:` lines; exactly one is allowed",
                    record.file,
                    lines.len()
                )),
            }
        }
    }

    // Every superseded_by must name a real record id.
    let ids: BTreeSet<&str> = records.iter().filter_map(|r| r.id.as_deref()).collect();
    for record in &records {
        if let Some(target) = &record.superseded_by
            && !ids.contains(target.as_str())
        {
            violations.push(format!(
                "{}: superseded_by `{target}` does not resolve to a record",
                record.file
            ));
        }
    }

    // Index integrity.
    let index = fs::read_to_string(dir.join("index.md")).context("read index.md")?;
    let mut indexed_files: BTreeSet<&str> = BTreeSet::new();
    let mut index_status: BTreeMap<&str, &str> = BTreeMap::new();
    let mut previous: Option<u32> = None;
    for (row, line) in index.lines().enumerate() {
        let Some(rest) = line.strip_prefix("| [") else {
            continue;
        };
        let Some((number, tail)) = rest.split_once("](") else {
            continue;
        };
        if number.len() != 4 || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let target = tail.split(')').next().unwrap_or_default();
        let row_line = row + 1;
        if let Some((_, cell)) = line.trim_end().trim_end_matches('|').rsplit_once(" | ") {
            index_status.insert(target, cell.trim());
        }
        if !dir.join(target).exists() {
            violations.push(format!("index.md:{row_line}: `{target}` does not exist"));
        }
        if !indexed_files.insert(target) {
            violations.push(format!(
                "index.md:{row_line}: `{target}` is listed more than once"
            ));
        }
        let value: u32 = number.parse().expect("4 digits");
        if let Some(previous) = previous
            && value <= previous
        {
            violations.push(format!(
                "index.md:{row_line}: row {number} is not after {previous:04}"
            ));
        }
        previous = Some(value);
    }

    for record in &records {
        if !indexed_files.contains(record.file.as_str()) {
            violations.push(format!("{}: not registered in index.md", record.file));
        }
        let (Some(cell), Some(word)) = (
            index_status.get(record.file.as_str()),
            record.status.as_deref().and_then(status_word),
        ) else {
            continue;
        };
        if leading_word(cell) != word {
            violations.push(format!(
                "index.md: {} status `{cell}` does not open with `{word}`",
                record.file
            ));
        }
        if let Some(target) = &record.superseded_by {
            let number = target.trim_start_matches("ADR-");
            if !cell.starts_with(&format!("Superseded by [{number}]({number}-")) {
                violations.push(format!(
                    "index.md: {} status cell does not link its successor {target}",
                    record.file
                ));
            }
        }
    }

    // Reciprocal lineage: every Amends/Extends/Supersedes header link is
    // answered by a back-link in the target's header.
    let headers: BTreeMap<&str, Vec<String>> = records
        .iter()
        .map(|r| (r.number.as_str(), bullets(header(&r.source))))
        .collect();
    for record in &records {
        for bullet in &headers[record.number.as_str()] {
            if !["- Amends:", "- Extends:", "- Supersedes:"]
                .iter()
                .any(|p| bullet.starts_with(p))
            {
                continue;
            }
            for target in linked_numbers(bullet) {
                if target == record.number {
                    continue;
                }
                let Some(target_bullets) = headers.get(target) else {
                    continue; // unresolved links are reported below
                };
                if !target_bullets
                    .iter()
                    .any(|b| is_back_link(b, &record.number))
                {
                    violations.push(format!(
                        "{}: lineage link to ADR-{target} has no back-link in that record's header",
                        record.file
                    ));
                }
            }
        }
    }

    // Lineage links resolve to an on-disk record.
    let adr_files: BTreeSet<String> = records.iter().map(|r| r.file.clone()).collect();
    for record in &records {
        let source = fs::read_to_string(dir.join(&record.file))?;
        for link in adr_links(&source) {
            if !adr_files.contains(&link) {
                violations.push(format!(
                    "{}: link `{link}` does not resolve to a record",
                    record.file
                ));
            }
        }
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("ADR registry violation: {violation}");
        }
        bail!("{} ADR registry violation(s)", violations.len());
    }

    println!(
        "ADR registry: OK ({} records, statuses agree, index in sync, ids and reciprocal lineage resolve)",
        records.len()
    );
    Ok(())
}

/// `NNNN-slug.md` link targets findable anywhere on a line (any in-repo path
/// prefix). External URLs — Optics and portal ADRs are cited by full URL per
/// the agent directives — are not in this registry and are skipped.
fn adr_links(source: &str) -> BTreeSet<String> {
    let mut links = BTreeSet::new();
    let mut rest = source;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find(')') else {
            break;
        };
        let target = &rest[..close];
        if target.contains("://") {
            continue;
        }
        if let Some(base) = target.rsplit('/').next()
            && parse_number(base).is_some()
        {
            links.insert(base.to_owned());
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_fields() {
        let fm = frontmatter("---\nid: ADR-0001\ntitle: \"X\"\nstatus: accepted\n---\nbody");
        assert_eq!(field(&fm, "id").as_deref(), Some("ADR-0001"));
        assert_eq!(field(&fm, "title").as_deref(), Some("X"));
        assert_eq!(field(&fm, "superseded_by"), None);
    }

    #[test]
    fn extracts_numbered_links_only() {
        let links = adr_links("see [ADR-0006](0006-ffi.md) and [x](y.md)");
        assert!(links.contains("0006-ffi.md"));
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn status_words_cover_exactly_the_lifecycle() {
        for status in STATUSES {
            assert!(status_word(status).is_some(), "{status}");
        }
        assert_eq!(status_word("proposed"), None);
        assert_eq!(status_word("adopted"), None);
        assert_eq!(
            leading_word("Superseded by [ADR-0001](0001-x.md)"),
            "Superseded"
        );
        assert_eq!(leading_word("Deprecated — gone"), "Deprecated");
    }

    #[test]
    fn header_bullets_fold_continuations_and_stop_at_sections() {
        let source = "# T\n\n- Status: Accepted\n- Amends: [ADR-0002](0002-a.md) (the\n  rest)\n\n## Context\n- Amends: [x](0003-b.md)\n";
        let found = bullets(header(source));
        assert_eq!(found.len(), 2);
        assert_eq!(found[1], "- Amends: [ADR-0002](0002-a.md) (the rest)");
        assert_eq!(linked_numbers(&found[1]), vec!["0002"]);
    }

    #[test]
    fn back_links_need_a_verb_and_the_source_number() {
        assert!(is_back_link(
            "- Amended by: [ADR-0213](0213-x.md) — y",
            "0213"
        ));
        assert!(is_back_link(
            "- Superseded in part by: [ADR-0144](0144-x.md)",
            "0144"
        ));
        assert!(is_back_link(
            "- Status: Accepted (amended by [ADR-0172](0172-x.md))",
            "0172"
        ));
        assert!(!is_back_link("- Amends: [ADR-0213](0213-x.md)", "0213"));
        assert!(!is_back_link("- Amended by: [ADR-0214](0214-x.md)", "0213"));
    }

    #[test]
    fn parse_number_requires_four_digits() {
        assert!(parse_number("0008-x.md").is_some());
        assert!(parse_number("index.md").is_none());
        assert!(parse_number("08-x.md").is_none());
    }
}
