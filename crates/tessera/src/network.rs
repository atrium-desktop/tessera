//! Client and supervisor for the out-of-process network service (ADR-0242).
//!
//! The compositor never opens NetworkManager or iwd. It owns one supervised
//! `tessera-networkd` process, speaks its control protocol over a per-user Unix
//! socket, and publishes the service's backend-neutral [`NetworkSnapshot`].
//! Chrome therefore renders the service's facts, not a local guess.
//!
//! This module deliberately has no D-Bus dependency: the only system service
//! connection in the tree is in `tessera-network-system`, which is a sibling
//! process and never a library of the compositor.

use std::net::Shutdown;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tessera_network::protocol::{self, Event, Request};
use tessera_network::{BackendChoice, BackendKind, NetworkSnapshot};

/// How long the client waits before trying a missing service again.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// Backoff after the daemon exits on its own.
const RESTART_BACKOFF: Duration = Duration::from_secs(2);

/// The backend the compositor asks the service to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkServiceBackend {
    /// The persistent `[network] backend` choice.
    Config(BackendChoice),
    /// The deterministic preview/test backend. Never reachable from TOML.
    Mock,
}

impl NetworkServiceBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config(choice) => choice.as_str(),
            Self::Mock => "mock",
        }
    }
}

impl Default for NetworkServiceBackend {
    fn default() -> Self {
        Self::Config(BackendChoice::Auto)
    }
}

/// The `[network]` configuration handed to the service on spawn.
#[derive(Debug, Clone, Default)]
pub struct NetworkServiceOptions {
    pub backend: NetworkServiceBackend,
}

/// Non-blocking handle to the network service.
pub struct NetworkHandle {
    commands: mpsc::Sender<Request>,
    snapshot: Arc<Mutex<NetworkSnapshot>>,
}

impl NetworkHandle {
    /// Start the service and its client from the `[network]` configuration.
    pub fn spawn_auto(options: NetworkServiceOptions) -> Self {
        let socket = protocol::default_socket_path();
        let snapshot = Arc::new(Mutex::new(NetworkSnapshot::unavailable(
            BackendKind::Unavailable,
            "the network service is not reachable",
        )));
        let (commands, command_rx) = mpsc::channel();
        let thread_snapshot = snapshot.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("tessera-network".into())
            .spawn(move || {
                // Background network client: de-prioritized off the frame CPU.
                crate::runtime::sched::mark_background();
                run_client(socket, options, thread_snapshot, command_rx);
            })
        {
            log::error!("failed to spawn the network service client: {error}");
        }
        Self { commands, snapshot }
    }

    /// The latest service snapshot, without blocking.
    pub fn snapshot(&self) -> NetworkSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }

    pub fn request_scan(&self) {
        let _ = self.commands.send(Request::ScanWifi);
    }

    pub fn connect(&self, ssid: String, passphrase: Option<String>) {
        let _ = self
            .commands
            .send(Request::ConnectWifi { ssid, passphrase });
    }

    pub fn disconnect(&self) {
        let _ = self.commands.send(Request::DisconnectWifi);
    }

    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.commands.send(Request::SetWifiEnabled { enabled });
    }

    pub fn forget(&self, ssid: String) {
        let _ = self.commands.send(Request::ForgetWifi { ssid });
    }

    pub fn set_auto_connect(&self, ssid: String, auto_connect: bool) {
        let _ = self
            .commands
            .send(Request::SetWifiAutoConnect { ssid, auto_connect });
    }

    pub fn request_refresh(&self) {
        let _ = self.commands.send(Request::Refresh);
    }
}

