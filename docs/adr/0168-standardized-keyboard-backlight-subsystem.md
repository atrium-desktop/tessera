---
id: ADR-0168
title: "Standardized Keyboard Backlight Subsystem and Hardware-Agnostic Controls"
status: accepted
date: 2026-09-28
scope: live system, host system, hardware abstraction, control center
superseded_by: null
negative_knowledge: true
---

# 0168. Standardized Keyboard Backlight Subsystem and Hardware-Agnostic Controls

- Status: Accepted
- Date: 2026-09-28
- Amended: 2026-09-28 — §3 replaced the continuous fader with a tiered segmented control; see "Amendments" below.
- Deciders: Tessera Maintainers & Core Architects
- Extends: [ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md), [ADR-0167](0167-wireless-known-networks-and-credential-lifecycle.md)

---

## Context and Problem Statement

Portable laptops and mobile workstations frequently incorporate illuminated keyboard backlights. However, desktop environments historically suffered from fragmentation and unmaintainable vendor-specific workarounds (e.g. ad-hoc ACPI drivers for ThinkPad, ASUS ROG, Apple SMC, or Dell WMI).

Within our architecture:
1. `SystemStatus` provided screen backlight levels (`brightness`) but possessed no domain model or telemetry for keyboard illumination (`kbd_brightness`).
2. Quick Controls in the Control Center lacked direct, hardware-agnostic toggling or slider adjustments for keyboard backlights.
3. The newly unified `HostSystem` abstraction required a clean contract to mutate and probe keyboard backlights without leaking vendor assumptions into the display server.

We must establish a pure, hardware-agnostic, and standards-only keyboard backlight subsystem.

---

## Decision

We introduce keyboard backlight domain models, extend the `HostSystem` contract, and render adaptive Quick Controls in the Control Center based exclusively on established Linux kernel and FreeDesktop specifications.

### 1. Hardware-Agnostic Protocol and Domain Model (`tessera-desktop`)

We treat keyboard backlights as standard optional system hardware:
- Extend `SystemStatus`:
  - `kbd_brightness: Option<u8>`: Normalized 0..=100% illumination level. A value of `None` indicates the machine lacks keyboard illumination or that the service is unavailable.
  - `kbd_brightness_levels: Option<u8>` (amended 2026-09-28): the number of distinct illumination steps the hardware exposes, including "off", or `None` when unknown/effectively continuous. See the amendment below.
- Extend `SystemAction`:
  - `SetKeyboardBrightness { level: u8 }`: Continuous or percentage-based mutation (`level <= 100` enforced by `validate()`).
  - `StepKeyboardBrightness`: Discrete cycling through the hardware's illumination tiers (standard laptop behavior: `0% -> 33% -> 67% -> 100% -> 0%`), advancing the device's own ladder when one is reported.

### 2. Standards-Bound Subsystem Contracts (`tessera`)

We expand the `HostSystem` trait:
```rust
pub trait HostSystem: Send + Sync {
    // ...
    fn set_keyboard_brightness(&self, level: u8) -> Result<(), String>;
    fn step_keyboard_brightness(&self) -> Result<(), String>;
}
```

- **Standards Alignment**:
  1. **Linux Kernel LED Class (`/sys/class/leds/*::kbd_backlight/`)**:
     Per Linux kernel documentation (`Documentation/leds/leds-class.rst`), all compliant laptop drivers (ThinkPad, Asus, Dell, Apple, Clevo) register under the standardized `*::kbd_backlight` pattern.
  2. **FreeDesktop UPower D-Bus Specification (`org.freedesktop.UPower.KbdBacklight`)**:
     The system respects the desktop privilege model by interacting with standard UPower endpoints or standard `brightnessctl --device='*::kbd_backlight'`.
- **`LiveHostSystem` Implementation**:
  - Probes `/sys/class/leds/*::kbd_backlight/` to detect presence, current level, and step count (`max_brightness + 1`). If no matching nodes exist (e.g., standard desktop PCs), reports `kbd_brightness: None`.
  - Dispatches mutations via standard toolchains without vendor-specific ACPI branching.
- **`MockHostSystem` Implementation**:
  - Encapsulates deterministic in-memory state: seeds `kbd_brightness: Some(66)`.
  - Mutates `kbd_brightness` and cycles tiers cleanly in memory without subprocess side effects.

### 3. Adaptive Control Center Presentation (`tessera-shell`)

