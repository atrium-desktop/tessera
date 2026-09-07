//! GPU capture transport for the stream pipeline: capture surfaces, slot
//! exports, readback targets, and the content-paint port.

use std::os::fd::{AsRawFd, OwnedFd};

use tessera_model::{Point, Size};

use super::ring::{STREAM_SLOT_COUNT, SlotRing};

/// DRM fourcc announced for dmabuf stream frames: the capture surfaces hold
/// opaque BGRA8 pixels, which is XRGB8888 on the wire.
pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

/// The opaque one-sample pass configuration every capture-surface frame is
/// recorded with (no clear-triggered MSAA; the compositor output does not
/// preserve destination alpha between layers).
pub fn begin_opaque_frame(
    canvas: &flux::Canvas,
    frame: &flux::Frame<'_>,
    clear: u32,
) -> Result<(), flux::Error> {
    debug_assert_eq!(clear >> 24, 0xff, "compositor pass clear must be opaque");
    canvas.begin_pass(
        frame,
        flux::CanvasPassOptions {
            clear: Some(clear),
            antialias: flux::CanvasAntialias::None,
            render_area: None,
            skip_stencil: true,
        },
    )
}

/// Logical geometry of one window's surface tree, resolved by the
/// composition root from the live model: the toplevel's identity, capture
/// scale in milli-units, physical extent, and logical placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowTreeGeometry {
    pub window: tessera_model::window::WindowId,
    pub scale_milli: u32,
    pub physical_width: u32,
    pub physical_height: u32,
    pub origin: Point,
    pub logical_size: Size,
}

/// Cursor state sampled at a capture trigger instant: logical output
/// coordinates, the effective theme shape, whether the compositor-owned
/// cursor was hidden, and whether the visible cursor came from a
/// client-provided surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureCursorState {
    pub position: (f32, f32),
    pub shape: u32,
    pub hidden: bool,
    pub client_surface: bool,
}

/// A GPU cursor draw into a capture-surface pass (ADR-0127): the theme
/// cursor's shape, its logical position relative to the capture target's
/// origin, and the target's render scale. Drawn after the frame content so
/// the sprite composits above it, clipped by the target extent.
#[derive(Debug, Clone, Copy)]
pub struct StreamCursorBlit {
    pub shape: u32,
    pub position: (f32, f32),
    pub scale: f32,
}

/// The cursor draw request for one embedded output dmabuf stream (ADR-0127):
/// the theme cursor's shape and its logical position relative to the
/// capture target's origin, when the cursor position (its hotspot) falls
/// inside the target's logical rect. `None` keeps the capture cursor-free.
pub fn output_cursor_blit(
    state: &CaptureCursorState,
    target_logical: Option<tessera_model::Rect>,
    scale: f32,
) -> Option<StreamCursorBlit> {
    let rect = target_logical?;
    let (cx, cy) = state.position;
    let inside = cx >= rect.origin.x as f32
        && cx < (rect.origin.x + rect.size.w) as f32
        && cy >= rect.origin.y as f32
        && cy < (rect.origin.y + rect.size.h) as f32;
    inside.then_some(StreamCursorBlit {
        shape: state.shape,
        position: (cx - rect.origin.x as f32, cy - rect.origin.y as f32),
        scale,
    })
}

/// The cursor draw request for an embedded window stream (ADR-0127): the
/// theme cursor's shape and its logical position relative to the window's
/// origin, when the cursor position (its hotspot) falls inside the window's
/// logical rect. `None` keeps the frame cursor-free; a client-provided
/// cursor surface is filtered by the caller (it is not part of the
/// window's surface tree and can never appear in a window stream).
pub fn window_stream_cursor(
    state: &CaptureCursorState,
    origin: Point,
    logical_size: Size,
) -> Option<(u32, (f32, f32))> {
    let (cx, cy) = state.position;
    let inside = cx >= origin.x as f32
        && cx < (origin.x + logical_size.w) as f32
        && cy >= origin.y as f32
        && cy < (origin.y + logical_size.h) as f32;
    inside.then_some((state.shape, (cx - origin.x as f32, cy - origin.y as f32)))
}

