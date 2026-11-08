---
id: ADR-0220
title: "A Close Ghost Dissolves: Geometry and Alpha Are Separate Clocks, and the Hand-Off Frame Is Owned"
status: accepted
date: 2026-11-06
scope: desktop/transition, wayland/scene, wayland/window-manager, runtime/damage
superseded_by: null
negative_knowledge: true
---

# 0220. A Close Ghost Dissolves: Geometry and Alpha Are Separate Clocks, and the Hand-Off Frame Is Owned

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Maintainers
- Amends: [ADR-0124](0124-window-open-close-fade-transitions.md) (the close ghost's fade curve and its terminal frame), [ADR-0029](0029-animation-and-effect-policy.md) (one effect may now carry two clocks)
- Related: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md) (damage is a rectangle), [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md) (transition damage is reported, not guessed)

---

## Context and Problem Statement

Closing a window left a 180 ms compositor-owned ghost of its last frame that
shrank toward a one-sixteenth inset and faded to zero (ADR-0124). In practice
the animation was not visible for most of its life: the window appeared to sit
still and then vanish. Two independent defects produced that reading, and each
one alone would have been enough.

**The alpha was driven by the shrink's curve.** ADR-0124 gave the close flight
one value — `progress_at` on `Easing::EaseInCubic` — and used it for *both* the
rectangle and the opacity, as `1 - t`. `EaseInCubic` is `t³`, which is the right
shape for a shrink (it starts imperceptibly and accelerates into the exit) and
exactly the wrong shape for an alpha:

| elapsed | `1 - t³` (old alpha) | frame-to-frame drop |
| :--- | :--- | :--- |
| 25 % | 0.98 | ~0.02 |
| 50 % | 0.875 | ~0.05 |
| 75 % | 0.58 | ~0.16 |
| 92 % | 0.125 | ~0.24 |
| 100 % | 0 (settled) | ~0.125 in one 16 ms frame |

At 60 Hz the ghost was still **88 % opaque at 80 % of its duration** and then
dropped a quarter of its range inside the last two frames. That is not a fade;
it is a step, and a step at the end of an animation reads as "the animation was
cancelled and the window closed". The measured worst single-frame step under
the old curve was **0.37 of the full range**; under the new one it is 0.11.

**Nobody owned the frame after the ghost settled.** `settle_finished_transitions`
retained the ghosts and returned how many it retired, and the runtime calls it
at the start of every iteration. But a retired ghost's *pixels* were still on
the output, and the same iteration's damage assessment read
`transition_damage_rects()`, which iterates the ghosts that are still **in
flight**. Once the last ghost stopped being active, the rect it had been
painting was no longer damage: the assessment reported `FrameDamage::None`,
`transitions_pending` said the animation was over, and the loop was free to
sleep with a snapshot of the vanished window still composited on the desktop.
In the good case something unrelated dirtied the same area a frame or two later
and the ghost disappeared; in the bad case it sat there until the next input.

The two defects compound: the broken curve left a large opacity step at exactly
the moment the terminal frame stopped being repainted, so the last thing the
user saw was a *high-contrast* remnant switching off rather than a nearly
transparent one dissolving.

## Decision Drivers

- A fade must reach zero *at the sampled frame before it stops being drawn*, not
  at a deadline nobody renders.
- The shrink and the alpha answer different questions and must be tunable
  independently.
- No animation may end by leaving pixels nobody owns.
- Reduced motion must still resolve the whole flight in one frame.

## Decision Outcome

### 1. One effect, two clocks

`WindowTransition::opacity_at` no longer derives alpha from `progress_at`.
It samples the un-eased fraction of the timeline through the new
`linear_progress_at`, and each effect owns its own curve:

- **Open** keeps `easing.apply(linear)` — a fade-*in* should be legible early,
  so it eases out of the transparent start.
- **Close** uses `1 - smoothstep(linear)`. `smoothstep` (`t²(3 - 2t)`) has a
  zero slope at both ends, which is what a cross-fade needs: the last rendered
  frame is already within 0.06 of transparent, so retiring the ghost at the
  deadline cannot be seen.

`WindowTransition::progress_at` remains the **geometry** clock for every effect.
A close compresses it into the first `CLOSE_GEOMETRY_FRACTION` (0.8) of the
timeline through a private `geometry_clock` remap, so the shrink lands at 144 ms
and the alpha keeps falling for the remaining 36 ms. Every other effect keeps
its whole timeline for its rectangle, so nothing else changes shape.

`[INV-ARCH-111] A fade's alpha reaches zero before the frame it stops being
drawn.` Sampled at the presentation rate, no single frame of an `Open` or
`Close` flight may carry a step larger than a sixth of the range, and the last
in-flight frame must be within 0.06 of its endpoint. Enforced by
`a_close_fade_never_drops_a_visible_step_on_its_last_frames` and
`smoothstep_has_zero_velocity_at_both_ends`.

### 2. Retiring a ghost owes exactly one hand-off repaint

