//! The casting use cases.
//!
//! Only the handshake is implemented, and it is named for what it does: it drives
//! M1–M7 against a sink and reports how far it got. Media (encode, mux, transmit)
//! is not implemented, so nothing here claims to cast
//! ([ADR-0192](../docs/adr/0192-first-party-miracast-caster.md) §8).
//!
//! Separating the handshake is not a stepping stone for its own sake: a sink
//! accepting M1–M7 is the first thing that has ever been unverifiable offline, and
//! it is also where the untested spec assumptions (parameter bitfields, ordering)
//! are actually decided.

use std::io::{Read, Write};
use std::time::Instant;

use crate::wfd::rtsp::{Method, RtspMessage, StartLine};
use crate::wfd::session::{Effect, WfdSourceSession};
use crate::wfd::transport::{RtspTransport, TransportError};

/// How long the driver keeps serving RTSP after the sink asks for a session.
///
/// A source may not stop reading the moment streaming starts: a real sink still
/// sends its reply to the source's trigger, then `SETUP` and `PLAY`, and later
/// `TEARDOWN`. Returning immediately leaves those unanswered — against a real sink
/// that surfaces as a broken pipe on the sink's side and a failed session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drain {
    /// Return as soon as the sink asks for a session. Only for a caller that has
    /// nothing listening next.
    None,
    /// Keep answering up to this many further messages, stopping early on EOF or
    /// a read timeout. Silence is not an error here: it means the sink is waiting
    /// for media, which is the normal state after `PLAY`.
    UntilQuiet(usize),
}

/// How a handshake ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeOutcome {
    /// The sink drove the session to a streaming trigger. The handshake is
    /// complete; media is the caller's next step (and is not implemented).
    Streaming {
        presentation_url: String,
        /// Messages answered after the trigger, before the sink went quiet.
        drained: usize,
    },
    /// The sink tore the session down.
    Terminated,
    /// The peer stopped answering inside the deadline.
    TimedOut { messages: usize },
    /// The peer closed the connection.
    Closed { messages: usize },
    /// The message budget ran out before the session reached a conclusion. A
    /// budget exists because a peer that answers forever must not hang the tool.
    MessageBudget { messages: usize },
}

