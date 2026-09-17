# ADR-0160: Consolidate application subsystem into tessera-apps

- Status: Accepted
- Date: 2026-09-17
- Scope: Application discovery, icon-theme lookup, process execution, and intent search
- Amends: [ADR-0158](0158-clean-break-workspace-boundaries.md), [ADR-0159](0159-package-boundaries-follow-capabilities.md)

## Context

Desktop application lifecycle in Tessera encompasses scanning freedesktop
`.desktop` entries, resolving theme-accurate SVG/PNG icons, launching child
processes under XDG environment and sandbox constraints, and querying applications
and intent commands through search matchers.

Previously, this single domain was partitioned across four micro-packages:
- `tessera-desktop-entries` (~1400 lines): `.desktop` parsing and directory walking.
- `tessera-icons` (~1000 lines): Icon theme directory traversal and scale lookup.
- `tessera-launcher` (~500 lines): Process spawning and sandbox setup.
- `tessera-search` (~800 lines): Query matching and calculator evaluation.

This separation suffered from several architectural flaws:
1. **Artificial Lifecycle Slicing**: Callers (`tessera`, `tessera-shell`,
   `tessera-cli`, `tessera-mcp`) almost always consumed these packages in tandem.
2. **Leaked Implementation Details**: Internal INI helpers, directory walk
   utilities, and token expansion algorithms were forced to be public across
   package boundaries.
3. **High Ritual Overhead**: Maintaining four separate `Cargo.toml` files, version
   numbers, and capability matrix entries in `dependency-policy.toml` for
   interdependent ~500–1000 line libraries.

## Decision

Consolidate `tessera-desktop-entries`, `tessera-icons`, `tessera-launcher`, and
`tessera-search` into a single, cohesive domain crate: **`crates/tessera-apps`**.

The crate is structured internally into modular capabilities:
- `entries`: XDG desktop-entry parsing, enumeration, and token expansion.
- `icons`: Freedesktop icon-theme lookup, theme inheritance, and scale-aware resolution.
- `launcher`: Process invocation, detached spawning, and interaction domain sandbox boundaries.
- `search`: Intent dispatch engine, prefix/fuzzy matching, and math evaluation.

The root of `tessera-apps` re-exports the primary types (`Entry`, `AppEntry`,
`IntentEngine`, `IntentAction`, `LaunchOpts`, `resolve_icon`, `eval_math`, etc.)
so callers interact with a unified application discovery and lifecycle API.

### Invariants & Behavioral Boundaries

- `tessera-apps` is effect-bounded: it performs filesystem inspection (desktop files,
  icon paths) and process execution (`libc`/`fork`/`exec`), but contains no Wayland,
  GPU, IPC transport, or UI chrome implementations.
- `tessera-desktop` remains pure and side-effect-free; it does not perform filesystem
  walks or process execution.
- Consumers requiring application enumeration, icon resolution, intent query, or
  application spawning depend solely on `tessera-apps`.

## Rejected Alternatives

### Keep four distinct packages

Rejected. The crates do not evolve independently, have no distinct platform or
feature boundaries, and are not published separately.

### Merge directly into `tessera-desktop`

Rejected. `tessera-desktop` is explicitly designed as a pure, deterministic,
zero-I/O model crate testable without OS filesystem or process spawning
capabilities. Merging file reading and process launching into `tessera-desktop`
would violate this foundational invariant.

## Consequences

- Reduces workspace crate count from 30 to 27.
- Eliminates cross-package boilerplate and internal API leaks.
- Consumers (`tessera`, `tessera-shell`, `tessera-cli`, `tessera-mcp`,
  `tessera-tray`, `tessera-avatar`) now interact with a single, coherent
  application service dependency.
