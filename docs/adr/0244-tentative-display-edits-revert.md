---
id: ADR-0244
title: "Tentative Display Edits Revert on a Compositor-Owned Deadline"
status: accepted
date: 2026-11-18
scope: desktop/settings, protocol/schema, runtime/settings, settings
superseded_by: null
negative_knowledge: true
---

# 0244. Tentative Display Edits Revert on a Compositor-Owned Deadline

- Status: Accepted
- Date: 2026-11-18
- Amends: [ADR-0028](0028-output-and-monitor-model.md), [ADR-0243](0243-output-roles-and-mirroring.md)

## Context

Applying a new display mode or topology can leave a panel blank when the
connector rejects the mode, the TV switches input, or the selected arrangement
places the only visible screen outside the panel. Persisting immediately makes
that state survive a restart and gives the user no recovery path from the
Settings window if it is on the failed output.

## Decision

`DisplaySettings` carries `tentative`. A tentative edit:

1. Is still persisted and applied immediately through the normal
   `ConfigEdit::SetOutput` path.
2. Records the connector's previous effective `DisplaySettings` in the
   compositor and starts a 15-second revert deadline.
3. Exposes the remaining seconds in `DisplayStatus.pending_revert_seconds`.
4. Is kept by `SettingsAction::ConfirmDisplay` (Settings' **Keep changes**,
   `tessera display confirm`).
5. Is reverted by the compositor event loop when the deadline expires, by
   re-applying the recorded previous settings with `tentative = false`.

The timer lives in the compositor, not the Settings client: closing Settings or
losing the client must not cancel the safety net. The previous settings are
reconstructed from the live effective output (role and `role_source`, mode,
scale, position, primary) before the new transaction is applied.

## Invariants

- `[INV-ARCH-142]` A tentative display edit has exactly one compositor-owned
  pending revert. A second tentative edit replaces the pending record.
- `[INV-ARCH-143]` Confirmation and timeout are mutually exclusive: the event
  loop takes the pending record before applying a revert, so a late
  `ConfirmDisplay` cannot resurrect it.
- `[INV-ARCH-144]` A revert never starts another tentative edit.

## Consequences

- A bad mode/topology cannot persist without an explicit confirmation.
- The Settings page shows a countdown and a **Keep changes** action.
- CLI display-role edits are tentative and require `tessera display confirm`;
  display-topology edits remain immediate because they do not change the
  current connector's timing.
- The timer only proves the compositor is alive; a hard GPU hang still needs
  VT recovery. Real two-output hardware verification remains required.
