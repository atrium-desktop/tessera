---
id: ADR-0254
title: "Dual-Channel Reachability and Component-Mounted Eight-Zone Hot Zones"
status: accepted
date: 2026-11-25
scope: shell, shell/hot-zone, shell/dock, config, architecture
superseded_by: null
negative_knowledge: true
---

# 0254. Dual-Channel Reachability and Component-Mounted Eight-Zone Hot Zones

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Supersedes: [ADR-0198](0198-collapsed-dock-capsule-hugs-its-edge-behind-an-independent-reveal-corridor.md) (the collapsed Dock capsule and its independent reveal corridor)
- Amends: [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md) (Dock autohide reveal is now owned by the unified Hot Zone system rather than a Dock-specific collapsed capsule), [ADR-0251](0251-the-dock-sits-a-sliver-off-its-edge.md) (the collapsed stadium capsule is removed; expanded dock edge margin remains 4 px)
- Related: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md), [ADR-0239](0239-the-control-center-is-a-right-anchored-sheet.md), [ADR-0217](0217-idle-chrome-does-not-keep-the-frame-clock-and-hover-damage-is-bounded-by-the-component-that-owns-it.md)
- Amended by: [ADR-0259](0259-topmost-fullscreen-output-ownership-and-stacking-aware-space-use.md) — Hot Zones remain silent only while a fullscreen window is topmost; raising a non-fullscreen window above it immediately restores Hot Zone reachability. [ADR-0267](0267-in-bounds-bezel-flush-hot-zone-geometry-and-damage-parity.md) — Hot Zone affordances paint strictly in-bounds using Optics ADR-0175 per-corner analytic squircle SDF with bounded `pointer_damage_regions` and immediate suppression retirement.

---

## Context and Problem Statement

Desktop users alternate between two single-handed postures: keyboard-only
(typing, coding) and pointer-only (browsing, reading, watching media). Removing
the persistent top status bar ([ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md))
left the desktop clean, but pushed mouse-accessible system entry points onto the
Dock — bloating it with non-application controls (readings, workspace pager) and
forcing the Dock to maintain a bespoke collapsed-capsule morph state machine.

Meanwhile, traditional desktop hot corners (instant trigger on pointer touch)
are notorious for accidental activations when reaching for window controls near
screen corners.

## Decision Drivers

- **Tiered dual-channel reachability**: Core desktop workflows (launching and
  switching apps/windows, switching workspaces, per-window lifecycle, reading
  ambient status, opening Control Center, dismissing modals) must be reachable
  via keyboard alone and via pointer alone without dead ends.
- **Surface-trigger decoupling**: Chrome surfaces stay single-purpose and pure;
  input triggers (keyboard chords, touchpad gestures, screen-edge hot zones)
  map onto those surfaces without adding permanent chrome pixels.
- **Zero idle footprint**: Unapproached hot zones draw zero pixels, submit zero
  damage, and request zero frame clock ticks.
- **Component-owned geometry constraints**: Components declare which screen
  slots they can validly mount onto rather than letting the compositor hardcode
  placements or allowing geometrically impossible bindings (such as a linear
  Dock mounted to a corner).
- **Eliminate bespoke Dock collapse morphing**: Replace the Dock's custom
  collapsed stadium handle with the unified Hot Zone trigger and clean
  horizontal/vertical edge reveal.

## Considered Options

- **Keep Dock's custom collapsed capsule and add separate corner triggers.**
  Rejected: duplicates proximity, dwell, fullscreen lockout, and edge-geometry
  machinery across two systems.
- **Permanent thin edge indicators always visible on screen.** Rejected: reintroduces
  persistent chrome noise along screen edges and pays idle damage/visual clutter.
- **Unified component-mounted 8-zone Hot Zone system with proximity-revealed
  bezel-flush geometry and per-component Hover/Click trigger semantics.** Chosen.

## Decision Outcome

Chosen option: **Unified component-mounted 8-zone Hot Zone system**, because it
gives pointer users reachability to every core surface with zero idle chrome,
eliminates Dock's collapsed capsule state machine, and enforces geometric
validity at component registration and config validation time.

### 1. Eight Screen Slots

The output perimeter exposes eight mutually exclusive slots:
- **Four Edges**: `Top`, `Bottom`, `Left`, `Right` — centred along each edge,
  strictly separated from the corner spans so a pointer sweeping into a corner
  never oscillates across an edge slot.
- **Four Corners**: `TopLeft`, `TopRight`, `BottomLeft`, `BottomRight` —
  occupying a bounded span from each output vertex.

Each slot may be bound to **at most one** chrome component (`One Slot, One Owner`).
Unbound slots are completely inert and never paint.

