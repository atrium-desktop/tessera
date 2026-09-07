//! The live capture-stream registry: pacing, damage accumulation, and
//! per-stream production state (ADR-0052/0126/0127/0130).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use tessera_ipc::{StreamCursorMode, StreamInfo, StreamTarget};

use crate::damage::{union_frame_damage, FrameDamage};

use super::transport::{DmabufCapture, DmabufStream, WindowShmTarget, DRM_FORMAT_XRGB8888};
use super::ring::SlotRing;

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
    pub origin: tessera_model::Point,
    pub damage: FrameDamage,
}

/// The full-target damage rect reported whenever precise damage is
/// unavailable (a forced/liveness frame, a moved crop origin, or damage
/// that never intersected the target): over-reporting is always safe.
pub fn full_target_damage(size: (u32, u32)) -> Vec<tessera_model::Rect> {
    vec![tessera_model::Rect::new(0, 0, size.0 as i32, size.1 as i32)]
}

/// Translate a sampled desktop-space damage into one stream's target
/// coordinate space: shift by the crop origin and clip to the target
/// extent. Falls back to the full target rect when nothing precise
/// survives — including when every accumulated rect lay outside the crop,
/// which keeps the wire contract free of empty damage lists.
pub fn damage_in_target(sampled: &SampledDamage, size: (u32, u32)) -> Vec<tessera_model::Rect> {
    let Some(rects) = sampled.damage.area_rects() else {
        return full_target_damage(size);
    };
    let extent = tessera_model::Rect::new(0, 0, size.0 as i32, size.1 as i32);
    let translated: Vec<tessera_model::Rect> = rects
        .iter()
        .filter_map(|rect| {
            tessera_model::Rect::new(
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

/// Production state of one window stream's SHM frame (ADR-0127). dmabuf
/// window streams track their in-flight frames in the slot ring instead.
#[derive(Debug)]
pub enum WindowStreamStage {
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

/// Per-window-stream independent rendering state (ADR-0127). The window's
/// complete surface tree renders into the stream's own target — never
/// cropped from the desktop frame — so occlusion, minimization, and foreign
/// workspaces cannot leak foreign pixels into the stream.
pub struct WindowStream {
    /// The SHM readback target; `None` when the stream runs the dmabuf
    /// transport (its surface and canvas live in [`DmabufStream`]).
    pub shm: Option<WindowShmTarget>,
    /// Window/output model signatures at the last geometry re-resolution;
    /// the live geometry is re-resolved only when one of them moves.
    pub geometry_sig: (u64, u64),
    /// Toplevel logical origin and extent at the last geometry
    /// re-resolution, for renders between re-resolutions and the
    /// cursor-over-window test.
    pub origin: tessera_model::Point,
    pub logical_size: tessera_model::Size,
    /// Capture scale in milli-units at the last geometry re-resolution.
    pub scale_milli: u32,
    /// Content generations of the window's surface tree at the last
    /// rendered frame; a mismatch against the live tree marks it dirty.
    pub generations: std::collections::HashMap<usize, u64>,
    /// The tree changed since the last rendered frame (recomputed each
    /// drive); a dirty tree paces renders at the stream's `max_fps`.
    pub dirty: bool,
    /// In-flight SHM frame state.
    pub stage: WindowStreamStage,
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
    /// video size and restart explicitly.
    pub size: (u32, u32),
    /// Frozen after a geometry change (IPC protocol 29): the stream stays
    /// registered but is never due and never forces presentation until the
    /// client restarts it.
    pub frozen: bool,
    /// Zero-copy transport state (IPC protocol 25); `None` for SHM streams.
    pub dmabuf: Option<DmabufStream>,
    /// Independent per-window rendering state (ADR-0127); `None` for output
    /// targets.
    pub window: Option<WindowStream>,
    /// Damage accumulated since this stream's last *delivered* frame, in
    /// physical desktop pixels. Initialized full so the first frame is
    /// complete; sampled (cloned) when a frame is captured for the stream
    /// and cleared only on successful delivery, so dropped frames keep
    /// their regions accumulated (ADR-0127).
    pub damage_since_delivery: FrameDamage,
    /// The crop origin the damage accumulator was last sampled against, for
    /// the origin-move guard (a moved crop invalidates the coordinate space
    /// of everything accumulated).
    pub damage_origin: Option<tessera_model::Point>,
    /// SHM output streams: the damage sample for the frame currently
    /// traversing the shared readback lane, cloned post-present for every
    /// due stream (window and dmabuf streams carry theirs in their own
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
                window: None,
                damage_since_delivery: FrameDamage::Full,
                damage_origin: None,
                pending_frame_damage: None,
            },
        );
        StreamInfo {
            stream_id,
            width: size.0,
            height: size.1,
            format: tessera_ipc::StreamPixelFormat::Bgra8,
            slots: None,
        }
    }

    /// Attach the independent-rendering state to a freshly started window
    /// stream (ADR-0127). The IPC reply is already computed; the state only
    /// takes part in the frame drives that follow.
    pub fn attach_window(&mut self, stream_id: u64, window: WindowStream) {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.window = Some(window);
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
        info.format = tessera_ipc::StreamPixelFormat::Dmabuf {
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
            .filter(|(_, stream)| stream.window.is_none())
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
    /// floor starved consumers on quiet desktops). Window streams render
    /// offscreen and never force presentation.
    fn forcing_due(&self, now: Instant, dmabuf: bool) -> bool {
        self.streams
            .values()
            .filter(|stream| !stream.frozen)
            .filter(|stream| stream.window.is_none())
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

    /// Whether any live OUTPUT-target stream exists. Direct scanout is
    /// disqualified while one does (ADR-0130): a page-flipped frame never
    /// passes through the compositor, so under scanout streams would only
    /// ever observe the forced-cadence composites.
    pub fn any_output_live(&self) -> bool {
        self.streams
            .values()
            .any(|stream| !stream.frozen && stream.window.is_none())
    }

    /// Time until the soonest stream-driven wakeup at `now` — zero when one
    /// is already reached — across every live stream, or `None` when no
    /// streams exist. The main loop caps its idle wait with this. Output
    /// streams wake at their `frame_interval` deadline: a due output stream
    /// forces a presentation, so the loop must wake in time to drive it
    /// (ADR-0130). Window streams (ADR-0127) additionally wake at their
    /// `max_fps` cadence while their tree is dirty, fall back to the
    /// liveness tick otherwise, and poll briefly while a readback is in
    /// flight.
    pub fn next_stream_wake_in(&self, now: Instant) -> Option<Duration> {
        self.streams
            .values()
            .filter(|stream| !stream.frozen)
            .map(|stream| {
                let pacing = stream.last_frame.map_or(Duration::ZERO, |last| {
                    (last + stream.frame_interval).saturating_duration_since(now)
                });
                let Some(window) = &stream.window else {
                    return pacing;
                };
                let liveness = stream.last_frame.map_or(Duration::ZERO, |last| {
                    (last + LIVENESS_INTERVAL).saturating_duration_since(now)
                });
                if matches!(window.stage, WindowStreamStage::AwaitingReadback { .. }) {
                    // The GPU readback is in flight; poll for it shortly.
                    return liveness.min(Duration::from_millis(1));
                }
                if window.dirty && matches!(window.stage, WindowStreamStage::Idle) {
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
            .filter(|(_, stream)| stream.window.is_none())
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
            .filter(|(_, stream)| stream.window.is_none())
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
    pub fn target_of(
        &self,
        stream_id: u64,
    ) -> Option<(StreamTarget, (u32, u32))> {
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
    /// snapshot for the worker to blend (ADR-0127); window streams draw
    /// their cursor on the GPU and never need it.
    pub fn any_shm_embedded(&self) -> bool {
        self.streams.values().any(|stream| {
            !stream.frozen
                && stream.window.is_none()
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
            if stream.frozen {
                continue;
            }
            stream.damage_since_delivery = union_frame_damage(
                std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                damage.clone(),
            );
        }
    }

    /// Clone one stream's damage accumulator for a frame about to be
    /// captured, applying the origin-move guard: if the stream's crop
    /// origin moved since the last sample, everything accumulated belongs
    /// to the old coordinate space and only full damage is honest. The
    /// accumulator itself is NOT reset here; it clears on delivery
    /// ([`OutputStream::note_delivered`]) so a dropped frame's regions stay
    /// accumulated for the next one.
    pub fn sample_damage(&mut self, stream_id: u64, origin: tessera_model::Point) -> SampledDamage {
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
    /// client restarts it with `StreamOutputStop` + `StreamOutputStart`.
    pub fn freeze(&mut self, stream_id: u64) {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.frozen = true;
        }
    }

    /// Decide what a new presentation-surface geometry means for every live
    /// stream (IPC protocol 29, ADR-0126): a whole-desktop stream whose
    /// desktop size changed, and a connector stream whose output's size
    /// changed, are frozen with `StreamGeometryChanged` (the client
    /// restarts them at the new geometry); a connector that disappeared
    /// ends its stream. Pure position moves are followed silently at the
    /// next frame. Window streams detect their own geometry changes lazily
    /// at delivery.
    pub fn reconcile_geometry(
        &self,
        frame_size: (u32, u32),
        scale: f32,
        outputs: &[tessera_model::output::OutputInfo],
    ) -> Vec<(u64, GeometryAction)> {
        use super::crop::resolve_output_rect;
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
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
                            None => GeometryAction::End(format!(
                                "output '{connector}' disconnected"
                            )),
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
mod tests {
    use super::*;
    use tessera_model::window::WindowId;
    use crate::stream::transport::{window_stream_cursor, CaptureCursorState};

    /// Start a plain whole-desktop SHM stream with a hidden cursor.
    fn start(streams: &mut OutputStreams, conn: u64, fps: Option<u32>, size: (u32, u32)) -> u64 {
        streams
            .start(
                conn,
                fps,
                size,
                StreamTarget::Output { output: None },
                StreamCursorMode::Hidden,
            )
            .stream_id
    }

    #[test]
    fn max_fps_defaults_and_clamps() {
        let mut streams = OutputStreams::new();
        let default = start(&mut streams, 1, None, (1920, 1080));
        assert_eq!(
            streams.streams[&default].frame_interval,
            Duration::from_secs(1) / 30
        );
        // 240 is the new negotiated ceiling (IPC protocol 29).
        let high = start(&mut streams, 1, Some(240), (1920, 1080));
        assert_eq!(
            streams.streams[&high].frame_interval,
            Duration::from_secs(1) / 240
        );
        let beyond = start(&mut streams, 1, Some(500), (1920, 1080));
        assert_eq!(
            streams.streams[&beyond].frame_interval,
            Duration::from_secs(1) / 240
        );
        let low = start(&mut streams, 1, Some(0), (1920, 1080));
        assert_eq!(streams.streams[&low].frame_interval, Duration::from_secs(1));
    }

    #[test]
    fn due_streams_respect_the_throttle() {
        let mut streams = OutputStreams::new();
        let fast = start(&mut streams, 1, Some(60), (100, 100));
        let slow = start(&mut streams, 1, Some(1), (100, 100));
        let t0 = Instant::now();
        // Both start due.
        assert_eq!(streams.due_shm_ids(t0), vec![fast, slow]);
        streams.record_frame(fast, t0, true);
        streams.record_frame(slow, t0, true);
        // 20ms later: only the 60fps stream is due again.
        assert_eq!(streams.due_shm_ids(t0 + Duration::from_millis(20)), vec![fast]);
        // 1.1s later: both.
        assert_eq!(
            streams.due_shm_ids(t0 + Duration::from_millis(1100)),
            vec![fast, slow]
        );
    }

    #[test]
    fn first_frame_is_immediately_due_and_forces_a_present() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        // A just-started stream is forcing-due (its first frame is forced
        // even on a static screen) and wakes the loop immediately.
        assert!(streams.forcing_due_shm(t0));
        assert_eq!(streams.next_stream_wake_in(t0), Some(Duration::ZERO));
        // Once framed, it forces again as soon as its max-fps interval
        // elapses — the stream paces the loop at its negotiated cadence.
        streams.record_frame(id, t0, true);
        let interval = Duration::from_secs(1) / 30;
        assert!(!streams.forcing_due_shm(t0 + Duration::from_millis(10)));
        assert!(streams.forcing_due_shm(t0 + interval));
    }

    #[test]
    fn max_fps_due_ness_forces_a_present_and_paces_the_loop() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let fast = start(&mut streams, 1, Some(60), (100, 100));
        streams.record_frame(fast, t0, true);
        // 20ms later the 60fps stream is due and may force a frame, even on
        // a static screen (ADR-0130).
        let t1 = t0 + Duration::from_millis(20);
        assert_eq!(streams.due_shm_ids(t1), vec![fast]);
        assert!(streams.forcing_due_shm(t1));
        // The loop wakes at the fps deadline, not a liveness tick.
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(10))
            .expect("stream live");
        let interval = Duration::from_secs(1) / 60;
        assert!(
            wait <= interval && wait > Duration::ZERO,
            "pacing wait: {wait:?}"
        );
        assert_eq!(
            streams.next_stream_wake_in(t0 + interval),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn frozen_streams_are_neither_due_nor_forcing_due() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(60), (100, 100));
        streams.freeze(id);
        // A frozen stream produces nothing and never wakes the loop.
        assert!(streams.due_shm_ids(t0).is_empty());
        assert!(!streams.forcing_due_shm(t0));
        assert!(!streams.any_output_live());
        assert_eq!(streams.next_stream_wake_in(t0), None);
        // A second, live stream is unaffected.
        let live = start(&mut streams, 2, Some(60), (100, 100));
        assert_eq!(streams.due_shm_ids(t0), vec![live]);
        assert!(streams.any_output_live());
        // Restart works: stop the frozen stream and start fresh at the new
        // geometry; the new stream is due immediately again.
        streams.stop(id);
        let restarted = start(&mut streams, 1, Some(60), (2560, 1440));
        assert!(streams.forcing_due_shm(t0));
        assert_eq!(
            streams.target_of(restarted).map(|(_, size)| size),
            Some((2560, 1440))
        );
    }

    #[test]
    fn next_stream_wake_paces_the_loop_at_the_soonest_due_stream() {
        let mut streams = OutputStreams::new();
        // No streams: no stream-driven wakeup.
        assert_eq!(streams.next_stream_wake_in(Instant::now()), None);

        // A stream that never received a frame is due immediately.
        let fast = start(&mut streams, 1, Some(60), (100, 100));
        assert_eq!(
            streams.next_stream_wake_in(Instant::now()),
            Some(Duration::ZERO)
        );

        // After frames, the wait is the remaining frame interval; a
        // second stream framed later pushes its own deadline out but the
        // soonest one wins.
        let t0 = Instant::now();
        streams.record_frame(fast, t0, true);
        let slow = start(&mut streams, 2, Some(1), (100, 100));
        streams.record_frame(slow, t0 + Duration::from_millis(100), true);
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(10))
            .expect("streams live");
        let fast_interval = Duration::from_secs(1) / 60;
        assert!(
            wait <= fast_interval && wait > Duration::from_millis(3),
            "fast stream is due at +16.7ms: {wait:?}"
        );
        assert_eq!(
            streams.next_stream_wake_in(t0 + fast_interval),
            Some(Duration::ZERO),
            "the fast stream reached its frame deadline"
        );

        // Stopping every stream removes the stream-driven wakeup.
        streams.stop(fast);
        streams.stop(slow);
        assert_eq!(streams.next_stream_wake_in(Instant::now()), None);
    }

    #[test]
    fn sequence_and_dropped_track_delivery() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        let now = Instant::now();
        assert_eq!(streams.sequence_and_dropped(id), Some((1, 0)));
        streams.record_frame(id, now, true);
        streams.record_frame(id, now, false);
        assert_eq!(streams.sequence_and_dropped(id), Some((2, 1)));
        streams.record_frame(id, now, true);
        assert_eq!(streams.sequence_and_dropped(id), Some((3, 1)));
    }

    #[test]
    fn window_stream_remembers_target_and_start_size() {
        let mut streams = OutputStreams::new();
        let target = StreamTarget::Window { window: WindowId(9) };
        let id = streams
            .start(
                1,
                None,
                (640, 480),
                target.clone(),
                StreamCursorMode::Hidden,
            )
            .stream_id;
        assert_eq!(streams.target_of(id), Some((target, (640, 480))));
        assert_eq!(streams.target_of(999), None);
    }

    #[test]
    fn cursor_mode_is_stored_on_the_stream_state() {
        let mut streams = OutputStreams::new();
        let id = streams
            .start(
                1,
                None,
                (100, 100),
                StreamTarget::Output { output: None },
                StreamCursorMode::Embedded,
            )
            .stream_id;
        assert_eq!(streams.streams[&id].cursor, StreamCursorMode::Embedded);
    }

    #[test]
    fn stop_and_disconnect_remove_stream_state() {
        let mut streams = OutputStreams::new();
        let a = start(&mut streams, 1, None, (100, 100));
        let b = start(&mut streams, 2, None, (100, 100));
        let c = start(&mut streams, 2, None, (100, 100));
        streams.stop(a);
        assert!(!streams.streams.contains_key(&a));
        streams.disconnect(2);
        assert!(streams.streams.is_empty());
        assert!(!streams.streams.contains_key(&b));
        assert!(!streams.streams.contains_key(&c));
    }

    #[test]
    fn live_stream_count_tracks_start_stop_and_disconnect() {
        // The recording indicator's state source (ADR-0128): every registry
        // mutation moves the count the shell mirrors.
        let mut streams = OutputStreams::new();
        assert_eq!(streams.len(), 0);
        let a = start(&mut streams, 1, None, (100, 100));
        let b = start(&mut streams, 1, None, (100, 100));
        start(&mut streams, 2, None, (100, 100));
        assert_eq!(streams.len(), 3);
        streams.stop(a);
        assert_eq!(streams.len(), 2);
        streams.stop(b);
        assert_eq!(streams.len(), 1);
        streams.disconnect(2);
        assert_eq!(streams.len(), 0);
    }

    /// A window stream in registry-only form (no GPU target): the state the
    /// pacing and wakeup tests exercise.
    fn attach_stub_window(
        streams: &mut OutputStreams,
        stream_id: u64,
        stage: WindowStreamStage,
        dirty: bool,
    ) {
        streams.attach_window(
            stream_id,
            WindowStream {
                shm: None,
                geometry_sig: (0, 0),
                origin: tessera_model::Point { x: 0, y: 0 },
                logical_size: tessera_model::Size { w: 100, h: 50 },
                scale_milli: 1000,
                generations: std::collections::HashMap::new(),
                dirty,
                stage,
                held_since: None,
            },
        );
    }

    #[test]
    fn window_streams_never_join_the_presentation_driven_lanes() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(60), (100, 50));
        attach_stub_window(&mut streams, id, WindowStreamStage::Idle, true);
        // Window streams render independently of presentation: they never
        // appear in the shared-readback or presentation-forcing sets.
        assert!(streams.due_shm_ids(t0).is_empty());
        assert!(streams.due_dmabuf_ids(t0).is_empty());
        assert!(!streams.forcing_due_shm(t0));
        assert!(!streams.forcing_due_dmabuf(t0));
        // ... but they still wake the loop: a dirty window stream with no
        // frame yet renders immediately.
        assert_eq!(streams.next_stream_wake_in(t0), Some(Duration::ZERO));
    }

    #[test]
    fn window_stream_wake_follows_pacing_state() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(60), (100, 50));
        attach_stub_window(&mut streams, id, WindowStreamStage::Idle, true);
        streams.record_frame(id, t0, true);
        // Dirty and idle: the wake is the fps deadline, not the liveness tick.
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .expect("stream live");
        assert!(
            wait <= Duration::from_millis(12) && wait > Duration::from_millis(5),
            "60fps deadline: {wait:?}"
        );
        // Clean tree: only the liveness tick remains.
        streams
            .streams
            .get_mut(&id)
            .unwrap()
            .window
            .as_mut()
            .unwrap()
            .dirty = false;
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .expect("stream live");
        assert!(
            wait > Duration::from_millis(900) && wait <= LIVENESS_INTERVAL,
            "liveness deadline: {wait:?}"
        );
        // Readback in flight: poll shortly regardless of pacing.
        streams
            .streams
            .get_mut(&id)
            .unwrap()
            .window
            .as_mut()
            .unwrap()
            .stage = WindowStreamStage::AwaitingReadback {
            security_generation: 1,
            damage: SampledDamage {
                origin: tessera_model::Point { x: 0, y: 0 },
                damage: FrameDamage::Full,
            },
        };
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .expect("stream live");
        assert!(wait <= Duration::from_millis(1), "readback poll: {wait:?}");
    }

    #[test]
    fn window_stream_render_due_paces_dirty_and_liveness() {
        let t0 = Instant::now();
        let interval = Duration::from_millis(16);
        // Never framed: due immediately (first frame is forced).
        assert!(window_stream_render_due(false, true, None, interval, t0));
        // Dirty + fps interval elapsed: render.
        assert!(window_stream_render_due(
            true,
            true,
            Some(t0),
            interval,
            t0 + interval
        ));
        // Dirty but inside the interval: wait.
        assert!(!window_stream_render_due(
            true,
            true,
            Some(t0),
            interval,
            t0 + Duration::from_millis(8)
        ));
        // Clean and inside the liveness tick: no render.
        assert!(!window_stream_render_due(
            false,
            true,
            Some(t0),
            interval,
            t0 + Duration::from_millis(500)
        ));
        // Clean but a full liveness tick gone: re-render (keeps minimized
        // thumbnails honest and the consumer fed).
        assert!(window_stream_render_due(
            false,
            true,
            Some(t0),
            interval,
            t0 + LIVENESS_INTERVAL
        ));
        // A frame in flight blocks everything, liveness included.
        assert!(!window_stream_render_due(true, false, None, interval, t0));
    }

    #[test]
    fn window_stream_cursor_clips_to_the_window_rect() {
        let state = |position| CaptureCursorState {
            position,
            shape: 1,
            hidden: false,
            client_surface: false,
        };
        let origin = tessera_model::Point { x: 100, y: 50 };
        let size = tessera_model::Size { w: 200, h: 100 };
        // Inside: the draw position is window-relative.
        let drawn =
            window_stream_cursor(&state((150.0, 80.0)), origin, size).expect("cursor inside");
        assert_eq!(drawn, (1, (50.0, 30.0)));
        // On the far edge (exclusive): outside.
        assert!(window_stream_cursor(&state((300.0, 80.0)), origin, size).is_none());
        // Outside on every other side.
        assert!(window_stream_cursor(&state((99.0, 80.0)), origin, size).is_none());
        assert!(window_stream_cursor(&state((150.0, 49.0)), origin, size).is_none());
        assert!(window_stream_cursor(&state((150.0, 150.0)), origin, size).is_none());
        // Exactly on the near edge: inside.
        assert!(window_stream_cursor(&state((100.0, 50.0)), origin, size).is_some());
    }

    #[test]
    fn damage_translates_and_clips_into_target_space() {
        let sampled = SampledDamage {
            origin: tessera_model::Point { x: 100, y: 50 },
            damage: FrameDamage::Area(vec![
                tessera_model::Rect::new(110, 60, 20, 10),
                // Fully outside the target: clipped away.
                tessera_model::Rect::new(500, 500, 20, 20),
            ]),
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_model::Rect::new(10, 10, 20, 10)]
        );
        // Partially overlapping damage clips to the target extent.
        let sampled = SampledDamage {
            origin: tessera_model::Point { x: 100, y: 50 },
            damage: FrameDamage::Area(vec![tessera_model::Rect::new(90, 40, 20, 20)]),
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_model::Rect::new(0, 0, 10, 10)]
        );
        // Damage that never intersected the target reports the full rect
        // (the wire contract never carries an empty list).
        let sampled = SampledDamage {
            origin: tessera_model::Point { x: 0, y: 0 },
            damage: FrameDamage::Area(vec![tessera_model::Rect::new(900, 900, 10, 10)]),
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_model::Rect::new(0, 0, 200, 100)]
        );
        // Full and no accumulated damage both stay conservative.
        for damage in [FrameDamage::Full, FrameDamage::None] {
            let sampled = SampledDamage {
                origin: tessera_model::Point { x: 0, y: 0 },
                damage,
            };
            assert_eq!(
                damage_in_target(&sampled, (200, 100)),
                vec![tessera_model::Rect::new(0, 0, 200, 100)]
            );
        }
    }

    #[test]
    fn damage_accumulates_until_delivery_and_survives_drops() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        let origin = tessera_model::Point { x: 0, y: 0 };
        // The first sample of a fresh stream is full (and the origin guard
        // would force that anyway).
        let sampled = streams.sample_damage(id, origin);
        assert!(matches!(sampled.damage, FrameDamage::Full));
        streams.streams.get_mut(&id).unwrap().note_delivered();

        // Two presented frames accumulate; the sample carries both rects.
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(1, 2, 3, 4)]));
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(5, 6, 7, 8)]));
        let sampled = streams.sample_damage(id, origin);
        assert_eq!(
            sampled.damage,
            FrameDamage::Area(vec![
                tessera_model::Rect::new(1, 2, 3, 4),
                tessera_model::Rect::new(5, 6, 7, 8),
            ])
        );
        // Sampling is a clone: a second sample (a retried capture) sees the
        // same accumulation; only delivery clears it.
        let again = streams.sample_damage(id, origin);
        assert_eq!(again.damage, sampled.damage);
        streams.streams.get_mut(&id).unwrap().note_delivered();
        let after = streams.sample_damage(id, origin);
        assert!(matches!(after.damage, FrameDamage::None));

        // A dropped frame folds its damage back: nothing is lost.
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(9, 9, 1, 1)]));
        let sampled = streams.sample_damage(id, origin);
        streams.streams.get_mut(&id).unwrap().note_delivered();
        streams
            .streams
            .get_mut(&id)
            .unwrap()
            .fold_damage_back(sampled);
        let reaccumulated = streams.sample_damage(id, origin);
        assert_eq!(
            reaccumulated.damage,
            FrameDamage::Area(vec![tessera_model::Rect::new(9, 9, 1, 1)])
        );
    }

    #[test]
    fn damage_sample_reports_full_after_an_origin_move() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(1, 1, 5, 5)]));
        let moved = streams.sample_damage(id, tessera_model::Point { x: 50, y: 50 });
        // The origin moved (stream starts with no recorded origin): the old
        // accumulation belongs to another coordinate space.
        assert!(matches!(moved.damage, FrameDamage::Full));
        // Same origin next time: back to the accumulated (still full, the
        // accumulator was never cleared).
        streams.streams.get_mut(&id).unwrap().note_delivered();
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(2, 2, 5, 5)]));
        let steady = streams.sample_damage(id, tessera_model::Point { x: 50, y: 50 });
        assert_eq!(
            steady.damage,
            FrameDamage::Area(vec![tessera_model::Rect::new(2, 2, 5, 5)])
        );
    }

    #[test]
    fn any_shm_embedded_tracks_output_shm_streams_only() {
        let mut streams = OutputStreams::new();
        assert!(!streams.any_shm_embedded());
        let hidden = start(&mut streams, 1, None, (100, 100));
        assert!(!streams.any_shm_embedded());
        let embedded = streams
            .start(
                1,
                None,
                (100, 100),
                StreamTarget::Output { output: None },
                StreamCursorMode::Embedded,
            )
            .stream_id;
        assert!(streams.any_shm_embedded());
        // A frozen embedded stream does not trigger cursor blending.
        streams.freeze(embedded);
        assert!(!streams.any_shm_embedded());
        // A window stream with the embedded mode is irrelevant here (it
        // draws its cursor on the GPU).
        streams.stop(hidden);
        streams.stop(embedded);
        let window = start(&mut streams, 1, None, (100, 100));
        streams.streams.get_mut(&window).unwrap().cursor = StreamCursorMode::Embedded;
        attach_stub_window(&mut streams, window, WindowStreamStage::Idle, false);
        assert!(!streams.any_shm_embedded());
    }

    #[test]
    fn forcing_due_shm_paces_and_drives_presentation_without_client_damage() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(60), (1920, 1080));
        let t0 = Instant::now();

        // 1. A freshly started SHM stream is due immediately (first frame forced).
        assert!(streams.forcing_due_shm(t0));
        assert!(streams.any_output_live());

        // Record framed at t0.
        streams.record_frame(id, t0, true);

        // 2. Mid-interval (e.g. 5ms after t0 for a 60fps stream ~16.6ms): not due yet.
        assert!(!streams.forcing_due_shm(t0 + Duration::from_millis(5)));
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .unwrap();
        assert!(wait <= Duration::from_millis(12));

        // 3. Once interval has passed (17ms): forcing is due again.
        assert!(streams.forcing_due_shm(t0 + Duration::from_millis(17)));

        // 4. Record next frame at t0 + 17ms: resets pacing.
        streams.record_frame(id, t0 + Duration::from_millis(17), true);
        assert!(!streams.forcing_due_shm(t0 + Duration::from_millis(20)));
        assert!(streams.forcing_due_shm(t0 + Duration::from_millis(35)));
    }

    #[test]
    fn release_slot_ignores_shm_and_unknown_streams() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, None, (100, 100));
        // No dmabuf transport anywhere: releases are inert no-ops.
        streams.release_slot(id, 0);
        streams.release_slot(999, 0);
    }

    #[test]
    fn due_ids_split_by_transport() {
        let mut streams = OutputStreams::new();
        let shm = start(&mut streams, 1, None, (100, 100));
        let now = Instant::now();
        assert_eq!(streams.due_shm_ids(now), vec![shm]);
        assert!(streams.due_dmabuf_ids(now).is_empty());
    }
}
