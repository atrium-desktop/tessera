//! Open/close transition (ADR-0029) unit tests: ghost-frame interpolation,
//! the inset target, capacity bounding, and reduced-motion short-circuits.

use super::*;

/// A closing ghost interpolates from the full rect toward the inset target
/// while fading to zero, then settles (and is reclaimed by
/// `settle_finished_transitions`).
#[test]
fn closing_frame_interpolates_then_settles() {
    let rect = tessera_primitives::Rect::new(100, 100, 800, 600);
    let frame = ClosingFrame {
        id: 7,
        rect,
        pixels: vec![0u8; 4],
        dmabuf: None,
        buffer_width: 800,
        buffer_height: 600,
        color: None,
        transition: tessera_desktop::transition::WindowTransition::close(rect, 1000),
    };

    let start = frame.rect_at(1000).expect("in flight at start");
    assert_eq!(start, rect, "close starts at the full rect");
    assert_eq!(frame.opacity_at(1000), Some(1.0));

    let mid = frame
        .rect_at(1000 + tessera_desktop::transition::CLOSE_DURATION_MS / 2)
        .expect("mid flight");
    // Shrinking toward the inset target: smaller and contained in the
    // original rect.
    assert!(mid.size.w < rect.size.w && mid.size.h < rect.size.h);
    assert!(mid.origin.x >= rect.origin.x && mid.origin.y >= rect.origin.y);
    let mid_opacity = frame
        .opacity_at(1000 + tessera_desktop::transition::CLOSE_DURATION_MS / 2)
        .expect("mid flight opacity");
    assert!(mid_opacity > 0.0 && mid_opacity < 1.0);

    let deadline = 1000 + tessera_desktop::transition::CLOSE_DURATION_MS;
    assert_eq!(frame.rect_at(deadline), None);
    assert_eq!(frame.opacity_at(deadline), None);
}

/// The close target insets by one-sixteenth per axis, centred, and never
/// collapses below the 2×2 floor.
#[test]
fn inset_rect_targets_sixteenth_and_keeps_floor() {
    let rect = tessera_primitives::Rect::new(0, 0, 160, 96);
    let inset = inset_rect(rect, rect.size.w / 16, rect.size.h / 16);
    assert_eq!(inset.size.w, 160 - 20);
    assert_eq!(inset.size.h, 96 - 12);
    // Centred shrink.
    assert_eq!(inset.origin.x, 10);
    assert_eq!(inset.origin.y, 6);

    let tiny = inset_rect(tessera_primitives::Rect::new(0, 0, 3, 3), 50, 50);
    assert_eq!(tiny.size, tessera_primitives::Size { w: 2, h: 2 });
}

/// Ghost-frame ids never repeat, so renderer texture caches cannot collide
/// across successive closes.
#[test]
fn closing_frame_ids_never_repeat() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1000 {
        assert!(seen.insert(next_closing_frame_id()));
    }
}

