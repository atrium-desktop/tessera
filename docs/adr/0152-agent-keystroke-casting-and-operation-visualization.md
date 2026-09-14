# ADR-0152: Observability-first keystroke casting and operation visualization for Agent Interaction Domains

- Status: Proposed
- Date: 2026-09-09
- Scope: `tessera-chrome`, `tessera-shell`, `tessera-compositor`, `tessera`;
  amends [ADR-0121](0121-neutral-mask-feedback-movable-mirrors-and-non-raising-agent-input.md)
  and extends [ADR-0150](0150-standard-xdg-cursors-and-unified-asset-pipeline-for-agent-feedback.md)

## Context

[ADR-0121](0121-neutral-mask-feedback-movable-mirrors-and-non-raising-agent-input.md)
introduced neutral visual feedback for Agent operations on read-only mirrors.
That decision intentionally withheld key codes from presentation state:
`AgentInputKind::Keyboard` was an opaque enum variant, and runtime translation
explicitly refrained from copying key codes into feedback activity to avoid
persisting typed contents in chrome.

In practice, this privacy safeguard severely impaired supervisory
observability. While pointer movements and clicks provide explicit visual
feedback through cursor motion and click ripples (ADR-0150), keyboard actions
by an Agent became an opaque black box:
- The observer only saw a generic "Keyboard" label in the window title bar.
- When an Agent pressed `Enter`, `Escape`, `Tab`, arrow navigation keys, or
  shortcuts (such as `Ctrl+C` or `Ctrl+S`), the observer had no indication of
  what command was executed or whether an unexpected shortcut was triggered.
- When an Agent typed text or shell commands, the observer could not verify
  what was being entered until after application-side side effects occurred.

For human supervisors overseeing autonomous desktop agents, real-time
operational transparency is essential to trust and verification. An agent
operating the desktop needs clear, immediate visual manifestation of its
keystrokes without cluttering the active document area.

## Decision

1. **Keystroke Casting HUD on the Mirror Surface:**
   - On the read-only mirror of an Agent-controlled window, Shell chrome renders
     a dedicated, transient **Keycast HUD** anchored to the **bottom-right
     corner** of the window region.
   - The HUD presents recent keystrokes as a horizontal sequence of styled
     keycap badges (using `application_surface` background, subtle borders, and
     design typography).
   - Up to 4 recent keystrokes are retained in a FIFO queue, sliding left as
     new keys are pressed.

2. **Semantic Keycode Formatting:**
   - Extend `AgentInputKind` to carry human-readable key representations:
     ```rust
     pub enum AgentInputKind {
         PointerMove,
         Click { button: u32 },
         Scroll { dx: f32, dy: f32 },
         Keyboard { key_name: String },
     }
     ```
   - Linux evdev scancodes are mapped to standardized, concise glyphs:
     - Navigation & Control: `↵ Enter`, `⎋ Esc`, `⇥ Tab`, `⌫ Backspace`,
       `␣ Space`, `↑`, `↓`, `←`, `→`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`
     - Modifiers: `Ctrl`, `Alt`, `Shift`, `Super`
     - Alphanumeric & Symbols: clean uppercase characters and punctuation.

3. **Smooth Transient Lifecycle (Fade-in & Fade-out):**
   - Each keycap badge pops in with full opacity immediately upon delivery.
   - Badges remain fully visible for a hold window (~1.2 seconds) and smoothly
     fade out over 400 ms.
   - Compositor `damage_region` and `anim_pending` tracks the bottom-right HUD
     bounds so animations render fluidly and suspend repaints when the queue
     clears.

4. **Security and Capture Invariants:**
   - The Keycast HUD is compositor-owned trusted chrome; it is never injected
     into client window buffers.
   - Directed Interaction Domain captures (`interaction_domain_capture`) render
     client surfaces directly and exclude all Shell chrome, ensuring an Agent
     cannot self-observe or become distracted by its own feedback layer.
   - When the desktop session is locked, all feedback rendering suspends.

## Alternatives

- **Retain opaque "Keyboard" indicators without key names:**
  Rejected. An opaque label provides zero actionable telemetry to a human
  supervisor monitoring an Agent's commands, navigation, or form submissions.
- **Render keystrokes floating directly at the pointer position:**
  Rejected. Anchoring keystrokes to the cursor obstructs the text insertion
  point and interferes with reading the document text beneath the cursor.
- **Log keystrokes only to the terminal or status bar:**
  Rejected. Disconnecting keystroke display from the window being operated
  forces the human supervisor to split attention across disparate screens.

## Consequences

- **Easier:**
  - Human supervisors gain instant, unambiguous visibility into Agent keyboard
    commands, shortcuts, and typed arguments.
  - The supervisor immediately detects if an Agent executes an unintended key
    combination.
- **Required Work:**
  - `tessera-chrome` updates `AgentInputKind::Keyboard` to carry `key_name: String`.
  - `tessera` runtime translates evdev codes to canonical key names in
    `agent_activities_from_applied_input`.
  - `agent_feedback.rs` maintains a bounded recent-keys queue and renders the
    Keycast HUD in the mirror's bottom-right corner with fade animations.
