//! Wireless network management subsystem (ADR-0162).
//!
//! Provides a daemon-neutral asynchronous bridge to host wireless services
//! (primarily `iwd`), with deterministic mock support for test suites.

pub mod iwd;
pub mod mock;

use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tessera_desktop::system::{WifiLinkState, WifiNetwork};

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
    /// Get current link state.
    fn state(&self) -> WifiLinkState;
    /// Get currently discovered networks.
    fn networks(&self) -> Vec<WifiNetwork>;
    /// Get the SSID of the currently associated network, if any.
    fn active_ssid(&self) -> Option<String>;
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
}

/// Snapshot of the wireless state shared with the compositor.
#[derive(Debug, Clone, Default)]
pub struct WirelessSnapshot {
    pub state: WifiLinkState,
    pub active_ssid: Option<String>,
    pub networks: Vec<WifiNetwork>,
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
        }));

        let worker_snapshot = snapshot.clone();
        thread::Builder::new()
            .name("tessera-wireless".into())
            .spawn(move || {
                let backend = Box::new(backend) as Box<dyn WirelessBackend>;
                loop {
                    // Drain all pending commands non-blockingly or wait on first
                    match cmd_rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(cmd) => {
                            match cmd {
                                WirelessCommand::Scan => {
                                    let _ = backend.scan();
                                }
                                WirelessCommand::Connect { ssid, passphrase } => {
                                    let _ = backend.connect(&ssid, passphrase.as_deref());
                                }
                                WirelessCommand::Disconnect => {
                                    let _ = backend.disconnect();
                                }
                                WirelessCommand::SetEnabled(enabled) => {
                                    let _ = backend.set_enabled(enabled);
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }

                    // Update cached snapshot
                    if let Ok(mut snap) = worker_snapshot.lock() {
                        snap.state = backend.state();
                        snap.active_ssid = backend.active_ssid();
                        snap.networks = backend.networks();
                    }
                }
            })
            .expect("failed to spawn wireless worker thread");

        Self { cmd_tx, snapshot }
    }

    /// Try spawning with native iwd backend, falling back to mock if unavailable.
    pub fn spawn_auto() -> Self {
        if let Ok(iwd) = IwdBackend::open() {
            log::info!("wireless subsystem: initialized native iwd backend");
            Self::spawn_with_backend(iwd)
        } else {
            log::info!("wireless subsystem: iwd not available, using mock fallback");
            Self::spawn_with_backend(MockWirelessBackend::new())
        }
    }

    /// Dispatch an asynchronous scan request.
    pub fn request_scan(&self) {
        let _ = self.cmd_tx.send(WirelessCommand::Scan);
    }

    /// Dispatch an asynchronous connect request.
    pub fn connect(&self, ssid: String, passphrase: Option<String>) {
        let _ = self.cmd_tx.send(WirelessCommand::Connect { ssid, passphrase });
    }

    /// Dispatch an asynchronous disconnect request.
    pub fn disconnect(&self) {
        let _ = self.cmd_tx.send(WirelessCommand::Disconnect);
    }

    /// Dispatch an asynchronous radio state change.
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.send(WirelessCommand::SetEnabled(enabled));
    }

    /// Read the latest snapshot without blocking.
    pub fn snapshot(&self) -> WirelessSnapshot {
        self.snapshot
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}
