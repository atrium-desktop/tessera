# tessera-presentation

Pure presentation-domain value layer for the tessera compositor.

This crate holds the *facts* of the output-damage pipeline — frame damage
verdicts (`FrameDamage`), the per-consumer damage assessment split, the
swapchain slot-ring repaint history, surface damage baselines, and the
logical→physical damage mapping — as pure functions over `tessera-model`
geometry types. It also defines the narrow `SurfaceDamageFrame` observation
seam that the composition root implements when feeding client surfaces into
the `ClientDamageTracker`.

No compositor state, no IPC, no engine handles, no side effects. The
orchestrating damage assessment (change-signal sampling, wallpaper and
notification policies) lives with the composition root and consumes this
crate's value types.
