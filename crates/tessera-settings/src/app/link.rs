//! The compositor link: the only way Settings reaches the compositor.
//!
//! Two threads, each owning one IPC connection, keep blocking socket I/O off
//! the render thread:
//!
//! - the **command** connection (`query` + `session`) fetches snapshots and
//!   commits typed `SettingsAction`s, renewing its privileged lease while idle;
//! - the **watch** connection (`query`) subscribes to compositor events and
//!   turns every `SettingsChanged` into a refresh on the command connection,
//!   so edits made elsewhere (the CLI, the Control Center, another client)
//!   reach the open window.
//!
//! Both reconnect with a bounded backoff. Outcomes reach the UI through a
//! calloop channel, which wakes the event loop.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use tessera_desktop::settings::{SettingsAction, SettingsSnapshot};
use tessera_ipc::{Client, is_transport_failure};
use tessera_protocol::{ConnectionCapabilities, Event};
use tessera_wl_client::WakeSender as LoopSender;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// How long the command thread waits for work when no lease needs renewing.
const IDLE_WAKE: Duration = Duration::from_secs(30);

/// What the link reports to the window.
#[derive(Debug)]
pub enum LinkEvent {
    Snapshot(SettingsSnapshot),
    Rejected(String),
    Lost(String),
    /// A connection was established. `read_only` carries the reason when the
    /// compositor refused this executable the Settings scope: settings stay
    /// readable, and every change is refused with that reason.
    Linked {
        read_only: Option<String>,
    },
}

enum Request {
    Apply {
        expected_revision: Option<u64>,
        action: SettingsAction,
    },
    Refresh,
    /// Raise the running Settings window after a second invocation handed
    /// over a page (or just asked to show the window).
    FocusSelf,
}

/// Handle the window keeps to submit work to the command connection.
pub struct Link {
    requests: mpsc::Sender<Request>,
}

impl Link {
    pub fn spawn(socket: PathBuf, events: LoopSender<LinkEvent>) -> io::Result<Link> {
        let (requests, inbox) = mpsc::channel();
        let command_socket = socket.clone();
        thread::Builder::new()
            .name("settings-command".into())
            .spawn(move || command_loop(&command_socket, &inbox, &events))?;
        let refresh = requests.clone();
        thread::Builder::new()
            .name("settings-watch".into())
            .spawn(move || watch_loop(&socket, &refresh))?;
        Ok(Link { requests })
    }

    pub fn apply(&self, expected_revision: Option<u64>, action: SettingsAction) {
        let _ = self.requests.send(Request::Apply {
            expected_revision,
            action,
        });
    }

    pub fn focus_self(&self) {
        let _ = self.requests.send(Request::FocusSelf);
    }
}

/// Connect to the compositor. The command connection claims the built-in
/// Settings scope, which the compositor grants to an installed
/// `tessera-settings` executable (or its development sibling): under the
/// default agent lockdown an anonymous connection is query-only, and the
/// scope permits settings transactions plus focusing a Settings-owned
/// window. The watch connection needs no privilege.
fn connect(socket: &Path, session: bool) -> io::Result<Client> {
    let caps = ConnectionCapabilities {
        query: true,
        session,
        ..ConnectionCapabilities::default()
    };
    let client = if session {
        Client::connect_scoped_with_timeout(
            socket,
            caps,
            tessera_protocol::LOCAL_SETTINGS_SCOPE,
            HANDSHAKE_TIMEOUT,
        )?
    } else {
        Client::connect_with_timeout(socket, caps, HANDSHAKE_TIMEOUT)?
    };
    client.set_io_timeout(Some(REQUEST_TIMEOUT))?;
    if session && !client.caps().session {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the compositor did not grant the session capability",
        ));
    }
    Ok(client)
}

/// Exponential backoff that resets once a connection succeeds.
struct Backoff(Duration);

impl Backoff {
    fn new() -> Self {
        Backoff(BACKOFF_START)
    }

    fn wait(&mut self) {
        thread::sleep(self.0);
        self.0 = (self.0 * 2).min(BACKOFF_MAX);
    }

    fn reset(&mut self) {
        self.0 = BACKOFF_START;
    }
}

/// Whether a handshake failure is a standing refusal of privilege rather than
/// a transport problem worth retrying.
fn is_privilege_refusal(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || error.to_string().contains("not available to this process")
}

