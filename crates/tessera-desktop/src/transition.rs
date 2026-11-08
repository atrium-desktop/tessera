//! Declarative window transitions (ADR-0029).
//!
//! When the window manager changes a toplevel's rectangle non-interactively
//! (tiling, maximize, snap, IPC geometry), it records the previous and
//! target rectangles and a start time. The model always reports the target;
//! the server interpolates the rectangle for rendering only, so a transition
//! never mutates the state the chrome, the IPC, or the agent read. The
//! reduced-motion policy resolves every transition in at most one frame.

use crate::dock::MinimizeAnimationStyle;
use tessera_primitives::{Point, Rect, Size};

/// Default transition length. Short enough to feel instant, long enough to
/// read as motion.
pub const DEFAULT_DURATION_MS: u64 = 180;

/// Minimize/restore transition lengths per style. Genie runs longest so the
/// funnel deformation stays readable; scale and suck stay snappy.
pub const MINIMIZE_GENIE_DURATION_MS: u64 = 360;
pub const MINIMIZE_SCALE_DURATION_MS: u64 = 200;
pub const MINIMIZE_SUCK_DURATION_MS: u64 = 240;

/// Duration of the window open (map) transition: a quick grow that reads as
/// "appearing" without delaying interaction.
pub const OPEN_DURATION_MS: u64 = 200;

/// Duration of the window close (unmap) transition.
pub const CLOSE_DURATION_MS: u64 = 180;

/// Fraction of the close flight over which the geometry shrink happens.
///
/// The close animation is not one motion but two: a *scale* that should read as
/// the window leaving, and an *alpha* that must reach zero smoothly on the last
/// painted frame or the ghost pops off the screen (ADR-0220). `EaseInCubic` is
/// right for the scale — it starts imperceptibly and accelerates into the exit —
/// but applied to alpha it is catastrophic: `1 - t³` is still **0.875 at the
/// halfway point** and holds 0.58 at three quarters, so it dumps the last
/// `37 %` of its range into the final quarter and drops ~`0.24` in one 60 Hz
/// frame. The shrink therefore finishes at [`CLOSE_GEOMETRY_FRACTION`] of the
/// timeline while the opacity runs the full duration on its own curve.
pub const CLOSE_GEOMETRY_FRACTION: f32 = 0.8;

/// The interpolation curve of a [`WindowTransition`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Easing {
    /// `1 - (1 - t)³`: fast start, gentle settle — the standard compositor
    /// curve.
    #[default]
    EaseOutCubic,
    /// `t³`: slow start, accelerating finish — the "sucked in" feel.
    EaseInCubic,
}

impl Easing {
    /// Apply the curve to a linear progress value in `0.0..=1.0`.
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Easing::EaseOutCubic => ease_out_cubic(t),
            Easing::EaseInCubic => ease_in_cubic(t),
        }
    }
}

/// Presentation-only extra data for transitions that are more than a plain
/// rectangle interpolation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEffect {
    /// A minimize (or reversed-restore) flight into the dock. `target` is the
    /// dock icon's centre; `style` selects how the flight renders. Only the
    /// genie style needs renderer support (a strip warp); scale and suck are
    /// pure rectangle interpolations distinguished by their target and
    /// easing.
    Minimize {
        style: MinimizeAnimationStyle,
        target: Point,
    },
    /// A window opening: a fade-and-scale in from a slightly inset rect.
    /// The rectangle interpolation grows from `from` to the window rect
    /// while the renderer raises opacity with the same progress; the
    /// combination reads as the window "fading in".
    Open,
    /// A window closing: a fade-and-scale out toward a slightly inset rect.
    /// The model rect is gone, so the renderer keeps the last presented
    /// buffer and drives the same interpolation in reverse while opacity
    /// falls to zero. `from` holds the window's final rect (the flight
    /// starts at the full rect and shrinks toward the inset target).
    Close,
}

