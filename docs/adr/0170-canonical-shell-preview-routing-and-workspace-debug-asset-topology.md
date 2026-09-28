---
id: ADR-0170
title: "Canonical Shell Preview Routing and Workspace Debug Asset Topology"
status: accepted
date: 2026-09-29
scope: shell, runtime, preview, assets, persona, avatar
superseded_by: null
negative_knowledge: true
---

# 0170. Canonical Shell Preview Routing and Workspace Debug Asset Topology

- Status: Accepted
- Date: 2026-09-29
- Deciders: Tessera Maintainers & Core Architects
- Amends: [ADR-0080](0080-avatar-crate-xdg-conformant-vrm-aware.md), [ADR-0106](0106-shared-identity-portrait-contract-and-vrm-renderer-boundary.md), [ADR-0111](0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md), [ADR-0151](0151-pivot-unified-intent-and-action-surface.md), [ADR-0158](0158-clean-break-workspace-boundaries.md)

---

## Context and Problem Statement

As the Tessera desktop compositor evolved through successive architectural refactorings (notably the decoupling of the headless intent subsystem and Pivot in ADR-0151/0157 and clean-break flat package boundaries in ADR-0158), two developer ergonomics and resource topology anomalies emerged:

1. **Incomplete Component Preview Routing (`TESSERA_PREVIEW`)**:
   `TESSERA_PREVIEW` was established to boot the compositor directly into a mocked, zero-side-effect sandbox with simulated telemetry and an isolated state directory. However, while `control-center`, `overview`, and `switcher` were wired into the runtime dispatch, **Pivot** (the primary system intent and search surface) lacked a preview routing arm. Developers inspecting Pivot styling, search ranking, and motion transitions had to launch the full compositor and trigger input hotkeys (`Super+Space`), defeating the headless and quick-iteration purpose of the preview subsystem.

2. **Crate Pollution and Path Divergence in Debug Assets (`persona-debug-assets`)**:
   During the legacy Aegis era, local 3D VRM models and companion animations (`avatar.vrm`, `avatar.vrma`) were stored inside `crates/aegis-avatar/debug-assets`. When ADR-0111 folded persona into `aegis-shell`, the directory moved to `crates/tessera-shell/persona-debug-assets`. Following ADR-0158's decoupling of `tessera-avatar`, `crates/tessera-avatar` sought debug fixtures at `CARGO_MANIFEST_DIR/persona-debug-assets` while the physical directory remained orphaned in `crates/tessera-shell/`.
   Storing non-code binary test fixtures inside `crates/*` violates crate purity, creates duplicate or desynchronized path lookups across consumer crates, and obscures asset discovery.

---

## Decision

We establish two unified, non-compromising architectural contracts across the workspace:

### 1. Canonical Surface Preview Routing

The compositor entry point (`crates/tessera/src/runtime.rs`) canonicalizes `TESSERA_PREVIEW` routing across all primary human-interaction surfaces:

- **`pivot` (aliases: `launcher`, `spotlight`, `prism`, `apps`)**: Automatically executes `shell.toggle_pivot()` on startup within the mock sandbox.
- **`control-center` (aliases: `control_center`, `command-panel`, `command_panel`, `panel`)**: Opens the modular quick settings and system telemetry bento grid.
- **`overview`**: Dispatches the window spatial overview gesture state.
- **`switcher`**: Engages the live window switcher overlay.

All preview sessions inherit the fail-safe mock backend (`MockHostSystem`, `MockWirelessBackend`) and disposable temporary directory isolation.

### 2. Workspace-Level Asset Topology (`assets/debug/`)

We abolish crate-local debug asset directories (`crates/*/persona-debug-assets`) and canonize a unified workspace asset tree:

```text
assets/
├── cursors/          # Tracked production X11/Wayland SVG cursor themes
├── wallpapers/       # Tracked default desktop wallpapers
├── icons/            # Tracked fallback system iconography
└── debug/            # Untracked or license-restricted developer fixtures
    └── persona/      # VRM avatar models and VRMA motion captures
        ├── .gitignore# Ignores local *.vrm, *.vrma, motions/ binaries
        └── README.md # Documents MoCap origins (e.g. CMU database) and VRM format
```

### 3. Hierarchical Resolution for Debug Persona Fixtures

`tessera-avatar` and `tessera-shell` resolve debug avatar fixtures through a strict three-tier precedence:

