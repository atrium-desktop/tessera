//! Unit tests for the wireless subsystem and mock backend.

use super::*;
use std::time::Duration;
use tessera_desktop::system::WifiSecurity;

#[test]
fn mock_backend_initial_state() {
    let mock = MockWirelessBackend::new();
    assert_eq!(mock.state(), WifiLinkState::Disconnected);
    assert_eq!(mock.active_ssid(), None);
    let nets = mock.networks();
    assert_eq!(nets.len(), 3);
    assert_eq!(nets[0].ssid, "Home-WiFi-5G");
    assert_eq!(nets[0].security, WifiSecurity::WpaPsk);
}

#[test]
fn test_inv_arch_82_unavailable_backend_fabricates_nothing() {
    let backend = UnavailableWirelessBackend;
    // No daemon backs it, so it declares itself unavailable (`[INV-ARCH-82]`).
    assert!(!backend.available());
    // Every read is empty; there is no plausible-looking lie.
    assert!(backend.networks().is_empty());
    assert_eq!(backend.active_ssid(), None);
    assert_eq!(backend.state(), WifiLinkState::Disconnected);
    // Every verb is refused rather than silently accepted.
    assert!(backend.scan().is_err());
    assert!(backend.connect("Home-WiFi-5G", None).is_err());
    assert!(backend.disconnect().is_err());
    assert!(backend.set_enabled(true).is_err());
    assert!(backend.forget("Home-WiFi-5G").is_err());
    assert!(backend.set_auto_connect("Home-WiFi-5G", true).is_err());

    // The handle surfaces availability so the compositor can report wireless
    // as unavailable instead of "no networks found".
    let handle = WirelessHandle::spawn_with_backend(UnavailableWirelessBackend);
    let snap = handle.snapshot();
    assert!(!snap.available);
    assert!(snap.networks.is_empty());

    // A real backend (the mock stands in for a daemon-backed one) is available.
    let live = WirelessHandle::spawn_with_backend(MockWirelessBackend::new());
    assert!(live.snapshot().available);
}

#[test]
fn mock_backend_connect_and_disconnect() {
    let mock = MockWirelessBackend::new();
    // Connecting to known saved network succeeds without password
    assert!(mock.connect("Home-WiFi-5G", None).is_ok());
    assert_eq!(mock.state(), WifiLinkState::Connected);
    assert_eq!(mock.active_ssid().as_deref(), Some("Home-WiFi-5G"));

    // Connecting to enterprise or unsaved psk without password fails
    assert!(mock.connect("Enterprise-Office", None).is_err());

    // Connecting to enterprise with password succeeds
    assert!(mock.connect("Enterprise-Office", Some("sec456")).is_ok());
    assert_eq!(mock.active_ssid().as_deref(), Some("Enterprise-Office"));

    // Disconnect works
    assert!(mock.disconnect().is_ok());
    assert_eq!(mock.state(), WifiLinkState::Disconnected);
    assert_eq!(mock.active_ssid(), None);
}

#[test]
fn mock_backend_radio_toggle() {
    let mock = MockWirelessBackend::new();
    assert!(mock.set_enabled(false).is_ok());
    assert_eq!(mock.state(), WifiLinkState::Disabled);
    assert!(mock.connect("Home-WiFi-5G", None).is_err());
    assert!(mock.scan().is_err());

    assert!(mock.set_enabled(true).is_ok());
    assert_eq!(mock.state(), WifiLinkState::Disconnected);
}

#[test]
fn wireless_handle_processes_async_commands() {
    let mock = MockWirelessBackend::new();
    let handle = WirelessHandle::spawn_with_backend(mock);

    // Initial snapshot
    let snap = handle.snapshot();
    assert_eq!(snap.state, WifiLinkState::Disconnected);
    assert_eq!(snap.networks.len(), 3);

    // Send async connect
    handle.connect("Cafe-Guest-Open".to_string(), None);
    std::thread::sleep(Duration::from_millis(600));

    let snap = handle.snapshot();
    assert_eq!(snap.state, WifiLinkState::Connected);
    assert_eq!(snap.active_ssid.as_deref(), Some("Cafe-Guest-Open"));

    // Send async disconnect
    handle.disconnect();
    std::thread::sleep(Duration::from_millis(600));

    let snap = handle.snapshot();
    assert_eq!(snap.state, WifiLinkState::Disconnected);
    assert_eq!(snap.active_ssid, None);
}

