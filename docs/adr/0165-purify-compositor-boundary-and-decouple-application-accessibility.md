---
id: ADR-0165
title: "Purify Compositor Boundary and Decouple Application Accessibility"
status: accepted
date: 2026-09-19
scope: architecture/compositor, accessibility, agent/automation
superseded_by: null
negative_knowledge: true
---

# 0165. Purify Compositor Boundary and Decouple Application Accessibility

- Status: Accepted
- Date: 2026-09-19
- Deciders: Tessera Maintainers & Core Architects
- Amends: [ADR-0036](0036-scoped-semantic-automation.md), [ADR-0102](0102-actor-scoped-semantic-observation-and-transactional-actions.md), [ADR-0104](0104-actor-sessions-resource-grants-and-accessibility-adapter.md), [ADR-0164](0164-universal-domain-pruning-and-primitives-canonization.md)

---

## Context and Problem Statement

In early architectural phases (ADR-0036, ADR-0102), the compositor attempted to bridge the "physical gap" between AI agent automation and graphical user interfaces by inventing an in-compositor "Universal Semantic Layer":
1. Applications' internal control trees were extracted via a private sidecar daemon (`tessera-atspi`), serialized into custom IPC messages, and ingested directly into the compositor core.
2. The compositor was forced to maintain an in-memory `AccessibilityTreeRegistry`, enforce DoS quotas on internal UI trees (e.g. maximum depth 64, node count limits, 16 KB string bounds), and act as a private long-polling action broker between agents and the accessibility adapter.
3. This design created severe **Concern Contamination (关注点污染)**: the internal control tree model permeated 8 distinct crates (`tessera-protocol`, `tessera-desktop`, `tessera-authority`, `tessera-wayland`, `tessera-ipc`, `tessera-atspi`, `tessera-mcp`, and `tessera`).

In standard Linux/Wayland architectures (GNOME Mutter, KDE KWin, wlroots/Sway, COSMIC), **no display server or window manager ever caches or brokers application-internal control trees**.
- Applications expose their internal widgets through the standard **AT-SPI D-Bus bus** (`org.a11y.Bus`) or cross-platform SDKs like **AccessKit**.
- Modern AI agents (Claude Computer Use, OpenAI Operator, OSWorld) interact primarily via **high-fidelity screen capture and scoped physical coordinate input (Pointer/Keyboard)**.
- Any assistive tool or agent requiring structural accessibility can connect directly to the standard AT-SPI D-Bus bus without requiring the compositor to act as a stateful, custom intermediary.

Retaining this custom in-compositor widget pipeline constitutes unnecessary technical debt, architectural drift, and substantial cognitive friction.

## Decision Drivers

- **Compositor Purification**: A Wayland compositor's sole concern is display resource lifecycle, GPU frame composition, window geometry/placement, and scoped input mediation. It is not an application widget database.
- **Industry Standardization**: Align with Linux/freedesktop standards (AT-SPI / AccessKit) instead of proprietary in-compositor tree relay protocols.
- **Agent Evolution Reality**: Modern computer-use agents rely on high-performance pixel capture and scoped hardware input injection; the compositor should optimize for zero-copy capture and strict window-bounded input sandboxing.
- **Zero Legacy Compromise**: Eliminate custom shims, long-polling IPC brokers, and private sidecar daemons entirely.

## Decision Outcome

1. **Purify the Compositor Core**:
   - Strip `AccessibilityTreeRegistry` and in-compositor application tree validation from `tessera-desktop`, `tessera-wayland`, and `tessera`.
   - Remove custom accessibility tree publishing (`PublishAccessibilityTree`, `GetAccessibilityWindows`) and long-polling action dispatch (`AccessibilityActionRequest`, `next_accessibility_action`) from `tessera-protocol` and `tessera-ipc`.
   - Purge private sidecar process supervision for `tessera-atspi` from the compositor runtime session.

2. **Retire and Remove `tessera-atspi`**:
   - Delete `crates/tessera-atspi`. The compositor no longer ships a custom AT-SPI-to-IPC relay.
   - Applications and external assistive technologies continue to communicate over the standard freedesktop `org.a11y.Bus`.

3. **Re-center Compositor A11y & Automation Boundaries**:
   The compositor provides three legitimate, high-performance capabilities for automation and accessibility:
   - **Macroscopic Physical Geometry**: Expose window output placement (`Rect`), display coordinates, visibility, and focus state.
   - **Scoped Physical Input Mediation**: Allow authorized agents to inject pointer moves, clicks, scrolls, and keystrokes strictly bounded to the target window's surface extent.
   - **Direct Frame Capture**: Provide zero-copy and low-latency DMA-BUF / RGB readback for vision-based agents.
   - **Compositor-Level Assistive Features**: Screen magnification, color inversion, high contrast, and Daltonization shaders.

## Invariants & Behavioral Boundaries

- `[INV-A11Y-01] No Internal Widget Trees in Compositor`: The compositor MUST NOT ingest, store, or validate application-internal widget hierarchies.
- `[INV-INPUT-01] Window-Scoped Input Sandboxing`: Injected pointer and keyboard events MUST be validated against the targeted window's physical bounds and authorization grant.
- `[INV-IPC-01] No Stateful Action Long-Polling`: Compositor IPC connections MUST remain request-reply or stream-based; long-polling action distribution channels are forbidden.

## Rejected Alternatives & Negative Knowledge

### Retaining the Tree Inside `tessera-desktop`
- **Why considered**: Keeps the structural model in the workspace for potential hybrid agents.
- **Why rejected**: Perpetuates cross-crate contamination. The compositor has no legitimate reason to know whether an application contains 5 buttons or a 100-row list. It forces the compositor into handling IPC timeouts, tree version sync, and DoS validation for third-party apps.

### Wrapping AT-SPI Directly Inside the Compositor Process
- **Why considered**: Eliminates the out-of-process daemon and IPC long-polling overhead.
- **Why rejected**: Catastrophic stability and security failure. D-Bus I/O and GObject/AT-SPI event processing inside the compositor main loop would introduce unbounded latency, blocking presentation cycles, and expanding the compositor attack surface.

## Consequences

- Completely eliminates `crates/tessera-atspi` and drops workspace crate count by one.
- Eradicates over 1,500 lines of brittle long-polling, tree cache synchronization, and custom IPC protocol machinery.
- Drastically simplifies `tessera-protocol`, `tessera-ipc`, `tessera-wayland`, and `tessera` composition runtime.
- Restores clear architectural alignment with modern Wayland compositors (Mutter, KWin, COSMIC).