/// One in-flight geometry transition. `to` is always the window's model
/// rect, so it is not stored here; [`WindowTransition::rect_at`] takes it as
/// an argument.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowTransition {
    /// The rect the window is animating away from.
    pub from: Rect,
    /// Compositor-relative millisecond timestamp the transition started at.
    pub started_ms: u64,
    /// How long the transition runs, in milliseconds.
    pub duration_ms: u64,
    /// The interpolation curve. Everything but the suck minimize style keeps
    /// the default ease-out.
    #[cfg_attr(feature = "serde", serde(default))]
    pub easing: Easing,
    /// Optional presentation effect (minimize flight) layered on top of the
    /// rectangle interpolation.
    #[cfg_attr(feature = "serde", serde(default))]
    pub effect: Option<TransitionEffect>,
}

impl WindowTransition {
    /// The opacity multiplier this transition implies at `now_ms`: `None`
    /// means fully opaque (no fade), `Some(a)` is the alpha to paint with.
    ///
    /// Each effect owns its own curve, because a fade and a geometry flight
    /// answer different questions (ADR-0220). A close ghost's alpha must reach
    /// zero *smoothly and at the last frame*; driving it off the shrink's
    /// `EaseInCubic` clock made the final 16 ms drop a third of the remaining
    /// opacity, which is the "the animation just vanishes" the ghost used to
    /// read as. The close therefore runs `smoothstep` over the whole timeline
    /// while its rectangle finishes at [`CLOSE_GEOMETRY_FRACTION`].
    pub fn opacity_at(&self, now_ms: u64) -> Option<f32> {
        let linear = self.linear_progress_at(now_ms)?;
        match self.effect? {
            // A fade-in reads better easing *out* of the transparent start, so
            // the window is legible early instead of snapping into view.
            TransitionEffect::Open => Some(self.easing.apply(linear)),
            TransitionEffect::Close => Some(1.0 - smoothstep(linear)),
            TransitionEffect::Minimize { .. } => None,
        }
    }
    /// Start a transition from `from` at `now_ms` with the default duration.
    pub fn new(from: Rect, now_ms: u64) -> WindowTransition {
        WindowTransition {
            from,
            started_ms: now_ms,
            duration_ms: DEFAULT_DURATION_MS,
            easing: Easing::default(),
            effect: None,
        }
    }

    /// Start a minimize (or reversed restore) transition from `from` toward
    /// `icon_rect` — the window's dock tile. The style picks the duration,
    /// easing, and the exact target rectangle: scale and genie land on the
    /// icon itself, suck collapses into the icon's centre point.
    pub fn minimize(
        from: Rect,
        now_ms: u64,
        style: MinimizeAnimationStyle,
        icon_rect: Rect,
    ) -> WindowTransition {
        let centre = Point {
            x: icon_rect.origin.x + icon_rect.size.w / 2,
            y: icon_rect.origin.y + icon_rect.size.h / 2,
        };
        let (duration_ms, easing) = match style {
            MinimizeAnimationStyle::Genie => (MINIMIZE_GENIE_DURATION_MS, Easing::EaseOutCubic),
            MinimizeAnimationStyle::Scale => (MINIMIZE_SCALE_DURATION_MS, Easing::EaseOutCubic),
            MinimizeAnimationStyle::Suck => (MINIMIZE_SUCK_DURATION_MS, Easing::EaseInCubic),
        };
        WindowTransition {
            from,
            started_ms: now_ms,
            duration_ms,
            easing,
            effect: Some(TransitionEffect::Minimize {
                style,
                target: centre,
            }),
        }
    }

    /// Start a window-open transition from `from` (a slightly inset rect) at
    /// `now_ms`. The window's model rect is the target.
    pub fn open(from: Rect, now_ms: u64) -> WindowTransition {
        WindowTransition {
            from,
            started_ms: now_ms,
            duration_ms: OPEN_DURATION_MS,
            easing: Easing::EaseOutCubic,
            effect: Some(TransitionEffect::Open),
        }
    }

    /// Start a window-close transition: the flight begins at the window's
    /// final rect (`from`) and interpolates toward a slightly inset rect
    /// while opacity fades to zero. The renderer needs `from` because the
    /// model window is gone once the close begins.
    pub fn close(from: Rect, now_ms: u64) -> WindowTransition {
        WindowTransition {
            from,
            started_ms: now_ms,
            duration_ms: CLOSE_DURATION_MS,
            easing: Easing::EaseInCubic,
            effect: Some(TransitionEffect::Close),
        }
    }

