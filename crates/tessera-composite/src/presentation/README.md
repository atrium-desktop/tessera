# tessera-presentation (Presentation module)

Pure presentation-domain value layer for the tessera compositor, housed within `tessera-composite`.

This module holds the compositor-side *facts* of the output-damage pipeline —
the current frame's `FrameDamage`, surface damage baselines, the logical→
physical damage mapping, and the conversion to/from Flux's native damage
verdict — as pure functions over `tessera-primitives` geometry types. Per-image
epoch history and the derived repaint/scanout scopes belong to Flux (Optics
ADR-0155); this module deliberately keeps no second ledger. It also defines the
narrow `SurfaceDamageFrame` observation seam that the composition root
implements when feeding client surfaces into the `ClientDamageTracker`.

`AssessedFrameDamage` carries the resource ids of every surface the assessment
sampled alongside the damage verdicts. Those ids are the caller's evidence for
which client buffers the presented frame owns, so a fullscreen or uncapped
client's superseded buffers are released at the correct time
(`[INV-ARCH-70]`, [ADR-0182](../../../../docs/adr/0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)).

No compositor state, no IPC, no engine handles, no side effects. The
orchestrating damage assessment (change-signal sampling, wallpaper and
notification policies) lives with the composition root and consumes this
crate's value types.
