//! Development-only inspection harness for Tessera Settings (ADR-0257).
//!
//! Starts an in-process wire-level [`tessera_ipc::Server`] serving deterministic
//! snapshots and fault modes, then runs the production Settings window against
//! it (or serves the socket alone under `--mock-server-only`).

pub mod mock_server;

use std::path::PathBuf;
use std::time::Duration;

use mock_server::{FaultMode, MockSettingsHandler, Preset};

use crate::app;

pub const PREVIEW_APP_ID: &str = "dev.tessera.SettingsPreview";

const USAGE: &str = "\
Usage: tessera-settings-preview [OPTIONS] [PAGE]

Development-only inspection harness for Tessera Settings (ADR-0257).
Starts a wire-level mock compositor IPC server so Settings can be inspected,
interacted with, and pixel-captured without launching a compositor.

Pages: display, input, power, appearance, dock, accessibility, users,
       window-rules

Options:
  --preset NAME        Snapshot preset: desktop (default), multi-display,
                       laptop, minimal
  --mode MODE          Link / authority mode: live (default), read-only,
                       reject-stale, reject-locked, disconnected
  --size WIDTHxHEIGHT  Initial logical window size (e.g. 540x720 for compact
                       rail, 960x680 standard, 1440x900 wide)
  --ready-fd FD        Write one byte to FD after the first presented frame
  --mock-server-only   Run only the mock IPC server (pair with --socket to
                       test unmodified production tessera-settings or CLI)
  --socket PATH        Explicit mock IPC socket path (required for
                       --mock-server-only; defaults to an ephemeral temp path)
  -h, --help           Show this help
  -V, --version        Show the version";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    target: Option<String>,
    preset: Preset,
    mode: FaultMode,
    size: Option<(u32, u32)>,
    ready_fd: Option<i32>,
    mock_server_only: bool,
    socket: Option<PathBuf>,
}

fn parse_size(raw: &str) -> Result<(u32, u32), String> {
    let (w, h) = raw
        .split_once('x')
        .or_else(|| raw.split_once('X'))
        .ok_or_else(|| format!("invalid --size {raw:?}: expected WIDTHxHEIGHT"))?;
    let width: u32 = w
        .parse()
        .map_err(|_| format!("invalid --size width {w:?}"))?;
    let height: u32 = h
        .parse()
        .map_err(|_| format!("invalid --size height {h:?}"))?;
    if width == 0 || height == 0 {
        return Err("--size dimensions must be greater than zero".into());
    }
    Ok((width, height))
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        target: None,
        preset: Preset::Desktop,
        mode: FaultMode::Live,
        size: None,
        ready_fd: None,
        mock_server_only: false,
        socket: None,
    };
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("tessera-settings-preview {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--preset" => {
                let value = args.next().ok_or("--preset requires a value")?;
                parsed.preset = Preset::parse(&value)?;
            }
            value if value.starts_with("--preset=") => {
                parsed.preset = Preset::parse(&value["--preset=".len()..])?;
            }
            "--mode" => {
                let value = args.next().ok_or("--mode requires a value")?;
                parsed.mode = FaultMode::parse(&value)?;
            }
            value if value.starts_with("--mode=") => {
                parsed.mode = FaultMode::parse(&value["--mode=".len()..])?;
            }
            "--size" => {
                let value = args.next().ok_or("--size requires WIDTHxHEIGHT")?;
                parsed.size = Some(parse_size(&value)?);
            }
            value if value.starts_with("--size=") => {
                parsed.size = Some(parse_size(&value["--size=".len()..])?);
            }
            "--ready-fd" => {
                let value = args.next().ok_or("--ready-fd requires a file descriptor")?;
                let fd: i32 = value
                    .parse()
                    .map_err(|_| format!("invalid --ready-fd {value:?}"))?;
                if fd < 0 {
                    return Err("--ready-fd must be non-negative".into());
                }
                parsed.ready_fd = Some(fd);
            }
            value if value.starts_with("--ready-fd=") => {
                let raw = &value["--ready-fd=".len()..];
                let fd: i32 = raw
                    .parse()
                    .map_err(|_| format!("invalid --ready-fd {raw:?}"))?;
                if fd < 0 {
                    return Err("--ready-fd must be non-negative".into());
                }
                parsed.ready_fd = Some(fd);
            }
            "--mock-server-only" => {
                parsed.mock_server_only = true;
            }
            "--socket" => {
                let value = args.next().ok_or("--socket requires a path")?;
                parsed.socket = Some(PathBuf::from(value));
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
    if parsed.mock_server_only && parsed.socket.is_none() {
        return Err("--mock-server-only requires --socket PATH".into());
    }
    if parsed.mock_server_only && parsed.mode == FaultMode::Disconnected {
        return Err("--mock-server-only cannot be combined with --mode disconnected".into());
    }
    Ok(Some(parsed))
}

struct EphemeralSocketDir(PathBuf);

impl EphemeralSocketDir {
    fn create() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "tessera-settings-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn socket_path(&self) -> PathBuf {
        self.0.join("tessera.sock")
    }
}

