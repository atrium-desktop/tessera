//! Client and supervisor for the out-of-process cast service (ADR-0188,
//! ADR-0192).
//!
//! The compositor never opens iwd and never starts a session itself. It owns one
//! supervised `tessera-castd` process, speaks its control protocol over a
//! per-user Unix socket, and publishes the service's `CastStatus` snapshots.
//! Chrome therefore renders the service's facts, not a local guess.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tessera_cast::daemon::{self, Event, Request};
use tessera_cast::media::AudioSourceKind;
use tessera_cast::session::MediaSource;
use tessera_desktop::system::{CastDiscovery, CastServiceState, CastStatus};

/// How long the client waits before trying a missing service again.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// Backoff after the daemon exits on its own.
const RESTART_BACKOFF: Duration = Duration::from_secs(2);

/// The `[cast]` configuration handed to the service on spawn.
#[derive(Debug, Clone)]
pub struct CastServiceOptions {
    /// Defaults to the hardware-validated 720p30 value; empty refuses a session
    /// with a named reason.
    pub video_formats: String,
    pub source: MediaSource,
    pub opening: tessera_cast::cast::Opening,
    /// Capture and send AAC audio alongside the video stream.
    pub audio: bool,
    pub audio_source: AudioSourceKind,
    pub audio_bitrate_kbps: u32,
    pub fps: u32,
    pub control_port: u16,
    pub rtp_port: u16,
    pub settle: Duration,
    pub session_timeout: Duration,
}

impl Default for CastServiceOptions {
    fn default() -> Self {
        Self {
            video_formats: tessera_config::DEFAULT_CAST_VIDEO_FORMATS.to_owned(),
            source: MediaSource::Desktop,
            opening: tessera_cast::cast::Opening::Initiator,
            audio: false,
            audio_source: AudioSourceKind::System,
            audio_bitrate_kbps: 128,
            fps: 30,
            control_port: tessera_cast::firewall::WFD_CONTROL_PORT,
            rtp_port: 19_000,
            settle: Duration::from_secs(4),
            session_timeout: Duration::from_secs(60),
        }
    }
}

/// Non-blocking handle to the cast service.
pub struct CastHandle {
    commands: mpsc::Sender<Request>,
    snapshot: Arc<Mutex<CastStatus>>,
}

impl CastHandle {
    /// Start the service and its client from the `[cast]` configuration.
    pub fn spawn_auto(options: CastServiceOptions) -> Self {
        let socket = daemon::default_socket_path();
        let snapshot = Arc::new(Mutex::new(CastStatus {
            service: CastServiceState::Unavailable,
            ..CastStatus::default()
        }));
        let (commands, command_rx) = mpsc::channel();
        let thread_snapshot = snapshot.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("tessera-cast".into())
            .spawn(move || run_client(socket, options, thread_snapshot, command_rx))
        {
            log::error!("failed to spawn the cast service client: {err}");
        }
        Self { commands, snapshot }
    }

