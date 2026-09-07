# tessera-avatar

`tessera-avatar` owns the persona domain for tessera shell surfaces:
local account defaults, and — behind the `persona` feature — the shared
still/VRM portrait, motion playback, and hot-reload pipeline
(ADR-0080, ADR-0096, ADR-0097).

## Responsibilities

- The lightweight `persona::Profile` contract (available without features).
- Portrait discovery from XDG portrait directories (via `tessera-icons`'
  XDG base-directory resolution).
- VRM/still portrait texture upload through the compositor's flux device.
- Motion library playback, semantic triggers, and transactional hot reload.

## Non-goals

- No authentication: security principals stay in `tessera-security` and the
  lock-screen boundary.
- No host process: consumers (command panel, lock host) own the runtime and
  pass in the borrowed flux device.
