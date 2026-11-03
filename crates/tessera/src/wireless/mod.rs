//! Wireless network management subsystem (ADR-0162).
//!
//! Provides a daemon-neutral asynchronous bridge to host wireless services
//! (primarily `iwd`), with deterministic mock support for test suites.

#[cfg(feature = "dbus-system")]
pub mod iwd;
pub mod mock;

use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tessera_desktop::system::{WifiLinkState, WifiNetwork};

#[cfg(feature = "dbus-system")]
pub use iwd::IwdBackend;
pub use mock::{MockState, MockWirelessBackend};

#[cfg(test)]
mod tests;

/// Daemon-neutral backend contract for managing wireless interfaces.
pub trait WirelessBackend: Send + Sync {
    /// Request an active channel scan.
    fn scan(&self) -> Result<(), String>;
    /// Connect to a specific SSID.
    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), String>;
    /// Disconnect from the current active network.
    fn disconnect(&self) -> Result<(), String>;
    /// Enable or disable the wireless radio.
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
    /// Discard saved credentials and profile for a specified network (ADR-0167).
    fn forget(&self, ssid: &str) -> Result<(), String>;
    /// Set auto-connect behavior for a saved network (ADR-0167).
    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), String>;
    /// Refresh managed objects and state from the wireless backend daemon.
    /// The backend may coalesce or ignore this; the worker calls it only
    /// when a transition is plausibly in flight or a signal marked the
    /// daemon world dirty.
    fn refresh(&self) -> Result<(), String> {
        Ok(())
    }
    /// Poll for in-flight asynchronous transitions (e.g. connecting/scanning).
    /// Returns `true` when the poll observed a state that may have changed
    /// since the last snapshot, so the worker knows to publish again.
    /// A backend without asynchronous transitions returns `false`.
    fn poll(&self) -> bool {
        false
    }
    /// Get current link state.
    fn state(&self) -> WifiLinkState;
    /// Get currently discovered networks.
    fn networks(&self) -> Vec<WifiNetwork>;
    /// Get the SSID of the currently associated network, if any.
    fn active_ssid(&self) -> Option<String>;
    /// Whether a real wireless service backs this backend. `false` means the
    /// compositor must report wireless as **unavailable** and must never
    /// surface the backend's (empty) networks as discovered ones (ADR-0190,
    /// `[INV-ARCH-82]`). Only a selection placeholder returns `false`; every
    /// backend that talks to a daemon returns `true`.
    fn available(&self) -> bool;
}

/// The wireless backend used when no daemon answered: it fabricates nothing.
///
/// Every verb is refused and every read is empty, so the compositor's status
/// resolves wireless to unavailable (`wifi_enabled = None`, no networks) rather
/// than to a plausible-looking lie (ADR-0190).
pub struct UnavailableWirelessBackend;

impl WirelessBackend for UnavailableWirelessBackend {
    fn scan(&self) -> Result<(), String> {
        Err("no wireless service is available".into())
    }
    fn connect(&self, _ssid: &str, _passphrase: Option<&str>) -> Result<(), String> {
        Err("no wireless service is available".into())
    }
    fn disconnect(&self) -> Result<(), String> {
        Err("no wireless service is available".into())
    }
    fn set_enabled(&self, _enabled: bool) -> Result<(), String> {
        Err("no wireless service is available".into())
    }
    fn forget(&self, _ssid: &str) -> Result<(), String> {
        Err("no wireless service is available".into())
    }
    fn set_auto_connect(&self, _ssid: &str, _auto_connect: bool) -> Result<(), String> {
        Err("no wireless service is available".into())
    }
    fn state(&self) -> WifiLinkState {
        WifiLinkState::Disconnected
    }
    fn networks(&self) -> Vec<WifiNetwork> {
        Vec::new()
    }
    fn active_ssid(&self) -> Option<String> {
        None
    }
    fn available(&self) -> bool {
        false
    }
}

/// Commands routed to the wireless worker thread.
#[derive(Debug, Clone)]
pub enum WirelessCommand {
    Scan,
    Connect {
        ssid: String,
        passphrase: Option<String>,
    },
    Disconnect,
    SetEnabled(bool),
    Forget {
        ssid: String,
    },
    SetAutoConnect {
        ssid: String,
        auto_connect: bool,
    },
}

/// Snapshot of the wireless state shared with the compositor.
#[derive(Debug, Clone, Default)]
pub struct WirelessSnapshot {
    pub state: WifiLinkState,
    pub active_ssid: Option<String>,
    pub networks: Vec<WifiNetwork>,
    /// Whether a real wireless service backs the running backend (ADR-0190).
    /// `false` tells the compositor to report wireless as unavailable rather
    /// than show the backend's empty network list as "no networks found".
    pub available: bool,
}

/// Non-blocking handle to the wireless subsystem.
#[derive(Clone)]
pub struct WirelessHandle {
    cmd_tx: mpsc::Sender<WirelessCommand>,
    snapshot: Arc<Mutex<WirelessSnapshot>>,
}