    /// Whether this transition is still in flight at `now_ms`.
    ///
    /// Keep the lifetime predicate independent of the current target rect so
    /// every consumer (scene visibility, occlusion, callbacks, and rendering)
    /// agrees on the exact instant a transition settles.
    pub fn is_active_at(&self, now_ms: u64) -> bool {
        self.duration_ms > 0 && now_ms.saturating_sub(self.started_ms) < self.duration_ms
    }

    /// The eased progress in `0.0..=1.0` at `now_ms`, or `None` when the
    /// transition has settled. Effects that deform rather than interpolate
    /// (the genie warp) drive themselves from this value.
    ///
    /// For a [`TransitionEffect::Close`] this is the *geometry* clock: the
    /// shrink is compressed into the first [`CLOSE_GEOMETRY_FRACTION`] of the
    /// timeline so the alpha — which has its own, gentler curve in
    /// [`Self::opacity_at`] — is what the user sees at the end (ADR-0220).
    pub fn progress_at(&self, now_ms: u64) -> Option<f32> {
        self.linear_progress_at(now_ms)
            .map(|linear| self.easing.apply(self.geometry_clock(linear)))
    }

    /// The un-eased fraction of the timeline elapsed at `now_ms`, in
    /// `0.0..=1.0`, or `None` when the transition has settled.
    pub fn linear_progress_at(&self, now_ms: u64) -> Option<f32> {
        if !self.is_active_at(now_ms) {
            return None;
        }
        let elapsed = now_ms.saturating_sub(self.started_ms);
        Some((elapsed as f32 / self.duration_ms as f32).clamp(0.0, 1.0))
    }

    /// Remap linear time onto the geometry curve's own 0..1 span.
    ///
    /// Only the close flight splits its clocks; every other effect keeps the
    /// whole timeline for its rectangle, so nothing else changes shape.
    fn geometry_clock(&self, linear: f32) -> f32 {
        match self.effect {
            Some(TransitionEffect::Close) => (linear / CLOSE_GEOMETRY_FRACTION).clamp(0.0, 1.0),
            _ => linear,
        }
    }

    /// The interpolated rect at `now_ms`, or `None` when the transition has
    /// settled and the window renders at `target`.
    pub fn rect_at(&self, target: Rect, now_ms: u64) -> Option<Rect> {
        let t = self.progress_at(now_ms)?;
        Some(lerp_rect(self.from, target, t))
    }
}

/// The rectangle a minimize flight aims at: the icon rect itself, or — for
/// the suck style — a point at the icon's centre.
pub fn minimize_target_rect(style: MinimizeAnimationStyle, icon_rect: Rect) -> Rect {
    match style {
        MinimizeAnimationStyle::Genie | MinimizeAnimationStyle::Scale => icon_rect,
        MinimizeAnimationStyle::Suck => Rect {
            origin: Point {
                x: icon_rect.origin.x + icon_rect.size.w / 2 - 1,
                y: icon_rect.origin.y + icon_rect.size.h / 2 - 1,
            },
            size: Size { w: 2, h: 2 },
        },
    }
}

/// `1 - (1 - t)³`: fast start, gentle settle — the standard compositor curve.
pub fn ease_out_cubic(t: f32) -> f32 {
    let u = (1.0 - t.clamp(0.0, 1.0)).powi(3);
    1.0 - u
}

/// `t³`: slow start, accelerating finish.
pub fn ease_in_cubic(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(3)
}

