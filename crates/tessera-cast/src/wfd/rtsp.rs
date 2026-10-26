//! RTSP message framing and `wfd_*` parameter bodies (RFC 2326).
//!
//! Only what a WFD source needs: a start line, headers with a mandatory
//! `Content-Length` for bodies, and a byte body. Parsing is strict — an
//! unparseable message from a sink is a real failure mode that must surface as
//! an error with the offending bytes, never as a silently defaulted field.
//!
//! Bodies use the `text/parameters` media type WFD specifies: one
//! `name: value` pair per line, CRLF-terminated.

use std::collections::BTreeMap;
use std::fmt;

/// An RTSP start line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartLine {
    /// `METHOD uri RTSP/1.0`
    Request {
        method: Method,
        uri: String,
        version: String,
    },
    /// `RTSP/1.0 code reason`
    Status {
        version: String,
        code: u16,
        reason: String,
    },
}

/// The RTSP methods a WFD session uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Options,
    GetParameter,
    SetParameter,
    Setup,
    Play,
    Pause,
    Teardown,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Options => "OPTIONS",
            Self::GetParameter => "GET_PARAMETER",
            Self::SetParameter => "SET_PARAMETER",
            Self::Setup => "SETUP",
            Self::Play => "PLAY",
            Self::Pause => "PAUSE",
            Self::Teardown => "TEARDOWN",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "OPTIONS" => Self::Options,
            "GET_PARAMETER" => Self::GetParameter,
            "SET_PARAMETER" => Self::SetParameter,
            "SETUP" => Self::Setup,
            "PLAY" => Self::Play,
            "PAUSE" => Self::Pause,
            "TEARDOWN" => Self::Teardown,
            _ => return None,
        })
    }
}

impl fmt::Display for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A parsed RTSP message. `M1`–`M7` are carried by these; the WFD payloads ride
/// in [`RtspMessage::body`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspMessage {
    pub start: StartLine,
    /// Headers in wire order, lower-cased names, so lookups are
    /// case-insensitive as RFC 2326 requires.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtspError {
    /// The message ended before its headers or body did.
    Truncated { wanted: usize, got: usize },
    /// The start line was not a request or a status line.
    BadStartLine(String),
    /// A header line had no `:` separator.
    BadHeader(String),
    /// `Content-Length` was missing, unparseable, or disagreed with the body.
    BadContentLength(String),
    /// A body was not valid UTF-8 where `text/parameters` requires it.
    BadBody(String),
}

impl fmt::Display for RtspError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { wanted, got } => {
                write!(
                    formatter,
                    "truncated RTSP message: wanted {wanted} bytes, got {got}"
                )
            }
            Self::BadStartLine(line) => write!(formatter, "bad RTSP start line: {line:?}"),
            Self::BadHeader(line) => write!(formatter, "bad RTSP header line: {line:?}"),
            Self::BadContentLength(value) => write!(formatter, "bad Content-Length: {value:?}"),
            Self::BadBody(reason) => write!(formatter, "bad RTSP body: {reason}"),
        }
    }
}

impl std::error::Error for RtspError {}

impl RtspMessage {
    /// A request whose body announces its own `Content-Length`.
    pub fn request(method: Method, uri: &str, cseq: u32, body: Vec<u8>) -> Self {
        let mut headers = vec![("cseq".to_owned(), cseq.to_string())];
        if !body.is_empty() {
            headers.push(("content-type".to_owned(), "text/parameters".to_owned()));
            headers.push(("content-length".to_owned(), body.len().to_string()));
        }
        Self {
            start: StartLine::Request {
                method,
                uri: uri.to_owned(),
                version: "RTSP/1.0".to_owned(),
            },
            headers,
            body,
        }
    }

    /// A status reply whose body announces its own `Content-Length`.
    pub fn status(code: u16, reason: &str, cseq: u32, body: Vec<u8>) -> Self {
        let mut headers = vec![("cseq".to_owned(), cseq.to_string())];
        if !body.is_empty() {
            headers.push(("content-type".to_owned(), "text/parameters".to_owned()));
            headers.push(("content-length".to_owned(), body.len().to_string()));
        }
        Self {
            start: StartLine::Status {
                version: "RTSP/1.0".to_owned(),
                code,
                reason: reason.to_owned(),
            },
            headers,
            body,
        }
    }

