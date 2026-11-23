//! Repository automation for the Tessera workspace: architecture gates and
//! release-pin management. Run as `cargo run -p xtask -- <command>` (or the
//! `cargo xtask` alias from `.cargo/config.example.toml`) from anywhere in the
//! checkout; every path is resolved from the workspace root.
//!
//! Tests are not run from here; use `cargo nextest run`.

mod error;
mod tasks;
mod workspace;

use crate::error::{Result, bail};

fn print_usage() {
    println!(
        "Tessera workspace architecture gates\n\n\
         Usage: cargo xtask <COMMAND>\n\n\
         Commands:\n  \
           package-conformity [--fix]\n  \
           check-boundaries\n  \
           check-bootstrap-admission\n  \
           check-vtables\n  \
           check-adrs\n  \
           check-docs\n  \
           check-install-manifest\n  \
           check-system-files\n  \
           check-panics [--update]\n  \
           optics [--tag-only | --set <TAG>]"
    );
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        print_usage();
        return Ok(());
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        "package-conformity" => {
            let mut parsed = tasks::package_conformity::Args::default();
            for arg in args {
                match arg.as_str() {
                    "--fix" => parsed.fix = true,
                    other => bail!("unknown argument for package-conformity: {other}"),
                }
            }
            tasks::package_conformity::run(parsed)
        }
        "check-boundaries" => tasks::check_boundaries::run(),
        "check-bootstrap-admission" => tasks::check_bootstrap_admission::run(),
        "check-vtables" => tasks::check_vtables::run(),
        "check-adrs" => tasks::check_adrs::run(),
        "check-docs" => tasks::check_docs::run(),
        "check-install-manifest" => tasks::check_install_manifest::run(),
        "check-system-files" => tasks::check_system_files::run(),
        "check-panics" => {
            let mut parsed = tasks::check_panics::Args::default();
            for arg in args {
                match arg.as_str() {
                    "--update" => parsed.update = true,
                    other => bail!("unknown argument for check-panics: {other}"),
                }
            }
            tasks::check_panics::run(parsed)
        }
        "optics" => {
            let mut parsed = tasks::optics::Args::default();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-t" | "--tag-only" => parsed.tag_only = true,
                    "--set" => {
                        let tag = args.next().ok_or("--set requires TAG")?;
                        parsed.set = Some(tag);
                    }
                    s if s.starts_with("--set=") => {
                        parsed.set = Some(s["--set=".len()..].to_string());
                    }
                    other => bail!("unknown argument for optics: {other}"),
                }
            }
            if parsed.tag_only && parsed.set.is_some() {
                bail!("--tag-only conflicts with --set");
            }
            tasks::optics::run(parsed)
        }
        other => bail!("unknown xtask command: {other}"),
    }
}