/// `t²(3 - 2t)`: zero slope at both ends, which is what a *cross-fade* needs.
///
/// A cubic ease lands on its endpoint with a non-zero slope, so the final
/// sampled frame of a fade carries a visible opacity step. Smoothstep's
/// matching zero-velocity tangents mean the last in-flight frame is already
/// within a couple of levels of transparent: the ghost dissolves instead of
/// switching off (ADR-0220).
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Interpolate two rects component-wise, rounding to the nearest pixel.
pub fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * t).round() as i32;
    Rect {
        origin: Point {
            x: lerp(from.origin.x, to.origin.x),
            y: lerp(from.origin.y, to.origin.y),
        },
        size: Size {
            w: lerp(from.size.w, to.size.w).max(1),
            h: lerp(from.size.h, to.size.h).max(1),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_endpoints_and_shape() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        // Ease-out: ahead of linear early, catching up late.
        assert!(ease_out_cubic(0.25) > 0.25);
        assert!(ease_out_cubic(0.75) < 0.875 + 0.125);
        assert!(ease_out_cubic(0.75) > 0.75 - 0.25);
        // Monotonic.
        assert!(ease_out_cubic(0.3) < ease_out_cubic(0.6));
    }

    #[test]
    fn rect_at_interpolates_then_settles() {
        let from = Rect::new(0, 0, 100, 100);
        let target = Rect::new(100, 50, 200, 150);
        let tr = WindowTransition::new(from, 1000);

        // t=0 is still in flight, exactly at `from`.
        assert_eq!(tr.rect_at(target, 1000), Some(from));

        let mid = tr.rect_at(target, 1075).expect("mid-flight");
        assert!(mid.origin.x > 0 && mid.origin.x < 100, "mid x: {mid:?}");
        assert!(mid.size.w > 100 && mid.size.w < 200, "mid w: {mid:?}");

        assert!(tr.rect_at(target, 1180).is_none(), "settled at duration");
        assert!(tr.rect_at(target, 5000).is_none(), "settled long after");
        // A zero-duration transition resolves immediately (reduced motion).
        let instant = WindowTransition {
            duration_ms: 0,
            ..tr
        };
        assert!(instant.rect_at(target, 1000).is_none());
    }

    #[test]
    fn lerp_rect_hits_endpoints() {
        let from = Rect::new(10, 20, 100, 100);
        let to = Rect::new(110, 120, 300, 200);
        assert_eq!(lerp_rect(from, to, 0.0), from);
        assert_eq!(lerp_rect(from, to, 1.0), to);
        let mid = lerp_rect(from, to, 0.5);
        assert_eq!(mid.origin, Point { x: 60, y: 70 });
        assert_eq!(mid.size, Size { w: 200, h: 150 });
    }

    #[test]
    fn ease_in_cubic_endpoints_and_shape() {
        assert_eq!(ease_in_cubic(0.0), 0.0);
        assert_eq!(ease_in_cubic(1.0), 1.0);
        // Ease-in: behind linear early, accelerating late.
        assert!(ease_in_cubic(0.25) < 0.25);
        assert!(ease_in_cubic(0.75) > 0.25);
        assert!(ease_in_cubic(0.3) < ease_in_cubic(0.6));
    }

    #[test]
    fn minimize_transition_carries_style_easing_and_icon_centre() {
        let from = Rect::new(100, 100, 800, 600);
        let icon = Rect::new(500, 1020, 48, 48);
        let tr = WindowTransition::minimize(from, 1000, MinimizeAnimationStyle::Suck, icon);
        assert_eq!(tr.duration_ms, MINIMIZE_SUCK_DURATION_MS);
        assert_eq!(tr.easing, Easing::EaseInCubic);
        assert_eq!(
            tr.effect,
            Some(TransitionEffect::Minimize {
                style: MinimizeAnimationStyle::Suck,
                target: Point { x: 524, y: 1044 },
            })
        );

        let genie = WindowTransition::minimize(from, 1000, MinimizeAnimationStyle::Genie, icon);
        assert_eq!(genie.duration_ms, MINIMIZE_GENIE_DURATION_MS);
        assert_eq!(genie.easing, Easing::EaseOutCubic);
    }

    #[test]
    fn progress_at_uses_the_transition_easing() {
        let from = Rect::new(0, 0, 100, 100);
        let icon = Rect::new(500, 1020, 48, 48);
        let suck = WindowTransition::minimize(from, 1000, MinimizeAnimationStyle::Suck, icon);
        let half = suck.duration_ms / 2;
        let progress = suck.progress_at(1000 + half).expect("in flight");
        // Ease-in at the midpoint is well behind linear.
        assert!(progress < 0.5, "progress: {progress}");
        assert!(suck.progress_at(1000 + suck.duration_ms).is_none());
        assert!(suck.progress_at(1000).is_some());
    }

    #[test]
    fn minimize_target_rect_collapses_to_a_point_only_for_suck() {
        let icon = Rect::new(500, 1020, 48, 48);
        assert_eq!(
            minimize_target_rect(MinimizeAnimationStyle::Genie, icon),
            icon
        );
        assert_eq!(
            minimize_target_rect(MinimizeAnimationStyle::Scale, icon),
            icon
        );
        let suck = minimize_target_rect(MinimizeAnimationStyle::Suck, icon);
        assert_eq!(suck.size, Size { w: 2, h: 2 });
        // The point is centred on the icon.
        assert_eq!(suck.origin.x + 1, icon.origin.x + 24);
        assert_eq!(suck.origin.y + 1, icon.origin.y + 24);
    }

    #[test]
    fn open_and_close_transitions_carry_effect_duration_and_easing() {
        let rect = Rect::new(100, 100, 800, 600);
        let open = WindowTransition::open(rect, 1000);
        assert_eq!(open.duration_ms, OPEN_DURATION_MS);
        assert_eq!(open.easing, Easing::EaseOutCubic);
        assert_eq!(open.effect, Some(TransitionEffect::Open));
        assert!(open.rect_at(rect, 1000).is_some());
        assert!(open.rect_at(rect, 1000 + OPEN_DURATION_MS).is_none());

        let close = WindowTransition::close(rect, 1000);
        assert_eq!(close.duration_ms, CLOSE_DURATION_MS);
        assert_eq!(close.easing, Easing::EaseInCubic);
        assert_eq!(close.effect, Some(TransitionEffect::Close));
        assert!(close.rect_at(rect, 1000).is_some());
        assert!(close.rect_at(rect, 1000 + CLOSE_DURATION_MS).is_none());
    }

    #[test]
    fn opacity_at_fades_open_in_and_close_out() {
        let rect = Rect::new(0, 0, 100, 100);
        let open = WindowTransition::open(rect, 1000);
        // At start the open fade is fully transparent; at the end opaque.
        assert_eq!(open.opacity_at(1000), Some(0.0));
        assert_eq!(open.opacity_at(1000 + OPEN_DURATION_MS), None);
        let quarter = open.progress_at(1000 + OPEN_DURATION_MS / 4);
        let opacity = open.opacity_at(1000 + OPEN_DURATION_MS / 4);
        assert!(opacity.is_some_and(|o| o > 0.0 && o < 1.0));
        assert_eq!(opacity, quarter);
        let close = WindowTransition::close(rect, 1000);
        assert_eq!(close.opacity_at(1000), Some(1.0));
        assert_eq!(close.opacity_at(1000 + CLOSE_DURATION_MS), None);
        // Close fades out: opacity strictly decreases while in flight.
        let early = close.opacity_at(1000 + CLOSE_DURATION_MS / 4);
        let late = close.opacity_at(1000 + CLOSE_DURATION_MS / 2);
        assert!(early.is_some_and(|o| o > 0.0));
        assert!(late.is_some_and(|o| early.unwrap() > o));

        // Geometry flights and no-effect transitions stay opaque.
        let plain = WindowTransition::new(rect, 1000);
        assert_eq!(plain.opacity_at(1050), None);
        let icon = Rect::new(500, 1020, 48, 48);
        let genie = WindowTransition::minimize(rect, 1000, MinimizeAnimationStyle::Genie, icon);
        assert_eq!(genie.opacity_at(1050), None);
    }

    /// ADR-0220: the close ghost must dissolve, not switch off. Sampling the
    /// fade per rendered frame at 60 Hz, no single frame may take a large bite
    /// out of the remaining opacity, and the last in-flight frame must already
    /// be (nearly) invisible — that is what a `smoothstep` alpha over the whole
    /// timeline buys, and what the old `EaseInCubic`-driven `1 - t` did not.
    #[test]
    fn a_close_fade_never_drops_a_visible_step_on_its_last_frames() {
        let rect = Rect::new(0, 0, 800, 600);
        let close = WindowTransition::close(rect, 0);
        // 60 Hz over the flight, ending just before the settle instant.
        let frame_ms = 1000 / 60;
        let mut samples = Vec::new();
        let mut elapsed = 0;
        while elapsed < CLOSE_DURATION_MS {
            let alpha = close
                .opacity_at(elapsed)
                .expect("in flight until the deadline");
            samples.push((elapsed, alpha));
            elapsed += frame_ms;
        }
        let start = samples.first().copied().expect("at least one frame");
        assert_eq!((start.0, start.1), (0, 1.0), "the ghost starts opaque");

        // No frame-to-frame step may exceed a sixth of the range. The old
        // curve dropped ~0.37 in the final 16 ms; a smoothstep fade at 60 Hz
        // never exceeds ~0.11 anywhere.
        let mut worst = 0.0_f32;
        for window in samples.windows(2) {
            let step = window[0].1 - window[1].1;
            assert!(step >= 0.0, "the fade is monotonic: {samples:?}");
            worst = worst.max(step);
        }
        assert!(
            worst < 0.16,
            "a close fade step of {worst} reads as a cut, not a fade: {samples:?}"
        );

        // The last frame that still renders is already close to invisible, so
        // retiring the ghost at the deadline cannot be seen.
        let (_, last) = samples.last().copied().expect("frames rendered");
        assert!(
            last < 0.06,
            "the final in-flight frame must already be (almost) gone, got {last}: {samples:?}"
        );
    }

    /// ADR-0220: the shrink and the fade are separate clocks. The rectangle
    /// reaches its inset target at `CLOSE_GEOMETRY_FRACTION` of the timeline
    /// while the alpha keeps falling for the rest of it; the two used to be one
    /// `EaseInCubic` value, which is what made the fade front-load nothing and
    /// back-load everything.
    #[test]
    fn a_close_shrink_finishes_before_its_fade_does() {
        let rect = Rect::new(0, 0, 800, 600);
        let close = WindowTransition::close(rect, 0);
        let target = Rect::new(50, 50, 700, 500);
        let settled_by = (CLOSE_DURATION_MS as f32 * CLOSE_GEOMETRY_FRACTION) as u64;

        let before = close.rect_at(target, settled_by - 1).expect("in flight");
        assert!(before.size.w > target.size.w, "still shrinking");
        // At and after the geometry deadline the rect has arrived, even though
        // the transition as a whole is still running.
        assert_eq!(close.rect_at(target, settled_by), Some(target));
        assert_eq!(close.rect_at(target, settled_by + 10), Some(target));

        // The alpha, by contrast, is still visibly falling over that tail.
        let at_geometry_end = close.opacity_at(settled_by).expect("in flight");
        let at_deadline = close.opacity_at(CLOSE_DURATION_MS - 1).expect("in flight");
        assert!(
            at_geometry_end > at_deadline,
            "the fade must keep moving after the shrink lands: {at_geometry_end} vs {at_deadline}"
        );

        // Every other effect keeps its whole timeline for the rectangle: an
        // open flight at the same fraction of its duration has not landed yet,
        // whereas the close above had.
        let open_from = Rect::new(50, 50, 700, 500);
        let open = WindowTransition::open(open_from, 0);
        let at_same_fraction = (OPEN_DURATION_MS as f32 * CLOSE_GEOMETRY_FRACTION) as u64;
        assert_ne!(
            open.rect_at(rect, at_same_fraction),
            Some(rect),
            "only the close compresses its shrink; the open still owns its whole timeline"
        );
    }

    #[test]
    fn smoothstep_has_zero_velocity_at_both_ends() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        // Monotonic, and symmetric about the midpoint.
        assert!(smoothstep(0.25) < smoothstep(0.5));
        assert!(smoothstep(0.5) < smoothstep(0.75));
        assert_eq!(smoothstep(0.5), 0.5);
        assert!((smoothstep(0.25) + smoothstep(0.75) - 1.0).abs() < 1e-6);
        // Flat tangents: the first and last slices of the curve move far less
        // than the middle one, which is the whole reason a cross-fade uses it.
        let head = smoothstep(0.05) - smoothstep(0.0);
        let middle = smoothstep(0.55) - smoothstep(0.5);
        assert!(head < middle * 0.5, "{head} vs {middle}");
        // Clamped outside the unit interval, never extrapolated.
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(2.0), 1.0);
    }
}
