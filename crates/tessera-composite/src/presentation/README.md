# tessera-composite (Presentation module)

Pure presentation-domain value layer for the tessera compositor, housed within `tessera-composite`.

This module holds the compositor-side *facts* of the output-damage pipeline —
the current frame's `FrameDamage`, surface damage baselines, the logical→
physical damage mapping, and the conversion to/from gfx's native damage
verdict — as pure functions over `tessera-primitives` geometry types. Per-image
epoch history and the derived repaint/scanout scopes belong to gfx (Optics
ADR-0155); this module deliberately keeps no second ledger. It also defines the
narrow `SurfaceDamageFrame` observation seam that the composition root
implements when feeding client surfaces into the `ClientDamageTracker`.

`AssessedFrameDamage` carries the resource ids of every surface the assessment
sampled alongside the damage verdicts. Those ids are the caller's evidence for
which client buffers the presented frame owns, so a fullscreen or uncapped
client's superseded buffers are released at the correct time
(`[INV-ARCH-70]`, [ADR-0182](../../../../docs/adr/0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)).

The output-layer planners are also pure value layers. `scanout` evaluates the
single full-output primary-plane candidate; `overlay` evaluates one topmost,
opaque, single-surface overlay candidate and returns a stable rejection reason
when any scene, surface, or plane fact disqualifies it. Neither module imports
a buffer or allocates a plane: the backend still owns the complete-set
`TEST_ONLY` proof (ADR-0234, ADR-0235).

No compositor state, no IPC, no engine handles, no side effects. The
orchestrating damage assessment (change-signal sampling, wallpaper and
notification policies) lives with the composition root and consumes this
crate's value types.
