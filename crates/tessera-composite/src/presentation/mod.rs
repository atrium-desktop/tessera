//! GPU presentation, frame damage, scanout planning, and capture-stream production.
//! Delivery and security policy are supplied by the session through callbacks.

mod damage;
mod geometry;
mod overlay;
mod scanout;
mod scheduler;
mod stream;

pub use damage::{
    AssessedFrameDamage, ClientDamage, ClientDamageTracker, DamageAssessment, FrameDamage,
    SurfaceDamageFrame, SurfacePlacement, frame_damage_scope, logical_rects_to_frame,
    logical_to_physical, offscreen_damage_to_target, union_frame_damage,
};
pub use geometry::{clamp_logical_region, crop_rgba, logical_rect_to_physical};
pub use overlay::{
    OverlayChange, OverlayPlacement, OverlayPlan, OverlayPlaneFacts, OverlayRejectReason,
    OverlayRejection, OverlaySceneFacts, OverlayState, OverlaySurfaceFacts, OverlayTelemetry,
    intersects_any, overlay_transition_damage, plan_overlay,
};
pub use scanout::{
    PrimaryPlanePlan, PrimaryPlaneState, SCANOUT_SUSTAINED_BLOCK_WARNING, ScanoutLogEvent,
    ScanoutRejectReason, ScanoutRejection, ScanoutSceneFacts, ScanoutTelemetry, ScanoutTotals,
    evaluate_scene, evaluate_surface, overlay_plan_permitted, plan_scanout,
};
pub use scheduler::{
    ActivationChange, PresentationAvailability, PresentationOutcome, PresentationScheduler,
    RetryWait, presentation_groups_pending,
};
pub use stream::{
    BlitFailure, CaptureCursorState, DRM_FORMAT_XRGB8888, DmabufCapture, DmabufStream,
    GeometryAction, LIVENESS_INTERVAL, OffscreenDriveEffects, OffscreenGeometry,
    OffscreenShmTarget, OffscreenStream, OffscreenStreamDrive, OffscreenStreamStage,
    OffscreenTarget, OutputStream, OutputStreams, PendingSlotFrame, PresentedFrameRef,
    SLOT_FENCE_TIMEOUT, STREAM_SLOT_COUNT, SampledDamage, SlotRing, SlotState, StreamCursorBlit,
    StreamPainter, begin_opaque_frame, blit_presented_frame, crop_stream_frame, damage_in_target,
    enumerate_slot_ring, fence_signaled, full_target_damage, output_cursor_blit,
    render_offscreen_stream_dmabuf, render_offscreen_stream_shm, resolve_output_rect,
    submit_capture_frame, window_stream_cursor, window_stream_render_due,
};

/// The trailing `" (detail)"` suffix for the most recent gfx FFI error, or
/// the empty string when gfx recorded none. Shared by every crate that
/// formats gfx handle errors.
pub fn gfx_last_error_detail() -> String {
    let mut info: optics_gfx_sys::optics_gfx_error_info = unsafe { std::mem::zeroed() };
    unsafe { optics_gfx_sys::optics_gfx_get_last_error(&mut info) };
    if info.message.is_null() {
        return String::new();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(info.message) };
    format!(" ({})", message.to_string_lossy())
}
