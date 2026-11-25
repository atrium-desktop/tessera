---
id: ADR-0021
title: "Chrome component trait (pure core shell)"
status: accepted
date: 2026-06-18
superseded_by: null
---

# ADR-0021: Chrome component trait (pure core shell)

- Status: Accepted
- Date: 2026-06-18
- Extended by: [ADR-0132](0132-aegis-ui-composite-component-library.md) — *aegis-ui Composite Component Library and Chrome Scaffolding*
- Amended by: [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md) — *Chrome Animation Pre-Pass Synchronization and Surface Placement Invariants*
- Amended by: [ADR-0174](0174-universal-model-motion-view-chrome-architecture.md) — *Universal Model-Motion-View Architecture and Full Transit Adoption Across Shell Chrome*
- Amended by: [ADR-0176](0176-control-center-painted-canvas-beyond-backdrop-cover.md) — *Control Center Painted Canvas, Bounded Reveal, and Retired Backdrop Cover*
- Amended by: [ADR-0177](0177-dock-transit-spring-unification-and-anti-oscillation-latch.md) — *Dock Transit Spring Unification, Anti-Oscillation Dismiss Latching, and Non-Preemptible Swapchain Drain*
- Amended by: [ADR-0178](0178-optics-transit-smoothstep-unification-and-chrome-prepass-lifecycle.md) — *Optics Transit Smoothstep Unification, Chrome Prepass Lifecycle Decoupling, and Responsive Reveal Spring Tuning*
- Amended by: [ADR-0180](0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md) — *Single-Pass Chrome Fades, High-Water Backdrop Allocation Decoupling, and Dock Pre-Pass Coherence*
- Amended by: [ADR-0181](0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md) — *Canonical Chrome Pre-Pass Seam, Occlusion-Eliding Base Pass, and Deferred-Scanout Frame Ownership*
- Amended by: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) — the `Chrome` contract loses its material declarations
- Amended by: [ADR-0217](0217-idle-chrome-does-not-keep-the-frame-clock-and-hover-damage-is-bounded-by-the-component-that-owns-it.md) — the `Chrome` contract gains a hover-damage query
- Amended by: [ADR-0226](0226-single-pass-chrome-composition-preview-layering-and-modal-dock-inhibition.md) — *Single-Pass Chrome Composition, Live-Preview Thumbnail Layering, and Modal Dock Inhibition*

## Context

Through [ADR-0016](0016-shell-server-window-management-bridge.md),
[ADR-0017](0017-server-side-decorations-via-overlays.md), and
[ADR-0019](0019-dock-as-bottom-center-overlay.md), `aegis-shell::Shell::render`
had grown to hard-code three unrelated chrome surfaces — the window-list side
panel, the per-window server-side decorations, and the dock — inline in one
`Ui::frame` closure. Adding a new surface (a HUD bar, an overview, a launcher)
meant editing the same monolithic method, and the core host was coupled to the
appearance of every chrome piece it happened to ship with.

The repeated pattern across all three surfaces was identical: read the shared
per-frame window snapshot and input, draw through the `Frame`, and push a small
set of user intents (quit / focus / close / move). The capabilities the core
must own — the flux-ui device binding, the `Ui::frame` / `Ui::render` envelope,
the window snapshot, and the path to drain intents into the main loop — are
independent of *what* the chrome draws.

## Decision

### 1. Split `aegis-shell` into a core host and pluggable components

The `Shell` core keeps only the capability surface:

- the `Ui` context (flux device binding, `frame`/`render` envelope),
- the per-frame `Vec<Window>` snapshot from `Server::windows`,
- the interaction sink (`ChromeEvents`),
- a `Vec<Box<dyn Chrome>>` component registry.

It owns no chrome appearance. The previous inline surfaces move into a
`chrome/` module, each as its own `Chrome` implementation:
`WindowList`, `Decorations`, `Dock`.

### 2. A `Chrome` trait is the seam

