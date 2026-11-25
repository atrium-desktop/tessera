//! `tessera-settings [PAGE]` — open Tessera Settings, optionally on one page.

mod app;

use std::path::PathBuf;

const USAGE: &str = "\
Usage: tessera-settings [OPTIONS] [PAGE]

Open Tessera Settings. When a Settings window is already open, it switches
to PAGE instead of opening a second window.

Pages: display, input, power, appearance, dock, accessibility, users,
       window-rules

Options:
  --socket PATH   Compositor IPC socket (default: $TESSERA_IPC_SOCKET or $XDG_RUNTIME_DIR/tessera.sock)
  -h, --help      Show this help
  -V, --version   Show the version";

struct Args {
    target: Option<String>,
    socket: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        target: None,
        socket: None,
    };
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("tessera-settings {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--socket" => {
                let path = args.next().ok_or("--socket requires a path")?;
                parsed.socket = Some(PathBuf::from(path));
            }
            value if value.starts_with("--socket=") => {
                parsed.socket = Some(PathBuf::from(&value["--socket=".len()..]));
            }
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            value => {
                if parsed.target.replace(value.to_owned()).is_some() {
                    return Err("only one page may be named".into());
                }
            }
        }
    }
    Ok(Some(parsed))
}

fn main() {
    tessera_env::init("info");
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return,
        Err(error) => {
            eprintln!("tessera-settings: {error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(args) {
        log::error!("settings: {error}");
        eprintln!("tessera-settings: {error}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_dir = tessera_env::runtime_dir()?;
    let socket = match args.socket {
        Some(path) => path,
        None => match tessera_env::ipc_socket_override()? {
            Some(path) => path,
            None => tessera_ipc::socket_paths::default_socket_path(&runtime_dir),
        },
    };
    // Single-instance hand-off is a convenience: if the socket cannot be
    // claimed, the window still opens, it just cannot receive deep links.
    let instance = match app::instance::claim(
        &app::instance::socket_path(&runtime_dir),
        args.target.as_deref(),
    ) {
        Ok(app::instance::Claim::Forwarded) => return Ok(()),
        Ok(app::instance::Claim::Primary(listener)) => Some(listener),
        Err(error) => {
            log::warn!("settings: running without single-instance hand-off: {error}");
            None
        }
    };
    app::run(app::Options {
        socket,
        target: args.target,
        instance,
        app_id: tessera_settings::APP_ID.to_owned(),
        initial_size: None,
        ready_fd: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        parse_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn a_bare_argument_is_the_page() {
        let args = parse(&["display"]).unwrap().unwrap();
        assert_eq!(args.target.as_deref(), Some("display"));
        assert!(args.socket.is_none());
    }

    #[test]
    fn the_socket_accepts_both_spellings() {
        let split = parse(&["--socket", "/tmp/a.sock"]).unwrap().unwrap();
        let joined = parse(&["--socket=/tmp/a.sock"]).unwrap().unwrap();
        assert_eq!(split.socket, joined.socket);
    }

    #[test]
    fn two_pages_and_unknown_options_are_errors() {
        assert!(parse(&["display", "power"]).is_err());
        assert!(parse(&["--frobnicate"]).is_err());
        assert!(parse(&["--socket"]).is_err());
    }
}
