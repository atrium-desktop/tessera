//! The out-of-process cast service (ADR-0188, ADR-0192).
//!
//! One long-lived process owns discovery and sessions. The compositor connects
//! to a per-user Unix socket, sends commands, and renders the `CastStatus`
//! snapshots this service pushes. Nothing here is inferred by chrome, and no
//! command kills a process by name: a session stops through its own flag.
//!
//! The wire format is a 4-byte big-endian length followed by a JSON body. It is
//! versioned in [`PROTOCOL_VERSION`] and carries only `tessera-desktop`'s
//! backend-neutral vocabulary.

use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tessera_desktop::system::{
    CastCapability, CastDiscovery, CastProtocol, CastServiceState, CastSession, CastSessionState,
    CastSink, CastStatus,
};

use crate::session::{self, MediaSource, SessionEvent, SessionOptions};

/// Bumped whenever [`Request`] or [`Event`] changes shape.
pub const PROTOCOL_VERSION: u32 = 1;

/// A command from a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Hello { version: u32 },
    Subscribe,
    StartDiscovery,
    StopDiscovery,
    Connect { sink: String },
    Disconnect { session: u64 },
    Shutdown,
}

/// A push from the service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    HelloAck {
        version: u32,
        /// `false` means the service cannot start a session until an operator
        /// supplies a `wfd_video_formats` value.
        video_formats_configured: bool,
    },
    Status {
        status: CastStatus,
    },
    Error {
        message: String,
    },
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

/// Service configuration.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    pub socket: PathBuf,
    /// Defaults to the hardware-validated 720p30 value; empty makes a Connect
    /// fail with a named refusal instead of inventing a bitfield.
    pub video_formats: String,
    pub control_port: u16,
    pub rtp_port: u16,
    pub settle: Duration,
    pub session_timeout: Duration,
    pub fps: u32,
    pub source: MediaSource,
    /// Who opens the RTSP exchange (see [`crate::cast::Opening`]).
    pub opening: crate::cast::Opening,
    /// Capture and send AAC audio alongside the video stream.
    pub audio: bool,
    pub audio_source: crate::media::AudioSourceKind,
    pub audio_bitrate_kbps: u32,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            socket: default_socket_path(),
            video_formats: crate::session::VALIDATED_VIDEO_FORMATS.to_owned(),
            control_port: crate::firewall::WFD_CONTROL_PORT,
            rtp_port: 19_000,
            settle: Duration::from_secs(4),
            session_timeout: Duration::from_secs(60),
            fps: 30,
            source: MediaSource::Desktop,
            opening: crate::cast::Opening::Initiator,
            audio: false,
            audio_source: crate::media::AudioSourceKind::System,
            audio_bitrate_kbps: 128,
        }
    }
}

/// `$XDG_RUNTIME_DIR/tessera-castd.sock`.
pub fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("tessera-castd.sock")
}

struct SessionHandle {
    id: u64,
    stop: Arc<AtomicBool>,
}

struct DaemonState {
    status: CastStatus,
    subscribers: Vec<Sender<Event>>,
    discovery_stop: Option<Arc<AtomicBool>>,
    session: Option<SessionHandle>,
    next_session: u64,
}

/// Run the service until the socket is closed or a client sends `Shutdown`.
pub fn run(options: DaemonOptions) -> Result<(), String> {
    let listener = bind_socket(&options.socket)
        .map_err(|error| format!("could not bind {}: {error}", options.socket.display()))?;

    let mut status = CastStatus {
        service: CastServiceState::Available,
        ..CastStatus::default()
    };
    let inbound = crate::firewall::observe(options.control_port);
    let mut notes = Vec::new();
    if inbound.is_blocked() {
        notes.push(format!(
            "{}; add an ingress rule, e.g. sudo nft add rule inet filter input iifname \"wlan0-p2p*\" accept",
            inbound.describe()
        ));
    } else if matches!(inbound, crate::firewall::InboundVerdict::Unverified(_)) {
        notes.push(inbound.describe());
    }
    if options.video_formats.trim().is_empty() {
        notes.push("no [cast] video_formats configured; sessions will be refused".to_owned());
    }
    if !notes.is_empty() {
        status.last_error = Some(notes.join("; "));
    }

    let state = Arc::new(Mutex::new(DaemonState {
        status,
        subscribers: Vec::new(),
        discovery_stop: None,
        session: None,
        next_session: 0,
    }));
    let options = Arc::new(options);

    for connection in listener.incoming() {
        let Ok(stream) = connection else {
            continue;
        };
        // The socket is mode 0600 inside the user's XDG_RUNTIME_DIR (0700), so
        // the filesystem is the boundary. A kernel SO_PEERCRED check would need
        // an unsafe/libc dependency this crate forbids, and the protocol grants
        // nothing the user could not do by running the CLI directly.
        let state = state.clone();
        let options = options.clone();
        thread::spawn(move || handle_client(stream, state, options));
    }
    Ok(())
}