1. **Explicit Directory Override (`TESSERA_AVATAR_DEBUG_DIR`)**: Checked first in debug builds (`cfg!(debug_assertions)`). Allows test harnesses, scripts, and CI runners to target arbitrary temporary fixture directories.
2. **Canonical Workspace Asset Root (`assets/debug/persona`)**: Used when `TESSERA_AVATAR_DEBUG_ASSETS=1` is set in debug builds.
3. **Release Zero-Cost Immunity**: Under `--release` builds (`!cfg!(debug_assertions)`), debug asset search branches are permanently disabled at compile time, guaranteeing zero runtime overhead and complete isolation from local developer artifacts.

### 4. Orthogonal Appearance Scheme Override (`TESSERA_COLOR_SCHEME`)

We introduce `TESSERA_COLOR_SCHEME` (with alias `TESSERA_THEME`) into `PreferenceOverrides`:
- Allows instant evaluation of light, dark, or system palettes (`TESSERA_COLOR_SCHEME=light TESSERA_PREVIEW=pivot cargo run -p tessera`) without mutating `config.toml`.
- Strictly isolated in `preferences_for_persistence`: process-level environment overrides are never accidentally persisted to the user's persistent configuration file during runtime transactions.

---

## Invariants & Behavioral Boundaries

- `[INV-ASSET-01] Crate Cleanliness`: Crates under `crates/` must contain only source code, build scripts, crate manifests, and documentation. No binary test fixtures, media files, or ignored debug directories may reside directly inside crate root trees.
- `[INV-ASSET-02] License & Binary Confinement`: Any non-distributable, commercial, or large binary fixtures in `assets/debug/` must be gitignored via targeted `.gitignore` rules while preserving explanatory `README.md` metadata.
- `[INV-PREV-01] Preview Uniformity`: Any interactive modal surface implemented in `tessera-shell` must expose a deterministic startup toggle under `TESSERA_PREVIEW`.
- `[INV-PREV-02] Appearance Orthogonality`: Appearance scheme overrides (`TESSERA_COLOR_SCHEME`) must operate orthogonally to preview surface routing (`TESSERA_PREVIEW`) and never leak into persistent configuration.

---

## Rejected Alternatives & Negative Knowledge

### Force Developers to Use Host XDG Paths (`~/.local/share/tessera/avatars/`)
- **Why considered**: Avoids storing local fixtures inside the git repository worktree altogether.
- **Why rejected**: Violates test and workspace sandbox isolation rules (`AGENTS.md`: *“Never touch user home or global state paths”*). Polluting the developer's host user directory with mock avatars breaks repeatable local development, conflicts with everyday desktop usage, and prevents sandboxed execution.

### Keep Fixtures in `crates/tessera-avatar/persona-debug-assets/`
- **Why considered**: Fixes the path desync between `tessera-avatar` and `tessera-shell` by placing the directory adjacent to the avatar loader.
- **Why rejected**: Perpetuates crate pollution and breaks when companion tools (such as `tessera-shell`'s `debug_avatar` example or compositor preview modes) need access to the same assets. Test assets used across multiple crates belong at workspace scope.

### Hardcode Workspace Root Detection via Upward Directory Traversal
- **Why considered**: Searching upwards for `.git` or `Cargo.toml` dynamically at runtime.
- **Why rejected**: Fragile when running binaries in nested sandboxes, build caches, or containerized environments. Standardizing on `CARGO_MANIFEST_DIR`-relative paths (`../../assets/debug/persona`) combined with explicit `TESSERA_AVATAR_DEBUG_DIR` environment overrides provides deterministic compile-time resolution with testable runtime flexibility.

---

## Consequences

### Positive
- Developers can instantly inspect and iterate on Pivot via `TESSERA_PREVIEW=pivot cargo run -p tessera`.
- Crate directory trees are completely purged of binary debug residue and nested `.gitignore` anomalies.
- `tessera-avatar` and `tessera-shell` share a single source of truth for debug avatars without path desynchronization.
- Clean separation between production-shipped assets (`assets/{cursors,wallpapers,icons}`) and local developer fixtures (`assets/debug/`).

### Negative / Trade-offs
- Developers who previously placed `avatar.vrm` inside `crates/tessera-shell/persona-debug-assets/` must place fixtures in `assets/debug/persona/` or set `TESSERA_AVATAR_DEBUG_DIR`.
