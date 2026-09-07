//! The capture-stream value layer (ADR-0052/0126/0127/0130): the live
//! registry, slot rings, damage sampling, and the GPU capture transport,
//! driven through the narrow [`StreamPainter`] content port.

mod crop;
mod drive;
mod registry;
mod ring;
mod transport;

pub use crop::{crop_stream_frame, resolve_output_rect};
pub use drive::{WindowDriveEffects, WindowStreamDrive};
pub use registry::{
    GeometryAction, LIVENESS_INTERVAL, OutputStream, OutputStreams, SampledDamage, WindowStream,
    WindowStreamStage, damage_in_target, full_target_damage, window_stream_render_due,
};
pub use ring::{SLOT_FENCE_TIMEOUT, STREAM_SLOT_COUNT, SlotRing, SlotState};
pub use transport::{
    BlitFailure, CaptureCursorState, DRM_FORMAT_XRGB8888, DmabufCapture, DmabufStream,
    PendingSlotFrame, PresentedFrameRef, StreamCursorBlit, StreamPainter, WindowShmTarget,
    WindowTreeGeometry, begin_opaque_frame, blit_presented_frame, enumerate_slot_ring,
    fence_signaled, output_cursor_blit, render_window_stream_dmabuf, render_window_stream_shm,
    submit_capture_frame, window_stream_cursor,
};
