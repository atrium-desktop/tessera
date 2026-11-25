---
id: ADR-0257
title: "Explicit session IPC socket override and standalone Settings preview harness"
status: accepted
date: 2026-11-25
scope: ipc/transport, env/bootstrap, apps/settings, dev/testing
superseded_by: null
negative_knowledge: true
---

# ADR-0257: Explicit session IPC socket override and standalone Settings preview harness

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Consulted: `crates/tessera-env`, `crates/tessera-ipc`, `crates/tessera`, `crates/tessera-control`, `crates/tessera-settings`, `crates/tessera-mcp`, `scripts/run-nested-dev.sh`, `docs/dev/verification/`
- Extends: [ADR-0238](0238-settings-is-a-standalone-application-behind-its-own-scope.md) (standalone Settings application), [ADR-0147](0147-process-bootstrap-seam-and-runtime-environment-facts.md) (runtime-environment resolution)
- Related: [ADR-0027](0027-ipc-and-introspection.md), [ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)

---

## Context and Problem Statement

[ADR-0238](0238-settings-is-a-standalone-application-behind-its-own-scope.md) moved Settings into a standalone application (`tessera-settings`) that reaches the compositor strictly over the session IPC socket. Validating and iterating on Settings exposed three structural gaps in the development and verification workflow:

1. **No IPC socket isolation for nested sessions.** The compositor always bound `$XDG_RUNTIME_DIR/tessera.sock`, and first-party clients (`tessera` CLI, `tessera-settings`, `tessera-mcp`) defaulted to that same path. Running a nested Tessera instance inside an active Tessera host desktop therefore failed to bind IPC (`AddressAlreadyInUse`) and ran without IPC — making end-to-end Settings and CLI verification inside a nested session impossible without replacing `$XDG_RUNTIME_DIR` (which breaks Wayland and D-Bus socket discovery).
2. **No Layer 2 inspection harness for Settings.** While `tessera-lock` ships a feature-gated development harness (`tessera-lock-preview`) that renders the production surface in an ordinary Wayland window with deterministic state injection, `tessera-settings` had no equivalent. Every UI or edge-case check (multi-display layouts, libinput devices, stale-revision rejections, read-only scope refusal, locked-session refusal, compact `<640px` rail) required either a full compositor or physical hardware.
3. **Nested development sessions polluted host config and opened Settings read-only.** `scripts/run-nested-dev.sh` isolated `XDG_DATA_HOME` and `XDG_STATE_HOME` but left `XDG_CONFIG_HOME` pointing at the host user's `~/.config/tessera/config.toml`, and defaulted to `--profile release-fast` (`debug_assertions = false`), where the sibling-binary development allowlist is disabled and `tessera-settings` opened read-only.

## Decision Drivers

- Preserve strict security boundaries: no environment variable or preview feature may weaken peer-identity scope checks, allowlists, or production binary exclusion.
- Follow the workspace's four-layer testing hierarchy (`Unit -> Preview -> Nested Integration -> Physical VT/DRM`) without duplicating UI or transport code.
- Keep the production `tessera-settings` binary free of mock/preview branches and non-production code.
- Make nested Settings verification work out of the box even when the host desktop is itself Tessera, without touching host configuration or audit files.

## Considered Options

1. **Require a separate `XDG_RUNTIME_DIR` for nested sessions.** Rejected: breaks discovery of the outer Wayland display and session D-Bus socket unless manually symlinked, and still leaves no Layer 2 preview harness.
2. **Add an in-process `--mock` flag to production `tessera-settings`.** Rejected: violates the surface-testing rule that production binaries carry zero harness code or `cfg` branches for simulated state, and bypasses the real IPC transport.
3. **Add a validated `TESSERA_IPC_SOCKET` contract across the compositor and first-party clients, a separate feature-gated `tessera-settings-preview` harness backed by a real wire-level `tessera_ipc::Server`, and full config/socket isolation in `scripts/run-nested-dev.sh`.**

## Decision Outcome

Chosen option: **3**.

### 1. Explicit IPC Socket Override (`TESSERA_IPC_SOCKET`)

- `tessera-env` owns the environment variable `TESSERA_IPC_SOCKET` via `ipc_socket_override() -> Result<Option<PathBuf>, IpcSocketError>` (fail-closed: empty or relative paths are rejected, matching `runtime_dir()` under ADR-0147).
- Resolution precedence across the compositor and every first-party IPC client (`tessera` CLI, `tessera-settings`, `tessera-mcp`) is:
  1. Explicit CLI flag (`--socket PATH`, or `TESSERA_MCP_SOCKET` for `tessera-mcp`)
  2. `TESSERA_IPC_SOCKET` environment variable
  3. `$XDG_RUNTIME_DIR/tessera.sock` (default)
