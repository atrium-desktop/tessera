# tessera-bootstrap

`tessera-bootstrap` is the shared process bootstrap for every first-party
Tessera executable: the compositor binary, the idle/session sidecars, the
lock host, and out-of-tree consumers such as the portal backend.

## Responsibilities

- Observability init (`init`): installs the `tracing`-based subscriber and
  bridges `log::` records into it (ADR-0079). Filtering honors `RUST_LOG`;
  `TESSERA_LOG_FORMAT=json` switches the console to JSON.
- (Growth area) panic hooks, `$XDG_RUNTIME_DIR` probing, socket cleanup,
  and daemonization — the shared concerns of every Tessera process belong
  here instead of being re-derived per binary.

## Boundary

- Consumer crates keep using the `log` facade and never depend on this
  crate; only executables and sidecars call `init`.
- Bootstrap owns nothing that runs after the process is up: it hands a
  configured process to the composition root and exits the picture.
