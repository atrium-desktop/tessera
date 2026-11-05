---
id: ADR-0214
title: "Tiered System Readings: A Glance for Thresholds, a Barrier Only at the End"
status: accepted
date: 2026-11-04
scope: shell, runtime, i18n, interaction, architecture
superseded_by: null
negative_knowledge: true
---

# 0214. Tiered System Readings: A Glance for Thresholds, a Barrier Only at the End

- Status: Accepted
- Date: 2026-11-04
- Deciders: Tessera Maintainers
- Amends: [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md), [ADR-0083](0083-frameless-transient-toasts-and-hud-consolidation.md) (battery reporting leaves the modal band)
- Related: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) (the status HUD whose readings this replaces)

---

## Context and Problem Statement

A configured `[battery] warn_at` threshold means "tell the user their charge
crossed this line". The previous design read that as "stop what they are doing":
every threshold raised a full-screen modal that captured pointer and keyboard,
dimmed the output, and waited to be dismissed. A user typing mid-drop was
interrupted to be told something a glance at a corner would have covered.

The failure is a category error about what a threshold *is*. Thresholds are
graduated on purpose — the shipped policy is `[20, 5]` — and the two ends of that
gradient do not mean the same thing. Crossing 20% is information. Crossing 5% is
the last warning before the machine suspends.

The product also had no surface for the smaller class of report at all: a volume
step, a brightness step, a wireless link coming up. Those are not notifications —
nothing outside the machine is talking — and they are not history. There was
nowhere for them to go, so nothing reported them.

## Decision Drivers

- **An acknowledgement must not be able to steal a keystroke.** A reading raised
  by an action the user just took cannot interrupt the action after it.
- **Reporting must be graduated like the thresholds it reports.** One surface for
  both ends either overshoots the soft case or undershoots the hard one.
- **A reading with no history must be cheap.** A surface that holds no history
  cannot cost the machinery of one.
- **The last warning is a barrier.** If the machine is about to suspend, a
  glance is not enough and the design must say so rather than soften it.

## Considered Options

- **Keep the modal for every threshold, but auto-dismiss it.** Rejected: an
  auto-dismissing modal still captures pointer and keyboard, still dims the
  output, and still takes the frame's attention for its whole life. It is an
  interruption that ends sooner, not an acknowledgement.
- **Route battery readings through the notification queue.** Rejected: a battery
  warning must not be suppressible by do-not-disturb, and a notification is a
  message the user may want to act on rather than a reading of state.
- **Make the modal's dismissal require a hover dwell.** Rejected: this converts
  a passive reading into a compulsory interaction. A user who is typing has to
  stop, find the pointer, and aim at a surface — to clear something they did not
  ask for.
- **One transient surface, tiered by tone, with a barrier reserved for the
  threshold that ends the session.** Chosen.

## Decision Outcome

Chosen option: "One transient surface, tiered by tone, with a barrier reserved
for the threshold that ends the session", because the crossing that actually ends
the session deserves to interrupt and every other crossing deserves not to.

### The transient system OSD

`SystemOsd` reports a local hardware state change: the volume the user turned, the
brightness they moved, the battery that just crossed a level, a link that came up.
Three properties follow, and each is a refusal:

- **No pointer, no keyboard.** The surface never captures input, so a reading that
  arrives mid-sentence cannot steal a keystroke. This is the whole reason the
  tiered policy exists.
- **No stack.** A newer reading replaces the current one. Two volume steps in a
  row are one volume reading, not two panels.
- **No history.** When it leaves, it leaves. Anything worth keeping belongs in the
  notification stream or the command panel, which are built for it.

It anchors above the dock's edge — where the user's attention already is when they
reach for a hardware control — holds no reserved space, and self-ages from the
frame delta rather than asking the host for a second time source. It names its
expiry through `animation_deadline_in`, so a reading costs one scheduled wake
instead of pumping frames to discover that it is over.

`OsdKind` covers level readings (battery, volume, brightness) and link readings
(wireless, Bluetooth), so the volume and brightness controls have somewhere to
report as they land instead of being silent.

### The tiering

`poll_battery_warning` splits the crossing it was already computing:

- **Every threshold above the lowest** raises the OSD.
- **The lowest threshold** raises the existing `BatteryAlert` barrier, because
  that is the crossing after which the session ends.

The distinction was already available in the policy — the code computed
`critical = thresholds.min() == Some(threshold)` to pick the barrier's wording —
it just never changed which surface was used.

### Readings are localized where chrome is

The composition root reaches the shell's catalog through `Shell::localizer()`
instead of keeping a second copy of the strings, and
`Localizer::osd_battery_low` owns the escalate-vs-not wording decision so the
tier is expressed once.

### Invariants & Behavioral Boundaries

- An OSD never captures the pointer or the keyboard, never reserves space, and
  never enters the notification queue.
- An OSD replaces itself rather than stacking. At most one reading is on screen.
- The lowest configured `[battery] warn_at` threshold is the **only** value that
  raises the barrier. A policy with three thresholds still raises exactly one
  barrier.
- A reading that has left its window paints nothing and holds no frame loop.

## Rejected Alternatives & Negative Knowledge

- **A longer lifetime for a critical reading.** Rejected: a longer bar is a larger
  interruption, and the barrier exists for the case that actually needs to
  interrupt. The critical *tone* escalates; the duration does not.
- **A hover-to-dismiss reading.** Rejected above: it turns a glance into a task.
- **Reporting the OSD through `ChromeUpdate::SystemStatus` alone.** Rejected: a
  reading is an event with an instant, not a state to diff. A component diffing
  snapshots would have to invent the instant it began and would collapse two
  consecutive readings into one.

## Consequences

- A user turning volume or brightness now gets a reading where they previously got
  nothing, and a user whose battery crosses 20% is no longer stopped to be told.
- The battery barrier keeps its full weight for the crossing that matters, so the
  softer thresholds gain nothing at its expense.
- The OSD is one more surface that has to be registered and kept honest about its
  expiry; it reports its painted halo as damage so a partial repaint covers the
  shadow, and its age is derived rather than stored so it cannot drift from the
  frame clock.
- `ChromeCommand::ShowOsd` and `Chrome::osd_active` join the contract. A host that
  composes fewer components simply has no OSD, and `Shell::show_osd` becomes a
  broadcast that fewer components receive.
