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
}

#[test]
fn an_initiator_source_drives_m3_and_m5_then_waits_for_setup() {
    // The reference Android sink answered OPTIONS and then waited 30 s for
    // the source to send M3. After M5 the sink drives the trigger and SETUP;
    // sending M7 from the source made it close.
    let sink = ScriptedSink::new(vec![
        RtspMessage::status(200, "OK", 1, Vec::new()),
        RtspMessage::status(200, "OK", 2, Vec::new()),
        RtspMessage::status(200, "OK", 3, Vec::new()),
        request(Method::Setup, 5, Vec::new()),
    ]);
    let recorder = sink.recorder();
    let mut session = session();
    let (outcome, _) = handshake_as_initiator(
        RtspTransport::new(sink),
        &mut session,
        Instant::now() + std::time::Duration::from_secs(5),
        32,
        Drain::None,
    )
    .expect("handshake");
    assert!(outcome.is_streaming(), "{outcome:?}");

    let written = written_messages(&recorder.borrow());
    assert!(
        written.iter().any(|message| matches!(
            &message.start,
            StartLine::Request {
                method: Method::GetParameter,
                ..
            }
        )),
        "M3 GET_PARAMETER is missing: {written:?}"
    );
    let set_parameters = written
        .iter()
        .filter(|message| {
            matches!(
                &message.start,
                StartLine::Request {
                    method: Method::SetParameter,
                    ..
                }
            )
        })
        .count();
    assert_eq!(set_parameters, 1, "M5 only: {written:?}");
    assert!(
        written.iter().any(|message| {
            String::from_utf8_lossy(&message.body).contains("wfd_video_formats")
        }),
        "M5 must advertise the source parameters"
    );
    assert!(
        written.iter().any(|message| {
            String::from_utf8_lossy(&message.body).contains("wfd_trigger_method: SETUP")
        }),
        "M5 must carry the SETUP trigger"
    );
}

#[test]
fn a_stale_teardown_makes_the_source_open_the_exchange() {
    // The reference Android 9 sink connected, sent a session-less TEARDOWN,
    // and closed when the source stayed silent. The source must answer the
    // reset and then open with OPTIONS.
    let mut stale = request(Method::Teardown, 1, Vec::new());
    stale.headers.push(("session".to_owned(), String::new()));
    let sink = ScriptedSink::new(vec![
        stale,
        request(Method::Options, 2, Vec::new()),
        request(Method::GetParameter, 3, Vec::new()),
        trigger(4, "SETUP"),
    ]);
    let recorder = sink.recorder();
    let mut session = session();
    let (outcome, _) = handshake_as_server(
        RtspTransport::new(sink),
        &mut session,
        Instant::now() + std::time::Duration::from_secs(5),
        32,
        Drain::None,
    )
    .expect("handshake");
    assert!(outcome.is_streaming(), "{outcome:?}");

    let written = written_messages(&recorder.borrow());
    assert!(
        written.iter().any(|message| matches!(
            &message.start,
            StartLine::Request {
                method: Method::Options,
                ..
            }
        )),
        "the source must open with OPTIONS after the stale reset: {written:?}"
    );
}

#[test]
fn as_a_server_the_source_waits_for_the_sink_to_speak_first() {
    // Miracast: the sink is the RTSP client. The source must answer the
    // sink's OPTIONS, not pre-empt it with its own.
    let sink = ScriptedSink::new(vec![
        request(Method::Options, 1, Vec::new()),
        request(Method::GetParameter, 2, Vec::new()),
        trigger(3, "SETUP"),
    ]);
    let recorder = sink.recorder();
    let mut session = session();
    let (outcome, _) = handshake_as_server(
        RtspTransport::new(sink),
        &mut session,
        Instant::now() + std::time::Duration::from_secs(5),
        32,
        Drain::None,
    )
    .expect("handshake");
    assert!(outcome.is_streaming(), "{outcome:?}");

    let written = written_messages(&recorder.borrow());
    assert!(
        !written.is_empty(),
        "the source must answer the sink's opening OPTIONS"
    );
    assert!(
        matches!(written[0].start, StartLine::Status { .. }),
        "the source's first message must be a response, not an unsolicited OPTIONS: {:?}",
        written[0].start
    );
    assert_eq!(
        written[0].header("cseq"),
        Some("1"),
        "the answer must echo the sink's CSeq"
    );
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
