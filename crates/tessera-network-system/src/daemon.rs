//! The out-of-process network service (ADR-0242).
//!
//! One long-lived process owns the system D-Bus connection, a backend, and a
//! neutral [`NetworkSnapshot`]. The compositor connects to a per-user Unix
//! socket, subscribes, and sends verbs. The wire format is the 4-byte
//! big-endian length + JSON framing in [`tessera_network::protocol`].
//!
//! The service never fabricates a reading: when no backend answers, it publishes
//! `available = false` with empty state (`[INV-ARCH-82]`, ADR-0190).

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tessera_network::protocol::{Event, PROTOCOL_VERSION, Request, read_frame, write_frame};
use tessera_network::{BackendKind, NetworkBackend, NetworkSnapshot};

use crate::selection::{DaemonBackend, open_backend};

const IDLE_CADENCE: Duration = Duration::from_millis(250);
/// Backoff before re-running backend selection when no service answered.
const BACKEND_RETRY: Duration = Duration::from_secs(10);
/// Backoff before asking a live backend that stopped answering to refresh.
/// A daemon restart (iwd) or a bus reconnect must not need a new session.
const LIVENESS_RETRY: Duration = Duration::from_secs(5);
const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// Service configuration.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    pub socket: PathBuf,
    pub backend: DaemonBackend,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            socket: tessera_network::protocol::default_socket_path(),
            backend: DaemonBackend::Auto,
        }
    }
}

struct DaemonState {
    status: NetworkSnapshot,
    subscribers: Vec<Sender<Event>>,
}

/// Run the service until the socket is closed or a client sends `Shutdown`.
pub fn run(options: DaemonOptions) -> Result<(), String> {
    let shutdown = Arc::new(AtomicBool::new(false));
    tessera_env::signals::install_shutdown_flag_into(&shutdown)
        .map_err(|error| format!("could not install signal handlers: {error}"))?;
    let listener = bind_socket(&options.socket)
        .map_err(|error| format!("could not bind {}: {error}", options.socket.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("could not configure {}: {error}", options.socket.display()))?;

    let backend = open_backend(options.backend);
    let initial = backend.snapshot();
    if !initial.available {
        log::info!(
            "network service: unavailable: {}",
            initial
                .last_error
                .as_deref()
                .unwrap_or("no network service is available")
        );
    }
    let state = Arc::new(Mutex::new(DaemonState {
        status: initial,
        subscribers: Vec::new(),
    }));
    let (commands, worker) = spawn_worker(options.backend, backend, state.clone())
        .map_err(|error| format!("could not start the network worker: {error}"))?;

    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = state.clone();
                let commands = commands.clone();
                let shutdown = shutdown.clone();
                thread::Builder::new()
                    .name("tessera-network-client".into())
                    .spawn(move || handle_client(stream, state, commands, shutdown))
                    .ok();
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                log::warn!("network service: accept failed: {error}");
                thread::sleep(ACCEPT_POLL);
            }
        }
    }

    drop(commands);
    let _ = worker.join();
    let _ = std::fs::remove_file(&options.socket);
    Ok(())
}

