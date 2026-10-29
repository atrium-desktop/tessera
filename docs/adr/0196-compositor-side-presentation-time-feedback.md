---
id: ADR-0196
title: "Compositor-side presentation-time feedback (adopting Optics ADR-0122)"
status: accepted
date: 2026-10-27
scope: wayland/presentation, wayland/server, optics-interop
superseded_by: null
negative_knowledge: true
---

# 0196. Compositor-side presentation-time feedback (adopting Optics ADR-0122)

- Status: Accepted
- Date: 2026-10-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0077](0077-presentation-domain-redraw-state-machine.md)
- Adopts: Optics [ADR-0122](https://github.com/ming2k/optics/blob/main/docs/adr/0122-compositor-driven-presentation-pacing.md)

---

## Context and Problem Statement

Optics `v0.0.62` retired client-side timer pacing across its three backends
(Optics [ADR-0122](https://github.com/ming2k/optics/blob/main/docs/adr/0122-compositor-driven-presentation-pacing.md)).
An Optics host now asks the compositor *when* to draw (`wl_surface.frame`) and
learns the display's real refresh interval from
`wp_presentation_feedback.presented`. Its Wayland backend keeps
`refresh_ns = 0` until a compositor reports one, and a 0 period means the host
paces on its "refresh not learned yet" fallback — so on a 120/144 Hz panel an
Optics window stays at the fallback cadence no matter how well the panel
performs (`[INV-PACE-02]`).

Tessera bound the `wp_presentation` interface vtables from the start, but never
created the global. [ADR-0077](0077-presentation-domain-redraw-state-machine.md)
records why: a feedback request has to be tied to the surface commit it stands
for and completed from the presentation of *that* frame, and the single-threaded
redraw machine did not carry that association. The deferred decision left the
protocol in the tree but unadvertised, with a test pinning the absence.

This record is the deferred half. It does not change how Tessera paces its own
output — the DRM copy is already display-locked — it is the *report* Optics needs
back: which frames reached the display, when, at what refresh, and in what order.

## Decision Drivers

- A client cannot infer refresh rate from `wl_output.mode` alone reliably
  (VRR, mode switching), and Optics deliberately reads it from presentation
  feedback.
- Feedback must never describe a frame the client did not see: an occluded,
  unmapped, or destroyed surface must not be reported as presented.
- The compositor must not invent capability. A nested backend owns no vblank,
  so it must be able to say "refresh unknown" rather than fabricate 60 Hz.
- Resource lifetime has to be airtight: feedback objects are created by the
  client but are one-shot and destroyed by the server, and a client disconnect
  must not leave a dangling pointer queued for a later presentation.

## Considered Options

- **Advertise `wp_presentation` and complete feedback from the presented
  frame.**
- **Keep the protocol unadvertised** and rely on `wl_surface.frame` alone.
- **Advertise the protocol but complete every feedback with an estimated
  callback clock.**

## Decision Outcome

Chosen option: "Advertise `wp_presentation` and complete feedback from the
presented frame", because it is the compositor half of the pacing contract
Optics now depends on, and the association ADR-0077 asked for already exists at
one point: the frame-callback completion that runs after the output presents.

### 1. Queue on the surface, complete with the frame

`wp_presentation.feedback(surface)` creates a `wp_presentation_feedback` whose
user data is the target `SurfaceRec`; the object is queued on that surface. Like
a `wl_surface.frame` callback, it is *not* completed at request time. When the
presented frame is finalized, every feedback queued on a surface the frame
actually sampled is completed with one shared event:

- `tv_sec_hi`/`tv_sec_lo`/`tv_nsec`: the presentation instant on
  `CLOCK_MONOTONIC`, the clock the global's `clock` event announces
  (`monotonic_ns`).
- `refresh`: the output period in nanoseconds, derived from the fastest physical
  output's `refresh_mhz` (`1e12 / mhz`), or `0` for unknown.
- `seq_lo`/`seq_hi`: a monotonic presented-frame counter (`State::presentation_seq`),
  incremented once per frame that completes at least one feedback.
- `flags`: `0`. The compositor claims no versus-clock, hardware-clock, or
  zero-copy guarantee it cannot substantiate from a page flip today.

A frame streamed offscreen (ADR-0186) presents the surfaces it samples too, so
the same completion runs for a virtual output's frame.

### 2. Discard, never dangle

A feedback that cannot be tied to a live surface is completed as `discarded`
immediately. A queued feedback whose surface unmaps or is destroyed before its
frame presents is drained and `discarded` (`discard_surface_feedback`). The
resource carries a destroy hook (`feedback_resource_destroy`) that unlinks it
from the surface queue, so our own completion, a client teardown, and a client
disconnect are all safe in any order: the queue is emptied with `mem::take`
before any resource is destroyed, so the hook never observes a partially
drained list.

### 3. Advertise, do not fabricate

The global is created at version 1 in `server/lifecycle.rs`, alongside the other
stable extension globals. Advertisement is unconditional; *precision* is not:
the nested backend reports `refresh_mhz = 0`, so it advertises "refresh unknown"
rather than a synthesized 60 Hz, and the flags word stays empty.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-84] Presentation Feedback Is Commit-Correlated`: a
  `wp_presentation_feedback` may be completed as `presented` only from a frame
  that sampled the surface it was requested on; a surface that unmaps, is
  destroyed, or never presents is reported `discarded`, never `presented`.
- `[INV-ARCH-85] No Fabricated Presentation Timing`: the `refresh` field is the
  real output period or `0`; the timestamp is `CLOCK_MONOTONIC`; no capability
  flag is set that the compositor cannot substantiate from the presentation
  path.

## Rejected Alternatives & Negative Knowledge

### Keep the protocol unadvertised

- **Why considered**: it is the current state, it is safe, and Tessera's own
  scanout does not need the protocol.
- **Why rejected**: it makes Tessera an opaque compositor for every Optics host
  that paces on display feedback. Those hosts degrade to a fixed fallback
  cadence on high-refresh panels, and the failure appears as the host's fault
  (micro-stutter, a soft frame rate), which is precisely the misattribution
  `docs/dev/triage/fullscreen-frame-pacing.md` exists to correct.

### Advertise but complete with an estimated callback clock

- **Why considered**: it needs no presentation-path plumbing — the
  frame-callback timestamp is already at hand.
- **Why rejected**: [ADR-0077](0077-presentation-domain-redraw-state-machine.md)
  already prohibited it. An estimated deadline is a scheduling guess, and a
  client that subtracts its own `CLOCK_MONOTONIC` reading from a guess computes
  a latency that is not a latency. The completion point chosen here is the
  post-present frame finalization, so the timestamp is the instant the frame was
  handed to the display rather than a prediction of it.

## Consequences

### Positive

- An Optics host learns the real refresh interval and presentation latency from
  Tessera, and can pace to a 120/144 Hz panel instead of its fallback.
- `discarded` is delivered promptly on unmap and teardown, so a client's
  feedback bookkeeping cannot leak or stall across a window close.
- The compositor stays honest on nested/headless backends: refresh `0`, flags
  `0`, and no protocol error.

### Negative / Trade-offs

- **Page-flip sequence is compositor-local.** `seq` is a monotonic counter, not
  the DRM/KMS flip sequence; it orders presentations but does not name a kernel
  flip. Plumbing the real `PageFlipEvent` sequence and timestamp up from
  `tessera-platform` would make `seq` and `tv_sec`/`tv_nsec` exact and could
  additionally set the `VSYNC`/`HW_CLOCK` flags, and remains follow-up work.
- **Occluded surfaces wait.** A surface that is mapped but not sampled by any
  presented frame keeps its feedback queued until it is next presented or
  destroyed; the client is not told about a frame it did not see. This matches
  other compositors but means a fully hidden window's feedback resolves on the
  next visible frame rather than immediately.
