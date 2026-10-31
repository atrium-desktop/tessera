//! The WFD source session: a **permissive responder** driven by the sink.
//!
//! # Why this reacts instead of scripting the order
//!
//! The M1–M7 exchange has a nominal order, but sinks in the field vary it:
//! some repeat `OPTIONS`, some send `SET_PARAMETER` before the trigger, some
//! pipeline. A state machine that *insists* on an order turns a working sink
//! into a failure, and — without hardware to check against — it would also
//! enshrine a guess as a test (`[INV-ARCH-83]`'s spirit applied to ordering,
//! [ADR-0192](../../../docs/adr/0192-first-party-miracast-caster.md) §7).
//!
//! So the session answers whatever it is asked, in whatever order, and reports
//! what it *understood* as [`Effect`]s for the driver to act on. The mandatory
//! facts are enforced instead of the sequence:
//!
//! - every reply echoes the request's `CSeq` (RFC 2326 requires it);
//! - a request we do not implement is answered `501`, never ignored;
//! - a trigger is only honoured when it names a method we know;
//! - nothing is emitted as a follow-up that the sink did not ask for, except the
//!   `SETUP` trigger after a `SETUP` instruction, which is what a source must
//!   send to make the sink start.

use super::parameters::{SourceParameters, TriggerMethod};
use super::rtsp::{Method, RtspMessage, StartLine};
use super::trace::{Direction, Trace, describe_inbound};

/// The source RTP port named in the SETUP response. The media sender binds this
/// exact port and the local address of the RTSP connection, because a sink may
/// filter RTP by the source port it was told to expect.
pub const SERVER_RTP_PORT: u16 = 19002;

/// What the driver must do in response to the sink's latest message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// The sink asked us to start the media session: open the RTP sink and begin
    /// presenting on `presentation_url`.
    StartStreaming { presentation_url: String },
    /// The sink asked to pause presentation; keep the session, stop producing.
    PauseStreaming,
    /// The session is over; stop producing and release the media resources.
    Terminate,
}

/// The session's phase, for diagnostics and for deciding whether presentation is
/// expected. It is advisory: the responder never refuses a message because of
/// the phase it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No `OPTIONS` seen yet.
    AwaitingOptions,
    /// Core negotiation in progress.
    Negotiating,
    /// The sink issued a `SETUP` trigger and presentation is expected.
    Streaming,
    /// The sink paused presentation.
    Paused,
    /// The sink tore the session down.
    Terminated,
}

/// A source-side WFD session.
#[derive(Debug)]
pub struct WfdSourceSession {
    parameters: SourceParameters,
    phase: Phase,
    trace: Trace,
    sink_rtp_port: Option<u16>,
    sink_audio_codecs: Option<String>,
    audio_enabled: bool,
}

impl WfdSourceSession {
    pub fn new(parameters: SourceParameters) -> Self {
        Self {
            parameters,
            phase: Phase::AwaitingOptions,
            trace: Trace::new(),
            sink_rtp_port: None,
            sink_audio_codecs: None,
            audio_enabled: false,
        }
    }

    /// Adopt the sink's audio codec in M5 and expect an audio stream.
    pub fn enable_audio(&mut self) {
        self.audio_enabled = true;
    }

    /// The codec the sink advertised in M4, if any.
    pub fn sink_audio_codecs(&self) -> Option<&str> {
        self.sink_audio_codecs.as_deref()
    }

    /// The audio codec this source selected, or `None` when audio is off or the
    /// sink offered no AAC.
    pub fn audio_codec(&self) -> Option<&str> {
        (self.parameters.audio_codecs != "none").then_some(self.parameters.audio_codecs.as_str())
    }

