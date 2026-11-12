# System Architecture Governance

This document codifies the workspace topology, subsystem boundaries, and
non-negotiable architectural red-lines of Tessera.

The normative source of truth for any individual edge is
`tooling/xtask/dependency-policy.toml`, checked from Cargo metadata by
`cargo run -p xtask -- check-boundaries`. This document states the layering
intent that policy encodes.

---

## 1. Concentric Workspace Topology

Tessera organizes its Cargo workspace into concentric layers. Dependencies flow
strictly from outer layers inward; backward or circular dependencies across
layer boundaries are prohibited.

```text
┌───────────────────────────────────────────────────────────────┐
│  Layer 5: Composition Root (`crates/tessera`)                 │
│  - Display engine ownership, session orchestration, teardown  │
├───────────────────────────────────────────────────────────────┤
│  Layer 4: Presentation & Surfaces                             │
│  - Chrome host: `tessera-shell`                               │
│  - Standalone surfaces: `tessera-lock`, `tessera-wallpaper`   │
│  - Portrait runtime: `tessera-avatar`                         │
├───────────────────────────────────────────────────────────────┤
│  Layer 3: Capabilities, Platform & Adapters                   │
│  - GPU composition: `tessera-composite`                       │
│  - Hardware: `tessera-platform`, `tessera-wayland`            │
│  - Services & management: `tessera-launch-services`,          │
│    `tessera-assets`, `tessera-cli`, `tessera-mcp`,            │
│    `tessera-idle`, `tessera-config`, `tessera-design`,        │
│    `tessera-env`                                              │
├───────────────────────────────────────────────────────────────┤
│  Layer 2: Domain Contracts & Wire                              │
│  - Desktop state: `tessera-desktop`                           │
│  - Authority: `tessera-authority`                             │
│  - Wire protocol: `tessera-protocol`, `tessera-ipc`           │
│  - Durable storage: `tessera-audit`                           │
├───────────────────────────────────────────────────────────────┤
│  Layer 1: Axiomatic Primitives                                 │
│  - Physical values: `tessera-primitives`                      │
│  - Locale negotiation: `tessera-i18n`                         │
└───────────────────────────────────────────────────────────────┘
```

The `tessera-wayland-protocols` package is a generated C ABI table owned by the
`tessera-wayland` boundary, not an independent layer.

---

## 2. Architectural Red-Lines

### 2.1 Composition-Root Resource Ownership
- **Red-Line**: The `tessera` binary owns the display engine lifecycle (host,
  GPU device, presentation surface, canvas, renderer, and Wayland server) with
  strict drop order ([ADR-0158](../adr/0158-clean-break-workspace-boundaries.md)).
- **Enforcement**: No production library depends on the `tessera` package
  ([ADR-0159](../adr/0159-package-boundaries-follow-capabilities.md)).

### 2.2 Protocol Purity
- **Red-Line**: `tessera-protocol` contains versioned wire DTOs and audit
  vocabulary only. It has no socket, filesystem persistence, or GPU
  dependency ([ADR-0158](../adr/0158-clean-break-workspace-boundaries.md)).
- **Enforcement**: Unix framing, socket paths, and client/server connection
  handling live in `tessera-ipc`; the composition root supplies application
  policy at the boundary.

### 2.3 Domain Isolation
- **Red-Line**: Domain packages (`tessera-desktop`, `tessera-authority`) cannot
  depend on platform, storage, transport, or graphics implementations.
  `tessera-primitives` holds foundational values only and cannot become a
  miscellaneous helper crate ([ADR-0159](../adr/0159-package-boundaries-follow-capabilities.md)).
- **Enforcement**: `check-boundaries` rejects any normal dependency outside the
  package's declared capability edge set.

### 2.4 Shell/Platform Firewall
- **Red-Line**: Chrome components in `tessera-shell` communicate through the
  component contract and cannot depend on Wayland or renderer internals. Shell
  components emit typed events; they do not mutate the server or write
  configuration directly ([ADR-0158](../adr/0158-clean-break-workspace-boundaries.md)).
- **Enforcement**: `tessera-shell` declares no `tessera-wayland`, `tessera-platform`,
  or `tessera-composite` normal edge.

### 2.5 Native-Library Build Ownership
- **Red-Line**: Native-library generation has exactly one build owner shared by
  its consumers. Subsystems must not use intermediate crates as passthrough
  re-export conduits ([ADR-0159](../adr/0159-package-boundaries-follow-capabilities.md)).
- **Enforcement**: `tessera-wayland-protocols` is built once for all consumers.

### 2.6 Test and Workspace Environment Isolation
- **Red-Line**: Automated tests, previews, and ephemeral tool runs must NEVER
  mutate, read, or depend on the host user's home directory (`$HOME`, `~/.config`,
  `~/.local`) or global host state. All stateful filesystem operations must isolate
  inside `tempfile::tempdir()` or local test scratch roots. Network listeners must
  bind ephemeral ports (`:0`) ([ADR-0201](../adr/0201-decouple-documentation-governance-from-agent-configuration-and-canonize-sandbox-isolation.md)).
- **Enforcement**: Dedicated test assertions enforce `tempdir` lifecycle and
  sandbox isolation (`[INV-ARCH-87]`).

### 2.7 Subsystem Modules Inside the Composition Root
- **Red-Line**: A host-service bridge that (a) links no client library, (b) ships
  no binary, and (c) reaches its service only through short-lived CLI probes is a
  **module** of the composition root, not a package — `wireless` (ADR-0162),
  `bluetooth` (ADR-0175), and `audio` (ADR-0216). Each owns a backend trait, one
  live adapter, one parse seam testable against captured documents, and one
  contract-faithful mock. Grouping them into a shared "peripheral" package would
  bundle a frame-path input library, a D-Bus session, and a daemon-owned audio
  graph behind one manifest.
- **Enforcement**: `check-boundaries` finds no package for these subsystems, and
  the composition root's dependency set is unchanged by them
  ([ADR-0159](../adr/0159-package-boundaries-follow-capabilities.md),
  [ADR-0216](../adr/0216-audio-subsystem-owns-both-directions-and-capture-is-a-privacy-reading.md)).

---

## 3. Crate Addition and Workspace Integrity

1. **Flat Directory Layout**: Package directories are one level below `crates/`
   and match package names. Directory nesting grants no dependency rights.
2. **Modules by Default**: Retain a package only for a real API, process,
   platform boundary, feature boundary, or resource-ownership boundary. A large
   module with one owner remains a module.
3. **Explicit Dependency Policy**: Every production package has an explicit
   entry in `tooling/xtask/dependency-policy.toml`. Unknown packages fail
   validation — there is no default unrestricted role.
4. **Moving Code Preserves Behavior**: Refactors must preserve tests and
   behavior, and must not leave compatibility facade packages for removed names.

See [Project Layout](project-layout.md) for the source tree map and
per-package ownership notes.
