use std::process::ExitCode;

/// Exit codes are part of the tool's contract: a script (or a stage gate) branches
/// on them rather than scraping text.
///
/// `probe`: 0 both halves present, 1 a half is definitively absent, 2 indeterminate.
/// `discover`: 0 at least one castable sink, 1 the run failed, 3 the run worked but
/// found no castable sink — distinct from 1 because the fix is on the sink's side
/// (open its Screen Mirroring mode), not this machine's.
const EXIT_READY: u8 = 0;
const EXIT_BLOCKED: u8 = 1;
const EXIT_UNKNOWN: u8 = 2;
const EXIT_NO_SINK: u8 = 3;

/// How long discovery is given to accumulate peers after it is requested.
const DEFAULT_SETTLE_SECS: u64 = 4;
/// The RTP port this caster receives media on, advertised to sinks.
const DEFAULT_RTP_PORT: u16 = 19_000;

fn main() -> ExitCode {
    tessera_env::init("info");
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("probe") => run_probe(),
        Some("discover") => run_discover(&args[1..]),
        Some("handshake") => run_handshake(&args[1..]),
        Some("cast") => run_cast(&args[1..]),
        Some("formats") => run_formats(&args[1..]),
        Some("sink") => run_sink(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("tessera-cast: unknown command '{other}'");
            print_usage();
            ExitCode::from(EXIT_UNKNOWN)
        }
    }
}

fn print_usage() {
    println!(
        "tessera-cast: Miracast caster for the Tessera desktop (ADR-0192)\n\n\
         USAGE:\n    \
         tessera-cast probe                    Report whether this machine can cast, and which half is missing\n    \
         tessera-cast discover [OPTIONS]       Enable P2P, advertise as a source, and list nearby sinks\n    \
         tessera-cast handshake (--host IP | --peer PEER) --video-formats FMT [OPTIONS]\n                                          \
         Run the WFD M1-M7 handshake against a sink and trace it\n    \
         tessera-cast cast (--host IP | --peer PEER) --video-formats FMT [OPTIONS]\n                                          \
         Stream H.264 video to a sink over RTSP/TS/RTP\n    \
         tessera-cast formats \"VALUE\"          Decode a wfd_video_formats value into tokens and bits\n    \
         tessera-cast sink [--listen ADDR]     Act as a scripted sink, so a caster can be exercised locally\n\n\
         PROBE is read-only: it never enables an adapter, starts a scan, or writes a property.\n\
         DISCOVER is an action: it enables the P2P role and registers a Wi-Fi Display source.\n\
         HANDSHAKE is a diagnostic: it proves the negotiation, not the media. No frames are sent.\n\
         CAST streams media: it completes the handshake and streams video frames over UDP/RTP.\n\n\
         HANDSHAKE / CAST OPTIONS:\n    \
         --host <IP>          The sink's RTSP address\n    \
         --peer <PEER>        Target peer by name, MAC address, or path substring\n    \
         --port <PORT>        The sink's RTSP port (default 7236, or from peer advertisement)\n    \
         --rtp-port <PORT>    Target sink RTP port (overriding negotiation)\n    \
         --settle <SECONDS>   Discover peers before matching --peer\n    \
         --timeout <SECONDS>  Socket and overall deadline (default 10)\n    \
         --video-formats FMT  The wfd_video_formats value (required; never defaulted, see below)\n    \
         --fps <FPS>          Frame rate (default 30)\n    \
         --duration <SECONDS> Stop streaming after this duration\n    \
         --max-frames <COUNT> Stop streaming after this many frames\n    \
         --source <SOURCE>    Video source: 'desktop' (live screen via portal + VA-API) or 'pattern' (test pattern)\n\n\
         DISCOVER OPTIONS:\n    \
         --settle <SECONDS>   Wait this long for peers after requesting discovery (default {DEFAULT_SETTLE_SECS})\n    \
         --port <PORT>        RTP port to advertise (default {DEFAULT_RTP_PORT})\n\n\
         EXIT CODES:\n    \
         0  probe: both halves present; discover: at least one castable sink; cast: completed stream\n    \
         1  a half is definitively absent, or the run failed\n    \
         2  probe: indeterminate (something could not be inspected)\n    \
         3  discover: no castable sink was found"
    );
}

