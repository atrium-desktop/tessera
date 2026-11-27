---
id: ADR-0263
title: "First-party crates carry the tessera namespace prefix"
status: accepted
date: 2026-11-27
scope: workspace, xtask, crates
superseded_by: null
negative_knowledge: true
---

# 0263. First-party crates carry the tessera namespace prefix

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Extends: [ADR-0066](0066-canonical-aegis-namespace.md)
  (the canonical product namespace now also governs Cargo package names and
  their directories)

---

## Context and Problem Statement

[ADR-0066](0066-canonical-aegis-namespace.md) fixes the product namespace for
binaries, environment variables, configuration paths, sockets, and MCP tools,
but says nothing about Cargo package names. The workspace follows
`tessera-<role>` by habit: 34 packages under `crates/` plus the root
`tessera-tests` suite, none published (`publish = false`). Because nothing is
published, the prefix looks redundant inside the repository, and the question
of dropping it (`tessera-desktop` → `desktop`) recurs.

Rust has no package namespaces. Every crate in a build graph — first-party and
third-party alike — shares one flat set of names, and code imports a crate by
that name alone (`use tessera_desktop::…`). The packages-as-namespaces proposal
([RFC 3243](https://rust-lang.github.io/rfcs/3243-packages-as-optional-namespaces.html))
would allow `tessera::desktop`, but it is not usable on the pinned toolchain.
Until it is, a name prefix is the only namespace a Rust workspace has.

## Decision Drivers

- **No collisions with the dependency graph.** A first-party name must never
  shadow or collide with a third-party crate, now or after a future dependency
  is added.
- **First-party provenance at every import.** About 6,600 import paths name
  workspace crates; a reader, `check-boundaries`, and
  `xtask/dependency-policy.toml` all key on those names.
- **Forward compatibility.** Publishing a client-facing crate, moving a crate
  to its own repository, or adopting RFC 3243 must not require a rename.
- **One identifier per package.** The package name, its directory, and its
  import path should be the same word, so one search finds all three.

## Considered Options

1. Keep `tessera` / `tessera-<role>` for packages and directories, enforced by
   a gate.
2. Drop the prefix from package names (`desktop`, `protocol`, `config`).
3. Keep prefixed package names but drop the prefix from directories
   (`crates/desktop/` holding package `tessera-desktop`).
4. Leave the rule unwritten and revisit when RFC 3243 stabilizes.

## Decision Outcome

Chosen option: 1, keep the prefix on package names and directories, because
it is the only option that is collision-free today, keeps provenance visible,
and maps one-to-one onto RFC 3243 namespaces (`tessera-desktop` →
`tessera::desktop`) if they arrive.

### Rules

1. Every first-party package is named `tessera` (the composition root) or
   `tessera-<role>`, where `<role>` is lowercase kebab-case ASCII words joined
   by single hyphens.
2. Every package lives directly in `crates/<package-name>/`; the directory name
   equals the package name.
3. Two root members are fixed exceptions to rule 2, each reserved for exactly
   one package: `tests/` holds `tessera-tests`, and `xtask/` holds `xtask`. `xtask`
   is also exempt from rule 1 because it is repository tooling with a
   Cargo-ecosystem conventional name ([ADR-0240](0240-repository-automation-at-the-root-and-fixtures-with-their-consumer.md)),
   not part of the product.
4. `<role>` names what the crate is responsible for, specifically enough to
   stand without the prefix. A generic role (`utils`, `common`, `misc`) is
   not admissible for a new crate; prefer the precise role
   (`tessera-test-support`, not `tessera-utils`).

Installed binary names are governed by [ADR-0066](0066-canonical-aegis-namespace.md)
and the explicit `[[bin]]` names in each manifest, not by this record.

Rules 1–3 are a repository convention enforced by a gate, not a System
Invariant: a violation breaks consistency and tooling, not process correctness,
so it does not meet the blast-radius condition of
[Invariants](../dev/invariants.md) §3 and carries no `INV-` tag. Rule 4 is
review guidance and has no mechanical check.

## Consequences

### Positive

- First-party names cannot collide with third-party crates: no
  `wayland-protocols` crate exists under the `tessera-` prefix.
- Every import shows provenance, which keeps dependency-boundary review and
  `dependency-policy.toml` readable.
- Publishing, extracting a crate to another repository, or adopting RFC 3243
  needs no rename.
- A new package that breaks the rule fails `package-conformity` in CI with a
  message naming the expected name or directory.

### Negative / Trade-offs

- Paths and imports carry eight extra characters. Accepted: the cost is
  cosmetic and constant.
- Package names are coupled to the product brand. The project has already been
  renamed once (Aegis → Tessera, [ADR-0066](0066-canonical-aegis-namespace.md)
  note); another rebrand means another workspace-wide mechanical rename.
  Accepted: a rebrand is rare and mechanical, while collisions and publishing
  are ongoing risks.

## Rejected Alternatives & Negative Knowledge

### Bare package names (option 2)

- **Why considered**: Nothing is published, so the prefix repeats information
  the repository already implies. rust-analyzer (`hir`, `ide`, `syntax`) and
  Zed (`gpui`, `editor`) use bare names successfully.
- **Why rejected**: Bare names only work when each name is distinctive on its
  own. Here `tessera-wayland-protocols` would collide with the
  `wayland-protocols` crate already in `Cargo.lock`, and most other roles
  (`config`, `protocol`, `platform`, `env`, `image`, `audit`, `ipc`, `dbus`)
  are names of popular crates the workspace could add at any time. Imports such
  as `use protocol::…` would no longer distinguish first-party code, and no
  bare name could be published to crates.io. Dropping the prefix safely would
  require renaming those roles to longer, more specific names, which removes
  the brevity that motivated the change.

### Unprefixed directories only (option 3)

- **Why considered**: It shortens file paths without touching imports or
  creating collisions.
- **Why rejected**: It breaks the one-identifier property: the directory,
  package, and import path become different words, so searching for a crate's
  name no longer finds its directory. rust-analyzer, Zed, Bevy, and Cargo all
  keep directory equal to package name.

### Leave the rule unwritten (option 4)

- **Why considered**: The workspace already conforms, and RFC 3243 may change
  the trade-off.
- **Why rejected**: An unwritten convention has to be re-argued for each new
  crate and drifts quietly. RFC 3243 makes the prefix more valuable, not less:
  prefixed names migrate to namespaced ones directly.

## Verification

`cargo run -p xtask -- package-conformity` (CI job `fmt-and-clippy`, Gate A in
[Development Workflow](../dev/workflow.md)) runs
`naming_violations` in `xtask/src/tasks/package_conformity.rs` over every
package `cargo metadata` reports and fails on any package whose name breaks
rule 1 or whose directory breaks rules 2–3. Its unit tests cover accepted
names, out-of-namespace names, mismatched and nested directories, and the
reserved root members.
