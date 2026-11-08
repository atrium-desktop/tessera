---
id: ADR-0223
title: "A failed chrome build is isolated, not fatal — the last good scene stays and the retry is paced"
status: accepted
date: 2026-11-06
scope: shell/runtime, shell/components
superseded_by: null
negative_knowledge: true
---

# ADR-0223: A failed chrome build is isolated, not fatal — the last good scene stays and the retry is paced

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-shell` (scene build, submit, component pass),
  `crates/tessera-composite` (canvas pass), `libs/lens` and `libs/flux`
  (encoder argument validation)
- Amends: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md),
  [ADR-0210](0210-performance-core-affinity-allocation-zero-copy-strided-uploads-and-quiescent-lens-lifecycle.md)
- Related: [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md)

---

## Context and Problem Statement

A live session logged this, repeatedly, at output rate:

```
WARN shell: chrome scene build failed: shell render: Render(FLUX_ERROR_INVALID_ARGUMENT)
```

The message alone is not remarkable — a chrome build can fail for a mundane
reason. What made it an architectural defect was that **one failure was
permanent**, and the cost of a permanent failure was unbounded.

### The failure amplifies itself

`Shell::prepare_frame` decided whether to build a scene like this:

```rust
let needs_build =
    self.dirty || input_active || self.anim_pending() || self.pending_scene.is_none();
