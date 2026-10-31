//! One casting session, from the P2P group to media.
//!
//! Shared by the `cast` CLI and the long-lived service, so both drive exactly
//! the same state machine. The source is the RTSP server: this binds the control
//! port, advertises it, waits for the sink to connect, serves the exchange, and then
//! streams media to the address the sink named.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::cast::{self, AcceptError, Drain, Opening};
use crate::media::{
    AacCapture, AudioFrameSource, AudioSourceKind, DesktopCaptureSource, RtcpSender, StreamConfig,
    TestPatternSource, UdpMediaSender, stream, stream_ts_file,
};
use crate::radio::P2pRadio;
use crate::wfd::parameters::SourceParameters;
use crate::wfd::session::WfdSourceSession;
use crate::wfd::transport::RtspTransport;

/// The source format validated end-to-end against the reference Skyworth A3F:
/// CEA 720p30 with that sink's native/preferred fields. Override it when a sink
/// does not offer 720p30.
pub const VALIDATED_VIDEO_FORMATS: &str =
    "38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none";

/// Which frames the session sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSource {
    /// The live desktop through the `ScreenCast` portal and VA-API.
    Desktop,
    /// The self-contained H.264 test pattern (no portal, no GPU).
    Pattern,
}

/// Everything one session needs.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub bind: String,
    pub control_port: u16,
    pub peer: Option<String>,
    pub settle: Duration,
    pub video_formats: String,
    pub rtp_port: u16,
    pub timeout: Duration,
    pub fps: u32,
    pub source: MediaSource,
    pub duration: Option<Duration>,
    pub max_frames: Option<u64>,
    /// Whether to stream media after the handshake. `false` is the diagnostic
    /// (`handshake`) mode.
    pub stream_media: bool,
    /// Diagnostic: send this pre-muxed MPEG-TS file instead of the built-in
    /// muxer. It still uses the real RTSP/P2P/RTP/RTCP path.
    pub ts_file: Option<std::path::PathBuf>,
    /// The nominal duration of `ts_file`, used to pace it in real time.
    pub ts_seconds: u64,
    /// Who opens the RTSP exchange. Miracast's documented direction is
    /// [`Opening::Sink`]; [`Opening::Source`] is a compatibility path for sinks
    /// (observed: Android 9 `stagefright/1.2`) that clear a stale session and
    /// then wait for the source to speak.
    pub opening: Opening,
    /// Capture and send AAC audio alongside the video stream.
    pub audio: bool,
    /// The PipeWire node to capture when `audio` is set.
    pub audio_source: AudioSourceKind,
    /// AAC bitrate in kbit/s (32..=512).
    pub audio_bitrate_kbps: u32,
    /// Owner-supplied cancellation, checked while waiting and per frame.
    pub stop: Option<Arc<AtomicBool>>,
}

/// Progress a caller can project into its own state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Listening(SocketAddr),
    FirewallBlocked(String),
    SinkConnected(SocketAddr),
    Negotiating,
    Streaming,
    /// Audio capture and encoding started with this codec.
    AudioActive(String),
    /// Audio was requested but could not start, with the named reason.
    AudioUnavailable(String),
    /// RTCP sender reports could not start; A/V sync may be absent.
    RtcpUnavailable(String),
    /// The UDP media endpoint the source bound and the sink's RTP target.
    MediaEndpoint {
        local: SocketAddr,
        target: SocketAddr,
    },
    /// The full two-directional RTSP trace of the negotiation.
    Trace(String),
    /// Desktop capture failed and the session fell back to the test pattern.
    MediaFallback(String),
    Stats {
        frames_sent: u64,
        audio_frames_sent: u64,
        bytes_sent: u64,
    },
    /// The session ended normally (or was stopped), with its termination cause.
    Stopped(String),
    /// The session ended in a named failure.
    Failed(String),
}

