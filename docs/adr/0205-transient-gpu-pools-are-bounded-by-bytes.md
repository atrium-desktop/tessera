---
id: ADR-0205
title: "Transient GPU pools are bounded by bytes, not by entry count"
status: accepted
date: 2026-11-02
scope: rendering, material, optics-interop
superseded_by: null
negative_knowledge: true
---

# 0205. Transient GPU pools are bounded by bytes, not by entry count

- Status: Accepted
- Date: 2026-11-02
- Deciders: Tessera Maintainers
- Amends: Optics ADR-0069 (effect transient pooling), [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md)
- Related: [ADR-0202](0202-single-pass-full-output-backdrop-capture-and-glass-interior-compute-elision.md), [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md)

---

## Context and Problem Statement

The reference compositor held **1.49–1.56 GiB** of resident GPU memory
(`drm-resident-system0`) with two chrome bodies on screen: a HUD chip row and a
Dock. Nothing was leaking — the figure was stable to the megabyte across a
sixty-second sample — but it was far larger than the scene justified.

Budgeting the resident images at 3072×1920:

| consumer                                                     | bytes    |
| ------------------------------------------------------------ | -------- |
| 3 scanout images, RGBA8                                       | 68 MiB   |
| 3 backdrop-capture slots (`capture` + 2 layers × `composite`/`resolved`) | ~330 MiB |
| flux effect pool: 16 intermediates + 16 outputs, full-res      | up to 720 MiB RGBA8 / **1440 MiB RGBA16F** |
| wallpaper 3840×2160                                           | 32 MiB   |

The dominant term is the flux effect pool, and the reason is the *dimension*
its budget uses. `EFFECT_POOL_CAP` bounded each pool at 16 entries. On a small
display 16 transients cost a few MiB. At 3072×1920 an RGBA16F transient is
**45 MiB**, so 32 of them is 1.4 GiB for a desktop whose glass bodies occupy
perhaps 4 % of the panel.

The entry cap was chosen when the pool's growth mode was understood to be
"a handful of fixed-size blurs". The comment on the cap states the actual
failure mode it was added for: "turns a size-sweeping animation (resizes,
reveals) into one pooled image per visited size." That is a *bytes* problem
wearing an *entries* costume. A pool that admits 16 entries regardless of size
bounds the small case and unbounds the large one.

The second-order effect: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md)
removed the full-output repaint that had been masking this. Once the desktop
stops re-compositing on every frame, the pool stops being touched at a
different size each frame and settles — but the *bound* is still wrong for the
sizes it settles at.

## Decision Drivers

- A memory budget must be expressed in the unit that hurts. Entry count is a
  proxy that is correct only at one resolution.
- The pool must still guarantee progress: a single full-resolution transient
  larger than any reasonable per-entry share must be admissible when the pool
  is otherwise empty, or the first blur of a 4K frame could never run.
- Eviction must remain safe with batches in flight, which means it must keep
  using `flux_image_release`'s device retire queue rather than destroying
  images.
- The fix belongs in the pool, not in the callers. Every consumer of
  `flux_effect_blur`, `flux_effect_backdrop`, and `flux_effect_shadow` inherits
  it, including prism and lens.

## Considered Options

- **Lower `EFFECT_POOL_CAP` to 4.** Rejected: it bounds 4K at 180 MiB while
  starving a 1080p display that could comfortably hold 16 small transients. The
  dimension is still wrong; only the constant moved.
- **Cap the pool at each caller's expected working set.** Rejected: callers do
  not know the device's total budget, and three modules (blur, backdrop,
  shadow) would each have to duplicate the arithmetic.
- **Never pool; allocate per call.** Rejected: this is precisely the
  `vkCreateImage`/`vkAllocateMemory` thrash that
  [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md)
  §2 eliminated after measuring it as a stutter source.
- **Bound by bytes with a minimum-entry floor.** Chosen.

## Decision Outcome

Chosen option: **both transient pools are bounded by summed resident bytes,
with a minimum entry count that guarantees progress.**

```
EFFECT_POOL_BUDGET_BYTES = 256 MiB   (per pool)
EFFECT_POOL_MIN_ENTRIES  = 4
```

`transient_bytes(w, h, fmt)` is the entry's footprint: 4 bytes per pixel for
RGBA8, 8 for RGBA16F. `effect_state` carries `intermediate_bytes` and
`output_bytes`, updated on allocation and decremented on eviction, so the check
is O(1) and does not walk the list.

`pool_make_room` evicts LRU over *unleased* entries until the incoming entry
fits, and returns `false` when it cannot — every candidate is in flight. The
acquire functions then fail with `FLUX_ERROR_OUT_OF_MEMORY` and a
`FLUX_FAIL` detail, rather than growing past the budget. A caller that has
leased every transient in a 256 MiB pool is genuinely out of transient budget;
silently exceeded memory is how the 1.4 GiB state arose.

