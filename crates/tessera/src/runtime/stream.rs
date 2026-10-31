//! Continuous output frame streaming (ADR-0052) — composition-root
//! orchestration.
//!
//! The registry lives on the compositor main loop. A stream is registered by
//! an IPC `StreamOutputStart` (authorization happens in `tessera-ipc` before the
//! request reaches this loop), is throttled to its `max_fps`, and fans one
//! shared GPU readback out to every due SHM stream. Delivery goes through
//! `tessera_ipc::Server::push_stream_frame`, whose bounded lane reports drops
//! back so the stream's cumulative `dropped` counter stays accurate.
//!
//! A client that explicitly opts in (IPC protocol 25) gets the zero-copy
//! dmabuf transport instead: a per-stream exportable capture surface receives
//! a GPU copy of each presented frame, the client learns the fixed slot ring
//! once at start, and frame events reference a slot without a pixel blob. A
//! delivered slot stays consumer-owned — pinned — until the client's
//! `StreamBufferRelease`; only a free slot may be rendered into again.
//!
//! Window targets (ADR-0127) do not crop the shared frame: each window
//! stream renders the window's complete surface tree into its own cached
//! offscreen target, independently of presentation and therefore safe
//! against occlusion, minimization, and foreign workspaces. Both transports
//! share the pacing (dirty tree at `max_fps`, one liveness re-render per
//! second), the cursor compositing, and the damage accumulation machinery.
//!
//! # Boundary
//!
//! The pacing, slot-ring, damage, and transport machines live in
//! `tessera-presentation` (api tier), driven through the narrow
//! [`StreamPainter`] content port. This module keeps the composition-root
//! side: scene observation (output geometry, window trees, the cursor
//! state), security gating against the capture worker, the IPC delivery
//! lane, and the capture-worker fan-out.

use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use super::*;

// Stream value layer re-exports: every bin module reaches these through the
// runtime's glob chain, exactly as when they lived here.
pub(super) use tessera_composite::presentation::{
    CaptureCursorState, DmabufCapture, OffscreenGeometry, OffscreenShmTarget, OffscreenStream,
    OffscreenStreamDrive, OffscreenStreamStage, OffscreenTarget, OutputStreams, PresentedFrameRef,
    StreamCursorBlit, StreamPainter, SurfaceDamageFrame, blit_presented_frame, crop_stream_frame,
    damage_in_target, enumerate_slot_ring, full_target_damage, output_cursor_blit,
    resolve_output_rect,
};

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors the capture/interaction domain-control request pattern.
pub(super) struct StreamControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: StreamControl,
}

pub(super) enum StreamControl {
    Start {
        max_fps: Option<u32>,
        target: tessera_protocol::StreamTarget,
        /// The client's explicit zero-copy opt-in (IPC protocol 25).
        allow_dmabuf: bool,
        /// The negotiated cursor mode (IPC protocol 29), already defaulted
        /// to `Hidden` by the IPC dispatcher.
        cursor: tessera_protocol::StreamCursorMode,
        reply: std::sync::mpsc::Sender<Result<tessera_primitives::stream::StreamInfo, String>>,
    },
    /// Renegotiate a frozen or live stream in place at its target's current
    /// physical geometry (`[INV-ARCH-77]`, ADR-0186).
    Renegotiate {
        stream_id: u64,
        max_fps: Option<u32>,
        allow_dmabuf: bool,
        reply: std::sync::mpsc::Sender<Result<tessera_primitives::stream::StreamInfo, String>>,
    },
    /// Create a dynamic virtual extend output (`VIRTUAL-*`, ADR-0186).
    CreateVirtualOutput {
        spec: tessera_desktop::output::VirtualOutputSpec,
        reply: std::sync::mpsc::Sender<Result<tessera_protocol::OutputInfo, String>>,
    },
    /// Resize, rescale, or reposition a live virtual extend output
    /// (`VIRTUAL-*`, ADR-0186).
    ResizeVirtualOutput {
        connector: String,
        mode: tessera_desktop::output::OutputMode,
        scale: Option<tessera_desktop::output::Scale>,
        logical_origin: Option<tessera_primitives::Point>,
        reply: std::sync::mpsc::Sender<Result<tessera_protocol::OutputInfo, String>>,
    },
    /// Destroy a live virtual extend output (`VIRTUAL-*`, ADR-0186).
    DestroyVirtualOutput {
        connector: String,
        reply: std::sync::mpsc::Sender<Result<String, String>>,
    },
    /// The server already unregistered the delivery lane (`StreamOutputStop`
    /// request, per-frame authorization failure, or server-side end); the
    /// main loop only drops its own state.
    Stop { stream_id: u64 },
    /// The consumer finished reading a delivered dmabuf slot (IPC protocol
    /// 25); the slot may be rendered into again.
    ReleaseSlot { stream_id: u64, slot: u32 },
    /// The connection disconnected; every stream and non-persistent virtual
    /// output it owned is reclaimed.
    Disconnect,
}

/// Content painter backed by the composition root's renderer, server, and
/// cursor cache — the [`StreamPainter`] implementation. Drawing a window's
/// surface tree needs the core-tier renderer and server, which is exactly
/// why the port exists.
struct RuntimePainter<'a> {
    renderer: &'a mut tessera_composite::Renderer,
    server: &'a tessera_wayland::Server,
    cursor_cache: &'a mut cursor::CursorCache,
    /// Shared desktop wallpaper, drawn as the virtual outputs' background so
    /// an extended screen matches a physical one (ADR-0186). Window streams
    /// render only their own tree and never touch it.
    wallpaper: &'a mut Option<wallpaper::Wallpaper>,
}