fn handle_client(stream: UnixStream, state: Arc<Mutex<DaemonState>>, options: Arc<DaemonOptions>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel::<Event>();
    let writer_thread = thread::spawn(move || {
        for event in rx {
            if write_frame(&mut writer, &event).is_err() {
                break;
            }
        }
    });

    let mut reader = stream;
    while let Ok(Some(request)) = read_frame::<_, Request>(&mut reader) {
        match request {
            Request::Hello { version } => {
                let _ = tx.send(Event::HelloAck {
                    version: PROTOCOL_VERSION,
                    video_formats_configured: !options.video_formats.trim().is_empty(),
                });
                if version != PROTOCOL_VERSION {
                    let _ = tx.send(Event::Error {
                        message: format!(
                            "protocol version {version} does not match this service ({PROTOCOL_VERSION})"
                        ),
                    });
                }
            }
            Request::Subscribe => {
                let status = {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    let status = state.status.clone();
                    state.subscribers.retain(|subscriber| {
                        subscriber
                            .send(Event::Status {
                                status: status.clone(),
                            })
                            .is_ok()
                    });
                    state.subscribers.push(tx.clone());
                    status
                };
                let _ = tx.send(Event::Status { status });
            }
            Request::StartDiscovery => start_discovery(&state, &options),
            Request::StopDiscovery => stop_discovery(&state),
            Request::Connect { sink } => {
                if let Err(message) = connect(&state, &options, sink) {
                    let _ = tx.send(Event::Error { message });
                }
            }
            Request::Disconnect { session } => disconnect(&state, session),
            Request::Shutdown => {
                stop_discovery(&state);
                if let Some(handle) = state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .session
                    .as_ref()
                {
                    handle.stop.store(true, Ordering::Relaxed);
                }
                // The accept loop is blocked in `incoming()`; terminating here
                // is the explicit service stop, and the supervisor restarts us
                // only if the session still wants a service.
                std::process::exit(0);
            }
        }
    }

    // Drop this connection's subscription; the writer ends when the channel
    // closes.
    {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        let status = state.status.clone();
        state.subscribers.retain(|subscriber| {
            subscriber
                .send(Event::Status {
                    status: status.clone(),
                })
                .is_ok()
        });
    }
    drop(tx);
    let _ = writer_thread.join();
}

fn start_discovery(state: &Arc<Mutex<DaemonState>>, options: &DaemonOptions) {
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    if guard.discovery_stop.is_some() {
        guard.status.discovery = CastDiscovery::Scanning;
        broadcast(&mut guard);
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    guard.discovery_stop = Some(stop.clone());
    guard.status.service = CastServiceState::Available;
    guard.status.discovery = CastDiscovery::Scanning;
    broadcast(&mut guard);
    drop(guard);

    let state = state.clone();
    let control_port = options.control_port;
    let settle = options.settle;
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let result = discover_sinks(control_port, settle);
            {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(sinks) => {
                        guard.status.sinks = sinks;
                        guard.status.discovery = CastDiscovery::Scanning;
                    }
                    Err(error) => {
                        guard.status.discovery = CastDiscovery::Failed;
                        guard.status.last_error = Some(error);
                    }
                }
                broadcast(&mut guard);
            }
            for _ in 0..80 {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.discovery_stop = None;
        guard.status.discovery = CastDiscovery::Idle;
        broadcast(&mut guard);
    });
}

