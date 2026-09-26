---
id: ADR-0166
title: "Decoupled Native Accessibility Architecture Across Compositor and Optics"
status: accepted
date: 2026-09-26
scope: architecture/compositor, accessibility, optics/rendering
superseded_by: null
negative_knowledge: true
---

# 0166. Decoupled Native Accessibility Architecture Across Compositor and Optics

- Status: Accepted
- Date: 2026-09-26
- Deciders: Tessera Maintainers & Core Architects
- Extends: [ADR-0029](0029-animation-and-effect-policy.md), [ADR-0165](0165-purify-compositor-boundary-and-decouple-application-accessibility.md)

---

## Context and Problem Statement

Following [ADR-0165](0165-purify-compositor-boundary-and-decouple-application-accessibility.md), the compositor successfully purged in-compositor application widget caching (`AccessibilityTreeRegistry`) and decoupled application-internal accessibility to the standard Linux `org.a11y.Bus`.

However, the desktop shell and the underlying graphics stack (`../optics`) still lacked a definitive, long-term, and uncompromising accessibility architecture. Historically, Linux desktop accessibility suffered from two failure modes:
1. **The Over-Reaching Compositor Trap**: The display server attempts to become a central clearinghouse for all third-party widget trees, introducing unbounded memory footprint, DoS vectors, and thread latency (purged by ADR-0165).
2. **The Opaque Shell and Insecure Totalitarian Bus Trap**: Compositors draw custom hardware-accelerated UI surfaces (Launcher, Dock, Settings, Lock screen) that remain opaque black boxes to screen readers (e.g., Orca), while the legacy AT-SPI2 D-Bus bus exposes unauthenticated broadcast channels, plaintext credential leaks, and covert global keylogging (`DeviceEventController`).

We must establish a clean-break, modern Linux/Wayland-native accessibility architecture with clear boundaries between `Tessera` (Compositor & Desktop Shell) and `Optics` (`flux` / `lens` / `prism` / `anim`).

---

## Decision Drivers

- **Strict Separation of Concerns**: Graphics pipelines and shaders belong in `optics`; system protocols, IPC transports, peer authentication, and security arbitration belong in `tessera`.
- **Pure Linux/Wayland Orientation**: Zero cross-platform abstraction overhead and zero legacy X11 shims. Embrace modern Linux kernel primitives (`zbus`, `atspi`, `SO_PEERCRED`, `pidfd`, `libei`, `memfd`).
- **Uncompromising Security Boundary**: Neutralize historical D-Bus accessibility attack vectors (peer authentication, password redaction, action focus barriers, and absolute elimination of unmediated keyloggers).
- **GPU-Accelerated Visual Assistance**: Implement visual assistive features (magnifier, color inversion, Daltonization) directly in Vulkan post-processing passes with zero IPC overhead and zero CPU readback latency.

---

## Decision Outcome

We establish a multi-tier, decoupled accessibility architecture across `Optics` and `Tessera`:

```
+─────────────────────────────────────────────────────────────────────────────+
|                                    OPTICS                                   |
|                                                                             |
|  [flux] Vulkan Post-Processing Pass                                         |
|    - Viewport Magnifier (Zoom blit with smooth focal tracking)              |
|    - Color Matrix Shaders (Inversion, High Contrast, Daltonization)         |
|                                                                             |
|  [lens] Transport-Neutral UI Semantics                                      |
|    - Semantic tree generation (Role, Rect, Focused, Disabled, Value)        |
|    - Pure memory data structures; ZERO D-Bus or IPC dependencies           |
|                                                                             |
|  [anim] Motion Vocabulary                                                   |
|    - Enforces [ui] reduced_motion (instantaneous terminal state resolution) |
+─────────────────────────────────────────────────────────────────────────────+
                                      ▲
                                      │ Render commands & semantic nodes
                                      ▼
+─────────────────────────────────────────────────────────────────────────────+
|                                   TESSERA                                   |
|                                                                             |
|  [tessera-shell] Linux-Native A11y Export                                   |
|    - Bridges lens semantic nodes to org.a11y.Bus via zbus & atspi           |
|    - Kernel Peer Authentication: Verifies SO_PEERCRED / pidfd               |
|    - Password Redaction: Role::PasswordText, zero text emitted              |
|    - Action Barriers: Remote DoAction(0) requires active physical focus     |
|                                                                             |
|  [tessera-wayland] Modern Input & Protocol Arbitration                       |
|    - Input Method & OSK: zwp_text_input_v3, zwp_input_method_v2             |
|    - Virtual Input: libei (Emulated Input Server) with window-bounded clipping|
|    - XKB Assistive Keys: Sticky Keys, Slow Keys, Bounce Keys filters        |
|                                                                             |
|  [tessera-desktop / tessera-config] Authority & Preferences                 |
|    - [accessibility] schema: zoom_factor, daltonism, high_contrast, osk     |
|    - Dynamic runtime propagation to flux shaders and input filters          |
+─────────────────────────────────────────────────────────────────────────────+
```

