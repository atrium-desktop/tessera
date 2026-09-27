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
- Extend `SystemAction`:
  - `SetKeyboardBrightness { level: u8 }`: Continuous or percentage-based mutation (`level <= 100` enforced by `validate()`).
  - `StepKeyboardBrightness`: Discrete cycling through hardware illumination tiers (standard laptop behavior: `0% -> 33% -> 66% -> 100% -> 0%`).

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
  - Probes `/sys/class/leds/*::kbd_backlight/` to detect presence and current level. If no matching nodes exist (e.g., standard desktop PCs), reports `kbd_brightness: None`.
  - Dispatches mutations via standard toolchains without vendor-specific ACPI branching.
- **`MockHostSystem` Implementation**:
  - Encapsulates deterministic in-memory state: seeds `kbd_brightness: Some(66)`.
  - Mutates `kbd_brightness` and cycles tiers cleanly in memory without subprocess side effects.

### 3. Adaptive Control Center Presentation (`tessera-shell`)

In `render_quick_controls_section`:
- **Conditional Visibility**: If `status.kbd_brightness == None`, the keyboard illumination fader is entirely omitted from presentation, keeping desktop PC layouts minimal and uncluttered.
- **Interactive Stepping and Fading**: If `status.kbd_brightness == Some(val)`, a horizontal fader renders alongside display brightness and audio volume. Clicking the icon cycles tiers (`StepKeyboardBrightness`), while dragging the slider sets exact percentages (`SetKeyboardBrightness`).

---

## Consequences

### Positive
- Zero vendor-specific driver code in the compositor or shell; 100% compliant with standard Linux kernel drivers.
- Automatic adaptation: laptops display the control automatically; desktops and VMs omit it with zero configuration.
- Completely testable in CI and preview mode via `MockHostSystem`.

### Negative / Trade-offs
- Laptops running out-of-tree or broken proprietary kernel modules that do not register standard `*::kbd_backlight` LED nodes will read as `None` until their kernel drivers conform.

---

## Rejected Alternatives & Negative Knowledge

- **Vendor-Specific ACPI / WMI Detection**:
  - *Rejected*: Hardcoding vendor checks (e.g. matching `/sys/devices/platform/asus-nb-wmi` or `/sys/devices/platform/thinkpad_acpi`) creates exponential maintenance burden. The display server only communicates with standard kernel interfaces.
- **Always Displaying an Inactive Fader on Desktop PCs**:
  - *Rejected*: Displaying disabled controls for hardware that physically does not exist clutters mobile and desktop interfaces. Conditional visibility is the correct pattern.
