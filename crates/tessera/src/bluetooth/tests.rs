//! Unit tests for the Bluetooth subsystem and mock backend.

use super::*;
use std::time::Duration;

#[test]
fn mock_backend_initial_state() {
    let mock = MockBluetoothBackend::new();
    // Radio on/off is not exposed by the backend: the host rfkill probe owns
    // `SystemStatus::bluetooth_enabled`, exactly as for Wi-Fi (ADR-0175).
    assert_eq!(mock.state(), BluetoothLinkState::Idle);
    let devices = mock.devices();
    assert_eq!(devices.len(), 3);
    assert_eq!(devices[0].name, "Sony WH-1000XM5");
    assert!(devices[0].paired);
    assert!(!devices[0].connected);
}

#[test]
fn mock_backend_connect_disconnect_and_scan() {
    let mock = MockBluetoothBackend::new();
    // Scanning latches and settles back to Idle on the next poll.
    assert!(mock.scan().is_ok());
    assert_eq!(mock.state(), BluetoothLinkState::Scanning);
    mock.poll();
    assert_eq!(mock.state(), BluetoothLinkState::Idle);

    assert!(mock.connect("AC:12:34:56:78:9A").is_ok());
    assert_eq!(mock.state(), BluetoothLinkState::Connected);
    let connected = mock
        .devices()
        .into_iter()
        .find(|device| device.connected)
        .expect("a connected device");
    assert_eq!(connected.address, "AC:12:34:56:78:9A");

    assert!(mock.disconnect("AC:12:34:56:78:9A").is_ok());
    assert_eq!(mock.state(), BluetoothLinkState::Idle);
    assert!(mock.devices().iter().all(|device| !device.connected));
}

#[test]
fn mock_backend_rejects_unknown_device_and_disabled_adapter() {
    let mock = MockBluetoothBackend::new();
    assert!(mock.connect("00:00:00:00:00:00").is_err());
    assert!(mock.pair("00:00:00:00:00:00").is_err());

    assert!(mock.set_enabled(false).is_ok());
    assert_eq!(mock.state(), BluetoothLinkState::Disabled);
    assert!(mock.scan().is_err());
    assert!(mock.connect("AC:12:34:56:78:9A").is_err());

    assert!(mock.set_enabled(true).is_ok());
    assert_eq!(mock.state(), BluetoothLinkState::Idle);
}

#[test]
fn mock_backend_pair_and_forget_lifecycle() {
    let mock = MockBluetoothBackend::new();
    let unpaired = "F0:11:22:33:44:55";
    let entry = |mock: &MockBluetoothBackend| {
        mock.devices()
            .into_iter()
            .find(|device| device.address == unpaired)
            .expect("Pixel Buds")
    };
    assert!(!entry(&mock).paired);

    assert!(mock.pair(unpaired).is_ok());
    assert!(entry(&mock).paired);

    // Forgetting drops the pairing and any live link.
    assert!(mock.connect(unpaired).is_ok());
    assert!(mock.forget(unpaired).is_ok());
    let forgotten = entry(&mock);
    assert!(!forgotten.paired);
    assert!(!forgotten.connected);
    assert_eq!(mock.state(), BluetoothLinkState::Idle);
}

#[test]
fn unavailable_backend_is_honest_and_inert() {
    // The fallback for a host with no Bluetooth service must not fabricate a
    // device list and must refuse every mutation. Tested directly rather than
    // through `spawn_auto`, whose backend depends on the host bus.
    let backend = UnavailableBackend;
    assert_eq!(backend.state(), BluetoothLinkState::Unavailable);
    assert!(backend.devices().is_empty());
    for result in [
        backend.set_enabled(true),
        backend.scan(),
        backend.pair("AC:12:34:56:78:9A"),
        backend.connect("AC:12:34:56:78:9A"),
        backend.disconnect("AC:12:34:56:78:9A"),
        backend.forget("AC:12:34:56:78:9A"),
    ] {
        assert!(result.is_err(), "an absent service must refuse mutation");
    }
}

#[test]
fn bluetooth_handle_processes_async_commands() {
    let mock = MockBluetoothBackend::new();
    let handle = BluetoothHandle::spawn_with_backend(mock);

    let snap = handle.snapshot();
    assert_eq!(snap.state, BluetoothLinkState::Idle);
    assert_eq!(snap.devices.len(), 3);

    handle.connect("AC:12:34:56:78:9A".to_string());
    std::thread::sleep(Duration::from_millis(600));
    let snap = handle.snapshot();
    assert_eq!(snap.state, BluetoothLinkState::Connected);
    assert_eq!(
        snap.connected_device().map(|device| device.name.as_str()),
        Some("Sony WH-1000XM5")
    );

    handle.disconnect("AC:12:34:56:78:9A".to_string());
    std::thread::sleep(Duration::from_millis(600));
    let snap = handle.snapshot();
    assert_eq!(snap.state, BluetoothLinkState::Idle);
    assert!(snap.connected_device().is_none());
}

#[test]
fn bluetooth_handle_processes_pair_forget_and_radio_toggle() {
    let mock = MockBluetoothBackend::new();
    let handle = BluetoothHandle::spawn_with_backend(mock);

    handle.pair("F0:11:22:33:44:55".to_string());
    std::thread::sleep(Duration::from_millis(600));
    let paired = handle
        .snapshot()
        .devices
        .into_iter()
        .find(|device| device.address == "F0:11:22:33:44:55")
        .expect("Pixel Buds");
    assert!(paired.paired);

    handle.forget("F0:11:22:33:44:55".to_string());
    std::thread::sleep(Duration::from_millis(600));
    let forgotten = handle
        .snapshot()
        .devices
        .into_iter()
        .find(|device| device.address == "F0:11:22:33:44:55")
        .expect("Pixel Buds");
    assert!(!forgotten.paired);

    // Powering the adapter off is observable through the link state: the
    // radio boolean itself is owned by the host probe, not the subsystem.
    handle.set_enabled(false);
    std::thread::sleep(Duration::from_millis(600));
    let snap = handle.snapshot();
    assert_eq!(snap.state, BluetoothLinkState::Disabled);
    assert!(snap.connected_device().is_none());
}