impl StreamPainter for RuntimePainter<'_> {
    fn paint_offscreen_frame(
        &mut self,
        device: &flux::Device,
        canvas: &flux::CanvasCommands<'_>,
        target: &OffscreenTarget,
        geometry: &OffscreenGeometry,
        cursor: Option<(u32, (f32, f32))>,
    ) {
        match target {
            OffscreenTarget::Window(window) => {
                draw_window_tree(device, self.renderer, self.server, canvas, *window, geometry);
                if let Some((shape, position)) = cursor {
                    let scale = geometry.scale_milli as f32 / 1000.0;
                    draw_software_cursor(canvas, device, self.cursor_cache, position, shape, scale);
                }
            }
            OffscreenTarget::VirtualOutput(connector) => {
                let scale = geometry.scale_milli as f32 / 1000.0;
                // Parity with physical outputs: the extended screen paints the desktop
                // wallpaper behind its windows, not the raw clear colour. The
                // wallpaper is already uploaded at the desktop resolution, so this is
                // a GPU-scaled draw and never re-decodes.
                draw_wallpaper_background(
                    canvas,
                    device,
                    self.wallpaper,
                    (
                        geometry.logical_size.w.max(0) as u32,
                        geometry.logical_size.h.max(0) as u32,
                    ),
                    scale,
                );
                canvas.save();
                if scale != 1.0 {
                    canvas.scale(scale, scale);
                }
                let origin = geometry.origin;
                let map = move |_: Option<tessera_desktop::window::WindowId>,
                                natural: tessera_primitives::Rect| {
                    tessera_primitives::Rect::new(
                        natural.origin.x - origin.x,
                        natural.origin.y - origin.y,
                        natural.size.w,
                        natural.size.h,
                    )
                };
                let shm = self.server.output_capture_frames(connector);
                let dmabuf = self
                    .server
                    .output_capture_dmabuf_frames(connector);
                let surface_order = self.server.output_capture_frame_order(connector);
                self.renderer.draw_surfaces_ordered_mapped(
                    device,
                    canvas,
                    &surface_order,
                    &shm,
                    &dmabuf,
                    &map,
                );
                canvas.restore();
                if let Some((shape, position)) = cursor {
                    draw_software_cursor(canvas, device, self.cursor_cache, position, shape, scale);
                }
            }
        }
    }
}

pub(super) fn virtual_output_geometry(
    server: &tessera_wayland::Server,
    connector: &str,
) -> Result<OffscreenGeometry, String> {
    let info = server
        .output_infos()
        .into_iter()
        .find(|output| output.connector == connector && output.kind.is_virtual())
        .ok_or_else(|| format!("virtual output '{connector}' disconnected"))?;
    let (physical_width, physical_height) = if info.geometry.transform.swap_axes() {
        (info.geometry.mode.height, info.geometry.mode.width)
    } else {
        (info.geometry.mode.width, info.geometry.mode.height)
    };
    if physical_width <= 0 || physical_height <= 0 {
        return Err(format!(
            "virtual output '{connector}' has no measurable extent"
        ));
    }
    let scale_milli = (info.geometry.scale.as_f32() * 1000.0).round().max(1.0) as u32;
    Ok(OffscreenGeometry {
        scale_milli,
        physical_width: physical_width as u32,
        physical_height: physical_height as u32,
        origin: info.geometry.logical_origin,
        logical_size: info.geometry.logical_size(),
    })
}

fn virtual_output_tree_generations(
    server: &tessera_wayland::Server,
    connector: &str,
) -> std::collections::HashMap<usize, u64> {
    server
        .output_capture_frames(connector)
        .iter()
        .map(|frame| (frame.id, frame.generation))
        .chain(
            server
                .output_capture_dmabuf_frames(connector)
                .iter()
                .map(|frame| (frame.id, frame.generation)),
        )
        .collect()
}

/// Content-generation snapshot of one window's surface tree (ADR-0127):
/// every surface id with its current commit generation. The scene frame
/// lists enumerate exactly the window's tree (toplevel, subsurfaces,
/// popups), so a diff against the snapshot from the last render is the
/// per-window dirty signal — the same per-surface generation machinery the
/// output damage tracker diffs globally.
fn window_tree_generations(
    server: &tessera_wayland::Server,
    window: tessera_desktop::window::WindowId,
) -> std::collections::HashMap<usize, u64> {
    server
        .window_capture_frames(window)
        .iter()
        .map(|frame| (frame.id, frame.generation))
        .chain(
            server
                .window_capture_dmabuf_frames(window)
                .iter()
                .map(|frame| (frame.id, frame.generation)),
        )
        .collect()
}

/// The scale the output frame renders at: the primary output's geometry
/// (backend + `[[output]]` overrides), falling back to the host's own scale
/// (nested). Mirrors the presentation path's computation so window crops
/// land on the same physical pixels the readback carries.
pub(super) fn output_render_scale(server: &tessera_wayland::Server, host: &Host) -> f32 {
    server
        .output_infos()
        .first()
        .map(|output| output.geometry.scale.as_f32())
        .filter(|scale| *scale > 0.0)
        .unwrap_or_else(|| host.scale())
}

