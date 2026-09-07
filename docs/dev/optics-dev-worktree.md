# Optics Development Worktree Workflow

This guide establishes the standard workflow for developing Tessera alongside
its companion C rendering and material engine, **Optics**. It uses a dual
Git worktree architecture to combine zero-friction local iteration on the
development branch with strict, reproducible builds on `main`.

## Architecture and Invariants

Cross-repository development splits responsibilities across two linked Git
worktrees sharing a single repository storage:

| Concern | Primary worktree (`main`) | Development worktree (`dev`) |
|---------|---------------------------|------------------------------|
| Directory path | `../tessera/` | `../tessera-dev/` |
| Active branch | `main` | `dev` |
| Rust bindings | Tagged Optics Git source | Sibling `../optics` via `[patch]` |
| `Cargo.lock` | Canonical, tagged, committed | Worktree-local, never committed |
| Cargo config | `.cargo/config.toml` absent | Copied from `.cargo/optics-local.toml` |
| Native libraries | System-installed `/usr/lib/lib*.so` | Sibling `../optics/build/` via `-rpath` |
| Target cache | Primary `target/` | Worktree-local `target/` |

### The Foundational Principle: Truth on Main vs Local Illusion

A feature compiling and passing tests in `tessera-dev` is often a **local
illusion**: it succeeds only because Cargo and the runtime secretly link
against the privileged, uncommitted state in your sibling `../optics/build`
directory.

The foundational principle of this dual-worktree architecture is:
> **`main` represents external reality. It must genuinely build and run in
> the wild for users, CI, and package managers—without depending on any
> developer's private machine state.**

Every rule in this workflow (Optics Upstream First, canonical manifest
updates, and locked lockfile validation) exists solely to prevent local
development privilege from masquerading as a functional release on `main`.

### Core Invariants

1. **Local Patch Containment**: `.cargo/config.toml` and path-resolved
   `Cargo.lock` belong strictly to the `tessera-dev` worktree. The tracked
   pre-commit hook automatically excludes them from regular commits.
2. **Optics Upstream First**: Any feature merged into `main` that relies on
   new or modified Optics APIs must depend solely on a tagged, public
   Optics commit. `main` must never reference uncommitted or local-only
   Optics revisions.
3. **Always-Buildable Main**: Every commit on `main` must be individually
   buildable with `cargo check --locked --workspace`. Canonical lockfiles
   are updated alongside feature promotions, never left to guess at release.

Separate target directories are required. Do not configure a shared
`CARGO_TARGET_DIR` across worktrees, as mixing incremental compilation
artifacts between canonical Git dependencies and local path overrides leads
to cache invalidation churn.

## Workspace Setup

### Directory Topology

Place all repositories under a common parent directory:

```text
projects/
├── tessera/       # Primary worktree on main (canonical)
├── tessera-dev/   # Linked development worktree on dev (local patch)
└── optics/        # Sibling checkout of the Optics C engine
```

### 1. Create the Linked Worktree

Run from the primary `tessera` repository (`../tessera/`, branch `main`):

```bash
# In projects/tessera/ (branch main):
git worktree add -b dev ../tessera-dev main
```

### 2. Activate Local Optics Mode

Enter the development worktree, copy the local patch configuration, and
install the repository-owned Git hooks:

```bash
# In projects/tessera-dev/ (branch dev):
cd ../tessera-dev
cp .cargo/optics-local.toml .cargo/config.toml
git config core.hooksPath .githooks
```

Verify that the sibling Optics repository and Meson build exist:

```bash
# In projects/tessera-dev/ (branch dev):
test -f ../optics/meson.build
meson compile -C ../optics/build
```

#### Understanding `.cargo/optics-local.toml`

The repository tracks `.cargo/optics-local.toml` as the reviewed template for
local development. Because `.cargo/config.toml` is ignored by Git to keep
worktree state isolated, copying this template activates source-replacement
without dirtying version control:

```toml
[patch."https://github.com/ming2k/optics"]
flux = { path = "../optics/bindings/flux-rs/crates/flux" }
flux-sys = { path = "../optics/bindings/flux-rs/crates/flux-sys" }
lens = { path = "../optics/bindings/lens-rs/crates/lens" }
prism = { path = "../optics/bindings/prism-rs/crates/prism" }
iris = { path = "../optics/bindings/iris-rs/crates/iris" }
```

This configuration achieves two things:

