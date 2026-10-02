# Cross-Repository Development Workflow

This guide establishes the standard engineering workflow for developing Tessera
alongside its companion C rendering and material engine, **Optics**. It uses a
dual Git worktree architecture to combine zero-friction local iteration on the
development branch with strict, reproducible, and bisectable builds on `main`.

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
illusion**: it succeeds only because Cargo and the dynamic linker secretly link
against the privileged, uncommitted state in your sibling `../optics/build`
directory.

The foundational principle of this dual-worktree architecture is:
> **`main` represents external reality. It must genuinely build and run in
> the wild for users, CI, and package managers—without depending on any
> developer's private machine state.**

Every rule in this workflow (Optics Upstream First, atomic promotion, canonical
manifest updates, and locked lockfile validation) exists solely to prevent local
development privilege from masquerading as a functional release on `main`.

### Core Invariants

1. **Local Patch Containment**: `.cargo/config.toml` and path-resolved
   `Cargo.lock` belong strictly to the `tessera-dev` worktree. The tracked
   pre-commit hook automatically excludes them from regular commits.
2. **Optics Upstream First**: Any feature merged into `main` that relies on
   new or modified Optics APIs must depend solely on an immutable, tagged, public
   Optics commit. `main` must never reference uncommitted or local-only
   Optics revisions.
3. **Always-Buildable Main (Atomic Promotion)**: Every commit on `main` must be
   individually buildable with `cargo check --locked --workspace`. Feature
   code changes and their corresponding upstream dependency tag bumps and
   lockfiles must land together in a **single atomic commit**, ensuring zero
   broken intermediate states for `git bisect`.
4. **Target Directory Isolation**: Separate target directories are required.
   Do not configure a shared `CARGO_TARGET_DIR` across worktrees, as mixing
   incremental compilation artifacts between canonical Git dependencies and
   local path overrides leads to cache invalidation churn.
5. **Version Authority and User Intent**: All version elevations and release
   actions—both internal workspace version bumps (`workspace.package.version`)
   and external upstream dependency promotions (such as Optics Git release tags)—
   are strictly governed by **explicit user intent**. Automated agents and AI
   assistants MUST NOT autonomously decide, guess, or bump version numbers or
   upstream tags without unambiguous user instruction. Automation and tooling
   (such as `cargo run -p xtask -- optics --set <TAG>`) exist purely to execute
   and validate the upgrade deterministically, never to usurp version policy.

## Workspace Setup

### Directory Topology

Place all repositories under a common parent directory:

```text
projects/
├── tessera/       # Primary worktree on main (canonical, production-ready)
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
   cargo nextest run -p tessera
   ```

Do not run concurrent Meson or Ninja builds against `../optics/build` from
multiple terminals, as the build output directory is a shared write location.

### Committing Tessera Changes on dev

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

#### Why Cargo.lock Must Never Be Committed on dev

Even though features on `dev` are eventually squash-promoted onto `main`,
allowing mutated `Cargo.lock` entries into `dev` commits introduces three
severe failure modes:

1. **Rebase Lockfile Conflicts**: Whenever `main` updates any canonical
   dependency, rebasing `dev` onto `main` triggers massive, opaque merge
   conflicts across hundreds of lockfile lines. When `Cargo.lock` is kept
   uncommitted on `dev`, rebasing is completely conflict-free: a quick
   `git restore Cargo.lock && git rebase main` followed by `cargo check`
   lets Cargo regenerate the local lockfile in milliseconds.
2. **CI and Remote Collaboration Breakage**: If `dev` is pushed to remote,
   remote CI runners or peer developers lack your machine's sibling
   `../optics` directory. Pushing path-overridden lockfiles causes remote
   `cargo check --locked` commands to immediately fail.
3. **Poisoned Squash Risk**: If `Cargo.lock` were committed on `dev`, running
   `git merge --squash dev` on `main` would pull `dev`'s local path lockfile
   into `main`'s staging area. Any subsequent commit would accidentally publish
   broken local path dependencies to `main`.

## Feature-Level Atomic Promotion to Main

Promote changes from `dev` to `main` at **feature-level boundaries** rather
than accumulating a monolithic, unreviewable multi-month release dump.

### Relationship Between `dev` and `main` Commit Histories

The commit history on `dev` and `main` is **deliberately distinct**:

- **`dev` history**: Contains rapid, exploratory, and fine-grained iteration
  commits created during co-development with local Optics. These commits were
  built against the local path-patched lockfile, so they are **not**
  individually buildable in canonical mode.
- **`main` history**: Contains curated, atomic feature commits that are
  guaranteed to build independently with `cargo check --locked --workspace`,
  pinned to canonical remote Git tags.

Because intermediate `dev` commits cannot build canonically, they must never
be merged onto `main` verbatim via `--ff-only`. Promotion is always an
**Atomic Squash Promotion**: the feature implementation, its updated Optics
tag in `Cargo.toml`, and the canonical `Cargo.lock` land together in
**exactly one atomic commit on `main`**. `dev` is then reset onto the promoted
`main`. The fine-grained history survives through dated archive tags, not
through `main`'s graph.

