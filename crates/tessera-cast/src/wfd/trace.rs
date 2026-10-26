//! A session trace: every RTSP message in both directions, with both its bytes
//! and its interpretation.
//!
//! A Miracast failure without a trace is indistinguishable from a radio failure,
//! a permission failure, and a wrong bitfield. The trace is therefore not a
//! debugging afterthought — it is the thing that makes the untested layers
//! (ADR-0192 §7) reviewable at all, and it is what a maintainer compares against
//! a working sink's exchange.

use super::rtsp::{Method, RtspMessage, StartLine};

/// Which side sent a traced message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    SinkToSource,
    SourceToSink,
}

impl Direction {
    fn arrow(self) -> &'static str {
        match self {
            Self::SinkToSource => "sink -> source",
            Self::SourceToSink => "source -> sink",
        }
    }
}

/// One traced message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub direction: Direction,
    pub message: RtspMessage,
    /// Interpretation lines, one per fact worth naming (the `wfd_*` parameters
    /// it carried, the trigger method, and so on).
    pub notes: Vec<String>,
}

/// An ordered trace of a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trace {
    entries: Vec<TraceEntry>,
    /// Set when the trace was recorded from an exchange we could not verify
    /// against real hardware, so a dump never reads as evidence.
    unverified: bool,
}

impl Trace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark this trace as recorded without hardware validation
    /// (ADR-0192 §7). The rendered dump says so; a trace must never be mistaken
    /// for proof that a sink accepted the exchange.
    pub fn unverified(mut self) -> Self {
        self.unverified = true;
        self
    }

    pub fn record(&mut self, direction: Direction, message: RtspMessage, notes: Vec<String>) {
        self.entries.push(TraceEntry {
            direction,
            message,
            notes,
        });
    }

    pub fn entries(&self) -> &[TraceEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Render the trace: one block per message, showing the summary, the named
    /// facts, and a hex dump of the exact bytes.
    ///
    /// The hex dump is not decoration. When a sink rejects an exchange, the
    /// difference is almost always a byte — a wrong length, a missing CR, a
    /// header spelled differently — and a pretty summary hides exactly that.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.unverified {
            out.push_str(
                "note: recorded without hardware validation; a sink has not accepted this exchange\n\n",
            );
        }
        if self.entries.is_empty() {
            out.push_str("(no messages)\n");
            return out;
        }
        for (index, entry) in self.entries.iter().enumerate() {
            out.push_str(&format!(
                "[{index}] {}  {}\n",
                entry.direction.arrow(),
                summarize(&entry.message)
            ));
            for note in &entry.notes {
                out.push_str(&format!("      {note}\n"));
            }
            let bytes = entry.message.to_bytes();
            for line in hex_dump(&bytes) {
                out.push_str(&format!("      | {line}\n"));
            }
            out.push('\n');
        }
        out
    }
}

/// A one-line summary of a message's start line and body size.
fn summarize(message: &RtspMessage) -> String {
    let start = match &message.start {
        StartLine::Request { method, uri, .. } => format!("{} {uri}", method.as_str()),
        StartLine::Status { code, reason, .. } => format!("{code} {reason}"),
    };
    let cseq = message
        .header("cseq")
        .map(|value| format!(" cseq={value}"))
        .unwrap_or_default();
    format!("{start}{cseq} ({} byte body)", message.body.len())
}

