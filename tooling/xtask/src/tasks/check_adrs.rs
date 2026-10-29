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

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Parser, Debug)]
pub struct CheckAdrsArgs {}

const STATUSES: &[&str] = &["draft", "accepted", "superseded", "rejected", "deprecated"];

struct Record {
    file: String,
    number: String,
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    date: Option<String>,
    superseded_by: Option<String>,
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
        });
    }
    Ok(records)
}

pub fn run_check_adrs(_args: CheckAdrsArgs) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("docs/adr");
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
            _ => {}
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
        "ADR registry: OK ({} records, index in sync, ids and lineage resolve)",
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
    fn parse_number_requires_four_digits() {
        assert!(parse_number("0008-x.md").is_some());
        assert!(parse_number("index.md").is_none());
        assert!(parse_number("08-x.md").is_none());
    }
}
