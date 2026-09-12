# Setup

How to build and run tessera for development.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | `rustc` and `cargo`, edition 2024 (1.88+) |
| Optics `<OPTICS_VERSION>` | Use a sibling checkout for cross-repository development or installed native libraries for the canonical build |
| meson and a C23 compiler | To build the optics libraries |
| Vulkan 1.3 runtime and loader | flux is Vulkan-first |
| Wayland client and protocols | `wayland-client`, `wayland-protocols`, and `wayland-scanner` for the nested backend |
| libxkbcommon | Server compiles the default keymap at startup for `wl_keyboard` |
| A running Wayland session | `$WAYLAND_DISPLAY` must be set to run the nested backend |
| bubblewrap + systemd user manager | Required for real Interaction Domain application sandbox tests |

## Choose your workflow

Tessera development splits into two roles. Pick the row that describes you;
everything below this table follows from it.

| You are… | Workflow | Why |
|----------|----------|-----|
| **Contributing to Tessera only** | [Canonical workflow](#canonical-workflow-contributor) | You never touch Optics, so the locked `v<OPTICS_VERSION>` bindings and the system-installed native libraries are all you need. No sibling checkout, no Cargo patch. |
| **Maintaining Tessera and Optics together** | [Local workflow](#local-workflow-dual-maintainer) | You isolate the live sibling Optics patch, lockfile, and target directory in a linked Tessera worktree. |

### How the two roles differ

Tessera deliberately keeps Rust source selection separate from native library
discovery. That separation is what makes both roles coexist on the same
checkout without one forcing its setup on the other:

| Concern | Canonical workflow | Local workflow |
|---------|--------------------|----------------|
| Rust bindings | Locked Optics `v<OPTICS_VERSION>` Git source | `[patch]` entries for `../optics/bindings` |
| Native libraries | System `pkg-config` and dynamic-loader paths | The sibling uninstalled Meson tree (via `meson-uninstalled/*.pc`) |
| `.cargo/config.toml` | Absent | A copy of `.cargo/optics-local.toml` |
| `Cargo.lock` | Canonical and committed | Worktree-local and excluded from commits |
| `target/` | Primary worktree cache | Linked-worktree cache |
| Sibling `../optics` required | No | Yes |

The root `Cargo.toml` always records the canonical Git dependencies.
`.cargo/optics-local.toml` is an explicit, opt-in development override, so an
independent Tessera checkout never requires a sibling repository. Distribution
packaging and CI always use the canonical workflow; see
[Distribution Packaging](packaging.md).

### Local workflow (dual maintainer)

For maintainers who edit Tessera and Optics together. Keep the primary Tessera
worktree in canonical mode and create one long-lived linked development
worktree:

```bash
git worktree add -b dev ../tessera-dev main
cd ../tessera-dev
cp .cargo/optics-local.toml .cargo/config.toml
git config core.hooksPath .githooks
```

Run these commands only in the linked worktree. They install the local
`[patch]` configuration and enable the repository commit hook. The hook
preserves ordinary `git add .` usage by automatically removing the local
`Cargo.lock` and `.cargo/config.toml` from the staged set. Keep the worktree
on its long-lived local `dev` branch and fast-forward `main` to completed
`dev` commits.

The Rust bindings build against the unified optics Meson build tree, so build
`libflux`, `libflux-scene-graph`, `liblens`, and `libiris` first:

```bash
meson compile -C ../optics/build
```

If a build tree does not exist yet, configure it once before compiling:

```bash
meson setup ../optics/build ../optics -Dtests=false -Dbuildtype=debugoptimized
```

`debugoptimized` keeps assertions but compiles the C libraries with `-O2`;
a plain `debug` (`-O0`) build is visibly slow on HiDPI outputs. An existing
tree can be switched in place with `meson configure ../optics/build
-Dbuildtype=debugoptimized && meson compile -C ../optics/build`.

The `-sys` build scripts locate the tree through
`meson-uninstalled/flux-uninstalled.pc` and
`meson-uninstalled/flux-scene-graph-uninstalled.pc`, plus
`meson-uninstalled/lens-uninstalled.pc` and
`meson-uninstalled/iris-uninstalled.pc`.

Resolve the patched graph once without `--locked`, then use locked commands
until an Optics manifest changes:

```bash
cargo check -p tessera
cargo check --locked --workspace
cargo test --locked --workspace
```

The local lockfile and target directory remain inside the linked worktree.
Follow
[Tessera and Optics Cross-Repository Development](cross-repository-development.md)
for daily commits, rebases, release promotion, and merging `dev` into
`main`.

### Canonical workflow (contributor)

For everyone contributing to Tessera only. You do **not** need a sibling
`optics` checkout. Build the matching Optics `<OPTICS_VERSION>` once so its native
libraries, headers, and `.pc` files land in the system paths, then drive Tessera
with ordinary `cargo` commands against the locked bindings.

**One-time Optics install** (or upgrade it whenever a project bump moves to a
new Optics tag):

```bash
meson setup ../optics/build-release ../optics \
  -Dtests=false --buildtype=release
meson compile -C ../optics/build-release
sudo meson install -C ../optics/build-release
sudo ldconfig
pkg-config --modversion flux flux-scene-graph lens iris   # sanity check
```

> If your distribution already ships an Optics `<OPTICS_VERSION>` package, install that
> instead and skip the manual build. What matters is that the four
> `pkg-config --modversion` checks above each report an `<OPTICS_VERSION>`-compatible
> version.

**Daily development** needs no further Optics work — just Cargo:

```bash
cargo run --locked -p tessera          # build & run (see Build and run below)
cargo test --locked --workspace
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

This is the exact boundary the full CI job exercises: it installs the tagged
Optics C libraries, verifies their `pkg-config` metadata, and builds the
locked remote Rust bindings without any local Cargo override.

Building distribution packages is a separate concern with its own rules
(vendoring, offline builds, install manifests, integration triggers). Do not
apply them to contributor development; see
[Distribution Packaging](packaging.md) instead.

## Build and run

Run the compositor from a terminal in an existing Wayland session:

```bash
cargo run --locked -p tessera
```

The default `TESSERA_BACKEND=auto` selects nested presentation when
`$WAYLAND_DISPLAY` is present and direct DRM/KMS otherwise. Set
`TESSERA_BACKEND=nested` or `TESSERA_BACKEND=drm` only when a test must force one
backend.

The development commands have distinct responsibilities:

| Command | Role |
|---------|------|
| `cargo run --locked -p tessera` | Build and run the compositor with automatic backend selection |
| `TESSERA_BACKEND=drm cargo run --locked -p tessera` | Force direct-display testing |

Persistent settings have no separate binary: the settings pages are tabs in
the command panel (`Super+S`), hosted in-process from the `tessera-settings`
module library. Distribution installation,
which owns systemd units, D-Bus services, and portal metadata, is documented
separately in [Distribution Packaging](packaging.md).

The compositor creates a `VkSurfaceKHR` on flux's Vulkan instance and presents
the shell. Local-workflow binaries and test harnesses re-emit the uninstalled
build-tree rpaths published by the binding crates. Canonical builds use the
system dynamic-loader configuration.

Use the [Nested Backend Development](nested-backend.md) workflow for daily
iteration, Cargo command selection, inner client launch, and the boundary
between nested and DRM/KMS validation.

The compositor logs through the `log` facade; `RUST_LOG` controls verbosity
(default `info`):

```bash
RUST_LOG=debug cargo run --locked -p tessera
RUST_LOG=warn cargo run --locked -p tessera
```

## Screenshot PNG benchmark

Run the privacy-safe generated-scene comparison in an optimized build:

```bash
cargo run --locked -p tessera-capture --example png_profiles --profile release-fast
```

The example prints CSV for RGB and RGBA with Fast+Up, Fast+Adaptive,
Default+Adaptive, and Fast+Paeth at 1920×1080 and 3840×2160. Scenes contain
pseudo-text/UI panels, smooth gradients, and deterministic textured
photo-like colors; these are synthetic workloads, not real desktop captures
or photographs. No screen access, input files, or new dependencies are needed.
Every combination is decoded and checked against its original RGBA pixels.

Each case has one warm-up and five measured encodes (median/min/max in
milliseconds). Timing includes opacity scanning and in-place RGB compaction
for RGB cases, plus PNG encoding and output allocation; it excludes scene
generation, input cloning, decode validation, GPU readback, unpremultiplication,
publication, and disk I/O. The RGB preparation mirrors the encoder's owned
buffer path. Run on an otherwise idle machine and retain the CSV with CPU,
Rust/image versions, and build profile when comparing changes.

A sample run on an Intel Core Ultra 9 285H, Rust 1.97.1, image 0.25.8,
png 0.18.1, `release-fast` produced these 3840×2160 results. Cells contain
encoded bytes / median milliseconds; RGB timings include compaction.

| Scene / channels | Fast+Up | Fast+Adaptive | Default+Adaptive | Fast+Paeth |
|---|---:|---:|---:|---:|
| UI / RGBA | 6,121,533 / 6.60 | 945,857 / 18.33 | 89,466 / 50.21 | 1,245,039 / 7.76 |
| UI / RGB | 5,798,097 / 13.17 | 879,030 / 21.66 | 73,537 / 45.91 | 1,179,408 / 13.44 |
| Gradient / RGBA | 1,563,109 / 6.52 | 511,808 / 36.47 | 203,180 / 79.20 | 511,808 / 8.51 |
| Gradient / RGB | 1,317,502 / 12.83 | 511,424 / 35.22 | 186,893 / 80.42 | 511,424 / 13.10 |
| Photo-like / RGBA | 23,380,914 / 41.57 | 22,680,728 / 73.26 | 10,828,634 / 1,656.39 | 23,571,535 / 47.75 |
| Photo-like / RGB | 21,311,118 / 46.64 | 20,608,186 / 66.26 | 10,563,750 / 1,104.61 | 21,502,061 / 45.09 |

Fast+Paeth is the fixed screenshot default: it substantially reduces
UI/gradient size compared with Fast+Up without Adaptive's filter-search
cost or Default's texture compression latency. It is not universally
smallest or fastest. RGB further reduces bytes but its scan/compaction can
increase CPU time; it reuses the owned RGBA allocation rather than allocating
another full frame. Against the former RGBA Fast+Up baseline, RGB Fast+Paeth
reduced these 4K samples by about 81%, 67%, and 8%, respectively, with
median times increasing by about 7, 7, and 4 ms. At 1080p the same comparison
was 1,512,389→292,676, 744,217→244,719, and 5,846,577→5,374,365 bytes.
These measurements justify a size/interactive-latency tradeoff on this
corpus, not a performance guarantee for real desktops or other CPUs.

## Tests

```bash
cargo test --locked --workspace
```

`tessera-model` and `tessera-compositor` unit tests run without the flux dependency;
the rest need either the sibling Optics Meson tree in the local workflow or
the installed libraries in the canonical workflow.

The ordinary workspace run skips kernel-level Interaction Domain launcher tests when the
test process is not alone in a controller-delegated cgroup. Run those tests
in the production topology with:

```bash
scripts/test-interaction-domain-sandbox.sh
```

The script starts the compiled `tessera-launcher` test binary as a transient
systemd user service with delegated `cpu`, `memory`, and `pids` controllers.
It verifies mount-scoped multi-connection Wayland portals, mandatory resource
limits, cgroup freeze/resume, and `cgroup.kill` against a worker that escapes
its process group.

## Troubleshooting

| Symptom | First check |
|---------|-----|
| `cannot connect to host Wayland display` | `$WAYLAND_DISPLAY` is unset or points at no compositor |
| `$XDG_RUNTIME_DIR is unset` | Log in through PAM/logind; do not create a shared runtime directory under `/tmp` |
| DRM runner rejects root | Log in as the normal seat user; do not bypass logind or seatd with `sudo` |
| Missing a `flux`, `flux-scene-graph`, `lens`, or `iris` pkg-config file | In the local workflow, build the sibling tree; in the canonical workflow, install the matching Optics release |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` / `liblens*.so` / `libiris*.so` | In the local workflow, rebuild after moving the Meson tree; in the canonical workflow, refresh the loader cache or configure the installed prefix |
| `Interaction Domain cgroup isolation is unavailable` | Run Tessera in the packaged systemd user service; a shared terminal scope cannot satisfy controller delegation |

## See Also

- [Tessera and Optics Cross-Repository Development](cross-repository-development.md)
- [Project Layout](project-layout.md)
- [First-Party Application Development](first-party-applications.md)
- [VT/DRM Manual Testing](vt-drm-testing.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
