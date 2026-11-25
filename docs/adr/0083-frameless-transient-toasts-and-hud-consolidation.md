---
id: ADR-0083
title: "Frameless transient toasts and HUD consolidation"
status: accepted
date: 2026-07-31
superseded_by: null
---

# ADR-0083: Frameless transient toasts and HUD consolidation

- Status: Accepted
- Date: 2026-07-31
- Note (2026-11-25): the *command panel* this record names is the **Control Center**; the surface was renamed, and [ADR-0239](0239-the-control-center-is-a-right-anchored-sheet.md) removed its last `command_panel` aliases and fixes its current shape. The decision applies to it unchanged.
- Amended by: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) — the display-only status HUD and its `[hud]` table are removed
- Amended by: [ADR-0214](0214-tiered-system-readings-and-a-barrier-only-at-the-end.md) — battery reporting leaves the modal band
- Amended by: [ADR-0252](0252-chrome-materials-are-one-swappable-shell-wide-choice.md) — toasts gain a body in the shell's material; they stay display-only and transient

## Context

ADR-0080 made the HUD display-only and moved every interaction to the
command panel, but the notification toasts were still framed, interactive
panels: they captured pointer input over their rects and dismissed on
click. Their lifetime was also coupled to the shared `NotificationQueue`'s
five-second TTL, so an entry vanished from the command panel's Messages
list, the HUD count, and the IPC history at the same moment its popup
disappeared — leaving nothing to interact with after the fact.

The product direction is an ambient, VR/AR-style notification strip:
plain floating text with no background panel that never competes with
windows, plus a HUD that keeps the top-right corner free for it. The Agent
Workspaces status the HUD carried belongs with the panel's other
interactive surfaces.

## Decision

1. **Toasts are display-only, frameless, and transient.** The toast strip
   renders plain floating text — no background, border, or radius — stacked
   at the top-right, newest on top. It captures no pointer input
   (`Chrome::captures_pointer` keeps its default) and offers no
   click-to-dismiss. Each toast is visible for three seconds, measured
   against the compositor clock the queue is ticked with
   (`NotificationQueue::now_ms`, recorded by `expire`); aging out is not a
   queue mutation, so the component reports `anim_pending` while any toast
   is inside its window to guarantee the clearing frame is drawn.
   Notification interaction (dismissal) happens only in the command
   panel's Messages section.

2. **Retention is split from presentation.** The queue's TTL is the
   retention horizon for the command panel's Messages list, the HUD
   notification count, and the IPC history — raised from five seconds to
   one hour. The toast strip applies its own three-second presentation
   window on top of the same entries. Do-not-disturb semantics are
   unchanged: it suppresses toast presentation while history keeps
   accumulating.

3. **The HUD consolidates to two chips.** The right chip is removed; the
   top-right is reserved for the toast strip. The clock and the
   notification bell move into the left chip after the tray row; the
   center workspace dots are unchanged.

4. **The Agent Workspaces status moves to the command panel.** The HUD
   drops its state-colored pill and its Realm snapshot entirely; the
   panel's System section gains a display-only status row aggregating the
   live Agent Realms (Idle / Active / Paused / PartiallyPaused), ported
   from the HUD's indicator logic.

## Alternatives

- **Keep framed, clickable toasts.** A popup that eats clicks near the
  top edge contradicts the click-through chrome policy of ADR-0080, and a
  second dismissal surface duplicates what the Messages section already
  does.
- **Shorten the queue TTL to three seconds.** Toast and history would stay
  coupled: the panel's Messages list and the HUD count would lose every
  entry the moment its popup disappeared, making "interact in the command
  panel" impossible.
- **Keep the Agent Workspaces pill in the HUD.** The HUD is a pure status
  display; AI session state belongs with the panel surfaces the user
  already opens to act on the session.

## Consequences

- Users can no longer dismiss a notification from its popup; dismissal
  lives in the command panel's Messages section (`Super+S`).
- Notification history survives the popup: the Messages list, the HUD
  bell count, and the IPC queries see entries for up to one hour, bounded
  by retention rather than presentation.
- The toast component repaints continuously while any toast is inside its
  three-second window (`anim_pending`); outside the window it costs no
  frames.
- `aegis-hud` no longer consumes Realm snapshots; `aegis-command-panel`
  implements `Chrome::update_realms` instead.
- `NotificationQueue` exposes the compositor clock it is ticked with
  (`now_ms`); any future transient surface can layer its own presentation
  window on the same history without touching the TTL.