    fn absorb_parameters(&mut self, parameters: &std::collections::BTreeMap<String, String>) {
        if let Some(ports) = parameters.get("wfd_client_rtp_ports")
            && let Some(port) = parse_client_rtp_port(ports)
        {
            self.sink_rtp_port = Some(port);
        }
        if let Some(codec) = parameters.get("wfd_audio_codecs") {
            self.sink_audio_codecs = Some(codec.clone());
            if self.audio_enabled && codec.starts_with("AAC") {
                self.parameters.audio_codecs = codec.clone();
            }
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// The RTP port the sink receives media on, negotiated during M4 or M6.
    pub fn sink_rtp_port(&self) -> Option<u16> {
        self.sink_rtp_port
    }

    /// The M5 body: the source parameters plus the SETUP trigger.
    pub fn parameters_trigger_body(&self) -> Result<Vec<u8>, super::parameters::ParameterError> {
        self.parameters.to_trigger_body()
    }

    /// Record a message this source sent outside the reply path — the opening
    /// `OPTIONS` — so the trace shows who actually spoke first.
    pub fn note_outbound(&mut self, message: &RtspMessage, notes: Vec<String>) {
        self.trace
            .record(Direction::SourceToSink, message.clone(), notes);
    }

    /// Handle one inbound RTSP message, returning the outbound messages to send
    /// and the effects for the driver, in that order.
    pub fn on_message(&mut self, message: &RtspMessage) -> (Vec<RtspMessage>, Vec<Effect>) {
        self.trace.record(
            Direction::SinkToSource,
            message.clone(),
            describe_inbound(message),
        );
        let mut outbound = Vec::new();
        let mut effects = Vec::new();
        let cseq = message.header("cseq").and_then(|value| value.parse().ok());

        let StartLine::Request { method, .. } = &message.start else {
            // A status line from the sink is informational: our replies are
            // answers, and answering an answer would loop. It was recorded
            // inbound above; recorded again as outbound would claim this source
            // sent a message it never sent. A 200 to our GET_PARAMETER *does*
            // carry the sink's parameters, so absorb them before returning.
            if let StartLine::Status { code: 200, .. } = &message.start
                && let Ok(parameters) = message.parameters()
            {
                self.absorb_parameters(&parameters);
            }
            return (outbound, effects);
        };

        match method {
            Method::Options => {
                self.phase = if self.phase == Phase::AwaitingOptions {
                    Phase::Negotiating
                } else {
                    self.phase
                };
                outbound.push(reply(cseq, Vec::new(), options_headers()));
            }
            Method::GetParameter => {
                self.phase = if self.phase == Phase::AwaitingOptions {
                    Phase::Negotiating
                } else {
                    self.phase
                };
                match self.parameters.to_body() {
                    Ok(body) => outbound.push(reply(cseq, body, Vec::new())),
                    Err(error) => {
                        // A parameter we cannot serialize correctly must stop
                        // the session, not be sent wrong.
                        let note = format!("refused: {error}");
                        outbound.push(reply_with_reason(
                            cseq,
                            500,
                            "Internal Server Error",
                            Vec::new(),
                        ));
                        self.phase = Phase::Terminated;
                        effects.push(Effect::Terminate);
                        self.trace.record(
                            Direction::SourceToSink,
                            outbound.last().expect("just pushed").clone(),
                            vec![note],
                        );
                        return (outbound, effects);
                    }
                }
            }
            Method::SetParameter => {
                let parsed = message.parameters();
                outbound.push(reply(cseq, Vec::new(), Vec::new()));
                match parsed {
                    Ok(parameters) => {
                        self.absorb_parameters(&parameters);
                        if let Some(trigger) = parameters.get("wfd_trigger_method") {
                            match TriggerMethod::parse(trigger) {
                                Some(TriggerMethod::Setup) => {
                                    self.phase = Phase::Streaming;
                                    effects.push(Effect::StartStreaming {
                                        presentation_url: self.parameters.presentation_url.clone(),
                                    });
                                    // A source must tell the sink to SETUP; this
                                    // is the one unsolicited follow-up allowed.
                                    let mut body = std::collections::BTreeMap::new();
                                    body.insert(
                                        "wfd_trigger_method".to_owned(),
                                        TriggerMethod::Setup.as_str().to_owned(),
                                    );
                                    outbound.push(RtspMessage::request(
                                        Method::SetParameter,
                                        "rtsp://localhost/wfd1.0",
                                        cseq.unwrap_or(0).wrapping_add(1),
                                        super::rtsp::write_parameters(&body),
                                    ));
                                }
                                Some(TriggerMethod::Teardown) => {
                                    self.phase = Phase::Terminated;
                                    effects.push(Effect::Terminate);
                                }
                                Some(TriggerMethod::Pause) => {
                                    self.phase = Phase::Paused;
                                    effects.push(Effect::PauseStreaming);
                                }
                                Some(TriggerMethod::Play) => {
                                    self.phase = Phase::Streaming;
                                    effects.push(Effect::StartStreaming {
                                        presentation_url: self.parameters.presentation_url.clone(),
                                    });
                                }
                                None => {
                                    // Unknown trigger: acknowledged, not acted on.
                                }
                            }
                        }
                    }
                    Err(error) => {
                        // The body was not `text/parameters`; the session can
                        // continue, but the trace must say why nothing happened.
                        self.trace.record(
                            Direction::SinkToSource,
                            message.clone(),
                            vec![format!("unparsed body: {error}")],
                        );
                    }
                }
            }
            Method::Setup => {
                self.phase = Phase::Streaming;
                let mut headers = vec![("session".to_owned(), "1".to_owned())];
                if let Some(transport) = message.header("transport") {
                    if let Some(port) = parse_transport_client_port(transport) {
                        self.sink_rtp_port = Some(port);
                    }
                    let value = if transport.to_ascii_lowercase().contains("server_port") {
                        transport.to_owned()
                    } else {
                        format!(
                            "{transport};server_port={SERVER_RTP_PORT}-{}",
                            SERVER_RTP_PORT + 1
                        )
                    };
                    headers.push(("transport".to_owned(), value));
                }
                effects.push(Effect::StartStreaming {
                    presentation_url: self.parameters.presentation_url.clone(),
                });
                outbound.push(reply(cseq, Vec::new(), headers));
            }
            Method::Play => {
                self.phase = Phase::Streaming;
                effects.push(Effect::StartStreaming {
                    presentation_url: self.parameters.presentation_url.clone(),
                });
                outbound.push(reply(
                    cseq,
                    Vec::new(),
                    vec![("session".to_owned(), "1".to_owned())],
                ));
            }
            Method::Pause => {
                self.phase = Phase::Paused;
                effects.push(Effect::PauseStreaming);
                outbound.push(reply(cseq, Vec::new(), Vec::new()));
            }
            Method::Teardown => {
                // A TEARDOWN with no session id is a stale-session reset, not
                // the end of this session. An Android 9 sink was observed
                // sending exactly that as its first message (empty `Session:`,
                // `user-agent: stagefright/1.2`); returning 200 and closing made
                // the sink give up before it ever sent M1. Acknowledge and keep
                // serving so it can start the real exchange. A TEARDOWN that
                // names a session is a genuine termination.
                let carries_session = message
                    .header("session")
                    .is_some_and(|value| !value.trim().is_empty());
                if carries_session {
                    self.phase = Phase::Terminated;
                    effects.push(Effect::Terminate);
                }
                outbound.push(reply(cseq, Vec::new(), Vec::new()));
            }
        }

        for message in &outbound {
            self.trace.record(
                Direction::SourceToSink,
                message.clone(),
                vec![summarize_reply(message)],
            );
        }
        (outbound, effects)
    }
}

fn summarize_reply(message: &RtspMessage) -> String {
    match &message.start {
        StartLine::Status { code, reason, .. } => format!("answer: {code} {reason}"),
        StartLine::Request { method, .. } => format!("follow-up: {}", method.as_str()),
    }
}

/// The `Public` header a WFD source advertises: the methods it implements.
fn options_headers() -> Vec<(String, String)> {
    vec![(
        "public".to_owned(),
        "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER, SETUP, PLAY, PAUSE, TEARDOWN".to_owned(),
    )]
}

/// Build a `200 OK` answer carrying `body` and `extra` headers.
///
/// The media type is set here because a body on this session is always
/// `text/parameters`; `Content-Length` is left to the RTSP encoder, which
/// recomputes it so a caller can never desynchronise the framing.
fn reply(cseq: Option<u32>, body: Vec<u8>, extra: Vec<(String, String)>) -> RtspMessage {
    let mut headers = extra;
    if !body.is_empty() {
        headers.push(("content-type".to_owned(), "text/parameters".to_owned()));
    }
    let mut message = status_reply(cseq, 200, "OK", headers);
    message.body = body;
    message
}

fn reply_with_reason(
    cseq: Option<u32>,
    code: u16,
    reason: &str,
    headers: Vec<(String, String)>,
) -> RtspMessage {
    status_reply(cseq, code, reason, headers)
}

/// A status message that echoes the request's `CSeq` when it had one, and omits
/// the header when it did not — a fabricated `CSeq` would desynchronise the
/// sink's request matching (RFC 2326).
fn status_reply(
    cseq: Option<u32>,
    code: u16,
    reason: &str,
    headers: Vec<(String, String)>,
) -> RtspMessage {
    match cseq {
        Some(cseq) => {
            let mut message = RtspMessage::status(code, reason, cseq, Vec::new());
            message.headers.extend(headers);
            message
        }
        None => {
            let mut message = RtspMessage::status(code, reason, 0, Vec::new());
            message.headers.retain(|(name, _)| name != "cseq");
            message.headers.extend(headers);
            message
        }
    }
}

/// Parse the client RTP port from a `wfd_client_rtp_ports` value
/// (e.g. `RTP/AVP/UDP;unicast 19000 0 mode=play`).
pub fn parse_client_rtp_port(value: &str) -> Option<u16> {
    for token in value.split_whitespace() {
        if let Ok(port) = token.parse::<u16>()
            && port != 0
        {
            return Some(port);
        }
    }
    None
}

/// Parse the client RTP port from an RTSP `Transport` header
/// (e.g. `RTP/AVP/UDP;unicast;client_port=19000-19001` or `client_port=19000`).
pub fn parse_transport_client_port(value: &str) -> Option<u16> {
    for part in value.split(';') {
        let trimmed = part.trim();
        if let Some(rest) = trimmed.strip_prefix("client_port=") {
            let port_str = rest.split('-').next().unwrap_or(rest);
            if let Ok(port) = port_str.parse::<u16>()
                && port != 0
            {
                return Some(port);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> WfdSourceSession {
        WfdSourceSession::new(
            SourceParameters::new("00 00 02 10")
                .with_presentation_url("rtsp://192.168.49.1/wfd1.0/streamid=0"),
        )
    }

    fn request(method: Method, cseq: u32, body: Vec<u8>) -> RtspMessage {
        RtspMessage::request(method, "rtsp://localhost/wfd1.0", cseq, body)
    }

    fn trigger(cseq: u32, value: &str) -> RtspMessage {
        request(
            Method::SetParameter,
            cseq,
            format!("wfd_trigger_method: {value}\r\n").into_bytes(),
        )
    }

    #[test]
    fn m1_options_is_answered_with_the_public_header_and_the_cseq() {
        let mut session = session();
        let (outbound, effects) = session.on_message(&request(Method::Options, 0, Vec::new()));
        assert_eq!(outbound.len(), 1);
        assert!(effects.is_empty());
        let reply = &outbound[0];
        assert_eq!(
            reply.start,
            StartLine::Status {
                version: "RTSP/1.0".to_owned(),
                code: 200,
                reason: "OK".to_owned()
            }
        );
        assert_eq!(reply.header("cseq"), Some("0"));
        let public = reply.header("public").expect("Public header");
        assert!(public.contains("org.wfa.wfd1.0"), "{public}");
        assert_eq!(session.phase(), Phase::Negotiating);
    }

    #[test]
    fn m3_get_parameter_is_answered_with_our_parameters() {
        let mut session = session();
        let (outbound, _) = session.on_message(&request(Method::GetParameter, 2, Vec::new()));
        assert_eq!(outbound.len(), 1);
        let parameters = outbound[0].parameters().expect("parameters");
        assert_eq!(parameters["wfd_video_formats"], "00 00 02 10");
        assert_eq!(parameters["wfd_content_protection"], "none");
        assert_eq!(outbound[0].header("cseq"), Some("2"));
        assert_eq!(outbound[0].header("content-type"), Some("text/parameters"));
    }

    #[test]
    fn the_cseq_is_echoed_for_every_method() {
        let mut session = session();
        for (cseq, method) in [
            (11, Method::Options),
            (12, Method::GetParameter),
            (13, Method::SetParameter),
            (14, Method::Setup),
            (15, Method::Play),
            (16, Method::Pause),
            (17, Method::Teardown),
        ] {
            let (outbound, _) = session.on_message(&request(method, cseq, Vec::new()));
            let expected = cseq.to_string();
            assert_eq!(
                outbound[0].header("cseq"),
                Some(expected.as_str()),
                "{method} must echo its CSeq"
            );
        }
    }

    #[test]
    fn a_setup_trigger_starts_streaming_and_tells_the_sink_to_setup() {
        let mut session = session();
        let (outbound, effects) = session.on_message(&trigger(5, "SETUP"));
        assert_eq!(
            effects,
            vec![Effect::StartStreaming {
                presentation_url: "rtsp://192.168.49.1/wfd1.0/streamid=0".to_owned()
            }]
        );
        assert_eq!(session.phase(), Phase::Streaming);
        assert_eq!(outbound.len(), 2, "an answer plus the SETUP trigger");
        assert!(matches!(
            outbound[0].start,
            StartLine::Status { code: 200, .. }
        ));
        let follow_up = &outbound[1];
        assert!(matches!(
            &follow_up.start,
            StartLine::Request {
                method: Method::SetParameter,
                ..
            }
        ));
        let parameters = follow_up.parameters().expect("parameters");
        assert_eq!(parameters["wfd_trigger_method"], "SETUP");
    }

    #[test]
    fn a_teardown_without_a_session_is_a_stale_reset_not_a_termination() {
        let mut session = session();
        let mut stale = request(Method::Teardown, 9, Vec::new());
        stale.headers.push(("session".to_owned(), String::new()));
        let (outbound, effects) = session.on_message(&stale);
        assert!(
            effects.is_empty(),
            "a stale-session reset must not terminate the source: {effects:?}"
        );
        assert_ne!(session.phase(), Phase::Terminated);
        assert_eq!(outbound.len(), 1, "the reset is acknowledged");

        // The sink can now start the real exchange.
        let (outbound, _) = session.on_message(&request(Method::Options, 10, Vec::new()));
        assert_eq!(outbound.len(), 1);
        assert_eq!(session.phase(), Phase::Negotiating);

        // A TEARDOWN naming a session is a genuine termination.
        let mut real = request(Method::Teardown, 11, Vec::new());
        real.headers.push(("session".to_owned(), "1".to_owned()));
        let (_, effects) = session.on_message(&real);
        assert_eq!(effects, vec![Effect::Terminate]);
        assert_eq!(session.phase(), Phase::Terminated);
    }

    #[test]
    fn a_teardown_trigger_ends_the_session() {
        let mut session = session();
        let (_, effects) = session.on_message(&trigger(6, "TEARDOWN"));
        assert_eq!(effects, vec![Effect::Terminate]);
        assert_eq!(session.phase(), Phase::Terminated);
    }

    #[test]
    fn a_pause_trigger_pauses_without_ending() {
        let mut session = session();
        let (_, effects) = session.on_message(&trigger(7, "PAUSE"));
        assert_eq!(effects, vec![Effect::PauseStreaming]);
        assert_eq!(session.phase(), Phase::Paused);
    }

    #[test]
    fn an_unknown_trigger_is_acknowledged_but_not_acted_on() {
        let mut session = session();
        let (outbound, effects) = session.on_message(&trigger(8, "REBOOT"));
        assert!(
            effects.is_empty(),
            "an unknown trigger must not start a stream"
        );
        assert_eq!(outbound.len(), 1, "only the acknowledgement");
        assert_eq!(session.phase(), Phase::AwaitingOptions);
    }

    #[test]
    fn out_of_order_messages_are_answered_not_refused() {
        // A sink that opens with SET_PARAMETER (no OPTIONS first) must still be
        // served: insisting on the nominal order breaks working sinks.
        let mut session = session();
        let (outbound, effects) = session.on_message(&trigger(1, "SETUP"));
        assert_eq!(outbound.len(), 2);
        assert_eq!(effects.len(), 1);
    }

    #[test]
    fn a_status_line_from_the_sink_is_not_answered_with_a_reply_loop() {
        let mut session = session();
        let status = RtspMessage::status(200, "OK", 4, Vec::new());
        let (outbound, effects) = session.on_message(&status);
        assert!(outbound.is_empty(), "answering an answer would loop");
        assert!(effects.is_empty());
    }

    #[test]
    fn a_request_without_a_cseq_is_answered_without_inventing_one() {
        let mut session = session();
        let mut message = request(Method::Options, 0, Vec::new());
        message.headers.retain(|(name, _)| name != "cseq");
        let (outbound, _) = session.on_message(&message);
        assert_eq!(outbound.len(), 1);
        assert_eq!(
            outbound[0].header("cseq"),
            None,
            "a fabricated CSeq would desynchronise the sink's matching"
        );
    }

    #[test]
    fn an_unserializable_parameter_stops_the_session_instead_of_sending_it() {
        // A video-format value with a line break cannot be put on the wire
        // without forging a parameter, so the session must fail closed.
        let mut session =
            WfdSourceSession::new(SourceParameters::new("00\r\nwfd_audio_codecs: AAC"));
        let (outbound, effects) = session.on_message(&request(Method::GetParameter, 2, Vec::new()));
        assert_eq!(effects, vec![Effect::Terminate]);
        assert_eq!(session.phase(), Phase::Terminated);
        assert!(matches!(
            outbound[0].start,
            StartLine::Status { code: 500, .. }
        ));
        assert!(outbound[0].body.is_empty(), "the bad body must not be sent");
    }

    #[test]
    fn the_trace_records_both_directions() {
        let mut session = session();
        let _ = session.on_message(&request(Method::Options, 0, Vec::new()));
        let _ = session.on_message(&trigger(1, "SETUP"));
        let trace = session.trace();
        assert!(trace.entries().len() >= 4);
        let rendered = trace.render();
        assert!(rendered.contains("sink -> source  OPTIONS"));
        assert!(rendered.contains("source -> sink  200 OK"));
        assert!(rendered.contains("wfd_trigger_method: SETUP"));
    }

    #[test]
    fn a_full_nominal_handshake_produces_one_streaming_effect() {
        let mut session = session();
        let mut streaming = 0;
        for message in [
            request(Method::Options, 0, Vec::new()),
            request(Method::GetParameter, 1, Vec::new()),
            trigger(2, "SETUP"),
            request(Method::Setup, 3, Vec::new()),
            request(Method::Play, 4, Vec::new()),
        ] {
            let (_, effects) = session.on_message(&message);
            streaming += effects
                .iter()
                .filter(|effect| matches!(effect, Effect::StartStreaming { .. }))
                .count();
        }
        assert_eq!(
            streaming, 3,
            "trigger, SETUP and PLAY each report a start; the driver must be idempotent"
        );
        assert_eq!(session.phase(), Phase::Streaming);
    }

    #[test]
    fn sink_rtp_port_is_extracted_from_parameters_and_transport() {
        let mut session = session();
        assert_eq!(session.sink_rtp_port(), None);

        // From wfd_client_rtp_ports
        let mut params = std::collections::BTreeMap::new();
        params.insert(
            "wfd_client_rtp_ports".to_owned(),
            "RTP/AVP/UDP;unicast 20000 0 mode=play".to_owned(),
        );
        let msg = RtspMessage::request(
            Method::SetParameter,
            "rtsp://localhost/wfd1.0",
            1,
            crate::wfd::rtsp::write_parameters(&params),
        );
        let _ = session.on_message(&msg);
        assert_eq!(session.sink_rtp_port(), Some(20000));

        // Overridden by Transport header in SETUP
        let mut setup =
            RtspMessage::request(Method::Setup, "rtsp://localhost/wfd1.0", 2, Vec::new());
        setup.headers.push((
            "transport".to_owned(),
            "RTP/AVP/UDP;unicast;client_port=21000-21001".to_owned(),
        ));
        let (responses, effects) = session.on_message(&setup);
        assert_eq!(session.sink_rtp_port(), Some(21000));
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].header("session"), Some("1"));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::StartStreaming { .. })),
            "SETUP must start the stream: {effects:?}"
        );
        let transport = responses[0]
            .header("transport")
            .expect("the SETUP response must name the transport");
        assert!(transport.contains("client_port=21000-21001"), "{transport}");
        assert!(transport.contains("server_port=19002-19003"), "{transport}");
    }

