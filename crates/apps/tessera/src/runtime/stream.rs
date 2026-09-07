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
pub(super) use tessera_presentation::{
    blit_presented_frame, crop_stream_frame, damage_in_target, enumerate_slot_ring,
    full_target_damage, output_cursor_blit, resolve_output_rect, CaptureCursorState,
    DmabufCapture, OutputStreams, PresentedFrameRef, StreamCursorBlit, StreamPainter,
    WindowShmTarget, WindowStream, WindowStreamDrive, WindowStreamStage, WindowTreeGeometry,
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
        target: tessera_ipc::StreamTarget,
        /// The client's explicit zero-copy opt-in (IPC protocol 25).
        allow_dmabuf: bool,
        /// The negotiated cursor mode (IPC protocol 29), already defaulted
        /// to `Hidden` by the IPC dispatcher.
        cursor: tessera_ipc::StreamCursorMode,
        reply: std::sync::mpsc::Sender<Result<tessera_ipc::StreamInfo, String>>,
    },
    /// The server already unregistered the delivery lane (`StreamOutputStop`
    /// request, per-frame authorization failure, or server-side end); the
    /// main loop only drops its own state.
    Stop { stream_id: u64 },
    /// The consumer finished reading a delivered dmabuf slot (IPC protocol
    /// 25); the slot may be rendered into again.
    ReleaseSlot { stream_id: u64, slot: u32 },
    /// The connection disconnected; every stream it owned was unregistered
    /// server-side.
    Disconnect,
}

/// Content painter backed by the composition root's renderer, server, and
/// cursor cache — the [`StreamPainter`] implementation. Drawing a window's
/// surface tree needs the core-tier renderer and server, which is exactly
/// why the port exists.
struct RuntimePainter<'a> {
    renderer: &'a mut tessera_render::Renderer,
    server: &'a tessera_compositor::Server,
    cursor_cache: &'a mut cursor::CursorCache,
}

impl StreamPainter for RuntimePainter<'_> {
    fn paint_window_frame(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        geometry: &WindowTreeGeometry,
        cursor: Option<(u32, (f32, f32))>,
    ) {
        draw_window_tree(device, self.renderer, self.server, canvas, geometry);
        if let Some((shape, position)) = cursor {
            let scale = geometry.scale_milli as f32 / 1000.0;
            draw_software_cursor(canvas, device, self.cursor_cache, position, shape, scale);
        }
    }
}