### 1. The Optics Responsibility Boundary

`optics` provides graphics, layout, and motion primitives without taking any OS IPC or D-Bus dependency:

- **`flux` (Visual Assistive Shader Pipeline)**:
  - **Screen Magnifier**: Implements a GPU blit pass that samples the composite swapchain texture around a normalized focal coordinate `(fx, fy)` scaled by `zoom_factor`. Sampling uses bilinear or bicubic filtering to avoid aliasing.
  - **Color Correction Shaders**: Implements a dedicated fullscreen fragment shader pass supporting:
    - RGB Inversion (`1.0 - rgb`).
    - High-contrast grayscale luminance mapping.
    - Daltonization: $3 \times 3$ color matrix multiplication tailored for Protanopia, Deuteranopia, and Tritanopia compensation.
- **`lens` (Semantic Topology Generation)**:
  - Every immediate-mode UI widget in `lens` emits structured semantic metadata: bounding rectangle (`Rect`), semantic role (`PushButton`, `TextInput`, `Slider`, `Label`, `List`), and accessibility state flags (`Focused`, `Disabled`, `Selected`).
  - `lens` remains completely decoupled from IPC protocols; it produces pure in-memory data structures consumed by the shell host.
- **`anim` (Reduced Motion Discipline)**:
  - Respects the desktop-wide `reduced_motion` policy ([ADR-0029](0029-animation-and-effect-policy.md)), clamping duration and easing transitions to resolve in $\le 1$ frame.

### 2. The Tessera Compositor & Shell Boundary

`tessera` owns session lifecycle, input mediation, desktop preferences, and the bridge to the Linux accessibility bus:

- **`tessera-shell` (Pure Linux-Native AT-SPI Bridge)**:
  - Ingests semantic node trees emitted by `lens` and translates them into AT-SPI2 interfaces via Rust `zbus` and `atspi`.
  - **Kernel Peer Authentication (`SO_PEERCRED` / `SO_PEERPIDFD`)**: Incoming D-Bus requests to inspect the shell's tree must undergo caller credential verification. Calls originating from unverified processes, arbitrary sandboxes, or unknown PIDs are rejected with `AccessDenied`.
  - **Sensitive Data Redaction**: Password entries in `tessera-lock` and sensitive dialogs expose `Role::PasswordText`. Methods on `org.a11y.atspi.Text` return empty strings, and `object:text-changed` signals omit text payloads, broadcasting only caret navigation.
  - **Action Guarding**: High-privilege actions (e.g. session unlock, administrative settings confirmation) reject remote `Action.DoAction(0)` unless validated against an active physical user focus state.
- **`tessera-wayland` (Input Accessibility & Emulation)**:
  - Implements `zwp_text_input_v3` and `zwp_input_method_v2` to support assistive on-screen keyboards (e.g., Squeekboard) and caret coordinate synchronization for the `flux` magnifier.
  - Replaces legacy AT-SPI key injection by implementing a `libei` (Emulated Input Server) endpoint, enforcing `[INV-INPUT-01]` window-scoped physical sandboxing.
  - Implements keyboard accessibility filters directly in the seat event pipeline:
    - **Sticky Keys**: Latching modifier state for single-finger input.
    - **Slow Keys**: Configurable debounce dwell threshold to filter involuntary tremors.
    - **Bounce Keys**: Minimum rejection intervals for rapid accidental key repeats.
