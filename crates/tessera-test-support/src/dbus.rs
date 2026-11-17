//! A private D-Bus daemon for tests that stand in for a system service.
//!
//! Tests that fake a system daemon (NetworkManager, iwd, …) must never touch
//! the host's real system bus: a test that talks to the real NetworkManager
//! can change the machine's networking. [`PrivateBus`] starts a throwaway
//! `dbus-daemon` with the session configuration and hands out its address,
//! which a client reaches through `DBUS_SYSTEM_BUS_ADDRESS`.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// A `dbus-daemon` owned by one test, killed when dropped.
pub struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    /// Start a private bus, or `None` when `dbus-daemon` is not installed so
    /// the caller can skip with a message instead of failing.
    pub fn start() -> Option<Self> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut line = String::new();
        if BufReader::new(stdout).read_line(&mut line).ok()? == 0 {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Some(Self {
            child,
            address: line.trim().to_owned(),
        })
    }

    /// The bus address, in `DBUS_*_BUS_ADDRESS` form.
    pub fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
