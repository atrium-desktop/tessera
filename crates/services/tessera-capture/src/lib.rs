//! Pure pixel-capture domain for tessera (ADR-0147-adjacent seam;
//! ADR-0037/0052/0055 pipeline).
//!
//! This crate is the compositor capture pipeline's **value layer**:
//! GPU readback staging, RGBA/BGRA encoding, cursor compositing, capture
//! geometry, and screenshot naming. Everything here is a pure function
//! over pixel buffers and rectangles plus the GPU readback handle — no
//! compositor state, no IPC, no filesystem writes beyond naming.
//!
//! # Boundary
//!
//! The orchestrating worker (event-loop integration, journal effects,
//! security gating, IPC replies) lives in the composition root; it
//! consumes this crate's value types. That split follows the ADR-0147
//! principle: facts (pixel formats, geometry rules, name formats) live
//! in a shared crate, decisions (when and what to capture) live with
//! the process that owns them.

mod encoding;
mod geometry;
mod output;

pub use encoding::{
    CapturedPixels, CapturedPixelsSource, CaptureCursor, PendingReadback, encode_capture,
    encode_rgba_capture, flux_last_error_detail, read_captured_pixels, read_captured_pixels_owned,
    read_picked_pixel, request_frame_readback, stream_pixels, StreamPixels,
};
pub use geometry::{clamp_logical_region, crop_rgba, logical_rect_to_physical};
pub use output::{atomic_write_capture, screenshot_uri_list};
