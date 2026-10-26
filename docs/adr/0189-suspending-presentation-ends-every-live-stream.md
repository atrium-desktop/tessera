---
id: ADR-0189
title: "Suspending Presentation Ends Every Live Stream"
status: accepted
date: 2026-10-27
scope: runtime/stream, composite/stream
superseded_by: null
negative_knowledge: true
---

# 0189. Suspending Presentation Ends Every Live Stream

- Status: Accepted
- Date: 2026-10-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0052](0052-scoped-output-frame-streaming.md), [ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md), [ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md), [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)

---

## Context and Problem Statement

When the session locks or the backend goes inactive, the compositor stops
serving streams: `CaptureWorker::set_allowed(!session_locked() && host.is_active())`
clears the delivery gate and bumps the security generation, so every frame
rendered after the boundary is dropped ([ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md),
[ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md)).

Dropping *future* frames does not retract the frame already delivered. The
consumer — a PipeWire client behind a `ScreenCast` session, a remote display, a
recording — **retains the last delivered frame indefinitely**. Two leaks
follow:

- A **mirror** of a physical output shows the pre-lock desktop on the remote
  sink, and had it kept delivering it would show the lock UI itself. The lock
  surface is a security surface ([ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md));
  it must never reach a remote consumer, and neither must the desktop it hides.
- A **virtual extend output** cast ([ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md))
  freezes on its last frame — the pre-lock desktop — for as long as the sink
  stays connected.

The compositor already accepts that "streams are not served while the session
is locked"; it did not close the loop that a *paused* stream is a *stale* stream
on the consumer side.

## Decision Drivers

- Fail closed on the security boundary: after a lock, no pre-lock pixel may
  remain reachable by any consumer.
- The fix must be uniform across every transport (SHM readback and zero-copy
  dmabuf, output / window / virtual-output targets) with no per-transport
  special case.
- No security-gate bypass: the blanking alternative would have to deliver a
  frame *through* the very gate that was just closed.
- No GPU work during the lock.

## Considered Options

1. **End every live stream on the suspension transition.**
2. **Deliver one black frame per stream, then stop.**
3. **Leave the pause as-is.**

## Decision Outcome

Chosen option: **Option 1.**

`OutputStreams::take_all` empties the registry and returns the live ids;
`CompositorRuntime::end_streams_for_suspension` sends `Event::StreamEnded` for
each, drops its offscreen damage tracker, and republishes the stream status. It
runs on the same transition the Agent-observation discard already uses
(`agent_suspended && !previous_agent_suspended`, where `agent_suspended =
session_locked() || !host.is_active()`), so every suspension path — lock, VT
switch, backend loss — ends streams exactly once.

Ending, rather than blanking, is chosen because a blank frame is itself a
frame: it must pass the delivery gate that the suspension just closed, and the
dmabuf transport has no CPU-side write path that avoids the GPU. Ending needs
neither: the consumer is told the session ended and stops retaining pixels.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-81] Suspension Ends Every Stream`: On the transition into
  suspension, every live stream MUST end with `Event::StreamEnded`, and no
  stream may survive it. Enforced by `OutputStreams::take_all` clearing the
  registry, verified by `test_inv_arch_81_take_all_ends_every_live_stream`, and
  composed with the already-verified post-boundary frame drop
  (`capture_security_generation_invalidates_pre_lock_frames`) so no pre-lock
  pixel remains deliverable after the boundary.

## Rejected Alternatives & Negative Knowledge

### 2. Deliver one black frame per stream, then stop
- **Why considered**: Keeping the cast session alive across a brief lock is
  friendlier than forcing the consumer to reconnect, and a black frame leaks
  nothing.
- **Why rejected**: The black frame must be delivered *after* the boundary, so
  it would need a carve-out in the delivery gate (`permits`) — a bypass in the
  most security-sensitive predicate in the capture path. On the dmabuf
  transport it also needs a GPU clear while the session is locked. Both costs
  buy only convenience.

### 3. Leave the pause as-is
- **Why considered**: The existing ADR text already says streams are not served
  while locked; "not served" reads like it covers this.
- **Why rejected**: "Not served" governs frames the compositor produces, not
  pixels the consumer already holds. The last delivered frame is retained, so
  the desktop and (on a mirror) the lock UI stay visible on the remote sink.

## Consequences

### Positive
- A lock, VT switch, or backend loss leaves no pre-lock pixel reachable by any
  producer.
- Uniform across SHM and dmabuf, output / window / virtual-output; no
  transport-specific code and no gate bypass.

### Negative / Trade-offs
- A cast does not survive a session lock: the consumer observes the stream end
  and must reconnect afterwards. This is the intended security posture, but it
  is a user-visible behaviour change from "the cast freezes" and must be noted
  in release notes.