fn run_client(
    socket: PathBuf,
    options: NetworkServiceOptions,
    snapshot: Arc<Mutex<NetworkSnapshot>>,
    commands: Receiver<Request>,
) {
    let mut child: Option<Child> = None;
    let mut next_spawn = Instant::now();
    loop {
        if child
            .as_mut()
            .and_then(|child| child.try_wait().ok())
            .flatten()
            .is_some()
        {
            child = None;
            next_spawn = Instant::now() + RESTART_BACKOFF;
        }
        if child.is_none() && Instant::now() >= next_spawn {
            child = spawn_daemon(&socket, &options);
            if child.is_none() {
                next_spawn = Instant::now() + RESTART_BACKOFF;
            }
        }

        // Never send a Wi-Fi passphrase to a socket in a directory another
        // user can write. The daemon refuses such a directory too, but a
        // pre-bound socket could exist before it starts.
        if !socket_parent_is_private(&socket) {
            discard_commands(&commands);
            set_unavailable(
                &snapshot,
                "the network service socket directory is not private",
            );
            std::thread::sleep(RECONNECT_DELAY);
            continue;
        }

        let Ok(stream) = UnixStream::connect(&socket) else {
            discard_commands(&commands);
            set_unavailable(&snapshot, "the network service is not reachable");
            std::thread::sleep(RECONNECT_DELAY);
            continue;
        };

        let Ok(mut writer) = stream.try_clone() else {
            discard_commands(&commands);
            std::thread::sleep(RECONNECT_DELAY);
            continue;
        };
        let Ok(shutdown_handle) = stream.try_clone() else {
            discard_commands(&commands);
            std::thread::sleep(RECONNECT_DELAY);
            continue;
        };
        let (events_tx, events_rx) = mpsc::channel();
        let mut reader = stream;
        let reader_thread = match std::thread::Builder::new()
            .name("tessera-network-reader".into())
            .spawn(move || {
                crate::runtime::sched::mark_background();
                while let Ok(Some(event)) = protocol::read_frame::<_, Event>(&mut reader) {
                    if events_tx.send(event).is_err() {
                        break;
                    }
                }
            }) {
            Ok(handle) => handle,
            Err(error) => {
                log::warn!("network service: could not start the event reader: {error}");
                discard_commands(&commands);
                let _ = shutdown_handle.shutdown(Shutdown::Both);
                std::thread::sleep(RECONNECT_DELAY);
                continue;
            }
        };
        let _ = protocol::write_frame(
            &mut writer,
            &Request::Hello {
                version: protocol::PROTOCOL_VERSION,
            },
        );
        let _ = protocol::write_frame(&mut writer, &Request::Subscribe);

        let mut connection_error = "the network service is not reachable".to_string();
        loop {
            match events_rx.try_recv() {
                Ok(Event::HelloAck { version, .. }) if version != protocol::PROTOCOL_VERSION => {
                    connection_error = format!(
                        "the network service speaks protocol version {version}; \
                         this client speaks {}",
                        protocol::PROTOCOL_VERSION
                    );
                    break;
                }
                Ok(event) => apply_event(&snapshot, event),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }

            match commands.recv_timeout(Duration::from_millis(100)) {
                Ok(request) => {
                    if protocol::write_frame(&mut writer, &request).is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = protocol::write_frame(&mut writer, &Request::Shutdown);
                    let _ = shutdown_handle.shutdown(Shutdown::Both);
                    let _ = reader_thread.join();
                    if let Some(mut child) = child.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    return;
                }
            }
        }

        let _ = shutdown_handle.shutdown(Shutdown::Both);
        let _ = reader_thread.join();
        // A command queued while the service was down must not be replayed
        // later: reconnecting with a stale passphrase would connect to a
        // network the user has since moved away from.
        discard_commands(&commands);
        set_unavailable(&snapshot, &connection_error);
        std::thread::sleep(RECONNECT_DELAY);
    }
}

/// Drop commands that were queued while no service was connected.
///
/// `Request`'s `Drop` zeroizes a staged passphrase; keeping the request in the
/// channel would either replay it on reconnect or grow without bound.
fn discard_commands(commands: &Receiver<Request>) {
    while commands.try_recv().is_ok() {}
}

/// Whether the socket's parent directory is an owner-only directory.
///
/// The daemon applies the same rule before binding. The client checks it too
/// because a compositor must never send a passphrase through a path an
/// attacker could have prepared first.
fn socket_parent_is_private(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(metadata) = std::fs::symlink_metadata(parent) else {
        return false;
    };
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let own_uid = unsafe { libc::getuid() };
    metadata.file_type().is_dir()
        && metadata.uid() == own_uid
        && metadata.permissions().mode() & 0o022 == 0
}

