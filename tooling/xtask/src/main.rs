mod tasks;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "cargo xtask")]
#[command(about = "Tessera workspace engineering and build automation tool")]
struct Args {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Runs cargo clippy across workspace with standard flags and --deny warnings
    Clippy(tasks::clippy::ClippyArgs),
    /// Checks that all workspace member packages conform to workspace dependency and lint standards
    PackageConformity(tasks::package_conformity::PackageConformityArgs),
    /// Checks architectural layer and dependency boundaries between crates
    CheckBoundaries(tasks::check_boundaries::CheckBoundariesArgs),
    /// Checks that every tessera-env item serves at least two process entry points (ADR-0147)
    CheckBootstrapAdmission(tasks::check_bootstrap_admission::CheckBootstrapAdmissionArgs),
    /// Enforces ADR registry invariants: unique ids, frontmatter, index sync, lineage (ADR standards §5)
    CheckAdrs(tasks::check_adrs::CheckAdrsArgs),
    /// Enforces metadata-derived vtable admission in tessera-wayland (ADR-0193, INV-ARCH-50)
    CheckVtables(tasks::check_vtables::CheckVtablesArgs),
    /// Enforces the panic discipline: no poisonable-lock panics, no new panic sites (ADR-0195)
    CheckPanics(tasks::check_panics::CheckPanicsArgs),
    /// Enforces the chrome zero-math invariant: no hand-rolled motion math outside the transit seam (ADR-0174, INV-ARCH-49)
    CheckMotion(tasks::check_motion::CheckMotionArgs),
    /// Inspects and validates Optics release tag resolution
    Optics(tasks::optics::OpticsArgs),
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        CliCommand::Clippy(args) => tasks::clippy::run_clippy(args),
        CliCommand::PackageConformity(args) => {
            tasks::package_conformity::run_package_conformity(args)
        }
        CliCommand::CheckBoundaries(args) => tasks::check_boundaries::run_check_boundaries(args),
        CliCommand::CheckBootstrapAdmission(args) => {
            tasks::check_bootstrap_admission::run_check_bootstrap_admission(args)
        }
        CliCommand::CheckAdrs(args) => tasks::check_adrs::run_check_adrs(args),
        CliCommand::CheckVtables(args) => tasks::check_vtables::run_check_vtables(args),
        CliCommand::CheckPanics(args) => tasks::check_panics::run_check_panics(args),
        CliCommand::CheckMotion(args) => tasks::check_motion::run_check_motion(args),
        CliCommand::Optics(args) => tasks::optics::run_optics(args),
    }
}
