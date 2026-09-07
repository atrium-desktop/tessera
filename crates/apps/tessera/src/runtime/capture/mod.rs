//! Frame capture orchestration for the compositor runtime.
//!
//! The pure pixel domain (readback staging, encoding, geometry, naming)
//! lives in `tessera-capture` (ADR-0147 principle: facts in a shared
//! crate). This module owns only the orchestrating worker and its
//! wiring into the runtime: wakeups, completions, security gating, and
//! the journal/IPC effects that a capture completion triggers.

mod worker;

pub use tessera_capture::{
    CaptureCursor, PendingReadback, StreamPixels, clamp_logical_region, flux_last_error_detail,
    logical_rect_to_physical, read_captured_pixels, read_captured_pixels_owned,
    request_frame_readback, screenshot_uri_list,
};
// Test-only capture helpers (BGRA conversion and PNG encoding assertions).
#[cfg(test)]
pub(crate) use tessera_capture::{encode_rgba_capture, stream_pixels};
pub(super) use worker::{
    CaptureCompletion, CaptureTarget, CaptureWorker, PendingCapture, queue_captured_pixels,
    refuse_capture_target,
};