1. **Cargo Source Replacement**: It instructs Cargo to intercept every
   dependency on the remote `ming2k/optics` repository and redirect it to
   the local sibling directory `../optics/`.
2. **Native Discovery and Precedence**: Loading `*-sys` crates locally
   causes their `build.rs` scripts to run from disk. These scripts detect
   `../optics/build/meson-uninstalled/`, prepend it to `PKG_CONFIG_PATH`,
   and inject `-Wl,-rpath` so that uninstalled local C libraries take
   absolute precedence over any system-installed versions.

### 3. Resolve the Local Dependency Graph

Run an initial check in `tessera-dev` to populate the local worktree
lockfile with path dependencies:

```bash
# In projects/tessera-dev/ (branch dev):
cargo check -p tessera
```

Verify that Cargo resolves the sibling paths instead of Git tags:

```bash
# In projects/tessera-dev/ (branch dev):
cargo tree -i flux
cargo tree -i lens
```

Both trees must display paths pointing into `../optics/bindings/`.

## Daily Development in Local Mode

> **Active Worktree**: All operations in this section execute inside the
> development worktree (`../tessera-dev/`, branch `dev`), interacting with
> the sibling directory `../optics/`.
>
> Do not run these daily development commands in the primary `tessera/`
> (`main`) worktree.

### Compiling and Testing

When modifying Optics and Tessera simultaneously:

1. Recompile the native C libraries whenever Optics C sources change:
   ```bash
   # From either directory, build the shared Meson tree:
   meson compile -C ../optics/build
   ```
2. Build and test Tessera in the development worktree. The `-sys` crates
   automatically inject `-Wl,-rpath` pointing to `../optics/build`,
   resolving uninstalled `.so` files at runtime:
   ```bash
   # In projects/tessera-dev/ (branch dev):
   cargo check -p tessera
   cargo test -p tessera
   ```

Do not run concurrent Meson or Ninja builds against `../optics/build` from
multiple terminals, as the build output directory is a shared write location.

### Committing Tessera Changes

Stage and commit changes on `dev` as usual:

```bash
# In projects/tessera-dev/ (branch dev):
git add .
git commit -m "feat(dock): improve intent dwell hit testing"
```

The tracked `.githooks/pre-commit` hook automatically unstages:
- `Cargo.lock` (mutated by the local patch); and
- `.cargo/config.toml` (if accidentally staged).

The commit proceeds with only your Tessera source changes. Never bypass this
guard with `--no-verify`.

## Feature-Level Merge to Main

Promote changes from `dev` to `main` at **feature-level boundaries** rather
than accumulating a monolithic, unreviewable multi-month release dump.

### Relationship Between `dev` and `main` Commit Histories

The commit history on `dev` and `main` is **deliberately distinct**:

- **`dev` history**: Contains rapid, exploratory, and fine-grained iteration
  commits created during co-development with local Optics.
- **`main` history**: Contains curated, atomic feature commits that are
  guaranteed to build independently with `cargo check --locked --workspace`,
  pinned to canonical remote Git tags.

Do not treat this process as a bidirectional "sync". It is a strict
**one-way promotion and merge** of completed feature milestones from `dev`
into `main`.

Follow this protocol whenever a completed feature on `dev` depends on new or
updated Optics functionality:

### Step 1: Upstream Optics First

Land and tag the required changes in `../optics` before making Tessera
canonical:

```bash
# In projects/optics/ (Optics repository, branch main):
cd ../optics
git checkout main
git pull
# Ensure the release tag exists and the build passes:
git tag -v vX.Y.Z || git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
meson compile -C build
```

### Step 2: Merge Feature Commits into Main Worktree

Switch to the primary `tessera/` worktree (`branch main`). Because `main`
deliberately lacks `.cargo/config.toml`, it operates natively in canonical
mode without disabling or toggling any local configurations.

Merge the completed feature commits from `dev`:

```bash
# In projects/tessera/ (primary worktree, branch main):
cd ../tessera
git switch main
git pull --ff-only
git merge dev
```

### Step 3: Update Manifests and Canonical Lockfile on Main

If the merged feature requires new Optics APIs, update every Optics dependency
in `projects/tessera/Cargo.toml` to the new tag `vX.Y.Z`:

```bash
# In projects/tessera/ (primary worktree, branch main):
scripts/optics-release-ref.sh
```