/// Run one session to completion.
pub fn run(options: &SessionOptions, events: &mut dyn FnMut(SessionEvent)) -> Result<(), String> {
    let listener =
        TcpListener::bind((options.bind.as_str(), options.control_port)).map_err(|error| {
            format!(
                "could not listen on {}:{}: {error}",
                options.bind, options.control_port
            )
        })?;
    let local = listener
        .local_addr()
        .map_err(|error| format!("could not read the listener address: {error}"))?;
    events(SessionEvent::Listening(local));

    let inbound = crate::firewall::observe(options.control_port);
    if inbound.is_blocked() {
        events(SessionEvent::FirewallBlocked(inbound.describe()));
    }

    let radio = crate::radio::iwd::IwdRadio::open().map_err(|error| error.to_string())?;
    let report = crate::discover::discover(&radio, options.control_port, options.settle)
        .map_err(|error| format!("discovery failed: {error}"))?;

    let peer_path = match &options.peer {
        Some(needle) => match crate::endpoint::match_peer(&report.peers, needle) {
            crate::endpoint::PeerMatch::ExactlyOne(peer) => {
                if !peer.connected {
                    // WPS/P2P group formation is asynchronous and the sink's
                    // state is not ours: a stale credential on either side can
                    // fail one attempt and succeed on the next. Retry a bounded
                    // number of times, then name the operator action.
                    let mut last_error = None;
                    for attempt in 1..=3 {
                        match radio.form_group(&peer.path) {
                            Ok(()) => {
                                last_error = None;
                                break;
                            }
                            Err(error) => {
                                log::warn!("cast: P2P group attempt {attempt}/3 failed: {error}");
                                last_error = Some(error);
                                if attempt < 3 {
                                    std::thread::sleep(Duration::from_secs(2));
                                }
                            }
                        }
                    }
                    if let Some(error) = last_error {
                        return Err(format!(
                            "could not form the P2P group after 3 attempts: {error}; \
                             toggle the sink's Screen Mirroring mode (or reboot it) and retry"
                        ));
                    }
                }
                Some(peer.path.clone())
            }
            crate::endpoint::PeerMatch::None => {
                return Err(format!("no peer matched '{needle}'"));
            }
            crate::endpoint::PeerMatch::Ambiguous(candidates) => {
                let labels: Vec<&str> = candidates
                    .iter()
                    .map(|peer| peer.name.as_deref().unwrap_or(peer.path.as_str()))
                    .collect();
                return Err(format!(
                    "multiple peers matched '{needle}': {}",
                    labels.join(", ")
                ));
            }
        },
        None => None,
    };

    let deadline = Instant::now() + options.timeout;
    let sink_stream = match cast::accept_until(&listener, deadline, options.stop.as_deref()) {
        Ok(stream) => stream,
        Err(AcceptError::Stopped) => {
            if let Some(path) = &peer_path {
                let _ = radio.disconnect_peer(path);
            }
            events(SessionEvent::Stopped(
                "stopped while waiting for a sink".to_owned(),
            ));
            return Ok(());
        }
        Err(error) => {
            let reason = if inbound.is_blocked() {
                format!(
                    "{error}; the host firewall blocks inbound tcp/{}",
                    options.control_port
                )
            } else {
                error.to_string()
            };
            if let Some(path) = &peer_path {
                let _ = radio.disconnect_peer(path);
            }
            events(SessionEvent::Failed(reason.clone()));
            return Err(reason);
        }
    };

    let sink_address = sink_stream
        .peer_addr()
        .map_err(|error| format!("could not read the sink address: {error}"))?;
    events(SessionEvent::SinkConnected(sink_address));

    // Start audio capture before the RTSP exchange so the sink's audio decoder
    // receives a small pre-roll at PLAY instead of starting on an empty pipe.
    let mut pre_roll_audio = if options.audio && options.ts_file.is_none() {
        match AacCapture::start(options.audio_source, options.audio_bitrate_kbps) {
            Ok(capture) => Some(capture),
            Err(error) => {
                events(SessionEvent::AudioUnavailable(error));
                None
            }
        }
    } else {
        None
    };

    let timeout = Some(options.timeout.max(Duration::from_secs(1)));
    sink_stream
        .set_read_timeout(timeout)
        .and_then(|()| sink_stream.set_write_timeout(timeout))
        .map_err(|error| format!("could not set socket timeouts: {error}"))?;

    let local_ip = sink_stream
        .local_addr()
        .map(|address| address.ip())
        .unwrap_or(std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let parameters = SourceParameters::new(options.video_formats.clone())
        .with_presentation_url(format!("rtsp://{local_ip}/wfd1.0/streamid=0"));
    parameters.to_map().map_err(|error| error.to_string())?;
    let mut session = WfdSourceSession::new(parameters);
    if options.audio || options.ts_file.is_some() {
        session.enable_audio();
    }
    events(SessionEvent::Negotiating);

    let handshake = match options.opening {
        Opening::Source => cast::handshake(
            RtspTransport::new(sink_stream),
            &mut session,
            deadline,
            64,
            Drain::UntilQuiet(8),
        ),
        Opening::Sink => cast::handshake_as_server(
            RtspTransport::new(sink_stream),
            &mut session,
            deadline,
            64,
            Drain::UntilQuiet(8),
        ),
        Opening::Initiator => cast::handshake_as_initiator(
            RtspTransport::new(sink_stream),
            &mut session,
            deadline,
            64,
            Drain::UntilQuiet(8),
        ),
    };
    let (outcome, transport) = match handshake {
        Ok(result) => result,
        Err(error) => {
            events(SessionEvent::Trace(session.trace().render()));
            let reason = format!("handshake failed: {error}");
            if let Some(path) = &peer_path {
                let _ = radio.disconnect_peer(path);
            }
            events(SessionEvent::Failed(reason.clone()));
            return Err(reason);
        }
    };
    events(SessionEvent::Trace(session.trace().render()));

    if !outcome.is_streaming() {
        let reason = format!("the sink did not accept streaming: {}", outcome.describe());
        if let Some(path) = &peer_path {
            let _ = radio.disconnect_peer(path);
        }
        events(SessionEvent::Failed(reason.clone()));
        return Err(reason);
    }
    events(SessionEvent::Streaming);

    if !options.stream_media {
        if let Some(path) = &peer_path {
            let _ = radio.disconnect_peer(path);
        }
        events(SessionEvent::Stopped(
            "handshake only; no media was sent".to_owned(),
        ));
        return Ok(());
    }

    let sink_rtp_port = session.sink_rtp_port().unwrap_or(options.rtp_port);
    let target = SocketAddr::new(sink_address.ip(), sink_rtp_port);
    // Advertise and bind the same server port. A sink is allowed to filter RTP
    // by the source port named in the SETUP response; an ephemeral sender port
    // silently loses every packet on such sinks.
    let server_rtp = SocketAddr::new(local_ip, crate::wfd::session::SERVER_RTP_PORT);
    let sender = UdpMediaSender::bind_at(server_rtp, target)
        .map_err(|error| format!("could not bind the UDP media socket at {server_rtp}: {error}"))?;
    events(SessionEvent::MediaEndpoint {
        local: sender.local_addr().unwrap_or(server_rtp),
        target,
    });

    let rtcp = match RtcpSender::bind(
        SocketAddr::new(local_ip, crate::wfd::session::SERVER_RTP_PORT + 1),
        SocketAddr::new(sink_address.ip(), sink_rtp_port.saturating_add(1)),
        0x1900_0000,
    ) {
        Ok(sender) => Some(sender),
        Err(error) => {
            events(SessionEvent::RtcpUnavailable(format!(
                "could not bind the RTCP sender at {}: {error}",
                SocketAddr::new(local_ip, crate::wfd::session::SERVER_RTP_PORT + 1)
            )));
            None
        }
    };

    let _ = transport
        .stream()
        .set_read_timeout(Some(Duration::from_millis(5)));
    let config = StreamConfig {
        fps: options.fps,
        max_frames: options.max_frames,
        max_duration: options.duration,
        ssrc: 0x1900_0000,
        stop: options.stop.clone(),
    };

    let stats = if let Some(path) = &options.ts_file {
        let file = std::fs::File::open(path)
            .map_err(|error| format!("could not open the TS file {}: {error}", path.display()))?;
        match session.audio_codec() {
            Some(codec) => events(SessionEvent::AudioActive(codec.to_owned())),
            None => events(SessionEvent::AudioUnavailable(
                "the sink offered no AAC audio codec".to_owned(),
            )),
        }
        let (stats, _transport) = stream_ts_file(
            transport,
            &mut session,
            sender,
            file,
            rtcp,
            config,
            Duration::from_secs(options.ts_seconds.max(1)),
        );
        stats
    } else {
        let audio: Option<Box<dyn AudioFrameSource>> = if options.audio {
            match session.audio_codec() {
                Some(codec) if codec.starts_with("AAC") => {
                    if let Some(capture) = pre_roll_audio.take() {
                        events(SessionEvent::AudioActive(codec.to_owned()));
                        Some(Box::new(capture) as Box<dyn AudioFrameSource>)
                    } else {
                        None
                    }
                }
                Some(codec) => {
                    drop(pre_roll_audio.take());
                    events(SessionEvent::AudioUnavailable(format!(
                        "the sink selected {codec}, but this source only encodes AAC"
                    )));
                    None
                }
                None => {
                    drop(pre_roll_audio.take());
                    events(SessionEvent::AudioUnavailable(
                        "the sink offered no AAC audio codec".to_owned(),
                    ));
                    None
                }
            }
        } else {
            None
        };
        let (stats, _transport) = match options.source {
            MediaSource::Pattern => stream(
                transport,
                &mut session,
                sender,
                TestPatternSource::new(options.fps),
                audio,
                rtcp,
                config,
            ),
            MediaSource::Desktop => match DesktopCaptureSource::open(options.fps, true) {
                Ok(source) => stream(transport, &mut session, sender, source, audio, rtcp, config),
                Err(error) => {
                    events(SessionEvent::MediaFallback(error.to_string()));
                    stream(
                        transport,
                        &mut session,
                        sender,
                        TestPatternSource::new(options.fps),
                        audio,
                        rtcp,
                        config,
                    )
                }
            },
        };
        stats
    };

    events(SessionEvent::Stats {
        frames_sent: stats.frames_sent,
        audio_frames_sent: stats.audio_frames_sent,
        bytes_sent: stats.bytes_sent,
    });
    if let Some(reason) = stats.audio_failure.clone() {
        events(SessionEvent::AudioUnavailable(reason));
    }
    if let Some(reason) = stats.rtcp_failure.clone() {
        events(SessionEvent::RtcpUnavailable(reason));
    }

    if let Some(path) = &peer_path {
        let _ = radio.disconnect_peer(path);
    }
    events(SessionEvent::Stopped(stats.termination.to_string()));
    Ok(())
}
