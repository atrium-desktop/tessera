---
id: ADR-0226
title: "Single-Pass Chrome Composition, Live-Preview Thumbnail Layering, and Modal Dock Inhibition"
status: accepted
date: 2026-11-09
scope: runtime, composite/presentation, shell/dock
superseded_by: null
negative_knowledge: true
---

# ADR-0226: Single-Pass Chrome Composition, Live-Preview Thumbnail Layering, and Modal Dock Inhibition

- Status: Accepted
- Date: 2026-11-09
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera` (presentation, damage assessment), `crates/tessera-composite` (damage normalization, swapchain image history), `crates/tessera-shell` (modal coordination, Dock hover/preview lifecycle)
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md), [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md), [ADR-0220](0220-a-close-ghost-dissolves-separate-alpha-and-geometry-clocks-and-an-owned-handoff-frame.md), [ADR-0224](0224-a-pass-scope-is-the-damage-set.md), [ADR-0225](0225-damage-and-wakeups-follow-the-real-presentation-surface.md)
- Adopts: Optics ADR-0154 (pairwise-disjoint damage decomposition and idempotent UNORM8 output dither)

---

## Context and Problem Statement

Four compositor-level defects combined with the Flux pass/copy issues addressed
in Optics ADR-0154 to produce severe visual artifacts on the desktop:

1. **Redundant two-pass desktop/chrome split on every composited frame**:
   `begin_desktop_output` hardcoded `skip_stencil: true`, forcing
   `render_and_present` to close `base_session` (`out_encode` to 8-bit) and
   immediately open `begin_stencil_frame_overlay` (`LOAD` -> `out_decode` from
   8-bit back to `RGBA16F`, clearing stencil anyway, and running `out_encode` a
   second time) on every frame that drew shell chrome.
2. **Live-preview and window-switcher thumbnails buried under opaque `FloatingPanel` chrome**:
   `draw_window_switcher_cards` and `draw_live_preview_scenes` were called on
   `base_session` *before* `self.shell.submit`. After [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md)
   replaced analytic glass with flat opaque `FloatingPanel` surfaces (`#111622`,
   `alpha = 242`), `self.shell.submit` painted the opaque preview panel and card
   backgrounds *over* the window thumbnails that had just been drawn, leaving
   every Dock live-preview card and Window Switcher card as an empty dark-navy
   box above its title label.
3. **Dock hover popovers and magnification coexisting with modal surfaces (`Pivot`)**:
   Because `Dock::persistent_decoration()` is `true`, the resting Dock strip
   remains visible while a modal surface such as `Pivot` is open. However,
   `Dock::update` ignored `ChromeUpdate::LauncherActive`, and `Shell::prepare_frame`
   only sent `LauncherActive` during `build_scene` (after `component.prepare_frame`
   had already stepped springs and hover state). Consequently, opening `Pivot`
   did not dismiss an active Dock `live_preview` or `app_menu`, and moving the
   pointer over the bottom of the screen while `Pivot` was open still magnified
   Dock tiles and popped up `live_preview` cards while `Pivot::pointer_damage_regions`
   restricted repaint to `Pivot`'s central panel — leaving torn, unrepainted
   popover regions across the desktop.
4. **Premature `composite_repaint_for_image` history consumption and overlapping `FrameDamage::Area` rects**:
   `composite_repaint_for_image` used `std::mem::replace(&mut images[image], FrameDamage::None)`
   before the frame was submitted or presented; any frame that aborted or
   retried before `record_composite_present` lost its initial `FrameDamage::Full`
   or accumulated missed-frame damage. Meanwhile, `normalize_damage_rects` only
   removed fully-contained rectangles and left partially overlapping rectangles
   intact.
5. **Dock hover dwell/fade animation clobbered in `Dock::render` and tooltip damage band omitting magnification overshoot**:
   - `Dock::step_autohide_motion` (in `prepare_frame`) folded `tooltip_waiting`
     and `tooltip_fading` into `self.anim_active`, but `Dock::step_magnification_wave`
     (in `Dock::render`) unconditionally overwrote `self.anim_active = unsettled || autohide_moving || drag_active || dwell_active`
     *without* `tooltip_waiting` or `tooltip_fading`. As soon as the tile
     magnification spring settled (~180 ms into the 300 ms `TOOLTIP_DWELL`),
     `Runtime::update_animation_state` observed `anim_pending() == false` and
     parked the frame clock into `Idle` mid-dwell and mid-fade, making the Dock
     freeze unless the user continuously wiggled the pointer.
   - `Dock::damage_regions` measured the tooltip `band` outward from the resting
     panel height (`DOCK_PANEL_HEIGHT`) rather than from `self.capture_footprint(display)`
     (which includes `DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN`) and omitted the
     `SurfaceRole::Tooltip` shadow halo. On the opening frame of a tooltip above
     a magnified tile (before `render()` populated `hover_damage_rect`), the top
     cap of the tooltip pill extended ~15–29 px beyond the damage scissor,
     slicing the top of the `"Foot"` pill horizontally.
