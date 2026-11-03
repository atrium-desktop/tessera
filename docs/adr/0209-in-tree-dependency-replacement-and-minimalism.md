---
id: ADR-0209
title: "In-Tree Dependency Replacement and Minimalist Systems Architecture"
status: accepted
date: 2026-11-03
scope: workspace, dependencies, minimalism, architecture
superseded_by: null
negative_knowledge: true
---

# 0209. In-Tree Dependency Replacement and Minimalist Systems Architecture

- Status: Accepted
- Date: 2026-11-03
- Deciders: Tessera Maintainers
- Amends: [ADR-0159](0159-package-boundaries-follow-capabilities.md)
- Related: [ADR-0206](0206-pure-compositor-dependency-hygiene-and-build-parallelism.md), [ADR-0207](0207-compositor-dbus-isolation-and-prebaked-cursor-strategy.md)

---

## Context and Problem Statement

A thorough audit of direct third-party dependencies revealed severe crate-creep where small, single-purpose requirements dragged large transitive dependency graphs into the workspace:

1. **`rust-ini`**:
   Used solely in `tessera-launch-services` for reading `.desktop` files and `index.theme` files. Through its dependencies (`ordered-multimap`, `dlv-list`, `const-random`), it pulled an unneeded proc-macro (`const-random-macro`), Keccak cryptographic hashing (`tiny-keccak`), and forced compilation of an obsolete major line (`getrandom 0.2.17`), causing `getrandom` duplicate version fragmentation.
2. **`dirs`**:
   Invoked only four times across the entire workspace (`home_dir`, `config_dir`, `picture_dir`) for standard Linux XDG paths, dragging `dirs-sys` and `option-ext`.
3. **`base64`**:
   Called a single time in `tessera-mcp` to encode screenshot bytes in a JSON-RPC response.
4. **`fastrand`**:
   Used only in `tessera-avatar` to shuffle a small array of motion clip indices.

## Decision Drivers

- **Zero-Bloat Rule**: Simple and medium-complexity tasks that can be robustly implemented in under 50 lines of pure Rust must never carry an external crate dependency.
- **Dependency Graph Flattening**: Eliminate transitive proc-macros and version collisions caused by utility crates.
- **Supply-Chain Integrity**: Fewer external crates reduce auditing surface and build fragility.

## Decision Outcome

### 1. In-Tree INI Parser (`tessera-launch-services::ini`)

Implemented a self-contained, 50-line zero-allocation INI parser supporting `[Section]` headers and key-value pairs. `rust-ini` is eradicated, eliminating:
- `rust-ini`
- `ordered-multimap`
- `dlv-list`
- `const-random`
- `const-random-macro` (proc-macro)
- `tiny-keccak`
- `crunchy`
- `getrandom 0.2.17` (resolving the workspace version conflict)

### 2. In-Tree XDG Base Directory Resolver

Standardized `$XDG_CONFIG_HOME`, `$XDG_PICTURES_DIR`, and `$HOME` lookups directly on `std::env::var_os`. `dirs`, `dirs-sys`, and `option-ext` are removed from the workspace.

### 3. In-Tree Base64 Encoder (`tessera-mcp::base64_encode`)

Implemented standard Base64 encoding in 25 lines of pure Rust. `base64` is removed from workspace dependencies.

### 4. In-Tree XorShift64 PRNG (`tessera-avatar::Rng`)

Implemented Fisher-Yates shuffle over a 15-line XorShift64 generator for avatar motion selection. `fastrand` is removed from `tessera-avatar`.

## Negative Knowledge

- **Never import an INI crate for simple `.desktop` or `index.theme` parsing**: Modern crates often carry randomized hash maps or proc-macro seeds that drag heavy cryptographic and macro dependencies for what is fundamentally a 40-line string scanner.
- **Never import a crate for trivial XDG environment lookups**: Standard Linux paths are three environment variables (`$XDG_CONFIG_HOME`, `$XDG_DATA_DIRS`, `$HOME`); adding cross-platform abstraction layers adds compile latency for no platform value on a pure Wayland compositor.
