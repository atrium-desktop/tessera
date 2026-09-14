# ADR-0153: Continuous synthetic input primitives for drag-and-drop and modifier chording

- Status: Proposed
- Date: 2026-09-09
- Scope: `tessera-model`, `tessera-compositor`, `tessera-chrome`, `tessera-shell`, `tessera-mcp`, `tessera`;
  amends [ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md)
  and [ADR-0125](0125-ipc-primitive-families-and-shared-ipc-client.md)

## Context

Prior to this decision, synthetic input in Agent Interaction Domains was
restricted to discrete atomic operations:
- `PointerMove { position }`
- `Click { position, button }` (atomic move, press, release)
- `Scroll { position, dx, dy }`
- `KeyPress { code }` (atomic press, release)

The compositor also enforced rigid pre-condition guards in
`prepare_synthetic_input_for_seat`:
```rust
runtime.implicit_grab_active || runtime.depressed_mods != Mods::NONE => reject
```
This atomic-only model produced severe functional deficiencies for autonomous
agents:
1. **Inability to perform drag operations:**
   An Agent could not drag to select text across lines, drag range sliders,
   reorder list items, draw on a canvas, or initiate Wayland drag-and-drop
   (`wl_data_device`, `xdg_toplevel_drag`).
2. **Inability to perform modifier combinations (Chords):**
   An Agent could not perform `Ctrl + Click` (multi-select), `Shift + Click`
   (range select), or modifier shortcuts (`Ctrl+C`, `Ctrl+V`, `Ctrl+Z`)
   because modifier keys could not be held across actions, and residual
   modifiers failed the validation gate.

A modern AI Agent operating as a first-class desktop actor requires the same
continuous input capability as a human: the ability to press a button, move the
pointer to drag, release the button, and hold modifier keys while interacting.

## Decision

1. **Extend `SyntheticInputAction` with Continuous Primitives:**
   Add explicit button and key state actions to `tessera_model::input::SyntheticInputAction`:
   - `PointerButton { position: Option<Point>, button: u32, state: ButtonState }`
   - `Key { code: u32, state: ButtonState }`
   Retain `Click` and `KeyPress` as ergonomic single-step atomic conveniences.

2. **Permit Cross-Action Drag and Modifiers within the Same Window:**
   In `prepare_synthetic_input_for_seat`:
   - Remove the blanket rejection for `runtime.depressed_mods != Mods::NONE`.
     An Agent is permitted to depress modifier keys and hold them across
     subsequent actions.
   - If an implicit pointer grab is active (`runtime.implicit_grab_active`),
     permit subsequent actions provided they target the same window that holds
     the grab (`runtime.implicit_grab_surface == window_surface`).
   - When an Interaction Domain is revoked, reset, or disconnected, the
     compositor automatically releases all depressed keys and active grabs,
     preventing orphaned state.

3. **Expose Primitives in `tessera-mcp`:**
   Extend `interaction_domain_input` tool schema and deserialization with:
   - `pointer_button`: `{ x, y, button, state: "pressed" | "released" }`
   - `key`: `{ code, state: "pressed" | "released" }`

4. **Visual Feedback for Drag and Key Chords:**
   - In `tessera-shell::AgentFeedback`:
     - When a pointer button is pressed, the click ripple activates;
     - The Keycast HUD displays depressed modifier keys alongside subsequent
       keys (e.g. `Ctrl + C`, `Shift + Click`).

## Alternatives

- **Bundle entire drag operations into a single giant `Drag { from, to }` primitive:**
  Rejected. A high-level canned drag cannot handle non-linear paths, dynamic
  hover pauses, or application-specific drag-and-drop protocol steps.
- **Maintain a separate privileged virtual input device driver:**
  Rejected. Synthesizing input directly through the Interaction Domain's
  logical seat preserves capability checks, audit journaling, and target window
  authorization without giving the Agent raw system evdev privileges.

## Consequences

- **Easier:**
  - AI Agents can select text, drag sliders, move UI elements, and execute
    standard modifier chords (`Ctrl+Click`, `Ctrl+C`).
  - Wayland `wl_data_device` drag-and-drop works out of the box.
- **Invariants:**
  - An Agent's active drag grab cannot escape its authorized window boundary.
  - Revocation or disconnect cleans up all pressed buttons and modifier keys.
