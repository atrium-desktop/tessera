//! tessera — autonomous surface shell.
//!
//! With no subcommand, the process composition root selects a presentation
//! host, creates the Wayland server, renderer, shell, wallpaper,
//! configuration, and IPC surfaces, then runs the compositor loop. Resource
//! subcommands dispatch to a running session without entering that runtime.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Parse before logging or backend initialization so help, version, and
    // client commands never touch compositor runtime state.
    let cli = match tessera_cli::parse_env() {
        Ok(cli) => cli,
        Err(error) => {
            let use_stderr = error.use_stderr();
            error.print().expect("print clap message");
            return ExitCode::from(if use_stderr { 2 } else { 0 });
        }
    };

    if cli.runs_compositor() {
        if cli.json {
            eprintln!("error: --json requires a session-management command");
            return ExitCode::from(2);
        }
        return run_compositor();
    }

    run_session_command(cli)
}

fn run_compositor() -> ExitCode {
    // `RUST_LOG` controls verbosity; compositor bring-up is visible by default.
    tessera_bootstrap::init("info");
    match tessera::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("tessera: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_session_command(cli: tessera_cli::Cli) -> ExitCode {
    // One-shot commands print results, not a log stream.
    tessera_bootstrap::init("warn");
    match tessera_cli::execute(cli) {
        Ok(output) if !output.is_empty() => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}