fn stop_discovery(state: &Arc<Mutex<DaemonState>>) {
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    // Keep the handle until the loop observes the flag and clears it: a session
    // that starts before the radio is released would race the discovery pass.
    if let Some(stop) = &guard.discovery_stop {
        stop.store(true, Ordering::Relaxed);
    }
    guard.status.discovery = CastDiscovery::Idle;
    broadcast(&mut guard);
}

fn discover_sinks(control_port: u16, settle: Duration) -> Result<Vec<CastSink>, String> {
    let radio = crate::radio::iwd::IwdRadio::open().map_err(|error| error.to_string())?;
    let report = crate::discover::discover(&radio, control_port, settle)
        .map_err(|error| error.to_string())?;
    Ok(report
        .peers
        .into_iter()
        .map(|peer| {
            let display = peer.display;
            let mut capabilities = Vec::new();
            if display.is_some_and(|display| display.sink) {
                capabilities.push(CastCapability::ScreenMirroring);
            }
            if display.is_some_and(|display| display.has_audio) {
                capabilities.push(CastCapability::Audio);
            }
            if display.is_some_and(|display| display.has_uibc) {
                capabilities.push(CastCapability::InputBackchannel);
            }
            CastSink {
                id: peer.path.clone(),
                name: peer.name.clone().unwrap_or_else(|| peer.path.clone()),
                protocol: CastProtocol::Wfd,
                capabilities,
                blocker: peer.cast_blocker().map(str::to_owned),
            }
        })
        .collect())
}