6. **Unpaced nested redraws on intermediate wakeups, 1-second FIFO `vkQueuePresentKHR` stalls, and post-idle `frame_dt` / watchdog spikes**:
   - `PresentationState::WaitingForEstimatedVblank` treated `redraw_queued: true`
     as immediately renderable (`can_redraw() == true`) even when entered from
     `submitted(false, ...)` or an active animation cycle (`redraw_after_cycle: true`),
     and `run_loop` unconditionally invoked `queue_frame_state` -> `queue_redraw()`
     on every wakeup. Any pointer motion event or WSI `wl_buffer.release` socket
     read therefore bypassed the refresh-interval deadline and triggered
     unpaced sub-millisecond renders.
   - Under `VK_PRESENT_MODE_FIFO_KHR`, Mesa's Wayland WSI blocked
     `vkQueuePresentKHR` inside `wl_display_dispatch_queue_timeout(..., 1_000_000_000)`
     for a full 1.000 second per frame whenever the outer compositor throttled
     or withheld `wl_surface.frame` (e.g., when the nested window was occluded
     or unfocused).
   - On the first frame waking from an idle sleep, `event_loop` clamped
     `frame_dt` to `1/15 s` (`66.7 ms` — jumping four 60 Hz frames in one step)
     and checked `watchdog.check_pacing_stall` against post-frame
     `self.animating` instead of pre-frame `was_animating`, emitting false
     `frame pacing interval was ...ms during active animation` warnings after
     every idle wakeup.

---

## Decision Outcome

### 1. Desktop surfaces and shell chrome share a single Canvas pass

`render_and_present` computes `draw_chrome` before calling `begin_desktop_output`
and passes `skip_stencil: !draw_chrome` directly to the pass options. Both
`draw_direct_desktop_scene` and `self.shell.submit` execute inside the same
`RGBA16F` working-space Canvas pass, encoding to the 8-bit destination once at
the end of the frame.

### 2. Window-switcher and Dock live-preview thumbnails draw after `self.shell.submit`

In both the main composition path and the `screenshot_freeze` capture path,
`draw_window_switcher_cards` and `draw_live_preview_scenes` are invoked *after*
`self.shell.submit`. Because `card.geometry.preview` (the upper thumbnail slot)
and `card.geometry.label` (the bottom title strip) are disjoint, drawing the
client-surface thumbnail clipped to `card.geometry.preview` after `self.shell.submit`
places the live window miniature cleanly on top of the opaque `FloatingPanel`
card background without covering the card's title label.

### 3. Immersive and modal surfaces inhibit Dock transient UI and pointer capture

- `Shell::prepare_frame` broadcasts `ChromeUpdate::LauncherActive(immersive_active)`
  (covering `modal_active`, `window_switcher_active`, `pivot_active`,
  `control_center_active`, `command_panel_active`, and `overview_active`) to all
  components *before* invoking `component.prepare_frame`.
- When `Dock` receives `ChromeUpdate::LauncherActive(true)`, it immediately
  invokes `dismiss_transient_ui()` (recording vacated `hover_damage_rect` and
  `menu_damage_rect` footprints) and inhibits `captures_pointer`, tile
  magnification (`in_band = false`), and tile hit-testing for the duration of
  the modal session.
- `DamageTracking::last_chrome_mode` includes `modal_or_exclusive_active()` and
  `control_center_active()` so every modal or Control Center transition forces a
  full-output repaint.

### 4. Pairwise-disjoint `normalize_damage_rects` and non-destructive `composite_repaint_for_image`

- `normalize_damage_rects` decomposes partially overlapping `Rect`s into
  strictly pairwise-disjoint `Rect`s (`cand \ existing` split into up to four
  non-overlapping bands), falling back to a single bounding box if the split
  exceeds `MAX_FRAME_DAMAGE_RECTS`.
- `composite_repaint_for_image` clones `images[image]` without clearing it; only
  `record_composite_present` clears `images[image]` to `FrameDamage::None` after
  the frame has actually presented.

### 5. Dock hover animation persistence and overshoot-aware tooltip/preview damage

- `Dock::hover_surface_animating()` (`tooltip_waiting || tooltip_fading`) is
  preserved in `Dock::step_autohide_motion`, `Dock::step_magnification_wave`,
  and at the end of `Dock::update_hover_surfaces` so `Dock::render` never clears
  `self.anim_active` while a tooltip or live-preview dwell/fade is in flight.
- `Dock::damage_regions` measures the hover `band` outward from
  `self.capture_footprint(display)` (which includes `DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN`)
  plus `TOOLTIP_BAND` and the `SurfaceRole::Tooltip` shadow halo (or the
  `SurfaceRole::FloatingPanel` live-preview envelope when `tooltip_tile` owns
  running windows), ensuring the first frame of a tooltip or live preview is
  never clipped by the output render area.

