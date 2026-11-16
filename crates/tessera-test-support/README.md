# tessera-test-support

Shared test infrastructure for the Tessera workspace.

This crate is a **development dependency only**. It never ships in a production
binary and must not be added to a crate's normal dependency table.

It owns the small, boring pieces every test suite otherwise copies:

- `env` — a process-wide environment lock and restoring `EnvGuard`s.
- `temp` — scratch directories and files that are removed on drop.
- `net` — process-unique Unix socket paths for loopback servers.
- `poll` — bounded waits for asynchronous test conditions.
- `workspace` — workspace/crate/fixture paths and built binary lookup.

Unit tests stay close to the code they exercise. This crate exists for
infrastructure shared *between* crates and for the root integration/e2e suites.
