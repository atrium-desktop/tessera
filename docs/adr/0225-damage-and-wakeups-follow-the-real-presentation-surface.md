---
id: ADR-0225
title: "Damage and Wakeups Follow the Real Presentation Surface"
status: accepted
date: 2026-11-09
scope: composite/presentation, runtime, shell/dock, platform/nested
superseded_by: null
negative_knowledge: true
---

# ADR-0225: Damage and Wakeups Follow the Real Presentation Surface

- Status: Accepted
- Date: 2026-11-09
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-composite` (damage history, scheduler),
  `crates/tessera` (presentation, damage assessment, runtime wakeups),
  `crates/tessera-platform` (nested backend), `crates/tessera-ipc` (request
  wakeup), `crates/tessera-shell` (dock damage envelope), `libs/flux` (frame
  image index)
- Amends: [ADR-0077](0077-presentation-domain-redraw-state-machine.md),
  [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md),
  [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md),
  [ADR-0223](0223-a-failed-chrome-build-is-isolated-not-fatal.md)
- Extends: Optics ADR-0143 (regional damage) and ADR-0149 (damage-scoped
  frames)

---

## Context and Problem Statement

Two independent assumptions made the compositor present stale pixels and miss
wakeups:

1. **Damage history was keyed by GPU frame slot.** A nested FIFO swapchain
   owned five images while the compositor rendered three frames in flight.
   `Frame::index()` is the GPU slot (0..2); the acquired swapchain image is a
   different value (0..4). `record_composite_present` folded damage into
   `slots[frame_slot]`, so an image returning to scanout after several frames
   could be repainted with another image's history. The visible symptom was a
   window- or wallpaper-shaped ghost after a move: part of the frame was
   treated as "already up to date" because a different image had been.
2. **Damage rectangles stopped at geometry, not at the visual footprint.**
   Floating windows cast a shadow beyond their rect (4 px for the inline
   stroke, 44 px for the soft shadow), and a magnified dock tile grows inward
   past the panel body. Damage built from `Window`/panel geometry alone left
   those skirts and overshoots outside the repaint scope, so a moving window
   left its old shadow edge behind and a hovered tile could tear horizontally.
3. **Retry and wakeup causes were conflated.** A renderer acquire timeout
   (no swapchain image released) was handled like a KMS EBUSY rejection: the
   scheduler either retried at millisecond cadence or blocked for the
   one-second backend watchdog. Separately, the IPC server had no pollable
   wakeup, so an idle compositor observed commands only at the one-second
   maintenance tick.

---

## Decision Outcome

### 1. Damage history belongs to the swapchain image

Flux exposes `flux_frame_image_index()` / `Frame::image_index()`. Tessera keys
`composite_image_damage` by that value. `Frame::index()` remains the GPU slot
and is used only for per-slot GPU resources (filters, readback rings,
screenshot targets).

The two indices are not interchangeable. A swapchain may carry more images
than `FLUX_MAX_FRAMES_IN_FLIGHT`, and the presentation engine chooses the
image at acquire time. `record_composite_present` folds a presented frame's
damage into every *other image*, so the next acquisition of an image carries
every rect that changed while it was off screen.

### 2. A damage rectangle is the visual footprint

- A moving floating window's transition damage is inflated by the configured
  shadow reach: 4 px for `Resize`, `sigma * 3 + |offset_y| + 2` for `Soft`,
  zero for `None`.
- The dock's damage envelope includes the maximum magnified tile
  (`DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN`) on the side the icons grow
  toward. The shadow skirt is added on top of that envelope, never used as a
  substitute for it.

Both values are derived from the same constants the paint uses. A damage
envelope that guesses a different number is a second layout implementation.

### 3. Retry causes are explicit, and control requests wake the loop

`PresentationOutcome::Retry` carries a `RetryWait`:

- `BackendEvent` — a KMS batch owns scanout; the page-flip event wakes the
  loop, bounded by the watchdog. Unchanged DRM policy.
- `Deadline` — no backend event exists (an acquire timeout or a rebuilt
  surface); retry at the next presentation boundary. The scheduler never
  promotes a deadline retry on `reconcile_backend`: a deadline retry owns no
  event, so a "nothing pending" observation is not a promotion cause.

The acquire bound distinguishes what can actually block. A direct DRM surface
presents through the compositor's own offscreen ring; `vkAcquireNextImageKHR`
does not exist there, so `begin_frame` waits only for a slot fence three frames
old and uses a 250 ms bound. Dropping that frame forfeits its damage baselines
and forces a full repaint, so it is reserved for a genuinely stuck GPU. A host
swapchain can block because the outer compositor holds every image while the
nested window is occluded; its bound stays at one 60 Hz frame (16 ms), and the
deadline retry parks the loop instead of spinning.

The IPC server owns a shared eventfd (the capture worker's level-triggered
wake mux) and signals it for every inbound request. A command delivered to an
idle compositor is therefore observed on the next loop pass, not after the
maintenance timeout. The signal is causal readiness only; the request still
travels through its existing channel and authorization path.

### 4. Host dispatch takes read ownership

The nested backend uses the canonical libwayland sequence
`wl_display_prepare_read` → `poll` → `wl_display_read_events` /
`wl_display_cancel_read` → `wl_display_dispatch_pending`. The old
`wl_display_dispatch` could block in `read()` after the poll, even when the
poll returned for the compositor's wakeup fd or for an event the WSI had
already consumed. That parked the only loop thread and delayed input, IPC
commands, and window transitions past their deadlines. Read ownership makes
the wakeup fd and the host fd independent: the loop waits for either, reads
only when the host fd is readable, and never blocks in dispatch.

### 5. Nested presentation is mailbox-paced by an explicit frame callback

The nested surface is created with `vsync = false`
(`VK_PRESENT_MODE_MAILBOX_KHR` when the host advertises it). Before the
upcoming commit, `NestedHost::present` arms a `wl_surface.frame` callback. The
callback's `done` event is the outer compositor's latch acknowledgement:
`Backend::presentation_pending()` stays true while it is in flight, and
`PresentationScheduler` consumes it exactly like a DRM page-flip event. A
~2-refresh deadline (clamped to `16..=34 ms`) cancels a callback the host
withholds because the window is occluded, so an invisible nested window cannot
park the loop. Because `dispatch_host_events` owns the Wayland read side, no
WSI-internal FIFO wait can block the compositor inside `vkQueuePresentKHR`.

---

## Consequences

### Positive

- Partial repaint remains valid across any swapchain depth. An image is only
  treated as current when its own history says so.
- Window moves and dock hovers repaint their full visual footprint; stale
  shadow edges and clipped hover rows disappear without widening every frame
  to the output.
- A temporarily exhausted swapchain retries at vblank instead of spinning or
  sleeping for a second, and an IPC command is no longer invisible to the
  frame clock.
- Host input, IPC wakeups, and the compositor's own fd are independent poll
  sources; a host event that does not need a read cannot park the loop.
- The new flux accessor states the distinction in the API rather than leaving
  callers to assume slot == image.

### Negative / Trade-offs

- Damage history is sized by the swapchain's image count (five on the nested
  Wayland FIFO path) rather than the three frame slots. The vector is a few
  `FrameDamage` values; the correctness win is not close.
- Shadow inflation over-repaints the sides a shadow does not occupy. This is
  bounded by the shadow style and is cheaper than a second shadow-geometry
  oracle per frame.
- The IPC wake signals on queries too. One eventfd write per request is
  negligible compared with a second request taxonomy that would drift from
  the Handler trait.

---

## Rejected Alternatives

### Keep the slot-keyed ring and force the swapchain to three images

A driver is allowed to return more images than `minImageCount`. Clamping or
recreating until the count matches would depend on driver behaviour, break
when the presentation engine changes, and still misdescribe the acquired
image. The ring must follow what was acquired.

### Union all transition damage into one output-sized rect

The bounding box of a diagonal move is already an over-repaint; inflating it
to the output would delete regional damage for every window move. The shadow
skirt is a known constant, so inflate each rect instead.

### Increase the maintenance tick so IPC is "fast enough"

That trades an idle desktop's power for command latency and still leaves a
one-second worst case. A wakeup is the event the loop already knows how to
consume.

### Use FIFO for nested presentation

Measured with the old `wl_display_dispatch` loop: FIFO made Mesa's Wayland WSI
block inside `wl_display_dispatch_queue_timeout(..., 1s)` during
`vkQueuePresentKHR` whenever the host throttled or occluded the window. That
parked the compositor's only loop thread for a full second.

The constraint is gone now that dispatch owns the read side
(`wl_display_prepare_read` → `poll` → `wl_display_read_events` /
`wl_display_cancel_read`) and presentation is acknowledged by an explicit
`wl_surface.frame` callback instead of the WSI's internal FIFO wait. Nested
presentation therefore uses `VK_PRESENT_MODE_MAILBOX_KHR`; FIFO would
reintroduce the one-second stall without supplying synchronization the callback
does not already provide.
