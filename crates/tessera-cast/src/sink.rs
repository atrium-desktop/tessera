//! A scripted Wi-Fi Display **sink**: the other side of the handshake.
//!
//! Two uses, one implementation:
//!
//! - **Tests.** `cargo test` drives the real caster loop against this over a
//!   socketpair, so the whole M1–M7 exchange is exercised on every run with no
//!   radio, no GPU, and no television. That is what keeps the iteration loop
//!   cheap: a fix costs a compile, not a hardware session.
//! - **A manual harness.** `tessera-cast sink --listen 127.0.0.1:7236` makes a
//!   local fake sink, so the caster's full trace can be inspected without a sink
//!   device present.
//!
//! A scripted sink proves the *caster's* behaviour. It cannot prove that a real
//! sink accepts our parameter values — that still needs hardware
//! ([ADR-0192](../docs/adr/0192-first-party-miracast-caster.md) §8), which is why
//! it reports what it received instead of asserting the values are correct.

use std::io::{Read, Write};

use crate::wfd::rtsp::{Method, RtspMessage, StartLine};
use crate::wfd::transport::{RtspTransport, TransportError};

/// What the sink offers the caster during negotiation.
#[derive(Debug)]
pub struct SinkScript {
    /// The `wfd_video_formats` value this sink advertises.
    pub video_formats: String,
    /// Whether to go on to `SETUP` and `PLAY` after the trigger exchange.
    pub expect_setup_play: bool,
    /// Target number of RTP datagrams to receive after PLAY before concluding.
    pub receive_rtp_packets: Option<usize>,
    /// Optional UDP socket to receive RTP media on.
    pub rtp_socket: Option<std::net::UdpSocket>,
}

impl Default for SinkScript {
    fn default() -> Self {
        Self {
            video_formats: "00 00 02 10 00000000 00000000 00 0000 0000 00 none none".to_owned(),
            expect_setup_play: false,
            receive_rtp_packets: None,
            rtp_socket: None,
        }
    }
}

/// What the sink observed, for a caller to assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkReport {
    /// Every message the sink received, in order.
    pub received: Vec<RtspMessage>,
    /// The `wfd_*` parameters the source advertised in its `GET_PARAMETER` reply.
    pub source_parameters: Vec<(String, String)>,
    /// Whether every response the sink received echoed its `CSeq`.
    pub cseq_matched: bool,
    /// Whether the source sent the `SETUP` trigger a source must send after the
    /// sink asks for one.
    pub source_triggered_setup: bool,
    /// Total RTP packets received by the sink, if an RTP socket was monitored.
    pub rtp_packets_received: u64,
    /// Total RTP bytes received.
    pub rtp_bytes_received: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkError {
    Transport(TransportError),
    /// The source answered with a message the sink could not use.
    Unexpected(String),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Unexpected(reason) => write!(formatter, "sink: {reason}"),
        }
    }
}

impl std::error::Error for SinkError {}

impl From<TransportError> for SinkError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

