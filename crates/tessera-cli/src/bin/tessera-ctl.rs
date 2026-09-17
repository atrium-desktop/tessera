//! Headless entry point for the shared Tessera command model.
use clap::{CommandFactory, FromArgMatches};
use std::process::ExitCode;

fn main() -> ExitCode {
    let command = tessera_cli::Cli::command()
        .name("tessera-ctl")
        .about("Inspect and control a Tessera session")
        .subcommand_required(true)
        .mut_subcommand("run", |command| command.hide(true));
    let cli = match command
        .try_get_matches()
        .and_then(|matches| tessera_cli::Cli::from_arg_matches(&matches))
    {
        Ok(cli) => cli,
        Err(error) => {
            let failure = error.use_stderr();
            let _ = error.print();
            return ExitCode::from(if failure { 2 } else { 0 });
        }
    };
    if cli.runs_compositor() {
        eprintln!("Use tessera to start the desktop session.");
        return ExitCode::from(2);
    }
    match tessera_cli::execute(cli) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tessera-ctl: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}