fn resolve_sink_target(
    host: Option<String>,
    peer_needle: Option<String>,
    port: Option<u16>,
    settle_secs: Option<u64>,
) -> Result<(String, u16), ExitCode> {
    if host.is_some() && peer_needle.is_some() {
        eprintln!("tessera-cast: cannot specify both --host and --peer");
        return Err(ExitCode::from(EXIT_BLOCKED));
    }
    if host.is_none() && peer_needle.is_none() {
        eprintln!("tessera-cast: command needs either --host <sink-ip> or --peer <needle>");
        return Err(ExitCode::from(EXIT_BLOCKED));
    }

    if let Some(needle) = peer_needle {
        let radio = match tessera_cast::radio::iwd::IwdRadio::open() {
            Ok(radio) => radio,
            Err(error) => {
                eprintln!("tessera-cast: {error}");
                return Err(ExitCode::from(EXIT_BLOCKED));
            }
        };
        let peers = if let Some(settle) = settle_secs {
            match tessera_cast::discover::discover(
                &radio,
                DEFAULT_RTP_PORT,
                std::time::Duration::from_secs(settle),
            ) {
                Ok(report) => report.peers,
                Err(error) => {
                    eprintln!("tessera-cast: discovery failed: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            }
        } else {
            match tessera_cast::radio::P2pRadio::peers(&radio) {
                Ok(peers) => peers,
                Err(error) => {
                    eprintln!("tessera-cast: could not read peers: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            }
        };

        let matched = match tessera_cast::endpoint::match_peer(&peers, &needle) {
            tessera_cast::endpoint::PeerMatch::None => {
                eprintln!("tessera-cast: no peer matched '{needle}'");
                eprintln!(
                    "  run 'tessera-cast discover' or pass --settle <sec> to scan for nearby sinks"
                );
                return Err(ExitCode::from(EXIT_BLOCKED));
            }
            tessera_cast::endpoint::PeerMatch::Ambiguous(candidates) => {
                eprintln!("tessera-cast: multiple peers matched '{needle}':");
                for p in candidates {
                    let label = p
                        .name
                        .as_deref()
                        .or(p.device_address.as_deref())
                        .unwrap_or("(unnamed)");
                    eprintln!("  - {label} ({})", p.path);
                }
                eprintln!("please specify a more specific --peer value");
                return Err(ExitCode::from(EXIT_BLOCKED));
            }
            tessera_cast::endpoint::PeerMatch::ExactlyOne(p) => p.clone(),
        };

        let mut current_peer = matched;
        if !current_peer.connected {
            let label = current_peer
                .name
                .as_deref()
                .or(current_peer.device_address.as_deref())
                .unwrap_or(&current_peer.path);
            println!("forming P2P group with {label}...");
            if let Err(error) =
                tessera_cast::radio::P2pRadio::form_group(&radio, &current_peer.path)
            {
                eprintln!("tessera-cast: could not form P2P group: {error}");
                return Err(ExitCode::from(EXIT_BLOCKED));
            }
            // Refresh peers to get updated connection details
            if let Ok(updated) = tessera_cast::radio::P2pRadio::peers(&radio)
                && let Some(refreshed) = updated.into_iter().find(|p| p.path == current_peer.path)
            {
                current_peer = refreshed;
            }
        }

        let endpoint = tessera_cast::endpoint::resolve_live(
            &current_peer,
            current_peer.connected_interface.as_deref(),
        );
        match endpoint {
            tessera_cast::endpoint::Endpoint::Ready { host: h, port: p } => {
                let effective_port = port.unwrap_or(p);
                Ok((h, effective_port))
            }
            other => {
                let blocker = other
                    .blocker()
                    .unwrap_or_else(|| "endpoint not ready".to_owned());
                let label = current_peer.name.as_deref().unwrap_or(&current_peer.path);
                eprintln!("tessera-cast: peer {label}: {blocker}");
                Err(ExitCode::from(EXIT_BLOCKED))
            }
        }
    } else {
        Ok((host.unwrap(), port.unwrap_or(7236)))
    }
}

fn run_handshake(args: &[String]) -> ExitCode {
    let mut host: Option<String> = None;
    let mut peer_needle: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut timeout_secs: u64 = 10;
    let mut settle_secs: Option<u64> = None;
    let mut video_formats: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let Some(value) = args.get(index + 1) else {
            eprintln!("tessera-cast: {} needs a value", args[index]);
            return ExitCode::from(EXIT_BLOCKED);
        };
        match args[index].as_str() {
            "--host" => host = Some(value.clone()),
            "--peer" => peer_needle = Some(value.clone()),
            "--settle" => match value.parse() {
                Ok(parsed) => settle_secs = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --settle {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--port" => match value.parse() {
                Ok(parsed) => port = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --port {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--timeout" => match value.parse() {
                Ok(parsed) => timeout_secs = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --timeout {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--video-formats" => video_formats = Some(value.clone()),
            other => {
                eprintln!("tessera-cast: unknown handshake option '{other}'");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
        index += 2;
    }

    let (host, port) = match resolve_sink_target(host, peer_needle, port, settle_secs) {
        Ok(target) => target,
        Err(code) => return code,
    };

    // Refusing to default this is deliberate: the value is a spec bitfield that
    // cannot be guessed, and a wrong one makes a sink reject the session with no
    // diagnostic that distinguishes it from a radio failure.
    let Some(video_formats) = video_formats else {
        eprintln!(
            "tessera-cast: handshake needs --video-formats\n\
             \n\
             The wfd_video_formats value is a packed bitfield (resolutions, micro-steps,\n\
             profile/level). This tool will not invent it: a wrong bit makes the sink reject\n\
             the session indistinguishably from a radio failure. Take the value from the\n\
             sink's own M3 GET_PARAMETER (see --trace) or from a working caster's exchange."
        );
        return ExitCode::from(EXIT_BLOCKED);
    };

    let parameters = tessera_cast::wfd::parameters::SourceParameters::new(video_formats)
        .with_presentation_url(format!("rtsp://{host}/wfd1.0/streamid=0"))
        .with_rtp_port(DEFAULT_RTP_PORT);
    if let Err(error) = parameters.to_map() {
        eprintln!("tessera-cast: {error}");
        return ExitCode::from(EXIT_BLOCKED);
    }
    let mut session = tessera_cast::wfd::session::WfdSourceSession::new(parameters);

    let endpoint = format!("{host}:{port}");
    let stream = match std::net::TcpStream::connect(&endpoint) {
        Ok(stream) => stream,
        Err(error) => {
            // Say what was tried: the usual cause is that no P2P group exists
            // yet, not that the sink is broken.
            eprintln!("tessera-cast: could not connect to {endpoint}: {error}");
            eprintln!(
                "  a WFD session needs the sink reachable on its RTSP port; with Wi-Fi \
                 Direct that means the P2P group is formed first"
            );
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    let timeout = Some(std::time::Duration::from_secs(timeout_secs));
    if let Err(error) = stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
    {
        eprintln!("tessera-cast: could not set socket timeouts: {error}");
        return ExitCode::from(EXIT_BLOCKED);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let outcome = tessera_cast::cast::handshake(
        tessera_cast::wfd::transport::RtspTransport::new(stream),
        &mut session,
        deadline,
        64,
        // Keep serving after the sink asks for a session: a real sink still has
        // its reply, SETUP, PLAY and TEARDOWN to send.
        tessera_cast::cast::Drain::UntilQuiet(8),
    );
    let outcome = match outcome {
        Ok((outcome, _transport)) => outcome,
        Err(error) => {
            eprintln!("tessera-cast: {error}");
            print!("{}", session.trace().render());
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    println!("{}\n", outcome.describe());
    print!("{}", session.trace().render());
    if outcome.is_streaming() {
        println!(
            "note: media is not implemented, so the sink will not receive frames.\n\
             This command proves the handshake only."
        );
        ExitCode::from(EXIT_READY)
    } else {
        ExitCode::from(EXIT_BLOCKED)
    }
}

fn run_cast(args: &[String]) -> ExitCode {
    let mut host: Option<String> = None;
    let mut peer_needle: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut rtp_port: Option<u16> = None;
    let mut timeout_secs: u64 = 10;
    let mut settle_secs: Option<u64> = None;
    let mut video_formats: Option<String> = None;
    let mut fps: u32 = 30;
    let mut duration_secs: Option<u64> = None;
    let mut max_frames: Option<u64> = None;
    let mut source_kind = "desktop".to_owned();
    let mut index = 0;
    while index < args.len() {
        let Some(value) = args.get(index + 1) else {
            eprintln!("tessera-cast: {} needs a value", args[index]);
            return ExitCode::from(EXIT_BLOCKED);
        };
        match args[index].as_str() {
            "--host" => host = Some(value.clone()),
            "--peer" => peer_needle = Some(value.clone()),
            "--source" => source_kind = value.clone(),
            "--settle" => match value.parse() {
                Ok(parsed) => settle_secs = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --settle {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--port" => match value.parse() {
                Ok(parsed) => port = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --port {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--rtp-port" => match value.parse() {
                Ok(parsed) => rtp_port = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --rtp-port {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--timeout" => match value.parse() {
                Ok(parsed) => timeout_secs = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --timeout {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--video-formats" => video_formats = Some(value.clone()),
            "--fps" => match value.parse() {
                Ok(parsed) => fps = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --fps {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--duration" => match value.parse() {
                Ok(parsed) => duration_secs = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --duration {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--max-frames" => match value.parse() {
                Ok(parsed) => max_frames = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --max-frames {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            other => {
                eprintln!("tessera-cast: unknown cast option '{other}'");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
        index += 2;
    }

    let (host, port) = match resolve_sink_target(host, peer_needle, port, settle_secs) {
        Ok(target) => target,
        Err(code) => return code,
    };

    let Some(video_formats) = video_formats else {
        eprintln!(
            "tessera-cast: cast needs --video-formats\n\
             \n\
             The wfd_video_formats value is a packed bitfield (resolutions, micro-steps,\n\
             profile/level). This tool will not invent it: take the value from the\n\
             sink's own M3 GET_PARAMETER (see --trace) or from a working caster's exchange."
        );
        return ExitCode::from(EXIT_BLOCKED);
    };

    let parameters = tessera_cast::wfd::parameters::SourceParameters::new(video_formats)
        .with_presentation_url(format!("rtsp://{host}/wfd1.0/streamid=0"))
        .with_rtp_port(rtp_port.unwrap_or(DEFAULT_RTP_PORT));
    if let Err(error) = parameters.to_map() {
        eprintln!("tessera-cast: {error}");
        return ExitCode::from(EXIT_BLOCKED);
    }
    let mut session = tessera_cast::wfd::session::WfdSourceSession::new(parameters);

    let endpoint = format!("{host}:{port}");
    let stream = match std::net::TcpStream::connect(&endpoint) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("tessera-cast: could not connect to {endpoint}: {error}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    let timeout = Some(std::time::Duration::from_secs(timeout_secs));
    if let Err(error) = stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
    {
        eprintln!("tessera-cast: could not set socket timeouts: {error}");
        return ExitCode::from(EXIT_BLOCKED);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let outcome = tessera_cast::cast::handshake(
        tessera_cast::wfd::transport::RtspTransport::new(stream),
        &mut session,
        deadline,
        64,
        tessera_cast::cast::Drain::UntilQuiet(8),
    );
    let (outcome, transport) = match outcome {
        Ok(res) => res,
        Err(error) => {
            eprintln!("tessera-cast: handshake error: {error}");
            print!("{}", session.trace().render());
            return ExitCode::from(EXIT_BLOCKED);
        }
    };

    if !outcome.is_streaming() {
        eprintln!(
            "tessera-cast: sink did not accept streaming: {}",
            outcome.describe()
        );
        print!("{}", session.trace().render());
        return ExitCode::from(EXIT_BLOCKED);
    }

    let sink_rtp_port = rtp_port
        .or_else(|| session.sink_rtp_port())
        .unwrap_or(DEFAULT_RTP_PORT);

    let host_ip: std::net::IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(err) => {
            eprintln!("tessera-cast: invalid sink IP address '{host}': {err}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    let sink_rtp_target = std::net::SocketAddr::new(host_ip, sink_rtp_port);

    let sender = match tessera_cast::media::UdpMediaSender::bind_ephemeral(sink_rtp_target) {
        Ok(sender) => sender,
        Err(err) => {
            eprintln!("tessera-cast: could not bind UDP sender: {err}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };

    println!("handshake accepted: {}", outcome.describe());

    let config = tessera_cast::media::StreamConfig {
        fps,
        max_frames,
        max_duration: duration_secs.map(std::time::Duration::from_secs),
        ssrc: 0x1900_0000,
    };

    let _ = transport
        .stream()
        .set_read_timeout(Some(std::time::Duration::from_millis(5)));

    let (stats, mut transport) = if source_kind == "desktop" {
        match tessera_cast::media::DesktopCaptureSource::open(fps) {
            Ok(desktop_source) => {
                let res = desktop_source.resolution();
                println!(
                    "streaming live desktop ({}x{}) via VA-API H.264 to {sink_rtp_target} at {fps} fps",
                    res.0, res.1
                );
                println!("press Ctrl-C to stop streaming\n");
                tessera_cast::media::stream(transport, &mut session, sender, desktop_source, config)
            }
            Err(err) => {
                println!("could not start desktop portal capture: {err}");
                println!("falling back to 720p H.264 test pattern\n");
                println!("streaming H.264 test pattern to {sink_rtp_target} at {fps} fps");
                println!("press Ctrl-C to stop streaming\n");
                tessera_cast::media::stream(
                    transport,
                    &mut session,
                    sender,
                    tessera_cast::media::TestPatternSource::new(fps),
                    config,
                )
            }
        }
    } else {
        println!("streaming H.264 test pattern to {sink_rtp_target} at {fps} fps");
        println!("press Ctrl-C to stop streaming\n");
        tessera_cast::media::stream(
            transport,
            &mut session,
            sender,
            tessera_cast::media::TestPatternSource::new(fps),
            config,
        )
    };

    println!("stream completed: {}", stats.termination);
    println!("  duration: {:?}", stats.duration);
    println!("  frames sent: {}", stats.frames_sent);
    println!(
        "  data sent: {} bytes across {} RTP datagrams (~{} kbps)",
        stats.bytes_sent,
        stats.rtp_packets_sent,
        stats.average_bitrate_bps() / 1000
    );

    if session.phase() != tessera_cast::wfd::session::Phase::Terminated {
        let teardown = tessera_cast::wfd::rtsp::RtspMessage::request(
            tessera_cast::wfd::rtsp::Method::Teardown,
            &format!("rtsp://{host}/wfd1.0/streamid=0"),
            999,
            Vec::new(),
        );
        let _ = transport.send(&teardown);
    }

    ExitCode::from(EXIT_READY)
}

fn run_formats(args: &[String]) -> ExitCode {
    let Some(value) = args.first() else {
        eprintln!(
            "tessera-cast: formats needs a wfd_video_formats value\n\
             \n\
             Pass the value a sink advertised (or one a working caster sent). The tool\n\
             decodes it to offsets, bytes and bits, and shows the field names this build\n\
             assumes — marked unverified, because a wrong bit makes a sink reject a\n\
             session indistinguishably from a radio failure. It never rewrites the value."
        );
        return ExitCode::from(EXIT_BLOCKED);
    };
    match tessera_cast::wfd::formats::VideoFormats::parse(value) {
        Ok(formats) => {
            println!("wire form: {}\n", formats.to_wire());
            let canonical = formats.to_wire();
            if canonical != value.trim() {
                println!("note: the value was normalised (lower-case hex, single spaces)\n");
            }
            for line in formats.annotate() {
                println!("{line}");
            }
            println!(
                "\nthe declared field names are unverified: correct FIELD_SCHEMA against a real\n\
                 sink and the annotation becomes exact. The value itself is never rewritten."
            );
            ExitCode::from(EXIT_READY)
        }
        Err(error) => {
            eprintln!("tessera-cast: {error}");
            ExitCode::from(EXIT_BLOCKED)
        }
    }
}

fn run_sink(args: &[String]) -> ExitCode {
    let mut listen = "127.0.0.1:7236".to_owned();
    let mut rtp_port: Option<u16> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--listen" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("tessera-cast: --listen needs an address");
                    return ExitCode::from(EXIT_BLOCKED);
                };
                listen = value.clone();
                index += 2;
            }
            "--rtp-port" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("tessera-cast: --rtp-port needs a port");
                    return ExitCode::from(EXIT_BLOCKED);
                };
                match value.parse::<u16>() {
                    Ok(parsed) => rtp_port = Some(parsed),
                    Err(error) => {
                        eprintln!("tessera-cast: --rtp-port {value}: {error}");
                        return ExitCode::from(EXIT_BLOCKED);
                    }
                }
                index += 2;
            }
            other => {
                eprintln!("tessera-cast: unknown sink option '{other}'");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
    }
    let listener = match std::net::TcpListener::bind(&listen) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("tessera-cast: could not listen on {listen}: {error}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };

    let rtp_socket = if let Some(port) = rtp_port {
        let addr = format!("0.0.0.0:{port}");
        match std::net::UdpSocket::bind(&addr) {
            Ok(sock) => Some(sock),
            Err(error) => {
                eprintln!("tessera-cast: could not bind RTP socket on {addr}: {error}");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
    } else {
        None
    };

    let expect_setup_play = rtp_socket.is_some();

    eprintln!("scripted sink listening on {listen}; waiting for one caster");
    let (stream, peer) = match listener.accept() {
        Ok(accepted) => accepted,
        Err(error) => {
            eprintln!("tessera-cast: accept failed: {error}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    eprintln!("caster connected from {peer}\n");
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));

    let script = tessera_cast::sink::SinkScript {
        video_formats: "00 00 02 10 00000000 00000000 00 0000 0000 00 none none".to_owned(),
        expect_setup_play,
        receive_rtp_packets: None,
        rtp_socket,
    };

    let outcome = tessera_cast::sink::run(
        tessera_cast::wfd::transport::RtspTransport::new(stream),
        &script,
    );
    match outcome {
        Ok(report) => {
            println!("sink completed the negotiation\n");
            println!("cseq matched: {}", report.cseq_matched);
            println!(
                "source sent its own SETUP trigger: {}",
                report.source_triggered_setup
            );
            println!("messages received: {}", report.received.len());
            for (name, value) in &report.source_parameters {
                println!("  {name}: {value}");
            }
            if report.rtp_packets_received > 0 {
                println!(
                    "\nmedia stream received: {} RTP datagrams ({} bytes)",
                    report.rtp_packets_received, report.rtp_bytes_received
                );
            }
            ExitCode::from(EXIT_READY)
        }
        Err(error) => {
            eprintln!("tessera-cast: {error}");
            ExitCode::from(EXIT_BLOCKED)
        }
    }
}

fn run_probe() -> ExitCode {
    let report = tessera_cast::observe::probe();
    print!("{}", report.render());
    match report.blocker() {
        tessera_cast::probe::Blocker::None => ExitCode::from(EXIT_READY),
        tessera_cast::probe::Blocker::Radio | tessera_cast::probe::Blocker::Pipeline => {
            ExitCode::from(EXIT_BLOCKED)
        }
        tessera_cast::probe::Blocker::Unknown => ExitCode::from(EXIT_UNKNOWN),
    }
}

fn run_discover(args: &[String]) -> ExitCode {
    let mut settle = DEFAULT_SETTLE_SECS;
    let mut port = DEFAULT_RTP_PORT;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--settle" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("tessera-cast: --settle needs a value");
                    return ExitCode::from(EXIT_BLOCKED);
                };
                match value.parse::<u64>() {
                    Ok(seconds) => settle = seconds,
                    Err(error) => {
                        eprintln!("tessera-cast: --settle {value}: {error}");
                        return ExitCode::from(EXIT_BLOCKED);
                    }
                }
                index += 2;
            }
            "--port" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("tessera-cast: --port needs a value");
                    return ExitCode::from(EXIT_BLOCKED);
                };
                match value.parse::<u16>() {
                    Ok(value) => port = value,
                    Err(error) => {
                        eprintln!("tessera-cast: --port {value}: {error}");
                        return ExitCode::from(EXIT_BLOCKED);
                    }
                }
                index += 2;
            }
            other => {
                eprintln!("tessera-cast: unknown discover option '{other}'");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
    }

    let radio = match tessera_cast::radio::iwd::IwdRadio::open() {
        Ok(radio) => radio,
        Err(error) => {
            // The radio could not even be opened: report the exact reason rather
            // than a generic failure, because each cause has a different fix.
            eprintln!("tessera-cast: {error}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    match tessera_cast::discover::discover(&radio, port, std::time::Duration::from_secs(settle)) {
        Ok(report) => {
            print!("{}", report.render());
            if report.castable().count() > 0 {
                ExitCode::from(EXIT_READY)
            } else {
                ExitCode::from(EXIT_NO_SINK)
            }
        }
        Err(error) => {
            eprintln!("tessera-cast: {error}");
            ExitCode::from(EXIT_BLOCKED)
        }
    }
}
