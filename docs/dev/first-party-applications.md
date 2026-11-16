# First-Party Application Development

Use this workflow for standalone applications that ship with Tessera but run as
ordinary Wayland clients. **Settings** (`tessera-settings`,
[ADR-0238](../adr/0238-settings-is-a-standalone-application-behind-its-own-scope.md))
is the in-tree reference: a library crate for its model and pages, a binary
for its window, and the compositor reached only over IPC.

The installation and staging model is defined by
[ADR-0069](../adr/0069-documentation-owned-installation-and-throwaway-development-staging.md).

## Process Boundary

Sharing one repository and release does not make an application part of the
compositor process:

```text
Tessera repository and release
├── tessera
│   └── Wayland compositor and IPC server
└── first-party application
    └── independent Wayland client and IPC client
```

Cargo workspace membership coordinates source and builds. It does not load
one binary into another, place sibling binaries on `PATH`, or register XDG
metadata.

A standalone application reaches Tessera through two runtime boundaries:

- Wayland provides the window, input, rendering, and application identity.
- `$XDG_RUNTIME_DIR/tessera.sock` provides scoped IPC snapshots and actions.

Do not add the application crate as an `tessera` Rust dependency. Shared
schemas and models belong in library crates such as `tessera-desktop`,
`tessera-protocol`, and `tessera-primitives`. A surface that should render in-process belongs in a chrome
component crate on the `tessera-shell` contract instead.

An application that needs privileged IPC claims a built-in scope bound to its
installed executable path (ADR-0128), never ambient privilege: Settings claims
`tessera-settings`, which permits settings transactions and focuses only a
Settings-owned window (so a second invocation can raise the running one).

## Installation Contract

A production package installs all parts of an application as one unit:

| Artifact | Path relative to the installation prefix |
|----------|------------------------------------------|
| Executable | `bin/<application>` |
| Desktop entry | `share/applications/<application-id>.desktop` |
| Application icon | `share/icons/hicolor/scalable/apps/<application-id>.svg` |

The desktop entry's `Exec`, `TryExec`, `Icon`, and `StartupWMClass` values
must agree with the installed executable, icon name, and Wayland application
id. Do not use an absolute build-directory path in packaged metadata.

Cargo does not install desktop entries or icons. Distribution packages and
installation tooling must install the files under `system/share/` alongside the
compiled binaries; `system/` mirrors the installed prefix (see
[Packaging](packaging.md#install-manifest)).

## Development Staging

Direct Cargo commands are the default development loop. Stage the complete
first-party application contract — executable, desktop entry, and icon —
into a throwaway prefix only when a test needs XDG discovery, so the user's
`~/.local` stays clean.

```bash
prefix=$(mktemp -d)
install -Dm0755 target/debug/tessera-settings "$prefix/bin/tessera-settings"
cp -r system/share "$prefix/"
PATH="$prefix/bin:$PATH" XDG_DATA_DIRS="$prefix/share:$XDG_DATA_DIRS" \
  TESSERA_BACKEND=nested cargo run -p tessera
```

A staged binary is not at an allowlisted path, so add it to the nested
session's `[ipc.scope_executables]` (`tessera-settings = ["<prefix>/bin/tessera-settings"]`)
or Settings opens read-only.

The desktop entry's `Exec` resolves through `PATH` and its `Icon` through
`XDG_DATA_DIRS`, so the launcher follows the same lookup path as a packaged
installation. Distribution installation also owns the systemd unit, D-Bus
service, and portal metadata; its complete manifest is in
[Distribution Packaging](packaging.md).

## New First-Party Applications

When adding an external application:

1. Add a workspace binary with an independent process boundary.
2. Assign one reverse-DNS application id.
3. Add a desktop entry whose filename and `StartupWMClass` match that id.
4. Add a hicolor icon whose name matches the desktop entry's `Icon`.
5. Add the binary and metadata to the development staging recipe and
   production package manifest.
6. Test launching it from Applications instead of synthesizing a catalog
   entry.

Compositor-owned virtual surfaces are different: they do not have an
external executable and may remain explicit built-in catalog entries.