fn connect(
    state: &Arc<Mutex<DaemonState>>,
    options: &DaemonOptions,
    sink_id: String,
) -> Result<(), String> {
    if options.video_formats.trim().is_empty() {
        return Err(
            "no wfd_video_formats is configured for the cast service; start \
             tessera-castd with --video-formats <value>"
                .to_owned(),
        );
    }

    // A session owns the radio: stop the discovery loop and wait for it to
    // release the adapter before the session registers and forms a group.
    stop_discovery(state);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let running = state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .discovery_stop
            .is_some();
        if !running {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .discovery_stop
        .is_some()
    {
        return Err("the discovery pass did not release the radio".to_owned());
    }

    let (id, sink, stop) = {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.session.is_some() {
            return Err("a casting session is already active".to_owned());
        }
        let sink = guard
            .status
            .sinks
            .iter()
            .find(|sink| sink.id == sink_id)
            .cloned()
            .ok_or_else(|| format!("sink '{sink_id}' is not in the current discovery list"))?;
        if let Some(blocker) = &sink.blocker {
            return Err(blocker.clone());
        }
        guard.next_session += 1;
        let id = guard.next_session;
        let stop = Arc::new(AtomicBool::new(false));
        guard.session = Some(SessionHandle {
            id,
            stop: stop.clone(),
        });
        guard.status.sessions = vec![CastSession {
            id,
            sink_id: sink.id.clone(),
            sink_name: sink.name.clone(),
            state: CastSessionState::Connecting,
            frames_sent: 0,
            audio: None,
            failure: None,
        }];
        broadcast(&mut guard);
        (id, sink, stop)
    };

    let session_options = SessionOptions {
        bind: "0.0.0.0".to_owned(),
        control_port: options.control_port,
        peer: Some(sink.id),
        settle: options.settle,
        video_formats: options.video_formats.clone(),
        rtp_port: options.rtp_port,
        timeout: options.session_timeout,
        fps: options.fps,
        source: options.source,
        duration: None,
        max_frames: None,
        stream_media: true,
        ts_file: None,
        ts_seconds: 30,
        opening: options.opening,
        audio: options.audio,
        audio_source: options.audio_source,
        audio_bitrate_kbps: options.audio_bitrate_kbps,
        stop: Some(stop),
    };
    let state = state.clone();
    thread::spawn(move || {
        let result = session::run(&session_options, &mut |event| {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(session) = guard.status.sessions.iter_mut().find(|s| s.id == id) {
                match event {
                    SessionEvent::Listening(_) | SessionEvent::SinkConnected(_) => {
                        session.state = CastSessionState::Connecting;
                    }
                    SessionEvent::Negotiating => {
                        session.state = CastSessionState::Negotiating;
                    }
                    SessionEvent::Streaming => {
                        session.state = CastSessionState::Streaming;
                        // A live session proves inbound reachability; a stale
                        // firewall note must not read as a blocker beside it.
                        guard.status.last_error = None;
                    }
                    SessionEvent::Stats { frames_sent, .. } => {
                        session.frames_sent = frames_sent;
                    }
                    SessionEvent::Stopped(_) => {
                        session.state = CastSessionState::Stopping;
                    }
                    SessionEvent::Failed(reason) => {
                        session.state = CastSessionState::Failed;
                        session.failure = Some(reason.clone());
                        guard.status.last_error = Some(reason);
                    }
                    SessionEvent::FirewallBlocked(description) => {
                        guard.status.last_error = Some(description);
                    }
                    SessionEvent::AudioActive(codec) => {
                        session.audio = Some(codec);
                    }
                    SessionEvent::AudioUnavailable(reason) => {
                        guard.status.last_error = Some(reason);
                    }
                    SessionEvent::RtcpUnavailable(reason) => {
                        guard.status.last_error = Some(reason);
                    }
                    SessionEvent::MediaFallback(error) => {
                        guard.status.last_error = Some(error);
                    }
                    SessionEvent::MediaEndpoint { .. } => {}
                    SessionEvent::Trace(_) => {}
                }
            }
            broadcast(&mut guard);
        });

        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.session = None;
        if let Err(error) = result {
            guard.status.last_error = Some(error);
            let failure = guard.status.last_error.clone();
            if let Some(session) = guard.status.sessions.iter_mut().find(|s| s.id == id)
                && session.state != CastSessionState::Failed
            {
                session.state = CastSessionState::Failed;
                session.failure = failure;
            }
        } else if let Some(session) = guard.status.sessions.iter_mut().find(|s| s.id == id)
            && session.state != CastSessionState::Failed
        {
            session.state = CastSessionState::Stopping;
        }
        broadcast(&mut guard);
    });
    Ok(())
}

fn disconnect(state: &Arc<Mutex<DaemonState>>, session_id: u64) {
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(handle) = &guard.session
        && handle.id == session_id
    {
        handle.stop.store(true, Ordering::Relaxed);
    }
    if let Some(session) = guard
        .status
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    {
        session.state = CastSessionState::Stopping;
    }
    broadcast(&mut guard);
}

fn broadcast(state: &mut DaemonState) {
    let status = state.status.clone();
    state.subscribers.retain(|subscriber| {
        subscriber
            .send(Event::Status {
                status: status.clone(),
            })
            .is_ok()
    });
}

fn bind_socket(path: &Path) -> io::Result<UnixListener> {
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
                format!("another cast service is listening on {}", path.display()),
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

    #[test]
    fn frames_round_trip_and_a_clean_eof_is_none() {
        let mut buffer = Vec::new();
        let request = Request::Connect {
            sink: "peer-1".to_owned(),
        };
        write_frame(&mut buffer, &request).expect("write frame");

        let mut cursor = std::io::Cursor::new(buffer.clone());
        let decoded: Option<Request> = read_frame(&mut cursor).expect("read frame");
        assert!(matches!(
            decoded,
            Some(Request::Connect { sink }) if sink == "peer-1"
        ));
        assert!(
            read_frame::<_, Request>(&mut cursor)
                .expect("clean EOF")
                .is_none()
        );
    }

    #[test]
    fn an_oversized_frame_is_refused() {
        let mut cursor = std::io::Cursor::new(vec![0x00, 0x20, 0x00, 0x00]);
        let error = read_frame::<_, Request>(&mut cursor).expect_err("must refuse");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_non_socket_at_the_path_is_never_replaced() {
        let directory = std::env::temp_dir().join(format!(
            "tessera-castd-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create temp dir");
        let path = directory.join("castd.sock");
        std::fs::write(&path, b"not a socket").expect("write file");
        let error = bind_socket(&path).expect_err("must refuse");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(directory);
    }
}
