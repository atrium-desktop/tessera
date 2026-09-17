# tessera-platform

`tessera-platform` abstracts the compositor's physical hardware devices,
operating system platform interfaces, and development host targets.

## Responsibilities

- Define the `Backend` contract used by the compositor frame loop.
- Own platform-specific window, display, resize, and input device state.
- Provide the **nested** Wayland host window used for development inside an
  existing session.
- Provide the **DRM/KMS** platform driver that presents to display hardware
  directly from a bare TTY with `libinput` event processing and `libseat` session
  management (VT console switching, hotplug detection, explicit sync via `IN_FENCE_FD`).
- Expose the platform-specific device extensions and surface factory required by
  the flux output surface (`host::Host`).

## Boundaries

`tessera-platform` does not implement the Wayland client protocol server, composite
client buffers, draw chrome, or choose window-management policy. Those concerns
belong to `tessera-wayland`, `tessera-render`, `tessera-shell`, and `tessera-desktop`.

## Runtime Effect

The active platform host pumps OS/kernel events into backend-neutral `InputEvent`
values and presents compositor frames to the hardware target. Resize, hotplug,
and VT suspend/resume are reported through the same interface; the nested target
ignores direct-display-only calls (VT switch, surface recreation).

## Use

The composition root selects a target through `host::Host` from `TESSERA_BACKEND`:
`auto` nests when `$WAYLAND_DISPLAY` is set and drives KMS on a bare TTY:

```rust
use tessera_platform::host::{BackendKind, Host};
use tessera_platform::Backend;

let mut host = Host::open(BackendKind::Auto, "tessera", 1280, 720, Default::default())?;
let device = host.create_device()?;
let mut surface = host.create_surface(&device)?;
while host.dispatch_timeout(std::time::Duration::from_secs(1)) {
    let events = host.take_input();
    // Route events and render the next frame.
}
```

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [ADR-0161: Rename tessera-backend to tessera-platform](../../docs/adr/0161-rename-backend-to-platform.md)
- [Bare-metal bring-up checklist](../../docs/how-to/bare-metal-drm.md)
- [Project Layout](../../docs/dev/project-layout.md)