/// Run the sink side of a WFD negotiation to completion.
///
/// The sink drives the nominal order (it is the side that asks for a session), and
/// it answers any request the source interleaves — a source is allowed to send its
/// trigger while the sink is waiting for a reply.
pub fn run<S: Read + Write>(
    mut transport: RtspTransport<S>,
    script: &SinkScript,
) -> Result<SinkReport, SinkError> {
    let mut report = SinkReport {
        received: Vec::new(),
        source_parameters: Vec::new(),
        cseq_matched: true,
        source_triggered_setup: false,
        rtp_packets_received: 0,
        rtp_bytes_received: 0,
    };

    // 1. The source opens with OPTIONS. Answer it.
    let opening = next(&mut transport, &mut report)?;
    if !matches!(
        &opening.start,
        StartLine::Request {
            method: Method::Options,
            ..
        }
    ) {
        return Err(SinkError::Unexpected(format!(
            "expected the source to open with OPTIONS, got {:?}",
            opening.start
        )));
    }
    reply(&mut transport, opening.header("cseq"), Vec::new())?;

    // 2. Ask for the source's capabilities.
    transport.send(&RtspMessage::request(
        Method::GetParameter,
        "rtsp://localhost/wfd1.0",
        1,
        Vec::new(),
    ))?;
    let capabilities = expect_response(&mut transport, &mut report, "1")?;
    report.source_parameters = capabilities
        .parameters()
        .map(|parameters| parameters.into_iter().collect())
        .unwrap_or_default();

    // 3. Ask the source to start a session.
    let mut body = std::collections::BTreeMap::new();
    body.insert("wfd_trigger_method".to_owned(), "SETUP".to_owned());
    transport.send(&RtspMessage::request(
        Method::SetParameter,
        "rtsp://localhost/wfd1.0",
        2,
        crate::wfd::rtsp::write_parameters(&body),
    ))?;
    let _ = expect_response(&mut transport, &mut report, "2")?;

    // 4. The source must now send its own SETUP trigger; answer it.
    let trigger = next(&mut transport, &mut report)?;
    if let Ok(parameters) = trigger.parameters()
        && parameters
            .get("wfd_trigger_method")
            .is_some_and(|value| value == "SETUP")
    {
        report.source_triggered_setup = true;
    }
    reply(&mut transport, trigger.header("cseq"), Vec::new())?;

    // 5. Optionally go on to SETUP and PLAY. The caster does not implement the
    //    streaming loop, so by default it stops after the trigger exchange and
    //    asking further would block; the flag makes that explicit rather than
    //    hiding it in a timeout.
    if script.expect_setup_play {
        for (cseq, method) in [(3u32, Method::Setup), (4u32, Method::Play)] {
            transport.send(&RtspMessage::request(
                method,
                "rtsp://localhost/wfd1.0/streamid=0",
                cseq,
                Vec::new(),
            ))?;
            let expected = cseq.to_string();
            let _ = expect_response(&mut transport, &mut report, &expected)?;
        }

        if let Some(socket) = &script.rtp_socket {
            let mut buf = [0u8; 1500];
            let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(500)));
            let max = script.receive_rtp_packets.unwrap_or(usize::MAX);
            while (report.rtp_packets_received as usize) < max {
                match socket.recv_from(&mut buf) {
                    Ok((len, _)) => {
                        report.rtp_packets_received += 1;
                        report.rtp_bytes_received += len as u64;
                    }
                    Err(_) => {
                        // Check if caster closed or sent TEARDOWN
                        match transport.read_message() {
                            Ok(Some(msg)) => {
                                if matches!(
                                    msg.start,
                                    StartLine::Request {
                                        method: Method::Teardown,
                                        ..
                                    }
                                ) {
                                    let _ = reply(&mut transport, msg.header("cseq"), Vec::new());
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(TransportError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                }
            }
        }
    }

    Ok(report)
}

/// Read one message, recording it.
fn next<S: Read + Write>(
    transport: &mut RtspTransport<S>,
    report: &mut SinkReport,
) -> Result<RtspMessage, SinkError> {
    let Some(message) = transport.read_message()? else {
        return Err(SinkError::Unexpected(
            "the source closed the connection mid-negotiation".to_owned(),
        ));
    };
    report.received.push(message.clone());
    Ok(message)
}

/// Read messages until one is a response carrying `expected_cseq`, answering any
/// request the source interleaves.
fn expect_response<S: Read + Write>(
    transport: &mut RtspTransport<S>,
    report: &mut SinkReport,
    expected_cseq: &str,
) -> Result<RtspMessage, SinkError> {
    for _ in 0..8 {
        let message = next(transport, report)?;
        match &message.start {
            StartLine::Status { code, .. } => {
                if message.header("cseq") != Some(expected_cseq) {
                    report.cseq_matched = false;
                }
                if *code != 200 {
                    return Err(SinkError::Unexpected(format!(
                        "the source answered {code} to cseq {expected_cseq}"
                    )));
                }
                return Ok(message);
            }
            StartLine::Request { .. } => {
                // The source interleaved a request; answer it and keep waiting.
                reply(transport, message.header("cseq"), Vec::new())?;
            }
        }
    }
    Err(SinkError::Unexpected(
        "the source never answered the sink's request".to_owned(),
    ))
}

fn reply<S: Read + Write>(
    transport: &mut RtspTransport<S>,
    cseq: Option<&str>,
    body: Vec<u8>,
) -> Result<(), SinkError> {
    let cseq = cseq
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    transport.send(&RtspMessage::status(200, "OK", cseq, body))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cast::{Drain, HandshakeOutcome, handshake};
    use crate::wfd::parameters::SourceParameters;
    use crate::wfd::session::WfdSourceSession;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    /// A socketpair with read/write timeouts, so a protocol deadlock fails a test
    /// instead of stalling the suite.
    fn pair() -> (UnixStream, UnixStream) {
        let (left, right) = UnixStream::pair().expect("socketpair");
        let timeout = Some(Duration::from_secs(5));
        for stream in [&left, &right] {
            stream.set_read_timeout(timeout).expect("read timeout");
            stream.set_write_timeout(timeout).expect("write timeout");
        }
        (left, right)
    }

    /// Drive the real caster and the scripted sink against each other over a
    /// socketpair: no radio, no GPU, no sink device.
    #[test]
    fn the_caster_completes_a_handshake_against_the_scripted_sink() {
        let (caster_side, sink_side) = pair();
        let script = SinkScript::default();

        let sink = std::thread::spawn(move || run(RtspTransport::new(sink_side), &script));

        let mut session = WfdSourceSession::new(
            SourceParameters::new("00 00 02 10")
                .with_presentation_url("rtsp://192.168.49.1/wfd1.0/streamid=0"),
        );
        let (outcome, transport) = handshake(
            RtspTransport::new(caster_side),
            &mut session,
            Instant::now() + Duration::from_secs(10),
            32,
            // Keep serving: the sink still replies to the source's trigger.
            Drain::UntilQuiet(8),
        )
        .expect("handshake");
        // Release the socket: the sink's remaining reads must see EOF, not wait.
        drop(transport);

        assert!(
            outcome.is_streaming(),
            "the caster must reach streaming against a well-behaved sink: {outcome:?}"
        );
        assert_eq!(
            outcome,
            HandshakeOutcome::Streaming {
                presentation_url: "rtsp://192.168.49.1/wfd1.0/streamid=0".to_owned(),
                // The sink's reply to our trigger, answered before it closed.
                drained: 1,
            }
        );

        let report = sink.join().expect("sink thread").expect("sink run");
        assert!(report.cseq_matched, "every reply must echo the sink's CSeq");
        assert!(
            report.source_triggered_setup,
            "a source must send its own SETUP trigger after the sink asks for one"
        );
        let advertised: std::collections::BTreeMap<_, _> =
            report.source_parameters.iter().cloned().collect();
        assert_eq!(advertised["wfd_video_formats"], "00 00 02 10");
        assert_eq!(advertised["wfd_content_protection"], "none");
        assert!(
            advertised["wfd_client_rtp_ports"].contains("19000"),
            "{:?}",
            advertised["wfd_client_rtp_ports"]
        );
        assert!(
            advertised["wfd_presentation_URL"].contains("rtsp://192.168.49.1/wfd1.0/streamid=0"),
            "{:?}",
            advertised["wfd_presentation_URL"]
        );
    }

    #[test]
    fn a_caster_that_advertises_a_forged_parameter_cannot_hide_it_from_the_sink() {
        // The injection defence, observed from the receiving end: a video-format
        // value carrying a line break must never reach the sink as two parameters.
        let (caster_side, sink_side) = UnixStream::pair().expect("socketpair");
        let script = SinkScript::default();
        let sink = std::thread::spawn(move || run(RtspTransport::new(sink_side), &script));

        let mut session = WfdSourceSession::new(SourceParameters::new(
            "00 00 02 10\r\nwfd_content_protection: HDCP2.2",
        ));
        let _ = handshake(
            RtspTransport::new(caster_side),
            &mut session,
            Instant::now() + Duration::from_secs(10),
            32,
            Drain::None,
        );

        // The sink either sees no negotiation at all (the source failed closed) or
        // sees a clean one; what it must never see is a forged protection value.
        if let Ok(report) = sink.join().expect("sink thread") {
            let forged = report
                .source_parameters
                .iter()
                .any(|(name, value)| name == "wfd_content_protection" && value != "none");
            assert!(!forged, "a forged parameter reached the sink: {report:?}");
        }
    }

    #[test]
    fn the_sink_refuses_a_source_that_opens_with_something_else() {
        let (caster_side, sink_side) = pair();
        let sink =
            std::thread::spawn(move || run(RtspTransport::new(sink_side), &SinkScript::default()));
        // Not a source: it answers instead of opening.
        let mut transport = RtspTransport::new(caster_side);
        transport
            .send(&RtspMessage::status(200, "OK", 0, Vec::new()))
            .expect("send");
        match sink.join().expect("sink thread") {
            Err(SinkError::Unexpected(reason)) => {
                assert!(
                    reason.contains("expected the source to open with OPTIONS"),
                    "{reason}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_caster_streams_frames_to_scripted_sink_over_udp() {
        use crate::media::{StreamConfig, TestPatternSource, UdpMediaSender, stream};
        use std::net::UdpSocket;

        let (caster_side, sink_side) = pair();
        sink_side
            .set_read_timeout(Some(Duration::from_millis(10)))
            .expect("sink read timeout");
        caster_side
            .set_read_timeout(Some(Duration::from_millis(10)))
            .expect("caster read timeout");

        // Bind ephemeral UDP socket for the sink's RTP receiver
        let sink_udp = UdpSocket::bind("127.0.0.1:0").expect("bind sink udp");
        let sink_udp_addr = sink_udp.local_addr().expect("sink udp addr");
        let sink_udp_clone = sink_udp.try_clone().expect("clone udp");

        let script = SinkScript {
            video_formats: "00 00 02 10".to_owned(),
            expect_setup_play: true,
            receive_rtp_packets: None,
            rtp_socket: Some(sink_udp_clone),
        };

        let sink_thread = std::thread::spawn(move || run(RtspTransport::new(sink_side), &script));

        let mut session = WfdSourceSession::new(
            SourceParameters::new("00 00 02 10")
                .with_presentation_url("rtsp://127.0.0.1/wfd1.0/streamid=0")
                .with_rtp_port(sink_udp_addr.port()),
        );

        let (outcome, transport) = handshake(
            RtspTransport::new(caster_side),
            &mut session,
            Instant::now() + Duration::from_secs(10),
            32,
            Drain::UntilQuiet(8),
        )
        .expect("handshake");

        assert!(outcome.is_streaming());

        let media_sender = UdpMediaSender::bind_ephemeral(sink_udp_addr).expect("bind sender");
        let config = StreamConfig {
            fps: 60,
            max_frames: Some(5),
            max_duration: None,
            ssrc: 0x1122_3344,
        };

        let (stats, transport) = stream(
            transport,
            &mut session,
            media_sender,
            TestPatternSource::new(60),
            config,
        );
        drop(transport);

        assert_eq!(stats.frames_sent, 5);
        assert!(stats.rtp_packets_sent >= 5);

        let report = sink_thread
            .join()
            .expect("sink thread")
            .expect("sink report");
        assert!(report.rtp_packets_received >= 5);
        assert!(report.rtp_bytes_received > 0);
    }
}
