//! The live capture-stream registry: pacing, damage accumulation, and
//! per-stream production state (ADR-0052/0126/0127/0130).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use tessera_primitives::stream::{StreamCursorMode, StreamTarget};

use tessera_primitives::stream::StreamInfo;

use crate::presentation::damage::{FrameDamage, union_frame_damage};

use super::ring::SlotRing;
use super::transport::{DRM_FORMAT_XRGB8888, DmabufCapture, DmabufStream, OffscreenShmTarget};

/// Default frame-rate cap when the client leaves `max_fps` unset.
pub const DEFAULT_MAX_FPS: u32 = 30;
/// Hard bounds on the negotiated frame-rate cap.
pub const MIN_MAX_FPS: u32 = 1;
pub const MAX_MAX_FPS: u32 = 240;
/// How long a live WINDOW stream may go without a rendered frame before the
/// liveness tick forces one re-render of its (clean) tree, so a consumer
/// observes ~1 fps and minimized windows keep honest thumbnails
/// (ADR-0127). Output streams pace differently: a due output stream
/// *forces a presentation* at its negotiated `max_fps` cadence, so the
/// liveness concept does not apply to them (ADR-0130).
pub const LIVENESS_INTERVAL: Duration = Duration::from_secs(1);

/// Damage sampled for one in-flight stream frame (ADR-0127): the raw
/// desktop-space accumulation plus the crop origin it was sampled against.
/// The stream's accumulator clears only when the frame is *delivered*; a
/// backpressure or security drop folds the sample back so the regions a
/// missed frame carried stay accumulated for the next one.
#[derive(Clone, Debug)]
pub struct SampledDamage {
    pub origin: tessera_primitives::Point,
    pub damage: FrameDamage,
}

/// The full-target damage rect reported whenever precise damage is
/// unavailable (a forced/liveness frame, a moved crop origin, or damage
/// that never intersected the target): over-reporting is always safe.
pub fn full_target_damage(size: (u32, u32)) -> Vec<tessera_primitives::Rect> {
    vec![tessera_primitives::Rect::new(
        0,
        0,
        size.0 as i32,
        size.1 as i32,
    )]
}

/// Translate a sampled desktop-space damage into one stream's target
/// coordinate space: shift by the crop origin and clip to the target
/// extent. Falls back to the full target rect when nothing precise
/// survives — including when every accumulated rect lay outside the crop,
/// which keeps the wire contract free of empty damage lists.
pub fn damage_in_target(
    sampled: &SampledDamage,
    size: (u32, u32),
) -> Vec<tessera_primitives::Rect> {
    let Some(rects) = sampled.damage.area_rects() else {
        return full_target_damage(size);
    };
    let extent = tessera_primitives::Rect::new(0, 0, size.0 as i32, size.1 as i32);
    let translated: Vec<tessera_primitives::Rect> = rects
        .iter()
        .filter_map(|rect| {
            tessera_primitives::Rect::new(
                rect.origin.x - sampled.origin.x,
                rect.origin.y - sampled.origin.y,
                rect.size.w,
                rect.size.h,
            )
            .intersect(extent)
        })
        .collect();
    if translated.is_empty() {
        full_target_damage(size)
    } else {
        translated
    }
}

/// Production state of one offscreen stream's SHM frame (ADR-0127, ADR-0186).
/// dmabuf offscreen streams track their in-flight frames in the slot ring instead.
#[derive(Debug)]
pub enum OffscreenStreamStage {
    /// No frame in flight: the stream may render when its pacing says so.
    Idle,
    /// A frame was submitted to the cached surface; its readback has not
    /// completed. Only one frame traverses a stream's surface at a time.
    AwaitingReadback {
        security_generation: u64,
        damage: SampledDamage,
    },
    /// The completed readback is being converted on the capture worker; the
    /// completion arrives keyed by stream id.
    Converting { damage: SampledDamage },
}

/// What kind of offscreen scene an offscreen stream renders (ADR-0127, ADR-0186).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OffscreenTarget {
    Window(tessera_desktop::window::WindowId),
    VirtualOutput(String),
}