In `render_quick_controls_section`:
- **Conditional Visibility**: If `status.kbd_brightness == None`, the keyboard illumination control is entirely omitted from presentation, keeping desktop PC layouts minimal and uncluttered.
- **Tiered Selection, not a Fader**: If `status.kbd_brightness == Some(val)`, a **tiered segmented control** renders alongside the display-brightness and audio-volume faders (amended 2026-09-28). Keyboard backlights are stepped hardware, so an analogue fader over-promises precision the device cannot deliver and misrepresents a discrete state as a continuous one. The control renders one segment per rung of the standard ladder `0% → 33% → 66% → 100%`, marks the nearest rung to the reported level as active, and slides a single spring-driven highlight between segments. Selecting a segment dispatches `SetKeyboardBrightness { level }` with that rung's exact level. The `StepKeyboardBrightness` action remains part of the domain contract for keybinding and CLI paths, but the Quick Controls surface no longer cycles tiers by clicking the icon.
- **Shared Tiered-Control Widget**: The selector is `tessera-shell::widgets::tiered` — a reusable composite with caller-owned spring state advanced through the single motion seam (`widgets::motion`, `[INV-ARCH-49]`). It is deliberately a shell userland widget, not an Optics primitive: Optics already ships the static form (`lens_segmented_control`), and Optics ADR-0082 (compound widgets belong in userland), ADR-0061 (springs/slides are caller-owned "flavor"), and ADR-0077 (lens must not depend on `transit`) exclude the animated form from the library.
- **Keyboard-Backlight Glyph**: The row icon is a keyboard outline with three illumination rays, registered at runtime via `lens_icon_register_svg` (`tessera-shell::widgets::icons`). It replaces the previously used `LENS_ICON_EDIT`, whose pencil-in-a-box reads as "edit" rather than "illumination"; the built-in `LENS_ICON_KEY` is the fallback if registration fails.
- **Row Sizing**: The Quick Controls Bento grid sizes rows to their content rather than forcing a uniform `row_height`. The fader/tier card declares `24 (title) + 8 (gap) + 20 (trough/selector) + 2×10 (pad) = 72px`; a uniform row height previously clamped the taller fader rows and pushed their trough into the card's bottom rim.


---

## Consequences

### Positive
- Zero vendor-specific driver code in the compositor or shell; 100% compliant with standard Linux kernel drivers.
- Automatic adaptation: laptops display the control automatically; desktops and VMs omit it with zero configuration.
- Completely testable in CI and preview mode via `MockHostSystem`.

### Negative / Trade-offs
- Laptops running out-of-tree or broken proprietary kernel modules that do not register standard `*::kbd_backlight` LED nodes will read as `None` until their kernel drivers conform.
- A device whose granularity is fine-grained or unreported (`kbd_brightness_levels == None`) falls back to a continuous fader. That is the honest control for such hardware, but it means the *presence* of a stepped selector depends on the kernel reporting `max_brightness`; a driver that omits it will not get a stepped control even if the device is physically stepped.

---

## Amendments

### 2026-09-28 — Tiered selector replaces the continuous fader (§3)

The original §3 rendered the keyboard backlight as a horizontal fader whose icon click cycled tiers. Review found the fader the wrong instrument: keyboard backlights are stepped hardware, so a free-dragging trough over-promises precision and presents a discrete state as analogue. §3 now renders a tiered segmented control (`tessera-shell::widgets::tiered`) whose segments are the hardware's real rungs and whose highlight is spring-driven. The `StepKeyboardBrightness` action is retained in the domain contract (keybindings, CLI) but is no longer produced by the Quick Controls surface. The static form of the control already exists upstream as `lens_segmented_control`; the animated form stays in shell userland per Optics ADR-0082/0061/0077.

The selector supports both a click (press and release on one rung) and a drag (press, scrub across rungs, release): the highlight follows the pointer through one under-damped spring, and a release off the control cancels the preview, exactly as a slider released off its track. The *feel* is physical; the *look* stays flat, because this panel's design language (`control_center/mod.rs`) explicitly refuses skeuomorphic materials and liquid glass.

The spring is tuned under-damped (`ζ = 0.72`, `ω₀² = 380`) so the highlight overshoots its target by ≈4% and settles in ≈0.29 s. This corrects a long-standing defect: the indicator had been tuned critically damped (`ζ = 1.0`), first via a `damping = 22.0` constant that the integrator clamped to `1.0` and then via an explicit `1.0`, so the "light bounce" the surrounding comments and this ADR promised was mathematically impossible. A regression test now asserts the indicator crosses its target before settling.

