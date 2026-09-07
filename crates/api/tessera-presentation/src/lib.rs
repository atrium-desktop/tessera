//! Pure presentation-domain value layer for tessera (ADR-0039/0077/0101
//! damage and presentation pipeline, ADR-0052/0126/0127/0130 stream
//! pipeline).
//!
//! This crate is the presentation pipeline's **value layer**: frame damage
//! verdicts, the per-consumer damage assessment split, swapchain slot-ring
//! repaint history, surface damage baselines, the logical→physical
//! damage mapping, the backdrop effect-cache region algebra, and the
//! capture-stream registry — pacing, slot rings, damage sampling, and the
//! GPU capture transport. The machines here are pure over `tessera-model`
//! geometry types, `tessera-ipc` wire values, and flux value handles — no
//! compositor state, no worker lanes, no IPC server of its own, no side
//! effects beyond the GPU transport calls their names declare.
//!
//! # Boundary
//!
//! The orchestrating damage assessment (change-signal sampling across the
//! server, shell, notifications, and wallpaper; the decision of when a
//! frame is presented at all) lives in the composition root; it consumes
//! this crate's value types and feeds client surfaces to
//! [`ClientDamageTracker`] through the narrow [`SurfaceDamageFrame`]
//! observation seam.
//!
//! The capture-stream registry drives itself: the composition root hands it
//! the frame clock, a security gate, and the delivery lane, and receives
//! back the outcomes it must apply (freezes, ends, published frames). The
//! one irreducible composition-root dependency — painting a window's scene
//! content — is the single-method [`stream::StreamPainter`] port. Facts and
//! state machines live in this shared crate; decisions about what is
//! observed, what is secure, and what is published live with the process
//! that owns them.
//!
//! That split follows the same principle as `tessera-capture`: facts live
//! in a shared crate, decisions live with the process that owns them.

mod backdrop;
mod damage;
mod geometry;
mod scanout;
mod scheduler;
mod stream;

pub use backdrop::{
    BACKDROP_DOWNSAMPLE, BackdropCaptureRegion, backdrop_refresh_regions, blur_regions_in_capture,
    intersect_blur_regions, refresh_regions_covering_material_change, slot_material_changed,
};
pub use damage::{
    AssessedFrameDamage, ClientDamage, ClientDamageTracker, DamageAssessment, FrameDamage,
    SurfaceDamageFrame, composite_repaint_for_slot, frame_damage_render_area,
    logical_rects_to_frame, logical_to_physical, record_composite_present, union_frame_damage,
};
pub use geometry::{clamp_logical_region, crop_rgba, logical_rect_to_physical};
pub use scanout::{
    PrimaryPlanePlan, PrimaryPlaneState, ScanoutRejectReason, ScanoutRejection, ScanoutSceneFacts,
    ScanoutTelemetry, evaluate_scene, evaluate_surface, plan_scanout,
};
pub use scheduler::{
    ActivationChange, PresentationAvailability, PresentationOutcome, PresentationScheduler,
};
pub use stream::{
    BlitFailure, CaptureCursorState, DRM_FORMAT_XRGB8888, DmabufCapture, DmabufStream,
    GeometryAction, LIVENESS_INTERVAL, OutputStream, OutputStreams, PendingSlotFrame,
    PresentedFrameRef, SLOT_FENCE_TIMEOUT, STREAM_SLOT_COUNT, SampledDamage, SlotRing, SlotState,
    StreamCursorBlit, StreamPainter, WindowDriveEffects, WindowShmTarget, WindowStream,
    WindowStreamDrive, WindowStreamStage, WindowTreeGeometry, begin_opaque_frame,
    blit_presented_frame, crop_stream_frame, damage_in_target, enumerate_slot_ring, fence_signaled,
    full_target_damage, output_cursor_blit, render_window_stream_dmabuf, render_window_stream_shm,
    resolve_output_rect, submit_capture_frame, window_stream_cursor, window_stream_render_due,
};

/// The trailing `" (detail)"` suffix for the most recent flux FFI error, or
/// the empty string when flux recorded none. Shared by every crate that
/// formats flux handle errors.
pub fn flux_last_error_detail() -> String {
    let mut info: flux_sys::flux_error_info = unsafe { std::mem::zeroed() };
    unsafe { flux_sys::flux_get_last_error(&mut info) };
    if info.message.is_null() {
        return String::new();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(info.message) };
    format!(" ({})", message.to_string_lossy())
}