- When the compositor binds an explicit `TESSERA_IPC_SOCKET`, it keeps that variable in its process environment so supervised children and applications launched from its launcher or Control Center (such as `tessera-settings`) automatically target the same compositor instance. Nested sessions never export `TESSERA_IPC_SOCKET` to the host D-Bus or systemd activation environment.
- `TESSERA_IPC_SOCKET` selects the transport endpoint only; it never alters scope authorization, peer executable checks, or lock-screen refusal.

### 2. Standalone Settings Preview Harness (`tessera-settings-preview`)

- `crates/tessera-settings` declares a non-default Cargo feature `dev-preview` and a development-only binary target `tessera-settings-preview` (`required-features = ["dev-preview"]`).
- The production `tessera-settings` binary compiles with zero preview code and zero `dev-preview` branches. `xtask check-install-manifest` registers `tessera-settings-preview` in `DEV_ONLY` alongside `tessera-lock-preview`, ensuring distribution builds and `scripts/install.sh` exclude it.
- Instead of bypassing IPC, `tessera-settings-preview` starts an in-process wire-level `tessera_ipc::Server` on an ephemeral private socket (or an explicit `--socket PATH` in `--mock-server-only` mode so an unmodified production `tessera-settings --socket PATH` binary can also be tested against it) and runs the production Settings window and `Link` against it.
- The harness supports deterministic inspection flags:
  - `--preset desktop|multi-display|laptop|minimal` — realistic hardware/configuration snapshots including multi-output and libinput states unreachable in nested mode;
  - `--mode live|read-only|reject-stale|reject-locked|disconnected` — deterministic authority and fault injection;
  - `--size WIDTHxHEIGHT` — fixed initial logical window size (including `<640px` compact rail validation);
  - `--ready-fd FD` — writes one byte after the first presented frame for log-free agent pixel capture;
  - `--mock-server-only --socket PATH` — runs only the mock IPC server for testing unmodified production binaries or CLI tooling;
  - `[PAGE]` — deep-links directly to a page.
- The preview window uses app ID `dev.tessera.SettingsPreview` and never touches the production single-instance socket.

### 3. Isolated Nested Development Workflow (`scripts/run-nested-dev.sh`)

- `scripts/run-nested-dev.sh` isolates `XDG_CONFIG_HOME` (default `/tmp/tessera-dev/config`), `XDG_DATA_HOME`, `XDG_STATE_HOME`, and `TESSERA_IPC_SOCKET` (default `/tmp/tessera-dev/tessera.sock`) so nested runs never collide with a live host Tessera session or mutate `~/.config/tessera/config.toml`.
- The script ensures the isolated config's `[ipc.scope_executables]` authorizes the workspace-built `tessera-settings` binary for the active Cargo profile (`release-fast` or `debug`), so Settings is immediately writable without weakening compositor scope enforcement.

### Invariants & Behavioral Boundaries

- `[INV-SETTINGS-05]` `TESSERA_IPC_SOCKET`, when set, must be an absolute path; empty or relative values fail closed.
- `[INV-SETTINGS-06]` `TESSERA_IPC_SOCKET` changes only the socket endpoint; it never bypasses peer-identity scope binding or lock-screen refusal.
- `[INV-SETTINGS-07]` Production builds of `tessera-settings` (`--no-default-features`) contain no `tessera-settings-preview` binary and no mock server code.
- `[INV-SETTINGS-08]` `scripts/run-nested-dev.sh` never writes to the host user's `$HOME/.config/tessera/config.toml` or binds the host session's `$XDG_RUNTIME_DIR/tessera.sock`.

## Rejected Alternatives & Negative Knowledge

### In-process `--mock` flag on `tessera-settings`
- **Why considered**: avoids a second binary target.
- **Why rejected**: puts mock state and simulated authority inside a privileged first-party binary, violating `docs/dev/verification/surface-verification.md`, and cannot verify production exclusion at packaging time.

### Bypassing IPC in the preview harness
- **Why considered**: slightly less code than implementing `tessera_ipc::Handler`.
- **Why rejected**: skips `Link`'s two-connection command/watch lifecycle, lease renewal, `SettingsChanged` event fan-out, and read-only scope fallback, leaving transport regressions invisible until Layer 3.

## Consequences

### Positive
- Settings UI, compact layouts, multi-display/libinput pages, and error states can be inspected and pixel-captured in seconds without a compositor.
- Nested Tessera sessions inside a live Tessera desktop now run with full IPC, writable Settings, and zero risk to host configuration.
- Both split-screen debugging (Settings on the host desktop targeting nested `TESSERA_IPC_SOCKET`) and in-nested debugging (`WAYLAND_DISPLAY=wayland-N`) work cleanly.

### Negative / Trade-offs
- Adds one development-only binary target (`tessera-settings-preview`) to `DEV_ONLY` in `xtask` and one documented environment variable (`TESSERA_IPC_SOCKET`).