impl CompositorRuntime<'_> {
    /// Publish the live capture/virtual-display/cast stream counts to the
    /// shell's recording+cast indicators and IPC status subscribers when any
    /// changed (ADR-0128, ADR-0188). Called after every mutation of the stream
    /// set or the virtual-output set — control drains on the iteration path,
    /// geometry/blit endings on the presentation paths — so the indicators
    /// track the frame the change happened on instead of waiting for unrelated
    /// damage.
    pub(super) fn publish_stream_status(&mut self) {
        let capture = self.streams.len() as u32;
        let virtual_displays = self
            .server
            .output_infos()
            .iter()
            .filter(|output| output.kind.is_virtual())
            .count() as u32;
        let cast = self.streams.live_virtual_outputs();
        if self.system_status.capture_streams == capture
            && self.system_status.virtual_displays == virtual_displays
            && self.system_status.cast_streams == cast
        {
            return;
        }
        self.system_status.capture_streams = capture;
        self.system_status.virtual_displays = virtual_displays;
        self.system_status.cast_streams = cast;
        publish_system_status_parts(&self.system_status, &mut self.shell, &self.live, &self.ipc);
        self.damage.chrome_dirty = true;
    }

    /// `[INV-ARCH-81]` (ADR-0189): suspension — a session lock or an inactive
    /// backend — ends every live stream. The capture security generation
    /// already drops frames rendered after the boundary, but a consumer
    /// retains the last *delivered* frame, so a mirror or cast would otherwise
    /// keep showing pre-lock pixels (a physical mirror would even retain the
    /// lock UI) indefinitely. Ending is the transport-uniform, fail-closed
    /// fix: SHM blanking would need a delivery-gate bypass and dmabuf blanking
    /// would need GPU work during the lock.
    pub(super) fn end_streams_for_suspension(&mut self) {
        let ids = self.streams.take_all();
        if ids.is_empty() {
            return;
        }
        log::info!(
            "session suspended: ending {} live stream(s) (ADR-0189)",
            ids.len()
        );
        if let Some(ipc) = self.ipc.as_ref() {
            for stream_id in &ids {
                ipc.end_stream(*stream_id, "session locked or backend inactive");
            }
        }
        for stream_id in &ids {
            self.offscreen_damage.remove(stream_id);
        }
        self.publish_stream_status();
    }

    /// Fan one converted readback out to every due SHM stream. Bounded per
    /// stream: a full delivery lane reports `false` from
    /// `tessera_ipc::Server::push_stream_frame` and the frame counts as dropped
    /// (ADR-0052), so a stalled consumer only ever loses frames. Connector
    /// streams crop the shared frame to their output's current rectangle
    /// (ADR-0126); window streams are no longer here — they render their
    /// own frames offscreen (ADR-0127). A target whose size changed freezes
    /// its stream with `StreamGeometryChanged` instead of delivering a size
    /// the consumer never negotiated; a connector that disappeared ends its
    /// stream. Streams that negotiated the embedded cursor mode are served
    /// the worker's cursor-composited twin of the frame when one was
    /// produced (ADR-0127); everyone else shares the pristine frame.
    pub(super) fn deliver_stream_frame(&mut self, frame: StreamPixels) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let now = Instant::now();
        let mut outputs = None;
        let mut ended: Vec<(u64, String)> = Vec::new();
        let mut frozen: Vec<(u64, u32, u32)> = Vec::new();
        for stream_id in self.streams.due_shm_ids(now) {
            let Some((sequence, dropped)) = self.streams.sequence_and_dropped(stream_id) else {
                continue;
            };
            let Some((target, size)) = self.streams.target_of(stream_id) else {
                continue;
            };
            let cursor_embedded = self.streams.cursor_of(stream_id)
                == Some(tessera_protocol::StreamCursorMode::Embedded);
            let pixels = if cursor_embedded {
                frame.cursor_bgra.as_ref().unwrap_or(&frame.bgra)
            } else {
                &frame.bgra
            };
            let sampled = self
                .streams
                .streams
                .get_mut(&stream_id)
                .and_then(|stream| stream.pending_frame_damage.take());
            let payload_damage = |size: (u32, u32)| match &sampled {
                Some(sampled) => damage_in_target(sampled, size),
                None => full_target_damage(size),
            };
            let cropped = |rect: tessera_primitives::Rect| {
                let (width, height, pixels) = crop_stream_frame(frame.width, pixels, rect);
                let damage = payload_damage((width, height));
                tessera_primitives::stream::StreamFramePayload::Pixels(
                    tessera_primitives::stream::StreamPixelFrame {
                        stream_id,
                        sequence,
                        width,
                        height,
                        stride: width * 4,
                        format: tessera_protocol::StreamPixelFormat::Bgra8,
                        damage,
                        dropped,
                        pixels,
                    },
                )
            };
            let payload = match &target {
                tessera_protocol::StreamTarget::Output { output: None } => {
                    let damage = payload_damage((frame.width, frame.height));
                    tessera_primitives::stream::StreamFramePayload::Pixels(
                        tessera_primitives::stream::StreamPixelFrame {
                            stream_id,
                            sequence,
                            width: frame.width,
                            height: frame.height,
                            stride: frame.width * 4,
                            format: tessera_protocol::StreamPixelFormat::Bgra8,
                            damage,
                            dropped,
                            pixels: std::sync::Arc::clone(pixels),
                        },
                    )
                }
                tessera_protocol::StreamTarget::Output {
                    output: Some(connector),
                } => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    let scale = output_render_scale(self.server, self.host);
                    match resolve_output_rect(outputs, connector, scale, frame.width, frame.height)
                    {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == size => {
                            cropped(rect)
                        }
                        Some(rect) => {
                            frozen.push((stream_id, rect.size.w as u32, rect.size.h as u32));
                            continue;
                        }
                        None => {
                            ended.push((stream_id, format!("output '{connector}' disconnected")));
                            continue;
                        }
                    }
                }
                // Window streams render independently (ADR-0127) and are
                // never due for the shared readback.
                tessera_protocol::StreamTarget::Window { .. } => continue,
            };
            let delivered = ipc.push_stream_frame(payload);
            self.streams.record_frame(stream_id, now, delivered);
            if delivered && let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                stream.note_delivered();
            }
        }
        for (stream_id, width, height) in frozen {
            log::info!("stream {stream_id}: target geometry changed to {width}x{height}; freezing");
            ipc.stream_geometry_changed(stream_id, width, height);
            self.streams.freeze(stream_id);
        }
        for (stream_id, reason) in ended {
            log::info!("stream {stream_id}: {reason}; ending");
            ipc.end_stream(stream_id, &reason);
            self.streams.stop(stream_id);
        }
        self.publish_stream_status();
    }

    /// Clone the damage accumulator of every due SHM stream for the frame
    /// just presented (ADR-0127). Runs once per composite that bound a
    /// stream readback, after the frame's own damage was accumulated, so
    /// the samples describe exactly the pixels this frame carries. The
    /// accumulators keep their contents: they clear on delivery, so a
    /// backpressure drop loses nothing.
    pub(super) fn stash_shm_stream_damage(&mut self) {
        let now = Instant::now();
        let scale = output_render_scale(self.server, self.host);
        let (frame_width, frame_height) = self.surface.size();
        let mut outputs = None;
        for stream_id in self.streams.due_shm_ids(now) {
            let origin = match self.streams.target_of(stream_id) {
                Some((tessera_protocol::StreamTarget::Output { output: None }, _)) => {
                    tessera_primitives::Point { x: 0, y: 0 }
                }
                Some((
                    tessera_protocol::StreamTarget::Output {
                        output: Some(connector),
                    },
                    _,
                )) => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    match resolve_output_rect(outputs, &connector, scale, frame_width, frame_height)
                    {
                        Some(rect) => rect.origin,
                        // Delivery ends or freezes the stream; nothing to sample.
                        None => continue,
                    }
                }
                _ => continue,
            };
            let sampled = self.streams.sample_damage(stream_id, origin);
            if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                stream.pending_frame_damage = Some(sampled);
            }
        }
    }

    /// The cursor state to attach to a shared stream readback binding
    /// (ADR-0127): `Some` only when at least one live SHM output stream
    /// negotiated the embedded cursor mode and a theme cursor is currently
    /// drawable. The state is rasterized when the frame's readback is
    /// requested; the capture worker then produces a cursor-composited twin
    /// of the frame next to the pristine one. On the software-cursor
    /// fallback the presented frame already contains the cursor, so nothing
    /// is attached (`hidden` cannot subtract it there — nested/degraded
    /// only); a client-provided cursor surface is already composited into
    /// the frame as an overlay and is likewise left alone.
    pub(super) fn stream_shm_cursor_state(&self) -> Option<CaptureCursorState> {
        if !self.streams.any_shm_embedded() || self.host.uses_software_cursor() {
            return None;
        }
        let state = self.capture_cursor_state();
        (!state.hidden && !state.client_surface).then_some(state)
    }

    /// Create a dmabuf stream's capture surface and enumerate its slot ring
    /// (IPC protocol 25). The modifier is constrained to the presentation
    /// surface's, so the post-present copy never crosses formats. Any
    /// failure is reported to the caller, which falls back to SHM.
    pub(super) fn create_dmabuf_capture(
        &self,
        width: u32,
        height: u32,
    ) -> Result<DmabufCapture, String> {
        let modifier = self
            .surface
            .dmabuf_modifier()
            .ok_or_else(|| "presentation surface is not dma-buf exportable".to_owned())?;
        enumerate_slot_ring(self.device, modifier, width, height)
    }

    /// Register and start an offscreen stream (window or virtual-output, ADR-0127/0186),
    /// allocating dmabuf slots or an offscreen SHM readback surface.
    pub(super) fn start_offscreen_stream(
        &mut self,
        conn_id: u64,
        max_fps: Option<u32>,
        target: tessera_protocol::StreamTarget,
        cursor: tessera_protocol::StreamCursorMode,
        allow_dmabuf: bool,
        offscreen_target: OffscreenTarget,
        geometry: OffscreenGeometry,
    ) -> Result<tessera_primitives::stream::StreamInfo, String> {
        let size = (geometry.physical_width, geometry.physical_height);
        let offscreen_state = |shm| OffscreenStream {
            target: offscreen_target,
            shm,
            geometry_sig: (
                self.server.all_windows_signature(),
                self.server.outputs_revision(),
            ),
            origin: geometry.origin,
            logical_size: geometry.logical_size,
            scale_milli: geometry.scale_milli,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: true,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        };
        let mut capture = None;
        if allow_dmabuf {
            match self.create_dmabuf_capture(size.0, size.1) {
                Ok(created) => capture = Some(created),
                Err(reason) => log::warn!(
                    "stream: dmabuf capture unavailable ({reason}); falling back to shm"
                ),
            }
        }
        match capture {
            Some(capture) => {
                let info = self.streams.start_dmabuf(
                    conn_id,
                    max_fps,
                    size,
                    target,
                    cursor,
                    capture,
                );
                self.streams.attach_offscreen(
                    info.stream_id,
                    offscreen_state(None),
                );
                Ok(info)
            }
            None => match OffscreenShmTarget::new(
                self.device,
                size.0,
                size.1,
            ) {
                Ok(target_shm) => {
                    let info = self.streams.start(
                        conn_id,
                        max_fps,
                        size,
                        target,
                        cursor,
                    );
                    self.streams.attach_offscreen(
                        info.stream_id,
                        offscreen_state(Some(target_shm)),
                    );
                    Ok(info)
                }
                Err(reason) => Err(reason),
            },
        }
    }

    /// Copy the just-presented frame into every due dmabuf stream's next ring
    /// slot (IPC protocol 25). Runs after a successful composite present; the
    /// frame event is delivered later, once the slot's acquire fence signals
    /// (`poll_dmabuf_stream_fences`). A slot still owned by the consumer
    /// drops the due frame instead of stalling the ring. A frame that was
    /// submitted but could not be exported ends its stream: flux's ring
    /// advanced while the tracking here did not, so continuing could
    /// overwrite a consumer-owned slot. Window streams render their own
    /// tree into their capture surface (ADR-0127) and are not driven here.
    pub(super) fn blit_dmabuf_stream_frames(&mut self, acquire_fence: Option<&OwnedFd>) {
        let Some(presented) = self.host.presented_dmabuf() else {
            return;
        };
        let presented = PresentedFrameRef {
            fd: &presented.fd,
            width: presented.width,
            height: presented.height,
            stride: presented.stride,
            modifier: presented.modifier,
        };
        let now = Instant::now();
        let scale = output_render_scale(self.server, self.host);
        let mut outputs = None;
        let mut ended: Vec<(u64, String)> = Vec::new();
        let mut frozen: Vec<(u64, u32, u32)> = Vec::new();
        // The cursor sprite is composited on the GPU for embedded streams
        // (ADR-0127). On the software-cursor fallback the presented frame
        // already contains the cursor; drawing a second one would double it
        // (and `hidden` cannot subtract it there — nested/degraded only).
        let cursor_state = (!self.host.uses_software_cursor())
            .then(|| self.capture_cursor_state())
            .filter(|state| !state.hidden && !state.client_surface);
        let mut draw_cursor = |canvas: &flux::CanvasCommands<'_>, blit: &StreamCursorBlit| {
            draw_software_cursor(
                canvas,
                self.device,
                &mut self.cursor_cache,
                blit.position,
                blit.shape,
                blit.scale,
            );
        };
        for stream_id in self.streams.due_dmabuf_ids(now) {
            // Connector-addressed streams blit only their output's
            // sub-region of the presented frame (IPC protocol 29); the same
            // geometry-change/gone rules as the SHM path apply.
            let mut cursor_blit = None;
            let src_rect = match self.streams.target_of(stream_id) {
                Some((
                    tessera_protocol::StreamTarget::Output {
                        output: Some(connector),
                    },
                    size,
                )) => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    match resolve_output_rect(
                        outputs,
                        &connector,
                        scale,
                        presented.width,
                        presented.height,
                    ) {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == size => {
                            cursor_blit = cursor_state.as_ref().and_then(|state| {
                                output_cursor_blit(
                                    state,
                                    outputs
                                        .iter()
                                        .find(|output| output.connector == connector)
                                        .map(|output| output.geometry.logical_rect()),
                                    scale,
                                )
                            });
                            Some(rect)
                        }
                        Some(rect) => {
                            frozen.push((stream_id, rect.size.w as u32, rect.size.h as u32));
                            continue;
                        }
                        None => {
                            ended.push((stream_id, format!("output '{connector}' disconnected")));
                            continue;
                        }
                    }
                }
                Some((tessera_protocol::StreamTarget::Output { output: None }, size)) => {
                    cursor_blit = cursor_state.as_ref().and_then(|state| {
                        let logical = tessera_primitives::Rect::new(
                            0,
                            0,
                            (size.0 as f32 / scale).round() as i32,
                            (size.1 as f32 / scale).round() as i32,
                        );
                        output_cursor_blit(state, Some(logical), scale)
                    });
                    None
                }
                _ => None,
            };
            let cursor_blit = cursor_blit.filter(|_| {
                self.streams.cursor_of(stream_id)
                    == Some(tessera_protocol::StreamCursorMode::Embedded)
            });
            let damage_origin = src_rect
                .as_ref()
                .map(|rect| rect.origin)
                .unwrap_or(tessera_primitives::Point { x: 0, y: 0 });
            let sampled = self.streams.sample_damage(stream_id, damage_origin);
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let Some(dmabuf) = stream.dmabuf.as_mut() else {
                continue;
            };
            let Some(_slot) = dmabuf.ring.next_submission_slot() else {
                // The consumer still owns the ring's next slot.
                stream.dropped += 1;
                if dmabuf.ring.next_is_pinned() && !dmabuf.ring_stalled {
                    dmabuf.ring_stalled = true;
                    log::warn!(
                        "stream {stream_id}: every capture slot is consumer-owned; \
                         dropping frames until the consumer releases one"
                    );
                }
                continue;
            };
            let sequence = stream.sequence + 1;
            let dropped = stream.dropped;
            // Snapshot the security generation with the blit: delivery below
            // re-checks it, mirroring the SHM worker's completion check.
            let security_generation = self.capture_worker.security_generation();
            match blit_presented_frame(
                self.device,
                dmabuf,
                &presented,
                acquire_fence,
                src_rect.as_ref(),
                cursor_blit,
                &mut draw_cursor,
            ) {
                Ok((slot, fence, source)) => {
                    if dmabuf.ring_stalled {
                        dmabuf.ring_stalled = false;
                        log::info!("stream {stream_id}: capture slots are flowing again");
                    }
                    dmabuf.ring.submitted(slot);
                    dmabuf
                        .pending
                        .push(tessera_composite::presentation::PendingSlotFrame {
                            slot,
                            fence,
                            source: Some(source),
                            sequence,
                            dropped,
                            submitted_at: now,
                            security_generation,
                            damage: sampled,
                        });
                    stream.sequence += 1;
                    stream.last_frame = Some(now);
                }
                Err(tessera_composite::presentation::BlitFailure::Retryable(reason)) => {
                    log::warn!("stream {stream_id}: dmabuf capture frame failed: {reason}");
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                }
                Err(tessera_composite::presentation::BlitFailure::Submitted(reason)) => {
                    ended.push((stream_id, reason))
                }
            }
        }
        if let Some(ipc) = self.ipc.as_ref() {
            for (stream_id, width, height) in frozen {
                log::info!(
                    "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
                );
                ipc.stream_geometry_changed(stream_id, width, height);
                self.streams.freeze(stream_id);
            }
            for (stream_id, reason) in ended {
                log::warn!("stream {stream_id}: {reason}; ending");
                ipc.end_stream(stream_id, &reason);
                self.streams.stop(stream_id);
            }
        }
        self.publish_stream_status();
    }

    /// Deliver dmabuf stream frames whose acquire fence signaled (IPC
    /// protocol 25). The fence poll is a registry-owned machine step; the
    /// composition root supplies the security gate and the delivery lane.
    pub(super) fn poll_dmabuf_stream_fences(&mut self) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let now = Instant::now();
        let worker = &self.capture_worker;
        let streams = &mut self.streams;
        streams.poll_dmabuf_fences(
            now,
            |generation| worker.permits(generation),
            |frame| ipc.push_stream_frame(frame),
        );
    }

    /// Reconcile every live stream with a new presentation-surface geometry
    /// (IPC protocol 29, ADR-0126). Called for ANY surface-size change —
    /// hotplug or config mode change through `take_resize`, and the flux
    /// `begin_frame` failure rebuild — so streams can no longer keep
    /// delivering at a stale geometry. The registry decides; this applies
    /// the decisions to the IPC lane and the shell indicator.
    pub(super) fn handle_output_geometry_change(&mut self) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let frame_size = self.surface.size();
        let scale = output_render_scale(self.server, self.host);
        let outputs = self.server.output_infos();
        let actions = self.streams.reconcile_geometry(frame_size, scale, &outputs);
        if actions.is_empty() {
            return;
        }
        log::info!(
            "stream: output geometry changed; reconciling {} stream(s)",
            actions.len()
        );
        for (stream_id, action) in actions {
            match action {
                tessera_composite::presentation::GeometryAction::Freeze(width, height) => {
                    log::info!(
                        "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
                    );
                    ipc.stream_geometry_changed(stream_id, width, height);
                    self.streams.freeze(stream_id);
                }
                tessera_composite::presentation::GeometryAction::End(reason) => {
                    log::info!("stream {stream_id}: {reason}; ending");
                    ipc.end_stream(stream_id, &reason);
                    self.streams.stop(stream_id);
                }
                tessera_composite::presentation::GeometryAction::Unchanged => {}
            }
        }
        self.publish_stream_status();
    }

    /// Drive every live offscreen stream one step (ADR-0127, ADR-0186). The drive itself
    /// is a registry-owned machine step; this observes the scene
    /// (signatures, target geometry, tree generations, cursor state) and
    /// applies the returned outcomes to the IPC lane and Wayland frame callbacks.
    pub(super) fn drive_offscreen_streams(&mut self) {
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        let sig = (
            self.server.all_windows_signature(),
            self.server.outputs_revision(),
        );
        if !self
            .streams
            .streams
            .values()
            .any(|stream| !stream.frozen && stream.offscreen.is_some())
        {
            return;
        }
        let now = Instant::now();
        let scheme = self.shell.design().scheme;
        let security_generation = self.capture_worker.security_generation();
        // The theme cursor, when one is currently drawable.
        let cursor_state = {
            let state = self.capture_cursor_state();
            (!state.hidden && !state.client_surface).then_some(state)
        };
        let device = &self.device;
        let server = &self.server;
        let renderer = &mut self.renderer;
        let cursor_cache = &mut self.cursor_cache;
        let wallpaper = &mut self.wallpaper;
        let mut painter = RuntimePainter {
            renderer,
            server,
            cursor_cache,
            wallpaper,
        };
        let mut resolve_geometry = |target: &OffscreenTarget| match target {
            OffscreenTarget::Window(window) => window_tree_geometry(server, *window),
            OffscreenTarget::VirtualOutput(connector) => {
                virtual_output_geometry(server, connector)
            }
        };
        let mut tree_generations = |target: &OffscreenTarget| match target {
            OffscreenTarget::Window(window) => window_tree_generations(server, *window),
            OffscreenTarget::VirtualOutput(connector) => {
                virtual_output_tree_generations(server, connector)
            }
        };
        let damage_trackers = &mut self.offscreen_damage;
        let mut tree_damage = |stream_id: u64, target: &OffscreenTarget| match target {
            OffscreenTarget::Window(window) => {
                let shm = server.window_capture_frames(*window);
                let dmabuf = server.window_capture_dmabuf_frames(*window);
                damage_trackers.entry(stream_id).or_default().assess(
                    shm.iter()
                        .map(|frame| frame as &dyn SurfaceDamageFrame)
                        .chain(dmabuf.iter().map(|frame| frame as &dyn SurfaceDamageFrame)),
                )
            }
            OffscreenTarget::VirtualOutput(connector) => {
                let shm = server.output_capture_frames(connector);
                let dmabuf = server.output_capture_dmabuf_frames(connector);
                damage_trackers.entry(stream_id).or_default().assess(
                    shm.iter()
                        .map(|frame| frame as &dyn SurfaceDamageFrame)
                        .chain(dmabuf.iter().map(|frame| frame as &dyn SurfaceDamageFrame)),
                )
            }
        };
        let effects = self.streams.drive_offscreen_streams(OffscreenStreamDrive {
            now,
            device,
            signature: sig,
            resolve_geometry: &mut resolve_geometry,
            tree_generations: &mut tree_generations,
            tree_damage: &mut tree_damage,
            cursor_state,
            clear: interaction_domain_clear(scheme),
            security_generation,
            painter: &mut painter,
        });
        let now_ms = self.start.elapsed().as_millis() as u32;
        for connector in &effects.rendered_virtual_outputs {
            self.server.send_output_frame_callbacks(connector, now_ms);
        }
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        for (stream_id, width, height) in &effects.frozen {
            log::info!("stream {stream_id}: target geometry changed to {width}x{height}; freezing");
            ipc.stream_geometry_changed(*stream_id, *width, *height);
            self.streams.freeze(*stream_id);
            // A frozen stream's next frame is a fresh, fully-damaged target.
            self.offscreen_damage.remove(stream_id);
        }
        for (stream_id, reason) in &effects.ended {
            log::info!("stream {stream_id}: {reason}; ending");
            ipc.end_stream(*stream_id, reason);
            self.streams.stop(*stream_id);
            self.offscreen_damage.remove(stream_id);
        }
        self.publish_stream_status();
    }

    /// Hand every completed offscreen-stream SHM readback (window or
    /// virtual-output) to the capture worker for BGRA conversion
    /// (ADR-0127/0186). The worker lane is shared with one-shot captures,
    /// which keep priority: while a one-shot reserves the lane a ready frame
    /// waits on its surface (flux keeps the completed frame mapped), and a
    /// frame held that way for too long logs once instead of starving
    /// silently.
    pub(super) fn poll_offscreen_stream_readbacks(&mut self) {
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        let now = Instant::now();
        let ids: Vec<u64> = self
            .streams
            .streams
            .iter()
            .filter(|(_, stream)| {
                stream.offscreen.as_ref().is_some_and(|offscreen| {
                    matches!(offscreen.stage, OffscreenStreamStage::AwaitingReadback { .. })
                })
            })
            .map(|(stream_id, _)| *stream_id)
            .collect();
        for stream_id in ids {
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let (width, height) = stream.size;
            let Some((shm, stage, held_since_slot)) = stream.offscreen_shm_parts() else {
                continue;
            };
            let Some(target) = shm.as_ref() else {
                continue;
            };
            let ready = match target.surface.read_pixels_ready() {
                Ok(ready) => ready,
                Err(error) => {
                    log::warn!(
                        "stream {stream_id}: offscreen readback readiness failed: {error}{}",
                        flux_last_error_detail()
                    );
                    *stage = OffscreenStreamStage::Idle;
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                    continue;
                }
            };
            if !ready {
                continue;
            }
            if self.capture_worker.is_busy() {
                let held_since = held_since_slot.get_or_insert(now);
                if now.duration_since(*held_since) >= Duration::from_secs(2) {
                    log::warn!(
                        "stream {stream_id}: offscreen frame held behind one-shot captures \
                         for {:?}; the capture worker keeps one-shot priority",
                        now.duration_since(*held_since)
                    );
                    *held_since = now + Duration::from_secs(58);
                }
                continue;
            }
            let OffscreenStreamStage::AwaitingReadback {
                security_generation,
                damage,
            } = std::mem::replace(stage, OffscreenStreamStage::Idle)
            else {
                continue;
            };
            if !self.capture_worker.permits(security_generation) {
                // The frame was rendered before a lock/VT boundary: drop it
                // without spending a worker conversion (ADR-0127).
                stream.dropped += 1;
                stream.fold_damage_back(damage);
                continue;
            }
            let readback = PendingReadback {
                width,
                height,
                crop: None,
                cursor: None,
                security_generation,
            };
            let captured = read_captured_pixels_owned(&target.surface, readback);
            let Some((_shm, stage, held_since_slot)) = stream.offscreen_shm_parts() else {
                continue;
            };
            match captured {
                Ok(capture) => {
                    *held_since_slot = None;
                    *stage = OffscreenStreamStage::Converting { damage };
                    queue_captured_pixels(
                        &self.capture_worker,
                        capture,
                        CaptureTarget::StreamWindow { stream_id },
                        &self.journal,
                        &self.ipc,
                    );
                }
                Err(reason) => {
                    log::warn!("stream {stream_id}: offscreen readback failed: {reason}");
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                    stream.fold_damage_back(damage);
                }
            }
        }
    }

    /// Deliver one converted offscreen-stream frame arriving from the capture
    /// worker (ADR-0127/0186). Frames that crossed a lock/VT boundary or
    /// failed conversion count as dropped and fold their damage back; a
    /// stopped stream's late completion is discarded.
    pub(super) fn deliver_offscreen_stream_frame(
        &mut self,
        stream_id: u64,
        security_generation: u64,
        pixels: Result<StreamPixels, String>,
    ) {
        let now = Instant::now();
        let damage = match self
            .streams
            .streams
            .get_mut(&stream_id)
            .and_then(|stream| stream.offscreen_shm_parts())
        {
            Some((_shm, stage, _held_since)) => {
                match std::mem::replace(stage, OffscreenStreamStage::Idle) {
                    OffscreenStreamStage::Converting { damage } => Some(damage),
                    // The stream was re-rendered or stopped meanwhile; this
                    // completion is stale.
                    prev_stage => {
                        *stage = prev_stage;
                        None
                    }
                }
            }
            None => None,
        };
        let Some(damage) = damage else {
            return;
        };
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        if !self.capture_worker.permits(security_generation) {
            if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                stream.dropped += 1;
                stream.fold_damage_back(damage);
            }
            return;
        }
        match pixels {
            Ok(frame) => {
                let Some((sequence, dropped)) = self.streams.sequence_and_dropped(stream_id) else {
                    return;
                };
                let Some((_, size)) = self.streams.target_of(stream_id) else {
                    return;
                };
                let payload = tessera_primitives::stream::StreamFramePayload::Pixels(
                    tessera_primitives::stream::StreamPixelFrame {
                        stream_id,
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        stride: frame.width * 4,
                        format: tessera_protocol::StreamPixelFormat::Bgra8,
                        damage: damage_in_target(&damage, size),
                        dropped,
                        pixels: frame.bgra,
                    },
                );
                let delivered = ipc.push_stream_frame(payload);
                self.streams.record_frame(stream_id, now, delivered);
                if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                    if delivered {
                        stream.note_delivered();
                    } else {
                        stream.fold_damage_back(damage);
                    }
                }
            }
            Err(reason) => {
                log::warn!("stream {stream_id}: window frame conversion failed: {reason}");
                if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                    stream.dropped += 1;
                    stream.fold_damage_back(damage);
                }
            }
        }
    }
}

