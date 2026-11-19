//! The daemon-neutral network contract.
//!
//! A backend owns the live network service connection and publishes one
//! [`NetworkSnapshot`]. Consumers see only that snapshot and the verbs below;
//! they never see a D-Bus interface, object path, or daemon-specific state name
//! (`[INV-NET-DAEMON-NEUTRAL]`, ADR-0162).

use serde::{Deserialize, Serialize};
use tessera_desktop::system::{NetworkState, WifiLinkState, WifiNetwork};

/// Which live service backs a network snapshot.
///
/// This is diagnostic vocabulary, not a selection knob: the compositor reads
/// it for logs and status detail, and chooses a backend only through
/// [`BackendChoice`] in configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    /// `com.hihusky.Kestrel`.
    Kestrel,
    /// `org.freedesktop.NetworkManager`.
    #[serde(rename = "networkmanager")]
    NetworkManager,
    /// `net.connman.iwd`.
    Iwd,
    /// The deterministic preview/test backend. Never a production fallback.
    Mock,
    /// No service answered.
    #[default]
    Unavailable,
}

impl BackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kestrel => "kestrel",
            Self::NetworkManager => "networkmanager",
            Self::Iwd => "iwd",
            Self::Mock => "mock",
            Self::Unavailable => "unavailable",
        }
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The user's explicit backend preference (ADR-0242).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendChoice {
    /// Prefer Kestrel, then NetworkManager, then iwd (only when the higher-
    /// priority daemon is not running).
    ///
    /// A service is used only when it already owns its bus name: selection
    /// never D-Bus-activates any daemon. A stopped service is an operator
    /// decision, and starting one on a host managed by another authority would
    /// create a second radio manager.
    #[default]
    Auto,
    /// Kestrel only.
    Kestrel,
    /// NetworkManager only.
    #[serde(rename = "networkmanager")]
    NetworkManager,
    /// iwd only.
    Iwd,
}

impl BackendChoice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Kestrel => "kestrel",
            Self::NetworkManager => "networkmanager",
            Self::Iwd => "iwd",
        }
    }
}

impl std::fmt::Display for BackendChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A backend failure, classified by what a caller can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    /// No service owns the network role.
    Unavailable,
    /// The service exists but does not implement the requested operation.
    Unsupported(String),
    /// The service rejected the request (authorization, conflicting state).
    Refused(String),
    /// The named wireless network is not known to the service.
    NetworkNotFound(String),
    /// A transport, protocol, or platform failure.
    Service(String),
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("no network service is available"),
            Self::Unsupported(op) => write!(f, "network operation is not supported: {op}"),
            Self::Refused(msg) => write!(f, "network service refused the request: {msg}"),
            Self::NetworkNotFound(name) => write!(f, "network '{name}' is not known"),
            Self::Service(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for NetworkError {}

impl NetworkError {
    /// Whether this error means "no service", the only condition that lets
    /// [`BackendChoice::Auto`] try its next candidate.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }

    pub fn message(&self) -> String {
        self.to_string()
    }
}

/// One coherent reading of the host's network state.
///
/// The field names mirror [`tessera_desktop::system::SystemStatus`] so the
/// compositor can merge a snapshot without translating it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSnapshot {
    /// Whether a real service backs this snapshot. `false` means every other
    /// field is an honest empty reading, never last-known state
    /// (`[INV-ARCH-82]`, ADR-0190).
    pub available: bool,
    /// Which service answered.
    pub backend: BackendKind,
    /// The primary connection's coarse kind.
    pub network: NetworkState,
    /// Whether `network`/`network_interface` are authoritative.
    ///
    /// NetworkManager owns the primary link; the iwd fallback owns only the
    /// station role, so the compositor keeps its sysfs link probe for the
    /// coarse wired/offline answer when this is `false`.
    #[serde(default)]
    pub network_authoritative: bool,
    /// The primary interface's sysfs name, empty when offline.
    pub network_interface: String,
    /// `Some` only when a service answers; `None` means unavailable.
    pub wifi_enabled: Option<bool>,
    /// Fine-grained station link state.
    pub wifi_state: WifiLinkState,
    /// The associated network, if any.
    pub wifi_ssid: Option<String>,
    /// Discovered access points, deduplicated by SSID.
    pub wifi_networks: Vec<WifiNetwork>,
    /// A named failure from the service, if the last refresh failed.
    pub last_error: Option<String>,
}

impl NetworkSnapshot {
    /// The one honest reading for a missing service: no fabricated state.
    pub fn unavailable(backend: BackendKind, reason: impl Into<String>) -> Self {
        Self {
            available: false,
            backend,
            network: NetworkState::Offline,
            network_authoritative: false,
            network_interface: String::new(),
            wifi_enabled: None,
            wifi_state: WifiLinkState::Disconnected,
            wifi_ssid: None,
            wifi_networks: Vec::new(),
            last_error: Some(reason.into()),
        }
    }
}

