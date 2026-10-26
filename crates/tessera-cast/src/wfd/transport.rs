//! RTSP over a byte stream: framing a message-oriented protocol on a socket.
//!
//! Generic over `Read + Write` on purpose: the framing rules (partial reads,
//! pipelined messages, clean EOF, truncated EOF) are exactly the things that go
//! wrong on a real socket and are exactly the things a scripted duplex can prove
//! without one.
//!
//! Timeouts belong to the socket, not here: a transport that silently invented a
//! deadline would hide a sink that never answers.

use std::io::{Read, Write};

use super::rtsp::{RtspError, RtspMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The link failed.
    Io(String),
    /// The peer did not answer inside the socket's timeout.
    ///
    /// Separate from [`Self::Io`] on purpose: an expired read timeout is the
    /// single most common WFD failure (the sink said nothing), and reporting it
    /// as "Resource temporarily unavailable" hides that from the operator.
    Timeout,
    /// The message framing was invalid. Carried separately because a framing
    /// error means the peer is not speaking RTSP as expected, while an I/O error
    /// means the link died — different diagnoses.
    Framing(RtspError),
    /// The stream ended in the middle of a message.
    TruncatedOnEof(usize),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(reason) => write!(formatter, "transport: {reason}"),
            Self::Timeout => {
                formatter.write_str("the peer did not answer before the socket timeout")
            }
            Self::Framing(error) => write!(formatter, "framing: {error}"),
            Self::TruncatedOnEof(buffered) => write!(
                formatter,
                "stream ended mid-message with {buffered} bytes buffered"
            ),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<RtspError> for TransportError {
    fn from(error: RtspError) -> Self {
        Self::Framing(error)
    }
}

/// One RTSP connection.
#[derive(Debug)]
pub struct RtspTransport<S> {
    stream: S,
    /// Bytes read but not yet consumed as a message. A socket hands over
    /// arbitrary chunks, so a message boundary never coincides with a read.
    pending: Vec<u8>,
}

impl<S: Read + Write> RtspTransport<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            pending: Vec::new(),
        }
    }

    /// Access the underlying transport stream.
    pub fn stream(&self) -> &S {
        &self.stream
    }

    /// Mutably access the underlying transport stream.
    pub fn stream_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Read one message.
    ///
    /// `Ok(None)` is a *clean* EOF: the peer closed with no partial message
    /// buffered. A partial message at EOF is an error, because a half-received
    /// reply is not "no reply".
    pub fn read_message(&mut self) -> Result<Option<RtspMessage>, TransportError> {
        let mut chunk = [0u8; 4096];
        loop {
            // Consume anything already buffered before reading more, so a
            // pipelined burst does not need another read to be dispatched.
            if let Some(message) = self.take_buffered()? {
                return Ok(Some(message));
            }
            let read = self
                .stream
                .read(&mut chunk)
                .map_err(|error| match error.kind() {
                    // A socket with SO_RCVTIMEO reports its expiry as
                    // WouldBlock or TimedOut, not as a distinct timeout.
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                        TransportError::Timeout
                    }
                    _ => TransportError::Io(error.to_string()),
                })?;
            if read == 0 {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Err(TransportError::TruncatedOnEof(self.pending.len()))
                };
            }
            self.pending.extend_from_slice(&chunk[..read]);
        }
    }

    /// Parse one message out of the pending buffer, if it is complete.
    fn take_buffered(&mut self) -> Result<Option<RtspMessage>, TransportError> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        match RtspMessage::parse(&self.pending) {
            Ok((message, consumed)) => {
                self.pending.drain(..consumed);
                Ok(Some(message))
            }
            // Not enough bytes yet: a truncation at this point is "read more",
            // which is only an error once the stream reports EOF.
            Err(RtspError::Truncated { .. }) => Ok(None),
            Err(error) => Err(TransportError::Framing(error)),
        }
    }

    /// Write one message and flush it.
    ///
    /// Every send is flushed: an RTSP exchange is a lock-step request/response
    /// protocol, so a buffered request is a stalled session.
    pub fn send(&mut self, message: &RtspMessage) -> Result<(), TransportError> {
        let bytes = message.to_bytes();
        self.stream
            .write_all(&bytes)
            .map_err(|error| TransportError::Io(error.to_string()))?;
        self.stream
            .flush()
            .map_err(|error| TransportError::Io(error.to_string()))
    }

    /// Unconsumed bytes (diagnostics: a non-empty buffer after a failure is the
    /// peer's unfinished message).
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfd::rtsp::{Method, StartLine};
    use std::collections::VecDeque;

    /// A duplex whose reads return pre-scripted chunks, so a test controls
    /// exactly where message boundaries fall.
    struct ScriptedDuplex {
        reads: VecDeque<Vec<u8>>,
        written: Vec<u8>,
        eof: bool,
    }

    impl ScriptedDuplex {
        fn new(chunks: Vec<&[u8]>) -> Self {
            Self {
                reads: chunks.into_iter().map(<[u8]>::to_vec).collect(),
                written: Vec::new(),
                eof: false,
            }
        }

        /// Script a clean EOF after the given chunks.
        fn with_eof(chunks: Vec<&[u8]>) -> Self {
            let mut duplex = Self::new(chunks);
            duplex.eof = true;
            duplex
        }
    }

    impl Read for ScriptedDuplex {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let Some(chunk) = self.reads.pop_front() else {
                if self.eof {
                    return Ok(0);
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "script exhausted",
                ));
            };
            let take = chunk.len().min(buffer.len());
            buffer[..take].copy_from_slice(&chunk[..take]);
            if take < chunk.len() {
                // Put the remainder back so a small buffer still works.
                self.reads.push_front(chunk[take..].to_vec());
            }
            Ok(take)
        }
    }

    impl Write for ScriptedDuplex {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_message_split_across_reads_is_reassembled() {
        // The classic socket failure: the header and body arrive separately.
        let mut transport = RtspTransport::new(ScriptedDuplex::new(vec![
            b"RTSP/1.0 200 OK\r\nContent-Length: 5\r\n",
            b"\r\nhel",
            b"lo",
        ]));
        let message = transport
            .read_message()
            .expect("read")
            .expect("a message, not EOF");
        assert!(matches!(message.start, StartLine::Status { code: 200, .. }));
        assert_eq!(message.body, b"hello");
    }

    #[test]
    fn pipelined_messages_come_out_one_at_a_time_from_one_read() {
        let first = RtspMessage::status(200, "OK", 1, Vec::new()).to_bytes();
        let second = RtspMessage::status(200, "OK", 2, Vec::new()).to_bytes();
        let mut burst = first;
        burst.extend_from_slice(&second);
        let mut transport = RtspTransport::new(ScriptedDuplex::with_eof(vec![&burst]));
        let one = transport.read_message().expect("read").expect("first");
        assert_eq!(one.header("cseq"), Some("1"));
        let two = transport.read_message().expect("read").expect("second");
        assert_eq!(two.header("cseq"), Some("2"));
        assert!(
            transport.read_message().expect("read").is_none(),
            "then a clean EOF"
        );
    }

    #[test]
    fn a_socket_read_timeout_is_a_timeout_not_a_generic_io_error() {
        struct Silent;
        impl Read for Silent {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                // What SO_RCVTIMEO expiry actually looks like.
                Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Resource temporarily unavailable",
                ))
            }
        }
        impl Write for Silent {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                Ok(buffer.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut transport = RtspTransport::new(Silent);
        assert_eq!(
            transport.read_message(),
            Err(TransportError::Timeout),
            "an expired read timeout must not read as 'transport: Resource temporarily unavailable'"
        );
    }

    #[test]
    fn a_clean_eof_is_none_and_a_partial_eof_is_an_error() {
        let mut clean = RtspTransport::new(ScriptedDuplex::with_eof(Vec::new()));
        assert!(clean.read_message().expect("read").is_none());

        let mut partial = RtspTransport::new(ScriptedDuplex::with_eof(vec![
            b"RTSP/1.0 200 OK\r\nContent-Length: 9\r\n\r\nab",
        ]));
        match partial.read_message() {
            Err(TransportError::TruncatedOnEof(buffered)) => assert!(buffered > 0),
            other => panic!("a half-received reply must not read as 'no reply': {other:?}"),
        }
    }

    #[test]
    fn a_malformed_message_is_a_framing_error_not_an_io_error() {
        let mut transport =
            RtspTransport::new(ScriptedDuplex::with_eof(vec![b"NOT-RTSP-AT-ALL\r\n\r\n"]));
        match transport.read_message() {
            Err(TransportError::Framing(_)) => {}
            other => panic!("expected a framing error, got {other:?}"),
        }
    }

    #[test]
    fn send_writes_the_encoded_message_including_its_length() {
        let mut transport = RtspTransport::new(ScriptedDuplex::new(Vec::new()));
        let message = RtspMessage::request(
            Method::SetParameter,
            "rtsp://x/wfd1.0",
            7,
            b"a: b\r\n".to_vec(),
        );
        transport.send(&message).expect("send");
        let written = &transport.stream.written;
        let text = String::from_utf8_lossy(written);
        assert!(
            text.starts_with("SET_PARAMETER rtsp://x/wfd1.0 RTSP/1.0\r\n"),
            "{text}"
        );
        assert!(text.contains("content-length: 6\r\n"), "{text}");
        // What was written parses back to the same message.
        let (parsed, consumed) = RtspMessage::parse(written).expect("parse");
        assert_eq!(consumed, written.len());
        assert_eq!(parsed, message);
    }

    #[test]
    fn a_small_read_buffer_still_reassembles_a_large_message() {
        // 9 KiB body forces multiple 4 KiB reads through the internal chunk.
        let body = "x".repeat(9000);
        let wire = format!(
            "RTSP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut transport = RtspTransport::new(ScriptedDuplex::with_eof(vec![wire.as_bytes()]));
        let message = transport.read_message().expect("read").expect("message");
        assert_eq!(message.body.len(), 9000);
    }
}
