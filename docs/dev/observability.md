# Observability

How tessera logs, how to control it, and the level discipline contributors are
expected to follow. The architectural decision is
[ADR-0079](../adr/0079-tracing-based-observability-seam.md).

## The seam

Most crates emit through the standard `log` facade; code that needs
structured, correlated fields uses `tracing` directly (the damage pipeline is
the current example). Each first-party binary installs a `tracing`-based
subscriber through the shared `tessera-env` crate before doing anything else:

```rust
fn main() {
    tessera_env::init("info");
    // ...
}
```

The subscriber bridges `log::` records into `tracing`, so existing
`log::{error,warn,info,debug}` callsites are captured with span context
without rewriting them. Structured spans are added with the `tracing` crate
where request or lifecycle correlation is worth it (see
[Spans](#spans)).

## Configuration

All output is written to stderr so journal capture is uniform.

- `RUST_LOG` — a `tracing_subscriber::EnvFilter` directive. Examples:
  `info`, `debug`, `info,tessera_backend::drm=trace`,
  `warn,tessera_portal=debug`.
- `TESSERA_LOG_FORMAT=json` — emit machine-readable JSON for log aggregation.
  Any other value (or unset) selects the human-readable text formatter.
- `NO_COLOR` — disable ANSI color, matching the convention honored by
  `env_logger` and most CLI tooling. Color is also auto-disabled when stderr
  is not a TTY.

Each binary passes a sensible default used when `RUST_LOG` is unset: `info`
for long-running services (`tessera`, `tessera-idle`, `tessera-lock`,
`xdg-desktop-portal-atrium`) and `warn` when `tessera` runs a
one-shot management command.

## Nested workflow

The default nested loop is quiet enough to read:

```bash
cargo run --locked -p tessera
```

To trace the DRM backend or a specific subsystem without flooding the
console, scope the filter:

```bash
RUST_LOG="info,tessera_backend::drm=debug" cargo run --locked -p tessera
```

## Log levels

Use the standard pyramid. The default `info` run should read like a concise
narrative of significant events, not a per-request trace.

| Level | When |
|-------|------|
| `error` | Something is broken and needs attention now (a subsystem cannot start, a fatal invariant failed). |
| `warn` | An unexpected condition that operation survived but that a maintainer would want to see — once, not per retry. |
| `info` | Process and session lifecycle: startup, shutdown, output added/removed, session locked/unlocked, a settings or policy reload applied. One-time or user-driven, not per request. |
| `debug` | Per-request flow, routine recovery, retries, fallbacks, and detailed diagnostics. The level you turn on when something needs investigating. |
| `trace` | Very detailed, often per-frame or per-buffer. Reserved for deep debugging. |

Downgrade, do not silence. If a record fires on a hot path or on every retry,
it almost certainly belongs at `debug`, not `info` or `warn`. A transient or
recoverable failure that the code already handles (a D-Bus method declined, a
fallback taken, a pick cancelled) is `debug`; reserve `warn` for conditions
that indicate the system or one of its dependencies is misbehaving.

When adding a record in a hot loop, prefer a `debug` or `trace` level so it is
elided at the default `info` level and free in production.

## Spans

Spans correlate the events of one logical operation across functions. Add
them with the `tracing` crate, and only where the correlation is worth it —
typically request- or lifecycle-scoped work, not every function.

```rust
let _span = tracing::debug_span!("ipc", kind = "system_control", ?action).entered();
```

Prefer `debug_span` (elided at the default `info` level) for anything that
could run frequently, so instrumentation is free unless someone raises the
filter. Events emitted through `log::` inside an entered span are captured
into that span automatically via the bridge.

## Damage diagnostics

The damage pipeline emits structured `tracing` events under the
`tessera.damage.*` target namespace. They are `trace`-level (per commit or per
frame), and the field expressions are gated with `tracing::enabled!` so a
disabled trace costs nothing.

Enable them with a prefix filter:

```bash
RUST_LOG="info,tessera.damage=trace" TESSERA_LOG_FORMAT=json \
  cargo run --locked -p tessera
```

| Target | Event | Key fields |
|--------|-------|------------|
| `tessera.damage.commit` | `commit.decision` | `surface`, `window`, `generation`, `buffer_set`, `damage_reported`, `geometry_revision`, `damage_space_changed`, `unknown_full`, `buffer_extent`, `frame_damage`, `surface_damage`, `buffer_damage`, `committed_damage`, `committed_full` |
| `tessera.damage.shm` | `shm.snapshot` | `surface`, `window`, `generation`, `width`, `height`, `transform`, `buffer_scale`, `viewport_src`, `viewport_dst`, `surface_extent`, `damage_reported`, `unknown_full`, `geometry_revision`, `reason`, `surface_commit_rects`, `buffer_commit_rects`, `mapped` |
| `tessera.damage.assess` | `damage.surface` / `damage.assess` | per-surface `id`, `generation`, `damage_full`, `damage_rects`, `placement`; aggregate `surfaces`, `rects`, `area` |
| `tessera.damage.upload` | `shm.upload` | `id`, `window`, `generation`, `full`, `damage_rects`, `mode` (`full`/`partial`/`none`), `box_*` |

Every event carries `metric` and `count = 1`. A tracing layer can therefore
treat them as counters without a second telemetry type:
`commit.full`, `shm.snapshot.unknown_full`, `shm.upload.none`, and so on. The
`reason` field distinguishes `unknown_full`, `snapshot_mismatch`,
`reported_empty`, and `incremental`; do not infer the reason from the rectangle
count alone.

`window` is the stable `WindowId` when the surface belongs to a toplevel;
`surface` is the Wayland resource pointer used as the renderer cache identity.
Use both: the pointer correlates with `SurfacePixels::id`, the window id
correlates with user-visible clients.