impl HandshakeOutcome {
    /// Whether the sink completed the WFD negotiation.
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Streaming {
                presentation_url,
                drained,
            } => format!(
                "sink accepted the handshake; it will SETUP {presentation_url} \
                 (answered {drained} further message(s))"
            ),
            Self::Terminated => "sink tore the session down".to_owned(),
            Self::TimedOut { messages } => {
                format!("sink stopped answering after {messages} message(s)")
            }
            Self::Closed { messages } => {
                format!("sink closed the connection after {messages} message(s)")
            }
            Self::MessageBudget { messages } => {
                format!("message budget exhausted after {messages} message(s)")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastError {
    Transport(TransportError),
}

impl std::fmt::Display for CastError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CastError {}

/// Drive the WFD handshake over an already-connected transport.
///
/// The loop is lock-step and bounded three ways — a deadline, a message budget,
/// and the peer going quiet — because each failure has a different meaning:
/// silence is a network or sink problem, a budget overrun is a peer that talks
/// without concluding, and a deadline is the operator's patience.
pub fn handshake<S: Read + Write>(
    mut transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    deadline: Instant,
    message_budget: usize,
    drain: Drain,
) -> Result<(HandshakeOutcome, RtspTransport<S>), CastError> {
    // A source opens with `OPTIONS` carrying the WFD requirement, unless the sink
    // spoke first. Sending it unconditionally is what the nominal M1/M2 pair
    // describes, and a permissive sink answers it either way.
    let opening = RtspMessage::request(Method::Options, "*", 0, Vec::new());
    transport.send(&opening).map_err(CastError::Transport)?;
    // Recorded as outbound: routing it through `on_message` would have labelled
    // our own message as the sink's, which is exactly the confusion a trace
    // exists to prevent.
    session.note_outbound(
        &opening,
        vec!["opening OPTIONS (source speaks first)".to_owned()],
    );

    let mut messages = 0;
    // Set once the sink has asked for a session; from then on the loop drains
    // rather than negotiates.
    let mut streaming: Option<String> = None;
    let mut drained = 0usize;
    loop {
        if Instant::now() >= deadline {
            return Ok((
                conclude(&streaming, drained, HandshakeOutcome::TimedOut { messages }),
                transport,
            ));
        }
        let inbound = match transport.read_message() {
            Ok(Some(message)) => message,
            Ok(None) => {
                return Ok((
                    conclude(&streaming, drained, HandshakeOutcome::Closed { messages }),
                    transport,
                ));
            }
            // A silent peer is an outcome, not a transport failure: the run
            // reached a conclusion and the conclusion is "no answer".
            Err(TransportError::Timeout) => {
                return Ok((
                    conclude(&streaming, drained, HandshakeOutcome::TimedOut { messages }),
                    transport,
                ));
            }
            Err(error) => return Err(CastError::Transport(error)),
        };
        messages += 1;
        if streaming.is_some() {
            drained += 1;
        }
        let (outbound, effects) = session.on_message(&inbound);
        for message in &outbound {
            transport.send(message).map_err(CastError::Transport)?;
        }
        for effect in effects {
            match effect {
                Effect::StartStreaming { presentation_url } => match drain {
                    // Keep serving: a real sink still has its reply to this
                    // trigger, then SETUP, PLAY and TEARDOWN to send.
                    Drain::UntilQuiet(limit) if limit > 0 => streaming = Some(presentation_url),
                    _ => {
                        return Ok((
                            HandshakeOutcome::Streaming {
                                presentation_url,
                                drained,
                            },
                            transport,
                        ));
                    }
                },
                Effect::Terminate => {
                    return Ok((HandshakeOutcome::Terminated, transport));
                }
                // A pause before the session ever started is meaningless; keep
                // listening rather than inventing a terminal state.
                Effect::PauseStreaming => {}
            }
        }
        // Once the sink has issued PLAY and we have answered it, the negotiation is
        // complete; the sink is now expecting media and will send no further
        // negotiation messages.
        if let Some(url) = &streaming
            && matches!(
                inbound.start,
                StartLine::Request {
                    method: Method::Play,
                    ..
                }
            )
        {
            return Ok((
                HandshakeOutcome::Streaming {
                    presentation_url: url.clone(),
                    drained,
                },
                transport,
            ));
        }
        if messages >= message_budget {
            return Ok((
                conclude(
                    &streaming,
                    drained,
                    HandshakeOutcome::MessageBudget { messages },
                ),
                transport,
            ));
        }
        // A drain ends when the sink has said everything it is going to say,
        // which is what a real sink does after PLAY: it waits for media.
        if let (Some(url), Drain::UntilQuiet(limit)) = (&streaming, drain)
            && drained >= limit
        {
            return Ok((
                HandshakeOutcome::Streaming {
                    presentation_url: url.clone(),
                    drained,
                },
                transport,
            ));
        }
    }
}

/// Resolve a terminal condition: once the sink has asked for a session, every
/// ending — EOF, silence, budget, deadline — is that session, not a failure.
fn conclude(
    streaming: &Option<String>,
    drained: usize,
    fallback: HandshakeOutcome,
) -> HandshakeOutcome {
    match streaming {
        Some(presentation_url) => HandshakeOutcome::Streaming {
            presentation_url: presentation_url.clone(),
            drained,
        },
        None => fallback,
    }
}

/// Whether a message is the sink's opening `OPTIONS`, which is recorded so a
/// trace can show who spoke first.
pub fn is_sink_options(message: &RtspMessage) -> bool {
    matches!(
        &message.start,
        StartLine::Request {
            method: Method::Options,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfd::parameters::SourceParameters;
    use std::collections::VecDeque;

    /// A scripted sink: pre-loaded replies, recording what the source sent.
    struct ScriptedSink {
        reads: VecDeque<Vec<u8>>,
        /// Shared so a test can inspect what was written after the transport
        /// takes ownership of the sink.
        written: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    }

    impl ScriptedSink {
        fn new(replies: Vec<RtspMessage>) -> Self {
            Self {
                reads: replies
                    .into_iter()
                    .map(|message| message.to_bytes())
                    .collect(),
                written: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            }
        }

        fn recorder(&self) -> std::rc::Rc<std::cell::RefCell<Vec<u8>>> {
            std::rc::Rc::clone(&self.written)
        }
    }

    impl Read for ScriptedSink {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let Some(chunk) = self.reads.pop_front() else {
                return Ok(0);
            };
            let take = chunk.len().min(buffer.len());
            buffer[..take].copy_from_slice(&chunk[..take]);
            if take < chunk.len() {
                self.reads.push_front(chunk[take..].to_vec());
            }
            Ok(take)
        }
    }

    impl Write for ScriptedSink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written.borrow_mut().extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

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

    /// The messages the source wrote, parsed back.
    fn written_messages(bytes: &[u8]) -> Vec<RtspMessage> {
        let mut messages = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let (message, consumed) = RtspMessage::parse(rest).expect("parse written");
            messages.push(message);
            rest = &rest[consumed..];
        }
        messages
    }

    #[test]
    fn a_sink_that_follows_the_nominal_exchange_reaches_streaming() {
        let sink = ScriptedSink::new(vec![
            request(Method::Options, 1, Vec::new()),
            request(Method::GetParameter, 2, Vec::new()),
            trigger(3, "SETUP"),
        ]);
        let recorder = sink.recorder();
        let mut session = session();
        let (outcome, _transport) = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            32,
            Drain::None,
        )
        .expect("handshake");
        assert!(outcome.is_streaming(), "{outcome:?}");
        assert_eq!(
            outcome,
            HandshakeOutcome::Streaming {
                presentation_url: "rtsp://192.168.49.1/wfd1.0/streamid=0".to_owned(),
                drained: 0,
            }
        );

        // The source answered each request with a CSeq-matching reply, so the
        // sink can match them.
        let written = written_messages(&recorder.borrow());
        let cseqs: Vec<&str> = written
            .iter()
            .filter(|message| matches!(message.start, StartLine::Status { .. }))
            .filter_map(|message| message.header("cseq"))
            .collect();
        for expected in ["1", "2", "3"] {
            assert!(
                cseqs.contains(&expected),
                "missing reply for cseq {expected}: {cseqs:?}"
            );
        }
    }

    #[test]
    fn a_silent_sink_ends_the_handshake_as_timed_out_not_as_success() {
        // A peer with no replies and no EOF: the interesting silence.
        let sink = ScriptedSink::new(Vec::new());
        let mut session = session();
        let started = Instant::now();
        let (outcome, _) = handshake(
            RtspTransport::new(sink),
            &mut session,
            started, // deadline already reached
            32,
            Drain::None,
        )
        .expect("handshake");
        assert_eq!(outcome, HandshakeOutcome::TimedOut { messages: 0 });
        assert!(!outcome.is_streaming());
        assert!(outcome.describe().contains("stopped answering"));
    }

    #[test]
    fn a_sink_that_closes_is_reported_as_closed_with_its_message_count() {
        let sink = ScriptedSink::new(vec![request(Method::Options, 1, Vec::new())]);
        let mut session = session();
        let (outcome, _) = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            32,
            Drain::None,
        )
        .expect("handshake");
        assert_eq!(outcome, HandshakeOutcome::Closed { messages: 1 });
    }

