---
id: ADR-0252
title: "Chrome Materials Are One Swappable, Shell-Wide Choice Capped by Ceilings"
status: accepted
date: 2026-11-25
scope: design, shell, config, settings, rendering
superseded_by: null
negative_knowledge: true
---

# 0252. Chrome Materials Are One Swappable, Shell-Wide Choice Capped by Ceilings

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) (a chrome body is no longer required to be opaque; a backdrop-sampling material may return behind its own record, and only while it is the effective material)
- Amends: [ADR-0083](0083-frameless-transient-toasts-and-hud-consolidation.md) (notification toasts gain a body in the shell's material instead of floating as bare text)
- Amended by: [ADR-0253](0253-a-backdrop-capture-beneath-chrome-draws-the-glass-materials.md) — the backdrop capture lands; `frosted` and `liquid-glass` are drawn as named and keep their own body tint

---

## Context and Problem Statement

[ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) removed the
analytic-glass material and made every chrome body an opaque painted fill. It
measured the reason: the backdrop pipeline dominated steady-state GPU time and
most resident VRAM. That finding stands. What it settled was the cost of
*always* running glass, with no way to turn it off.

It left two gaps:

- **The product has one look and it is not a choice.** A user on mains power
  with a capable GPU cannot have glass at any price, and the one look it does
  have, a slightly translucent navy (alpha 236–250), was neither opaque nor
  usefully translucent.
- **Surfaces disagree.** The dock, the Control Center, the battery alert and
  the system OSD all paint a `SurfaceRole` body, but notification toasts were
  bare floating text (ADR-0083). The desktop had no single answer to "what
  is chrome made of".

ADR-0213 rejected "keep the material, add a low-power mode" because the
expensive look stayed the default and the whole capture path stayed compiled
in. This record keeps that rejection's substance (the default is the cheapest
material, and nothing samples the backdrop unless the effective material
needs it) but makes the material a user choice.

## Decision Drivers

- **One material for the whole shell.** Surfaces that disagree about what they
  are made of read as broken, not layered.
- **The default costs what ADR-0213 bought.** Opaque is the default and draws
  exactly one fill per body.
- **Automatic rules only make things cheaper.** A power or accessibility rule
  must never raise the cost above what the user chose.
- **A preference the renderer cannot honour says so.** It must not silently
  differ from what Settings shows.

## Considered Options

- **Per-component materials.** Rejected: a frosted dock beside an opaque
  Control Center is the disagreement this record exists to remove.
- **Material as a theme (a new `ColorScheme`-like axis multiplied into
  palettes).** Rejected: a material changes body alpha and, for glass, a
  compositor pass. Ink, geometry and every non-body token are shared, so
  multiplying palettes would duplicate everything a material does not touch.
- **A preference capped by ordered ceilings.** Chosen.

## Decision Outcome

Chosen option: "a preference capped by ordered ceilings", because it keeps the
manual choice and every future automatic rule in one total order. It also
makes "automatic rules only make things cheaper" a property of `min`, so no
reviewer has to enforce it by eye.

### The vocabulary

`tessera_desktop::settings::Material` has four variants, declared cheapest
first. The derived `Ord` *is* the cost order:

| Material | Config spelling | What is drawn | Samples backdrop |
| --- | --- | --- | --- |
| `Opaque` (default) | `opaque` | One neutral fill at alpha 255 | no |
| `Translucent` | `translucent` | One alpha-blended fill; content shows through unblurred | no |
| `Frosted` | `frosted` | Blurred, tinted capture beneath the body | yes |
| `LiquidGlass` | `liquid-glass` | Refracting lens over a full-resolution capture | yes |

It is configured as `[appearance] material` and edited in Settings →
Appearance next to Contrast.

### The switching policy

`resolve_material(preferred, ceilings)` returns the preference lowered to
the lowest `MaterialCeiling` in force, plus the reason that decided it. A
ceiling is `{ max, reason }`. The ceilings are:

- **`Renderer`**: the most expensive material the compositor can draw
  (`RENDERER_MATERIAL_CEILING`). This was `Translucent` until
  [ADR-0253](0253-a-backdrop-capture-beneath-chrome-draws-the-glass-materials.md)
  landed the backdrop capture; it is now `LiquidGlass`. A capture that fails
  at run time demotes the session to `Translucent` and reports this reason.
- **`HighContrast`**: `Opaque` whenever `[appearance] contrast = "high"` or
  `[accessibility.visual] high_contrast` is on. Content must not show through
  a surface for a user who asked for contrast.
