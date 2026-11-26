# Project Layout

Tessera uses a flat Cargo workspace. Every package lives directly under
`crates/` and its directory name matches its package name. Package names carry
the architectural role; directory nesting does not grant dependency rights.

## Source tree

```text
crates/
  tessera                 composition root binary, display resource lifecycle, and session orchestration
  tessera-assets          theme asset decoding: vector sources and premultiplied-BGRA rasters (ADR-0233)
  tessera-audit           bounded, integrity-checked durable event storage
  tessera-authority       actor identity, credentials, grants, ceilings, capabilities, and observation leases
  tessera-avatar          profile contract, portrait assets, and VRM runtime
  tessera-cast            out-of-process Miracast caster and cast capability probe (ADR-0192)
  tessera-control         domain-oriented session control and command dispatch library for tessera
  tessera-composite       GPU composition, damage tracking, scanout, streams, and capture encoding
  tessera-config          declarative configuration schema, loader, and live reload
  tessera-design          product design tokens, themes, and glass materials
  tessera-desktop         pure desktop state, windows, workspaces, layouts, and deterministic rules
  tessera-env             shared process environment, tracing/log bridge, and runtime hygiene
  tessera-i18n            locale negotiation and translated chrome message catalog
  tessera-idle            out-of-process idle detection and inhibition sidecar
  tessera-ipc             Unix socket transport, framing, client, and server connection handling
  tessera-launch-services desktop application discovery, icon lookup, process launching, and intent search (ADR-0164)
  tessera-lock            standalone session lock client
  tessera-mcp             Model Context Protocol adapter daemon
  tessera-network         pure network snapshots, backend contract, and service protocol (ADR-0242)
  tessera-network-system  NetworkManager/iwd backends and the tessera-networkd service (ADR-0242)
  tessera-platform        DRM/KMS, nested display, seat, and input adapters
  tessera-primitives      foundational physical geometry, color, buffer descriptor, identity, and input primitives (ADR-0164)
  tessera-protocol        versioned wire DTOs and audit vocabulary (zero socket/GPU dependencies)
  tessera-shell           Optics UI host and built-in chrome components (dock, hud, pivot, control-center, settings, tray)
  tessera-wallpaper       desktop wallpaper and continuous parallax runtime
  tessera-wayland         Wayland protocol handlers and client state machines
  tessera-wayland-protocols generated Wayland C ABI interface tables
  tessera-test-support    shared test-only infrastructure (dev-dependency only)
tests/                    root external suites: common/, integration/, e2e/
assets/                   data compiled into binaries (cursors, icons, wallpapers)
system/                   files installed beside the binaries; mirrors the install prefix (ADR-0241)
xtask/                    repository architecture gates and Optics pin management (`cargo run -p xtask --`)
```

Repository-level tool configuration sits at the root:

| File | Owns |
|------|------|
| `rust-toolchain.toml` | The pinned compiler and the `rustfmt`/`clippy` components CI uses |
| `clippy.toml` | Workspace lints shared by every crate |
| `typos.toml` | Spell-check allowances for protocol identifiers and project vocabulary |
| `deny.toml` | Supply-chain policy: advisories, license allowlist, wildcard requirements, dependency sources |
| `.config/nextest.toml` | Test profile, exclusive-resource groups, and the setup script that builds the sibling binaries |
| `.cargo/optics-local.toml` | The opt-in sibling-Optics `[patch]` and `OPTICS_DYNAMIC=1` template (never the committed configuration) |

`test-support` is not a production package. It owns environment guards, scratch
directories, socket names, polling, and workspace/binary lookup shared by
in-crate tests and the root external suites.

## Ownership rules

`tessera-primitives` contains foundational stable physical and geometric values only. Desktop policy
belongs to `tessera-desktop`; authority and identity policy belongs to
`tessera-authority`; external compatibility belongs to `tessera-protocol`.
None of those packages may depend on sockets, Wayland, GPU bindings, or UI.

The `tessera` binary is the composition root. It owns display engine resources
(host, device, presentation surface, canvas, renderer, and Wayland server) with
strict drop order, executes desktop use cases, applies configuration, and
supervises child adapters. Shell components in `tessera-shell` emit typed events;
they do not mutate the server or write configuration directly.

Wire protocol types live in `tessera-protocol` with zero network, filesystem,
or serialization dependencies. Unix framing, socket paths, and client/server
abstractions live in `tessera-ipc`. Durable event storage lives in
`tessera-audit`.

Application discovery, icon lookup, process launching, and intent search live
in `tessera-launch-services` (ADR-0164). Theme asset bytes and pixels — SVG/SVGZ
sources via in-process `librsvg-2.0` + `cairo`, icon rasters, symbolic
recolouring — live in `tessera-assets` (ADR-0233, ADR-0261). GPU capture
encoding is integrated into `tessera-composite`.

Create a new package only for an independent public API, process, feature or
platform boundary, resource owner, or compile-time dependency restriction. A
large module with one owner remains a module.

## Dependency verification

The boundary check reads `cargo metadata` and validates every edge against explicit capabilities:

```bash
cargo run -p xtask -- check-boundaries
```

The check runs in CI. Full workspace validation uses `cargo nextest run`.
