//! The painted canvas beneath the control-center cluster (ADR-0176).

use super::*;

impl ControlCenter {
    /// The opaque scheme-adaptive canvas under the whole panel (ADR-0176).
    ///
    /// A full-display solid fill in the `Backdrop` band — below every chrome
    /// surface — faded by the panel's reveal spring. It is the panel's own
    /// painted background: the dark scheme paints the near-black grouped
    /// canvas, the light scheme the pale grouped canvas. It replaces the
    /// retired full-screen backdrop blur + scrim cover, so the panel no longer
    /// captures, blurs, or composites the desktop behind it.
    ///
    /// `reveal` is the eased reveal progress: the canvas alpha scales with it,
    /// so an exit drains the canvas with the cluster it backs instead of
    /// popping out at teardown. The `Backdrop` band keeps it under `Chrome`
    /// surfaces regardless of build order.
    pub(crate) fn render_canvas(
        hud: &ControlCenterColors,
        f: &mut Frame,
        display: (f32, f32),
        reveal: f32,
    ) {
        let alpha = canvas_alpha(reveal);
        if alpha == 0 || display.0 <= 0.0 || display.1 <= 0.0 {
            return;
        }
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: display.1,
        };
        f.place(
            "tessera-hud-canvas",
            &PlaceOpts {
                band: Band::Backdrop,
                mode: PlaceMode::Exact,
                rect,
                layout: LayoutOpts {
                    bg: hud.background.with_alpha(alpha),
                    pad: 0.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            |f| {
                f.row_ex(&sized(rect.w, rect.h), |_| {});
            },
        );
    }
}

/// The painted canvas's alpha for a reveal progress, `0..=255` (ADR-0176).
///
/// Pure so the drain behaviour is unit-testable without a GPU: zero at a
/// hidden panel, fully opaque at a presented one, proportional in between, so
/// the canvas drains with the reveal instead of popping at teardown.
pub(crate) fn canvas_alpha(reveal: f32) -> u8 {
    (reveal.clamp(0.0, 1.0) * 255.0).round() as u8
}