Follow this protocol whenever a completed feature on `dev` depends on new or
updated Optics functionality:

### Step 1: Upstream Optics First

Land and tag the required changes in `../optics` before making Tessera
canonical:

> **Version Policy Authority**: The release tag `vX.Y.Z` represents an external
> release milestone and must be explicitly specified or approved by the user.
> AI assistants and automated tools must never autonomously guess, invent, or
> bump upstream version numbers.

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

### Step 2: Switch to Main Worktree and Fast-Forward

Switch to the primary `tessera/` worktree (`branch main`). Because `main`
deliberately lacks `.cargo/config.toml`, it operates natively in canonical
mode without disabling or toggling any local configurations.

```bash
# In projects/tessera/ (primary worktree, branch main):
cd ../tessera
git switch main
git pull --ff-only
```

#### Why `--ff-only` Is Mandatory Here

The `--ff-only` flag is an intentional safety fuse:
- **Refuses Divergence**: If `main` has diverged or contains unpushed local
  edits, `git pull --ff-only` immediately aborts rather than generating an
  unintended `Merge branch 'main'` commit.
- **Guarantees Clean Baseline**: It ensures the promotion squash executes
  strictly on top of the authoritative remote HEAD, avoiding merge anomalies
  and push rejections later.

### Step 3: Atomic Promotion (Squash + Manifest + Lockfile + Verification)

Execute the entire promotion sequence as a single, indivisible unit of work.
**Do not commit the squashed feature before updating the dependency manifests.**

```bash
# In projects/tessera/ (primary worktree, branch main):

# 1. Squash feature changes from dev into the staging area (DO NOT COMMIT YET):
git merge --squash dev

# 2. Update all Optics dependency tags in Cargo.toml to the newly published tag:
cargo run -p xtask -- optics --set vX.Y.Z

# 3. Regenerate canonical Cargo.lock against the authoritative remote Git tags:
cargo check -p tessera

# 4. Verify that dependencies resolve to remote Git sources, not local paths:
cargo tree -i flux
cargo tree -i lens

# 5. Full workspace verification under --locked enforcement:
# (If the newly tagged native C libraries are not yet installed to /usr/lib,
# prepend PKG_CONFIG_PATH=../optics/build/meson-uninstalled so pkg-config finds them):
PKG_CONFIG_PATH=../optics/build/meson-uninstalled cargo check --locked --workspace
PKG_CONFIG_PATH=../optics/build/meson-uninstalled cargo nextest run --locked --workspace
PKG_CONFIG_PATH=../optics/build/meson-uninstalled cargo build --locked -p tessera

# 6. Create the SINGLE atomic promotion commit:
git commit -m "feat(dock): improve intent dwell hit testing

Adopt Optics vX.Y.Z."

# 7. Push canonical commit to upstream:
git push origin main
```

Collect `Co-authored-by:` trailers in the commit message for any co-authors of
the squashed iteration commits; squash merging discards intermediate commit
authorship otherwise.

By combining the squashed feature diff and the dependency upgrade in one
commit, **every single commit on `main` remains 100% buildable and bisectable**.

### Step 4: Archive Dev History, Reset Dev, and Continue

Notice that **`tessera-dev` never touched or disabled its `.cargo/config.toml`**.
Your local development environment remained active throughout the merge.

Before moving the `dev` pointer, preserve the fine-grained iteration history
with a dated archive tag. `git reset --hard` only moves the branch pointer—
the commit objects remain reachable through the tag and through
`dev@{1}` reflog (~90 days) and are free of extra disk cost (both worktrees
share one object store)—but the tag makes them permanently findable for
bisecting, archaeology, and cherry-picking:

```bash
# In projects/tessera-dev/ (development worktree, branch dev):
cd ../tessera-dev
git tag archive/dev-YYYYMMDD-<feature-slug> dev
git push origin archive/dev-YYYYMMDD-<feature-slug>   # optional, offsite backup
```

Then reset `dev` onto the freshly promoted `main`. Do **not** rebase here:
the dev content already lives inside the squash commit, so rebasing yields
empty commits and conflicts—`reset --hard main` is the only correct
operation:

```bash
# In projects/tessera-dev/ (development worktree, branch dev):
git reset --hard main
```

`Cargo.lock` now holds the canonical git-tag state. Simply run any Cargo
command to rewrite it into the local path-patched form; no manual
`git restore Cargo.lock` is needed:

```bash
# In projects/tessera-dev/ (development worktree, branch dev):
cargo check -p tessera
cargo tree -i flux   # confirm ../optics/bindings/ paths reappear
```

If `dev` was previously pushed, force-update the remote branch after the
reset (never with bare `--force`; the lease protects concurrent updates):

```bash
# In projects/tessera-dev/ (development worktree, branch dev):
git push --force-with-lease origin dev
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
`dev` onto the updated `main`. This applies only to **unpromoted WIP commits**;
after a feature has been squash-merged and promoted, use the
`git reset --hard main` flow in Step 4 instead of rebasing:

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
