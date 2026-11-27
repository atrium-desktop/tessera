# Local Optics Development

This guide defines how to develop and integrate Tessera against a local sibling
**Optics** checkout. To keep canonical builds isolated from local overrides,
the workflow uses a linked Git worktree: the primary worktree (`main`) stays
canonical and statically linked for production and CI, while the linked
worktree (`dev`) patches the sibling Optics checkout and links shared libraries
for fast iteration.

## Architecture and Invariants

| Concern | Primary worktree (`main`) | Development worktree (`dev`) |
|---------|---------------------------|------------------------------|
| Directory | `../tessera/` | `../tessera-dev/` |
| Branch | `main` | `dev` |
| `Cargo.toml` | Identical across both branches (`features = ["static"]` on native Optics deps) | Same |
| Rust bindings | Tagged Optics Git source | Sibling `../optics/crates/*` via `[patch]` |
| `Cargo.lock` | Canonical, tagged, committed | Worktree-local, never committed |
| Cargo config | `.cargo/config.toml` absent | Copied from [.cargo/optics-local.toml](../../.cargo/optics-local.toml) |
| Native linkage | Static `liboptics_*.a` archives (default `static` feature) | Shared `../optics/build/` via `[env] OPTICS_DYNAMIC = "1"` + `-rpath` |
| Target cache | Primary `target/` | Worktree-local `target/` |

### Core Invariants

1. **Identical Manifests, Worktree-Local Overrides**: `main` and `dev` never
   diverge on `Cargo.toml` link settings. `Cargo.toml` enables
   `features = ["static"]` everywhere so canonical and production builds embed
   Optics archives by default; only `tessera-dev`'s untracked
   `.cargo/config.toml` overrides the mode with `OPTICS_DYNAMIC = "1"`.
2. **Optics Upstream First**: `main` must depend solely on an immutable,
   publicly tagged Optics release — never on local or uncommitted Optics state.
3. **Always-Buildable Main (Atomic Promotion)**: Every commit on `main` must
   build independently with `cargo check --locked --workspace`. Feature code,
   the Optics tag bump in `Cargo.toml`, and the regenerated canonical
   `Cargo.lock` land together in one atomic squash commit.
4. **Target Directory Isolation**: Keep separate `target/` directories per
   worktree; do not share `CARGO_TARGET_DIR` across canonical and patched trees.
5. **Version Authority**: Internal workspace version bumps
   (`workspace.package.version`) and upstream Optics tag bumps are strictly
   governed by explicit user instruction. Tooling (`cargo run -p xtask -- optics --set <TAG>`)
   executes approved upgrades and never decides version policy.

## Workspace Setup

Place the repositories under a common parent directory:

```text
projects/
├── tessera/       # Primary worktree on main (canonical, static Optics)
├── tessera-dev/   # Linked worktree on dev (local patch, dynamic Optics)
└── optics/        # Sibling Optics checkout
```

1. **Create the linked worktree** from `projects/tessera/`:
   ```bash
   git worktree add -b dev ../tessera-dev main
   ```
2. **Enable local Optics mode and hooks** inside `projects/tessera-dev/`:
   ```bash
   cd ../tessera-dev
   cp .cargo/optics-local.toml .cargo/config.toml
   git config core.hooksPath .githooks
   ```
   [.cargo/optics-local.toml](../../.cargo/optics-local.toml) redirects every
   `ming2k/optics` crate to `../optics/crates/*` and sets `[env] OPTICS_SOURCE_DIR`,
   `OPTICS_BUILD_DIR`, and `OPTICS_DYNAMIC = "1"`, overriding the workspace
   `static` feature so local builds discover
   `../optics/build/meson-uninstalled/*.pc` and inject `-Wl,-rpath`.
3. **Build Optics and resolve the local graph**:
   ```bash
   meson setup ../optics/build ../optics -Dtests=false -Dbuildtype=debugoptimized  # first time only
   meson compile -C ../optics/build
   cargo check -p tessera
   cargo tree -i optics-gfx   # confirm path points to ../optics/crates/optics-gfx
   ```

## Daily Development (`tessera-dev`)

Run all day-to-day commands inside `projects/tessera-dev/`:

```bash
# Recompile Optics after C or shader edits (zero Rust relink when headers are unchanged):
meson compile -C ../optics/build

# Build and test Tessera against the shared libraries in ../optics/build:
cargo check -p tessera
cargo nextest run -p tessera

# After `../optics` bumps its workspace version ahead of `Cargo.toml`:
cargo update -p optics-build -p optics-gfx -p optics-gfx-sys -p optics-ui \
  -p optics-ui-sys -p optics-motion

# Verify the production static closure inside dev on demand:
OPTICS_DYNAMIC=0 cargo build -p tessera
```