impl Drop for EphemeralSocketDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static SHUTDOWN_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn handle_shutdown_signal(_sig: libc::c_int) {
    SHUTDOWN_FLAG.store(true, std::sync::atomic::Ordering::Release);
}

fn install_shutdown_flag() -> std::io::Result<&'static std::sync::atomic::AtomicBool> {
    SHUTDOWN_FLAG.store(false, std::sync::atomic::Ordering::Release);
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = handle_shutdown_signal as *const () as usize;
    sa.sa_flags = libc::SA_RESTART;
    // SAFETY: `sa` is zero-initialized and `handle_shutdown_signal` only stores
    // into a static `AtomicBool`, which is async-signal-safe.
    unsafe {
        libc::sigemptyset(&mut sa.sa_mask);
        for sig in [libc::SIGINT, libc::SIGTERM] {
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(&SHUTDOWN_FLAG)
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return Ok(()),
        Err(error) => {
            eprintln!("tessera-settings-preview: {error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let _ephemeral_dir: Option<EphemeralSocketDir>;
    let socket = match args.socket {
        Some(path) => {
            _ephemeral_dir = None;
            path
        }
        None => {
            let dir = EphemeralSocketDir::create()?;
            let path = dir.socket_path();
            _ephemeral_dir = Some(dir);
            path
        }
    };

    let _server = if args.mode == FaultMode::Disconnected {
        log::info!(
            "settings preview: disconnected mode (no IPC server listening at {})",
            socket.display()
        );
        None
    } else {
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (handler, server) = MockSettingsHandler::start(&socket, args.preset, args.mode)?;
        log::info!(
            "settings preview: mock IPC listening on {} (preset={:?}, mode={:?})",
            socket.display(),
            args.preset,
            args.mode
        );
        Some((handler, server))
    };

    if args.mock_server_only {
        let stop = install_shutdown_flag()?;
        eprintln!(
            "tessera-settings-preview: mock IPC server listening on {}\n  \
             Run: tessera-settings --socket {}",
            socket.display(),
            socket.display()
        );
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        return Ok(());
    }

    app::run(app::Options {
        socket,
        target: args.target,
        instance: None,
        app_id: PREVIEW_APP_ID.to_owned(),
        initial_size: args.size,
        ready_fd: args.ready_fd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        parse_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn parses_presets_modes_sizes_and_page() {
        let args = parse(&[
            "--preset",
            "multi-display",
            "--mode=read-only",
            "--size",
            "540x720",
            "--ready-fd",
            "5",
            "display",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(args.preset, Preset::MultiDisplay);
        assert_eq!(args.mode, FaultMode::ReadOnly);
        assert_eq!(args.size, Some((540, 720)));
        assert_eq!(args.ready_fd, Some(5));
        assert_eq!(args.target.as_deref(), Some("display"));
    }

    #[test]
    fn rejects_invalid_combinations() {
        assert!(parse(&["--mock-server-only"]).is_err());
        assert!(
            parse(&[
                "--mock-server-only",
                "--socket",
                "/tmp/a.sock",
                "--mode",
                "disconnected"
            ])
            .is_err()
        );
        assert!(parse(&["--size", "0x500"]).is_err());
        assert!(parse(&["--size", "wide"]).is_err());
        assert!(parse(&["--preset", "unknown"]).is_err());
        assert!(parse(&["--mode", "bogus"]).is_err());
    }
}
