# tessera-capture

`tessera-capture` is the **pure pixel domain** of the compositor's
capture pipeline (ADR-0037/0052/0055/0037): everything that transforms
frames after the GPU has finished rendering them, and everything before
a screenshot or stream frame leaves the process.

## Responsibilities

- `CapturedPixels` / `PendingReadback`: GPU readback staging detached
  from the presentation surface, with the security-generation tag that
  lets the compositor revoke captures on policy transitions.
- Encoding: cursor compositing, alpha unpremultiplication, crop/rotate,
  PNG encode — all pure functions over RGBA/BGRA buffers.
- Geometry: logical-rect clamping against output bounds and
  logical→physical conversion at fractional scale.
- Publication naming: the screenshot URI list format.

## Non-goals (enforced by the tier law)

- No compositor state: the worker that *orchestrates* captures (event
  loop integration, journal effects, IPC replies) stays in the
  composition root; this crate never holds a `CompositorRuntime`.
- No IPC transport, no filesystem publication logic beyond naming.

The tier guard expresses this: `services` may not depend on `chrome`
or the binary, and the binary's orchestration module consumes this
crate through its narrow value types.