fn command_loop(socket: &Path, inbox: &mpsc::Receiver<Request>, events: &LoopSender<LinkEvent>) {
    let mut backoff = Backoff::new();
    loop {
        let mut read_only: Option<String> = None;
        let connected = match connect(socket, true) {
            Err(error) if is_privilege_refusal(&error) => {
                read_only = Some(error.to_string());
                connect(socket, false)
            }
            other => other,
        };
        let mut client = match connected.and_then(|mut client| {
            let snapshot = client.settings()?;
            Ok((client, snapshot))
        }) {
            Ok((client, snapshot)) => {
                let linked = LinkEvent::Linked {
                    read_only: read_only.clone(),
                };
                if events.send(linked).is_err()
                    || events.send(LinkEvent::Snapshot(snapshot)).is_err()
                {
                    return;
                }
                backoff.reset();
                client
            }
            Err(error) => {
                if events.send(LinkEvent::Lost(error.to_string())).is_err() {
                    return;
                }
                backoff.wait();
                continue;
            }
        };
        match serve(&mut client, read_only.as_deref(), inbox, events) {
            Served::WindowClosed => return,
            Served::Disconnected(error) => {
                if events.send(LinkEvent::Lost(error.to_string())).is_err() {
                    return;
                }
                backoff.wait();
            }
        }
    }
}

enum Served {
    WindowClosed,
    Disconnected(io::Error),
}

fn serve(
    client: &mut Client,
    read_only: Option<&str>,
    inbox: &mpsc::Receiver<Request>,
    events: &LoopSender<LinkEvent>,
) -> Served {
    let mut renew_at = lease_deadline(client);
    loop {
        let wait = renew_at.map_or(IDLE_WAKE, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        let request = match inbox.recv_timeout(wait) {
            Ok(request) => Some(request),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Served::WindowClosed,
        };
        if renew_at.is_some_and(|deadline| Instant::now() >= deadline) {
            let ttl = client.lease().map_or(0, |lease| lease.ttl_ms);
            if let Err(error) = client.renew_lease(ttl) {
                return Served::Disconnected(error);
            }
            renew_at = lease_deadline(client);
        }
        let outcome = match request {
            None => continue,
            Some(Request::Refresh) => refresh(client, events),
            Some(Request::Apply { .. }) if let Some(reason) = read_only => {
                let _ = events.send(LinkEvent::Rejected(reason.to_owned()));
                refresh(client, events)
            }
            Some(Request::FocusSelf) if read_only.is_some() => {
                log::debug!("settings: cannot raise the window without the Settings scope");
                Ok(())
            }
            Some(Request::FocusSelf) => match focus_self(client) {
                Ok(()) => Ok(()),
                Err(error) if is_transport_failure(&error) => Err(error),
                Err(error) => {
                    log::warn!("settings: could not raise the window: {error}");
                    Ok(())
                }
            },
            Some(Request::Apply {
                expected_revision,
                action,
            }) => match client.apply_settings(expected_revision, action) {
                Ok(_receipt) => refresh(client, events),
                Err(error) if is_transport_failure(&error) => Err(error),
                Err(refusal) => {
                    let _ = events.send(LinkEvent::Rejected(refusal.to_string()));
                    // The refused draft must give way to the authoritative state.
                    refresh(client, events)
                }
            },
        };
        if let Err(error) = outcome {
            return Served::Disconnected(error);
        }
    }
}

fn refresh(client: &mut Client, events: &LoopSender<LinkEvent>) -> io::Result<()> {
    let snapshot = client.settings()?;
    let _ = events.send(LinkEvent::Snapshot(snapshot));
    Ok(())
}

/// Focus and raise the live Settings toplevel. The window list is the only
/// stable handle IPC exposes; the app id distinguishes this instance from any
/// other window. The Settings scope permits this one command when it targets
/// a window the scope's owner owns.
fn focus_self(client: &mut Client) -> io::Result<()> {
    let Some(window) = client
        .windows()?
        .into_iter()
        .find(|window| window.app_id.as_deref() == Some(tessera_settings::APP_ID))
    else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the Settings window is not in the compositor's window list",
        ));
    };
    client.command(tessera_protocol::Command::Focus {
        id: window.id,
        reveal: true,
    })
}

/// When to renew the session lease: halfway through its lifetime.
fn lease_deadline(client: &Client) -> Option<Instant> {
    client
        .lease()
        .filter(|lease| lease.renewable && lease.ttl_ms > 0)
        .map(|lease| Instant::now() + Duration::from_millis(lease.ttl_ms / 2))
}

fn watch_loop(socket: &Path, refresh: &mpsc::Sender<Request>) {
    let mut backoff = Backoff::new();
    loop {
        let subscribed = connect(socket, false).and_then(|mut client| {
            // Events arrive only when something changes; the watcher must
            // block indefinitely between them.
            client.set_io_timeout(None)?;
            client.subscribe()?;
            Ok(client)
        });
        let mut client = match subscribed {
            Ok(client) => {
                backoff.reset();
                // Anything that changed while the watcher was away.
                if refresh.send(Request::Refresh).is_err() {
                    return;
                }
                client
            }
            Err(error) => {
                log::debug!("settings: event subscription unavailable: {error}");
                backoff.wait();
                continue;
            }
        };
        loop {
            match client.next_event() {
                Ok(Event::SettingsChanged { .. }) => {
                    if refresh.send(Request::Refresh).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    log::debug!("settings: event stream ended: {error}");
                    break;
                }
            }
        }
        backoff.wait();
    }
}