/// Non-blocking poll on a sync_file fence: a signaled (or errored) fence is
/// readable.
pub fn fence_signaled(fence: &OwnedFd) -> bool {
    let mut pollfd = libc::pollfd {
        fd: fence.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pollfd` references a live descriptor for the duration of the
    // call and a zero timeout never blocks.
    let signaled = unsafe { libc::poll(&mut pollfd, 1, 0) };
    signaled > 0
}

/// Why a capture-surface blit failed. The distinction that matters is
/// whether flux's ring advanced: only a submission moves it, and the slot
/// tracking in [`SlotRing`] must stay in lockstep with it.
#[derive(Debug)]
pub enum BlitFailure {
    /// Nothing was submitted; the stream may retry its next due frame.
    Retryable(String),
    /// The frame was submitted but the slot could not be exported; the ring
    /// position diverged and the stream must end.
    Submitted(String),
}

/// The just-presented frame, narrowed to the fields the capture blit
/// consumes. The composition root converts its backend descriptor (whose
/// type lives in the core tier) into this view.
#[derive(Debug, Clone, Copy)]
pub struct PresentedFrameRef<'a> {
    pub fd: &'a OwnedFd,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub modifier: u64,
}

/// Content port of the capture-stream machine: the one irreducible
/// composition-root dependency. Everything else — pacing, slot rings,
/// damage sampling, fence bookkeeping, the GPU transport — lives in this
/// crate; the composition root supplies only the scene content itself,
/// because painting it requires the renderer and the server (core tier).
pub trait StreamPainter {
    /// Paint one window stream frame: the window's complete surface tree
    /// into `canvas`, plus the embedded cursor sprite when `cursor` is set
    /// (its scale is `geometry.scale_milli` in milli-units).
    fn paint_window_frame(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        geometry: &WindowTreeGeometry,
        cursor: Option<(u32, (f32, f32))>,
    );
}

/// The cached offscreen readback target of an SHM window stream (ADR-0127):
/// created at stream start at the negotiated physical extent and reused for
/// every frame until the stream stops (a geometry restart is a fresh stream
/// with a fresh target).
pub struct WindowShmTarget {
    pub surface: flux::Surface,
    pub canvas: flux::Canvas,
}

impl WindowShmTarget {
    pub fn new(device: &flux::Device, width: u32, height: u32) -> Result<Self, String> {
        let surface =
            flux::Surface::offscreen_readback(device, width, height).map_err(|error| {
                format!(
                    "allocate window stream target: {error}{}",
                    crate::flux_last_error_detail()
                )
            })?;
        surface.prepare_readback().map_err(|error| {
            format!(
                "prepare window stream readback: {error}{}",
                crate::flux_last_error_detail()
            )
        })?;
        let canvas = flux::Canvas::new(&surface).map_err(|error| {
            format!(
                "create window stream canvas: {error}{}",
                crate::flux_last_error_detail()
            )
        })?;
        Ok(Self { surface, canvas })
    }
}

/// A frame rendered into a capture-surface slot whose acquire fence has not
/// signaled yet. The imported source image, when the frame was blitted from
/// the presented dma-buf, is held until the fence fires — the slot's GPU
/// work may sample it — and retired with the entry's drop. Per-window
/// renders carry no source: the renderer's texture cache owns the sampled
/// images.
pub struct PendingSlotFrame {
    pub slot: usize,
    pub fence: OwnedFd,
    pub source: Option<flux::Image>,
    pub sequence: u64,
    pub dropped: u64,
    pub submitted_at: std::time::Instant,
    /// Capture security generation snapshot at blit time (the SHM/readback
    /// path carries the same value through `CaptureCompletion::Stream`).
    /// A frame whose generation no longer matches at delivery covered a
    /// lock→unlock (or VT) boundary and is dropped, never handed to the
    /// consumer, mirroring the SHM worker's completion check.
    pub security_generation: u64,
    /// Damage sampled at capture time, delivered with the frame.
    pub damage: super::registry::SampledDamage,
}

/// GPU state of a zero-copy dmabuf stream (IPC protocol 25): the per-stream
/// capture surface and its canvas, the slot ring, and the frames awaiting
/// their acquire fence.
pub struct DmabufStream {
    pub surface: flux::Surface,
    pub canvas: flux::Canvas,
    pub modifier: u64,
    pub slot_stride: u32,
    pub slot_bytes: u64,
    pub ring: SlotRing,
    pub pending: Vec<PendingSlotFrame>,
    /// Set when a ring-full drop has been logged; cleared (with a recovery
    /// log) on the next successful submission, so a stall logs once per
    /// episode instead of once per dropped frame.
    pub ring_stalled: bool,
}

/// Everything a new dmabuf stream needs, built at start time: the capture
/// surface with its canvas, the announced modifier, and the slot table the
/// IPC layer transfers to the client.
pub struct DmabufCapture {
    pub surface: flux::Surface,
    pub canvas: flux::Canvas,
    pub modifier: u64,
    pub table: tessera_ipc::StreamSlotTable,
}

/// Create a dmabuf stream's capture surface and enumerate its slot ring
/// (IPC protocol 25): `STREAM_SLOT_COUNT` blank frames visit the slots in
/// order, exporting one descriptor per slot for the client's slot table.
/// The modifier is constrained to the presentation surface's, so the
/// post-present copy never crosses formats. Any failure is reported to the
/// caller, which falls back to SHM.
pub fn enumerate_slot_ring(
    device: &flux::Device,
    modifier: u64,
    width: u32,
    height: u32,
) -> Result<DmabufCapture, String> {
    let surface =
        flux::Surface::offscreen_dmabuf(device, width, height, &[modifier]).map_err(|error| {
            format!(
                "capture surface: {error}{}",
                crate::flux_last_error_detail()
            )
        })?;
    let canvas = flux::Canvas::new(&surface)
        .map_err(|error| format!("capture canvas: {error}{}", crate::flux_last_error_detail()))?;
    let mut fds: Vec<Option<OwnedFd>> = (0..STREAM_SLOT_COUNT).map(|_| None).collect();
    let mut stride = None;
    for (expected_slot, slot_fd) in fds.iter_mut().enumerate() {
        let frame = surface
            .begin_frame()
            .map_err(|error| format!("capture slot clear: {error}"))?;
        begin_opaque_frame(&canvas, &frame, flux::rgba(0, 0, 0, 255))
            .map_err(|error| format!("capture slot clear: {error}"))?;
        canvas
            .end_frame_checked()
            .map_err(|error| format!("capture slot clear: {error}"))?;
        let submitted = frame
            .submit()
            .map_err(|error| format!("capture slot clear: {error}"))?;
        submitted
            .present()
            .map_err(|error| format!("capture slot clear: {error}"))?;
        // Blocking export: start-up latency is acceptable, and the ring
        // order is the slot order.
        let export = surface.export_dmabuf().map_err(|error| {
            format!(
                "capture slot export: {error}{}",
                crate::flux_last_error_detail()
            )
        })?;
        if export.slot as usize != expected_slot {
            return Err(format!(
                "capture ring visited slot {}, expected {expected_slot}",
                export.slot
            ));
        }
        if export.width != width || export.height != height {
            return Err("capture slot extent mismatch".to_owned());
        }
        match stride {
            Some(known) if known != export.stride => {
                return Err("capture slots disagree on row stride".to_owned());
            }
            None => stride = Some(export.stride),
            _ => {}
        }
        *slot_fd = Some(export.fd);
    }
    let stride = stride.expect("the slot ring exported at least one slot");
    Ok(DmabufCapture {
        surface,
        canvas,
        modifier,
        table: tessera_ipc::StreamSlotTable {
            stride,
            byte_len: u64::from(stride) * u64::from(height),
            fds: fds
                .into_iter()
                .map(|fd| fd.expect("every ring slot exported"))
                .collect(),
        },
    })
}

/// Submit one rendered capture-surface frame and export its slot with the
/// explicit acquire fence (ADR-0055). Shared by the post-present blit and
/// the per-window capture-surface renders (ADR-0127); everything past the
/// submit advances flux's ring, so failures there are
/// [`BlitFailure::Submitted`].
pub fn submit_capture_frame(
    dmabuf: &DmabufStream,
    frame: flux::Frame<'_>,
) -> Result<(usize, OwnedFd), BlitFailure> {
    let submitted = frame
        .submit()
        .map_err(|error| BlitFailure::Retryable(format!("capture submit: {error}")))?;
    submitted
        .present()
        .map_err(|error| BlitFailure::Submitted(format!("capture present: {error}")))?;
    let export = dmabuf.surface.export_dmabuf_explicit().map_err(|error| {
        BlitFailure::Submitted(format!(
            "capture slot export: {error}{}",
            crate::flux_last_error_detail()
        ))
    })?;
    if export.slot as usize >= dmabuf.ring.states.len() {
        return Err(BlitFailure::Submitted(format!(
            "capture export reported out-of-ring slot {}",
            export.slot
        )));
    }
    let Some(fence) = export.acquire_fence else {
        return Err(BlitFailure::Submitted(
            "capture slot export returned no acquire fence".to_owned(),
        ));
    };
    Ok((export.slot as usize, fence))
}

/// Import the presented frame and copy it into the capture surface's next
/// ring slot, exporting the slot with its explicit acquire fence. Returns
/// the slot, the fence to poll, and the imported source image (retired when
/// the fence fires). The presented descriptor stays owned by the caller;
/// flux receives duplicates. `src_rect` (IPC protocol 29) samples only that
/// physical-pixel sub-region of the presented frame, for
/// connector-addressed streams; `None` copies the whole frame. `cursor`
/// (ADR-0127) composites the theme cursor sprite above the copied frame,
/// for streams that negotiated the embedded cursor mode; `draw_cursor`
/// receives the canvas and the draw request (the sprite lives with the
/// composition root).
#[allow(clippy::too_many_arguments)]
pub fn blit_presented_frame(
    device: &flux::Device,
    dmabuf: &mut DmabufStream,
    presented: &PresentedFrameRef<'_>,
    acquire_fence: Option<&OwnedFd>,
    src_rect: Option<&tessera_model::Rect>,
    cursor: Option<StreamCursorBlit>,
    draw_cursor: &mut dyn FnMut(&flux::Canvas, &StreamCursorBlit),
) -> Result<(usize, OwnedFd, flux::Image), BlitFailure> {
    let fd = presented
        .fd
        .try_clone()
        .map_err(|error| BlitFailure::Retryable(format!("duplicate presented dma-buf: {error}")))?;
    let fence = acquire_fence
        .map(|fence| {
            fence.try_clone().map_err(|error| {
                BlitFailure::Retryable(format!("duplicate acquire fence: {error}"))
            })
        })
        .transpose()?;
    // SAFETY: the backend's descriptor references a live dma-buf matching the
    // metadata it reported; the fence, when present, orders the capture pass
    // after the compositor's own rendering of that image. The import entry
    // points take `OwnedFd`s by value: on success flux consumes and closes
    // them, on error the `OwnedFd` drops close them — no leak, no
    // double-close.
    let import = unsafe {
        match fence {
            Some(fence) => flux::Image::import_dmabuf_with_acquire_fence(
                device,
                presented.width,
                presented.height,
                flux::Format::Bgra8Unorm,
                presented.modifier,
                fd,
                0,
                presented.stride,
                fence,
            ),
            None => flux::Image::import_dmabuf(
                device,
                presented.width,
                presented.height,
                flux::Format::Bgra8Unorm,
                presented.modifier,
                fd,
                0,
                presented.stride,
            ),
        }
    };
    let source = import.map_err(|error| {
        BlitFailure::Retryable(format!(
            "import presented dma-buf: {error}{}",
            crate::flux_last_error_detail()
        ))
    })?;
    let frame = dmabuf.surface.begin_frame().map_err(|error| {
        BlitFailure::Retryable(format!(
            "capture begin_frame: {error}{}",
            crate::flux_last_error_detail()
        ))
    })?;
    begin_opaque_frame(&dmabuf.canvas, &frame, flux::rgba(0, 0, 0, 255))
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    let (dst_w, dst_h) = dmabuf.surface.size();
    match src_rect {
        // Connector-addressed stream: sample only the output's sub-region
        // of the desktop frame into the (output-sized) capture surface.
        Some(rect) => {
            let full_w = presented.width as f32;
            let full_h = presented.height as f32;
            dmabuf.canvas.draw_image_opaque_sub(
                &source,
                0.0,
                0.0,
                dst_w as f32,
                dst_h as f32,
                rect.origin.x as f32 / full_w,
                rect.origin.y as f32 / full_h,
                rect.size.w as f32 / full_w,
                rect.size.h as f32 / full_h,
            );
        }
        None => dmabuf.canvas.draw_image_opaque(
            &source,
            0.0,
            0.0,
            presented.width as f32,
            presented.height as f32,
        ),
    }
    // Embedded cursor mode (ADR-0127): the theme cursor composits above the
    // captured frame, translated into the target's coordinate space.
    if let Some(cursor) = cursor {
        draw_cursor(&dmabuf.canvas, &cursor);
    }
    dmabuf
        .canvas
        .end_frame_checked()
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    let (slot, fence) = submit_capture_frame(dmabuf, frame)?;
    Ok((slot, fence, source))
}

/// Render the window's surface tree (plus the negotiated cursor, drawn by
/// the painter) into the SHM stream's cached readback target and submit the
/// frame. Mirrors the one-shot `begin_window_capture` sequence, but reuses
/// the per-stream surface and leaves the in-flight bookkeeping to the
/// caller.
pub fn render_window_stream_shm(
    device: &flux::Device,
    target: &WindowShmTarget,
    clear: u32,
    geometry: &WindowTreeGeometry,
    cursor: Option<(u32, (f32, f32))>,
    painter: &mut dyn StreamPainter,
) -> Result<(), String> {
    let mut frame = target.surface.begin_frame().map_err(|error| {
        format!(
            "begin window stream frame: {error}{}",
            crate::flux_last_error_detail()
        )
    })?;
    begin_opaque_frame(&target.canvas, &frame, clear)
        .map_err(|error| format!("begin window stream pass: {error}"))?;
    painter.paint_window_frame(device, &target.canvas, geometry, cursor);
    target
        .canvas
        .end_frame_checked()
        .map_err(|error| format!("end window stream pass: {error}"))?;
    frame
        .request_readback()
        .map_err(|error| format!("request window stream readback: {error}"))?;
    frame
        .submit()
        .and_then(flux::SubmittedFrame::present)
        .map_err(|error| format!("submit window stream frame: {error}"))?;
    Ok(())
}

/// Render the window's surface tree (plus the negotiated cursor) into the
/// dmabuf stream's next capture-surface slot and export it with its
/// explicit acquire fence (ADR-0127). Shares the submit/export tail with
/// the presented-frame blit so the ring bookkeeping — and its failure
/// semantics — stay identical.
pub fn render_window_stream_dmabuf(
    device: &flux::Device,
    dmabuf: &mut DmabufStream,
    clear: u32,
    geometry: &WindowTreeGeometry,
    cursor: Option<(u32, (f32, f32))>,
    painter: &mut dyn StreamPainter,
) -> Result<(usize, OwnedFd), BlitFailure> {
    let frame = dmabuf.surface.begin_frame().map_err(|error| {
        BlitFailure::Retryable(format!(
            "capture begin_frame: {error}{}",
            crate::flux_last_error_detail()
        ))
    })?;
    begin_opaque_frame(&dmabuf.canvas, &frame, clear)
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    painter.paint_window_frame(device, &dmabuf.canvas, geometry, cursor);
    dmabuf
        .canvas
        .end_frame_checked()
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    submit_capture_frame(dmabuf, frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    /// End-to-end capture-surface exercise: slot enumeration order, the
    /// explicit acquire fence of an exported frame, and slot reuse. Skipped
    /// without a dma-buf-capable Vulkan device on this machine.
    #[test]
    fn dmabuf_capture_surface_enumerates_and_exports_slots() {
        let Ok(device) = flux::Device::new_with_options(flux::DeviceOptions {
            headless: true,
            frames_in_flight: STREAM_SLOT_COUNT as u32,
            required_features: flux::DeviceFeatures::DMABUF,
            optional_features: flux::DeviceFeatures::DMABUF_SYNC_FILE,
            ..flux::DeviceOptions::default()
        }) else {
            return;
        };
        const LINEAR: u64 = 0; // DRM_FORMAT_MOD_LINEAR
        let Ok(surface) = flux::Surface::offscreen_dmabuf(&device, 64, 48, &[LINEAR]) else {
            return;
        };
        let canvas = flux::Canvas::new(&surface).unwrap();
        let mut exports = Vec::new();
        for expected_slot in 0..STREAM_SLOT_COUNT {
            let frame = surface.begin_frame().unwrap();
            begin_opaque_frame(&canvas, &frame, flux::rgba(0, 0, 0, 255)).unwrap();
            canvas.end_frame_checked().unwrap();
            frame.submit().unwrap().present().unwrap();
            let export = surface.export_dmabuf().unwrap();
            assert_eq!(export.slot as usize, expected_slot);
            assert_eq!((export.width, export.height), (64, 48));
            assert!(export.stride >= 64 * 4);
            exports.push(export);
        }
        // Every slot exported a distinct live descriptor.
        let raw: std::collections::HashSet<_> =
            exports.iter().map(|export| export.fd.as_raw_fd()).collect();
        assert_eq!(raw.len(), STREAM_SLOT_COUNT);

        // A submitted frame exports with a pollable acquire fence, and the
        // ring wrapped back to slot 0.
        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame(&canvas, &frame, flux::rgba(10, 20, 30, 255)).unwrap();
        canvas.end_frame_checked().unwrap();
        frame.submit().unwrap().present().unwrap();
        let export = surface.export_dmabuf_explicit().unwrap();
        assert_eq!(export.slot, 0);
        let fence = export
            .acquire_fence
            .expect("explicit export carries a fence");
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pollfd` references a live descriptor for the call.
        let result = unsafe { libc::poll(&mut pollfd, 1, 2000) };
        assert!(result > 0, "capture slot fence did not signal");
    }
}