`RetiredGhostDamage` records the rects of ghosts that stopped being active (and
of ghosts evicted by the `MAX_CLOSING_FRAMES` bound, which are still painting at
full strength). `Server::transition_damage_rects` drains them into the next
assessment, and `Server::transitions_pending` reports `true` while any are
pending, so the frame that overwrites the ghost's last pixels is guaranteed to
happen before the loop sleeps.

`[INV-ARCH-112] An animation's terminal frame is damage somebody owns.` The frame
that retires an animation record must repaint the area that record painted, in
the same iteration, and keep the frame clock alive until it has. Enforced by
`settling_a_ghost_owes_one_handoff_repaint`, `an_evicted_ghost_also_owes_a_handoff_repaint`
and `closing_frame_capacity_drops_oldest` (which drives the production push
path, not a local `Vec`).

The hand-off is one-shot by construction: `RetiredGhostDamage::take` empties the
list, so a drained hand-off cannot re-arm itself and pin the loop.

### 3. What did not change

The retained-frame mechanism, the one-sixteenth inset target, the 180 ms
duration, the `MAX_CLOSING_FRAMES = 8` bound, the ghost's never-reused
presentation id, the dma-buf duplicate, and the reduced-motion short-circuit are
all unchanged. Reduced motion clears `duration_ms`, and `is_active_at` returns
false immediately — the ghost is recorded and retired in the same sweep, so the
hand-off books `min`-cost damage for a window that was never animated.

## Consequences

### Positive

- The close reads as a dissolve: motion for the first 144 ms, then a fading
  remnant for 36 ms that is already nearly invisible when it is reclaimed.
- The terminal frame is composed by a real frame instead of waiting for an
  unrelated event, so a closing window can no longer leave a stale rectangle on
  the desktop.
- The shrink and the alpha are independent knobs. Tuning how fast a window
  scales away no longer changes how it fades, which is what made the old single
  curve impossible to fix in one place.

### Negative / Trade-offs

- `WindowTransition` now carries two clocks and an effect-dependent remap. That
  is one more thing a future effect must reason about when it calls
  `progress_at`; the doc comment on that method states which clock it is.
- A ghost's geometry arrives at its inset target 36 ms before the transition
  settles. During that window `rect_at` returns the target, which is correct but
  means "the rect stopped changing" no longer implies "the transition is over".
  Consumers that care already use `is_active_at`, which did not change.
- The hand-off costs one extra composed frame per close. On a burst of closes
  the rects coalesce into one assessment, so the cost is bounded by the ghost
  table, not by the close count.

## Rejected Alternatives & Negative Knowledge

### Symmetric easing for both clocks (`ease_in_out_cubic` on alpha)
- **Why considered**: One curve for scale and alpha is simpler, and an
  ease-in-out alpha has the same zero-velocity endpoints `smoothstep` provides.
- **Why rejected**: An ease-in-out alpha holds near 1.0 for the first quarter,
  which makes the ghost look stuck before it starts fading — the same
  "nothing happens, then it's gone" complaint in a milder form. The asymmetry is
  the point: a shrink should accelerate, an alpha should leave immediately.

### Keep `EaseInCubic` and just lengthen the close to 400 ms
- **Why considered**: The step would be smaller in absolute milliseconds, and
  no curve or plumbing would change.
- **Why rejected**: It does not fix the shape. `1 - t³` at 80 % is still 0.49,
  so a 400 ms close would still drop half its opacity in the last 80 ms and the
  animation would additionally feel sluggish for its first 300 ms. A longer
  wrong curve is a longer wrong animation.

### Rely on the next damage event to overwrite the ghost
- **Why considered**: Any input, client commit, or clock tick repaints the area
  eventually, and the ghost table already bounds memory.
- **Why rejected**: "Eventually" is unbounded. A compositor with no input and a
  static desktop is the normal case for a closing window, and the failure is
  visible: a rectangle of a program the user just quit stays on screen. Damage
  ownership is not a cache-eviction hint.

### Retain the last frame at zero opacity for one extra frame instead of
recording damage
- **Why considered**: Keeping the ghost in the table for one more tick is a
  smaller diff than a pending-damage queue.
- **Why rejected**: It conflates "the animation is over" with "the pixels are
  gone". Every consumer of the table — occlusion, `transitions_pending`,
  the scene's ghost list — would then have to know about a zombie entry, and the
  loop would have to tick for a ghost whose opacity is provably zero. Recording
  the rect is the smaller *conceptual* change even though it is a larger diff.

## Verification

- `cargo nextest run -p tessera-desktop -E 'test(transition)'` — the two clocks,
  the per-frame step bound on a 60 Hz sample, the geometry landing at
  `CLOSE_GEOMETRY_FRACTION` while the alpha keeps falling, `smoothstep`'s zero
  endpoints and clamping, and that no other effect changed shape.
- `cargo nextest run -p tessera-wayland -E 'test(transitions)'` — ghost
  interpolation, the inset floor, the capacity bound through the production push
  path, the one-shot hand-off (including that it does not re-arm), and the
  evicted ghost's rect.
- `cargo nextest run -p tessera` — the runtime loop's damage and transition
  tests.