    #[test]
    fn a_chatty_sink_that_never_concludes_hits_the_message_budget() {
        // A peer that answers forever must not hang the tool.
        let replies = (0..10)
            .map(|index| request(Method::Options, index, Vec::new()))
            .collect();
        let sink = ScriptedSink::new(replies);
        let mut session = session();
        let (outcome, _) = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            3,
            Drain::None,
        )
        .expect("handshake");
        assert_eq!(outcome, HandshakeOutcome::MessageBudget { messages: 3 });
    }

    #[test]
    fn a_teardown_trigger_ends_the_handshake_as_terminated() {
        let sink = ScriptedSink::new(vec![
            request(Method::Options, 1, Vec::new()),
            trigger(2, "TEARDOWN"),
        ]);
        let mut session = session();
        let (outcome, _) = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            32,
            Drain::None,
        )
        .expect("handshake");
        assert_eq!(outcome, HandshakeOutcome::Terminated);
        assert_eq!(session.phase(), crate::wfd::session::Phase::Terminated);
    }

    #[test]
    fn the_trace_covers_both_directions_of_the_whole_exchange() {
        let sink = ScriptedSink::new(vec![
            request(Method::Options, 1, Vec::new()),
            trigger(2, "SETUP"),
        ]);
        let mut session = session();
        let (_outcome, _transport) = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            32,
            Drain::None,
        )
        .expect("handshake");
        let rendered = session.trace().render();
        assert!(rendered.contains("sink -> source  OPTIONS"), "{rendered}");
        assert!(rendered.contains("source -> sink  200 OK"), "{rendered}");
        assert!(rendered.contains("wfd_trigger_method: SETUP"), "{rendered}");
        assert!(rendered.starts_with("note: recorded without hardware validation"));
    }

    #[test]
    fn the_opening_options_is_traced_as_ours_not_as_the_sinks() {
        // The trace's whole job is showing who said what; labelling our own
        // opening as the sink's would invert the diagnosis.
        let sink = ScriptedSink::new(Vec::new());
        let mut session = session();
        let _ = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            32,
            Drain::None,
        )
        .expect("handshake");
        let entries = session.trace().entries();
        assert_eq!(entries.len(), 1, "only the opening OPTIONS was exchanged");
        assert_eq!(
            entries[0].direction,
            crate::wfd::trace::Direction::SourceToSink,
            "the opening OPTIONS is ours"
        );
        assert!(matches!(
            &entries[0].message.start,
            StartLine::Request {
                method: Method::Options,
                ..
            }
        ));
        let rendered = session.trace().render();
        assert!(rendered.contains("source -> sink  OPTIONS *"), "{rendered}");
        assert!(!rendered.contains("sink -> source  OPTIONS"), "{rendered}");
    }

    #[test]
    fn a_sink_status_line_does_not_loop_the_exchange() {
        let sink = ScriptedSink::new(vec![
            RtspMessage::status(200, "OK", 1, Vec::new()),
            request(Method::Options, 2, Vec::new()),
        ]);
        let mut session = session();
        let (outcome, _) = handshake(
            RtspTransport::new(sink),
            &mut session,
            Instant::now() + std::time::Duration::from_secs(5),
            32,
            Drain::None,
        )
        .expect("handshake");
        // The stray status produced no reply, so the exchange continued to EOF.
        assert_eq!(outcome, HandshakeOutcome::Closed { messages: 2 });
    }
}
