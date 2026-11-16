//! The casting use cases.
//!
//! Miracast's control plane is sink-driven: the **source is the RTSP server**,
//! the sink connects to it, and the sink sends the first `OPTIONS`. [`accept`]
//! waits for that connection and [`handshake_as_server`] serves the exchange.
//! [`handshake`] (source speaks first) exists only for the scripted diagnostics
//! that invert the roles; it is not the protocol.
//!
//! Separating the server loop from the protocol core is not ceremony: the
//! direction is where implementations silently diverge, and a sink that is
//! waiting to speak first deadlocks against a source that insists on opening.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// Which peer opens the RTSP exchange.
///
/// Miracast documents the **sink** as the RTSP client: the source listens, the
/// sink connects and sends the first `OPTIONS`. [`Opening::Source`] is the
/// scripted diagnostic that inverts only who speaks first.
/// [`Opening::Initiator`] is the source-driven compatibility path: the sink
/// connects, answers the source's `OPTIONS`, and then waits for the source to
/// drive `GET_PARAMETER`/`SET_PARAMETER`/trigger. An Android 9 sink
/// (`stagefright/1.2`) was observed timing out after 30 s when the source only
/// answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// The source sends `OPTIONS` and then waits. Diagnostic only.
    Source,
    /// The sink connects and speaks first. Miracast's documented direction.
    Sink,
    /// The sink connected; the source drives the exchange and triggers in M5.
    Initiator,
}

/// How far the source-driven exchange has progressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitiatorPhase {
    /// Waiting for the sink's `200` to our opening `OPTIONS`.
    AwaitingOptionsReply,
    /// Sent M3 `GET_PARAMETER`; waiting for M4.
    AwaitingM4,
    /// Sent M5 `SET_PARAMETER`; the sink now drives the trigger/`SETUP`.
    AwaitingM6,
    /// M5 is acknowledged; the sink's trigger and `SETUP` follow.
    Done,
}

/// Why accepting the sink's connection did not produce a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptError {
    /// No sink connected before the deadline.
    Timeout,
    /// The session owner asked to stop while waiting.
    Stopped,
    /// The listener could not accept.
    Io(String),
}

impl std::fmt::Display for AcceptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => formatter
                .write_str("no sink connected to the source's RTSP port before the deadline"),
            Self::Stopped => {
                formatter.write_str("the session was stopped while waiting for a sink")
            }
            Self::Io(reason) => write!(formatter, "accept failed: {reason}"),
        }
    }
}

impl std::error::Error for AcceptError {}

/// Wait for the sink to open the RTSP connection this source is serving.
///
/// The listener is polled rather than left blocking so the caller's deadline is
/// authoritative; a sink that never connects is a named outcome, not a hang.
pub fn accept(listener: &TcpListener, deadline: Instant) -> Result<TcpStream, AcceptError> {
    accept_until(listener, deadline, None)
}

/// [`accept`] with an owner-supplied stop flag, so a service can cancel a
/// pending session without killing a process.
pub fn accept_until(
    listener: &TcpListener,
    deadline: Instant,
    stop: Option<&AtomicBool>,
) -> Result<TcpStream, AcceptError> {
    listener
        .set_nonblocking(true)
        .map_err(|error| AcceptError::Io(error.to_string()))?;
    loop {
        if stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) {
            return Err(AcceptError::Stopped);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|error| AcceptError::Io(error.to_string()))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(AcceptError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(AcceptError::Io(error.to_string())),
        }
    }
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
    /// The session could not build a message it must send (for example an
    /// unserializable parameter body).
    Session(String),
}

impl std::fmt::Display for CastError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Session(reason) => write!(formatter, "session: {reason}"),
        }
    }
}

impl std::error::Error for CastError {}

/// Drive the WFD handshake over an already-connected transport, with the source
/// speaking first.
///
/// This inverts Miracast's direction and exists only for the scripted-sink
/// diagnostics and their tests. Production uses [`handshake_as_server`].
pub fn handshake<S: Read + Write>(
    transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    deadline: Instant,
    message_budget: usize,
    drain: Drain,
) -> Result<(HandshakeOutcome, RtspTransport<S>), CastError> {
    handshake_with(
        transport,
        session,
        deadline,
        message_budget,
        drain,
        Opening::Source,
    )
}

/// Drive the WFD handshake as Miracast specifies it: the sink connected to this
/// source's RTSP port and **the sink speaks first**.
///
/// The source must not send an unsolicited `OPTIONS`; a sink that is waiting to
/// send its own `OPTIONS` may treat one as a protocol error or simply deadlock
/// waiting for its answer.
pub fn handshake_as_server<S: Read + Write>(
    transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    deadline: Instant,
    message_budget: usize,
    drain: Drain,
) -> Result<(HandshakeOutcome, RtspTransport<S>), CastError> {
    handshake_with(
        transport,
        session,
        deadline,
        message_budget,
        drain,
        Opening::Sink,
    )
}

