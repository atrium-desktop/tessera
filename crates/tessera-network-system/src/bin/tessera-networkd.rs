use std::process::ExitCode;
use std::str::FromStr;

use tessera_network_system::DaemonBackend;
use tessera_network_system::daemon::{self, DaemonOptions};

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    {
        // The compositor supervises this process. If the compositor dies
        // without sending Shutdown, the network service must not linger with
        // the system bus and a live socket.
        // SAFETY: `prctl(PR_SET_PDEATHSIG)` only configures a signal on parent
        // death and has no memory-safety preconditions.
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
        }
        // SAFETY: `getppid` has no preconditions and cannot fail.
        if unsafe { libc::getppid() } == 1 {
            return ExitCode::SUCCESS;
        }
    }

    tessera_env::init("info");
    let mut options = DaemonOptions::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        if matches!(args[index].as_str(), "help" | "--help" | "-h") {
            println!(
                "tessera-networkd: the out-of-process network service (ADR-0242)\n\n\
                 USAGE:\n    \
                 tessera-networkd [--socket PATH] [--backend auto|networkmanager|iwd|mock]\n\n\
                 The service listens on a per-user Unix socket (default\n\
                 $XDG_RUNTIME_DIR/tessera-networkd.sock), owns the system D-Bus\n\
                 connection, and publishes a backend-neutral network snapshot.\n\
                 `auto` prefers NetworkManager and falls back to iwd only when\n\
                 NetworkManager is not running. Neither daemon is D-Bus-activated;\n\
                 a stopped service is reported as unavailable. `mock` is for\n\
                 preview and tests."
            );
            return ExitCode::SUCCESS;
        }
        let Some(value) = args.get(index + 1) else {
            eprintln!("tessera-networkd: {} needs a value", args[index]);
            return ExitCode::from(1);
        };
        match args[index].as_str() {
            "--socket" => options.socket = value.into(),
            "--backend" => match DaemonBackend::from_str(value) {
                Ok(backend) => options.backend = backend,
                Err(error) => {
                    eprintln!("tessera-networkd: {error}");
                    return ExitCode::from(1);
                }
            },
            other => {
                eprintln!("tessera-networkd: unknown option '{other}'");
                return ExitCode::from(1);
            }
        }
        index += 2;
    }

    match daemon::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera-networkd: {error}");
            ExitCode::from(1)
        }
    }
}
