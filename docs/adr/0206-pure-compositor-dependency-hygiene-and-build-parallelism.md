---
id: ADR-0206
title: "Pure Compositor Dependency Hygiene and Build Pipeline Parallelization"
status: accepted
date: 2026-11-03
scope: workspace, build, dependencies, governance
superseded_by: null
negative_knowledge: true
---

# 0206. Pure Compositor Dependency Hygiene and Build Pipeline Parallelization

- Status: Accepted
- Date: 2026-11-03
- Deciders: Tessera Maintainers
- Amends: [ADR-0159](0159-package-boundaries-follow-capabilities.md)
- Related: [ADR-0026](0026-configuration-system.md), [ADR-0070](0070-svg-cursors-with-bundled-bibata-fallback.md)

---

## Context and Problem Statement

Local development builds suffered from persistent single-threaded pauses where the CPU fell back to 100% utilization on a single core while all other cores sat idle. A fresh compilation of workspace dependencies took over four minutes, with lengthy freezes before workspace crates could begin compiling.

Inspection of the build topology revealed five concurrent architectural defects:

1. **Proc-Macro O3 Serialization**:
   `Cargo.toml` configured `syn`, `quote`, `proc-macro2`, `serde_derive`, `thiserror-impl`, and `zbus_macros` with `opt-level = 3` under `[profile.dev.package]`. Because `syn` is a massive crate (>60,000 LOC) on the critical dependency path of nearly every derive macro in the workspace, compiling `syn` with aggressive LLVM `-O3` passes took 25–40 seconds on a single core. During this period, all downstream crates requiring `#[derive(Serialize)]`, `#[derive(Deserialize)]`, `#[derive(Error)]`, or `#[derive(Parser)]` were completely blocked, collapsing Cargo's build concurrency to 1.
2. **Blanket LLVM -O2 on All Dependencies**:
   `[profile.dev.package."*"] opt-level = 2` forced LLVM loop vectorization, function cloning, and full optimization passes across all 386 transitive dependencies.
3. **Sequential Protocol Code Generation**:
   `crates/tessera-wayland-protocols/build.rs` executed `wayland-scanner private-code` 24+ times in a synchronous single-threaded loop, halting downstream platform and wayland crates while processes were spawned one by one.
4. **TOML and XKB Version Fragmentation**:
   `Cargo.toml` pinned `toml_edit = "0.22"` at the workspace root despite a manifest comment noting that the TOML 1.x stack standardized on `toml_edit 0.25`. This pulled two incompatible copies of `winnow` (0.6 and 1.0), `toml_datetime`, and `toml_parser` into the build graph. Simultaneously, `xkbcommon = "0.9"` clashed with `smithay-client-toolkit`'s pin of `0.8`, forcing two independent builds of the XKB translation library.
5. **Accumulated Dead Dependencies**:
   Multiple workspace crates carried obsolete dependency declarations (`thiserror` in `tessera`, `tessera-composite`, `tessera-authority`; `libc` in `tessera-authority`; `serde` and `toml` in `tessera-shell`; `lens` in `tessera-avatar`; `serde_json` in `tessera-primitives`; `serde` and `walkdir` in `xtask`), adding spurious crate edges to Cargo's build planner.

## Decision Drivers

- **Maximum Parallel Concurrency**: The build dependency graph must not serialize on macro compilation. Macro execution in dev builds is measured in milliseconds; spending 30 seconds optimizing a macro compiler is an unacceptable trade-off.
- **Dependency Hygiene and Zero Duplication**: Redundant major versions of parser and protocol primitives must be eradicated. Every workspace member must strictly inherit shared versions.
- **Fast Build Script Execution**: Code generation in `build.rs` scripts must exploit host multicore capability.

## Decision Outcome

### 1. Proc-Macro Dev Profile Optimization Removed

In `Cargo.toml`, `opt-level = 3` overrides for `syn`, `quote`, `proc-macro2`, `serde_derive`, `thiserror-impl`, and `zbus_macros` are removed. They now build at the default dev level without triggering expensive LLVM optimization pipelines. High-computation pixel/raster engines (`resvg`, `tiny-skia`, `usvg`, `image`) retain `opt-level = 2` so software rendering does not drop frame pacing. Blanket dependency optimization is set to `opt-level = 1` to preserve runtime inlining without `-O2` compile pauses.

### 2. Parallel Protocol Code Generation

`crates/tessera-wayland-protocols/build.rs` uses `std::thread::scope` to parallelize all `wayland-scanner` invocations across available worker threads. Protocol code generation drops from ~1.5s sequential execution to sub-100ms.

### 3. Unified Dependency Versions

- `toml_edit` is unified at `0.25` across workspace and `tessera-config`, removing duplicate `winnow` 0.6 trees.
- `xkbcommon` is unified at `0.8` across workspace and `tessera-wayland`, eliminating the split 0.8/0.9 dependency.
- All 25 member packages inherit `workspace = true` declarations for `ash`, `cc`, `pkg-config`, `tempfile`, and `toml_edit`.

### 4. Dead Dependency Elimination

All unused dependencies identified by `cargo-machete` across `tessera`, `tessera-authority`, `tessera-shell`, `tessera-composite`, `tessera-avatar`, `tessera-primitives`, and `xtask` are removed from both package manifests and `dependency-policy.toml`.

### 5. Development sessions default to an optimized profile

`scripts/run-nested-dev.sh` and `scripts/run-drm-dev.sh` default to
`--profile release-fast` (inherits `release`, keeps full debug info, disables
LTO). `TESSERA_DEV_PROFILE=debug` is opt-in. The README and Getting Started
tutorial use the same profile for their `cargo run` examples.

`opt-level = 1` is the right trade for edit-compile latency, but it is not a
measurement of compositor pacing: a 3072×1920 output at 120 Hz spends the whole
8.3 ms budget in the frame loop before any GPU work. Judging frame drops from a
debug binary measures the compiler's choices, not the architecture.

## Negative Knowledge

- **Do not benchmark or judge frame pacing from `cargo run`'s dev profile**: the
  dev profile deliberately trades runtime for rebuild latency. Use
  `release-fast` for any interactive or pacing check.
- **Do not set `opt-level = 3` on proc-macro crates in dev profiles**: The theory that "fast macro execution amortizes slow macro compilation" only holds when code is edited hundreds of times without rebuilding dependencies. In practice, clean builds, CI runs, toolchain updates, and feature changes pay a 30-second single-threaded compilation tax on `syn` that dwarfs any runtime savings.
- **Do not declare raw versions in member manifests when workspace dependencies exist**: Individual crates declaring uninherited versions (e.g. `xkbcommon = "0.9"` vs `0.8`) silently fragment the workspace lockfile and force redundant compilation of multiple major/minor lines.