### 2. Bezel-Flush Geometry & Continuous Curvature

- **Edge tabs**: Flush against the physical screen edge (90° right angles on the
  two vertices touching the screen boundary), with Apple-style continuous
  curvature (squircle / G2 transition) on the two interior corners facing the
  desktop.
- **Corner arcs**: Flush against the two orthogonal screen edges meeting at the
  vertex, with a smooth curved corner-transition profile facing the desktop
  interior.

### 3. Proximity-Revealed Affordance & Three-Band Separation

To prevent accidental triggers and avoid swallowing clicks on maximized window
buttons (e.g. top-right close button), each active Hot Zone separates three
geometric regions:
1. **Proximity Zone** (user-configurable pixel range, default 48 px): Pointer
   entry smoothly fades/grows the Hot Zone affordance in; leaving smoothly
   collapses it back to zero pixels. Never intercepts pointer events.
2. **Visual Affordance**: Thin bezel-flush edge tab or corner arc (6 px edge
   thickness, smooth corner arc) rendered only while reveal > 0.
3. **Trigger Hitbox**: Tightly hugged to the physical screen boundary (6 px edge
   depth, 10×10 px corner tip), leveraging Fitts's Law (screen edge as infinite
   stop) without covering interior window buttons.

### 4. Component-Declared Capabilities & Trigger Modes

Each chrome component declares a `HotZoneCapability`:
- `allowed_slots`: Bitmask of valid slots (e.g. Dock allows `Edges` only and
  adapts to horizontal on `Top`/`Bottom` and vertical on `Left`/`Right`;
  Overview allows all 8 slots; Control Center allows side edges and corners;
  Glance HUD allows `Edges` and `Corners`).
- `default_trigger`: Either `Hover` (with short intent dwell, suited to
  lightweight/peek surfaces like Dock and Glance HUD) or `Click` (press-and-release
  inside the trigger hitbox without drag, suited to focus-taking surfaces like
  Control Center and Overview). Users may override trigger mode in config.

### 5. Fullscreen & Active-Surface Suppression

- Under `SpaceUse::Fullscreen`, all eight Hot Zones are completely suppressed
  (no proximity reveal, no trigger).
- When a mounted component is already permanently visible on its edge (e.g. a
  pinned, non-autohide, unobscured Dock), its edge Hot Zone stands down.

### Invariants & Behavioral Boundaries

- `[INV-HOTZONE-01] Zero idle cost`: When the pointer is outside every bound
  slot's proximity band and all reveal animations are settled at 0, the Hot Zone
  chrome submits zero damage regions and returns `anim_pending() == false`.
- `[INV-HOTZONE-02] Mutual exclusion`: No two of the eight slots overlap in
  either their trigger hitboxes or visual footprints, and each slot binds at most
  one target component.
- `[INV-HOTZONE-03] Capability enforcement`: Binding a component to a slot
  outside its declared `allowed_slots` is rejected during configuration
  validation; invalid bindings never reach runtime rendering.
- `[INV-HOTZONE-04] Fullscreen silence`: Under `SpaceUse::Fullscreen`, all Hot
  Zones remain hidden and non-interactive regardless of pointer position.
- `[INV-HOTZONE-05] No Dock capsule morph`: The Dock has no collapsed stadium
  handle state; when hidden it has zero visual footprint and is summoned by its
  bound edge Hot Zone, revealing cleanly via slide and fade in horizontal
  (`Top`/`Bottom`) or vertical (`Left`/`Right`) orientation.

## Rejected Alternatives & Negative Knowledge

### Keeping Dock's collapsed stadium capsule alongside Hot Zones
- **Why considered**: Preserves existing `ADR-0198`/`ADR-0251` capsule morph tests.
- **Why rejected**: Leaves two competing edge-summon mechanisms, keeps a
  permanent 4 px bar on screen when idle, and preserves fragile morph geometry.

### Instant hover-only hot corners
- **Why considered**: Minimal code.
- **Why rejected**: Causes frequent accidental modal/overview triggers when
  moving toward window titlebar controls.

## Consequences

### Positive
- Pointer and keyboard users can reach every core surface cleanly without
  persistent chrome clutter.
- Dock loses its collapsed-capsule morph complexity and gains clean 4-edge
  support (`Top`, `Bottom`, `Left`, `Right`).
- Future chrome components opt into Hot Zone mounting declaratively.

### Negative / Trade-offs
- Users accustomed to seeing a permanent 4 px bottom capsule while Dock is
  autohidden will now see the bezel-flush affordance appear as the pointer
  approaches the edge instead.
