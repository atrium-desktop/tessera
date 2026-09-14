# ADR-0150: Dual-track standard XDG cursor themes and unified asset pipeline for Agent feedback

- Status: Proposed
- Date: 2026-09-09
- Scope: `tessera-shell`, `tessera-compositor`, `scripts/prepare-tessera-cursors.py`, `assets/cursors`;
  amends [ADR-0121](0121-neutral-mask-feedback-movable-mirrors-and-non-raising-agent-input.md)
  and [ADR-0122](0122-original-mit-cursor-theme-replaces-bibata.md)

## Context

[ADR-0121](0121-neutral-mask-feedback-movable-mirrors-and-non-raising-agent-input.md)
established neutral mask feedback for Agent Interaction Domains: a read-only
mirror of an Agent-controlled window displays a semi-transparent scrim mask, an
Interaction Domain label, an arrow-cursor sprite, and simplified mouse sprites
highlighting pressed buttons or scroll wheels. [ADR-0122](0122-original-mit-cursor-theme-replaces-bibata.md)
subsequently replaced Bibata with original MIT-licensed vector cursor themes,
generating `tessera-user-light` / `tessera-user-dark` as full XDG cursor themes
and depositing four bespoke SVG files (`pointer.svg`, `mouse-left.svg`,
`mouse-right.svg`, `mouse-middle.svg`) under `assets/cursors/tessera-ai-*`.

In practice, this bespoke visual feedback suffers from three architectural gaps:

1. **Total loss of XDG cursor shape semantics:**
   Wayland clients emit `wp_cursor_shape_device_v1` requests or call
   `wl_pointer.set_cursor` when the pointer hovers interactive elements (e.g.
   `text` / I-beam over editors, `pointer` / hand over hyperlinks, `col-resize`
   over splitters, or `wait` during heavy computation). In the current runtime,
   the compositor records the shape on the Agent's seat, but `AgentFeedback`
   discards it and unconditionally paints the single static arrow sprite. The
   human observer (and any observation channel) receives zero contextual shape
   feedback from the underlying application.
2. **Skeuomorphic hardware sprites violate modern desktop conventions:**
   Representing clicks by drawing a literal physical mouse casing with tinted
   buttons is an ad-hoc screen-recording artifact. Modern Wayland desktop
   standards represent clicks and active states through application-level state
   transitions and minimal, non-skeuomorphic visual pulses (ripples), not by
   drawing miniature peripheral hardware on top of the content.
3. **Fragmented asset pipeline and hardcoded sprite loading:**
   While the compositor and shell manage standard cursor themes via
   `CursorCache` and vector icon infrastructure using `resvg`/`tiny-skia`,
   `agent_feedback.rs` loads its four bespoke SVGs via `include_str!` arrays
   and performs independent GPU texture uploads, creating an uncoordinated
   rendering silo.

Crucially, **the physical user and the AI Agent must never share identical
visual cursor appearance**. The physical user operates on `HUMAN_SEAT` with a
dedicated KMS hardware cursor plane; an Agent operates within an isolated
`InteractionDomain` on a read-only mirror. If an Agent's cursor were
visually indistinguishable from the human's cursor, the physical user would
experience immediate cognitive friction and ambiguity over who controls the
pointer. Visual distinction and strict seat isolation are indispensable
invariants.

## Decision

1. **Promote `tessera-ai` to a full, first-class XDG cursor theme:**
   - Expand `tessera-ai-light` and `tessera-ai-dark` from 4 ad-hoc files into
     full XDG cursor themes containing the complete suite of 36+ standard
     `wp_cursor_shape` shapes and legacy aliases (`default`, `pointer`,
     `text`, `crosshair`, `wait`, `grab`, resize directions, etc.).
   - The generator `scripts/prepare-tessera-cursors.py` generates both
     `tessera-user-*` (solid, classical human pointer silhouettes) and
     `tessera-ai-*` (distinct geometric silhouette featuring the split-tail
     open fork and negative-space styling) from shared vector geometry.
   - The user immediately distinguishes the AI cursor from their own physical
     pointer by its distinct shape language across all standard cursor roles.

2. **Unify the cursor and vector asset rendering pipeline:**
   - `AgentFeedback` integrates with the compositor's unified vector cursor
     pipeline (`CursorCache`).
   - The compositor exposes the active `cursor_shape` of the Agent seat
     associated with each Interaction Domain.
   - When projecting pointer feedback onto the mirror window, `AgentFeedback`
     requests the appropriate shape raster directly from the unified cache
     according to current shell appearance polarity (light/dark).

3. **Retain strict seat isolation and non-interfering interaction:**
   - Physical user input and Agent synthetic input remain completely disjoint.
   - The human seat controls hardware plane presentation and desktop focus.
   - Agent synthetic input targets only authorized windows in its own
     `InteractionDomain` and seat, without raising windows, taking human
     keyboard focus, or perturbing the physical cursor position.

4. **Deprecate bespoke mouse-casing sprites in favor of geometric feedback:**
   - Remove `mouse-left.svg`, `mouse-right.svg`, and `mouse-middle.svg`.
   - Click feedback transitions to a minimal, non-skeuomorphic ripple pulse;
     scroll feedback transitions to directional delta indicators.
   - The neutral window-level scrim mask and the Interaction Domain badge
     label on read-only mirrors are preserved to clearly demarcate windows
     under external agent control.

## Alternatives

- **Use the human `tessera-user` cursor theme for both Human and AI:**
  Rejected. Observers and users need immediate visual certainty whether an
  on-screen cursor is their physical mouse or an autonomous Agent. Identical
  appearance creates operational alarm and violates user expectation.
- **Retain the 4 ad-hoc SVG sprites and only add mouse shapes:**
  Rejected. Augmenting a bespoke sprite sheet perpetuates an out-of-spec
  silo and fails to interoperate with the XDG cursor shape protocol.
- **Implement Agent visual feedback as an unprivileged client overlay:**
  Rejected. Privileged feedback must remain compositor-owned and verified;
  an external client overlay would allow spoofing and create race conditions
  with window movement and mirror projections.

## Consequences

- **Easier:**
  - AI operations dynamically express standard cursor states (e.g. text selection,
    waiting, hand hover, edge resize) as requested by Wayland applications.
  - The codebase eliminates redundant embedded SVG strings and bespoke sprite
    upload logic in `tessera-shell`.
  - All cursor assets reside in standard XDG `cursors/` directories accompanied
    by standard `index.theme` files.
- **Harder / Follow-up Work:**
  - `scripts/prepare-tessera-cursors.py` must be expanded to generate all standard
    cursor shapes for the `tessera-ai` family using the distinct open-fork
    silhouette.
  - Compositor iteration and activity reporting must plumb the Agent seat's
    current `cursor_shape` into `AgentActivity` / `AgentFeedback`.
  - `AgentFeedback` rendering logic must be refactored to sample the active shape
    from the cursor cache and render click ripples instead of mouse sprites.