/// The daemon-neutral contract implemented by NetworkManager, iwd, and mocks.
///
/// Methods are synchronous because the service runs them on its own worker
/// thread; no implementation may block a compositor frame. `poll` is the
/// coalescing gate: it returns `true` only when the service saw a signal or
/// an in-flight transition, so an idle desktop performs no D-Bus traffic.
pub trait NetworkBackend: Send + Sync {
    /// Which service backs this backend.
    fn kind(&self) -> BackendKind;

    /// The latest coherent reading. Must never block on the network service.
    fn snapshot(&self) -> NetworkSnapshot;

    /// Request an active scan.
    fn scan(&self) -> Result<(), NetworkError>;

    /// Connect to a network, optionally with a passphrase.
    fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), NetworkError>;

    /// Disconnect the station.
    fn disconnect(&self) -> Result<(), NetworkError>;

    /// Enable or disable the wireless radio.
    fn set_enabled(&self, enabled: bool) -> Result<(), NetworkError>;

    /// Forget the saved profile(s) for a network.
    fn forget(&self, ssid: &str) -> Result<(), NetworkError>;

    /// Enable or disable automatic connection for a saved network.
    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), NetworkError>;

    /// Re-read service state. Backends may coalesce or ignore this.
    fn refresh(&self) -> Result<(), NetworkError> {
        Ok(())
    }

    /// Whether a signal or transition needs a new snapshot on this cadence.
    fn poll(&self) -> bool {
        false
    }
}

/// The backend used when no service answered: it fabricates nothing.
pub struct UnavailableNetworkBackend {
    reason: String,
}

impl UnavailableNetworkBackend {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl Default for UnavailableNetworkBackend {
    fn default() -> Self {
        Self::new("no network service is available")
    }
}

impl NetworkBackend for UnavailableNetworkBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Unavailable
    }

    fn snapshot(&self) -> NetworkSnapshot {
        NetworkSnapshot::unavailable(BackendKind::Unavailable, self.reason.clone())
    }

    fn scan(&self) -> Result<(), NetworkError> {
        Err(NetworkError::Unavailable)
    }

    fn connect(&self, _ssid: &str, _passphrase: Option<&str>) -> Result<(), NetworkError> {
        Err(NetworkError::Unavailable)
    }

    fn disconnect(&self) -> Result<(), NetworkError> {
        Err(NetworkError::Unavailable)
    }

    fn set_enabled(&self, _enabled: bool) -> Result<(), NetworkError> {
        Err(NetworkError::Unavailable)
    }

    fn forget(&self, _ssid: &str) -> Result<(), NetworkError> {
        Err(NetworkError::Unavailable)
    }

    fn set_auto_connect(&self, _ssid: &str, _auto_connect: bool) -> Result<(), NetworkError> {
        Err(NetworkError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inv_arch_82_unavailable_backend_fabricates_nothing() {
        let backend = UnavailableNetworkBackend::new("NetworkManager is not running");
        let snapshot = backend.snapshot();
        assert!(!snapshot.available);
        assert_eq!(snapshot.backend, BackendKind::Unavailable);
        assert_eq!(snapshot.wifi_enabled, None);
        assert!(snapshot.wifi_networks.is_empty());
        assert_eq!(snapshot.wifi_ssid, None);
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("NetworkManager is not running")
        );

        assert_eq!(backend.scan(), Err(NetworkError::Unavailable));
        assert_eq!(
            backend.connect("Home-WiFi-5G", None),
            Err(NetworkError::Unavailable)
        );
        assert_eq!(backend.disconnect(), Err(NetworkError::Unavailable));
        assert_eq!(backend.set_enabled(true), Err(NetworkError::Unavailable));
        assert_eq!(
            backend.forget("Home-WiFi-5G"),
            Err(NetworkError::Unavailable)
        );
        assert_eq!(
            backend.set_auto_connect("Home-WiFi-5G", true),
            Err(NetworkError::Unavailable)
        );
    }

    #[test]
    fn a_backend_choice_has_one_stable_wire_name() {
        assert_eq!(BackendChoice::Auto.as_str(), "auto");
        assert_eq!(BackendChoice::Kestrel.as_str(), "kestrel");
        assert_eq!(BackendChoice::NetworkManager.as_str(), "networkmanager");
        assert_eq!(BackendChoice::Iwd.as_str(), "iwd");
    }

    #[test]
    fn a_backend_choice_round_trips_through_toml_vocabulary() {
        assert_eq!(
            serde_json::from_str::<BackendChoice>("\"kestrel\"").unwrap(),
            BackendChoice::Kestrel
        );
        assert_eq!(
            serde_json::from_str::<BackendChoice>("\"networkmanager\"").unwrap(),
            BackendChoice::NetworkManager
        );
        assert_eq!(
            serde_json::to_string(&BackendChoice::Iwd).unwrap(),
            "\"iwd\""
        );
    }
}
