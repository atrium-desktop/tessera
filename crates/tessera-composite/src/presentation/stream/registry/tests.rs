use super::*;
use crate::presentation::stream::transport::{CaptureCursorState, window_stream_cursor};
use tessera_desktop::window::WindowId;

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
    assert_eq!(
        streams.due_shm_ids(t0 + Duration::from_millis(20)),
        vec![fast]
    );
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
    let target = StreamTarget::Window {
        window: WindowId(9),
    };
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
    stage: OffscreenStreamStage,
    dirty: bool,
) {
    streams.attach_offscreen(
        stream_id,
        OffscreenStream {
            target: OffscreenTarget::Window(tessera_desktop::window::WindowId(1)),
            shm: None,
            geometry_sig: (0, 0),
            origin: tessera_primitives::Point { x: 0, y: 0 },
            logical_size: tessera_primitives::Size { w: 100, h: 50 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
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
    attach_stub_window(&mut streams, id, OffscreenStreamStage::Idle, true);
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
    attach_stub_window(&mut streams, id, OffscreenStreamStage::Idle, true);
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
        .offscreen
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
        .offscreen
        .as_mut()
        .unwrap()
        .stage = OffscreenStreamStage::AwaitingReadback {
        security_generation: 1,
        damage: SampledDamage {
            origin: tessera_primitives::Point { x: 0, y: 0 },
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
    let origin = tessera_primitives::Point { x: 100, y: 50 };
    let size = tessera_primitives::Size { w: 200, h: 100 };
    // Inside: the draw position is window-relative.
    let drawn = window_stream_cursor(&state((150.0, 80.0)), origin, size).expect("cursor inside");
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
        origin: tessera_primitives::Point { x: 100, y: 50 },
        damage: FrameDamage::Area(vec![
            tessera_primitives::Rect::new(110, 60, 20, 10),
            // Fully outside the target: clipped away.
            tessera_primitives::Rect::new(500, 500, 20, 20),
        ]),
    };
    assert_eq!(
        damage_in_target(&sampled, (200, 100)),
        vec![tessera_primitives::Rect::new(10, 10, 20, 10)]
    );
    // Partially overlapping damage clips to the target extent.
    let sampled = SampledDamage {
        origin: tessera_primitives::Point { x: 100, y: 50 },
        damage: FrameDamage::Area(vec![tessera_primitives::Rect::new(90, 40, 20, 20)]),
    };
    assert_eq!(
        damage_in_target(&sampled, (200, 100)),
        vec![tessera_primitives::Rect::new(0, 0, 10, 10)]
    );
    // Damage that never intersected the target reports the full rect
    // (the wire contract never carries an empty list).
    let sampled = SampledDamage {
        origin: tessera_primitives::Point { x: 0, y: 0 },
        damage: FrameDamage::Area(vec![tessera_primitives::Rect::new(900, 900, 10, 10)]),
    };
    assert_eq!(
        damage_in_target(&sampled, (200, 100)),
        vec![tessera_primitives::Rect::new(0, 0, 200, 100)]
    );
    // Full and no accumulated damage both stay conservative.
    for damage in [FrameDamage::Full, FrameDamage::None] {
        let sampled = SampledDamage {
            origin: tessera_primitives::Point { x: 0, y: 0 },
            damage,
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_primitives::Rect::new(0, 0, 200, 100)]
        );
    }
}

#[test]
fn damage_accumulates_until_delivery_and_survives_drops() {
    let mut streams = OutputStreams::new();
    let id = start(&mut streams, 1, Some(30), (100, 100));
    let origin = tessera_primitives::Point { x: 0, y: 0 };
    // The first sample of a fresh stream is full (and the origin guard
    // would force that anyway).
    let sampled = streams.sample_damage(id, origin);
    assert!(matches!(sampled.damage, FrameDamage::Full));
    streams.streams.get_mut(&id).unwrap().note_delivered();

    // Two presented frames accumulate; the sample carries both rects.
    streams.accumulate_damage(&FrameDamage::Area(vec![tessera_primitives::Rect::new(
        1, 2, 3, 4,
    )]));
    streams.accumulate_damage(&FrameDamage::Area(vec![tessera_primitives::Rect::new(
        5, 6, 7, 8,
    )]));
    let sampled = streams.sample_damage(id, origin);
    assert_eq!(
        sampled.damage,
        FrameDamage::Area(vec![
            tessera_primitives::Rect::new(1, 2, 3, 4),
            tessera_primitives::Rect::new(5, 6, 7, 8),
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
    streams.accumulate_damage(&FrameDamage::Area(vec![tessera_primitives::Rect::new(
        9, 9, 1, 1,
    )]));
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
        FrameDamage::Area(vec![tessera_primitives::Rect::new(9, 9, 1, 1)])
    );
}

#[test]
fn damage_sample_reports_full_after_an_origin_move() {
    let mut streams = OutputStreams::new();
    let id = start(&mut streams, 1, Some(30), (100, 100));
    streams.accumulate_damage(&FrameDamage::Area(vec![tessera_primitives::Rect::new(
        1, 1, 5, 5,
    )]));
    let moved = streams.sample_damage(id, tessera_primitives::Point { x: 50, y: 50 });
    // The origin moved (stream starts with no recorded origin): the old
    // accumulation belongs to another coordinate space.
    assert!(matches!(moved.damage, FrameDamage::Full));
    // Same origin next time: back to the accumulated (still full, the
    // accumulator was never cleared).
    streams.streams.get_mut(&id).unwrap().note_delivered();
    streams.accumulate_damage(&FrameDamage::Area(vec![tessera_primitives::Rect::new(
        2, 2, 5, 5,
    )]));
    let steady = streams.sample_damage(id, tessera_primitives::Point { x: 50, y: 50 });
    assert_eq!(
        steady.damage,
        FrameDamage::Area(vec![tessera_primitives::Rect::new(2, 2, 5, 5)])
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
    attach_stub_window(&mut streams, window, OffscreenStreamStage::Idle, false);
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

#[test]
fn test_inv_arch_81_take_all_ends_every_live_stream() {
    let mut streams = OutputStreams::new();
    // One of each class: physical output, window, virtual output.
    start(&mut streams, 1, None, (100, 100));
    let window = streams
        .start(
            2,
            Some(30),
            (64, 64),
            StreamTarget::Window {
                window: tessera_desktop::window::WindowId(1),
            },
            StreamCursorMode::Hidden,
        )
        .stream_id;
    streams.attach_offscreen(
        window,
        OffscreenStream {
            target: OffscreenTarget::Window(tessera_desktop::window::WindowId(1)),
            shm: None,
            geometry_sig: (0, 0),
            origin: tessera_primitives::Point { x: 0, y: 0 },
            logical_size: tessera_primitives::Size { w: 64, h: 64 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: false,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        },
    );
    let cast = streams
        .start(
            3,
            Some(30),
            (1920, 1080),
            StreamTarget::Output {
                output: Some("VIRTUAL-1".into()),
            },
            StreamCursorMode::Embedded,
        )
        .stream_id;
    streams.attach_offscreen(
        cast,
        OffscreenStream {
            target: OffscreenTarget::VirtualOutput("VIRTUAL-1".into()),
            shm: None,
            geometry_sig: (1, 1),
            origin: tessera_primitives::Point { x: 1920, y: 0 },
            logical_size: tessera_primitives::Size { w: 1920, h: 1080 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: false,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        },
    );

    let mut ended = streams.take_all();
    ended.sort_unstable();
    assert_eq!(ended.len(), 3, "every live stream ends");
    assert!(streams.is_empty(), "no stream survives suspension");
    // A second call is inert (nothing left to end).
    assert!(streams.take_all().is_empty());
}

#[test]
fn live_virtual_outputs_counts_only_live_virtual_streams() {
    let mut streams = OutputStreams::new();
    // Physical output and window streams are not casts.
    start(&mut streams, 1, None, (100, 100));
    let window = streams
        .start(
            2,
            Some(30),
            (64, 64),
            StreamTarget::Window {
                window: tessera_desktop::window::WindowId(1),
            },
            StreamCursorMode::Hidden,
        )
        .stream_id;
    streams.attach_offscreen(
        window,
        OffscreenStream {
            target: OffscreenTarget::Window(tessera_desktop::window::WindowId(1)),
            shm: None,
            geometry_sig: (0, 0),
            origin: tessera_primitives::Point { x: 0, y: 0 },
            logical_size: tessera_primitives::Size { w: 64, h: 64 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: false,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        },
    );
    assert_eq!(streams.live_virtual_outputs(), 0);

    // A virtual-output stream counts; freezing it stops counting
    // (ADR-0188).
    let cast = streams
        .start(
            3,
            Some(30),
            (1920, 1080),
            StreamTarget::Output {
                output: Some("VIRTUAL-1".into()),
            },
            StreamCursorMode::Embedded,
        )
        .stream_id;
    streams.attach_offscreen(
        cast,
        OffscreenStream {
            target: OffscreenTarget::VirtualOutput("VIRTUAL-1".into()),
            shm: None,
            geometry_sig: (1, 1),
            origin: tessera_primitives::Point { x: 1920, y: 0 },
            logical_size: tessera_primitives::Size { w: 1920, h: 1080 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: false,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        },
    );
    assert_eq!(streams.live_virtual_outputs(), 1);
    streams.freeze(cast);
    assert_eq!(streams.live_virtual_outputs(), 0);
}

#[test]
fn test_inv_arch_79_offscreen_streams_exclude_physical_damage() {
    let mut streams = OutputStreams::new();
    let now = Instant::now();
    // A physical output stream accumulates the presented frame's damage...
    let physical = start(&mut streams, 1, None, (100, 100));
    // ...an offscreen window stream must not (`[INV-ARCH-79]`): a window
    // on (or moved to) a virtual output is excluded from the physical
    // scene, so physical damage describes coordinates the stream does not
    // render.
    let offscreen = streams
        .start(
            2,
            Some(30),
            (64, 64),
            StreamTarget::Window {
                window: tessera_desktop::window::WindowId(3),
            },
            StreamCursorMode::Hidden,
        )
        .stream_id;
    streams.attach_offscreen(
        offscreen,
        OffscreenStream {
            target: OffscreenTarget::Window(tessera_desktop::window::WindowId(3)),
            shm: None,
            geometry_sig: (0, 0),
            origin: tessera_primitives::Point { x: 2000, y: 40 },
            logical_size: tessera_primitives::Size { w: 64, h: 64 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: false,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        },
    );

    // Prime both streams' crop origins, then deliver a frame so each
    // accumulator starts clean (`note_delivered` clears it).
    let origin = tessera_primitives::Point { x: 0, y: 0 };
    streams.sample_damage(physical, origin);
    streams.sample_damage(offscreen, origin);
    streams
        .streams
        .get_mut(&physical)
        .expect("physical stream")
        .note_delivered();
    streams
        .streams
        .get_mut(&offscreen)
        .expect("offscreen stream")
        .note_delivered();

    // The physical frame's damage reaches the physical stream only.
    streams.accumulate_damage(&FrameDamage::Full);
    assert_eq!(
        streams.sample_damage(physical, origin).damage,
        FrameDamage::Full
    );
    assert_eq!(
        streams.sample_damage(offscreen, origin).damage,
        FrameDamage::None
    );

    // Offscreen damage comes only from its own tree, seeded in target
    // pixels with origin `(0, 0)`.
    let own = FrameDamage::from_rects(vec![tessera_primitives::Rect::new(1, 2, 3, 4)]);
    let sampled = streams.seed_offscreen_damage(offscreen, own);
    assert_eq!(sampled.origin, origin);
    assert_eq!(
        sampled.damage.area_rects(),
        Some(&[tessera_primitives::Rect::new(1, 2, 3, 4)][..])
    );
    // A drop folds the sample back, so the next delivery still covers it.
    streams
        .streams
        .get_mut(&offscreen)
        .expect("offscreen stream")
        .fold_damage_back(sampled);
    let again = streams.seed_offscreen_damage(offscreen, FrameDamage::None);
    assert_eq!(
        again.damage.area_rects(),
        Some(&[tessera_primitives::Rect::new(1, 2, 3, 4)][..])
    );
    let _ = now;
}

#[test]
fn test_inv_arch_76_virtual_output_stream_preserves_physical_direct_scanout() {
    let mut streams = OutputStreams::new();
    let now = Instant::now();
    let id = streams
        .start(
            1,
            Some(60),
            (1920, 1080),
            StreamTarget::Output {
                output: Some("VIRTUAL-1".into()),
            },
            StreamCursorMode::Embedded,
        )
        .stream_id;
    streams.attach_offscreen(
        id,
        OffscreenStream {
            target: OffscreenTarget::VirtualOutput("VIRTUAL-1".into()),
            shm: None,
            geometry_sig: (1, 1),
            origin: tessera_primitives::Point { x: 1920, y: 0 },
            logical_size: tessera_primitives::Size { w: 1920, h: 1080 },
            scale_milli: 1000,
            generations: std::collections::HashMap::new(),
            last_cursor: None,
            dirty: true,
            stage: OffscreenStreamStage::Idle,
            held_since: None,
        },
    );
    // `[INV-ARCH-76]`: a virtual-output stream renders offscreen and must
    // never disqualify physical direct scanout (`any_output_live`) or
    // force a physical composite (`forcing_due_*` / `due_shm_ids`).
    assert!(!streams.any_output_live());
    assert!(!streams.forcing_due_shm(now));
    assert!(!streams.forcing_due_dmabuf(now));
    assert!(streams.due_shm_ids(now).is_empty());
    assert!(streams.due_dmabuf_ids(now).is_empty());
    assert!(!streams.any_shm_embedded());
    // Yet it still wakes the main loop for its own offscreen drive.
    assert_eq!(streams.next_stream_wake_in(now), Some(Duration::ZERO));
}

#[test]
fn test_inv_arch_77_stream_renegotiate_rebuilds_target_in_place() {
    let mut streams = OutputStreams::new();
    let now = Instant::now();
    let id = start(&mut streams, 7, Some(30), (1920, 1080));
    streams.record_frame(id, now, true);
    streams.record_frame(id, now, false);
    assert_eq!(streams.sequence_and_dropped(id), Some((2, 1)));

    // Freeze on geometry change.
    streams.freeze(id);
    assert!(!streams.forcing_due_shm(now + Duration::from_secs(1)));

    // Another connection may not renegotiate stream `id`.
    assert!(
        streams
            .renegotiate(99, id, Some(60), (2560, 1440), None, None)
            .is_err()
    );

    // Owning connection renegotiates in place: `stream_id`, `sequence`,
    // and `dropped` survive, size updates, and the stream unfreezes.
    let info = streams
        .renegotiate(7, id, Some(60), (2560, 1440), None, None)
        .expect("renegotiate succeeds");
    assert_eq!(info.stream_id, id);
    assert_eq!((info.width, info.height), (2560, 1440));
    assert_eq!(streams.sequence_and_dropped(id), Some((2, 1)));
    assert_eq!(streams.target_of(id).map(|(_, s)| s), Some((2560, 1440)));
    assert!(streams.forcing_due_shm(now));
}