Regenerate the canonical `Cargo.lock` directly in the primary worktree.
Because there are no local path overrides, Cargo naturally connects to
GitHub and pins the authoritative remote Git SHA:

```bash
# In projects/tessera/ (primary worktree, branch main):
cargo check -p tessera
```

Confirm that `cargo tree -i flux` and `cargo tree -i lens` report the tagged
Git source instead of local filesystem paths.

### Step 4: Validate and Commit Canonical State on Main

Verify that the canonical tree compiles cleanly under `--locked`:

```bash
# In projects/tessera/ (primary worktree, branch main):
cargo check --locked --workspace
cargo test --locked --workspace
cargo build --locked -p tessera
```

Commit the canonical dependency update directly on `main` and push:

```bash
# In projects/tessera/ (primary worktree, branch main):
git add Cargo.toml Cargo.lock
git commit -m "build: adopt Optics vX.Y.Z"
git push origin main
```

### Step 5: Continue in Dev Worktree Without Interruption

Notice that **`tessera-dev` never touched or disabled its `.cargo/config.toml`**.
Your local development environment remained active throughout the merge.

To bring `dev` up to date with the newly adopted Optics tag:

```bash
# In projects/tessera-dev/ (development worktree, branch dev):
cd ../tessera-dev
git restore Cargo.lock
git rebase main
cargo check -p tessera
```

## Automated Git Hook Guards (.githooks/pre-commit)

To minimize cognitive overhead and prevent human error, repository constraints
are codified into tracked Git hooks in `.githooks/`. Standard `.git/hooks/`
scripts are private to a local checkout and cannot be synchronized or reviewed
in version control. Setting `git config core.hooksPath .githooks` activates the
repository-owned guardrails across linked worktrees. The hook implementation
is tracked directly in [.githooks/pre-commit](../../.githooks/pre-commit).

### Dual-Mode Guard Behavior

The tracked `.githooks/pre-commit` hook dynamically detects the active
worktree mode and enforces corresponding constraints:

#### 1. In Local Optics Mode (`tessera-dev/`)

- **Automatic Path Unstaging**: During regular feature commits, the hook
  silently unstages `Cargo.lock` (mutated by the local patch) and
  `.cargo/config.toml` (if force-staged), printing a non-intrusive notice:
  ```text
  Local Optics mode: excluded Cargo.lock from this commit.
  ```
  The commit proceeds with only your clean Tessera source changes.
- **Refusing Release-Shaped Commits**: If you attempt to commit a version bump
  (`+version = "..."` in `Cargo.toml`), the hook aborts the commit:
  ```text
  Local Optics mode: refusing a release-shaped commit.
  The staged Cargo.toml bumps the workspace version, but local
  mode cannot produce a committable canonical Cargo.lock.
  Perform release version bumps and canonical lockfile generation
  directly inside the primary main worktree (../tessera/).
  ```

#### 2. In Canonical Mode (`tessera/`, `main`)

- **Lockfile Integrity Validation**: Whenever `Cargo.toml` is staged on `main`,
  the hook automatically executes `cargo metadata --locked --format-version 1`.
  If the canonical lockfile is out of sync or missing, the commit is blocked
  until `cargo check` updates it against the remote Git tags.

### Recovering Canonical State

If a worktree's lockfile is accidentally polluted by path entries, restore it
immediately:

```bash
# In projects/tessera/ (main) or projects/tessera-dev/ (dev):
rm -f .cargo/config.toml
git restore Cargo.lock
cargo check --locked --workspace
```

### Rebasing dev when Main Advances

If `main` advances independently through peer pull requests or hotfixes, rebase
`dev` onto the updated `main`:

```bash
# 1. Pull the primary worktree (in projects/tessera/, branch main):
cd ../tessera
git pull --ff-only

# 2. Rebase the development worktree (in projects/tessera-dev/, branch dev):
cd ../tessera-dev
git restore Cargo.lock
git rebase main
cargo check -p tessera
```

## Sibling Project Alignment

The same dual-worktree pattern, `.cargo/optics-local.toml` template, and Git
hook guards apply uniformly across companion repositories in the desktop stack:

- `arca-dev`: Desktop file manager and chooser.
- `sigil-dev`: Secret service daemon and PAM provider.
- `aphrodite-dev`: Companion workspace.

All applications maintain clean canonical branches on `main` and use
worktree-local patch files during daily development.