#[cfg(test)]
mod dmabuf_tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use tessera_composite::presentation::{
        DmabufStream, STREAM_SLOT_COUNT, SlotRing, render_offscreen_stream_dmabuf,
        render_offscreen_stream_shm,
    };

    /// End-to-end window-stream render exercise (ADR-0127): the SHM target
    /// renders and reads back a uniform frame, and the dmabuf path shares
    /// the blit's submit/export tail (slot 0 with a signaling fence). The
    /// scene carries no window, so the tree draw is the clear alone — this
    /// exercises the pipeline (surface, canvas, pass, submit, readback /
    /// export), not window content. Skipped without a Vulkan device or a
    /// Wayland runtime dir on this machine.
    #[test]
    fn window_stream_render_pipeline_smoke() {
        let Ok(device) = flux::Device::new_with_options(flux::DeviceOptions {
            headless: true,
            frames_in_flight: STREAM_SLOT_COUNT as u32,
            required_features: flux::DeviceFeatures::DMABUF,
            optional_features: flux::DeviceFeatures::DMABUF_SYNC_FILE,
            ..flux::DeviceOptions::default()
        }) else {
            return;
        };
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            return;
        }
        let Ok(server) = tessera_wayland::Server::new() else {
            return;
        };
        let mut renderer = tessera_composite::Renderer::new();
        let scheme = tessera_desktop::settings::ColorScheme::Dark;
        let geometry = OffscreenGeometry {
            scale_milli: 1000,
            physical_width: 64,
            physical_height: 48,
            origin: tessera_primitives::Point { x: 0, y: 0 },
            logical_size: tessera_primitives::Size { w: 64, h: 48 },
        };
        struct EmptyPainter<'a> {
            renderer: &'a mut tessera_composite::Renderer,
            server: &'a tessera_wayland::Server,
        }
        impl StreamPainter for EmptyPainter<'_> {
            fn paint_offscreen_frame(
                &mut self,
                device: &flux::Device,
                canvas: &flux::CanvasCommands<'_>,
                target: &OffscreenTarget,
                geometry: &OffscreenGeometry,
                _cursor: Option<(u32, (f32, f32))>,
            ) {
                if let OffscreenTarget::Window(window) = target {
                    draw_window_tree(device, self.renderer, self.server, canvas, *window, geometry);
                }
            }
        }
        let mut painter = EmptyPainter {
            renderer: &mut renderer,
            server: &server,
        };

        // SHM: render into the cached readback target and read it back.
        let mut target = OffscreenShmTarget::new(&device, 64, 48).expect("shm target");
        let offscreen_target = OffscreenTarget::Window(tessera_desktop::window::WindowId(1));
        render_offscreen_stream_shm(
            &device,
            &mut target,
            interaction_domain_clear(scheme),
            &offscreen_target,
            &geometry,
            None,
            &mut painter,
        )
        .expect("window shm render");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if target.surface.read_pixels_ready().unwrap_or(false) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "window readback never completed"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let capture = read_captured_pixels_owned(
            &target.surface,
            PendingReadback {
                width: 64,
                height: 48,
                crop: None,
                cursor: None,
                security_generation: 1,
            },
        )
        .expect("read window pixels");
        let pixels = stream_pixels(capture).expect("convert window pixels");
        assert_eq!((pixels.width, pixels.height), (64, 48));
        assert_eq!(pixels.bgra.len(), 64 * 48 * 4);
        // The empty tree renders the opaque clear: one flat color modulo
        // the renderer's ordered dither (±1–2 per channel), alpha always 255.
        let first = &pixels.bgra[0..4];
        assert!(
            pixels.bgra.chunks_exact(4).all(|px| {
                px[3] == 255 && (0..3).all(|channel| px[channel].abs_diff(first[channel]) <= 2)
            }),
            "empty window tree should render a flat opaque clear, first pixel {first:?}"
        );

        // dmabuf: the window render shares the blit's submit/export tail.
        const LINEAR: u64 = 0; // DRM_FORMAT_MOD_LINEAR
        let Ok(surface) = flux::Surface::offscreen_dmabuf(&device, 64, 48, &[LINEAR]) else {
            return;
        };
        let canvas = flux::Canvas::new(&surface).unwrap();
        let mut dmabuf = DmabufStream {
            surface,
            canvas,
            modifier: LINEAR,
            slot_stride: 0,
            slot_bytes: 0,
            ring: SlotRing::new(STREAM_SLOT_COUNT),
            pending: Vec::new(),
            ring_stalled: false,
        };
        let (slot, fence) = render_offscreen_stream_dmabuf(
            &device,
            &mut dmabuf,
            interaction_domain_clear(scheme),
            &offscreen_target,
            &geometry,
            None,
            &mut painter,
        )
        .expect("window dmabuf render");
        assert_eq!(slot, 0, "the first window frame lands in slot 0");
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pollfd` references a live descriptor for the call.
        let result = unsafe { libc::poll(&mut pollfd, 1, 2000) };
        assert!(result > 0, "window capture slot fence did not signal");
    }

    #[test]
    fn virtual_output_stream_render_pipeline_smoke() {
        use tessera_composite::presentation::{
            render_offscreen_stream_dmabuf, render_offscreen_stream_shm,
        };

        let Ok(device) = flux::Device::new_with_options(flux::DeviceOptions {
            headless: true,
            frames_in_flight: STREAM_SLOT_COUNT as u32,
            required_features: flux::DeviceFeatures::DMABUF,
            optional_features: flux::DeviceFeatures::DMABUF_SYNC_FILE,
            ..flux::DeviceOptions::default()
        }) else {
            return;
        };
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            return;
        };
        let Ok(mut server) = tessera_wayland::Server::new() else {
            return;
        };
        server
            .create_virtual_output(
                Some(1),
                tessera_desktop::output::VirtualOutputSpec {
                    connector: Some("VIRTUAL-1".into()),
                    mode: tessera_desktop::output::OutputMode {
                        width: 64,
                        height: 64,
                        refresh_mhz: 60_000,
                    },
                    scale: tessera_desktop::output::Scale::IDENTITY,
                    logical_origin: None,
                    persistent: false,
                },
            )
            .expect("create VIRTUAL-1");
        let geometry = virtual_output_geometry(&server, "VIRTUAL-1").expect("virtual geometry");
        let mut renderer = tessera_composite::Renderer::new();
        let mut cursor_cache = cursor::CursorCache::default();
        let mut wallpaper: Option<wallpaper::Wallpaper> = None;
        let scheme = tessera_desktop::settings::ColorScheme::Dark;
        let mut painter = RuntimePainter {
            renderer: &mut renderer,
            server: &server,
            cursor_cache: &mut cursor_cache,
            wallpaper: &mut wallpaper,
        };

        let mut target = OffscreenShmTarget::new(&device, 64, 64).expect("shm target");
        let offscreen_target = OffscreenTarget::VirtualOutput("VIRTUAL-1".into());
        render_offscreen_stream_shm(
            &device,
            &mut target,
            interaction_domain_clear(scheme),
            &offscreen_target,
            &geometry,
            None,
            &mut painter,
        )
        .expect("virtual output shm render");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if target.surface.read_pixels_ready().unwrap_or(false) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "virtual output readback never completed"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        const LINEAR: u64 = 0;
        let Ok(surface) = flux::Surface::offscreen_dmabuf(&device, 64, 64, &[LINEAR]) else {
            return;
        };
        let canvas = flux::Canvas::new(&surface).unwrap();
        let mut dmabuf = DmabufStream {
            surface,
            canvas,
            modifier: LINEAR,
            slot_stride: 0,
            slot_bytes: 0,
            ring: SlotRing::new(STREAM_SLOT_COUNT),
            pending: Vec::new(),
            ring_stalled: false,
        };
        let (slot, fence) = render_offscreen_stream_dmabuf(
            &device,
            &mut dmabuf,
            interaction_domain_clear(scheme),
            &offscreen_target,
            &geometry,
            None,
            &mut painter,
        )
        .expect("virtual output dmabuf render");
        assert_eq!(slot, 0);
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut pollfd, 1, 2000) };
        assert!(
            result > 0,
            "virtual output capture slot fence did not signal"
        );
    }
}