- **`tessera-desktop` / `tessera-config` (Unified Authority)**:
  - Adds a first-class `[accessibility]` configuration module to `tessera-config` and an interactive settings tab in `tessera-shell`.
  - Governs on-demand activation: when no assistive technology or screen reader is enabled, the shell's A11y D-Bus endpoints remain dormant with zero active memory or IPC overhead.

---

## Invariants & Behavioral Boundaries

- `[INV-A11Y-02] Optics Transport Neutrality`: Crates and libraries within `optics` (`flux`, `lens`, `prism`, `anim`) MUST NOT link against D-Bus, `zbus`, or Linux IPC libraries. UI semantics emitted by `lens` must remain transport-agnostic pure Rust data structures.
- `[INV-A11Y-03] Zero Data Leak on Sensitive Fields`: Credential inputs (such as lock screen passwords and vault passphrases) MUST declare `Role::PasswordText` and suppress text payloads from both D-Bus queries and `object:text-changed` signals.
- `[INV-A11Y-04] Kernel-Authenticated A11y Peers`: The shell's A11y D-Bus adapter MUST verify caller PID and cgroup via kernel `SO_PEERCRED` or `SO_PEERPIDFD` before serving tree traversal requests.
- `[INV-A11Y-05] Hardware-Bound Shaders for Visual Aids`: Screen magnification, Daltonization, and color inversion MUST execute exclusively in the `flux` GPU post-composition pass, completely decoupled from IPC channels.
- `[INV-INPUT-02] No Global Keystroke Snooping`: The compositor MUST NOT implement or expose the AT-SPI `DeviceEventController` keystroke recording interface. Simulated input MUST use `libei` bounded by compositor capability grants.

---

## Rejected Alternatives & Negative Knowledge

### Adopting Cross-Platform Abstraction Layers (AccessKit) in Core Compositor
- **Why considered**: AccessKit provides an ergonomic, declarative Rust tree update model.
- **Why rejected**: Tessera is an explicitly Linux-first Wayland compositor tightly integrated with DRM/KMS, Vulkan, and Linux kernel primitives. Introducing a cross-platform intermediary designed for Windows UIA and macOS NSAccessibility adds unnecessary impedance mismatches, generic type indirections, and dependency churn without any platform benefit. Direct integration using `zbus` and the official `atspi` crate yields an uncompromised, zero-overhead Linux implementation.

### Implementing Visual Accessibility via CPU Pixel Manipulation or Client Shaders
- **Why considered**: Simple to prototype using software rendering or delegating to individual clients.
- **Why rejected**: Catastrophic performance and incomplete coverage. CPU pixel readback introduces multi-millisecond pipeline stalls. Client-side shaders cannot invert or magnify overlapping shell overlays, popups, or external Wayland surfaces. Visual accessibility must reside in the final composite stage of the display engine (`flux`).

### Unauthenticated Open `org.a11y.Bus` Endpoints
- **Why considered**: Standard practice in historical X11 and early Linux desktop environments.
- **Why rejected**: Unacceptable security failure. Unauthenticated endpoints allow untrusted sandboxed apps, browser scripts, or rogue background processes to scrape desktop UI text, reconstruct user activity, and bypass Wayland surface isolation.

### Exposing Legacy AT-SPI `DeviceEventController` for Input Injection
- **Why considered**: Compatible with older accessibility automation tools.
- **Why rejected**: The `DeviceEventController` interface acts as an unmediated global keylogger and arbitrary input injector. Modern Wayland security mandates `libei` with compositor-governed authentication and target-window coordinate bounding.

---

## Consequences

- Formally integrates `optics` (`flux`/`lens`) into the accessibility architecture through shader passes and transport-neutral semantic trees.
- Extends the boundary purification of [ADR-0165](0165-purify-compositor-boundary-and-decouple-application-accessibility.md) by defining exactly how the compositor's own shell surfaces become accessible to Orca without compromising security.
- Eliminates the historical D-Bus accessibility security dilemma through kernel peer authentication (`SO_PEERCRED`), password redaction, and `libei` input mediation.
- Prepares the groundwork for the roadmap M9 accessibility deliverables (`screen-reader accessibility hooks`, `magnifier`, and `accessibility preferences`).
