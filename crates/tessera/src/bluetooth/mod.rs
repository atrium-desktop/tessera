//! Bluetooth peripheral management subsystem (ADR-0175).
//!
//! Provides a daemon-neutral asynchronous bridge to the host Bluetooth
//! service (BlueZ), with deterministic mock support for test suites. The
//! shape deliberately mirrors [`crate::wireless`]: a backend trait, a
//! command channel drained by one plain worker thread, and a shared snapshot
//! the compositor reads without blocking.

pub mod bluez;
pub mod mock;

use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tessera_desktop::system::{BluetoothDevice, BluetoothLinkState};

pub use bluez::BluezBackend;
pub use mock::{MockBluetoothBackend, MockState};

#[cfg(test)]
mod tests;

/// Daemon-neutral backend contract for managing Bluetooth peripherals.
pub trait BluetoothBackend: Send + Sync {
    /// Power the adapter on or off.
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
    /// Start an active discovery scan. Discovery auto-stops after a bounded
    /// window; a second call extends the window rather than restarting it.
    fn scan(&self) -> Result<(), String>;
    /// Pair with a discovered peripheral.
    fn pair(&self, address: &str) -> Result<(), String>;
    /// Connect to a known peripheral.
    fn connect(&self, address: &str) -> Result<(), String>;
    /// Tear down the link to a peripheral.
    fn disconnect(&self, address: &str) -> Result<(), String>;
    /// Remove host-side pairing keys for a peripheral.
    fn forget(&self, address: &str) -> Result<(), String>;
    /// Periodic refresh from the worker thread.
    ///
    /// Discovery results and link state move on their own, so the worker
    /// polls rather than only reacting to commands. Implementations bound
    /// the work internally (interval or discovery-window gate) so this can
    /// be called on every worker wakeup. Errors are non-fatal to the
    /// snapshot: a transient bus failure keeps the last known state.
    fn poll(&self);
    /// `Some(false)` is a powered-off adapter; `None` means no Bluetooth
    /// service answered at all.
    ///
    /// This is authoritative when a service exists. It is *stable* `None`
    /// when one does not — never a transient `None` from a failed bus call —
    /// so the runtime can fall back to the host's rfkill probe without the
    /// two sources ever fighting over the field.
    fn enabled(&self) -> Option<bool>;
    /// Current adapter/link state.
    fn state(&self) -> BluetoothLinkState;
    /// Known and discovered peripherals, ordered for presentation: connected
    /// first, then paired, then in-range only.
    fn devices(&self) -> Vec<BluetoothDevice>;
}

/// Commands routed to the Bluetooth worker thread.
#[derive(Debug, Clone)]
pub enum BluetoothCommand {
    SetEnabled(bool),
    Scan,
    Pair { address: String },
    Connect { address: String },
    Disconnect { address: String },
    Forget { address: String },
}

/// Snapshot of the Bluetooth state shared with the compositor.
#[derive(Debug, Clone, Default)]
pub struct BluetoothSnapshot {
    /// Adapter power from the service, or `None` when no service answered.
    pub enabled: Option<bool>,
    pub state: BluetoothLinkState,
    pub devices: Vec<BluetoothDevice>,
}

impl BluetoothSnapshot {
    /// The peripheral the HUD names, if the adapter holds a live link.
    pub fn connected_device(&self) -> Option<&BluetoothDevice> {
        self.devices.iter().find(|device| device.connected)
    }
}

/// Backend for a host with no Bluetooth service: honest and inert.
///
/// It never fabricates a radio state (`enabled()` is `None`) and refuses
/// every mutation, so chrome presents the adapter as unavailable instead of
/// pretending. Chosen over the wireless subsystem's mock fallback, which
/// seeds fixture networks — acceptable for a scan list, misleading for a
/// device list that would look like real hardware.
#[derive(Debug, Default)]
struct UnavailableBackend;

