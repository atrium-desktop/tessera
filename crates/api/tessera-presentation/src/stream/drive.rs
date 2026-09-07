//! The capture-stream drive policies: the per-iteration window-stream
//! drive and the dmabuf fence poll. Both are pure state-machine steps over
//! injected observations — the composition root supplies the scene painter,
//! the geometry resolver, the security gate, and the delivery lane.

use std::time::Instant;

use tessera_ipc::StreamFramePayload;
use tessera_model::window::WindowId;

use crate::damage::{union_frame_damage, FrameDamage};

use super::registry::{
    damage_in_target, window_stream_render_due, OutputStreams, WindowStreamStage,
};
use super::transport::window_stream_cursor;
use super::ring::SLOT_FENCE_TIMEOUT;
use super::transport::{
    fence_signaled, render_window_stream_dmabuf, render_window_stream_shm, BlitFailure,
    CaptureCursorState, DRM_FORMAT_XRGB8888, PendingSlotFrame, StreamPainter, WindowTreeGeometry,
};

/// One window-stream drive invocation: the clock, the observations, and the
/// content port. The composition root builds this once per main-loop
/// iteration; the registry mutates its own state and returns the outcomes
/// that must be applied to the world (freezes, ends).
pub struct WindowStreamDrive<'a> {
    pub now: Instant,
    pub device: &'a flux::Device,
    /// The window/output model signatures at this drive; geometry
    /// re-resolution is gated on them.
    pub signature: (u64, u64),
    /// Resolve a window's live tree geometry. `Err("unknown window ...")`
    /// means the window closed; any other error ends its stream.
    pub resolve_geometry: &'a mut dyn FnMut(WindowId) -> Result<WindowTreeGeometry, String>,
    /// Content-generation snapshot of a window's surface tree (the
    /// per-window dirty signal).
    pub tree_generations: &'a mut dyn FnMut(WindowId) -> std::collections::HashMap<usize, u64>,
    /// The theme cursor, when one is currently drawable. A client-owned
    /// cursor surface is not part of any window's surface tree, so window
    /// streams can never show it; the caller filters.
    pub cursor_state: Option<CaptureCursorState>,
    /// The opaque clear colour for window frames (the interaction-domain
    /// scene colour of the active scheme).
    pub clear: u32,
    /// The capture security generation snapshot at render time.
    pub security_generation: u64,
    pub painter: &'a mut dyn StreamPainter,
}

/// What a window-stream drive decided and the composition root must apply:
/// streams to end (`StreamEnded`), and streams to freeze with
/// `StreamGeometryChanged(w, h)`.
#[derive(Default)]
pub struct WindowDriveEffects {
    pub ended: Vec<(u64, String)>,
    pub frozen: Vec<(u64, u32, u32)>,
}