    /// The first header with `name` (case-insensitive), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        let wanted = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == wanted)
            .map(|(_, value)| value.as_str())
    }

    /// The body parsed as a `text/parameters` map.
    pub fn parameters(&self) -> Result<BTreeMap<String, String>, RtspError> {
        parse_parameters(&self.body)
    }

    /// Serialize to wire bytes. Every body-carrying message gets exactly one
    /// `Content-Length`, computed here rather than trusted from a caller.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        match &self.start {
            StartLine::Request {
                method,
                uri,
                version,
            } => {
                out.push_str(&format!("{} {} {}\r\n", method.as_str(), uri, version));
            }
            StartLine::Status {
                version,
                code,
                reason,
            } => {
                out.push_str(&format!("{version} {code} {reason}\r\n"));
            }
        }
        for (name, value) in &self.headers {
            // Content-Length is recomputed below; skipping it here keeps a
            // caller's stale value from desynchronising the framing.
            if name == "content-length" {
                continue;
            }
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        if !self.body.is_empty() {
            out.push_str(&format!("content-length: {}\r\n", self.body.len()));
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }

    /// Parse one complete message from the front of `input`, returning it and
    /// the number of bytes consumed. A sink may pipeline messages, so the
    /// caller loops on the remainder.
    pub fn parse(input: &[u8]) -> Result<(Self, usize), RtspError> {
        let Some(header_end) = find_header_end(input) else {
            return Err(RtspError::Truncated {
                wanted: input.len() + 1,
                got: input.len(),
            });
        };
        // `header_end` indexes the start of the blank-line terminator.
        let head = std::str::from_utf8(&input[..header_end])
            .map_err(|error| RtspError::BadStartLine(error.to_string()))?;
        let mut lines = head.split("\r\n");
        let start_line = lines
            .next()
            .ok_or_else(|| RtspError::BadStartLine(String::new()))?;
        let start = parse_start_line(start_line)?;
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(RtspError::BadHeader(line.to_owned()));
            };
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
        let body_start = header_end + 4; // CRLF CRLF
        let length = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .map(|(_, value)| {
                value
                    .parse::<usize>()
                    .map_err(|_| RtspError::BadContentLength(value.clone()))
            })
            .transpose()?
            .unwrap_or(0);
        let body_end = body_start + length;
        if input.len() < body_end {
            return Err(RtspError::Truncated {
                wanted: body_end,
                got: input.len(),
            });
        }
        Ok((
            Self {
                start,
                headers,
                body: input[body_start..body_end].to_vec(),
            },
            body_end,
        ))
    }
}

fn parse_start_line(line: &str) -> Result<StartLine, RtspError> {
    let mut parts = line.split(' ');
    let first = parts
        .next()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| RtspError::BadStartLine(line.to_owned()))?;
    if let Some(method) = Method::parse(first) {
        let uri = parts
            .next()
            .ok_or_else(|| RtspError::BadStartLine(line.to_owned()))?;
        let version = parts
            .next()
            .ok_or_else(|| RtspError::BadStartLine(line.to_owned()))?;
        Ok(StartLine::Request {
            method,
            uri: uri.to_owned(),
            version: version.to_owned(),
        })
    } else {
        // `RTSP/1.0 200 OK`
        let code = parts
            .next()
            .ok_or_else(|| RtspError::BadStartLine(line.to_owned()))?
            .parse::<u16>()
            .map_err(|_| RtspError::BadStartLine(line.to_owned()))?;
        let reason = parts.collect::<Vec<_>>().join(" ");
        Ok(StartLine::Status {
            version: first.to_owned(),
            code,
            reason,
        })
    }
}

fn find_header_end(input: &[u8]) -> Option<usize> {
    input.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Parse a `text/parameters` body: `name: value` per CRLF line.
pub fn parse_parameters(body: &[u8]) -> Result<BTreeMap<String, String>, RtspError> {
    let text = std::str::from_utf8(body).map_err(|error| RtspError::BadBody(error.to_string()))?;
    let mut map = BTreeMap::new();
    for line in text.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(RtspError::BadBody(line.to_owned()));
        };
        map.insert(name.trim().to_owned(), value.trim().to_owned());
    }
    Ok(map)
}