### 6. Non-blocking Wayland `wl_surface.frame` pacing, nested CPU-affinity isolation, and pointer-capture scene gating

- `NestedHost` (`crates/tessera-platform/src/nested/`) arms a Wayland
  `wl_surface.frame` callback (`arm_frame_callback`) immediately before
  `frame.present()` in `Host::present` and exposes `Backend::presentation_pending()`
  as `true` while the callback is in flight (bounded by a ~2-frame `16..=34 ms`
  safety deadline so an occluded or unfocused nested window never stalls). When
  the outer compositor latches the frame at its real vblank, `wl_callback.done`
  wakes `NestedHost::dispatch_host_events` and clears `frame_callback`, allowing
  `PresentationScheduler` to use `WaitingForVblank` on `Host::Nested` just like
  `Host::Drm`: animations lock to the outer compositor's true refresh rate, and
  interactive input arriving after vblank renders with zero artificial delay.
- `NestedHost` tracks per-output `(scale, refresh_mhz)` across `on_output_mode`
  and `on_surface_enter`, setting `mode_changed` so `take_resize` updates
  `server.set_outputs` without rebuilding the swapchain when only the refresh
  rate changed.
- `plan_from_config` (`crates/tessera/src/runtime/sched.rs`) disables
  single-core CPU pinning and `SCHED_FIFO` elevation when running on
  `Host::Nested` (`is_nested == true`), preventing an unprivileged nested
  development session from pinning its frame thread onto `CPU [1]` where the
  outer host `tessera` compositor runs at `SCHED_FIFO`.
- `Shell::prepare_frame` (`crates/tessera-shell/src/lib.rs`) and `process_input`
  (`crates/tessera/src/runtime/input.rs`) gate pure pointer motion on whether
  either the previous or current pointer position is captured by shell chrome
  (`captures_pointer_at`) and `!host.uses_software_cursor()`, eliminating
  redundant full-shell Lens scene rebuilds on every 1 px mouse report over
  client windows or the desktop wallpaper.
- `event_loop` records `was_animating` before `render_and_present`, advances
  `previous_render_at` only on `PresentationOutcome::Submitted` or active
  animation ticks, clamps the first post-idle frame's `frame_dt` to
  `refresh_interval`, and gates `watchdog.check_pacing_stall` on `was_animating`.

### 7. Per-window transitions are localized; only a workspace slide is full

A window move, open, close, minimize, or restore reports the union of the rect
it occupied and the rect it now occupies through `transition_damage_rects`,
inflated by the configured shadow reach. That set is complete: the scene draws
the window only at its current rect, the compositor shadow is bounded by the
same reach, and a settled ghost's hand-off is a one-shot rect (ADR-0220).
Treating every *live* transition as `FrameDamage::Full` made each moving-window
frame repaint the entire 5 MP output. At 120 Hz that cost more than one refresh
period, so the compositor missed every second vblank and a local 120 Hz session
ran at 60 Hz with visible motion judder.

Only `workspace_slide_pending` remains a full-output trigger: it translates
whole page layers across the output, so no localized rect describes it.

### 8. A moved surface's damage covers the new shadow skirt too

`ClientDamageTracker` reports both sides of a placement change as the
surface's *visual footprint* (`SurfacePlacement::bounds`, which includes the
compositor-drawn shadow skirt), not as the bare buffer rect. Using
`placement.rect()` for the destination side clipped the leading edge of a
moving floating window's shadow. Combined with Optics ADR-0154's damage-scope
ownership fix, the old body and shadow are now both repainted and the new
shadow is painted in full.

---

## Consequences

### Positive

- Every composited frame with shell chrome eliminates an entire full-damage
  `out_encode -> out_decode` round-trip and render-pass boundary.
- Dock live-preview cards and Window Switcher cards display live client window
  thumbnails above their opaque card backgrounds.
- Opening `Pivot` or any other modal surface cleanly dismisses Dock popovers and
  keeps the resting Dock strip inert until the modal closes.
- Aborted or retried frames never drop per-image swapchain damage history.
- A moving or closing window no longer repaints the full output: the 120 Hz
  cadence stays at 120 Hz, and the old body/shadow is covered by a local rect.
- Damage-scoped passes always encode their scope. The Rust binding builds its
  damage list in a temporary `Vec`; before Optics ADR-0154 made the pass own a
  copy, the end-of-pass encode resolved a dangling pointer and could emit zero
  output blits — the direct cause of persistent window-shadow trails and
  frozen closing-ghost rectangles.

### Negative

- A workspace slide still pays a full-output repaint per sampled frame. That is
  inherent to translating whole page layers, not a missing localization.
- The live-transition path depends on `transition_damage_rects` covering every
  effect. A future effect that paints outside its from/to union must teach that
  function its footprint rather than reintroducing a blanket full repaint.