```

and `build_scene` published the scene only on success:

```rust
let snapshot = self.ui.snapshot().map_err(ShellError::Render)?;
self.pending_scene = Some(snapshot);
self.dirty = false;
```

The two combine into a loop. A failed build leaves `pending_scene == None`,
which makes `needs_build` true **regardless of every other input** — so the next
frame rebuilds the entire chrome tree, fails again, and logs again. `dirty` is
never cleared, so there is no way out. The shell rebuilt its complete widget
tree, ran lens layout, and built a display list, once per frame, indefinitely,
for a defect that costs nothing to *keep* failing.

### One bad command destroys the whole tree

`FLUX_ERROR_INVALID_ARGUMENT` from `lens_snapshot_create` does not come from the
shell. It comes from flux's encoder argument validation: a non-finite rect,
radius or opacity, a gradient whose stops are not monotone, a stroke width that
degenerated. The encoder's error is **sticky** — `flux_encoder_fail` latches it
and every later draw in that encoder is dropped — so one degenerate value in one
component discards the entire chrome tree, not the offending widget.

That is the right behaviour for a command stream (a partially-applied frame is
worse than a rejected one) but it means the blast radius of one NaN is the whole
desktop chrome.

### The degradation was the wrong one

`submit` answered a `None` scene by drawing nothing:

```rust
let Some(snapshot) = self.pending_scene.as_ref() else {
    return Ok(());
};
```

The comment called this "a chrome-less desktop rather than dropping the
presentation". But chrome is not decoration — the dock *is* the way a user
launches, focuses and closes applications, and the command panel is the way out
of a stuck state. A chrome-less desktop is a much worse failure mode than
slightly stale chrome, and it is the state the user is least equipped to
recover from.

---

## Decision Drivers

- **A failure must not set its own retry frequency.** Cost per unit time must be
  bounded by policy, not by how often the loop happens to run.
- **Degrade to the last known-good state, not to nothing.** Stale chrome is
  information; no chrome is a broken session.
- **Recovery must remain possible.** A transient failure — a spring overshoot
  producing a non-finite rect for a few frames — must heal on its own.
- **Never hide the defect.** A failure has to stay visible in the log and in the
  code, or it becomes a permanent silent regression.

---

## Decision Outcome

### 1. The last successfully built scene is retained

`Shell` gains:

```rust
/// The last scene that was successfully built, retained so a failed build
/// degrades to the previous chrome instead of to no chrome at all.
last_good_scene: Option<lens::Snapshot>,
```

set in `build_scene` immediately after `ui.snapshot()` succeeds. `lens::Snapshot`
is refcounted (`lens_snapshot_retain`), so retaining it costs an atomic
increment, not a copy of the display list.

### 2. `submit` draws the pending scene, or the last good one

```rust
let Some(scene) = self.pending_scene.as_ref().or(self.last_good_scene.as_ref()) else {
    return Ok(());
};
unsafe { scene.submit(canvas as *mut lens::sys::flux_canvas) }?;
self.ui.activate(scene)?;
self.pending_scene = None;
```

`pending_scene` is cleared only because a *built* scene was consumed; the
fallback reference stays so it can be drawn again next frame. A failed build
therefore costs a stale frame, not a missing one.

### 3. The retry is paced by a deadline, not by the loop

`Shell` gains `build_retry_at: Option<Instant>`, and `prepare_frame` refuses to
rebuild inside the window:

```rust
if needs_build && self.build_retry_at.is_some_and(|at| Instant::now() < at) {
    return;
}
```

`BUILD_RETRY_BACKOFF` is 250 ms — the window's own idle-tick order of magnitude.
Fast enough that a transient failure recovers before a user notices; slow enough
that a persistent one costs four rebuild attempts per second instead of one per
frame, a reduction of roughly two orders of magnitude at 120 Hz. A successful
build clears the deadline.

### 4. The failure stays loud

The log line now names the degradation and the retry interval:

```
shell: chrome scene build failed: <error>; drawing the last good scene and
retrying in 250ms
```

Isolation is not suppression. The defect is still a defect, still logged on every
attempt, and still a `WARN`.

---

## Consequences

### Positive

- **Cost per unit time is bounded by policy.** A persistent build failure costs
  ~4 rebuilds/second instead of ~120, and each attempt is the same work as
  before.
- **The desktop stays usable.** The dock keeps drawing, so the user can still
  launch, focus and close applications while the defect exists.
- **Transient failures self-heal.** The deadline still fires, so a non-finite
  value that only appears at some animation phases resolves as soon as the phase
  passes.
- **The defect remains visible** in the log at every attempt, with an explicit
  statement of what the shell did instead.

### Negative / Trade-offs

- **Chrome can be stale for up to one backoff window.** During a persistent
  failure the dock shows state from before the failure. This is the deliberate
  trade: a stale dock is recoverable, an absent one is not.
- **One retained snapshot lives as long as the shell.** One display list's worth
  of memory, held for the purpose of being able to draw it again. Measured
  against the alternative (losing the chrome), it is not a close call.
- **The backoff masks repeated failures in timing terms.** Four warnings per
  second instead of 120 makes the log readable, but a persistent failure can now
  hide behind a slow drumbeat. Accepted: an unreadable log was the worse outcome,
  and the failure is still reported on every attempt.

---

## Rejected Alternatives & Negative Knowledge

### 1. Roll back the offending frame and let the next build retry immediately

- **Why considered**: keeps recovery latency at one frame, which is ideal for a
  transient failure.
- **Why rejected**: it is exactly the livelock. `pending_scene == None` already
  forces a rebuild every frame; "retry immediately" *is* the current behaviour,
  and its cost is the whole widget tree plus a log line per frame. Recovery
  latency for a transient failure is already one backoff window, which is well
  inside perception.

### 2. Fall back to a chrome-less desktop (retain the old `return Ok(())`)

- **Why considered**: it is what the code already did, and it is honest — it does
  not show the user state that is not true.
- **Why rejected**: the dock is the session's recovery surface. Removing it
  during a failure removes the user's ability to act, turning a cosmetic defect
  into an unusable session. "Honest" here means showing a stale dock, not an
  empty screen.

### 3. Make the lens encoder non-sticky so only the bad command is dropped

- **Why considered**: it addresses the root cause — one NaN would then cost one
  widget instead of the tree, and the livelock would not matter.
- **Why rejected as the *primary* fix**: a command stream that silently drops
  individual draws produces a frame that is partially applied, which is harder to
  reason about than a rejected frame, and it would hide the defect that produced
  the NaN. (It remains worth doing *in addition*: see "Follow-up" below.) The
  isolation rule is needed regardless, because a rejected build has non-command
  causes too — a device-lost surface, an out-of-memory display list.

### 4. Validate every chrome rect at the emit site and clamp the NaN

- **Why considered**: prevents the failure rather than containing it.
- **Why rejected as the whole answer**: it requires every component to be
  individually correct about finiteness across every animation phase, which is an
  unbounded set of obligations — the same class of design error as a
  hand-maintained dirty flag. It is the right *additional* work (and the reason
  the dock's `column_plan` guards its extents), but it cannot be the only line of
  defence, because the next component will get it wrong.

---

## Follow-up

The degradation above converts a per-frame avalanche into a paced, visible
failure. The defect that produced the degenerate value still has to be found:
`FLUX_ERROR_INVALID_ARGUMENT` from the shell means a non-finite or
out-of-contract draw argument, and the reachable sources are chrome layout
arithmetic during reveal springs (the dock's `column_plan` / `row_rects`
extents, `content_progress`-scaled rects, theme-derived gradient stops). Tracing
it needs the flux diagnostic (`flux_last_error_detail`) printed alongside the
error, which this change does not add.

---

## Links

- Amends [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md): the scene a
  frame draws is no longer assumed to be this frame's build.
- Amends [ADR-0210](0210-performance-core-affinity-allocation-zero-copy-strided-uploads-and-quiescent-lens-lifecycle.md):
  `[INV-ARCH-103]` quiescence now also holds across a build failure.
- Related [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md):
  same class of amplifier — an unbounded per-frame cost driven by a small defect.
- Optics ADR-0149 (damage-scoped frames), whose canvas-pass rejection is one of
  the failure surfaces this contains.