/// Serialize a `text/parameters` body. Ordering is stable so fixtures and
/// traces stay comparable between runs.
pub fn write_parameters(parameters: &BTreeMap<String, String>) -> Vec<u8> {
    let mut out = String::new();
    for (name, value) in parameters {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3_get_parameter_response_round_trips() {
        let mut parameters = BTreeMap::new();
        parameters.insert("wfd_video_formats".to_owned(), "00 00 02 10".to_owned());
        parameters.insert("wfd_audio_codecs".to_owned(), "00 00000000 00".to_owned());
        let message = RtspMessage::status(200, "OK", 2, write_parameters(&parameters));
        let bytes = message.to_bytes();
        let (parsed, consumed) = RtspMessage::parse(&bytes).expect("parse");
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed, message);
        assert_eq!(parsed.parameters().expect("parameters"), parameters);
        assert_eq!(parsed.header("cseq"), Some("2"));
    }

    #[test]
    fn content_length_is_recomputed_not_trusted() {
        // A caller-supplied stale Content-Length must not desynchronise framing.
        let mut message = RtspMessage::request(
            Method::SetParameter,
            "rtsp://x/wfd1.0",
            5,
            b"a: b\r\n".to_vec(),
        );
        message
            .headers
            .push(("content-length".to_owned(), "999".to_owned()));
        let bytes = message.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.matches("content-length").count(), 1, "{text}");
        assert!(text.contains("content-length: 6\r\n"), "{text}");
        let (parsed, _) = RtspMessage::parse(&bytes).expect("parse");
        assert_eq!(parsed.body, b"a: b\r\n");
    }

    #[test]
    fn pipelined_messages_consume_exactly_one() {
        let first = RtspMessage::status(200, "OK", 1, Vec::new()).to_bytes();
        let second = RtspMessage::status(200, "OK", 2, Vec::new()).to_bytes();
        let mut stream = first.clone();
        stream.extend_from_slice(&second);
        let (parsed, consumed) = RtspMessage::parse(&stream).expect("first");
        assert_eq!(consumed, first.len());
        assert_eq!(parsed.header("cseq"), Some("1"));
        let (next, _) = RtspMessage::parse(&stream[consumed..]).expect("second");
        assert_eq!(next.header("cseq"), Some("2"));
    }

    #[test]
    fn m1_request_parses_its_parameters() {
        let wire = b"OPTIONS * RTSP/1.0\r\n\
CSeq: 0\r\n\
Require: org.wfa.wfd1.0\r\n\
\r\n";
        let (message, consumed) = RtspMessage::parse(wire).expect("parse");
        assert_eq!(consumed, wire.len());
        assert_eq!(
            message.start,
            StartLine::Request {
                method: Method::Options,
                uri: "*".to_owned(),
                version: "RTSP/1.0".to_owned(),
            }
        );
        assert_eq!(message.header("require"), Some("org.wfa.wfd1.0"));
        assert!(message.body.is_empty());
    }

    #[test]
    fn truncated_body_is_an_error_not_a_short_read() {
        let wire = b"RTSP/1.0 200 OK\r\nContent-Length: 10\r\n\r\nshort";
        assert!(matches!(
            RtspMessage::parse(wire),
            Err(RtspError::Truncated { .. })
        ));
    }

    #[test]
    fn malformed_message_is_rejected_with_its_offending_line() {
        assert!(matches!(
            RtspMessage::parse(b"RTSP/1.0 xxz NotANumber\r\n\r\n"),
            Err(RtspError::BadStartLine(_))
        ));
        assert!(matches!(
            RtspMessage::parse(b"RTSP/1.0 200 OK\r\nNoColonHere\r\n\r\n"),
            Err(RtspError::BadHeader(line)) if line == "NoColonHere"
        ));
    }

    #[test]
    fn parameter_body_rejects_a_line_without_a_separator() {
        assert!(matches!(
            parse_parameters(b"wfd_presentation_URL: rtsp://x\r\ngarbage\r\n"),
            Err(RtspError::BadBody(line)) if line == "garbage"
        ));
    }
}
