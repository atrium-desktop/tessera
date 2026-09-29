---
id: ADR-0176
title: "Control Center Painted Canvas, Bounded Reveal, and Retired Backdrop Cover"
status: accepted
date: 2026-09-29
scope: shell/control-center, shell/chrome-contract, motion, presentation
superseded_by: null
negative_knowledge: true
---

# 0176. Control Center Painted Canvas, Bounded Reveal, and Retired Backdrop Cover

- Status: Accepted
- Date: 2026-09-29
- Deciders: Tessera Maintainers & Shell Architects
- Consulted: Compositor Runtime & Optics Graphics Team
- Informed: Design System & Accessibility Working Group
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0029](0029-animation-and-effect-policy.md), [ADR-0149](0149-backdrop-covers-fade-with-chrome.md), [ADR-0163](0163-decoupled-backdrop-blur-cache-and-continuous-fade.md), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md)

---

## Context and Problem Statement

The Control Center's enter and exit animations remained visibly janky on
high-refresh and high-resolution displays even after [ADR-0163](0163-decoupled-backdrop-blur-cache-and-continuous-fade.md)
decoupled the backdrop blur cache and removed the discrete `stepped_fade`
quantization. Users continued to report dropped frames and hitching opening and
closing the panel. Root-cause analysis across `tessera-shell`, the compositor
frame loop, and Optics `transit` found four compounding defects:

1. **A full-screen 16σ backdrop blur the panel never needed.** The Control
   Center declared a full-screen `BackdropCover` (`backdrop_blur_sigma() ==
   16.0` while active, `BackdropCover::region(display, ..)` covering the whole
   output). But the panel's own design (`crates/tessera-shell/src/components/control_center/README.md`,
   `docs/dev/design/command-panel.md`) specifies the opposite: *"The command
   panel paints through Lens only. Its `Chrome` implementation uses the default
   zero blur and empty backdrop-effect declarations even while open."* The code
   and its design document had silently diverged — the panel captured and
   blurred the entire desktop on every reveal frame only to fade a scrim cover
   over it.

2. **A material-key change on every reveal frame.** The cover's `opacity` and
   `wash` ride the eased reveal, and those fields are part of the *material*
   key, not the capture key. Every frame of the open and close therefore
   resolved `BackdropPlan::Recompute`: a full-resolution blur + glass + composite
   over the whole output, per frame, for the whole transition.

3. **A full-output repaint on every reveal frame.** `ControlCenter::damage_region`
   returned `None` (`conservative`) for the whole reveal because it animated the
   full-screen cover. `Shell::anim_damage_region` propagates that `None`, so the
   compositor fell back to `FrameDamage::Full` for every frame of the transition —
   the most expensive frame class the compositor has.

4. **An unbounded exponential reveal with no settle edge.** The reveal used
   `approach(reveal, target, 18.0, dt)` with a `0.004` snap. An exponential
   follow converges only asymptotically, so the panel kept issuing the (now
   expensive) full-output frames for ~0.3 s of visually imperceptible travel,
   and the frame loop could not prove when the panel had truly settled.

The absence of a settle drain (item 5) is a latent correctness bug the above
masked: the dock primes `FLUX_MAX_FRAMES_IN_FLIGHT` (3) full-output drain frames
when its morph settles so every swapchain ring slot overwrites transient
geometry ([ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md)
`[INV-ARCH-03]`). The Control Center never had an equivalent. While the panel's
backdrop cover held the desktop hostage at teardown this was invisible; the
moment the panel paints its own opaque full-screen canvas, a missing drain
reintroduces exactly the ghosted-ring artifact ADR-0171 eliminated for the dock —
worst of all under reduced motion, where the reveal snaps in a single frame and
the other ring slots still hold pre-open desktop pixels.

## Decision Drivers

- **Paint-only modals must pay paint-only cost.** A modal whose visual language
  is an opaque grouped canvas must not run a GPU blur pipeline or a full-output
  effect composite to render it.
- **Motion mechanism stays in Optics `transit`.** The reveal must travel on a
  bounded, provable-convergence primitive, not a hand-tuned exponential
  ([ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md) `[INV-ARCH-44]`).
- **Damage honesty under swapchain rotation.** Any chrome that covers a
  full-screen area must drain the ring on settle, exactly like the dock
  (`[INV-ARCH-03]`).
- **Design documents are contracts, not prose.** A documented rendering budget
  ("zero blur, empty backdrop-effect declarations") must be the enforced reality.

## Considered Options

- **Option 1 (Chosen): Paint the panel's own opaque canvas, retire its backdrop
  cover, bound the reveal on a `transit` spring, and add the settle drain.**
- **Option 2: Keep the 16σ backdrop blur cover and only localize its damage.**
- **Option 3: Keep the cover but stop animating its opacity/wash (declare it at
  constant strength).**
- **Option 4: Drop the full-screen cover entirely; float the cluster over the
  live desktop.**

## Decision Outcome

Chosen option: **Option 1**, because it makes the panel's documented rendering
budget true, removes the only two per-frame costs that scale with the output
resolution (the effect recompute and the forced full-output repaint), and closes
the latent ring-drain gap — all through primitives that already exist
(`transit::Spring`, the lens `Backdrop` placement band, the `[INV-ARCH-03]`
drain contract).

### Invariants & Behavioral Boundaries

