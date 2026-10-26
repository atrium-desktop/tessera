//! Media streaming pipeline (MPEG-2 TS over RTP, UDP delivery, and frame sources).
//!
//! A WFD source delivers H.264 video packaged in MPEG-2 Transport Stream (ISO/IEC 13818-1)
//! inside RTP datagrams (RFC 2250, payload type 33) transmitted over UDP to the sink's
//! negotiated RTP port.

pub mod desktop;
pub mod h264;
pub mod pattern;
pub mod portal;
pub mod stream;
pub mod udp;

pub use desktop::{DesktopCaptureError, DesktopCaptureSource};
pub use h264::H264AccessUnitReader;
pub use pattern::{FrameSource, TEST_PATTERN_720P, TestPatternSource};
pub use portal::{PipeWireStreamInfo, PortalError, ScreenCastSession};
pub use stream::{StreamConfig, StreamStats, StreamTermination, stream};
pub use udp::UdpMediaSender;
