use std::net::{TcpStream, UdpSocket};
use std::process::ExitCode;
use std::time::Duration;

use tessera_cast::cast::Opening;
use tessera_cast::media::AudioSourceKind;
use tessera_cast::session::{self, MediaSource, SessionEvent, SessionOptions};
use tessera_cast::wfd::transport::RtspTransport;

/// Exit codes are part of the tool's contract: a script (or a stage gate) branches
/// on them rather than scraping text.
const EXIT_READY: u8 = 0;
const EXIT_BLOCKED: u8 = 1;
const EXIT_UNKNOWN: u8 = 2;
const EXIT_NO_SINK: u8 = 3;

/// How long discovery is given to accumulate peers after it is requested.
const DEFAULT_SETTLE_SECS: u64 = 4;
/// The RTP port this source asks the sink to use when the sink names none.
const DEFAULT_RTP_PORT: u16 = 19_000;
/// How long the source waits for the sink to open the RTSP connection.
const DEFAULT_ACCEPT_TIMEOUT_SECS: u64 = 60;
/// The WFD session-management (RTSP) port. The **source listens here**; the sink
/// connects to it. This is the direction Miracast specifies.
const DEFAULT_CONTROL_PORT: u16 = tessera_cast::firewall::WFD_CONTROL_PORT;

fn main() -> ExitCode {
    tessera_env::init("info");
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("probe") => run_probe(),
        Some("discover") => run_discover(&args[1..]),
        Some("doctor") => run_doctor(&args[1..]),
        Some("cast" | "serve") => run_serve_command(&args[1..], true),
        Some("handshake") => run_serve_command(&args[1..], false),
        Some("formats") => run_formats(&args[1..]),
        Some("sink") => run_sink(&args[1..]),
        Some("record") => run_record(&args[1..]),
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
        "tessera-cast: Miracast and desktop recording for the Tessera desktop (ADR-0186, ADR-0192)\n\n\
         Miracast makes the SOURCE the RTSP server: the source listens on the control\n\
         port (7236 by default), the sink connects to it, and the sink sends the first\n\
         OPTIONS. `cast` and `handshake` therefore listen; they never dial the sink.\n\n\
         USAGE:\n    \
         tessera-cast probe                    Report whether this machine can host a cast\n    \
         tessera-cast discover [OPTIONS]       Enable P2P, advertise as a source, list sinks\n    \
         tessera-cast doctor [OPTIONS]         Probe, firewall, and discovery in one evidence report\n    \
         tessera-cast cast [OPTIONS]           Serve a WFD session and stream H.264 media\n    \
         tessera-cast handshake [OPTIONS]      Serve a WFD session and trace the negotiation\n    \
         tessera-cast formats \"VALUE\"          Decode a wfd_video_formats value\n    \
         tessera-cast sink [OPTIONS]           Act as a scripted SINK that connects to a source\n    \
         tessera-cast record [OPTIONS]         Record screen to video file (MP4/MKV/TS)\n\n\
         PROBE is read-only: it never enables an adapter, starts a scan, or writes a property.\n\
         DISCOVER is an action: it enables the P2P role and registers a Wi-Fi Display source.\n\
         HANDSHAKE is a diagnostic: it proves the negotiation, not the media.\n\
         CAST streams media: it serves the handshake and sends video over UDP/RTP.\n\n\
         CAST / HANDSHAKE OPTIONS:\n    \
         --bind <ADDR>          Address to listen on (default 0.0.0.0)\n    \
         --port <PORT>          RTSP control port to serve and advertise (default 7236)\n    \
         --peer <PEER>          Sink to form a P2P group with (name, MAC, or D-Bus path)\n    \
         --settle <SECONDS>     Peer-discovery settle time before matching --peer (default 4)\n    \
         --video-formats FMT    The wfd_video_formats value (required; never defaulted)\n    \
         --audio                Share audio with the sink (AAC-LC 48 kHz stereo)\n    \
         --audio-source <WHO>   'system' (default sink monitor) or 'microphone' (default system)\n    \
         --audio-bitrate <KBPS> AAC bitrate in kbit/s (default 128)\n    \
         --rtp-port <PORT>      RTP port to use when the sink does not name one (default 19000)\n    \
         --timeout <SECONDS>    How long to wait for the sink to connect, and the session deadline\n    \
         --fps <FPS>            Frame rate (default 30)\n    \
         --source <SOURCE>      'desktop' (portal + VA-API) or 'pattern' (default desktop)\n    \
         --opening <WHO>        'sink' (default) or 'source' (source drives M1–M6)\n    \
         --duration <SECONDS>   Stop streaming after this duration\n    \
         --max-frames <COUNT>   Stop streaming after this many frames\n    \
         --ts-file <PATH>       Diagnostic: send a pre-muxed MPEG-TS file over the real RTP path\n    \
         --ts-seconds <SECONDS> Nominal duration of --ts-file for pacing (default 30)\n\n\
         DISCOVER OPTIONS:\n    \
         --settle <SECONDS>     Wait this long for peers after requesting discovery (default 4)\n    \
         --port <PORT>          RTSP control port to advertise (default 7236)\n\n\
         SINK OPTIONS:\n    \
         --connect <ADDR>       Source RTSP address (default 127.0.0.1:7236)\n    \
         --rtp-port <PORT>      Bind this UDP port for RTP and advertise it in SETUP\n    \
         --play                 Continue through SETUP and PLAY\n\n\
         EXIT CODES:\n    \
         0  probe: both halves present; discover: at least one sink; cast: completed stream\n    \
         1  a half is definitively absent, a firewall blocks the sink, or the run failed\n    \
         2  probe: indeterminate (something could not be inspected)\n    \
         3  discover: no castable sink was found"
    );
}

