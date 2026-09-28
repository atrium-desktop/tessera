//! GPU presentation, frame damage, scanout planning, and capture-stream production.
//! Delivery and security policy are supplied by the session through callbacks.

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