The minimum-entry floor is what makes a single operation possible at all. It is
set to exactly the widest simultaneous lease any effect takes: `flux_effect_shadow`
leases two intermediates and one output (ping/pong + result), `flux_effect_blur`
one and one. Anything less would make a single shadow impossible.

The two rules compose, and the composition is worth stating precisely because
the floor is what binds at 4K:

```
working ceiling per pool = max(EFFECT_POOL_BUDGET_BYTES, 4 * transient_bytes(w, h, fmt))
```

At 3072×1920 RGBA16F (45 MiB per transient) four entries fit inside 256 MiB and
the budget binds. At 3840×2160 RGBA16F (66 MiB) four entries are 264 MiB, eight
above the nominal budget — so the floor, not the budget, is the ceiling. That
overrun is bounded by the floor's definition (four), and it is the price of
admitting a single 4K shadow at all. A budget that overrode the floor would make
every 4K shadow fail, which is a worse outcome than an 8 MiB overshoot.

When a pool genuinely cannot admit an entry — the budget is reached and every
candidate is leased — the operation fails with `FLUX_ERROR_OUT_OF_MEMORY` and a
detail string naming the budget, the resident entry count and bytes, the
requested extent, and that every evictable entry is in flight. A silent
overshoot is how the 1.4 GiB state arose; a silent *failure* would be worse, so
it is loud.

The captured/screenshot and backdrop-graph images are **not** in these pools;
they are owned by `BackdropCapture` and the surface, and remain governed by
[ADR-0202](0202-single-pass-full-output-backdrop-capture-and-glass-interior-compute-elision.md)
and the ring-slot lifetime rules. This ADR bounds the transient churn beneath
them.

### Invariants & Behavioral Boundaries

Each invariant names its enforcement mechanism. The `test_inv_*` naming is the
repository's audit convention for the invariant set
(`docs/dev/standards/invariants.md`).

- `[INV-ARCH-98] Transient pools are byte-bounded.` No code path may admit a
  pooled transient that would take its pool past `EFFECT_POOL_BUDGET_BYTES`
  while at or above `EFFECT_POOL_MIN_ENTRIES`. Enforced by
  `tests/flux/integration/test_effect_pool.c`, which was verified to fail when
  the byte budget is reverted to an entry cap.
- Eviction targets only entries with `leased == false`; a leased entry is owned
  by an in-flight batch and must survive.
- Eviction uses `flux_image_release`, never direct destruction, so resources
  park on the device retire queue and concurrent submissions stay valid.
- The resident-byte counters are the authority; they must be updated on both
  admission and eviction in the same critical section as the list mutation.
- A pool at its budget with every entry leased fails the operation; it does not
  grow, and it does not silently alias two leases onto one image.
- `tests/flux/integration/test_effect_pool.c` drives forty distinct transient
  sizes through `flux_effect_blur` — more than any entry-count cap would admit —
  and asserts the run completes, that a repeated size reuses its pooled entry,
  and that the device still submits afterwards. Reverting the byte budget to an
  entry cap makes it fail.

## Consequences

### Positive

- The steady state drops from a possible 1.4 GiB of transients to at most
  512 MiB across both pools (plus the floor's bounded overshoot on a 4K
  display), and in practice far less because normal chrome uses two or three
  transients.
- The bound is resolution-independent: a 1080p display still gets as many small
  transients as it can usefully hold.
- Callers inherit the bound; prism and lens need no change.

### Negative / Risks

- `EFFECT_POOL_BUDGET_BYTES` is a fixed constant, not derived from the device's
  reported heap size. On a small discrete GPU it is conservative; on an
  integrated GPU with shared system memory it is generous. Deriving it would
  require trusting `VK_EXT_memory_budget`/`heapBudget`, which is advisory and
  not universal. A fixed budget with a documented rationale is preferable to a
  reported one that disagrees between drivers.
- A workload that legitimately needs more than 256 MiB of same-size transients
  would start failing allocations rather than slowing down. No such workload
  exists in the compositor today; the failure is loud and labelled.
- The LRU walk is O(entries) per eviction. With the byte budget the entry count
  per pool is bounded by `budget / smallest_transient`, which at typical chrome
  sizes is under sixty — the same order as the old cap.

## Rejected Alternatives & Negative Knowledge

### Keep 16 entries and shrink the working resolution
- **Why considered**: `BACKDROP_DOWNSAMPLE` is `1`; halving it would quarter
  the transient sizes without touching the pool.
- **Why rejected**: it changes what the glass lens can sample. Liquid-glass
  refraction reads the sharp capture directly
  ([ADR-0202](0202-single-pass-full-output-backdrop-capture-and-glass-interior-compute-elision.md)),
  so a downsampled capture trades a visible quality regression for memory the
  budget can reclaim more cheaply. The pool is the right place.

### Evict aggressively on every frame
- **Why considered**: a pool that empties itself each frame has a trivially
  bounded peak.
- **Why rejected**: it converts a steady-state reuse into per-frame
  `vkCreateImage`, reintroducing ADR-0179 §2's allocation thrash. The pool
  exists to make a repeating operation allocation-free.