    fn m4_response(audio: Option<&str>) -> RtspMessage {
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert(
            "wfd_video_formats".to_owned(),
            "38 00 01 01 f9cf94a0 00000000 00000000 00 0000 0000 00 none none".to_owned(),
        );
        parameters.insert(
            "wfd_client_rtp_ports".to_owned(),
            "RTP/AVP/UDP;unicast 19000 0 mode=play".to_owned(),
        );
        if let Some(audio) = audio {
            parameters.insert("wfd_audio_codecs".to_owned(), audio.to_owned());
        }
        RtspMessage::status(
            200,
            "OK",
            2,
            crate::wfd::rtsp::write_parameters(&parameters),
        )
    }

    #[test]
    fn an_audio_enabled_source_adopts_the_sink_aac_codec_in_m5() {
        let mut session = session();
        session.enable_audio();
        let (_, effects) = session.on_message(&m4_response(Some("AAC 00000001 00")));
        assert!(effects.is_empty());
        assert_eq!(session.sink_audio_codecs(), Some("AAC 00000001 00"));
        assert_eq!(session.audio_codec(), Some("AAC 00000001 00"));
        let body = session.parameters_trigger_body().expect("M5 body");
        assert!(
            String::from_utf8_lossy(&body).contains("wfd_audio_codecs: AAC 00000001 00"),
            "M5 must echo the sink's AAC codec: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[test]
    fn audio_stays_off_when_the_source_did_not_enable_it() {
        let mut session = session();
        let _ = session.on_message(&m4_response(Some("AAC 00000001 00")));
        assert_eq!(session.sink_audio_codecs(), Some("AAC 00000001 00"));
        assert_eq!(session.audio_codec(), None);
        let body = session.parameters_trigger_body().expect("M5 body");
        assert!(String::from_utf8_lossy(&body).contains("wfd_audio_codecs: none"));
    }

    #[test]
    fn audio_stays_off_when_the_sink_offers_no_codec() {
        let mut session = session();
        session.enable_audio();
        let _ = session.on_message(&m4_response(None));
        assert_eq!(session.audio_codec(), None);
        let body = session.parameters_trigger_body().expect("M5 body");
        assert!(String::from_utf8_lossy(&body).contains("wfd_audio_codecs: none"));
    }
}
