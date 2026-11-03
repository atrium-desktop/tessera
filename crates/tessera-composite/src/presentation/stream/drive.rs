//! The capture-stream drive policies: the per-iteration offscreen-stream
//! drive and the dmabuf fence poll. Both are pure state-machine steps over
//! injected observations — the composition root supplies the scene painter,
//! the geometry resolver, the security gate, and the delivery lane.

use std::time::Instant;

use tessera_primitives::stream::StreamFramePayload;

use crate::presentation::damage::{
    ClientDamage, FrameDamage, offscreen_damage_to_target, union_frame_damage,
};

use super::registry::{
    OffscreenStreamStage, OffscreenTarget, OutputStreams, damage_in_target,
    window_stream_render_due,
};
use super::ring::SLOT_FENCE_TIMEOUT;
use super::transport::window_stream_cursor;
use super::transport::{
    BlitFailure, CaptureCursorState, DRM_FORMAT_XRGB8888, OffscreenGeometry, PendingSlotFrame,
    StreamPainter, fence_signaled, render_offscreen_stream_dmabuf, render_offscreen_stream_shm,
};

/// One offscreen-stream drive invocation (ADR-0127, ADR-0186): the clock, the
/// observations, and the content port for offscreen streams (windows and `VIRTUAL-*`).
pub struct OffscreenStreamDrive<'a> {
    pub now: Instant,
    pub device: &'a flux::Device,
    /// The window/output model signatures at this drive; geometry
    /// re-resolution is gated on them.
    pub signature: (u64, u64),
    /// Resolve an offscreen target's live geometry (window or virtual output).
    /// `Err` ends the stream with its reason.
    pub resolve_geometry: &'a mut dyn FnMut(&OffscreenTarget) -> Result<OffscreenGeometry, String>,
    /// Content-generation snapshot of an offscreen scene's surface tree (the
    /// dirty signal).
    pub tree_generations:
        &'a mut dyn FnMut(&OffscreenTarget) -> std::collections::HashMap<usize, u64>,
    /// The offscreen scene tree's own committed damage since the previous call, in
    /// compositor logical coordinates (ADR-0187). Called once per render,
    /// after the due decision, so the underlying tracker advances exactly
    /// once per rendered frame.
    pub tree_damage: &'a mut dyn FnMut(u64, &OffscreenTarget) -> ClientDamage,
    /// The theme cursor, when one is currently drawable.
    pub cursor_state: Option<CaptureCursorState>,
    /// The opaque clear colour for offscreen frames.
    pub clear: u32,
    /// The capture security generation snapshot at render time.
    pub security_generation: u64,
    pub painter: &'a mut dyn StreamPainter,
}

/// What an offscreen-stream drive decided and the composition root must apply:
/// streams to end (`StreamEnded`), streams to freeze with `StreamGeometryChanged(w, h)`,
/// and virtual outputs that rendered (for Wayland frame callbacks).
#[derive(Default)]
pub struct OffscreenDriveEffects {
    pub ended: Vec<(u64, String)>,
    pub frozen: Vec<(u64, u32, u32)>,
    pub rendered_virtual_outputs: Vec<String>,
}

