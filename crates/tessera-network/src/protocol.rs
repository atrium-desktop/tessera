//! The per-user Unix-socket protocol between the compositor and the network
//! service (ADR-0242).
//!
//! One framed JSON request/response stream: a 4-byte big-endian length followed
//! by a JSON body, with a 1 MiB ceiling. The socket lives in
//! `$XDG_RUNTIME_DIR` (mode 0600) so the filesystem is the credential; the
//! protocol grants nothing the same user could not ask NetworkManager for
//! directly.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::backend::{BackendKind, NetworkSnapshot};

/// Bumped whenever [`Request`] or [`Event`] changes shape.
pub const PROTOCOL_VERSION: u32 = 1;

/// A command from the compositor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Hello {
        version: u32,
    },
    Subscribe,
    ScanWifi,
    ConnectWifi {
        ssid: String,
        passphrase: Option<String>,
    },
    DisconnectWifi,
    SetWifiEnabled {
        enabled: bool,
    },
    ForgetWifi {
        ssid: String,
    },
    SetWifiAutoConnect {
        ssid: String,
        auto_connect: bool,
    },
    Refresh,
    Shutdown,
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Request::ConnectWifi {
            passphrase: Some(passphrase),
            ..
        } = self
        {
            passphrase.zeroize();
        }
    }
}

/// A push from the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    HelloAck {
        version: u32,
        backend: BackendKind,
        available: bool,
    },
    Status {
        status: NetworkSnapshot,
    },
    Error {
        message: String,
    },
}

/// `$XDG_RUNTIME_DIR/tessera-networkd.sock`.
///
/// Without a runtime directory, the socket lives in a per-user directory under
/// the temporary directory rather than directly in `/tmp`: a predictable
/// shared path would let another user pre-bind the socket and receive
/// passphrases. The service also refuses a parent it does not own or one that
/// is group/world-writable.
pub fn default_socket_path() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("tessera-networkd.sock");
    }
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir()
        .join(format!("tessera-network-{uid}"))
        .join("tessera-networkd.sock")
}

/// Write one length-prefixed JSON frame.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

/// Read one length-prefixed JSON frame. `Ok(None)` is a clean EOF.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length > (1 << 20) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds the 1 MiB limit",
        ));
    }
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::system::{NetworkState, WifiLinkState, WifiNetwork, WifiSecurity};

    #[test]
    fn requests_and_events_round_trip_across_every_shape() {
        let requests = [
            Request::Hello {
                version: PROTOCOL_VERSION,
            },
            Request::Subscribe,
            Request::ScanWifi,
            Request::ConnectWifi {
                ssid: "Home-WiFi-5G".to_owned(),
                passphrase: Some("secret".to_owned()),
            },
            Request::DisconnectWifi,
            Request::SetWifiEnabled { enabled: false },
            Request::ForgetWifi {
                ssid: "Home-WiFi-5G".to_owned(),
            },
            Request::SetWifiAutoConnect {
                ssid: "Home-WiFi-5G".to_owned(),
                auto_connect: true,
            },
            Request::Refresh,
            Request::Shutdown,
        ];
        for request in requests {
            let mut buffer = Vec::new();
            write_frame(&mut buffer, &request).expect("write");
            let decoded: Option<Request> =
                read_frame(&mut std::io::Cursor::new(buffer)).expect("read");
            assert_eq!(decoded, Some(request));
        }

        let events = [
            Event::HelloAck {
                version: PROTOCOL_VERSION,
                backend: BackendKind::NetworkManager,
                available: true,
            },
            Event::Status {
                status: NetworkSnapshot {
                    available: true,
                    backend: BackendKind::NetworkManager,
                    network: NetworkState::Wifi,
                    network_authoritative: true,
                    network_interface: "wlan0".to_owned(),
                    wifi_enabled: Some(true),
                    wifi_state: WifiLinkState::Connected,
                    wifi_ssid: Some("Home-WiFi-5G".to_owned()),
                    wifi_networks: vec![WifiNetwork {
                        ssid: "Home-WiFi-5G".to_owned(),
                        signal_bars: 4,
                        security: WifiSecurity::WpaPsk,
                        is_connected: true,
                        is_saved: true,
                        auto_connect: true,
                        frequency_mhz: Some(5180),
                    }],
                    last_error: None,
                },
            },
            Event::Error {
                message: "NetworkManager is not running".to_owned(),
            },
        ];
        for event in events {
            let mut buffer = Vec::new();
            write_frame(&mut buffer, &event).expect("write");
            let decoded: Option<Event> =
                read_frame(&mut std::io::Cursor::new(buffer)).expect("read");
            assert_eq!(decoded, Some(event));
        }
    }

    #[test]
    fn a_clean_eof_is_none_and_an_oversized_frame_is_refused() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let decoded: Option<Request> = read_frame(&mut cursor).expect("clean EOF");
        assert_eq!(decoded, None);

        let mut cursor = std::io::Cursor::new(vec![0x00, 0x20, 0x00, 0x00]);
        let error = read_frame::<_, Request>(&mut cursor).expect_err("must refuse");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_default_socket_lives_in_the_runtime_directory() {
        // SAFETY: tests in this module do not rely on a stable XDG_RUNTIME_DIR
        // across threads; this assertion only checks the suffix contract.
        assert_eq!(
            default_socket_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("tessera-networkd.sock")
        );
    }
}