impl BluetoothBackend for UnavailableBackend {
    fn set_enabled(&self, _enabled: bool) -> Result<(), String> {
        Err("no bluetooth service available".into())
    }
    fn scan(&self) -> Result<(), String> {
        Err("no bluetooth service available".into())
    }
    fn pair(&self, _address: &str) -> Result<(), String> {
        Err("no bluetooth service available".into())
    }
    fn connect(&self, _address: &str) -> Result<(), String> {
        Err("no bluetooth service available".into())
    }
    fn disconnect(&self, _address: &str) -> Result<(), String> {
        Err("no bluetooth service available".into())
    }
    fn forget(&self, _address: &str) -> Result<(), String> {
        Err("no bluetooth service available".into())
    }
    fn poll(&self) {}
    fn enabled(&self) -> Option<bool> {
        None
    }
    fn state(&self) -> BluetoothLinkState {
        BluetoothLinkState::Unavailable
    }
    fn devices(&self) -> Vec<BluetoothDevice> {
        Vec::new()
    }
}

/// Non-blocking handle to the Bluetooth subsystem.
#[derive(Clone)]
pub struct BluetoothHandle {
    cmd_tx: mpsc::Sender<BluetoothCommand>,
    snapshot: Arc<Mutex<BluetoothSnapshot>>,
}

impl BluetoothHandle {
    /// Spawn the Bluetooth worker with an explicit backend.
    pub fn spawn_with_backend<B: BluetoothBackend + 'static>(backend: B) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<BluetoothCommand>();
        let snapshot = Arc::new(Mutex::new(BluetoothSnapshot {
            enabled: backend.enabled(),
            state: backend.state(),
            devices: backend.devices(),
        }));

        let worker_snapshot = snapshot.clone();
        thread::Builder::new()
            .name("tessera-bluetooth".into())
            .spawn(move || {
                let backend = Box::new(backend) as Box<dyn BluetoothBackend>;
                loop {
                    // Drain one pending command, or wait on the first.
                    match cmd_rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(cmd) => match cmd {
                            BluetoothCommand::SetEnabled(enabled) => {
                                let _ = backend.set_enabled(enabled);
                            }
                            BluetoothCommand::Scan => {
                                let _ = backend.scan();
                            }
                            BluetoothCommand::Pair { address } => {
                                let _ = backend.pair(&address);
                            }
                            BluetoothCommand::Connect { address } => {
                                let _ = backend.connect(&address);
                            }
                            BluetoothCommand::Disconnect { address } => {
                                let _ = backend.disconnect(&address);
                            }
                            BluetoothCommand::Forget { address } => {
                                let _ = backend.forget(&address);
                            }
                        },
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }

                    // Asynchronous state also moves without a command (a peer
                    // arriving, a link dropping), so poll before publishing.
                    backend.poll();
                    if let Ok(mut snap) = worker_snapshot.lock() {
                        snap.enabled = backend.enabled();
                        snap.state = backend.state();
                        snap.devices = backend.devices();
                    }
                }
            })
            .expect("failed to spawn bluetooth worker thread");

        Self { cmd_tx, snapshot }
    }

    /// Try spawning with the native BlueZ backend, falling back to an inert
    /// unavailable backend when the host has no Bluetooth service.
    pub fn spawn_auto() -> Self {
        if let Ok(backend) = BluezBackend::open() {
            log::info!("bluetooth subsystem: initialized native BlueZ backend");
            Self::spawn_with_backend(backend)
        } else {
            log::info!("bluetooth subsystem: BlueZ not available, adapter reported unavailable");
            Self::spawn_with_backend(UnavailableBackend)
        }
    }

    /// Dispatch an asynchronous adapter power change.
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.send(BluetoothCommand::SetEnabled(enabled));
    }

    /// Dispatch an asynchronous discovery scan request.
    pub fn request_scan(&self) {
        let _ = self.cmd_tx.send(BluetoothCommand::Scan);
    }

    /// Dispatch an asynchronous pair request.
    pub fn pair(&self, address: String) {
        let _ = self.cmd_tx.send(BluetoothCommand::Pair { address });
    }

    /// Dispatch an asynchronous connect request.
    pub fn connect(&self, address: String) {
        let _ = self.cmd_tx.send(BluetoothCommand::Connect { address });
    }

    /// Dispatch an asynchronous disconnect request.
    pub fn disconnect(&self, address: String) {
        let _ = self.cmd_tx.send(BluetoothCommand::Disconnect { address });
    }

    /// Dispatch an asynchronous forget request.
    pub fn forget(&self, address: String) {
        let _ = self.cmd_tx.send(BluetoothCommand::Forget { address });
    }

    /// Read the latest snapshot without blocking.
    pub fn snapshot(&self) -> BluetoothSnapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }
}
