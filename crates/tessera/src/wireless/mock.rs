//! Mock implementation of the wireless backend for headless testing.

use std::sync::{Arc, Mutex};
use tessera_desktop::system::{WifiLinkState, WifiNetwork, WifiSecurity};

use super::WirelessBackend;

#[derive(Debug, Clone)]
pub struct MockState {
    pub enabled: bool,
    pub state: WifiLinkState,
    pub active_ssid: Option<String>,
    pub networks: Vec<WifiNetwork>,
    pub last_connected_passphrase: Option<String>,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            enabled: true,
            state: WifiLinkState::Disconnected,
            active_ssid: None,
            networks: vec![
                WifiNetwork {
                    ssid: "Home-WiFi-5G".to_string(),
                    signal_bars: 4,
                    security: WifiSecurity::WpaPsk,
                    is_connected: false,
                    is_saved: true,
                },
                WifiNetwork {
                    ssid: "Cafe-Guest-Open".to_string(),
                    signal_bars: 3,
                    security: WifiSecurity::Open,
                    is_connected: false,
                    is_saved: false,
                },
                WifiNetwork {
                    ssid: "Enterprise-Office".to_string(),
                    signal_bars: 2,
                    security: WifiSecurity::Enterprise,
                    is_connected: false,
                    is_saved: false,
                },
            ],
            last_connected_passphrase: None,
        }
    }
}

/// Headless controllable wireless backend for testing and simulation.
pub struct MockWirelessBackend {
    state: Arc<Mutex<MockState>>,
}

impl MockWirelessBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockState::default())),
        }
    }

    pub fn with_state(state: MockState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn inner(&self) -> Arc<Mutex<MockState>> {
        self.state.clone()
    }
}

impl Default for MockWirelessBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WirelessBackend for MockWirelessBackend {
    fn scan(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.enabled {
            return Err("wireless radio is disabled".to_string());
        }
        st.state = WifiLinkState::Scanning;
        // In mock, scanning retains or updates networks
        if st.active_ssid.is_some() {
            st.state = WifiLinkState::Connected;
        } else {
            st.state = WifiLinkState::Disconnected;
        }
        Ok(())
    }

    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.enabled {
            return Err("wireless radio is disabled".to_string());
        }
        let found = st.networks.iter_mut().find(|n| n.ssid == ssid);
        if let Some(target) = found {
            if target.security != WifiSecurity::Open && !target.is_saved && passphrase.is_none() {
                return Err("passphrase required".to_string());
            }
            target.is_connected = true;
            target.is_saved = true;
        }
        for n in &mut st.networks {
            if n.ssid != ssid {
                n.is_connected = false;
            }
        }
        st.active_ssid = Some(ssid.to_string());
        st.state = WifiLinkState::Connected;
        st.last_connected_passphrase = passphrase.map(str::to_string);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        for n in &mut st.networks {
            n.is_connected = false;
        }
        st.active_ssid = None;
        st.state = WifiLinkState::Disconnected;
        Ok(())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.enabled = enabled;
        if !enabled {
            st.state = WifiLinkState::Disabled;
            st.active_ssid = None;
            for n in &mut st.networks {
                n.is_connected = false;
            }
        } else if st.state == WifiLinkState::Disabled {
            st.state = WifiLinkState::Disconnected;
        }
        Ok(())
    }

    fn state(&self) -> WifiLinkState {
        self.state.lock().unwrap().state
    }

    fn networks(&self) -> Vec<WifiNetwork> {
        self.state.lock().unwrap().networks.clone()
    }

    fn active_ssid(&self) -> Option<String> {
        self.state.lock().unwrap().active_ssid.clone()
    }
}
