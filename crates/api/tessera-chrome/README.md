# tessera-chrome

`tessera-chrome` owns the chrome **contract** (ADR-0021): the `Chrome`
component trait, the host-owned snapshot and lifecycle update types, the
interaction event sink, the backdrop/liquid-glass declaration types, and
shared chrome geometry helpers (popup placement, preview cards, text
fitting, the application context menu).

## The dependency law

- **Components depend on this crate, never on the host.** `tessera-shell`
  is the host implementation that binds the contract to the compositor's
  lens context; component crates (`tessera-dock`, `tessera-hud`,
  `tessera-prism`, `tessera-command-panel`, `tessera-settings`) implement
  `Chrome` against this crate alone. Adding or removing a chrome surface
  is a component change, not a host change.
- **The contract is safe code.** The host's FFI (binding lens to the flux
  device) lives in `tessera-shell`; this crate is `#![forbid(unsafe_code)]`
  and carries no device or process state.
- **Chrome components do not depend on each other.** Anything shared
  between surfaces lives here as a contract type or helper.

## Non-goals

- No host state, no lens `Ui` context, no compositor socket: this crate
  never renders a frame by itself.
- No product policy: design values come from `tessera-design`; component
  behavior stays in the owning component crate.
