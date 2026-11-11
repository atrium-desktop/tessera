---
id: ADR-0228
title: "A Commit Repairs Its Own Buffer, Not the Frame's Damage Union"
status: accepted
date: 2026-11-09
scope: wayland/compositor, wayland/damage, runtime/damage
superseded_by: null
negative_knowledge: true
---

# ADR-0228: A Commit Repairs Its Own Buffer, Not the Frame's Damage Union

- Status: Accepted
- Date: 2026-11-09
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-wayland` (surface commit, SHM snapshot), `crates/tessera-composite` (client damage tracker), `crates/tessera` (frame damage)
- Amends: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md), [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md), [ADR-0210](0210-performance-core-affinity-allocation-zero-copy-strided-uploads-and-quiescent-lens-lifecycle.md)

---

## Context and Problem Statement

A Wayland surface may commit several times between two compositor frames. The
server accumulates every commit's damage into `committed_damage` for the frame
damage assessment and the renderer's texture upload. That aggregate answers
"what does this frame need to repaint/upload", and it is correct there.

It is **not** an instruction for repairing the retained SHM snapshot. The
snapshot is the surface's current content; each commit carries a **different
`wl_buffer`** (foot, like most terminals, alternates two). The buffered pixels
at an earlier commit's rectangles in the later commit's buffer are from an
older frame. Applying the aggregate to the later buffer therefore overwrites
the earlier commit's fresh pixels with that buffer's stale ones.

The visible result is a snapshot that walks backwards inside one frame:
typed text and the text caret return to an earlier position, then the next
commit adds new characters on top, then the next one rolls them back again.
Dragging the window forces a full placement repaint and converges every ring
image, which is why dragging appeared to "fix" it. Input was never lost; the
client's state was monotonic the whole time.

The bug is easy to miss because:

- it needs at least two commits with **different buffers** in one compositor
  frame;
- dma-buf clients (and maximized/fullscreen clients eligible for direct
  scanout) bypass the SHM snapshot entirely;
- a single-commit-per-frame test client never reproduces it.

## Decision Outcome

### 1. The snapshot copy takes only the committing buffer's damage

`apply_committed_buffer` receives the commit's own mapped damage slice
(`commit_damage`) and `unknown_full`. It copies only that slice from the new
buffer into the snapshot. The frame-level `committed_damage` remains untouched
for damage assessment and texture upload; it is never consumed by the copy.

### 2. Frame-level full is an unbounded latch while the interval is open

Once a commit in the interval cannot describe itself (empty damage on a new
buffer, unmappable buffer damage, or over-capacity), the frame-level list is
emptied and `committed_damage_full` stays set until the acknowledging present.
A later precise commit must not replace that verdict with its own narrower
rects: consumers read an empty list as "the whole surface changed". The later
commit still repairs its own rectangle in the snapshot through its per-commit
slice.

### 3. The distinct roles are named in the API

- **per-commit damage** (`pending_damage` / `pending_buffer_damage` →
  `commit_damage`): what to copy from *this* buffer into the snapshot.
- **frame damage** (`committed_damage` / `committed_damage_full`): what the
  frame must assess, repaint, and upload; cleared only by present.

`commit_snapshot_damage` makes the first role a named function instead of a
slice of the second.

## Consequences

### Positive

- A rapid typist no longer sees the caret and text roll back to an earlier
  buffer.
- Selection highlights cannot be partially reverted by a following commit.
- The renderer uploads the bounding box of the frame's accumulated damage from
  a snapshot that now holds the latest per-buffer result.
- Full-latched intervals conservatively repaint the whole surface instead of
  letting a later precise commit narrow the verdict.

### Negative

- The snapshot cannot be repaired from the accumulated list; each commit's
  damage must be carried to the copy site. This is one extra slice parameter
  and is strictly less work than re-copying earlier rects.

## Rejected Alternatives

### Copy the whole new buffer on every commit

Correct only when the client guarantees the new buffer equals the surface
outside its damage (the protocol does), but it destroys the incremental-copy
optimization that keeps SHM terminals cheap on HiDPI outputs. It also does not
fix the bug: the whole new buffer's undamaged pixels may be older than the
surface on an alternating buffer, which is exactly the case the snapshot must
ignore.

### Repair the snapshot at render time from the final buffer

The server releases SHM buffers immediately after copying, and the final
buffer is only one of the buffers the frame sampled. Re-reading it at render
time is both too late and semantically wrong: the frame-level damage does not
identify which buffer owns which rect.

### Disable SHM incremental copies

That trades a correctness bug for a whole-buffer memcpy on every commit — the
exact cost ADR-0210 removed — and would not fix the renderer's upload gating,
which shares the same per-commit/frame split.
