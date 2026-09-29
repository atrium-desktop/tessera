//! Mock implementation of the Bluetooth backend for headless testing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tessera_desktop::system::{BluetoothDevice, BluetoothKind, BluetoothLinkState};

use super::BluetoothBackend;

/// Controllable mock state. `devices` is the discovered/pairing database and
/// `enabled`/`connected` overlay the live link, mirroring how BlueZ keeps a
/// per-device `Connected` flag separate from the adapter's `Powered`.
#[derive(Debug, Clone)]
pub struct MockState {
    pub enabled: bool,
    pub state: BluetoothLinkState,
    pub devices: Vec<BluetoothDevice>,
    /// True while a discovery scan is "in flight"; the mock clears it on the
    /// next `poll`, so chrome observes a real Scanning → Idle transition.
    pub scanning: bool,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            enabled: true,
            state: BluetoothLinkState::Idle,
            devices: vec![
                BluetoothDevice {
                    address: "AC:12:34:56:78:9A".to_string(),
                    name: "Sony WH-1000XM5".to_string(),
                    kind: BluetoothKind::Audio,
                    connected: false,
                    paired: true,
                    trusted: true,
                },
                BluetoothDevice {
                    address: "10:20:30:40:50:60".to_string(),
                    name: "Keychron K3".to_string(),
                    kind: BluetoothKind::Input,
                    connected: false,
                    paired: true,
                    trusted: true,
                },
                BluetoothDevice {
                    address: "F0:11:22:33:44:55".to_string(),
                    name: "Pixel Buds".to_string(),
                    kind: BluetoothKind::Audio,
                    connected: false,
                    paired: false,
                    trusted: false,
                },
            ],
            scanning: false,
        }
    }
}

/// Headless controllable Bluetooth backend for testing and simulation.
pub struct MockBluetoothBackend {
    state: Arc<Mutex<MockState>>,
    /// Pairing keys BlueZ would hold host-side; `forget` drops them.
    paired: Arc<Mutex<HashMap<String, bool>>>,
}

impl MockBluetoothBackend {
    pub fn new() -> Self {
        Self::with_state(MockState::default())
    }

    pub fn with_state(state: MockState) -> Self {
        let paired = state
            .devices
            .iter()
            .map(|device| (device.address.clone(), device.paired))
            .collect();
        Self {
            state: Arc::new(Mutex::new(state)),
            paired: Arc::new(Mutex::new(paired)),
        }
    }

    pub fn inner(&self) -> Arc<Mutex<MockState>> {
        self.state.clone()
    }

    /// Connect a named seed device in one call, the mock analogue of the
    /// preview seeding the real runtime does for the wireless subsystem.
    pub fn connect(&self, address: &str) -> Result<(), String> {
        BluetoothBackend::connect(self, address)
    }
}

impl Default for MockBluetoothBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl BluetoothBackend for MockBluetoothBackend {
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.enabled = enabled;
        if !enabled {
            st.state = BluetoothLinkState::Disabled;
            st.scanning = false;
            for device in &mut st.devices {
                device.connected = false;
            }
        } else if st.state == BluetoothLinkState::Disabled {
            st.state = BluetoothLinkState::Idle;
        }
        Ok(())
    }

    fn scan(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.enabled {
            return Err("bluetooth adapter is disabled".to_string());
        }
        st.state = BluetoothLinkState::Scanning;
        st.scanning = true;
        Ok(())
    }

    fn pair(&self, address: &str) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.enabled {
            return Err("bluetooth adapter is disabled".to_string());
        }
        let device = st
            .devices
            .iter_mut()
            .find(|device| device.address == address)
            .ok_or_else(|| format!("device '{address}' not found"))?;
        device.paired = true;
        self.paired
            .lock()
            .unwrap()
            .insert(address.to_string(), true);
        Ok(())
    }

    fn connect(&self, address: &str) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.enabled {
            return Err("bluetooth adapter is disabled".to_string());
        }
        if !st.devices.iter().any(|device| device.address == address) {
            return Err(format!("device '{address}' not found"));
        }
        for device in &mut st.devices {
            device.connected = device.address == address;
        }
        st.state = BluetoothLinkState::Connected;
        Ok(())
    }

    fn disconnect(&self, address: &str) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let was = st.state == BluetoothLinkState::Connected;
        for device in &mut st.devices {
            if device.address == address {
                device.connected = false;
            }
        }
        if was {
            st.state = BluetoothLinkState::Idle;
        }
        Ok(())
    }

    fn forget(&self, address: &str) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let mut keys = self.paired.lock().unwrap();
        keys.remove(address);
        let was_connected = st.state == BluetoothLinkState::Connected;
        if let Some(device) = st.devices.iter_mut().find(|d| d.address == address) {
            device.paired = false;
            device.trusted = false;
            if device.connected {
                device.connected = false;
                if was_connected {
                    st.state = BluetoothLinkState::Idle;
                }
            }
        }
        Ok(())
    }

    fn poll(&self) {
        let mut st = self.state.lock().unwrap();
        // A scan settles on the next poll: the enumerated device list is
        // already present, so discovery only clears the `Scanning` flag.
        if st.scanning {
            st.scanning = false;
            st.state = if st.devices.iter().any(|device| device.connected) {
                BluetoothLinkState::Connected
            } else {
                BluetoothLinkState::Idle
            };
        }
    }

    fn enabled(&self) -> Option<bool> {
        Some(self.state.lock().unwrap().enabled)
    }

    fn state(&self) -> BluetoothLinkState {
        self.state.lock().unwrap().state
    }

    fn devices(&self) -> Vec<BluetoothDevice> {
        self.state.lock().unwrap().devices.clone()
    }
}