/// Per-offscreen-stream independent rendering state (ADR-0127, ADR-0186).
/// Renders an isolated scene tree (a single window or a virtual extend output's
/// visible surfaces) directly into the stream's own target — never expanding or
/// cropping the physical KMS union framebuffer (`[INV-ARCH-76]`).
pub struct OffscreenStream {
    pub target: OffscreenTarget,
    /// The SHM readback target; `None` when the stream runs the dmabuf
    /// transport (its surface and canvas live in [`DmabufStream`]).
    pub shm: Option<OffscreenShmTarget>,
    /// Window/output model signatures at the last geometry re-resolution;
    /// the live geometry is re-resolved only when one of them moves.
    pub geometry_sig: (u64, u64),
    /// Logical origin and extent at the last geometry re-resolution, for
    /// renders between re-resolutions and the cursor hit test.
    pub origin: tessera_primitives::Point,
    pub logical_size: tessera_primitives::Size,
    /// Capture scale in milli-units at the last geometry re-resolution.
    pub scale_milli: u32,
    /// Content generations of the scene's surface tree at the last
    /// rendered frame; a mismatch against the live tree marks it dirty.
    pub generations: std::collections::HashMap<usize, u64>,
    /// Cursor shape and position at the last render, to detect cursor-only dirtiness.
    pub last_cursor: Option<(u32, (f32, f32))>,
    /// The tree changed since the last rendered frame (recomputed each
    /// drive); a dirty tree paces renders at the stream's `max_fps`.
    pub dirty: bool,
    /// In-flight SHM frame state.
    pub stage: OffscreenStreamStage,
    /// When a completed readback first found the capture worker reserved by
    /// a one-shot; a frame held too long logs once instead of starving
    /// silently. Cleared when the frame leaves for the worker.
    pub held_since: Option<Instant>,
}

/// What [`OutputStreams::reconcile_geometry`] decided for one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeometryAction {
    Unchanged,
    /// Freeze production and notify with `StreamGeometryChanged(w, h)`.
    Freeze(u32, u32),
    /// End the stream with `StreamEnded(reason)`.
    End(String),
}

pub struct OutputStream {
    pub conn_id: u64,
    pub frame_interval: Duration,
    pub last_frame: Option<Instant>,
    pub sequence: u64,
    pub dropped: u64,
    /// What the stream captures (ADR-0054, ADR-0126).
    pub target: StreamTarget,
    /// The negotiated cursor mode (IPC protocol 29, ADR-0127).
    pub cursor: StreamCursorMode,
    /// Physical size at start. A stream whose live target size differs is
    /// frozen with `StreamGeometryChanged`: consumers negotiate one fixed
    /// video size and restart or renegotiate explicitly.
    pub size: (u32, u32),
    /// Frozen after a geometry change (IPC protocol 29): the stream stays
    /// registered but is never due and never forces presentation until the
    /// client restarts or renegotiates it.
    pub frozen: bool,
    /// Zero-copy transport state (IPC protocol 25); `None` for SHM streams.
    pub dmabuf: Option<DmabufStream>,
    /// Independent offscreen rendering state (ADR-0127, ADR-0186); `None` for physical
    /// output targets.
    pub offscreen: Option<OffscreenStream>,
    /// Damage accumulated since this stream's last *delivered* frame, in
    /// physical desktop pixels. Initialized full so the first frame is
    /// complete; sampled (cloned) when a frame is captured for the stream
    /// and cleared only on successful delivery, so dropped frames keep
    /// their regions accumulated (ADR-0127).
    pub damage_since_delivery: FrameDamage,
    /// The crop origin the damage accumulator was last sampled against, for
    /// the origin-move guard (a moved crop invalidates the coordinate space
    /// of everything accumulated).
    pub damage_origin: Option<tessera_primitives::Point>,
    /// SHM output streams: the damage sample for the frame currently
    /// traversing the shared readback lane, cloned post-present for every
    /// due stream (offscreen and dmabuf streams carry theirs in their own
    /// in-flight state).
    pub pending_frame_damage: Option<SampledDamage>,
}

/// The live output streams, keyed by stream id.
#[derive(Default)]
pub struct OutputStreams {
    next_id: u64,
    pub streams: BTreeMap<u64, OutputStream>,
}

impl OutputStream {
    /// Whether this stream renders into its own offscreen target
    /// ([`OffscreenStream`]) rather than capturing from the physical union
    /// presentation surface (`[INV-ARCH-76]`).
    pub fn is_offscreen(&self) -> bool {
        self.offscreen.is_some()
    }

