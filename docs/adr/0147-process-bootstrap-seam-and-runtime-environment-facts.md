# ADR-0147: Process bootstrap seam and shared runtime-environment facts

- Status: Proposed
- Date: 2026-08-30

## Context

Every first-party executable repeats the same pre-`run()` work, and the
repetitions have already drifted. As of this proposal:

- Observability init is unified: six process entry points call
  `tessera_bootstrap::init` (ADR-0079).
- `$XDG_RUNTIME_DIR` is resolved independently in six places with four
  different error styles (`crates/apps/tessera/src/main.rs`,
  `crates/apps/tessera/src/runtime.rs`,
  `crates/services/tessera-idle/src/main.rs`,
  `crates/services/tessera-atspi/src/main.rs`,
  `crates/services/tessera-mcp/src/config.rs`,
  `crates/core/tessera-compositor/src/server/interaction_domain.rs`).
  None validates that the directory is absolute, although the XDG
  basedir specification permits relative values that would corrupt
  socket paths.
- Per-binary socket override variables exist for two binaries only
  (`TESSERA_ATSPI_SOCKET`, `TESSERA_MCP_SOCKET`) and are mutually
  unaware.
- Socket cleanup is implemented twice: `tessera-idle` carries an inode-
  verified `SocketGuard` (Drop checks `dev`/`ino` so it never deletes a
  successor's socket), while `tessera-ipc`'s `Server::drop` performs a
  weaker unlink that can, in principle, delete a socket recreated by a
  successor after a race.
- No panic hook exists anywhere: a compositor crash produces no
  recorded, attributed final log record and no guaranteed socket
  cleanup.

The tier refactor (six tiers, `xtask check-boundaries`) gave these facts
an obvious home question: `tessera-bootstrap` (services) currently holds
only observability init, and its name promises more. The temptation this
ADR must resist is funneling all of the above through bootstrap at any
cost, which would invert a dependency law: `tessera-ipc` (api) would
need a services dependency to learn its own socket name.

The XDG basedir specification
(<https://specifications.freedesktop.org/basedir-spec/latest/>) defines
`$XDG_RUNTIME_DIR` as mandatory for conformant sessions and defines its
semantics (per-user, per-session, absolutely addressed, mode 0700,
ownership by the current user). The protocol constants themselves are
not environment: `$XDG_RUNTIME_DIR/tessera.sock` is part of the IPC
contract established in ADR-0027, exactly as wire types are.

## Decision

**1. A shared, restrictive bootstrap seam exists, and admission into it
is gated by a mechanical test.**

`tessera-bootstrap` (services tier) owns process-assembly facts whose
answer must be identical for every first-party process and whose
computation must happen before any component runs. It owns no business
assembly (that is the composition root in `apps/tessera`) and nothing
that runs after the entry point hands control to `run()`.

Admission criterion (enforced by `xtask check-bootstrap-admission`,
new): a function or type belongs in `tessera-bootstrap` only if it is
referenced by at least two distinct first-party process entry points.
The check parses `tessera_bootstrap::` references across `main.rs`-style
entry points; single-consumer pre-`run()` logic stays in its owning
binary. This is the guard against bootstrap becoming a junk drawer.

**2. Bootstrap owns exactly four responsibilities, enumerated, not
exemplified.**

- **Observability assembly** (already implemented, ADR-0079): the
  process-global tracing subscriber and the `log` bridge.
- **Runtime environment resolution**: `bootstrap::runtime_dir() ->
  Result<&'static-durable PathBuf>` resolving `$XDG_RUNTIME_DIR`, which
  MUST be an absolute path owned by the current user; relative values
  are rejected per the basedir spec's absolute-path requirement. This
  is a decision about the environment, identical for every process.
- **Panic and shutdown discipline**: one panic hook, installed by
  `init`, that (a) records the panic with location and payload as the
  final attributed log record before the default handler prints it,
  (b) releases registered cleanup guards via a registered-fixture
  pattern (not globals), and (c) preserves the abort/continue policy of
  the composition root. Signal handling remains per-binary; only the
  panic path is shared, because signals require per-binary
  disposition decisions.
- **Admission metering itself**: the check in (1) lives beside the
  crate it guards, so the boundary decays with the crate or not at all.

**3. Contract facts stay in the contract tier.**

Socket *names* are protocol (ADR-0027) and move into `tessera-ipc` as
pure functions over a caller-supplied runtime dir:
`ipc::socket_paths::default_socket_path(&runtime)` and companions for
each named socket (`idle` control socket, `tessera-portals` base
directory). `tessera-ipc` gains zero new dependencies. Consumers
compose: `ipc::socket_paths::default_socket_path(&bootstrap::runtime_dir()?)`.

**4. Socket hygiene moves to the domain that owns sockets.**

The inode-verified guard graduates from `tessera-idle` into
`tessera-ipc` as `ipc::socket_paths::SocketGuard` (RAII unlink with
`dev`/`ino` identity verification). `tessera-ipc`'s `Server` adopts it;
the weaker unlink path is deleted. `tessera-idle` drops its private
copy. Bootstrap does not own socket cleanup: an api-crate primitive
cannot depend on a services crate, and cleanup-after-crash is wired
through the panic hook's registered-fixture pattern instead.

**5. Per-binary override variables are per-binary decisions.**

`TESSERA_ATSPI_SOCKET` and `TESSERA_MCP_SOCKET` remain in their owning
binaries. No shared `TESSERA_SOCKET` variable is introduced; a shared
override would couple binaries that have no reason to share a debugging
affordance. Adding an override is a per-binary change, and its absence
is not drift.

**6. The seam is exercised by a startup conformance test.**

A workspace-level test instantiates each first-party binary's
bootstrap usage (init + runtime_dir + guard) against a sandboxed
environment and asserts the documented behaviors (filter defaulting,
JSON switching, absolute-path rejection, guard idempotence). Drift in
any binary is a build failure, not a review finding.

## Alternatives

- **Funnel everything through bootstrap.** Rejected: it would require
  `tessera-ipc` to depend on a services crate to learn its own protocol
  constant, inverting the tier law and splitting the socket name from
  the code that documents it. Correctness outranks convenience of
  deduplication.
- **Keep everything as-is; bootstrap stays logging-only.** Rejected:
  the six-fold XDG resolution and the weak-vs-strong socket guard are
  live drift with a latent correctness bug (relative-path acceptance,
  successor-socket deletion). "No new code" is not neutral; it is the
  choice to keep known bugs.
- **A `tessera-env` crate instead of bootstrap.** Rejected: it
  duplicates the seam bootstrap already claims; two assembly-layer
  crates would make the admission question ambiguous. If bootstrap
  were renamed away, this ADR would supersede with the same content.
- **Panic hook via `std::panic::set_hook` in each binary.** Rejected:
  the hook is where the guarantee "final record is attributed and
  cleanup runs" lives; six copies of it would drift exactly like the
  XDG resolution already has.
- **Global cleanup registry (static mutex of paths).** Rejected:
  global mutable state in an otherwise explicit process; the
  registered-fixture pattern keeps ownership local to the composition
  root and testable.

## Consequences

- The six `XDG_RUNTIME_DIR` call sites collapse to one validated
  implementation; relative-path acceptance and four error styles
  disappear.
- `tessera-ipc`'s socket cleanup gains identity verification; the
  successor-socket deletion hazard closes.
- Panics on any first-party binary produce one attributed final log
  record; crash triage no longer depends on which binary failed.
- `tessera-bootstrap` grows from one responsibility to four, with a
  mechanical admission gate that makes further growth a deliberate
  act rather than an accident.
- New requirement: `xtask check-bootstrap-admission` and the startup
  conformance test must run in CI; both are small and pure.
- New requirement: the portal repository (external consumer) adopts
  `runtime_dir()` at its next compatibility bump; the socket-name
  re-exports keep its current code compiling until then.
- Not solved here, recorded as future work: signal-to-shutdown
  discipline (logind path stays per-binary), daemonization, and any
  user-facing session-start sequence (wallpaper fade, dock entrance),
  which are composition-root and chrome concerns, not bootstrap ones.
