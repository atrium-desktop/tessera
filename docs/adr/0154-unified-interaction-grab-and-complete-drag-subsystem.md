# ADR-0154: Unified interaction grab architecture, touch drag-and-drop, and xdg-toplevel-drag

- Status: Proposed
- Date: 2026-09-10
- Scope: `tessera-compositor`, `tessera-model`, `tessera-wayland-protocols`;
  amends [ADR-0009](0009-input-pipeline-and-pointer-focus.md),
  [ADR-0013](0013-interactive-move-and-resize.md),
  and [ADR-0153](0153-continuous-synthetic-input-primitives-and-drag-support.md)

## Context

Prior to this decision, the compositor's input pipeline and drag-and-drop subsystem
suffered from three fundamental architectural limitations:

1. **Pointer-centric grab bias:**
   The compositor tracked implicit grabs exclusively through `implicit_grab_active`
   and `last_button_serial`, tied to pointer button presses (`BTN_LEFT`, etc.).
   Although `wl_touch` existed on the seat, `wl_touch.down` events were not
   recognized as establishing an implicit grab, nor did they populate a valid
   origin serial for `wl_data_device.start_drag`. Touch motions and touch releases
   bypassed DnD focus resolution (`update_drag_focus`) and drop execution (`finish_drag`).
   Consequently, touchscreens and stylus devices could not initiate or complete
   Wayland standard drag-and-drop operations.

2. **Fragmented drag state machines:**
   Dragging operations were represented across disconnected state fields:
   - `state.drag`: Wayland data transfer (`wl_data_device` v1–v3);
   - `state.interactive`: Window move/resize (`xdg_toplevel.move/resize`, Super+drag);
   - `state.pending_top_border_double_click`: Window top-border drag hysteresis;
   - Chrome drag state machines in `tessera-dock` and `tessera-shell`.
   Interactions between these states relied on opportunistic checks scattered
   across input routing paths, lacking a clean preemption and cleanup contract.

3. **Absence of `xdg-toplevel-drag-v1`:**
   Modern graphical environments require tab detachment and cross-window tab
   transfer (e.g. Chromium, Firefox, terminal emulators, IDEs). In Wayland, this
   is governed by `xdg-toplevel-drag-v1`. Without this protocol, clients either
   fall back to clunky client-internal drag overlays or cannot detach tabs into
   independent windows at all.

## Decision

We clean-break from the pointer-centric grab model and establish a unified,
first-class interaction grab architecture with full Touch DnD parity and
`xdg-toplevel-drag-v1` support:

### 1. Unified Seat Grab Architecture (`InteractionGrab`)
Replace disparate grab flags with an explicit, type-safe interaction grab model:
- Every seat maintains at most one active grab at a time (`Single Active Grab Invariant`).
- Grabs track the originating device (`Pointer` or `Touch(id)`), the origin surface,
  the trigger serial, and the payload.
- Session lock, client surface destruction, or compositor panic cancels the active
  grab atomically across all protocol channels.

### 2. Touch & Pointer Dual-Origin DnD Parity
- Extend `wl_data_device.start_drag` validation to accept matching serials from
  either `wl_pointer.button` or `wl_touch.down`.
- Route `wl_touch.motion` through `update_drag_focus` and `wl_touch.up` through
  `finish_drag`, giving touchscreen users the exact same data transfer capabilities
  as mouse users.
- Manage drag icon surfaces consistently regardless of whether the initiating device
  is a pointer or touch contact.

### 3. Implement `xdg-toplevel-drag-v1` Protocol
- Advertise `xdg_toplevel_drag_manager_v1` global.
- Support `get_toplevel_drag` binding to an active `wl_data_source`.
- Coordinate the detached toplevel surface as a live dragged window placeholder,
  supporting seamless transition to a fully mapped toplevel upon drop release.

### 4. Continuous Input Parity for Synthetic Agents (ADR-0153)
- Synthetic input from Agent Interaction Domains shares the exact same grab
  validation and lifecycle pathways as physical inputs, ensuring agents can
  perform both pointer and touch drags.

## Invariants & Behavioral Boundaries

- `[INV-GRAB-01] Exclusive Seat Grab`: Exactly one interaction grab may be active
  per seat at any instant. A new grab request while one is active cancels the
  prior grab before establishing the new one.
- `[INV-DND-01] Dual-Origin Parity`: Any serial originating from either a valid
  pointer press or a primary touch down on the client's focused surface is eligible
  to initiate a drag.
- `[INV-DND-02] Fail-Closed Cleanup`: Upon session lock, client disconnection, or
  grab surface unmap, active drag sessions immediately emit `wl_data_source.cancelled`
  and clear drag icon roles.

## Alternatives

- **Ad-hoc Touch Serial Hack:**
  Storing `last_touch_serial` alongside `last_button_serial` and adding `if` branches
  in `pointer.rs`. Rejected: leaves the fragmented grab state intact, perpetuates
  deadlock risks, and does not resolve touch motion/release event routing.
- **Client-Side Simulated Window Moves for Tab Detach:**
  Relying on clients to destroy tabs and spawn new floating windows at cursor coordinates.
  Rejected: produces visible flicker, loses smooth drag motion, and violates the
  Wayland desktop contract.

## Consequences

- Touchscreen and tablet users gain full Wayland drag-and-drop parity with mouse users.
- Applications utilizing `xdg-toplevel-drag-v1` can detach tabs smoothly into new windows.
- Compositor input routing is simplified into a deterministic grab state machine.