### 2026-09-28 — The tier count comes from the hardware, not a fixed ladder (§1, §3)

The first cut of the amendment rendered a fixed four-rung ladder (`0/33/66/100`) for every device, because `SystemStatus.kbd_brightness: Option<u8>` discarded the granularity the kernel actually exposes. That was dishonest: a ThinkPad with three illumination steps and a fine-grained PWM dimmer with 255 steps would both be shown as a four-segment selector.

`SystemStatus` therefore gains `kbd_brightness_levels: Option<u8>` — the number of *distinct* illumination steps including "off", probed as `max_brightness + 1` from `/sys/class/leds/*::kbd_backlight/`. This is an additive field with `serde(default)` (ADR-0140 precedent), so no protocol bump is required: a peer that predates it deserializes the same status without the key. The probe is the single authority that keeps the pairing honest — it never reports a step count without a level.

Presentation then follows the hardware's real granularity through one policy, `SystemStatus::kbd_brightness_tiers()`:

- **Stepped hardware** (`2..=5` steps, covering the real 2–4-rung devices with headroom) yields the exact evenly-spaced ladder, endpoints pinned, using the same rounding the host uses to normalize a raw reading — so a derived rung always equals the percentage the hardware reports.
- **Fine-grained or unknown granularity** (e.g. `max_brightness = 255`) yields `None`, and chrome falls back to a continuous fader, which is the honest control for a dimmer.
- **No backlight** (`kbd_brightness == None`) omits the row entirely.

The tier-stepping action also advances the hardware's own ladder when it is stepped, falling back to the documented `KBD_BRIGHTNESS_FALLBACK_TIERS` only for fine-grained/unknown hardware.

---

## Rejected Alternatives & Negative Knowledge

- **Vendor-Specific ACPI / WMI Detection**:
  - *Rejected*: Hardcoding vendor checks (e.g. matching `/sys/devices/platform/asus-nb-wmi` or `/sys/devices/platform/thinkpad_acpi`) creates exponential maintenance burden. The display server only communicates with standard kernel interfaces.
- **Always Displaying an Inactive Fader on Desktop PCs**:
  - *Rejected*: Displaying disabled controls for hardware that physically does not exist clutters mobile and desktop interfaces. Conditional visibility is the correct pattern.
- **Keeping the Continuous Fader for the Backlight (amendment)**:
  - *Rejected*: A continuous control implies a continuous value. The device exposes a handful of illumination steps; a fader that appears to sweep 0–100% is a false affordance, and dragging to an unsupported percentage silently quantizes on the hardware side.
- **Placing the Animated Selector in Optics `lens`**:
  - *Rejected*: `lens_segmented_control` already provides the static composite, and Optics ADR-0082 sends compound widgets to userland while ADR-0061 classifies springs/slides as caller-owned flavor. A spring-driven indicator inside `lens` would also force `lens` to depend on `transit`, the dependency arrow ADR-0077 forbids. The animated form belongs in shell userland.
- **A Fixed Four-Rung Ladder for Every Device**:
  - *Rejected*: It is the same false affordance as the fader, merely quantized. A three-rung backlight would display a rung it cannot reach, and a 255-step dimmer would be shown as four segments. The rung count must come from `max_brightness`.
- **Widening the Model by Replacing `kbd_brightness` with a Struct**:
  - *Rejected*: Renaming or restructuring the field would be a protocol break for a field already on the wire (`Response::SystemStatus`). The additive `kbd_brightness_levels` companion keeps every existing peer working and follows the ADR-0140 precedent for evolving `SystemStatus`.
- **Skeuomorphic Drag Control (textured knob, metal track)**:
  - *Rejected*: The command panel's documented design language is an opaque, scheme-adaptive canvas with solid elevated surfaces; it explicitly does not request skeuomorphic materials or liquid glass. "Physical feel" is delivered by spring physics (an under-damped sliding highlight), not by surface texture. Skeuomorphism here would contradict the panel's own charter for a purely decorative gain.
- **Per-Device Icon Vocabulary for the Session Buttons**:
  - *Rejected*: `Shield` (lock) and `Zap` (power off) were replaced by the built-in `LENS_ICON_LOCK` and `LENS_ICON_POWER`. These are universal, self-describing glyphs; inventing project-specific lock/power art would fragment the vocabulary for no benefit.

