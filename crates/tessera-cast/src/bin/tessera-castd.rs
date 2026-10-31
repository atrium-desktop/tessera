use std::process::ExitCode;
use std::time::Duration;

use tessera_cast::daemon::{self, DaemonOptions};
use tessera_cast::session::MediaSource;

fn main() -> ExitCode {
    tessera_env::init("info");
    let mut options = DaemonOptions::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--audio" {
            options.audio = true;
            index += 1;
            continue;
        }
        let Some(value) = args.get(index + 1) else {
            eprintln!("tessera-castd: {} needs a value", args[index]);
            return ExitCode::from(1);
        };
        match args[index].as_str() {
            "--socket" => options.socket = value.into(),
            "--video-formats" => options.video_formats = value.clone(),
            "--audio-source" => {
                options.audio_source = match value.as_str() {
                    "system" => tessera_cast::media::AudioSourceKind::System,
                    "microphone" => tessera_cast::media::AudioSourceKind::Microphone,
                    other => {
                        eprintln!(
                            "tessera-castd: --audio-source {other}: expected 'system' or 'microphone'"
                        );
                        return ExitCode::from(1);
                    }
                }
            }
            "--audio-bitrate" => match value.parse() {
                Ok(bitrate) if (32..=512).contains(&bitrate) => {
                    options.audio_bitrate_kbps = bitrate
                }
                _ => {
                    eprintln!("tessera-castd: --audio-bitrate {value}: expected 32..=512");
                    return ExitCode::from(1);
                }
            },
            "--port" => match value.parse() {
                Ok(port) => options.control_port = port,
                Err(error) => {
                    eprintln!("tessera-castd: --port {value}: {error}");
                    return ExitCode::from(1);
                }
            },
            "--rtp-port" => match value.parse() {
                Ok(port) => options.rtp_port = port,
                Err(error) => {
                    eprintln!("tessera-castd: --rtp-port {value}: {error}");
                    return ExitCode::from(1);
                }
            },
            "--settle" => match value.parse() {
                Ok(seconds) => options.settle = Duration::from_secs(seconds),
                Err(error) => {
                    eprintln!("tessera-castd: --settle {value}: {error}");
                    return ExitCode::from(1);
                }
            },
            "--timeout" => match value.parse() {
                Ok(seconds) => options.session_timeout = Duration::from_secs(seconds),
                Err(error) => {
                    eprintln!("tessera-castd: --timeout {value}: {error}");
                    return ExitCode::from(1);
                }
            },
            "--fps" => match value.parse() {
                Ok(fps) => options.fps = fps,
                Err(error) => {
                    eprintln!("tessera-castd: --fps {value}: {error}");
                    return ExitCode::from(1);
                }
            },
            "--source" => {
                options.source = match value.as_str() {
                    "pattern" => MediaSource::Pattern,
                    "desktop" => MediaSource::Desktop,
                    other => {
                        eprintln!(
                            "tessera-castd: --source {other}: expected 'desktop' or 'pattern'"
                        );
                        return ExitCode::from(1);
                    }
                }
            }
            "--opening" => {
                options.opening = match value.as_str() {
                    "sink" => tessera_cast::cast::Opening::Sink,
                    "source" => tessera_cast::cast::Opening::Initiator,
                    other => {
                        eprintln!("tessera-castd: --opening {other}: expected 'sink' or 'source'");
                        return ExitCode::from(1);
                    }
                }
            }
            "help" | "--help" | "-h" => {
                println!(
                    "tessera-castd: the out-of-process Miracast cast service (ADR-0188, ADR-0192)\n\n\
                     USAGE:\n    \
                     tessera-castd [--socket PATH] [--video-formats FMT] [--port 7236]\n\
                     \x20              [--rtp-port 19000] [--settle 4] [--timeout 60]\n\
                     \x20              [--fps 30] [--source desktop|pattern]\n\
                     \x20              [--opening sink|source] [--audio]\n\
                     \x20              [--audio-source system|microphone] [--audio-bitrate 128]\n\n\
                     The service listens on a per-user Unix socket (default\n\
                     $XDG_RUNTIME_DIR/tessera-castd.sock), owns discovery and sessions,\n\
                     and never dials a sink: Miracast's source is the RTSP server.\n\
                     --audio is opt-in and shares the selected PipeWire node.\n\
                     --video-formats defaults to the validated 720p30 value."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tessera-castd: unknown option '{other}'");
                return ExitCode::from(1);
            }
        }
        index += 2;
    }

    match daemon::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tessera-castd: {error}");
            ExitCode::from(1)
        }
    }
}
