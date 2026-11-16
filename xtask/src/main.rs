//! Repository automation for the Tessera workspace: architecture gates and
//! release-pin management. Run as `cargo run -p xtask -- <command>` (or the
//! `cargo xtask` alias from `.cargo/config.example.toml`) from anywhere in the
//! checkout; every path is resolved from the workspace root.
//!
//! Tests are not run from here; use `cargo nextest run`.

mod tasks;
mod workspace;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "cargo xtask", about = "Tessera workspace architecture gates")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Every member inherits workspace lints and workspace dependency declarations
    PackageConformity(tasks::package_conformity::Args),
    /// Every dependency edge is allowed by dependency-policy.toml (ADR-0159)
    CheckBoundaries,
    /// Every tessera-env item serves at least two process entry points (ADR-0147)
    CheckBootstrapAdmission,
    /// Every Wayland vtable is admitted and no resource bypasses admission (ADR-0193)
    CheckVtables,
    /// ADR registry: unique ids, frontmatter, index sync, lineage links
    CheckAdrs,
    /// system/ mirrors the install tree, the manifest, and the docs (ADR-0241)
    CheckSystemFiles,
    /// No poisonable-lock panics and no new panic sites (ADR-0195)
    CheckPanics(tasks::check_panics::Args),
    /// Read or move the pinned Optics release tag
    Optics(tasks::optics::Args),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::PackageConformity(args) => tasks::package_conformity::run(args),
        Command::CheckBoundaries => tasks::check_boundaries::run(),
        Command::CheckBootstrapAdmission => tasks::check_bootstrap_admission::run(),
        Command::CheckVtables => tasks::check_vtables::run(),
        Command::CheckAdrs => tasks::check_adrs::run(),
        Command::CheckSystemFiles => tasks::check_system_files::run(),
        Command::CheckPanics(args) => tasks::check_panics::run(args),
        Command::Optics(args) => tasks::optics::run(args),
    }
}
