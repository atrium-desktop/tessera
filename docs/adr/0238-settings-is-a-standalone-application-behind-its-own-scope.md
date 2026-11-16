---
id: ADR-0238
title: "Settings is a standalone application behind its own built-in scope"
status: accepted
date: 2026-11-16
scope: shell/control-center, apps/settings, ipc/authority
superseded_by: null
negative_knowledge: true
---

# ADR-0238: Settings is a standalone application behind its own built-in scope

- Status: Accepted
- Date: 2026-11-16
- Deciders: Tessera Maintainers
- Consulted: `crates/tessera-shell` (Control Center, settings module library),
  `crates/tessera-ipc` (handshake, scope binding), `crates/tessera`
  (built-in scopes, executable allowlists)
- Supersedes: [ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md)
  (panel-hosted settings)
- Amends: [ADR-0115](0115-command-panel-desktop-behavior-scope.md) (the
  Control Center's scope no longer includes persistent settings),
  [ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md) (a fifth
  built-in scope)
- Related: [ADR-0049](0049-standalone-modular-control-center.md),
  [ADR-0056](0056-system-settings-identity-and-boundary.md),
  [ADR-0239](0239-the-control-center-is-a-right-anchored-sheet.md)

---

## Context and Problem Statement

[ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md) moved every
settings page into the command panel because the standalone application of
[ADR-0049](0049-standalone-modular-control-center.md) had not delivered its
promises: no third-party module protocol existed, and the app duplicated the
panel's host and IPC plumbing. Re-reading the code — not the history — shows
the trade has turned:

- **The process boundary is free now.** The `SettingsModule` contract depends
  only on Lens, `tessera-design`, `tessera-i18n`, and `tessera-desktop`.
  `tessera-lock` already runs Lens in an ordinary Wayland client. IPC
  protocol 4 already carries revisioned snapshots, change events, and
  confirmed transactions (`GetSettings`, `SettingsChanged`, `Settings`). The
  panel's own hosting code was about thirty lines.
- **The cost of staying grows with every module.** Settings pages are the
  most edge-case-heavy UI in the shell (display, power, input, and appearance
  alone are ~2,000 lines of draft and reconciliation logic against hardware
  that changes underneath them), and nothing isolates a panic in chrome: a
  settings bug ends the session. The planned `users` module would bring
  AccountsService and polkit authority into the display server.
- **The panel is the wrong surface for settings.** An opaque, full-output,
  click-away modal hides exactly what appearance, dock, display, and
  accessibility edits change; it cannot sit beside an application; it cannot
  be deep-linked; and a tab per module does not scale past a handful.

## Decision Drivers

- A settings defect must not be able to take the session down.
- Privilege must be the narrowest that settings need, and bound to the binary.
- Settings must be reachable by deep link from the shell, the launcher, and
  the CLI, and show their effect on the live desktop.
- No new protocol: reuse the transaction path every IPC client already uses.

## Considered Options

1. Keep settings in the Control Center, with a second-level navigation view.
2. A standalone application on the existing IPC, under the owner-admin scope.
3. A standalone application on the existing IPC, under its own built-in scope.

## Decision Outcome

Chosen option: **3**.

1. **`tessera-settings` is its own crate and binary.** The library holds the
   module contract, the registry, the built-in pages, and the window model
   (`SettingsHost`: navigation, search, outbox, connection state). The binary
   is an `xdg_toplevel` (application id
   `com.hihusky.Tessera.Settings`) rendering through Flux/Lens, with
   server-side decorations, fractional scale, cursor-shape support, and the
   standard Wayland clipboard for its text fields. `tessera-shell` no longer
   contains or depends on any settings module.
2. **IPC is the only path.** A command connection fetches snapshots and
   commits `SettingsAction`s with the observed revision, renewing its lease;
   a watch connection subscribes and turns every `SettingsChanged` into a
   refresh. Both reconnect with bounded backoff, drafts are held while the
   link is down, and instant-apply drafts are coalesced to the newest value
   per action kind.
3. **A built-in `tessera-settings` scope.** Under the default agent lockdown
   an anonymous connection is query-only, so the app claims a fifth built-in
   scope, bound by [ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)
   to `/usr/bin/tessera-settings` and `/usr/local/bin/tessera-settings`
   (overridable through `[ipc.scope_executables]`). Its operation list is
   empty, and the dispatcher refuses every `Do` command on it by name —
   scopes do not gate session commands such as `Quit`, so the closure has to
   be explicit. The one exception is `Command::Focus` when the target window
   belongs to Settings itself: a later invocation raises the existing window
   without gaining authority over any other window. The scope's `session`
   lease therefore authorizes settings transactions plus self-focus.
4. **Refusal degrades to read-only.** When the scope claim is refused (an
   unlisted build), the app connects query-only, shows live settings, labels
   itself read-only, and refuses each edit visibly instead of retrying. A
   debug compositor additionally accepts the `tessera-settings` binary built
   beside it, so a workspace development run does not need a hand-written
   allowlist.
5. **One window per session.** A per-session socket in `$XDG_RUNTIME_DIR`
   hands a later invocation's page to the running window and asks it to
   raise itself. Every entry point uses `tessera-settings [PAGE]`; the
   Control Center's gear launches it through the ordinary launch path
   (`tessera_desktop::app::settings_app`).

### Invariants & Behavioral Boundaries

- `[INV-SETTINGS-01]` The compositor process links no settings module.
- `[INV-SETTINGS-02]` The Settings scope issues no `Do` command except
  `Command::Focus` targeting a window whose `app_id` is Settings; its lease
  authorizes `Request::Settings` and that self-focus only.
- `[INV-SETTINGS-03]` A refused scope claim never loops: the app is read-only
  until relaunched.
- `[INV-SETTINGS-04]` A snapshot older than the one shown is ignored.

## Rejected Alternatives & Negative Knowledge

### Keep settings in the Control Center (option 1)
- **Why considered**: zero process cost; ADR-0114's reasoning.
- **Why rejected**: it keeps settings defects session-fatal, keeps future
  privileged backends in the display server, and keeps settings in a modal
  that hides their effect. A second-level view fixes navigation only.

### Reuse `tessera-owner-admin` (option 2)
- **Why considered**: no new scope.
- **Why rejected**: owner-admin carries window, workspace, system, and
  screenshot operations. Settings needs none of them.

## Consequences

### Positive
- A settings crash closes a window, not the session.
- Least privilege by construction, audited like every other scope claim.
- Settings open beside the live desktop, resizable, deep-linkable, with
  search and a categorized sidebar.

### Negative / Trade-offs
- A second GPU context and a startup cost when Settings opens.
- An installed build that is not in the scope's executable allowlist opens
  read-only; development builds avoid that through the sibling-binary rule.