impl OutputStreams {
    /// Drive every live offscreen stream one step (ADR-0127, ADR-0186): re-resolve
    /// geometry when the model moved (a size change freezes the stream with
    /// `StreamGeometryChanged`, a closed window or destroyed output ends it,
    /// a pure position move is followed silently), then render the streams
    /// whose pacing is due into their own offscreen targets — independently
    /// of presentation. Runs once per main-loop iteration; the loop's idle
    /// wait consults [`OutputStreams::next_stream_wake_in`] so dirty targets
    /// keep their `max_fps` cadence.
    pub fn drive_offscreen_streams(
        &mut self,
        drive: OffscreenStreamDrive<'_>,
    ) -> OffscreenDriveEffects {
        let OffscreenStreamDrive {
            now,
            device,
            signature: sig,
            resolve_geometry,
            tree_generations,
            tree_damage,
            cursor_state,
            clear,
            security_generation,
            painter,
        } = drive;
        let mut effects = OffscreenDriveEffects::default();
        let ids: Vec<u64> = self
            .streams
            .iter()
            .filter(|(_, stream)| !stream.frozen && stream.offscreen.is_some())
            .map(|(stream_id, _)| *stream_id)
            .collect();

        for stream_id in ids {
            let Some(stream) = self.streams.get_mut(&stream_id) else {
                continue;
            };
            let offscreen = stream
                .offscreen
                .as_mut()
                .expect("offscreen stream state attached");
            let target = offscreen.target.clone();

            // Geometry re-resolution is signature-gated: between signature
            // moves the cached origin/size/scale are authoritative.
            let mut fresh_geometry = None;
            if offscreen.geometry_sig != sig {
                offscreen.geometry_sig = sig;
                match resolve_geometry(&target) {
                    Ok(geometry) => {
                        if (geometry.physical_width, geometry.physical_height) != stream.size {
                            effects.frozen.push((
                                stream_id,
                                geometry.physical_width,
                                geometry.physical_height,
                            ));
                            continue;
                        }
                        offscreen.origin = geometry.origin;
                        offscreen.logical_size = geometry.logical_size;
                        offscreen.scale_milli = geometry.scale_milli;
                        fresh_geometry = Some(geometry);
                    }
                    Err(reason) => {
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

            let geometry = fresh_geometry.unwrap_or(OffscreenGeometry {
                scale_milli: offscreen.scale_milli,
                physical_width: stream.size.0,
                physical_height: stream.size.1,
                origin: offscreen.origin,
                logical_size: offscreen.logical_size,
            });

            let cursor_draw = (stream.cursor
                == tessera_primitives::stream::StreamCursorMode::Embedded)
                .then_some(cursor_state.as_ref())
                .flatten()
                .and_then(|state| {
                    window_stream_cursor(state, geometry.origin, geometry.logical_size)
                });

            // Dirty check: the scene's content generations and cursor position against
            // the last rendered snapshot. A re-render of a clean scene happens only
            // at the liveness tick.
            let live = tree_generations(&target);
            offscreen.dirty = live != offscreen.generations || cursor_draw != offscreen.last_cursor;
            let stage_idle = matches!(offscreen.stage, OffscreenStreamStage::Idle);
            if !window_stream_render_due(
                offscreen.dirty,
                stage_idle,
                stream.last_frame,
                stream.frame_interval,
                now,
            ) {
                continue;
            }

            // Damage for this frame is the scene tree's own committed
            // damage (ADR-0187), mapped onto the target's pixels; the
            // physical desktop frame never contributes to an offscreen stream.
            let scale = geometry.scale_milli as f32 / 1000.0;
            let own = offscreen_damage_to_target(
                tree_damage(stream_id, &target),
                geometry.origin,
                scale,
                stream.size,
            );
            let sampled = self.seed_offscreen_damage(stream_id, own);
            let Some(stream) = self.streams.get_mut(&stream_id) else {
                continue;
            };
            let offscreen = stream
                .offscreen
                .as_mut()
                .expect("offscreen stream state attached");

            match &mut offscreen.shm {
                Some(target_shm) => {
                    match render_offscreen_stream_shm(
                        device,
                        target_shm,
                        clear,
                        &target,
                        &geometry,
                        cursor_draw,
                        painter,
                    ) {
                        Ok(()) => {
                            offscreen.generations = live;
                            offscreen.last_cursor = cursor_draw;
                            offscreen.dirty = false;
                            offscreen.stage = OffscreenStreamStage::AwaitingReadback {
                                security_generation,
                                damage: sampled,
                            };
                            stream.last_frame = Some(now);
                            if let OffscreenTarget::VirtualOutput(ref conn) = target
                                && !effects.rendered_virtual_outputs.contains(conn)
                            {
                                effects.rendered_virtual_outputs.push(conn.clone());
                            }
                        }
                        Err(reason) => {
                            log::warn!(
                                "stream {stream_id}: offscreen frame render failed: {reason}"
                            );
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
                    match render_offscreen_stream_dmabuf(
                        device,
                        dmabuf,
                        clear,
                        &target,
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
                            offscreen.generations = live;
                            offscreen.last_cursor = cursor_draw;
                            offscreen.dirty = false;
                            stream.sequence += 1;
                            stream.last_frame = Some(now);
                            if let OffscreenTarget::VirtualOutput(ref conn) = target
                                && !effects.rendered_virtual_outputs.contains(conn)
                            {
                                effects.rendered_virtual_outputs.push(conn.clone());
                            }
                        }
                        Err(BlitFailure::Retryable(reason)) => {
                            log::warn!(
                                "stream {stream_id}: offscreen dmabuf frame failed: {reason}"
                            );
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
        mut deliver: impl FnMut(StreamFramePayload) -> bool,
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
                let payload =
                    StreamFramePayload::Slot(tessera_primitives::stream::StreamSlotFrame {
                        stream_id,
                        sequence: pending.sequence,
                        width,
                        height,
                        stride: dmabuf.slot_stride,
                        format: tessera_primitives::stream::StreamPixelFormat::Dmabuf {
                            drm_format: DRM_FORMAT_XRGB8888,
                            modifier: dmabuf.modifier,
                        },
                        damage: damage_in_target(&pending.damage, (width, height)),
                        dropped: pending.dropped,
                        slot: pending.slot as u32,
                        byte_len: dmabuf.slot_bytes,
                    });
                let delivered = deliver(payload);
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