#[derive(Debug, Clone)]
struct ServeOptions {
    bind: String,
    control_port: u16,
    peer: Option<String>,
    settle: u64,
    video_formats: Option<String>,
    audio: bool,
    audio_source: String,
    audio_bitrate: u32,
    rtp_port: Option<u16>,
    timeout: u64,
    fps: u32,
    source: String,
    opening: String,
    duration: Option<u64>,
    max_frames: Option<u64>,
    ts_file: Option<String>,
    ts_seconds: u64,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0".to_owned(),
            control_port: DEFAULT_CONTROL_PORT,
            peer: None,
            settle: DEFAULT_SETTLE_SECS,
            video_formats: None,
            audio: false,
            audio_source: "system".to_owned(),
            audio_bitrate: 128,
            rtp_port: None,
            timeout: DEFAULT_ACCEPT_TIMEOUT_SECS,
            fps: 30,
            source: "desktop".to_owned(),
            opening: "sink".to_owned(),
            duration: None,
            max_frames: None,
            ts_file: None,
            ts_seconds: 30,
        }
    }
}

fn parse_serve_options(args: &[String]) -> Result<ServeOptions, ExitCode> {
    let mut options = ServeOptions::default();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--audio" {
            options.audio = true;
            index += 1;
            continue;
        }
        let Some(value) = args.get(index + 1) else {
            eprintln!("tessera-cast: {} needs a value", args[index]);
            return Err(ExitCode::from(EXIT_BLOCKED));
        };
        match args[index].as_str() {
            "--bind" => options.bind = value.clone(),
            "--peer" => options.peer = Some(value.clone()),
            "--video-formats" => options.video_formats = Some(value.clone()),
            "--ts-file" => options.ts_file = Some(value.clone()),
            "--ts-seconds" => match value.parse() {
                Ok(seconds) if seconds > 0 => options.ts_seconds = seconds,
                _ => {
                    eprintln!("tessera-cast: --ts-seconds {value}: expected a positive integer");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--audio-source" => match value.as_str() {
                "system" | "microphone" => options.audio_source = value.clone(),
                other => {
                    eprintln!(
                        "tessera-cast: --audio-source {other}: expected 'system' or 'microphone'"
                    );
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--audio-bitrate" => match value.parse() {
                Ok(bitrate) if (32..=512).contains(&bitrate) => options.audio_bitrate = bitrate,
                _ => {
                    eprintln!("tessera-cast: --audio-bitrate {value}: expected 32..=512");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--source" => options.source = value.clone(),
            "--opening" => match value.as_str() {
                "sink" | "source" => options.opening = value.clone(),
                other => {
                    eprintln!("tessera-cast: --opening {other}: expected 'sink' or 'source'");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--port" => match value.parse() {
                Ok(parsed) => options.control_port = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --port {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--settle" => match value.parse() {
                Ok(parsed) => options.settle = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --settle {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--rtp-port" => match value.parse() {
                Ok(parsed) => options.rtp_port = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --rtp-port {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--timeout" => match value.parse() {
                Ok(parsed) => options.timeout = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --timeout {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--fps" => match value.parse() {
                Ok(parsed) => options.fps = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --fps {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--duration" => match value.parse() {
                Ok(parsed) => options.duration = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --duration {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            "--max-frames" => match value.parse() {
                Ok(parsed) => options.max_frames = Some(parsed),
                Err(error) => {
                    eprintln!("tessera-cast: --max-frames {value}: {error}");
                    return Err(ExitCode::from(EXIT_BLOCKED));
                }
            },
            other => {
                eprintln!("tessera-cast: unknown option '{other}'");
                return Err(ExitCode::from(EXIT_BLOCKED));
            }
        }
        index += 2;
    }
    Ok(options)
}

fn run_serve_command(args: &[String], media: bool) -> ExitCode {
    let options = match parse_serve_options(args) {
        Ok(options) => options,
        Err(code) => return code,
    };
    let Some(video_formats) = options.video_formats.clone() else {
        eprintln!(
            "tessera-cast: {} needs --video-formats\n\
             \n\
             The wfd_video_formats value is a packed bitfield (resolutions, micro-steps,\n\
             profile/level). This tool will not invent it: take the value from a working\n\
             caster's exchange, or from the sink's own M3 GET_PARAMETER trace.",
            if media { "cast" } else { "handshake" }
        );
        return ExitCode::from(EXIT_BLOCKED);
    };
    if video_formats.trim().is_empty() {
        eprintln!(
            "tessera-cast: --video-formats is empty\n\
             \n\
             If you wrote `VF=\"...\" tessera-cast ... --video-formats \"$VF\"`, that is an\n\
             environment assignment for that command: `$VF` does not expand in the same\n\
             command. Put the assignment on its own line, or pass the value inline:\n\
             \n  VF=\"38 00 01 01 f9cf94a0 00000000 00000000 00 0000 0000 00 none none\"\n  \
             ./target/debug/tessera-cast cast ... --video-formats \"$VF\""
        );
        return ExitCode::from(EXIT_BLOCKED);
    }
    let control_port = options.control_port;
    let session_options = SessionOptions {
        bind: options.bind,
        control_port,
        peer: options.peer,
        settle: Duration::from_secs(options.settle),
        video_formats,
        audio: options.audio,
        audio_source: if options.audio_source == "microphone" {
            AudioSourceKind::Microphone
        } else {
            AudioSourceKind::System
        },
        audio_bitrate_kbps: options.audio_bitrate,
        rtp_port: options.rtp_port.unwrap_or(DEFAULT_RTP_PORT),
        timeout: Duration::from_secs(options.timeout),
        fps: options.fps,
        source: if options.source == "pattern" {
            MediaSource::Pattern
        } else {
            MediaSource::Desktop
        },
        duration: options.duration.map(Duration::from_secs),
        max_frames: options.max_frames,
        stream_media: media,
        ts_file: options.ts_file.map(std::path::PathBuf::from),
        ts_seconds: options.ts_seconds,
        opening: if options.opening == "source" {
            Opening::Initiator
        } else {
            Opening::Sink
        },
        stop: None,
    };

    let result = session::run(&session_options, &mut |event| match event {
        SessionEvent::Listening(address) => {
            println!("source RTSP server listening on {address}");
            println!("  (Miracast: the sink connects here; this source never dials the sink)\n");
        }
        SessionEvent::FirewallBlocked(description) => {
            println!("inbound tcp/{control_port}        {description}");
            eprintln!(
                "warning: the host firewall will drop the sink's connection.\n\
                 add an ingress rule for the P2P interface, for example:\n\
                 \n  sudo nft add rule inet filter input iifname \"wlan0-p2p*\" accept\n"
            );
        }
        SessionEvent::SinkConnected(address) => println!("sink connected from {address}\n"),
        SessionEvent::Negotiating => println!("negotiating Miracast..."),
        SessionEvent::Streaming => println!("sink accepted the handshake\n"),
        SessionEvent::MediaEndpoint { local, target } => {
            println!("media: RTP {local} -> {target}\n");
        }
        SessionEvent::Trace(trace) => print!("{trace}"),
        SessionEvent::MediaFallback(error) => {
            println!("could not start desktop portal capture: {error}");
            println!("falling back to the 720p H.264 test pattern\n");
        }
        SessionEvent::AudioActive(codec) => println!("audio: {codec}\n"),
        SessionEvent::AudioUnavailable(reason) => println!("audio unavailable: {reason}\n"),
        SessionEvent::RtcpUnavailable(reason) => println!("rtcp unavailable: {reason}\n"),
        SessionEvent::Stats {
            frames_sent,
            audio_frames_sent,
            bytes_sent,
        } => println!(
            "stream completed: {frames_sent} frames, {audio_frames_sent} audio frames, {bytes_sent} bytes"
        ),
        SessionEvent::Stopped(reason) => println!("session stopped: {reason}"),
        SessionEvent::Failed(_) => {}
    });

    match result {
        Ok(()) => ExitCode::from(EXIT_READY),
        Err(error) => {
            eprintln!("tessera-cast: {error}");
            ExitCode::from(EXIT_BLOCKED)
        }
    }
}

fn run_sink(args: &[String]) -> ExitCode {
    let mut connect = "127.0.0.1:7236".to_owned();
    let mut rtp_port: Option<u16> = None;
    let mut play = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--connect" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("tessera-cast: --connect needs an address");
                    return ExitCode::from(EXIT_BLOCKED);
                };
                connect = value.clone();
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
            "--play" => {
                play = true;
                index += 1;
            }
            other => {
                eprintln!("tessera-cast: unknown sink option '{other}'");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
    }

    let stream = match TcpStream::connect(&connect) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("tessera-cast: could not connect to {connect}: {error}");
            eprintln!("  the SOURCE listens; start 'tessera-cast cast' first");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

    let rtp_socket = match rtp_port {
        Some(port) => match UdpSocket::bind(("0.0.0.0", port)) {
            Ok(socket) => Some(socket),
            Err(error) => {
                eprintln!("tessera-cast: could not bind the RTP socket on {port}: {error}");
                return ExitCode::from(EXIT_BLOCKED);
            }
        },
        None => None,
    };

    let script = tessera_cast::sink::SinkScript {
        expect_setup_play: play,
        rtp_socket,
        ..tessera_cast::sink::SinkScript::default()
    };

    match tessera_cast::sink::run_client(RtspTransport::new(stream), &script) {
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
                    "\nmedia received: {} RTP datagrams ({} bytes)",
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
        tessera_cast::probe::Blocker::Radio
        | tessera_cast::probe::Blocker::Pipeline
        | tessera_cast::probe::Blocker::InboundFirewall => ExitCode::from(EXIT_BLOCKED),
        tessera_cast::probe::Blocker::Unknown => ExitCode::from(EXIT_UNKNOWN),
    }
}

fn run_discover(args: &[String]) -> ExitCode {
    let mut settle = DEFAULT_SETTLE_SECS;
    let mut port = DEFAULT_CONTROL_PORT;
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

    let inbound = tessera_cast::firewall::observe(port);
    println!("inbound tcp/{port}        {}", inbound.describe());
    if inbound.is_blocked() {
        println!(
            "  the sink's RTSP connection will be dropped; add an ingress rule, e.g.\n  \
             sudo nft add rule inet filter input iifname \"wlan0-p2p*\" accept"
        );
    }

    let radio = match tessera_cast::radio::iwd::IwdRadio::open() {
        Ok(radio) => radio,
        Err(error) => {
            eprintln!("tessera-cast: {error}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    match tessera_cast::discover::discover(&radio, port, Duration::from_secs(settle)) {
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

fn run_doctor(args: &[String]) -> ExitCode {
    let mut settle = 8u64;
    let mut port = DEFAULT_CONTROL_PORT;
    let mut index = 0;
    while index < args.len() {
        let Some(value) = args.get(index + 1) else {
            eprintln!("tessera-cast: {} needs a value", args[index]);
            return ExitCode::from(EXIT_BLOCKED);
        };
        match args[index].as_str() {
            "--settle" => match value.parse() {
                Ok(parsed) => settle = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --settle {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            "--port" => match value.parse() {
                Ok(parsed) => port = parsed,
                Err(error) => {
                    eprintln!("tessera-cast: --port {value}: {error}");
                    return ExitCode::from(EXIT_BLOCKED);
                }
            },
            other => {
                eprintln!("tessera-cast: unknown doctor option '{other}'");
                return ExitCode::from(EXIT_BLOCKED);
            }
        }
        index += 2;
    }

    println!("tessera-cast doctor\n");
    let probe = tessera_cast::observe::probe();
    print!("{}", probe.render());
    println!();

    let inbound = tessera_cast::firewall::observe(port);
    println!("firewall tcp/{port}      {}", inbound.describe());
    if inbound.is_blocked() {
        println!("  fix: sudo nft add rule inet filter input iifname \"wlan0-p2p*\" accept");
    } else if matches!(
        inbound,
        tessera_cast::firewall::InboundVerdict::Unverified(_)
    ) {
        println!("  inspect: sudo nft list ruleset");
    }
    println!();

    let radio = match tessera_cast::radio::iwd::IwdRadio::open() {
        Ok(radio) => radio,
        Err(error) => {
            eprintln!("discovery unavailable: {error}");
            return ExitCode::from(EXIT_BLOCKED);
        }
    };
    match tessera_cast::discover::discover(&radio, port, Duration::from_secs(settle)) {
        Ok(report) => {
            print!("{}", report.render());
            if let Some(peer) = report.castable().next() {
                println!(
                    "\nnext: tessera-cast cast --peer {} --settle {settle} \\\n      \
                     --source pattern --video-formats <value> --timeout 60",
                    peer.path
                );
            }
        }
        Err(error) => eprintln!("discovery failed: {error}"),
    }

    match probe.blocker() {
        tessera_cast::probe::Blocker::None => ExitCode::from(EXIT_READY),
        tessera_cast::probe::Blocker::Unknown => ExitCode::from(EXIT_UNKNOWN),
        _ => ExitCode::from(EXIT_BLOCKED),
    }
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

fn run_record(args: &[String]) -> ExitCode {
    let mut options = tessera_cast::media::RecordOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("tessera-cast record: missing argument for --output");
                    return ExitCode::from(EXIT_BLOCKED);
                }
                options.output_path = std::path::PathBuf::from(&args[i]);
            }
            "--fps" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("tessera-cast record: missing argument for --fps");
                    return ExitCode::from(EXIT_BLOCKED);
                }
                match args[i].parse::<u32>() {
                    Ok(fps) if fps > 0 && fps <= 240 => options.fps = fps,
                    _ => {
                        eprintln!("tessera-cast record: invalid fps value (expected 1..240)");
                        return ExitCode::from(EXIT_BLOCKED);
                    }
                }
            }
            "--virtual" => {
                options.virtual_source = true;
            }
            "--mirror" => {
                options.virtual_source = false;
            }
            "-d" | "--duration" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("tessera-cast record: missing argument for --duration");
                    return ExitCode::from(EXIT_BLOCKED);
                }
                match args[i].parse::<u64>() {
                    Ok(secs) if secs > 0 => options.duration = Some(Duration::from_secs(secs)),
                    _ => {
                        eprintln!("tessera-cast record: invalid duration (expected seconds > 0)");
                        return ExitCode::from(EXIT_BLOCKED);
                    }
                }
            }
            "--help" | "-h" => {
                println!(
                    "USAGE:\n    tessera-cast record [OPTIONS]\n\n\
                     OPTIONS:\n    \
                     -o, --output <PATH>    Destination video file (default: recording.mp4)\n    \
                     --fps <FPS>            Frame rate in frames/sec (default: 30)\n    \
                     --virtual              Record an isolated virtual extend output (ADR-0186)\n    \
                     --mirror               Record the physical desktop monitor (default)\n    \
                     -d, --duration <SECS>  Record for fixed duration, then stop automatically\n"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tessera-cast record: unknown option '{other}'");
                return ExitCode::from(EXIT_UNKNOWN);
            }
        }
        i += 1;
    }

    use std::io::IsTerminal;
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag_clone = stop_flag.clone();
    if std::io::stdin().is_terminal() {
        println!("Recording started. Press [Enter] to stop recording and finalize video file...");
        std::thread::spawn(move || {
            let mut line = String::new();
            if let Ok(n) = std::io::stdin().read_line(&mut line) {
                if n > 0 {
                    stop_flag_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
    } else if options.duration.is_none() {
        // In non-interactive mode without explicit duration, wait for stdin EOF (e.g. closed pipe)
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            stop_flag_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    match tessera_cast::media::record_screen(options, Some(stop_flag)) {
        Ok(report) => {
            println!(
                "Recording saved: {} ({}x{} @ {}fps, {:.2}s, {} bytes)",
                report.output_path.display(),
                report.resolution.0,
                report.resolution.1,
                report.fps,
                report.duration.as_secs_f64(),
                report.bytes_written
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("tessera-cast record failed: {err}");
            ExitCode::from(EXIT_BLOCKED)
        }
    }
}
