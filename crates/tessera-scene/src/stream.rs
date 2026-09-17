//! Capture stream negotiation and owned frame payloads, independent of transport.
use std::sync::Arc;
use tessera_types::WindowId;

/// The memory byte order of one stream frame pixel blob. Four
/// bytes per pixel, tightly packed rows of `stride` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StreamPixelFormat {
    /// Blue, green, red, alpha in memory order; alpha is always 255.
    /// Matches PipeWire's `SPA_VIDEO_FORMAT_BGRA`/`BGRx`.
    Bgra8,
    /// Red, green, blue, alpha in memory order; alpha is always 255.
    Rgba8,
    /// Direct GPU dmabuf zero-copy export (ADR-0055). `drm_format` is the
    /// DRM FOURCC format code; `modifier` is the DRM format modifier.
    Dmabuf { drm_format: u32, modifier: u64 },
}

/// What a stream request streams (ADR-0054). `Output` is the
/// version-5 behavior: the whole focused output; version 29 adds an optional
/// connector selector streaming only that output's region of the desktop
/// frame (ADR-0126). `Window` renders one window's complete surface tree
/// offscreen, independent of the desktop frame (ADR-0127): occlusion,
/// minimization, and foreign workspaces never leak foreign pixels into the
/// stream. The stream freezes with geometry-change event when
/// the target's size changes and ends when a streamed window closes or a
/// streamed connector disappears (PipeWire consumers negotiate a fixed
/// size).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StreamTarget {
    /// The whole desktop frame (version-5 default), or — when `output`
    /// names a live connector (version 29) — only that output's physical
    /// rectangle of the desktop frame. The serde shape is backward
    /// compatible: an absent selector serializes as `{"type":"Output"}`,
    /// exactly what pre-29 peers exchange. An unknown connector is refused
    /// at stream start; a connector that disappears mid-stream ends the
    /// stream.
    Output {
        /// Connector name (e.g. "HDMI-A-1") from
        /// output enumeration; `None` streams the whole desktop.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// One toplevel, rendered offscreen at the capture scale of the output
    /// it sits on (ADR-0127). The window keeps its real content whether it
    /// is visible, occluded, minimized, or on another workspace; popups
    /// extending past the toplevel bounds are clipped. The id comes from a
    /// user-consent window pick window pick.
    Window { window: WindowId },
}

impl Default for StreamTarget {
    /// The version-5 default: the whole desktop frame, no connector
    /// selector.
    fn default() -> Self {
        StreamTarget::Output { output: None }
    }
}

impl StreamTarget {
    /// Whether this target is the whole output with no connector selector
    /// (the serde-skipped default).
    pub fn is_output(&self) -> bool {
        matches!(self, StreamTarget::Output { output: None })
    }
}

/// How a stream treats the compositor cursor (version 29, ADR-0127). The
/// mode is negotiated at stream start and composited wherever the cursor
/// position falls inside the captured region: dmabuf streams (output and
/// window) draw the theme sprite into the capture target on the GPU; SHM
/// output streams receive a frame with the cursor blended into the readback
/// pixels. Only the compositor's theme cursor is embedded — a
/// client-provided cursor surface is ordinary scene content and appears in
/// output streams regardless of mode (it is never part of a window
/// stream's surface tree). On the software-cursor fallback (nested or
/// degraded direct display) the presented frame already contains the
/// cursor, so `embedded` adds nothing and `hidden` cannot subtract it
/// there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamCursorMode {
    /// The cursor never appears in stream frames beyond what the scene
    /// itself carries (the pre-29 behavior).
    #[default]
    Hidden,
    /// The cursor is composited into stream frames.
    Embedded,
}

/// Geometry and format of a stream started through
/// stream creation (ADR-0052). A zero-copy dmabuf stream
/// (protocol 25) also carries its slot table; the writer transfers the
/// descriptors on the blob channel right after the `StreamOutputStarted`
/// reply, one `0xfd`-marked `SCM_RIGHTS` message per slot in slot order.
#[derive(Debug)]
pub struct StreamInfo {
    pub stream_id: u64,
    pub width: u32,
    pub height: u32,
    pub format: StreamPixelFormat,
    /// dmabuf slot table (protocol 25); `None` for SHM streams.
    pub slots: Option<StreamSlotTable>,
}

/// The fixed dmabuf slot ring of a zero-copy stream (protocol 25). Every
/// slot shares `stride`/`byte_len`; `fds` holds one exportable descriptor
/// per slot, in slot order. The descriptors stay valid for the stream's
/// lifetime; frames reference slots by index and carry no descriptor.
#[derive(Debug)]
pub struct StreamSlotTable {
    pub stride: u32,
    pub byte_len: u64,
    pub fds: Vec<std::os::fd::OwnedFd>,
}

/// One presented frame pushed to a stream's connection. The two transports
/// are distinct variants so a slot-referenced frame can never accidentally
/// send (or be expected to send) a pixel blob.
#[derive(Debug, Clone)]
pub enum StreamFramePayload {
    /// SHM stream: `pixels` are raw, tightly packed (row stride `stride`)
    /// and transferred as a sealed memfd after the JSON
    /// stream frame metadata; the blob intentionally is not part
    /// of the JSON schema. Shared cheaply between streams fanning out from
    /// one readback.
    Pixels(StreamPixelFrame),
    /// Zero-copy dmabuf stream (protocol 25): the frame references one slot
    /// of the table transferred at start and no blob follows the JSON
    /// stream frame header.
    Slot(StreamSlotFrame),
}

impl StreamFramePayload {
    pub fn stream_id(&self) -> u64 {
        match self {
            Self::Pixels(frame) => frame.stream_id,
            Self::Slot(frame) => frame.stream_id,
        }
    }
}

/// SHM frame metadata plus the pixel bytes transferred as a sealed memfd.
#[derive(Debug, Clone)]
pub struct StreamPixelFrame {
    pub stream_id: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: StreamPixelFormat,
    pub damage: Vec<tessera_types::Rect>,
    pub dropped: u64,
    pub pixels: Arc<[u8]>,
}

/// dmabuf frame metadata (protocol 25): `slot` names the descriptor the
/// consumer already holds and `byte_len` is the slot's byte length; no blob
/// follows on the wire.
#[derive(Debug, Clone)]
pub struct StreamSlotFrame {
    pub stream_id: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: StreamPixelFormat,
    pub damage: Vec<tessera_types::Rect>,
    pub dropped: u64,
    pub slot: u32,
    pub byte_len: u64,
}