Do not run concurrent Meson/Ninja builds against `../optics/build` from
multiple terminals.

Leave the locally patched `Cargo.lock` unstaged when committing on `dev`. Never
bypass the hook with `--no-verify` — [.githooks/pre-commit](../../.githooks/pre-commit)
is strictly read-only and refuses commits that stage `Cargo.lock` or
`.cargo/config.toml` so local overrides never pollute commits, rebases, or
squash merges.

## Feature-Level Atomic Promotion to `main`

`dev` holds fine-grained WIP commits built against local Optics; `main` holds
curated, individually buildable atomic commits. Never fast-forward `main` onto
`dev`.

### 1. Publish the Optics Tag First

In `projects/optics/` (only after explicit tag approval):

```bash
cd ../optics
git checkout main && git pull --ff-only
git tag -v vX.Y.Z || git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
meson compile -C build
```

### 2. Squash, Re-pin, Verify, and Commit on `main`

In `projects/tessera/` (primary worktree, no `.cargo/config.toml`):

```bash
cd ../tessera
git pull --ff-only

# Squash dev changes into the staging area (do not commit yet):
git merge --squash dev

# Move all Optics workspace tags and regenerate canonical Cargo.lock:
cargo run -p xtask -- optics --set vX.Y.Z
OPTICS_SOURCE_DIR=../optics OPTICS_BUILD_DIR=../optics/build cargo check -p tessera

# Confirm remote Git resolution (no local ../optics paths):
cargo tree -i optics-gfx
cargo tree -i optics-ui

# Full locked static verification (omit OPTICS_* if matching Optics is installed system-wide):
OPTICS_SOURCE_DIR=../optics OPTICS_BUILD_DIR=../optics/build cargo check --locked --workspace
OPTICS_SOURCE_DIR=../optics OPTICS_BUILD_DIR=../optics/build cargo nextest run --locked --workspace
OPTICS_SOURCE_DIR=../optics OPTICS_BUILD_DIR=../optics/build cargo build --locked --release -p tessera

# Single atomic promotion commit:
git commit -m "feat(<scope>): <summary>

Adopt Optics vX.Y.Z."
git push origin main
```

For production host installation from `main`, run
[scripts/install.sh](../../scripts/install.sh): it compiles release static
archives in `../optics/build-static-release`, builds release binaries with
`OPTICS_STATIC=1`, and runs `readelf` checks to reject any shared Optics
dependency or non-system `RPATH` before installing.

### 3. Reset `dev` onto `main`

Because `dev`'s changes now live inside the squash commit on `main`, reset
rather than rebase (`tessera-dev/.cargo/config.toml` stays untouched):

```bash
cd ../tessera-dev
git reset --hard main
cargo check -p tessera   # regenerates the local patched Cargo.lock
```

If `dev` is tracked on a remote, update it with `git push --force-with-lease origin dev`.

## Workspace Release Tagging

When cutting a Tessera release (`vX.Y.Z`) on `main`:

```bash
cd ../tessera
# Update workspace.package.version in Cargo.toml and CHANGELOG.md, then:
OPTICS_SOURCE_DIR=../optics OPTICS_BUILD_DIR=../optics/build cargo check --locked --workspace
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): vX.Y.Z"
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin main --follow-tags
```

Then resynchronize `tessera-dev` with `git reset --hard main && cargo check -p tessera`.

## Git Hook Guard and Maintenance

[.githooks/pre-commit](../../.githooks/pre-commit) is strictly read-only — it
never mutates the Git index — and switches behavior based on whether
`.cargo/config.toml` contains `[patch."https://github.com/ming2k/optics"]`:

- **Always**: refuses a staged `.cargo/config.toml`.
- **Local Optics mode (`tessera-dev/`)**: refuses staged `Cargo.lock` changes
  and release-shaped version bumps in `Cargo.toml`.
- **Canonical mode (`tessera/`)**: validates `cargo metadata --locked` whenever
  `Cargo.toml` or `Cargo.lock` is staged.

### Rebasing Unpromoted `dev` Work When `main` Advances

```bash
cd ../tessera && git pull --ff-only
cd ../tessera-dev
git restore Cargo.lock
git rebase main
cargo check -p tessera
```

### Recovering Canonical State

If a worktree's `Cargo.lock` is accidentally polluted by local paths:

```bash
rm -f .cargo/config.toml
git restore Cargo.lock
cargo check --locked --workspace
```

## See Also

- [Setup](setup.md) — prerequisites, contributor vs dual-maintainer setup, and troubleshooting.
- [Distribution Packaging](packaging.md) — vendoring, install manifest, and shared-linking packager overrides.
