//! Explicit backend selection (ADR-0242).
//!
//! Selection is a policy, not a guess:
//!
//! - `auto` prefers NetworkManager, and falls through to iwd **only when
//!   NetworkManager is not running**. Neither service is D-Bus-activated: a
//!   stopped daemon is an operator decision, and starting one on a host where
//!   the other owns the radio would create a second radio manager.
//! - an explicit choice never falls back. If the named service is missing,
//!   the service reports unavailable.
//! - `mock` exists only for preview/tests and is never a production fallback
//!   (`[INV-ARCH-82]`, ADR-0190).

use std::str::FromStr;

use tessera_network::{
    MockNetworkBackend, NetworkBackend, NetworkError, UnavailableNetworkBackend,
};

use crate::iwd::IwdBackend;
use crate::networkmanager::NetworkManagerBackend;

/// The backend a service process was asked to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonBackend {
    Auto,
    NetworkManager,
    Iwd,
    Mock,
}

impl DaemonBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::NetworkManager => "networkmanager",
            Self::Iwd => "iwd",
            Self::Mock => "mock",
        }
    }
}

impl std::fmt::Display for DaemonBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for DaemonBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "networkmanager" | "network-manager" => Ok(Self::NetworkManager),
            "iwd" => Ok(Self::Iwd),
            "mock" => Ok(Self::Mock),
            other => Err(format!(
                "unknown network backend '{other}'; expected auto, networkmanager, iwd, or mock"
            )),
        }
    }
}

/// Open the selected backend, reporting one honest snapshot when none answers.
pub fn open_backend(choice: DaemonBackend) -> Box<dyn NetworkBackend> {
    let backend = select_backend(
        choice,
        || {
            NetworkManagerBackend::open()
                .map(|backend| Box::new(backend) as Box<dyn NetworkBackend>)
        },
        || IwdBackend::open().map(|backend| Box::new(backend) as Box<dyn NetworkBackend>),
    );
    let snapshot = backend.snapshot();
    if snapshot.available {
        log::info!(
            "network service: using {} backend",
            snapshot.backend.as_str()
        );
    } else {
        log::debug!(
            "network service: {} backend unavailable: {}",
            choice.as_str(),
            snapshot
                .last_error
                .as_deref()
                .unwrap_or("no network service is available")
        );
    }
    backend
}

/// The selection rule, separated from the live constructors so it is testable
/// without a bus or a radio.
fn select_backend(
    choice: DaemonBackend,
    networkmanager: impl FnOnce() -> Result<Box<dyn NetworkBackend>, NetworkError>,
    iwd: impl FnOnce() -> Result<Box<dyn NetworkBackend>, NetworkError>,
) -> Box<dyn NetworkBackend> {
    match choice {
        DaemonBackend::Mock => {
            // Preview and headless tests still show a live-looking control
            // center; the seed is the same fixture the old in-process preview
            // connected before the service existed.
            let backend = MockNetworkBackend::new();
            let _ = backend.connect("Home-WiFi-5G", None);
            Box::new(backend)
        }
        DaemonBackend::NetworkManager => match networkmanager() {
            Ok(backend) => backend,
            Err(error) => unavailable(format!("NetworkManager: {error}")),
        },
        DaemonBackend::Iwd => match iwd() {
            Ok(backend) => backend,
            Err(error) => unavailable(format!("iwd: {error}")),
        },
        DaemonBackend::Auto => match networkmanager() {
            Ok(backend) => backend,
            Err(error) if error.is_unavailable() => match iwd() {
                Ok(backend) => backend,
                Err(iwd_error) => unavailable(format!(
                    "NetworkManager is not running ({error}); iwd: {iwd_error}"
                )),
            },
            Err(error) => unavailable(format!("NetworkManager is present but unusable: {error}")),
        },
    }
}

fn unavailable(reason: String) -> Box<dyn NetworkBackend> {
    Box::new(UnavailableNetworkBackend::new(reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_network::BackendKind;

    fn mock() -> Result<Box<dyn NetworkBackend>, NetworkError> {
        Ok(Box::new(MockNetworkBackend::new()))
    }

    fn missing() -> Result<Box<dyn NetworkBackend>, NetworkError> {
        Err(NetworkError::Unavailable)
    }

    fn broken() -> Result<Box<dyn NetworkBackend>, NetworkError> {
        Err(NetworkError::Service("access denied".to_string()))
    }

    #[test]
    fn auto_prefers_networkmanager_when_it_answers() {
        let iwd_called = std::cell::Cell::new(false);
        let backend = select_backend(DaemonBackend::Auto, mock, || {
            iwd_called.set(true);
            Err(NetworkError::Unavailable)
        });
        assert_eq!(backend.kind(), BackendKind::Mock);
        assert!(
            !iwd_called.get(),
            "iwd must not be probed when NetworkManager answers"
        );
    }

    #[test]
    fn auto_falls_through_to_iwd_only_when_networkmanager_is_not_running() {
        let fallback = select_backend(DaemonBackend::Auto, missing, mock);
        assert_eq!(fallback.kind(), BackendKind::Mock);
        assert!(fallback.snapshot().available);

        let broken_nm = select_backend(DaemonBackend::Auto, broken, mock);
        assert_eq!(broken_nm.kind(), BackendKind::Unavailable);
        assert!(!broken_nm.snapshot().available);
        assert!(
            broken_nm
                .snapshot()
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("present but unusable")),
            "a present-but-broken NetworkManager must not start a second manager"
        );
    }

    #[test]
    fn explicit_backends_never_fall_back() {
        let nm = select_backend(DaemonBackend::NetworkManager, missing, mock);
        assert_eq!(nm.kind(), BackendKind::Unavailable);
        assert!(!nm.snapshot().available);

        let iwd = select_backend(DaemonBackend::Iwd, mock, missing);
        assert_eq!(iwd.kind(), BackendKind::Unavailable);
        assert!(!iwd.snapshot().available);
    }

    #[test]
    fn a_backend_name_parses_to_one_choice() {
        assert_eq!(
            "auto".parse::<DaemonBackend>().unwrap(),
            DaemonBackend::Auto
        );
        assert_eq!(
            "networkmanager".parse::<DaemonBackend>().unwrap(),
            DaemonBackend::NetworkManager
        );
        assert_eq!(
            "network-manager".parse::<DaemonBackend>().unwrap(),
            DaemonBackend::NetworkManager
        );
        assert_eq!("iwd".parse::<DaemonBackend>().unwrap(), DaemonBackend::Iwd);
        assert_eq!(
            "mock".parse::<DaemonBackend>().unwrap(),
            DaemonBackend::Mock
        );
        assert!("NetworkManager".parse::<DaemonBackend>().is_err());
    }
}