/// A classic offset/hex/ascii dump, 16 bytes per line.
pub fn hex_dump(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(16)
        .enumerate()
        .map(|(index, chunk)| {
            let offset = index * 16;
            let mut hex = String::with_capacity(48);
            for position in 0..16 {
                match chunk.get(position) {
                    Some(byte) => hex.push_str(&format!("{byte:02x} ")),
                    None => hex.push_str("   "),
                }
            }
            let ascii: String = chunk
                .iter()
                .map(|&byte| {
                    if (0x20..=0x7e).contains(&byte) {
                        char::from(byte)
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("{offset:06x}  {hex} {ascii}")
        })
        .collect()
}

/// The facts worth naming for an inbound message, so the trace explains what the
/// state machine acted on rather than leaving the reader to parse bytes.
pub fn describe_inbound(message: &RtspMessage) -> Vec<String> {
    let mut notes = Vec::new();
    match &message.start {
        StartLine::Request { method, .. } => {
            notes.push(format!("method: {}", method.as_str()));
            if let Some(require) = message.header("require") {
                notes.push(format!("require: {require}"));
            }
        }
        StartLine::Status { code, .. } => {
            notes.push(format!("status: {code}"));
        }
    }
    if let Ok(parameters) = message.parameters() {
        for (name, value) in parameters {
            notes.push(format!("{name}: {value}"));
        }
    }
    notes
}

/// Whether a method expects a body-carrying reply.
pub fn expects_body(method: Method) -> bool {
    matches!(method, Method::GetParameter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: Method, cseq: u32, body: Vec<u8>) -> RtspMessage {
        RtspMessage::request(method, "rtsp://x/wfd1.0", cseq, body)
    }

    #[test]
    fn the_dump_shows_offsets_hex_and_ascii() {
        // 13 bytes: exactly one row. CR and LF render as dots.
        let lines = hex_dump(b"RTSP/1.0 200\r\n");
        assert_eq!(lines.len(), 1, "13 bytes fit in one 16-byte row");
        assert!(lines[0].starts_with("000000  "), "{}", lines[0]);
        assert!(lines[0].ends_with("RTSP/1.0 200.."), "{}", lines[0]);
        // A long message breaks into 16-byte rows with increasing offsets.
        let long = hex_dump(&[b'A'; 20]);
        assert_eq!(long.len(), 2);
        assert!(long[1].starts_with("000010  "), "{}", long[1]);
        assert!(long[1].trim_end().ends_with("AAAA"), "{}", long[1]);
    }

    #[test]
    fn a_trace_names_what_the_state_machine_acted_on() {
        let mut trace = Trace::new();
        trace.record(
            Direction::SinkToSource,
            request(
                Method::SetParameter,
                3,
                b"wfd_trigger_method: SETUP\r\n".to_vec(),
            ),
            describe_inbound(&request(
                Method::SetParameter,
                3,
                b"wfd_trigger_method: SETUP\r\n".to_vec(),
            )),
        );
        let rendered = trace.render();
        assert!(rendered.contains("sink -> source"), "{rendered}");
        assert!(
            rendered.contains("SET_PARAMETER rtsp://x/wfd1.0 cseq=3"),
            "{rendered}"
        );
        assert!(rendered.contains("wfd_trigger_method: SETUP"), "{rendered}");
        assert!(rendered.contains("| 000000  "), "{rendered}");
    }

    #[test]
    fn an_unverified_trace_never_reads_as_proof() {
        let mut trace = Trace::new().unverified();
        trace.record(
            Direction::SourceToSink,
            RtspMessage::status(200, "OK", 1, Vec::new()),
            Vec::new(),
        );
        let rendered = trace.render();
        assert!(
            rendered.starts_with("note: recorded without hardware validation"),
            "{rendered}"
        );
    }

    #[test]
    fn an_empty_trace_says_so() {
        assert_eq!(Trace::new().render(), "(no messages)\n");
    }

    #[test]
    fn status_messages_are_summarized_with_their_code() {
        let summary = summarize(&RtspMessage::status(404, "Not Found", 9, Vec::new()));
        assert_eq!(summary, "404 Not Found cseq=9 (0 byte body)");
    }

    #[test]
    fn only_get_parameter_expects_a_body() {
        assert!(expects_body(Method::GetParameter));
        for method in [
            Method::Options,
            Method::SetParameter,
            Method::Setup,
            Method::Play,
            Method::Pause,
            Method::Teardown,
        ] {
            assert!(!expects_body(method), "{method} must not expect a body");
        }
    }
}