/// `MAX_CLOSING_FRAMES` bounds the ghost table: a burst of closes drops the
/// oldest ghost instead of growing compositor memory.
/// `MAX_CLOSING_FRAMES` bounds the ghost table: a burst of closes drops the
/// oldest ghost instead of growing compositor memory.
///
/// Drives the production push path rather than a local `Vec`, so the bound and
/// the eviction bookkeeping cannot drift from what the unmap path calls.
#[test]
fn closing_frame_capacity_drops_oldest() {
    use crate::State;

    const { assert!(MAX_CLOSING_FRAMES >= 4) };
    let state = Box::new(State::new(std::ptr::null_mut()));
    let mut server = std::mem::ManuallyDrop::new(Server {
        state,
        socket: String::new(),
        runtime_dir: None,
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    let rect = tessera_primitives::Rect::new(0, 0, 100, 100);
    for i in 0..(MAX_CLOSING_FRAMES + 3) {
        server.state.push_closing_frame(ClosingFrame {
            id: 100 + i,
            rect,
            pixels: vec![0u8; 4],
            dmabuf: None,
            buffer_width: 100,
            buffer_height: 100,
            color: None,
            transition: tessera_desktop::transition::WindowTransition::close(rect, 0),
        });
    }
    let frames = &server.state.closing_frames;
    assert_eq!(frames.len(), MAX_CLOSING_FRAMES);
    assert_eq!(
        frames.last().map(|frame| frame.id),
        Some(100 + MAX_CLOSING_FRAMES + 2)
    );
    // The evicted ghosts each owe a hand-off repaint (ADR-0220).
    assert!(server.state.retired_ghost_damage.pending());
}

/// ADR-0220: retiring a ghost owes exactly one hand-off repaint of the area it
/// was fading over, and `transitions_pending` must stay true for that frame —
/// otherwise the loop can sleep on a stale, almost-transparent ghost frame and
/// the close reads as a cut instead of a fade.
#[test]
fn settling_a_ghost_owes_one_handoff_repaint() {
    use crate::State;

    let state = Box::new(State::new(std::ptr::null_mut()));
    let mut server = std::mem::ManuallyDrop::new(Server {
        state,
        socket: String::new(),
        runtime_dir: None,
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    let rect = tessera_primitives::Rect::new(100, 100, 800, 600);

    // A ghost that is already past its deadline (a zero-duration transition
    // settles on the next sweep, like `reduced_motion` and the settled case).
    server.state.closing_frames.push(ClosingFrame {
        id: 1,
        rect,
        pixels: vec![0u8; 4],
        dmabuf: None,
        buffer_width: 800,
        buffer_height: 600,
        color: None,
        transition: tessera_desktop::transition::WindowTransition {
            duration_ms: 0,
            ..tessera_desktop::transition::WindowTransition::close(rect, 0)
        },
    });
    assert!(
        !server.state.retired_ghost_damage.pending(),
        "no hand-off is owed before the ghost settles"
    );

    assert_eq!(server.settle_finished_transitions(), 1);
    assert!(server.state.closing_frames.is_empty(), "ghost reclaimed");
    assert!(
        server.state.retired_ghost_damage.pending(),
        "the settled ghost's rect must be repainted once"
    );
    assert!(
        server.transitions_pending(),
        "the hand-off frame must be composed before the loop sleeps"
    );

    // The next damage assessment drains the hand-off and covers the ghost's
    // whole rect, so the desktop beneath it is recomposited.
    let rects = server.transition_damage_rects();
    assert_eq!(rects, vec![rect]);
    assert!(
        !server.state.retired_ghost_damage.pending(),
        "the hand-off is satisfied by exactly one frame"
    );
    assert_eq!(
        server.transition_damage_rects(),
        Vec::new(),
        "a drained hand-off must not re-arm itself"
    );
    assert!(
        !server.transitions_pending(),
        "nothing is in flight once the hand-off is composed"
    );
}

/// The hand-off frame is a *localized* repaint, not a full one.
///
/// `transitions_pending` and `live_transitions_pending` answer different damage
/// questions, and the runtime keys full-frame assessment off the second. If the
/// hand-off also reported `live_transitions_pending`, the base damage would be
/// `Full`, the drained rects would be subsumed, and the mechanism would be dead
/// on arrival — the exact defect this distinction exists to prevent (ADR-0220).
#[test]
fn a_settled_ghosts_handoff_is_localized_not_live() {
    use crate::State;

    let state = Box::new(State::new(std::ptr::null_mut()));
    let mut server = std::mem::ManuallyDrop::new(Server {
        state,
        socket: String::new(),
        runtime_dir: None,
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    let rect = tessera_primitives::Rect::new(100, 100, 800, 600);
    server.state.push_closing_frame(ClosingFrame {
        id: 1,
        rect,
        pixels: vec![0u8; 4],
        dmabuf: None,
        buffer_width: 800,
        buffer_height: 600,
        color: None,
        transition: tessera_desktop::transition::WindowTransition {
            duration_ms: 0,
            ..tessera_desktop::transition::WindowTransition::close(rect, 0)
        },
    });

    // While the ghost is *in flight* it is live: the frame may move pixels
    // outside the ghost's own rect, so it must be assessed in full.
    server.state.closing_frames[0].transition.duration_ms = 60_000;
    assert!(server.live_transitions_pending());
    server.state.closing_frames[0].transition.duration_ms = 0;

    assert_eq!(server.settle_finished_transitions(), 1);
    assert!(
        server.transitions_pending(),
        "the hand-off still owes a frame"
    );
    assert!(
        !server.live_transitions_pending(),
        "a settled ghost's hand-off frame is not a live animation: it must stay \
         localized, or the drained rect is subsumed by a full-output assessment"
    );
}

/// A ghost evicted by the table bound is still painting pixels, so it joins the
/// same hand-off bookkeeping a settled ghost uses.
#[test]
fn an_evicted_ghost_also_owes_a_handoff_repaint() {
    use crate::State;

    let state = Box::new(State::new(std::ptr::null_mut()));
    let mut server = std::mem::ManuallyDrop::new(Server {
        state,
        socket: String::new(),
        runtime_dir: None,
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    let ghost = |index: usize| {
        let rect = tessera_primitives::Rect::new(index as i32, 0, 100, 100);
        ClosingFrame {
            id: 1000 + index,
            rect,
            pixels: vec![0u8; 4],
            dmabuf: None,
            buffer_width: 100,
            buffer_height: 100,
            color: None,
            // Long enough that nothing settles on its own during the test.
            transition: tessera_desktop::transition::WindowTransition {
                duration_ms: 60_000,
                ..tessera_desktop::transition::WindowTransition::close(rect, 0)
            },
        }
    };
    for index in 0..MAX_CLOSING_FRAMES {
        server.state.push_closing_frame(ghost(index));
    }
    assert!(
        !server.state.retired_ghost_damage.pending(),
        "no eviction happened yet"
    );

    // One more close overflows the table and drops the oldest ghost.
    server.state.push_closing_frame(ghost(MAX_CLOSING_FRAMES));
    assert_eq!(server.state.closing_frames.len(), MAX_CLOSING_FRAMES);
    assert!(
        server
            .transition_damage_rects()
            .contains(&tessera_primitives::Rect::new(0, 0, 100, 100)),
        "the evicted ghost's rect must be repainted too"
    );
}