    /// The latest service snapshot, without blocking.
    pub fn snapshot(&self) -> CastStatus {
        self.snapshot
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn start_discovery(&self) {
        let _ = self.commands.send(Request::StartDiscovery);
    }

    pub fn stop_discovery(&self) {
        let _ = self.commands.send(Request::StopDiscovery);
    }

    pub fn connect(&self, sink: String) {
        let _ = self.commands.send(Request::Connect { sink });
    }

    pub fn disconnect(&self, session: u64) {
        let _ = self.commands.send(Request::Disconnect { session });
    }
}

fn run_client(
    socket: PathBuf,
    options: CastServiceOptions,
    snapshot: Arc<Mutex<CastStatus>>,
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

        let Ok(stream) = UnixStream::connect(&socket) else {
            {
                let mut status = snapshot.lock().unwrap_or_else(|e| e.into_inner());
                status.service = CastServiceState::Unavailable;
                status.discovery = CastDiscovery::Idle;
                status.sessions.clear();
                status.last_error = Some("the cast service is not reachable".to_owned());
            }
            std::thread::sleep(RECONNECT_DELAY);
            continue;
        };

        let Ok(mut writer) = stream.try_clone() else {
            continue;
        };
        let Ok(shutdown_handle) = stream.try_clone() else {
            continue;
        };
        let (events_tx, events_rx) = mpsc::channel();
        let mut reader = stream;
        let reader_thread = std::thread::spawn(move || {
            while let Ok(Some(event)) = daemon::read_frame::<_, Event>(&mut reader) {
                if events_tx.send(event).is_err() {
                    break;
                }
            }
        });
        let _ = daemon::write_frame(
            &mut writer,
            &Request::Hello {
                version: daemon::PROTOCOL_VERSION,
            },
        );
        let _ = daemon::write_frame(&mut writer, &Request::Subscribe);
        {
            let mut status = snapshot.lock().unwrap_or_else(|e| e.into_inner());
            status.service = CastServiceState::Available;
        }

        loop {
            match events_rx.try_recv() {
                Ok(event) => apply_event(&snapshot, event),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }

            match commands.recv_timeout(Duration::from_millis(100)) {
                Ok(request) => {
                    if daemon::write_frame(&mut writer, &request).is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = daemon::write_frame(&mut writer, &Request::Shutdown);
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
        {
            let mut status = snapshot.lock().unwrap_or_else(|e| e.into_inner());
            status.service = CastServiceState::Unavailable;
            status.discovery = CastDiscovery::Idle;
            status.sessions.clear();
            status.last_error = Some("the cast service is not reachable".to_owned());
        }
        std::thread::sleep(RECONNECT_DELAY);
    }
}

fn spawn_daemon(socket: &Path, options: &CastServiceOptions) -> Option<Child> {
    let program = crate::runtime::trusted_sibling_program("tessera-castd").ok()?;
    let mut command = Command::new(program);
    command
        .arg("--socket")
        .arg(socket)
        .arg("--source")
        .arg(match options.source {
            MediaSource::Desktop => "desktop",
            MediaSource::Pattern => "pattern",
        })
        .arg("--opening")
        .arg(match options.opening {
            tessera_cast::cast::Opening::Sink => "sink",
            tessera_cast::cast::Opening::Source | tessera_cast::cast::Opening::Initiator => {
                "source"
            }
        })
        .arg("--fps")
        .arg(options.fps.to_string())
        .arg("--audio-source")
        .arg(options.audio_source.as_str())
        .arg("--audio-bitrate")
        .arg(options.audio_bitrate_kbps.to_string())
        .arg("--port")
        .arg(options.control_port.to_string())
        .arg("--rtp-port")
        .arg(options.rtp_port.to_string())
        .arg("--settle")
        .arg(options.settle.as_secs().to_string())
        .arg("--timeout")
        .arg(options.session_timeout.as_secs().to_string());
    if !options.video_formats.trim().is_empty() {
        command.arg("--video-formats").arg(&options.video_formats);
    }
    if options.audio {
        command.arg("--audio");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn apply_event(snapshot: &Arc<Mutex<CastStatus>>, event: Event) {
    let mut status = snapshot.lock().unwrap_or_else(|e| e.into_inner());
    match event {
        Event::Status { status: next } => *status = next,
        Event::HelloAck { .. } => status.service = CastServiceState::Available,
        Event::Error { message } => status.last_error = Some(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_event_replaces_the_snapshot_and_an_error_is_named() {
        let snapshot = Arc::new(Mutex::new(CastStatus::default()));
        apply_event(
            &snapshot,
            Event::Status {
                status: CastStatus {
                    service: CastServiceState::Available,
                    discovery: CastDiscovery::Scanning,
                    ..CastStatus::default()
                },
            },
        );
        let status = snapshot.lock().expect("snapshot").clone();
        assert_eq!(status.service, CastServiceState::Available);
        assert_eq!(status.discovery, CastDiscovery::Scanning);

        apply_event(
            &snapshot,
            Event::Error {
                message: "no video formats".to_owned(),
            },
        );
        assert_eq!(
            snapshot.lock().expect("snapshot").last_error.as_deref(),
            Some("no video formats")
        );
    }
}
