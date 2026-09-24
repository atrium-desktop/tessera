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