```rust
pub struct ChromeEvents {
    pub quit: bool,
    pub clicked: Option<usize>,
    pub closed: Option<usize>,
    pub move_requested: Option<usize>,
}

pub trait Chrome {
    fn render(&mut self, frame: &mut Frame, input: &Input,
              windows: &[Window], out: &mut ChromeEvents);
}
```

A component renders itself for one frame from the shared snapshot and input,
drawing through `frame` and writing intents into the shared `out` sink. The
sink reuses the exact fields the main loop already drains, so no server API or
main-loop plumbing changes.

### 3. Registration is explicit; the core ships empty

`Shell::new` creates a host with no chrome. The binary composes it:

```rust
shell.add(Box::new(WindowList::new()));
shell.add(Box::new(Decorations::new()));
shell.add(Box::new(Dock::new()));
```

Components render once per frame in registration order. `Shell::set_windows`,
`Shell::render`, and the `take_*` accessors are unchanged, so the main loop is
unchanged.

### 4. Order and overlap

`WindowList` rides the top-left auto-layout flow; `Decorations` and `Dock` use
absolute `flux-ui` overlays (per [ADR-0017](0017-server-side-decorations-via-overlays.md)).
Overlays are keyed by id and positioned by anchor, so their draw order relative
to each other and to the flow panel does not affect output as long as their
rects do not overlap — which holds for the current set.

## Alternatives

- **Keep the chrome inline in `Shell::render`.** Rejected: every new surface
   (HUD bar, overview, launcher) re-couples the core to a specific appearance,
   and the method keeps growing. The dock ([ADR-0019](0019-dock-as-bottom-center-overlay.md))
   was the point where the pattern clearly repeated three times.
- **One crate per chrome component (`aegis-dock`, `ass-hudbar`, ...).** Deferred
   for now: the current components share the `flux-ui` dependency and have no
   independent lifecycle or state, so the cross-crate friction (each component
   would need to re-enter the core's `Ui::frame`, or the core would expose its
   `Frame`/`Input`/snapshot over a crate boundary) is not yet earned. The
   `Chrome` trait makes that promotion a near-zero-cost move later: when a
   component grows its own dependencies or state (per-app `.desktop` icon
   loading, a magnification animation loop), it can leave `aegis-shell` for its
   own crate without touching the core or its siblings. This mirrors how
   [ADR-0018](0018-wallpaper-crate.md) split `aegis-wallpaper` once it had a
   distinct dependency profile.
- **An enum of components instead of a trait.** Rejected: a closed enum forces
   every new surface to edit the core's type, re-coupling it. The open trait
   lets a component be defined next to its siblings and registered by the
   binary without core changes.
- **Generic `Shell<C: Chrome>` instead of `Box<dyn Chrome>`.** Rejected: the
   host holds a heterogeneous set of components registered at runtime by the
   binary, which a single type parameter cannot express without a tuple-type
   combinator. The per-frame vtable cost (a handful of calls) is negligible
   next to rendering.

## Consequences

- `Shell::render` no longer hard-codes any chrome; it runs every registered
  component inside one `Ui::frame` envelope and renders the result. The core
  is appearance-free.
- Adding a chrome surface is now local: a new module implementing `Chrome`,
  plus one `Shell::add` line in the binary. The core and the other components
  do not change.
- The dock from [ADR-0019](0019-dock-as-bottom-center-overlay.md), the side
  panel from [ADR-0016](0016-shell-server-window-management-bridge.md), and the
  decorations from [ADR-0017](0017-server-side-decorations-via-overlays.md)
  become peers behind the same seam; their individual decisions (placement,
  interaction model) are unchanged.
- Component state is per-struct today (`WindowList`, `Decorations`, `Dock` are
  unit structs). When a component needs per-frame-retained state (pinned apps,
  animation clocks), it moves into its struct without touching the trait.
- Promoting a component to its own crate is now cheap and localized: move the
  module out, re-export, keep the `Chrome` impl. The trigger is the same as
  ADR-0018 — the component gains a dependency or lifecycle the core should not
  own.