impl OutputStreams {
    /// Drive every live window stream one step (ADR-0127): re-resolve
    /// geometry when the window/output model moved (a size change freezes
    /// the stream with `StreamGeometryChanged`, a closed window ends it, a
    /// pure position move is followed silently), then render the streams
    /// whose pacing is due into their own offscreen targets — independently
    /// of presentation. Runs once per main-loop iteration; the loop's idle
    /// wait consults [`OutputStreams::next_stream_wake_in`] so dirty
    /// windows keep their `max_fps` cadence.
    pub fn drive_window_streams(&mut self, drive: WindowStreamDrive<'_>) -> WindowDriveEffects {
        let WindowStreamDrive {
            now,
            device,
            signature: sig,
            resolve_geometry,
            tree_generations,
            cursor_state,
            clear,
            security_generation,
            painter,
        } = drive;
        let mut effects = WindowDriveEffects::default();
        let ids: Vec<u64> = self
            .streams
            .iter()
            .filter(|(_, stream)| !stream.frozen && stream.window.is_some())
            .map(|(stream_id, _)| *stream_id)
            .collect();
        for stream_id in ids {
            let Some(stream) = self.streams.get_mut(&stream_id) else {
                continue;
            };
            let window_id = match stream.target {
                tessera_ipc::StreamTarget::Window { window } => window,
                _ => continue,
            };
            let window = stream
                .window
                .as_mut()
                .expect("window stream state attached");
            // Geometry re-resolution is signature-gated: between signature
            // moves the cached origin/size/scale are authoritative.
            let mut fresh_geometry = None;
            if window.geometry_sig != sig {
                window.geometry_sig = sig;
                match resolve_geometry(window_id) {
                    Ok(geometry) => {
                        if (geometry.physical_width, geometry.physical_height) != stream.size {
                            effects.frozen.push((
                                stream_id,
                                geometry.physical_width,
                                geometry.physical_height,
                            ));
                            continue;
                        }
                        window.origin = geometry.origin;
                        window.logical_size = geometry.logical_size;
                        window.scale_milli = geometry.scale_milli;
                        fresh_geometry = Some(geometry);
                    }
                    Err(reason) => {
                        // A window that vanished from the model closed; any
                        // other resolution failure (e.g. a zero-sized
                        // window) ends the stream with its own reason.
                        let reason = if reason.starts_with("unknown window") {
                            "window closed".to_owned()
                        } else {
                            reason
                        };
                        effects.ended.push((stream_id, reason));
                        continue;
                    }
                }
            }
            // Dirty check: the tree's content generations against the last
            // rendered snapshot. A re-render of a clean tree happens only
            // at the liveness tick.
            let live = tree_generations(window_id);
            window.dirty = live != window.generations;
            let stage_idle = matches!(window.stage, WindowStreamStage::Idle);
            if !window_stream_render_due(
                window.dirty,
                stage_idle,
                stream.last_frame,
                stream.frame_interval,
                now,
            ) {
                continue;
            }
            let geometry = fresh_geometry.unwrap_or(WindowTreeGeometry {
                window: window_id,
                scale_milli: window.scale_milli,
                physical_width: stream.size.0,
                physical_height: stream.size.1,
                origin: window.origin,
                logical_size: window.logical_size,
            });
            // Damage for this frame: the desktop-space accumulation clipped
            // to the window's physical rect, translated into target
            // coordinates at delivery time.
            let scale = geometry.scale_milli as f32 / 1000.0;
            let damage_origin = tessera_model::Point {
                x: (geometry.origin.x as f32 * scale).floor() as i32,
                y: (geometry.origin.y as f32 * scale).floor() as i32,
            };
            let cursor_draw = (stream.cursor == tessera_ipc::StreamCursorMode::Embedded)
                .then_some(cursor_state.as_ref())
                .flatten()
                .and_then(|state| window_stream_cursor(state, geometry.origin, geometry.logical_size));
            let sampled = self.sample_damage(stream_id, damage_origin);
            let Some(stream) = self.streams.get_mut(&stream_id) else {
                continue;
            };
            let window = stream
                .window
                .as_mut()
                .expect("window stream state attached");
            match &mut window.shm {
                Some(target) => {
                    match render_window_stream_shm(
                        device,
                        target,
                        clear,
                        &geometry,
                        cursor_draw,
                        painter,
                    ) {
                        Ok(()) => {
                            window.generations = live;
                            window.dirty = false;
                            window.stage = WindowStreamStage::AwaitingReadback {
                                security_generation,
                                damage: sampled,
                            };
                            stream.last_frame = Some(now);
                        }
                        Err(reason) => {
                            log::warn!("stream {stream_id}: window frame render failed: {reason}");
                            stream.dropped += 1;
                            stream.last_frame = Some(now);
                        }
                    }
                }
                None => {
                    let Some(dmabuf) = stream.dmabuf.as_mut() else {
                        continue;
                    };
                    let Some(_slot) = dmabuf.ring.next_submission_slot() else {
                        // The consumer still owns the ring's next slot.
                        stream.dropped += 1;
                        stream.last_frame = Some(now);
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
                    match render_window_stream_dmabuf(
                        device,
                        dmabuf,
                        clear,
                        &geometry,
                        cursor_draw,
                        painter,
                    ) {
                        Ok((slot, fence)) => {
                            if dmabuf.ring_stalled {
                                dmabuf.ring_stalled = false;
                                log::info!("stream {stream_id}: capture slots are flowing again");
                            }
                            dmabuf.ring.submitted(slot);
                            dmabuf.pending.push(PendingSlotFrame {
                                slot,
                                fence,
                                source: None,
                                sequence,
                                dropped,
                                submitted_at: now,
                                security_generation,
                                damage: sampled,
                            });
                            window.generations = live;
                            window.dirty = false;
                            stream.sequence += 1;
                            stream.last_frame = Some(now);
                        }
                        Err(BlitFailure::Retryable(reason)) => {
                            log::warn!("stream {stream_id}: window dmabuf frame failed: {reason}");
                            stream.dropped += 1;
                            stream.last_frame = Some(now);
                        }
                        Err(BlitFailure::Submitted(reason)) => {
                            effects.ended.push((stream_id, reason));
                        }
                    }
                }
            }
        }
        effects
    }

    /// Deliver dmabuf stream frames whose acquire fence signaled (IPC
    /// protocol 25), mirroring the SHM path's `read_pixels_ready` polling. A
    /// signaled frame is pushed to its connection lane and its slot becomes
    /// consumer-owned until `StreamBufferRelease`; a full lane or a wedged
    /// fence counts the frame as dropped and recycles the slot. A dropped
    /// frame's damage folds back into the stream's accumulator so the next
    /// delivered frame still covers its regions (ADR-0127).
    pub fn poll_dmabuf_fences(
        &mut self,
        now: Instant,
        permits: impl Fn(u64) -> bool,
        ipc: &tessera_ipc::Server,
    ) {
        for (stream_id, stream) in &mut self.streams {
            let stream_id = *stream_id;
            let Some(dmabuf) = stream.dmabuf.as_mut() else {
                continue;
            };
            let mut index = 0;
            while index < dmabuf.pending.len() {
                let signaled = fence_signaled(&dmabuf.pending[index].fence);
                let timed_out =
                    now.duration_since(dmabuf.pending[index].submitted_at) >= SLOT_FENCE_TIMEOUT;
                if !signaled && !timed_out {
                    index += 1;
                    continue;
                }
                let pending = dmabuf.pending.remove(index);
                if !signaled {
                    log::warn!(
                        "stream {stream_id}: slot {} acquire fence timed out; frame dropped",
                        pending.slot
                    );
                    dmabuf.ring.recycle(pending.slot);
                    stream.dropped += 1;
                    // Disjoint-field fold: `dmabuf` is still borrowed above.
                    stream.damage_since_delivery = union_frame_damage(
                        std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                        pending.damage.damage,
                    );
                    continue;
                }
                // Security-generation check (mirrors the SHM worker's
                // completion gate): a frame blitted before a lock→unlock or
                // VT boundary must never reach the consumer afterwards.
                if !permits(pending.security_generation) {
                    log::debug!(
                        "stream {stream_id}: slot {} crossed a security boundary; frame dropped",
                        pending.slot
                    );
                    dmabuf.ring.recycle(pending.slot);
                    stream.dropped += 1;
                    stream.damage_since_delivery = union_frame_damage(
                        std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                        pending.damage.damage,
                    );
                    continue;
                }
                let (width, height) = stream.size;
                let payload = StreamFramePayload::Slot(tessera_ipc::StreamSlotFrame {
                    stream_id,
                    sequence: pending.sequence,
                    width,
                    height,
                    stride: dmabuf.slot_stride,
                    format: tessera_ipc::StreamPixelFormat::Dmabuf {
                        drm_format: DRM_FORMAT_XRGB8888,
                        modifier: dmabuf.modifier,
                    },
                    damage: damage_in_target(&pending.damage, (width, height)),
                    dropped: pending.dropped,
                    slot: pending.slot as u32,
                    byte_len: dmabuf.slot_bytes,
                });
                let delivered = ipc.push_stream_frame(payload);
                dmabuf.ring.fence_signaled(pending.slot, delivered);
                if delivered {
                    // Disjoint from the `dmabuf` field borrow.
                    stream.damage_since_delivery = FrameDamage::None;
                } else {
                    stream.dropped += 1;
                    stream.damage_since_delivery = union_frame_damage(
                        std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                        pending.damage.damage,
                    );
                }
            }
        }
    }
}