/// Content-generation snapshot of one window's surface tree (ADR-0127):
/// every surface id with its current commit generation. The scene frame
/// lists enumerate exactly the window's tree (toplevel, subsurfaces,
/// popups), so a diff against the snapshot from the last render is the
/// per-window dirty signal — the same per-surface generation machinery the
/// output damage tracker diffs globally.
fn window_tree_generations(
    server: &tessera_compositor::Server,
    window: tessera_model::window::WindowId,
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
pub(super) fn output_render_scale(server: &tessera_compositor::Server, host: &Host) -> f32 {
    server
        .output_infos()
        .first()
        .map(|output| output.geometry.scale.as_f32())
        .filter(|scale| *scale > 0.0)
        .unwrap_or_else(|| host.scale())
}

impl CompositorRuntime {
    /// Publish the live capture-stream count to the shell's recording
    /// indicator and IPC status subscribers when it changed (ADR-0128).
    /// Called after every mutation of the stream set — control drains on
    /// the iteration path, geometry/blit endings on the presentation
    /// paths — so the indicator tracks the frame the change happened on
    /// instead of waiting for unrelated damage.
    pub(super) fn publish_capture_stream_count(&mut self) {
        let count = self.streams.len() as u32;
        if self.system_status.capture_streams == count {
            return;
        }
        self.system_status.capture_streams = count;
        publish_system_status_parts(&self.system_status, &mut self.shell, &self.live, &self.ipc);
        self.damage.chrome_dirty = true;
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
            let cursor_embedded =
                self.streams.cursor_of(stream_id) == Some(tessera_ipc::StreamCursorMode::Embedded);
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
            let cropped = |rect: tessera_model::Rect| {
                let (width, height, pixels) = crop_stream_frame(frame.width, pixels, rect);
                let damage = payload_damage((width, height));
                tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
                    stream_id,
                    sequence,
                    width,
                    height,
                    stride: width * 4,
                    format: tessera_ipc::StreamPixelFormat::Bgra8,
                    damage,
                    dropped,
                    pixels,
                })
            };
            let payload = match &target {
                tessera_ipc::StreamTarget::Output { output: None } => {
                    let damage = payload_damage((frame.width, frame.height));
                    tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
                        stream_id,
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        stride: frame.width * 4,
                        format: tessera_ipc::StreamPixelFormat::Bgra8,
                        damage,
                        dropped,
                        pixels: std::sync::Arc::clone(pixels),
                    })
                }
                tessera_ipc::StreamTarget::Output {
                    output: Some(connector),
                } => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    let scale = output_render_scale(&self.server, &self.host);
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
                tessera_ipc::StreamTarget::Window { .. } => continue,
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
        self.publish_capture_stream_count();
    }

    /// Clone the damage accumulator of every due SHM stream for the frame
    /// just presented (ADR-0127). Runs once per composite that bound a
    /// stream readback, after the frame's own damage was accumulated, so
    /// the samples describe exactly the pixels this frame carries. The
    /// accumulators keep their contents: they clear on delivery, so a
    /// backpressure drop loses nothing.
    pub(super) fn stash_shm_stream_damage(&mut self) {
        let now = Instant::now();
        let scale = output_render_scale(&self.server, &self.host);
        let (frame_width, frame_height) = self.surface.size();
        let mut outputs = None;
        for stream_id in self.streams.due_shm_ids(now) {
            let origin = match self.streams.target_of(stream_id) {
                Some((tessera_ipc::StreamTarget::Output { output: None }, _)) => {
                    tessera_model::Point { x: 0, y: 0 }
                }
                Some((
                    tessera_ipc::StreamTarget::Output {
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
        enumerate_slot_ring(&self.device, modifier, width, height)
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
        let scale = output_render_scale(&self.server, &self.host);
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
        let mut draw_cursor = |canvas: &flux::Canvas, blit: &StreamCursorBlit| {
            draw_software_cursor(
                canvas,
                &self.device,
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
                    tessera_ipc::StreamTarget::Output {
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
                Some((tessera_ipc::StreamTarget::Output { output: None }, size)) => {
                    cursor_blit = cursor_state.as_ref().and_then(|state| {
                        let logical = tessera_model::Rect::new(
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
                self.streams.cursor_of(stream_id) == Some(tessera_ipc::StreamCursorMode::Embedded)
            });
            let damage_origin = src_rect
                .as_ref()
                .map(|rect| rect.origin)
                .unwrap_or(tessera_model::Point { x: 0, y: 0 });
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
                &self.device,
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
                    dmabuf.pending.push(tessera_presentation::PendingSlotFrame {
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
                Err(tessera_presentation::BlitFailure::Retryable(reason)) => {
                    log::warn!("stream {stream_id}: dmabuf capture frame failed: {reason}");
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                }
                Err(tessera_presentation::BlitFailure::Submitted(reason)) => {
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
        self.publish_capture_stream_count();
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
        streams.poll_dmabuf_fences(now, |generation| worker.permits(generation), ipc);
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
        let scale = output_render_scale(&self.server, &self.host);
        let outputs = self.server.output_infos();
        let actions = self
            .streams
            .reconcile_geometry(frame_size, scale, &outputs);
        if actions.is_empty() {
            return;
        }
        log::info!(
            "stream: output geometry changed; reconciling {} stream(s)",
            actions.len()
        );
        for (stream_id, action) in actions {
            match action {
                tessera_presentation::GeometryAction::Freeze(width, height) => {
                    log::info!(
                        "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
                    );
                    ipc.stream_geometry_changed(stream_id, width, height);
                    self.streams.freeze(stream_id);
                }
                tessera_presentation::GeometryAction::End(reason) => {
                    log::info!("stream {stream_id}: {reason}; ending");
                    ipc.end_stream(stream_id, &reason);
                    self.streams.stop(stream_id);
                }
                tessera_presentation::GeometryAction::Unchanged => {}
            }
        }
        self.publish_capture_stream_count();
    }

    /// Drive every live window stream one step (ADR-0127). The drive itself
    /// is a registry-owned machine step; this observes the scene
    /// (signatures, window geometry, tree generations, cursor state) and
    /// applies the returned outcomes to the IPC lane.
    pub(super) fn drive_window_streams(&mut self) {
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
            .any(|stream| !stream.frozen && stream.window.is_some())
        {
            return;
        }
        let now = Instant::now();
        let scheme = self.shell.design().scheme;
        let security_generation = self.capture_worker.security_generation();
        // The theme cursor, when one is currently drawable. A client-owned
        // cursor surface is not part of any window's surface tree, so
        // window streams can never show it; nothing is drawn then.
        let cursor_state = {
            let state = self.capture_cursor_state();
            (!state.hidden && !state.client_surface).then_some(state)
        };
        let device = &self.device;
        let server = &self.server;
        let renderer = &mut self.renderer;
        let cursor_cache = &mut self.cursor_cache;
        let mut painter = RuntimePainter {
            renderer,
            server,
            cursor_cache,
        };
        let mut resolve_geometry =
            |window| window_tree_geometry(server, window);
        let mut tree_generations = |window| window_tree_generations(server, window);
        let effects = self.streams.drive_window_streams(WindowStreamDrive {
            now,
            device,
            signature: sig,
            resolve_geometry: &mut resolve_geometry,
            tree_generations: &mut tree_generations,
            cursor_state,
            clear: interaction_domain_clear(scheme),
            security_generation,
            painter: &mut painter,
        });
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        for (stream_id, width, height) in &effects.frozen {
            log::info!(
                "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
            );
            ipc.stream_geometry_changed(*stream_id, *width, *height);
            self.streams.freeze(*stream_id);
        }
        for (stream_id, reason) in &effects.ended {
            log::info!("stream {stream_id}: {reason}; ending");
            ipc.end_stream(*stream_id, reason);
            self.streams.stop(*stream_id);
        }
        self.publish_capture_stream_count();
    }

    /// Hand every completed window-stream SHM readback to the capture
    /// worker for BGRA conversion (ADR-0127). The worker lane is shared
    /// with one-shot captures, which keep priority: while a one-shot
    /// reserves the lane a ready frame waits on its surface (flux keeps
    /// the completed frame mapped), and a frame held that way for too long
    /// logs once instead of starving silently.
    pub(super) fn poll_window_stream_readbacks(&mut self) {
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        let now = Instant::now();
        let ids: Vec<u64> = self
            .streams
            .streams
            .iter()
            .filter(|(_, stream)| {
                stream.window.as_ref().is_some_and(|window| {
                    matches!(window.stage, WindowStreamStage::AwaitingReadback { .. })
                })
            })
            .map(|(stream_id, _)| *stream_id)
            .collect();
        for stream_id in ids {
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let window = stream
                .window
                .as_mut()
                .expect("window stream state attached");
            let Some(target) = window.shm.as_ref() else {
                continue;
            };
            let ready = match target.surface.read_pixels_ready() {
                Ok(ready) => ready,
                Err(error) => {
                    log::warn!(
                        "stream {stream_id}: window readback readiness failed: {error}{}",
                        flux_last_error_detail()
                    );
                    window.stage = WindowStreamStage::Idle;
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                    continue;
                }
            };
            if !ready {
                continue;
            }
            if self.capture_worker.is_busy() {
                let held_since = window.held_since.get_or_insert(now);
                if now.duration_since(*held_since) >= Duration::from_secs(2) {
                    log::warn!(
                        "stream {stream_id}: window frame held behind one-shot captures \
                         for {:?}; the capture worker keeps one-shot priority",
                        now.duration_since(*held_since)
                    );
                    *held_since = now + Duration::from_secs(58);
                }
                continue;
            }
            let WindowStreamStage::AwaitingReadback {
                security_generation,
                damage,
            } = std::mem::replace(&mut window.stage, WindowStreamStage::Idle)
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
            let (width, height) = stream.size;
            let readback = PendingReadback {
                width,
                height,
                crop: None,
                cursor: None,
                security_generation,
            };
            match read_captured_pixels_owned(&target.surface, readback) {
                Ok(capture) => {
                    window.held_since = None;
                    window.stage = WindowStreamStage::Converting { damage };
                    queue_captured_pixels(
                        &self.capture_worker,
                        capture,
                        CaptureTarget::StreamWindow { stream_id },
                        &self.journal,
                        &self.ipc,
                    );
                }
                Err(reason) => {
                    log::warn!("stream {stream_id}: window readback failed: {reason}");
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                    stream.fold_damage_back(damage);
                }
            }
        }
    }

    /// Deliver one converted window-stream frame arriving from the capture
    /// worker (ADR-0127). Frames that crossed a lock/VT boundary or failed
    /// conversion count as dropped and fold their damage back; a stopped
    /// stream's late completion is discarded.
    pub(super) fn deliver_window_stream_frame(
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
            .and_then(|stream| stream.window.as_mut())
        {
            Some(window) => {
                match std::mem::replace(&mut window.stage, WindowStreamStage::Idle) {
                    WindowStreamStage::Converting { damage } => Some(damage),
                    // The stream was re-rendered or stopped meanwhile; this
                    // completion is stale.
                    stage => {
                        window.stage = stage;
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
                let payload = tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
                    stream_id,
                    sequence,
                    width: frame.width,
                    height: frame.height,
                    stride: frame.width * 4,
                    format: tessera_ipc::StreamPixelFormat::Bgra8,
                    damage: damage_in_target(&damage, size),
                    dropped,
                    pixels: frame.bgra,
                });
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
    use tessera_presentation::{
        render_window_stream_dmabuf, render_window_stream_shm, DmabufStream, LIVENESS_INTERVAL,
        SlotRing, STREAM_SLOT_COUNT,
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
        let Ok(server) = tessera_compositor::Server::new() else {
            return;
        };
        let mut renderer = tessera_render::Renderer::new();
        let mut cursor_cache = cursor::CursorCache::default();
        let scheme = tessera_model::settings::ColorScheme::Dark;
        let geometry = WindowTreeGeometry {
            window: tessera_model::window::WindowId(1),
            scale_milli: 1000,
            physical_width: 64,
            physical_height: 48,
            origin: tessera_model::Point { x: 0, y: 0 },
            logical_size: tessera_model::Size { w: 64, h: 48 },
        };
        struct EmptyPainter<'a> {
            renderer: &'a mut tessera_render::Renderer,
            server: &'a tessera_compositor::Server,
        }
        impl StreamPainter for EmptyPainter<'_> {
            fn paint_window_frame(
                &mut self,
                device: &flux::Device,
                canvas: &flux::Canvas,
                geometry: &WindowTreeGeometry,
                _cursor: Option<(u32, (f32, f32))>,
            ) {
                draw_window_tree(device, self.renderer, self.server, canvas, geometry);
            }
        }
        let mut painter = EmptyPainter {
            renderer: &mut renderer,
            server: &server,
        };

        // SHM: render into the cached readback target and read it back.
        let target = WindowShmTarget::new(&device, 64, 48).expect("shm target");
        render_window_stream_shm(
            &device,
            &target,
            interaction_domain_clear(scheme),
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
            assert!(std::time::Instant::now() < deadline, "window readback never completed");
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
        let (slot, fence) = render_window_stream_dmabuf(
            &device,
            &mut dmabuf,
            interaction_domain_clear(scheme),
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
}