#[test]
fn mock_backend_forget_and_auto_connect() {
    let mock = MockWirelessBackend::new();
    let nets = mock.networks();
    assert!(nets[0].is_saved);
    assert!(nets[0].auto_connect);

    // Toggle auto connect
    assert!(mock.set_auto_connect("Home-WiFi-5G", false).is_ok());
    let nets = mock.networks();
    assert!(!nets[0].auto_connect);

    // Connect first
    assert!(mock.connect("Home-WiFi-5G", None).is_ok());
    assert_eq!(mock.state(), WifiLinkState::Connected);

    // Forget discards saved profile and tears down active connection
    assert!(mock.forget("Home-WiFi-5G").is_ok());
    assert_eq!(mock.state(), WifiLinkState::Disconnected);
    assert_eq!(mock.active_ssid(), None);
    let nets = mock.networks();
    assert!(!nets[0].is_saved);
    assert!(!nets[0].auto_connect);
    assert!(!nets[0].is_connected);
}

#[test]
fn wireless_handle_processes_forget_and_auto_connect() {
    let mock = MockWirelessBackend::new();
    let handle = WirelessHandle::spawn_with_backend(mock);

    // Connect to Home-WiFi-5G
    handle.connect("Home-WiFi-5G".to_string(), None);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(handle.snapshot().state, WifiLinkState::Connected);

    // Send async auto_connect update
    handle.set_auto_connect("Home-WiFi-5G".to_string(), false);
    std::thread::sleep(Duration::from_millis(600));
    let home = handle
        .snapshot()
        .networks
        .into_iter()
        .find(|n| n.ssid == "Home-WiFi-5G")
        .expect("Home-WiFi-5G");
    assert!(!home.auto_connect);

    // Send async forget
    handle.forget("Home-WiFi-5G".to_string());
    std::thread::sleep(Duration::from_millis(600));
    let snap = handle.snapshot();
    assert_eq!(snap.state, WifiLinkState::Disconnected);
    let home = snap
        .networks
        .into_iter()
        .find(|n| n.ssid == "Home-WiFi-5G")
        .expect("Home-WiFi-5G");
    assert!(!home.is_saved);
    assert!(!home.auto_connect);
}

/// A backend that counts snapshot reads, so tests can assert the worker
/// publishes only when the world actually moved.
struct CountingBackend {
    inner: Arc<Mutex<MockState>>,
    reads: Arc<std::sync::atomic::AtomicU64>,
}

impl CountingBackend {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockState::default())),
            reads: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

impl WirelessBackend for CountingBackend {
    fn scan(&self) -> Result<(), String> {
        Ok(())
    }
    fn connect(&self, _ssid: &str, _passphrase: Option<&str>) -> Result<(), String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).state = WifiLinkState::Connected;
        Ok(())
    }
    fn disconnect(&self) -> Result<(), String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).state = WifiLinkState::Disconnected;
        Ok(())
    }
    fn set_enabled(&self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }
    fn forget(&self, _ssid: &str) -> Result<(), String> {
        Ok(())
    }
    fn set_auto_connect(&self, _ssid: &str, _auto_connect: bool) -> Result<(), String> {
        Ok(())
    }
    fn poll(&self) -> bool {
        false
    }
    fn state(&self) -> WifiLinkState {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).state
    }
    fn networks(&self) -> Vec<WifiNetwork> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).networks.clone()
    }
    fn active_ssid(&self) -> Option<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).active_ssid.clone()
    }
    fn available(&self) -> bool {
        true
    }
}

#[test]
fn worker_skips_snapshot_publish_while_quiet() {
    let backend = CountingBackend::new();
    let reads_counter = backend.reads.clone();
    let initial_reads = reads_counter.load(std::sync::atomic::Ordering::Relaxed);
    let handle = WirelessHandle::spawn_with_backend(backend);

    // The spawn publishes the initial snapshot once.
    std::thread::sleep(Duration::from_millis(400));
    let after_idle = handle
        .snapshot()
        .networks
        .len();
    assert_eq!(after_idle, 3);

    // In a quiet window the worker must not keep re-reading the backend:
    // the snapshot reads stay at the initial + spawn count, i.e. the worker
    // did no extra publish work while nothing changed.
    let reads = reads_counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        reads <= initial_reads + 2,
        "worker re-read the backend {reads} times while idle; expected no periodic publish"
    );

    // A command causes one publish.
    handle.connect("Cafe-Guest-Open".to_string(), None);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(handle.snapshot().state, WifiLinkState::Connected);
}