/// Drive the WFD handshake as a source-driven initiator: after the sink
/// connects, the source runs `OPTIONS` → `GET_PARAMETER` → `SET_PARAMETER` →
/// trigger, then the sink's `SETUP`/`PLAY` complete the session.
///
/// This is the compatibility path for sinks that answer `OPTIONS` and then wait
/// for the source to continue (observed: Android 9 `stagefright/1.2`, which tore
/// the session down after 30 s of source silence).
pub fn handshake_as_initiator<S: Read + Write>(
    transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    deadline: Instant,
    message_budget: usize,
    drain: Drain,
) -> Result<(HandshakeOutcome, RtspTransport<S>), CastError> {
    handshake_with(
        transport,
        session,
        deadline,
        message_budget,
        drain,
        Opening::Initiator,
    )
}

fn handshake_with<S: Read + Write>(
    mut transport: RtspTransport<S>,
    session: &mut WfdSourceSession,
    deadline: Instant,
    message_budget: usize,
    drain: Drain,
    opening: Opening,
) -> Result<(HandshakeOutcome, RtspTransport<S>), CastError> {
    // Sink-first needs no unsolicited message. Source-first and source-driven
    // both open with the WFD `OPTIONS`.
    if opening != Opening::Sink {
        let opening_message = opening_options();
        transport
            .send(&opening_message)
            .map_err(CastError::Transport)?;
        // Recorded as outbound: routing it through `on_message` would have
        // labelled our own message as the sink's, which is exactly the confusion
        // a trace exists to prevent.
        session.note_outbound(
            &opening_message,
            vec![
                if opening == Opening::Initiator {
                    "opening OPTIONS (source drives the exchange)"
                } else {
                    "opening OPTIONS (source speaks first, diagnostic mode)"
                }
                .to_owned(),
            ],
        );
    }

    let mut initiator_phase =
        (opening == Opening::Initiator).then_some(InitiatorPhase::AwaitingOptionsReply);

    let mut messages = 0;
    // Some Android sinks open the TCP connection only to clear a stale session
    // and then wait for the source to speak. Observed on the reference TV
    // (`user-agent: stagefright/1.2`, empty `Session:`): without this kickoff it
    // received our 200 and closed before M1.
    let mut kickoff_sent = false;
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

        // Source-driven exchange: answer the sink's 200s with M3, M5 and M7.
        if let Some(phase) = initiator_phase {
            let cseq = inbound
                .header("cseq")
                .and_then(|value| value.parse::<u32>().ok());
            let acknowledged = matches!(inbound.start, StartLine::Status { code: 200, .. });
            let next = match (phase, acknowledged, cseq) {
                (InitiatorPhase::AwaitingOptionsReply, true, Some(1)) => {
                    let message = RtspMessage::request(Method::GetParameter, "*", 2, Vec::new());
                    transport.send(&message).map_err(CastError::Transport)?;
                    session.note_outbound(
                        &message,
                        vec!["M3 GET_PARAMETER (ask the sink for its parameters)".to_owned()],
                    );
                    Some(InitiatorPhase::AwaitingM4)
                }
                (InitiatorPhase::AwaitingM4, true, Some(2)) => {
                    let body = session
                        .parameters_trigger_body()
                        .map_err(|error| CastError::Session(error.to_string()))?;
                    let message = RtspMessage::request(Method::SetParameter, "*", 3, body);
                    transport.send(&message).map_err(CastError::Transport)?;
                    session.note_outbound(
                        &message,
                        vec!["M5 SET_PARAMETER (source parameters + SETUP trigger)".to_owned()],
                    );
                    Some(InitiatorPhase::AwaitingM6)
                }
                (InitiatorPhase::AwaitingM6, true, Some(3)) => {
                    // M5 is acknowledged. The trigger (when requested) was part
                    // of the M5 body; the sink drives SETUP and PLAY.
                    Some(InitiatorPhase::Done)
                }
                _ => None,
            };
            if let Some(next) = next {
                initiator_phase = Some(next);
            }
        }

        let stale_reset = opening == Opening::Sink
            && matches!(
                inbound.start,
                StartLine::Request {
                    method: Method::Teardown,
                    ..
                }
            )
            && inbound
                .header("session")
                .is_none_or(|value| value.trim().is_empty());
        if stale_reset && !kickoff_sent {
            let kickoff = opening_options();
            transport.send(&kickoff).map_err(CastError::Transport)?;
            session.note_outbound(
                &kickoff,
                vec!["opening OPTIONS after a stale-session reset".to_owned()],
            );
            kickoff_sent = true;
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

/// The source's opening `OPTIONS` (M1): `*` URI, WFD requirement, and a
/// user agent. The `Require` header is what tells a WFD sink this is a session
/// request rather than a stray RTSP probe.
fn opening_options() -> RtspMessage {
    let mut message = RtspMessage::request(Method::Options, "*", 1, Vec::new());
    message
        .headers
        .push(("require".to_owned(), "org.wfa.wfd1.0".to_owned()));
    message
        .headers
        .push(("user-agent".to_owned(), "tessera-cast".to_owned()));
    message
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
mod tests;