    /// Mutable access to the shared SHM readback state of an offscreen stream.
    pub fn offscreen_shm_parts(
        &mut self,
    ) -> Option<(
        &Option<OffscreenShmTarget>,
        &mut OffscreenStreamStage,
        &mut Option<Instant>,
    )> {
        self.offscreen.as_mut().map(|offscreen| {
            (
                &offscreen.shm,
                &mut offscreen.stage,
                &mut offscreen.held_since,
            )
        })
    }

    /// The frame was delivered: the damage accumulator restarts from here.
    pub fn note_delivered(&mut self) {
        self.damage_since_delivery = FrameDamage::None;
    }

    /// The frame never reached the consumer: fold its damage sample back
    /// into the accumulator so the next frame still covers its regions.
    pub fn fold_damage_back(&mut self, sampled: SampledDamage) {
        self.damage_since_delivery = union_frame_damage(
            std::mem::replace(&mut self.damage_since_delivery, FrameDamage::None),
            sampled.damage,
        );
    }
}

impl OutputStreams {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            streams: BTreeMap::new(),
        }
    }

    /// Register a stream and answer the IPC requester. `size` is the
    /// stream's physical pixel extent at start time — the output's extent
    /// for an output target, the window's for a window target (ADR-0054).
    pub fn start(
        &mut self,
        conn_id: u64,
        max_fps: Option<u32>,
        size: (u32, u32),
        target: StreamTarget,
        cursor: StreamCursorMode,
    ) -> StreamInfo {
        let max_fps = max_fps
            .unwrap_or(DEFAULT_MAX_FPS)
            .clamp(MIN_MAX_FPS, MAX_MAX_FPS);
        let stream_id = self.next_id;
        self.next_id += 1;
        self.streams.insert(
            stream_id,
            OutputStream {
                conn_id,
                frame_interval: Duration::from_secs(1) / max_fps,
                last_frame: None,
                sequence: 0,
                dropped: 0,
                target,
                cursor,
                size,
                frozen: false,
                dmabuf: None,
                offscreen: None,
                damage_since_delivery: FrameDamage::Full,
                damage_origin: None,
                pending_frame_damage: None,
            },
        );
        StreamInfo {
            stream_id,
            width: size.0,
            height: size.1,
            format: tessera_primitives::stream::StreamPixelFormat::Bgra8,
            slots: None,
        }
    }

    /// Attach the independent offscreen rendering state to a freshly started
    /// offscreen stream (ADR-0127, ADR-0186). The IPC reply is already
    /// computed; the state only takes part in the frame drives that follow.
    pub fn attach_offscreen(&mut self, stream_id: u64, offscreen: OffscreenStream) {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.offscreen = Some(offscreen);
        }
    }

    /// Renegotiate an existing stream in place after a target geometry change
    /// (`[INV-ARCH-77]`, ADR-0186). Replaces the backing dmabuf slot table or
    /// offscreen SHM readback surface at `size`, unfreezes the stream, and
    /// preserves `stream_id`, `sequence`, and `dropped` continuity.
    pub fn renegotiate(
        &mut self,
        conn_id: u64,
        stream_id: u64,
        max_fps: Option<u32>,
        size: (u32, u32),
        dmabuf_capture: Option<DmabufCapture>,
        offscreen_shm: Option<OffscreenShmTarget>,
    ) -> Result<StreamInfo, String> {
        let Some(stream) = self.streams.get_mut(&stream_id) else {
            return Err(format!("unknown stream {stream_id}"));
        };
        if stream.conn_id != conn_id {
            return Err(format!("unknown stream {stream_id}"));
        }
        if let Some(fps) = max_fps {
            let clamped = fps.clamp(MIN_MAX_FPS, MAX_MAX_FPS);
            stream.frame_interval = Duration::from_secs(1) / clamped;
        }
        stream.size = size;
        stream.frozen = false;
        stream.last_frame = None;
        stream.damage_since_delivery = FrameDamage::Full;
        stream.damage_origin = None;
        stream.pending_frame_damage = None;

        if let Some(offscreen) = stream.offscreen.as_mut() {
            offscreen.shm = if dmabuf_capture.is_some() {
                None
            } else {
                offscreen_shm
            };
            offscreen.generations.clear();
            offscreen.last_cursor = None;
            offscreen.dirty = true;
            offscreen.stage = OffscreenStreamStage::Idle;
            offscreen.held_since = None;
        }

        match dmabuf_capture {
            Some(capture) => {
                let slot_count = capture.table.fds.len();
                let slot_stride = capture.table.stride;
                let slot_bytes = capture.table.byte_len;
                let modifier = capture.modifier;
                let table = capture.table;
                stream.dmabuf = Some(DmabufStream {
                    surface: capture.surface,
                    canvas: capture.canvas,
                    modifier,
                    slot_stride,
                    slot_bytes,
                    ring: SlotRing::new(slot_count),
                    pending: Vec::new(),
                    ring_stalled: false,
                });
                Ok(StreamInfo {
                    stream_id,
                    width: size.0,
                    height: size.1,
                    format: tessera_primitives::stream::StreamPixelFormat::Dmabuf {
                        drm_format: DRM_FORMAT_XRGB8888,
                        modifier,
                    },
                    slots: Some(table),
                })
            }
            None => {
                stream.dmabuf = None;
                Ok(StreamInfo {
                    stream_id,
                    width: size.0,
                    height: size.1,
                    format: tessera_primitives::stream::StreamPixelFormat::Bgra8,
                    slots: None,
                })
            }
        }
    }

    /// Register a zero-copy dmabuf stream (IPC protocol 25): SHM start plus
    /// the capture-surface state, answered with the dmabuf format and the
    /// slot table the IPC layer transfers to the client.
    pub fn start_dmabuf(
        &mut self,
        conn_id: u64,
        max_fps: Option<u32>,
        size: (u32, u32),
        target: StreamTarget,
        cursor: StreamCursorMode,
        capture: DmabufCapture,
    ) -> StreamInfo {
        let slot_count = capture.table.fds.len();
        let slot_stride = capture.table.stride;
        let slot_bytes = capture.table.byte_len;
        let modifier = capture.modifier;
        let table = capture.table;
        let mut info = self.start(conn_id, max_fps, size, target, cursor);
        if let Some(stream) = self.streams.get_mut(&info.stream_id) {
            stream.dmabuf = Some(DmabufStream {
                surface: capture.surface,
                canvas: capture.canvas,
                modifier,
                slot_stride,
                slot_bytes,
                ring: SlotRing::new(slot_count),
                pending: Vec::new(),
                ring_stalled: false,
            });
        }
        info.format = tessera_primitives::stream::StreamPixelFormat::Dmabuf {
            drm_format: DRM_FORMAT_XRGB8888,
            modifier,
        };
        info.slots = Some(table);
        info
    }

    pub fn stop(&mut self, stream_id: u64) {
        self.streams.remove(&stream_id);
    }

    /// How many capture streams are live right now, across every transport
    /// and target. Drives the shell's recording indicator (ADR-0128).
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// Whether no capture streams are live right now.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Drop every stream `conn_id` owned (its IPC connection went away).
    pub fn disconnect(&mut self, conn_id: u64) {
        self.streams.retain(|_, stream| stream.conn_id != conn_id);
    }

    /// End every live stream and return their ids (`[INV-ARCH-81]`, ADR-0189).
    /// Used when presentation is suspended (session lock or an inactive
    /// backend): the security generation drops frames rendered after the
    /// boundary, but a consumer retains the last delivered frame, so a mirror
    /// or cast would otherwise keep showing pre-lock pixels. No stream
    /// survives suspension.
    pub fn take_all(&mut self) -> Vec<u64> {
        let ids: Vec<u64> = self.streams.keys().copied().collect();
        self.streams.clear();
        ids
    }

    /// The consumer finished reading a dmabuf stream's slot (IPC protocol
    /// 25). Unknown streams, SHM streams, and slots that are not pinned are
    /// ignored: releases carry no authority worth an error path.
    pub fn release_slot(&mut self, stream_id: u64, slot: u32) {
        let Some(dmabuf) = self
            .streams
            .get_mut(&stream_id)
            .and_then(|stream| stream.dmabuf.as_mut())
        else {
            return;
        };
        dmabuf.ring.release(slot);
    }

    /// Ids of OUTPUT-target streams due a frame at `now`, filtered by
    /// transport. A stream that never received a frame is due immediately.
    /// Frozen streams (geometry change pending a client restart) are never
    /// due. Window streams pace themselves independently of presentation
    /// (ADR-0127) and never appear here.
    fn due_ids_by_transport(&self, now: Instant, dmabuf: bool) -> Vec<u64> {
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
            .filter(|(_, stream)| !stream.is_offscreen())
            .filter(|(_, stream)| stream.dmabuf.is_some() == dmabuf)
            .filter(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Whether an OUTPUT-target stream is due a frame at its negotiated
    /// `max_fps` cadence — its first frame, or one full `frame_interval`
    /// without one — and may therefore *force* a presentation even on a
    /// static screen (ADR-0130: an active stream paces the loop, replacing
    /// ADR-0126's opportunistic-only capture whose one-second liveness
    /// floor starved consumers on quiet desktops). Window and virtual-output
    /// streams render offscreen and never force presentation (`[INV-ARCH-76]`).
    fn forcing_due(&self, now: Instant, dmabuf: bool) -> bool {
        self.streams
            .values()
            .filter(|stream| !stream.frozen)
            .filter(|stream| !stream.is_offscreen())
            .filter(|stream| stream.dmabuf.is_some() == dmabuf)
            .any(|stream| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
    }

    /// Whether a due SHM stream may force a presentation right now.
    pub fn forcing_due_shm(&self, now: Instant) -> bool {
        self.forcing_due(now, false)
    }

    /// Whether a due dmabuf stream may force a composite right now.
    pub fn forcing_due_dmabuf(&self, now: Instant) -> bool {
        self.forcing_due(now, true)
    }

    /// Whether any live physical OUTPUT-target stream exists. Direct scanout is
    /// disqualified while one does (ADR-0130): a page-flipped frame never
    /// passes through the compositor, so under scanout streams would only
    /// ever observe the forced-cadence composites. Offscreen window and
    /// virtual-output streams never disqualify physical direct scanout
    /// (`[INV-ARCH-76]`, ADR-0186).
    pub fn any_output_live(&self) -> bool {
        self.streams
            .values()
            .any(|stream| !stream.frozen && !stream.is_offscreen())
    }

    /// Count of live streams rendering a virtual extend output — the
    /// extended-display "casting" streams (ADR-0188). Distinct from a mirror
    /// (physical output) stream or a window capture.
    pub fn live_virtual_outputs(&self) -> u32 {
        self.streams
            .values()
            .filter(|stream| {
                !stream.frozen
                    && matches!(
                        stream.offscreen.as_ref().map(|o| &o.target),
                        Some(OffscreenTarget::VirtualOutput(_))
                    )
            })
            .count() as u32
    }

    /// Time until the soonest stream-driven wakeup at `now` — zero when one
    /// is already reached — across every live stream, or `None` when no
    /// streams exist. The main loop caps its idle wait with this. Output
    /// streams wake at their `frame_interval` deadline: a due output stream
    /// forces a presentation, so the loop must wake in time to drive it
    /// (ADR-0130). Offscreen streams (window ADR-0127 and virtual-output ADR-0186)
    /// additionally wake at their `max_fps` cadence while their tree is
    /// dirty, fall back to the liveness tick otherwise, and poll briefly
    /// while a readback is in flight.
    pub fn next_stream_wake_in(&self, now: Instant) -> Option<Duration> {
        self.streams
            .values()
            .filter(|stream| !stream.frozen)
            .map(|stream| {
                let pacing = stream.last_frame.map_or(Duration::ZERO, |last| {
                    (last + stream.frame_interval).saturating_duration_since(now)
                });
                let offscreen = stream.offscreen.as_ref().map(|o| (&o.stage, o.dirty));
                let Some((stage, dirty)) = offscreen else {
                    return pacing;
                };
                let liveness = stream.last_frame.map_or(Duration::ZERO, |last| {
                    (last + LIVENESS_INTERVAL).saturating_duration_since(now)
                });
                if matches!(stage, OffscreenStreamStage::AwaitingReadback { .. }) {
                    // The GPU readback is in flight; poll for it shortly.
                    return liveness.min(Duration::from_millis(1));
                }
                if dirty && matches!(stage, OffscreenStreamStage::Idle) {
                    return liveness.min(pacing);
                }
                liveness
            })
            .min()
    }

    /// Ids of due SHM streams (the shared-readback fan-out).
    pub fn due_shm_ids(&self, now: Instant) -> Vec<u64> {
        self.due_ids_by_transport(now, false)
    }

    /// Whether any SHM stream is due — the per-frame capture-gating hot path
    /// only needs the predicate; this is `due_shm_ids` without the Vec.
    pub fn any_shm_due(&self, now: Instant) -> bool {
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
            .filter(|(_, stream)| !stream.is_offscreen())
            .filter(|(_, stream)| stream.dmabuf.is_none())
            .any(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
    }

    /// Ids of due dmabuf streams (the post-present slot fan-out).
    pub fn due_dmabuf_ids(&self, now: Instant) -> Vec<u64> {
        self.due_ids_by_transport(now, true)
    }

    /// Whether any dmabuf stream is due (predicate-only, allocation-free).
    pub fn any_dmabuf_due(&self, now: Instant) -> bool {
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
            .filter(|(_, stream)| !stream.is_offscreen())
            .filter(|(_, stream)| stream.dmabuf.is_some())
            .any(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
    }

    /// Record that `stream_id` was offered a frame at `now`; `delivered`
    /// distinguishes a queued frame from a backpressure drop.
    pub fn record_frame(&mut self, stream_id: u64, now: Instant, delivered: bool) {
        let Some(stream) = self.streams.get_mut(&stream_id) else {
            return;
        };
        stream.last_frame = Some(now);
        if delivered {
            stream.sequence += 1;
        } else {
            stream.dropped += 1;
        }
    }

    /// The next sequence number and cumulative drop count for a frame about
    /// to be pushed. The frame metadata carries the drop count *before* this
    /// frame so a consumer can compute per-frame deltas.
    pub fn sequence_and_dropped(&self, stream_id: u64) -> Option<(u64, u64)> {
        self.streams
            .get(&stream_id)
            .map(|stream| (stream.sequence + 1, stream.dropped))
    }

    /// The crop target and start-time physical size of one live stream.
    pub fn target_of(&self, stream_id: u64) -> Option<(StreamTarget, (u32, u32))> {
        self.streams
            .get(&stream_id)
            .map(|stream| (stream.target.clone(), stream.size))
    }

    /// The negotiated cursor mode of one live stream (IPC protocol 29).
    pub fn cursor_of(&self, stream_id: u64) -> Option<StreamCursorMode> {
        self.streams.get(&stream_id).map(|stream| stream.cursor)
    }

    /// Whether any live SHM output stream negotiated the embedded cursor
    /// mode. Only then does a shared-readback binding attach a cursor
    /// snapshot for the worker to blend (ADR-0127); window and virtual-output
    /// streams draw their cursor on the GPU and never need it.
    pub fn any_shm_embedded(&self) -> bool {
        self.streams.values().any(|stream| {
            !stream.frozen
                && !stream.is_offscreen()
                && stream.dmabuf.is_none()
                && stream.cursor == StreamCursorMode::Embedded
        })
    }

    /// Fold one presented frame's damage into every live stream's
    /// accumulator (ADR-0127). Frozen streams skip: their accumulator is
    /// discarded with them when the client restarts into a fresh stream.
    pub fn accumulate_damage(&mut self, damage: &FrameDamage) {
        if matches!(damage, FrameDamage::None) {
            return;
        }
        for stream in self.streams.values_mut() {
            // Offscreen streams (window, virtual output) derive their own
            // repair regions from their surface tree, never from the physical
            // desktop frame (ADR-0187): a window on a virtual output is
            // excluded from the physical scene, so physical damage would
            // describe a coordinate space the stream does not render.
            if stream.frozen || stream.is_offscreen() {
                continue;
            }
            stream.damage_since_delivery = union_frame_damage(
                std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                damage.clone(),
            );
        }
    }

    /// Seed one offscreen stream's own-tree damage into its accumulator and
    /// return the sample (ADR-0187). Offscreen streams never receive the
    /// physical desktop's damage; their repair regions come from their own
    /// surface tree, already expressed in target pixels with origin `(0, 0)`
    /// by [`offscreen_damage_to_target`](crate::presentation::damage::offscreen_damage_to_target).
    /// A dropped frame folds its sample back (via `fold_damage_back`), so the
    /// next delivery still covers the missed regions.
    pub fn seed_offscreen_damage(&mut self, stream_id: u64, own: FrameDamage) -> SampledDamage {
        let origin = tessera_primitives::Point { x: 0, y: 0 };
        let Some(stream) = self.streams.get_mut(&stream_id) else {
            return SampledDamage {
                origin,
                damage: FrameDamage::Full,
            };
        };
        stream.damage_since_delivery = union_frame_damage(
            std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
            own,
        );
        stream.damage_origin = Some(origin);
        SampledDamage {
            origin,
            damage: stream.damage_since_delivery.clone(),
        }
    }

    /// Clone one stream's damage accumulator for a frame about to be
    /// captured, applying the origin-move guard: if the stream's crop
    /// origin moved since the last sample, everything accumulated belongs
    /// to the old coordinate space and only full damage is honest. The
    /// accumulator itself is NOT reset here; it clears on delivery
    /// ([`OutputStream::note_delivered`]) so a dropped frame's regions stay
    /// accumulated for the next one.
    pub fn sample_damage(
        &mut self,
        stream_id: u64,
        origin: tessera_primitives::Point,
    ) -> SampledDamage {
        let Some(stream) = self.streams.get_mut(&stream_id) else {
            return SampledDamage {
                origin,
                damage: FrameDamage::Full,
            };
        };
        let moved = stream.damage_origin != Some(origin);
        stream.damage_origin = Some(origin);
        let damage = if moved {
            FrameDamage::Full
        } else {
            stream.damage_since_delivery.clone()
        };
        SampledDamage { origin, damage }
    }

    /// Freeze a stream after a geometry change (IPC protocol 29): it stays
    /// registered but is never due and never forces presentation, until the
    /// client restarts it with `StreamOutputStop` + `StreamOutputStart` or
    /// renegotiates it with `StreamOutputRenegotiate`.
    pub fn freeze(&mut self, stream_id: u64) {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.frozen = true;
        }
    }

    /// Decide what a new presentation-surface geometry means for every live
    /// stream (IPC protocol 29, ADR-0126): a whole-desktop stream whose
    /// desktop size changed, and a connector stream whose output's size
    /// changed, are frozen with `StreamGeometryChanged` (the client
    /// restarts or renegotiates them at the new geometry); a connector that
    /// disappeared ends its stream. Pure position moves are followed silently
    /// at the next frame. Window and virtual-output streams detect their own
    /// geometry changes in their offscreen drives.
    pub fn reconcile_geometry(
        &self,
        frame_size: (u32, u32),
        scale: f32,
        outputs: &[tessera_desktop::output::OutputInfo],
    ) -> Vec<(u64, GeometryAction)> {
        use super::crop::resolve_output_rect;
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen && !stream.is_offscreen())
            .filter_map(|(stream_id, stream)| {
                let action = match &stream.target {
                    StreamTarget::Output { output: None } => {
                        if stream.size != frame_size {
                            GeometryAction::Freeze(frame_size.0, frame_size.1)
                        } else {
                            GeometryAction::Unchanged
                        }
                    }
                    StreamTarget::Output {
                        output: Some(connector),
                    } => {
                        match resolve_output_rect(
                            outputs,
                            connector,
                            scale,
                            frame_size.0,
                            frame_size.1,
                        ) {
                            Some(rect)
                                if (rect.size.w as u32, rect.size.h as u32) == stream.size =>
                            {
                                GeometryAction::Unchanged
                            }
                            Some(rect) => {
                                GeometryAction::Freeze(rect.size.w as u32, rect.size.h as u32)
                            }
                            None => {
                                GeometryAction::End(format!("output '{connector}' disconnected"))
                            }
                        }
                    }
                    // Window streams keep their lazy delivery-time detection.
                    StreamTarget::Window { .. } => GeometryAction::Unchanged,
                };
                (action != GeometryAction::Unchanged).then_some((*stream_id, action))
            })
            .collect()
    }
}

/// Whether a window stream renders in this drive (ADR-0127): a dirty tree
/// paces at the stream's `max_fps`; the liveness tick re-renders a clean
/// tree so a consumer still observes ~1 fps and minimized windows keep
/// honest thumbnails. A stream with a frame in flight never double-books
/// its surface.
pub fn window_stream_render_due(
    dirty: bool,
    stage_idle: bool,
    last_frame: Option<Instant>,
    frame_interval: Duration,
    now: Instant,
) -> bool {
    if !stage_idle {
        return false;
    }
    let fps_due = last_frame.is_none_or(|last| now.duration_since(last) >= frame_interval);
    let liveness_due = last_frame.is_none_or(|last| now.duration_since(last) >= LIVENESS_INTERVAL);
    (dirty && fps_due) || liveness_due
}

#[cfg(test)]
mod tests;