impl WirelessHandle {
    /// Spawn the wireless worker with an explicit backend.
    pub fn spawn_with_backend<B: WirelessBackend + 'static>(backend: B) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WirelessCommand>();
        let snapshot = Arc::new(Mutex::new(WirelessSnapshot {
            state: backend.state(),
            active_ssid: backend.active_ssid(),
            networks: backend.networks(),
            available: backend.available(),
        }));

        let worker_snapshot = snapshot.clone();
        thread::Builder::new()
            .name("tessera-wireless".into())
            .spawn(move || {
                // Background wireless bridge: de-prioritized off the frame CPU.
                tessera_sched::mark_background();
                let backend = Box::new(backend) as Box<dyn WirelessBackend>;
                loop {
                    // Drain all pending commands non-blockingly or wait with
                    // 250 ms cadence. The cadence is the async-transition
                    // coalescing window: when no command arrives and no
                    // transition is in flight the worker does no work at all.
                    let mut changed = false;
                    match cmd_rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(cmd) => match cmd {
                            // Every verb refreshes the backend cache after its
                            // D-Bus call, so the reads below are authoritative
                            // for the command's outcome.
                            WirelessCommand::Scan => {
                                changed = true;
                                let _ = backend.scan();
                            }
                            WirelessCommand::Connect { ssid, passphrase } => {
                                changed = true;
                                let _ = backend.connect(&ssid, passphrase.as_deref());
                            }
                            WirelessCommand::Disconnect => {
                                changed = true;
                                let _ = backend.disconnect();
                            }
                            WirelessCommand::SetEnabled(enabled) => {
                                changed = true;
                                let _ = backend.set_enabled(enabled);
                            }
                            WirelessCommand::Forget { ssid } => {
                                changed = true;
                                let _ = backend.forget(&ssid);
                            }
                            WirelessCommand::SetAutoConnect { ssid, auto_connect } => {
                                changed = true;
                                let _ = backend.set_auto_connect(&ssid, auto_connect);
                            }
                        },
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }

                    // Poll for in-flight transitions. The backend decides
                    // whether its async world moved (signal-dirty flag, or a
                    // connecting/scanning transition); `true` forces a
                    // snapshot re-read on this cadence.
                    changed = backend.poll() || changed;

                    if changed {
                        // Update cached snapshot only when the world moved.
                        if let Ok(mut snap) = worker_snapshot.lock() {
                            snap.state = backend.state();
                            snap.active_ssid = backend.active_ssid();
                            snap.networks = backend.networks();
                            snap.available = backend.available();
                        }
                    }
                }
            })
            .expect("failed to spawn wireless worker thread");

        Self { cmd_tx, snapshot }
    }

    /// Select the production wireless backend (ADR-0190). Only a backend that
    /// talks to a real daemon is a candidate: the native iwd bridge when it
    /// opens, otherwise the [`UnavailableWirelessBackend`], whose empty reads
    /// resolve status to unavailable. A mock is **never** a production
    /// fallback — it is reachable only through the explicit preview entry
    /// point (`[INV-ARCH-82]`).
    pub fn spawn_auto() -> Self {
        #[cfg(feature = "dbus-system")]
        {
            match IwdBackend::open() {
                Ok(iwd) => {
                    log::info!("wireless subsystem: initialized native iwd backend");
                    return Self::spawn_with_backend(iwd);
                }
                Err(error) => {
                    log::info!(
                        "wireless subsystem: no wireless service available ({error}); \
                         reporting wireless as unavailable"
                    );
                }
            }
        }
        Self::spawn_with_backend(UnavailableWirelessBackend)
    }

    /// Dispatch an asynchronous scan request.
    pub fn request_scan(&self) {
        let _ = self.cmd_tx.send(WirelessCommand::Scan);
    }

    /// Dispatch an asynchronous connect request.
    pub fn connect(&self, ssid: String, passphrase: Option<String>) {
        let _ = self
            .cmd_tx
            .send(WirelessCommand::Connect { ssid, passphrase });
    }

    /// Dispatch an asynchronous disconnect request.
    pub fn disconnect(&self) {
        let _ = self.cmd_tx.send(WirelessCommand::Disconnect);
    }

    /// Dispatch an asynchronous radio state change.
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.send(WirelessCommand::SetEnabled(enabled));
    }

    /// Dispatch an asynchronous forget request (ADR-0167).
    pub fn forget(&self, ssid: String) {
        let _ = self.cmd_tx.send(WirelessCommand::Forget { ssid });
    }

    /// Dispatch an asynchronous auto-connect preference update (ADR-0167).
    pub fn set_auto_connect(&self, ssid: String, auto_connect: bool) {
        let _ = self
            .cmd_tx
            .send(WirelessCommand::SetAutoConnect { ssid, auto_connect });
    }

    /// Read the latest snapshot without blocking.
    pub fn snapshot(&self) -> WirelessSnapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }
}
