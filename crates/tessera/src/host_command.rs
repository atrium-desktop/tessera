//! The compositor's only seam onto short-lived host CLI tools.
//!
//! Desktop services (PipeWire, WirePlumber, `brightnessctl`, `systemctl`,
//! `rfkill`) are reachable through their command-line tools, and the compositor
//! calls them in two distinct ways:
//!
//! - **Probe** ([`command_output`]): run, capture stdout, wait. Fork+exec on a
//!   background helper thread only — never on the frame path.
//! - **Mutate** ([`spawn_host_command`]): start, never wait. The compositor must
//!   not block a frame on a D-Bus round trip, so a mutation is fire-and-forget
//!   and its effect is confirmed by the next probe, not by an exit status.
//!
//! Both live here, in one module, so every subsystem that shells out shares one
//! reaper and one environment discipline (`LC_ALL=C`, closed stdin, discarded
//! stderr) instead of each hand-rolling `Command` plumbing.
//!
//! **Negative knowledge.** Do not call these from the event loop to make a
//! decision in the same frame: the probe forms block by construction, and a
//! stalled child (a wedged sound server) would then stall presentation. Probes
//! belong to the status poller thread; mutations belong to a fire-and-forget
//! spawn whose result arrives as an observed state change.

use std::process::{Child, Command, Stdio};
use std::sync::{OnceLock, mpsc};

/// Run `program` with `args` and return its trimmed stdout, or `None` when it
/// is missing, fails, or produces nothing usable.
///
/// Locale is pinned to `C` because every caller parses a value out of the
/// output, and localized decimals (`0,65`) or translated labels would silently
/// change the meaning of the parse.
pub(crate) fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Start a host control command without blocking the caller, while still
/// reaping the child so short-lived tools never accumulate as zombies.
///
/// Success means "the tool was started". It does not mean "the machine
/// changed": `pw-metadata` exits 0 even when the name it was handed resolves to
/// nothing (ADR-0215). Every caller must treat the next probe as the
/// authoritative answer.
pub(crate) fn spawn_host_command(program: &str, args: &[&str]) -> Result<(), String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start {program}: {error}"))?;
    host_command_reaper().reap(child);
    Ok(())
}

struct HostCommandReaper {
    tx: mpsc::Sender<Child>,
}

impl HostCommandReaper {
    fn reap(&self, child: Child) {
        let _ = self.tx.send(child);
    }
}

fn host_command_reaper() -> &'static HostCommandReaper {
    static REAPER: OnceLock<HostCommandReaper> = OnceLock::new();
    REAPER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Child>();
        std::thread::Builder::new()
            .name("tessera-host-cmd-reaper".into())
            .spawn(move || {
                tessera_sched::mark_background();
                while let Ok(mut child) = rx.recv() {
                    let _ = child.wait();
                }
            })
            .expect("spawn host-command reaper");
        HostCommandReaper { tx }
    })
}