fn spawn_worker(
    choice: DaemonBackend,
    mut backend: Box<dyn NetworkBackend>,
    state: Arc<Mutex<DaemonState>>,
) -> io::Result<(Sender<Request>, thread::JoinHandle<()>)> {
    let (commands, receiver) = mpsc::channel::<Request>();
    let handle = thread::Builder::new()
        .name("tessera-network".into())
        .spawn(move || {
            let mut reopen_at = (backend.kind() == BackendKind::Unavailable)
                .then(|| Instant::now() + BACKEND_RETRY);
            let mut refresh_at: Option<Instant> = None;
            loop {
                let mut changed = false;
                match receiver.recv_timeout(IDLE_CADENCE) {
                    Ok(Request::Shutdown) => break,
                    Ok(request) => {
                        changed = true;
                        if let Err(error) = apply(&*backend, &request) {
                            broadcast_error(&state, error.to_string());
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }

                // A session can start before NetworkManager/iwd or before a
                // USB Wi-Fi adapter appears. Only an inert unavailable
                // backend is reopened: a live backend owns signal threads and
                // retries its own reads.
                if backend.kind() == BackendKind::Unavailable
                    && reopen_at.is_none_or(|deadline| Instant::now() >= deadline)
                {
                    log::debug!("network service: retrying backend selection");
                    backend = open_backend(choice);
                    reopen_at = (backend.kind() == BackendKind::Unavailable)
                        .then(|| Instant::now() + BACKEND_RETRY);
                    changed = true;
                }

                changed = backend.poll() || changed;
                // A live backend can outlive the daemon it talks to (iwd
                // restarts, NetworkManager reconnects). `poll` only reacts to
                // signals or an in-flight transition; without a liveness
                // retry an iwd restart would leave the service unavailable
                // until the next session.
                let available = backend.snapshot().available;
                if available {
                    refresh_at = None;
                } else if backend.kind() != BackendKind::Unavailable
                    && refresh_at.is_none_or(|deadline| Instant::now() >= deadline)
                {
                    log::debug!("network service: asking a stopped backend to refresh");
                    let _ = backend.refresh();
                    refresh_at = Some(Instant::now() + LIVENESS_RETRY);
                    changed = true;
                }
                if changed {
                    publish(&state, backend.snapshot());
                }
            }
            publish(&state, backend.snapshot());
        })?;
    Ok((commands, handle))
}

fn apply(
    backend: &dyn NetworkBackend,
    request: &Request,
) -> Result<(), tessera_network::NetworkError> {
    match request {
        Request::ScanWifi => backend.scan(),
        Request::ConnectWifi { ssid, passphrase } => backend.connect(ssid, passphrase.as_deref()),
        Request::DisconnectWifi => backend.disconnect(),
        Request::SetWifiEnabled { enabled } => backend.set_enabled(*enabled),
        Request::ForgetWifi { ssid } => backend.forget(ssid),
        Request::SetWifiAutoConnect { ssid, auto_connect } => {
            backend.set_auto_connect(ssid, *auto_connect)
        }
        Request::Refresh => backend.refresh(),
        // Handled by the client or the worker loop itself.
        Request::Hello { .. } | Request::Subscribe | Request::Shutdown => Ok(()),
    }
}

fn publish(state: &Arc<Mutex<DaemonState>>, status: NetworkSnapshot) {
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    state.status = status.clone();
    state.subscribers.retain(|subscriber| {
        subscriber
            .send(Event::Status {
                status: status.clone(),
            })
            .is_ok()
    });
}

fn broadcast_error(state: &Arc<Mutex<DaemonState>>, message: String) {
    log::warn!("network service command failed: {message}");
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    state.subscribers.retain(|subscriber| {
        subscriber
            .send(Event::Error {
                message: message.clone(),
            })
            .is_ok()
    });
}

fn handle_client(
    stream: UnixStream,
    state: Arc<Mutex<DaemonState>>,
    commands: Sender<Request>,
    shutdown: Arc<AtomicBool>,
) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (events, receiver) = mpsc::channel::<Event>();
    let writer_thread = thread::Builder::new()
        .name("tessera-network-writer".into())
        .spawn(move || {
            for event in receiver {
                if write_frame(&mut writer, &event).is_err() {
                    break;
                }
            }
        })
        .ok();

    let mut reader = stream;
    let mut negotiated = false;
    while let Ok(Some(request)) = read_frame::<_, Request>(&mut reader) {
        match request {
            Request::Hello { version } => {
                let (backend, available) = {
                    let state = state.lock().unwrap_or_else(|error| error.into_inner());
                    (state.status.backend, state.status.available)
                };
                let _ = events.send(Event::HelloAck {
                    version: PROTOCOL_VERSION,
                    backend,
                    available,
                });
                negotiated = version == PROTOCOL_VERSION;
                if !negotiated {
                    let _ = events.send(Event::Error {
                        message: format!(
                            "protocol version {version} does not match this service ({PROTOCOL_VERSION})"
                        ),
                    });
                    break;
                }
            }
            Request::Subscribe => {
                if !negotiated {
                    let _ = events.send(Event::Error {
                        message: "the network service requires a matching Hello first".to_string(),
                    });
                    break;
                }
                let status = {
                    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                    let status = state.status.clone();
                    state.subscribers.retain(|subscriber| {
                        subscriber
                            .send(Event::Status {
                                status: status.clone(),
                            })
                            .is_ok()
                    });
                    state.subscribers.push(events.clone());
                    status
                };
                let _ = events.send(Event::Status { status });
            }
            Request::Shutdown => {
                shutdown.store(true, Ordering::Relaxed);
                let _ = commands.send(Request::Shutdown);
                break;
            }
            other => {
                if !negotiated {
                    let _ = events.send(Event::Error {
                        message: "the network service requires a matching Hello first".to_string(),
                    });
                    break;
                }
                if commands.send(other).is_err() {
                    break;
                }
            }
        }
    }

    // Drop this connection's subscription; the writer ends when the channel
    // closes.
    {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        let status = state.status.clone();
        state.subscribers.retain(|subscriber| {
            subscriber
                .send(Event::Status {
                    status: status.clone(),
                })
                .is_ok()
        });
    }
    drop(events);
    if let Some(handle) = writer_thread {
        let _ = handle.join();
    }
}

fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let metadata = std::fs::symlink_metadata(parent)?;
        // SAFETY: `getuid` has no preconditions and cannot fail.
        let own_uid = unsafe { libc::getuid() };
        if !metadata.file_type().is_dir()
            || metadata.uid() != own_uid
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing a network socket in {}: parent must be an owner-only directory",
                    parent.display()
                ),
            ));
        }
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing to replace non-socket {}", path.display()),
            ));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another network service is listening on {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::networkmanager::NetworkManagerBackend;
    use std::time::Instant;

    #[test]
    fn a_non_socket_at_the_path_is_never_replaced() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("tessera-networkd.sock");
        std::fs::write(&path, b"not a socket").expect("write");
        let error = bind_socket(&path).expect_err("must refuse");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).expect("read"), b"not a socket");
    }

    #[test]
    fn a_group_or_world_writable_parent_is_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777))
            .expect("chmod");
        let path = directory.path().join("tessera-networkd.sock");
        let error = bind_socket(&path).expect_err("must refuse a shared parent");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_protocol_mismatch_is_refused_before_subscribe() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("tessera-networkd.sock");
        let socket = path.clone();
        let service = thread::spawn(move || {
            run(DaemonOptions {
                socket,
                backend: DaemonBackend::Mock,
            })
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            if let Ok(stream) = UnixStream::connect(&path) {
                break stream;
            }
            assert!(Instant::now() < deadline, "service did not bind in time");
            thread::sleep(Duration::from_millis(20));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        write_frame(
            &mut stream,
            &Request::Hello {
                version: PROTOCOL_VERSION + 1,
            },
        )
        .expect("hello");
        let hello: Option<Event> = read_frame(&mut stream).expect("hello ack");
        assert!(matches!(hello, Some(Event::HelloAck { .. })));
        let error: Option<Event> = read_frame(&mut stream).expect("mismatch error");
        assert!(matches!(error, Some(Event::Error { .. })));
        drop(stream);

        // A fresh connection may still ask the service to stop.
        let mut shutdown = UnixStream::connect(&path).expect("shutdown connection");
        write_frame(&mut shutdown, &Request::Shutdown).expect("shutdown");
        drop(shutdown);
        let result = service.join().expect("service thread");
        assert!(result.is_ok(), "service returned {result:?}");
    }

    #[test]
    fn the_service_publishes_a_neutral_status_and_stops_on_shutdown() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("tessera-networkd.sock");
        let socket = path.clone();
        let service = thread::spawn(move || {
            run(DaemonOptions {
                socket,
                backend: DaemonBackend::Mock,
            })
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            if let Ok(stream) = UnixStream::connect(&path) {
                break stream;
            }
            assert!(Instant::now() < deadline, "service did not bind in time");
            thread::sleep(Duration::from_millis(20));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        write_frame(
            &mut stream,
            &Request::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .expect("hello");
        let hello: Option<Event> = read_frame(&mut stream).expect("hello ack");
        assert!(matches!(
            hello,
            Some(Event::HelloAck {
                backend: tessera_network::BackendKind::Mock,
                available: true,
                ..
            })
        ));

        write_frame(&mut stream, &Request::Subscribe).expect("subscribe");
        let status: Option<Event> = read_frame(&mut stream).expect("status");
        assert!(matches!(
            status,
            Some(Event::Status { status }) if status.available && status.backend == tessera_network::BackendKind::Mock
        ));

        write_frame(&mut stream, &Request::Shutdown).expect("shutdown");
        drop(stream);
        let result = service.join().expect("service thread");
        assert!(result.is_ok(), "service returned {result:?}");
        assert!(!path.exists(), "the socket is removed on shutdown");
    }

    /// A live backend whose daemon was restarted: `poll` sees no signal, so
    /// only the service's liveness retry can recover it.
    struct RecoveringBackend {
        available: AtomicBool,
    }

    impl NetworkBackend for RecoveringBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Mock
        }

        fn snapshot(&self) -> NetworkSnapshot {
            if self.available.load(Ordering::Relaxed) {
                NetworkSnapshot {
                    available: true,
                    backend: BackendKind::Mock,
                    ..NetworkSnapshot::default()
                }
            } else {
                NetworkSnapshot::unavailable(BackendKind::Mock, "the daemon restarted")
            }
        }

        fn scan(&self) -> Result<(), tessera_network::NetworkError> {
            Ok(())
        }

        fn connect(
            &self,
            _ssid: &str,
            _passphrase: Option<&str>,
        ) -> Result<(), tessera_network::NetworkError> {
            Ok(())
        }

        fn disconnect(&self) -> Result<(), tessera_network::NetworkError> {
            Ok(())
        }

        fn set_enabled(&self, _enabled: bool) -> Result<(), tessera_network::NetworkError> {
            Ok(())
        }

        fn forget(&self, _ssid: &str) -> Result<(), tessera_network::NetworkError> {
            Ok(())
        }

        fn set_auto_connect(
            &self,
            _ssid: &str,
            _auto_connect: bool,
        ) -> Result<(), tessera_network::NetworkError> {
            Ok(())
        }

        fn refresh(&self) -> Result<(), tessera_network::NetworkError> {
            self.available.store(true, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn a_live_backend_that_stopped_answering_is_retried_without_a_new_session() {
        let state = Arc::new(Mutex::new(DaemonState {
            status: NetworkSnapshot::unavailable(BackendKind::Mock, "the daemon restarted"),
            subscribers: Vec::new(),
        }));
        let backend = Box::new(RecoveringBackend {
            available: AtomicBool::new(false),
        });
        let (commands, worker) =
            spawn_worker(DaemonBackend::Mock, backend, state.clone()).expect("worker");

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let available = state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status
                .available;
            if available {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the liveness retry never refreshed the backend"
            );
            thread::sleep(Duration::from_millis(20));
        }

        commands.send(Request::Shutdown).expect("shutdown");
        worker.join().expect("worker");
    }

    #[test]
    #[ignore = "requires a running NetworkManager with a Wi-Fi device"]
    fn live_networkmanager_service_publishes_a_real_snapshot() {
        // Direct backend first: a failure here names the bus problem instead
        // of surfacing as an unavailable daemon snapshot.
        let direct = NetworkManagerBackend::open();
        eprintln!(
            "direct NetworkManager open error: {:?}",
            direct.as_ref().err()
        );
        let direct = direct.expect("NetworkManager backend");
        eprintln!("direct snapshot: {:?}", direct.snapshot());
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("tessera-networkd.sock");
        let socket = path.clone();
        let service = thread::spawn(move || {
            run(DaemonOptions {
                socket,
                backend: DaemonBackend::NetworkManager,
            })
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            if let Ok(stream) = UnixStream::connect(&path) {
                break stream;
            }
            assert!(Instant::now() < deadline, "service did not bind in time");
            thread::sleep(Duration::from_millis(20));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        write_frame(
            &mut stream,
            &Request::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .expect("hello");
        let hello: Option<Event> = read_frame(&mut stream).expect("hello ack");
        assert!(
            matches!(
                hello,
                Some(Event::HelloAck {
                    backend: BackendKind::NetworkManager,
                    available: true,
                    ..
                })
            ),
            "live NetworkManager did not answer: {hello:?}"
        );

        write_frame(&mut stream, &Request::Subscribe).expect("subscribe");
        let status: Option<Event> = read_frame(&mut stream).expect("status");
        let Some(Event::Status { status }) = status else {
            panic!("expected a status event, got {status:?}");
        };
        assert!(status.available, "live snapshot unavailable: {status:?}");
        assert_eq!(status.backend, BackendKind::NetworkManager);
        assert!(status.wifi_enabled.is_some());
        eprintln!(
            "live NetworkManager snapshot: state={:?} ssid={:?} networks={}",
            status.wifi_state,
            status.wifi_ssid,
            status.wifi_networks.len()
        );

        // Exercise a real RequestScan through the service. The result is
        // reported as a status or error event; both paths cross the socket.
        write_frame(&mut stream, &Request::ScanWifi).expect("scan");
        write_frame(&mut stream, &Request::Shutdown).expect("shutdown");
        drop(stream);
        let result = service.join().expect("service thread");
        assert!(result.is_ok(), "service returned {result:?}");
        assert!(!path.exists(), "the socket is removed on shutdown");
    }
}