- `[INV-ARCH-52] Paint-Only Modal Budget`: A chrome surface whose design language
  is an opaque painted canvas MUST NOT declare a backdrop capture, blur, frost,
  or analytic liquid glass. It paints its background in the lens pass. The
  Control Center (`ControlCenter`, `CommandPanel`) declares
  `backdrop_blur_sigma() == 0.0` for its entire lifecycle and returns no
  `backdrop_regions`.
- `[INV-ARCH-53] Full-Screen Chrome Drains the Swapchain Ring`: Any chrome that
  paints or animates a full-output (or full-width) opaque surface MUST, on the
  frame its motion settles, keep `anim_pending()` true and `damage_region()`
  full-output for exactly `FLUX_MAX_FRAMES_IN_FLIGHT` frames before releasing the
  chrome layer. This extends `[INV-ARCH-03]` from geometric morphs to full-screen
  opaque covers.
- `[INV-ARCH-54] Bounded Reveal Convergence`: Chrome reveal/close progress MUST
  advance on a `transit` spring whose settle is decided by the library's
  `settled(target, value_eps, velocity_eps)`, never by a hand-inlined
  `(value - target).abs() < eps` over an exponential follow.

### Implementation Specifics

1. **`tessera-shell::control_center`**:
   - `reveal` becomes a `transit::Spring` advanced with `SpringParams::new(300.0,
     1.0)` — critically damped so the full-screen canvas approaches its endpoint
      from one side (an under-damped reveal would overshoot past full opacity and
      flash). `[INV-ARCH-54]`.
   - `render_canvas` paints a full-display solid fill in the lens `Backdrop`
     band, its alpha scaled by `canvas_alpha(reveal)` (opaque at `1.0`, zero at
     `0.0`), so the canvas drains with the cluster instead of popping at teardown.
   - `backdrop_blur_sigma`/`backdrop_regions` are removed, delegating to the
     trait defaults (zero blur, no regions). `[INV-ARCH-52]`.
   - `settled_drain_frames` primes `SETTLED_DRAIN_FRAMES` (3) on the settle edge
     and holds `active()` true until the ring drains. `[INV-ARCH-53]`.
2. **`tessera-shell::component`**: `BackdropCover` is removed. It had no consumer
   other than the Control Center; the security dialogs and the app picker use
   `modal_scrim_backdrop`, which stays (constant-strength, no exit animation).

## Rejected Alternatives & Negative Knowledge

### Keep the 16σ Backdrop Blur and Only Localize Damage (Option 2)
- **Why considered**: Smallest diff; keep the panel's "island of blur over the
  desktop" look while removing the forced full-output repaint.
- **Why rejected**: The full-output repaint is not the dominant cost — the
  per-frame material-change `Recompute` of the full-resolution blur + glass +
  composite is. Localizing damage on a full-screen cover is also incoherent: the
  cover covers the whole output, so its footprint *is* the whole output. It keeps
  both the GPU cost and the contradiction with the panel's own design.

### Declare the Cover at Constant Strength (Option 3)
- **Why considered**: A constant-strength cover makes the material key stable, so
  every frame after the first resolves `BackdropPlan::Cached` — no per-frame
  recompute.
- **Why rejected**: This is precisely the "hold the gray plate then pop it away"
  defect [ADR-0149](0149-backdrop-covers-fade-with-chrome.md) was written to fix,
  reprised in reverse. It also still captures and blurs the entire desktop for a
  surface that paints its own opaque background — cost with no visual benefit.

### Float the Cluster Over the Live Desktop, No Cover (Option 4)
- **Why considered**: The fastest possible panel — the reveal's damage localizes
  to the cluster and the desktop is never occluded, so no full-output repaint is
  needed even mid-transition.
- **Why rejected**: It is a visible redesign with a behavioral regression. The
  Control Center is a modal that owns the screen: the desktop is not meant to
  show through, and click-away dismissal relies on a full-screen hit surface.
  Option 1 keeps the modal read while removing the cost; Option 4 changes the
  product.

### Expecting `transit` Adoption Alone to Fix the Jank (negative knowledge)
- **Why considered**: The reflex to "use the Optics animation library everywhere"
  as a performance fix.
- **Why rejected**: `transit` is pure scalar math on caller-owned state — no
  clock, no timeline, no scheduler, no rendering (`libs/transit/include/transit/transit.h`).
  [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md) already recorded this
  as negative knowledge: adopting `transit` buys mathematical provability and
  convergence, not frame-time. The jank lived in the compositor's damage and
  effect-graph policy, one layer above `transit`. The `transit` change in this ADR
  (a `Spring` for the reveal) is a *correctness and settle-edge* improvement that
  happens to also remove the exponential's long tail — not the fix itself.

## Consequences

### Positive
- Opening and closing the Control Center no longer captures, blurs, or composites
  the desktop; the transition is a paint-only canvas fade with no resolution-
  scaling GPU cost.
- The reveal's frame count is bounded and the frame loop can prove when the panel
  has settled (a `transit` spring `settled` query), replacing the asymptotic tail.
- The panel now drains the swapchain ring on settle, closing the ghosting gap
  that the retired cover had masked — including under reduced motion.

### Negative / Trade-offs
- The panel's background is now opaque by construction: it can never show the
  desktop or a live wallpaper behind it. This is the intended design (and the
  documented one), but it forecloses any future "glass control center" without a
  new ADR.
- The settle drain holds the chrome layer for 3 extra frames after every close;
  harmless while the panel is the only chrome, but every full-screen opaque
  surface now shares this obligation (`[INV-ARCH-53]`).