- **Power rules**, from `[appearance.material_power]`
  (`MaterialPowerPolicy`), each naming the most expensive material drawn in
  that state:
  - `OnBattery`, while a system battery discharges with no mains or USB
    supply online. Default `liquid-glass`, which is no limit. Settings →
    Appearance edits it.
  - `LowBattery`, while on battery at or below `low_battery_percent`
    (default 20, the first default `[battery] warn_at` line). Default
    `translucent`: a draining laptop stops paying for a capture.
  - `PowerSaver`, while the firmware platform profile is `low-power`
    (power-profiles-daemon's power-saver), on any source. Default
    `translucent`.

The power facts are `SystemStatus::power` (`{ on_battery, power_saver }`)
plus the existing battery level, read from `/sys/class/power_supply` and
`/sys/firmware/acpi/platform_profile` on the status poller. Charging is not
the signal: a full battery on mains reports "not charging". A peripheral
battery (`scope = Device`) never puts the machine on battery.

The runtime resolves the material in `apply_appearance` at startup (after
the first status sample), on every configuration change, and whenever a
status sample moves the power source, profile or battery level. The power
rules read the shell's latest status, so every path resolves against the
same facts. The shell receives the effective material
(`Shell::set_appearance`) and holds no policy. It also keeps the last
`ResolvedMaterial` as data so the settings snapshot can carry it.

`SettingsSnapshot::material` publishes the resolution. When a ceiling lowered
the preference, Settings → Appearance shows "Drawn as *X* right now:
*reason*" beneath the material row, so what is drawn never silently differs
from what Settings shows. A resolution that changes with the power source
is live state, not a setting: it republishes the snapshot under the same
revision and broadcasts `SettingsChanged`, so an in-flight Settings
transaction is not invalidated.

### Bodies

The palette declares every chrome body (`panel_surface`, `popover_surface`,
both dock bodies) **fully opaque** in neutral Apple-style grays (dark panel
`#1C1C1E`, menus `#2C2C2E`). A material sets only their alpha, in
`Design::with_material`, and every `SurfaceRole` is re-derived from them.
Because no component reads a material, a component cannot end up on a
different one from its neighbours.

Translucent alpha is tuned per body. Pure alpha over unblurred content has to
keep the body's own text legible over any page beneath it, so popovers stay
the most solid (230 dark / 236 light), then panels (209 / 214), then the dock
(189 / 196), which carries glyphs rather than prose.

### Notifications join the material

Toasts are drawn as `SurfaceRole::Chip` bodies through `place_floating`, like
the system OSD, and their damage band owns the elevation halo. They stay
display-only, non-capturing and three-second transient as ADR-0083 decided.

### Staging of the glass materials

`Frosted` and `LiquidGlass` are part of the vocabulary and are accepted from
configuration today, but `RENDERER_MATERIAL_CEILING` is `Translucent`, so they
are drawn as `Translucent` and the log names the `Renderer` ceiling. Drawing
them needs three things ADR-0213 deleted from Tessera (Optics still ships
the operators: `optics_gfx::BlurFilter`, `optics_material::FrostedFilter`,
`LiquidGlassFilter`):

1. A per-component body-region declaration on the `Chrome` contract.
2. A backdrop capture beneath chrome, which the single chrome pass of
   [ADR-0226](0226-single-pass-chrome-composition-preview-layering-and-modal-dock-inhibition.md)
   does not take.
3. The filter dispatch and its resident targets.

That work gets its own record, per ADR-0213's requirement that a surface
wanting optical treatment "argue for the capture and the shader on their own
merits". Landing it raises `RENDERER_MATERIAL_CEILING`. Nothing else in this
decision changes.

### Invariants & Behavioral Boundaries

- `[INV-MAT-01] One material`: every chrome body in a frame is drawn in the
  same effective material. Components read roles from `Design`, never a
  material.
- `[INV-MAT-02] Cost order`: `Material` variants are declared strictly
  cheapest first; `Ord` is the cost order (tested).
- `[INV-MAT-03] Ceilings only lower`: the effective material is never more
  expensive than the preference.
- `[INV-MAT-04] Opaque is opaque`: in `Opaque`, every floating role's body
  alpha is 255. This replaces ADR-0213's "alpha ≥ 236" for every material.
- `[INV-MAT-05] No idle capture`: the backdrop is captured only while the
  effective material samples it. With `Opaque` or `Translucent` in effect, the
  frame is exactly ADR-0213's frame.
- `[INV-MAT-06] High contrast is opaque`: high contrast always resolves to
  `Opaque`.
- `[INV-MAT-07] Power rules only lower`: every power rule is a ceiling. On
  mains with no power-saver profile, the power rules bind nothing.
- `[INV-MAT-08] A lowered material is explained`: whenever the effective
  material differs from the preference, the settings snapshot names the
  ceiling that decided it.

## Rejected Alternatives & Negative Knowledge

### Silently drop unsupported materials from configuration
- **Why considered**: a setting that does nothing today is the "setting most
  users never find" ADR-0213 warned about.
- **Why rejected**: the preference is the user's and should survive a
  renderer that cannot honour it yet (or a session on a weaker GPU). It is
  kept, resolved down, and the reason is logged.

### Keep the old navy palette as "Opaque"
- **Why considered**: no visual change for existing users.
- **Why rejected**: at alpha 242–248 it was neither opaque nor translucent,
  and it was the look the user explicitly replaced with a neutral dark gray.

### Raise translucency by blurring a cheap downscale
- **Why rejected**: this is ADR-0094's quarter-resolution smear, and it costs
  a capture anyway. If a capture is paid for, it belongs to `Frosted`.

## Consequences

### Positive
- One switch changes the dock, the Control Center, notifications, the battery
  alert, the OSD, prompts and menus together.
- The default keeps ADR-0213's cost (one fill per body) and is a fully
  opaque neutral dark gray instead of a 242-alpha navy.
- Power-aware materials needed only new ceilings, with no new policy code:
  unplugging, draining past the low line, or entering power saver makes
  chrome cheaper on the next status sample, and plugging in restores the
  choice.

### Negative / Trade-offs
- Power facts arrive on the status poller's cadence, so a change of power
  source shows up within one poll, not instantly.
- Power saver is read from the firmware platform profile. A machine whose
  power-profiles-daemon cannot drive one (no `platform_profile`) never
  reports power saver.
- Translucent over busy content is busier than an opaque body; legibility is
  held by alpha floors, not by an adaptive tint.
