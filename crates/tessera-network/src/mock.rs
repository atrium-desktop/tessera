//! Deterministic network backend for preview and headless tests.
//!
//! This exists so the shell and the service protocol can be exercised without
//! a radio. It is reachable in the service only through the explicit preview
//! entry point; production selection never names it (`[INV-ARCH-82]`,
//! ADR-0190).

use std::sync::{Arc, Mutex};

use tessera_desktop::system::{NetworkState, WifiLinkState, WifiNetwork, WifiSecurity};

use crate::backend::{BackendKind, NetworkBackend, NetworkError, NetworkSnapshot};

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
                    auto_connect: true,
                    frequency_mhz: Some(5180),
                },
                WifiNetwork {
                    ssid: "Cafe-Guest-Open".to_string(),
                    signal_bars: 3,
                    security: WifiSecurity::Open,
                    is_connected: false,
                    is_saved: false,
                    auto_connect: false,
                    frequency_mhz: Some(2412),
                },
                WifiNetwork {
                    ssid: "Enterprise-Office".to_string(),
                    signal_bars: 2,
                    security: WifiSecurity::Enterprise,
                    is_connected: false,
                    is_saved: false,
                    auto_connect: false,
                    frequency_mhz: Some(5240),
                },
            ],
            last_connected_passphrase: None,
        }
    }
}

/// Headless controllable network backend for testing and simulation.
pub struct MockNetworkBackend {
    state: Arc<Mutex<MockState>>,
}

impl MockNetworkBackend {
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

    fn snapshot_of(state: &MockState) -> NetworkSnapshot {
        let connected = state.state == WifiLinkState::Connected;
        NetworkSnapshot {
            available: true,
            backend: BackendKind::Mock,
            network: if connected {
                NetworkState::Wifi
            } else {
                NetworkState::Offline
            },
            network_authoritative: true,
            network_interface: if state.enabled {
                "wlan0".to_string()
            } else {
                String::new()
            },
            wifi_enabled: Some(state.enabled),
            wifi_state: state.state,
            wifi_ssid: state.active_ssid.clone(),
            wifi_networks: state.networks.clone(),
            last_error: None,
        }
    }
}

impl Default for MockNetworkBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkBackend for MockNetworkBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Mock
    }

    fn snapshot(&self) -> NetworkSnapshot {
        Self::snapshot_of(&self.state.lock().unwrap_or_else(|error| error.into_inner()))
    }

    fn scan(&self) -> Result<(), NetworkError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.enabled {
            return Err(NetworkError::Refused(
                "the wireless radio is disabled".to_string(),
            ));
        }
        state.state = if state.active_ssid.is_some() {
            WifiLinkState::Connected
        } else {
            WifiLinkState::Disconnected
        };
        Ok(())
    }

    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), NetworkError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.enabled {
            return Err(NetworkError::Refused(
                "the wireless radio is disabled".to_string(),
            ));
        }
        if let Some(target) = state
            .networks
            .iter_mut()
            .find(|network| network.ssid == ssid)
        {
            if target.security != WifiSecurity::Open && !target.is_saved && passphrase.is_none() {
                return Err(NetworkError::Refused("passphrase required".to_string()));
            }
            target.is_connected = true;
            target.is_saved = true;
            target.auto_connect = true;
        } else {
            state.networks.push(WifiNetwork {
                ssid: ssid.to_string(),
                signal_bars: 4,
                security: if passphrase.is_some() {
                    WifiSecurity::WpaPsk
                } else {
                    WifiSecurity::Open
                },
                is_connected: true,
                is_saved: true,
                auto_connect: true,
                frequency_mhz: Some(5180),
            });
        }
        for network in &mut state.networks {
            if network.ssid != ssid {
                network.is_connected = false;
            }
        }
        state.active_ssid = Some(ssid.to_string());
        state.state = WifiLinkState::Connected;
        state.last_connected_passphrase = passphrase.map(str::to_string);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), NetworkError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        for network in &mut state.networks {
            network.is_connected = false;
        }
        state.active_ssid = None;
        state.state = WifiLinkState::Disconnected;
        Ok(())
    }

    fn forget(&self, ssid: &str) -> Result<(), NetworkError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let was_active = state.active_ssid.as_deref() == Some(ssid);
        if let Some(target) = state
            .networks
            .iter_mut()
            .find(|network| network.ssid == ssid)
        {
            target.is_saved = false;
            target.auto_connect = false;
            target.is_connected = false;
        }
        if was_active {
            state.active_ssid = None;
            state.state = WifiLinkState::Disconnected;
        }
        Ok(())
    }

    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), NetworkError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(target) = state
            .networks
            .iter_mut()
            .find(|network| network.ssid == ssid)
        {
            target.auto_connect = auto_connect;
            Ok(())
        } else {
            Err(NetworkError::NetworkNotFound(ssid.to_string()))
        }
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), NetworkError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.enabled = enabled;
        if !enabled {
            state.state = WifiLinkState::Disabled;
            state.active_ssid = None;
            for network in &mut state.networks {
                network.is_connected = false;
            }
        } else if state.state == WifiLinkState::Disabled {
            state.state = WifiLinkState::Disconnected;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_reports_a_complete_available_snapshot() {
        let snapshot = MockNetworkBackend::new().snapshot();
        assert!(snapshot.available);
        assert_eq!(snapshot.backend, BackendKind::Mock);
        assert_eq!(snapshot.wifi_enabled, Some(true));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Disconnected);
        assert_eq!(snapshot.wifi_networks.len(), 3);
        assert_eq!(snapshot.network, NetworkState::Offline);
    }

    #[test]
    fn connecting_updates_link_network_and_profiles() {
        let backend = MockNetworkBackend::new();
        backend.connect("Home-WiFi-5G", None).expect("connect");
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.wifi_state, WifiLinkState::Connected);
        assert_eq!(snapshot.wifi_ssid.as_deref(), Some("Home-WiFi-5G"));
        assert_eq!(snapshot.network, NetworkState::Wifi);
        assert_eq!(snapshot.network_interface, "wlan0");
        assert!(
            snapshot
                .wifi_networks
                .iter()
                .find(|network| network.ssid == "Home-WiFi-5G")
                .is_some_and(|network| network.is_connected && network.is_saved)
        );

        backend.disconnect().expect("disconnect");
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.wifi_state, WifiLinkState::Disconnected);
        assert_eq!(snapshot.wifi_ssid, None);
        assert_eq!(snapshot.network, NetworkState::Offline);
    }

    #[test]
    fn disabling_the_radio_clears_the_link_and_keeps_the_ap_list() {
        let backend = MockNetworkBackend::new();
        backend.connect("Home-WiFi-5G", None).expect("connect");
        backend.set_enabled(false).expect("disable");
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.wifi_enabled, Some(false));
        assert_eq!(snapshot.wifi_state, WifiLinkState::Disabled);
        assert_eq!(snapshot.wifi_ssid, None);
        assert_eq!(snapshot.network, NetworkState::Offline);
        assert_eq!(snapshot.wifi_networks.len(), 3);
    }

    #[test]
    fn a_secured_unsaved_network_requires_a_passphrase() {
        let backend = MockNetworkBackend::new();
        assert_eq!(
            backend.connect("Enterprise-Office", None),
            Err(NetworkError::Refused("passphrase required".to_string()))
        );
        backend
            .connect("Enterprise-Office", Some("secret"))
            .expect("connect");
        assert_eq!(
            backend.snapshot().wifi_ssid.as_deref(),
            Some("Enterprise-Office")
        );
    }
}