fn set_unavailable(snapshot: &Arc<Mutex<NetworkSnapshot>>, reason: &str) {
    let mut snapshot = snapshot.lock().unwrap_or_else(|error| error.into_inner());
    *snapshot = NetworkSnapshot::unavailable(BackendKind::Unavailable, reason);
}

fn spawn_daemon(socket: &Path, options: &NetworkServiceOptions) -> Option<Child> {
    let program = crate::runtime::trusted_sibling_program("tessera-networkd").ok()?;
    Command::new(program)
        .arg("--socket")
        .arg(socket)
        .arg("--backend")
        .arg(options.backend.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Keep the service's tracing output on the compositor's stderr: under
        // systemd that is the journal, and losing it would make an unavailable
        // backend indistinguishable from a broken one.
        .stderr(Stdio::inherit())
        .spawn()
        .ok()
}

fn apply_event(snapshot: &Arc<Mutex<NetworkSnapshot>>, event: Event) {
    let mut snapshot = snapshot.lock().unwrap_or_else(|error| error.into_inner());
    match event {
        Event::Status { status } => *snapshot = status,
        Event::HelloAck { .. } => {}
        Event::Error { message } => {
            log::warn!("network service: {message}");
            snapshot.last_error = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::system::{NetworkState, WifiLinkState};

    #[test]
    fn a_status_event_replaces_the_whole_snapshot() {
        let snapshot = Arc::new(Mutex::new(NetworkSnapshot::unavailable(
            BackendKind::Unavailable,
            "not reachable",
        )));
        apply_event(
            &snapshot,
            Event::Status {
                status: NetworkSnapshot {
                    available: true,
                    backend: BackendKind::NetworkManager,
                    network: NetworkState::Wifi,
                    network_interface: "wlan0".to_owned(),
                    wifi_enabled: Some(true),
                    wifi_state: WifiLinkState::Connected,
                    wifi_ssid: Some("Home-WiFi-5G".to_owned()),
                    ..NetworkSnapshot::default()
                },
            },
        );
        let snapshot = snapshot.lock().expect("snapshot").clone();
        assert!(snapshot.available);
        assert_eq!(snapshot.backend, BackendKind::NetworkManager);
        assert_eq!(snapshot.wifi_ssid.as_deref(), Some("Home-WiFi-5G"));
        assert_eq!(snapshot.last_error, None);
    }

    #[test]
    fn an_error_event_is_named_without_fabricating_availability() {
        let snapshot = Arc::new(Mutex::new(NetworkSnapshot::unavailable(
            BackendKind::Unavailable,
            "not reachable",
        )));
        apply_event(
            &snapshot,
            Event::Error {
                message: "access denied".to_owned(),
            },
        );
        let snapshot = snapshot.lock().expect("snapshot").clone();
        assert!(!snapshot.available);
        assert_eq!(snapshot.last_error.as_deref(), Some("access denied"));
    }

    #[test]
    fn a_shared_socket_directory_is_not_trusted() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(socket_parent_is_private(
            &directory.path().join("tessera-networkd.sock")
        ));
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777))
            .expect("chmod");
        assert!(!socket_parent_is_private(
            &directory.path().join("tessera-networkd.sock")
        ));
    }

    #[test]
    fn commands_queued_while_disconnected_are_discarded() {
        let (sender, receiver) = mpsc::channel();
        sender.send(Request::ScanWifi).expect("scan");
        sender
            .send(Request::ConnectWifi {
                ssid: "Home-WiFi-5G".to_owned(),
                passphrase: Some("stale secret".to_owned()),
            })
            .expect("connect");
        discard_commands(&receiver);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn the_service_backend_name_is_the_protocol_name() {
        assert_eq!(
            NetworkServiceBackend::Config(BackendChoice::NetworkManager).as_str(),
            "networkmanager"
        );
        assert_eq!(
            NetworkServiceBackend::Config(BackendChoice::Iwd).as_str(),
            "iwd"
        );
        assert_eq!(NetworkServiceBackend::Mock.as_str(), "mock");
    }
}
