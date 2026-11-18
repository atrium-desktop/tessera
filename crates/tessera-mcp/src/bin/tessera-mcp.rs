use std::process::ExitCode;
use std::time::Duration;

use tessera_mcp::{BridgeConfig, TesseraPlatform};

enum Command {
    Serve,
    Check,
    Smoke {
        observe_seconds: u64,
        input_window: Option<u64>,
    },
    PrintConfig,
}

fn parse_cli() -> Result<Command, String> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        return Ok(Command::Serve);
    };
    match cmd.as_str() {
        "-h" | "--help" => {
            println!(
                "Scoped Tessera desktop and Agent Interaction Domain MCP bridge\n\n\
                 Usage: tessera-mcp [COMMAND]\n\n\
                 Commands:\n  \
                   serve         Serve MCP over newline-delimited JSON-RPC on stdin/stdout (default)\n  \
                   check         Probe the granted capability ceiling and print the effective tool grant\n  \
                   smoke         Run a live, reversible notification and Agent Interaction Domain smoke test\n  \
                   print-config  Print an MCP client config entry for this executable"
            );
            std::process::exit(0);
        }
        "-V" | "--version" => {
            println!("tessera-mcp {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        "serve" => Ok(Command::Serve),
        "check" => Ok(Command::Check),
        "print-config" => Ok(Command::PrintConfig),
        "smoke" => {
            let mut observe_seconds = 8u64;
            let mut input_window = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--observe-seconds" => {
                        let val = args.next().ok_or("--observe-seconds requires a value")?;
                        observe_seconds = val
                            .parse::<u64>()
                            .map_err(|e| format!("invalid --observe-seconds: {e}"))?;
                        if !(1..=30).contains(&observe_seconds) {
                            return Err("--observe-seconds must be in 1..=30".into());
                        }
                    }
                    "--input-window" => {
                        let val = args.next().ok_or("--input-window requires a value")?;
                        let id = val
                            .parse::<u64>()
                            .map_err(|e| format!("invalid --input-window: {e}"))?;
                        if id < 1 {
                            return Err("--input-window must be >= 1".into());
                        }
                        input_window = Some(id);
                    }
                    other => return Err(format!("unknown argument for smoke: {other}")),
                }
            }
            Ok(Command::Smoke {
                observe_seconds,
                input_window,
            })
        }
        other => Err(format!("unknown command: {other}")),
    }
}

fn main() -> ExitCode {
    let command = match parse_cli() {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("tessera-mcp: {err}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::PrintConfig => print_config(),
        Command::Check => with_platform(|platform| {
            let output = serde_json::json!({
                "grant": platform.grant(),
                "tools": platform.tool_names()
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output).expect("JSON value")
            );
            Ok::<(), std::convert::Infallible>(())
        }),
        Command::Smoke {
            observe_seconds,
            input_window,
        } => with_platform(|platform| {
            let report = platform.smoke_with_input(
                Duration::from_secs(observe_seconds),
                input_window.map(tessera_desktop::window::WindowId),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("serializable smoke report")
            );
            Ok::<(), tessera_mcp::PlatformError>(())
        }),
        Command::Serve => with_config(tessera_mcp::serve_config),
    }
}

fn with_config<E: std::fmt::Display>(run: impl FnOnce(BridgeConfig) -> Result<(), E>) -> ExitCode {
    let config = match BridgeConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("tessera-mcp: {error}");
            return ExitCode::from(2);
        }
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn with_platform<E: std::fmt::Display>(
    run: impl FnOnce(&mut TesseraPlatform) -> Result<(), E>,
) -> ExitCode {
    let config = match BridgeConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("tessera-mcp: {error}");
            return ExitCode::from(2);
        }
    };
    let mut platform = match TesseraPlatform::connect(config) {
        Ok(platform) => platform,
        Err(error) => {
            eprintln!("tessera-mcp: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&mut platform) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_config() -> ExitCode {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("tessera-mcp: cannot resolve executable path: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut random = [0_u8; 16];
    if let Err(error) = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut random))
    {
        eprintln!("tessera-mcp: cannot generate connector instance id: {error}");
        return ExitCode::FAILURE;
    }
    let instance_id = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let command = serde_json::to_string(&executable.to_string_lossy()).expect("string JSON");
    println!(
        "[mcp.tessera]\ncommand = [{command}]\nenabled = true\nread_only = false\n\
         environment = {{ TESSERA_MCP_INSTANCE_ID = \"{instance_id}\" }}"
    );
    ExitCode::SUCCESS
}
