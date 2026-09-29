//! Tiered segmented level control: an exclusive, discrete-value selector with
//! a spring-driven sliding indicator.
//!
//! Some system controls are *tiered*, not continuous. A laptop keyboard
//! backlight is the canonical case: the hardware exposes a small set of
//! illumination steps (ADR-0168), and a free-dragging fader both over-promises
//! precision the hardware does not have and misrepresents a discrete state as
//! an analogue one.
//!
//! This widget renders one segment per tier, marks the active tier, and slides a
//! single highlight between segments. A press can be released in place (a plain
//! click) or scrubbed across segments before release (a drag), the highlight
//! tracking the pointer in both cases through one under-damped spring, so the
//! travel reads as physical rather than as a hard jump. The *feel* is physical;
//! the *look* stays flat — this panel's design language is explicit that it does
//! not use skeuomorphic materials or liquid glass.
//!
//! It is deliberately built **here**, in `tessera-shell::widgets`, and not in
//! Optics:
//!
//! - Optics already ships the *static* form of this control
//!   (`lens_segmented_control`, `libs/lens/include/lens/patterns.h`), so the
//!   shape is not novel surface for upstream.
//! - The *animated* part is exactly what Optics' own ADRs exclude: ADR-0082
//!   sends compound widgets to userland, and ADR-0061 classifies spring
//!   physics and slides as "flavor" that "must not ship in the library". A
//!   spring-driven indicator inside lens would also force lens to depend on
//!   `transit`, the arrow ADR-0077 forbids.
//! - The motion math comes from the single authorized seam,
//!   [`crate::widgets::motion`] (`[INV-ARCH-49]`): this module never integrates
//!   a spring by hand.

use lens::{Align, Color, Frame, LayoutOpts};
use tessera_design::{ControlCenterColors, themes};

use crate::widgets::motion::{Spring, SpringParams};

/// Indicator spring tuning: under-damped (ζ = 0.72), so the highlight overshoots
/// its target by ≈4% and settles in ≈0.29 s — one quick lift-and-settle that
/// reads as physical without ringing.
///
/// This is shared by every segmented control in the shell (the power-mode
/// selector and the keyboard-backlight selector), so they feel identical. It
/// replaces a critically-damped (`ζ = 1.0`) tuning that, by construction, could
/// not overshoot at all — the "bounce" the surrounding comments promised had
/// never been real.
pub const INDICATOR_SPRING: SpringParams = SpringParams::new(380.0, 0.72);

/// What a [`tiered_control`] call observed this frame.
///
/// The caller owns the policy (what a tier *means*, whether to dispatch, how to
/// preview a drag); the widget only reports what the pointer did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TieredOutcome {
    /// The segment released-on this frame, when the release landed on the same
    /// segment (an ordinary click, or an assistive-technology activation).
    pub clicked: Option<usize>,
    /// The segment currently under the pointer, whether or not a press is
    /// active. Drives the drag preview.
    pub hovered: Option<usize>,
    /// A press that began on a segment is still held, so a pointer move across
    /// segments should retarget the highlight.
    pub dragging: bool,
}

/// One tier of a [`tiered_control`] selector.
pub struct Tier {
    /// Short label drawn inside the segment (e.g. "Off", "33%").
    pub label: String,
    /// The action value this tier commits when selected. Opaque to the widget.
    pub value: u8,
    /// Whether the tier is currently available (unavailable tiers draw muted
    /// and reject clicks).
    pub enabled: bool,
}

impl Tier {
    /// A selectable tier.
    pub fn new(label: impl Into<String>, value: u8) -> Self {
        Self {
            label: label.into(),
            value,
            enabled: true,
        }
    }

    /// A tier that draws muted and rejects clicks (e.g. a level the current
    /// hardware does not expose).
    pub fn disabled(label: impl Into<String>, value: u8) -> Self {
        Self {
            label: label.into(),
            value,
            enabled: false,
        }
    }
}

/// The persistent, per-control spring state the caller owns across frames.
///
/// This is mechanism-free storage: the widget advances it through
/// [`crate::widgets::motion`], but the caller decides *when* it exists and
/// *what* its target is (policy), matching the `widgets/motion` contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct TieredIndicator {
    /// Spring position in segment units (0.0 == first segment).
    pub spring: Spring,
    /// Whether the spring was primed for a first frame.
    primed: bool,
}

impl TieredIndicator {
    /// A fresh indicator resting on `index`.
    pub fn at(index: usize) -> Self {
        Self {
            spring: Spring::new(index as f32),
            primed: true,
        }
    }

    /// Advance the indicator toward `index` for this frame.
    ///
    /// `reduced_motion` resolves the travel in one frame, per ADR-0029 /
    /// `[INV-ARCH-45]`.
    pub fn advance(&mut self, index: usize, dt_seconds: f32, reduced_motion: bool) {
        let target = index as f32;
        if !self.primed {
            // A control that appears mid-session (hardware detected, tab
            // opened) must not fly in from segment zero.
            self.spring.snap_to(target);
            self.primed = true;
            return;
        }
        self.spring
            .advance(target, INDICATOR_SPRING, dt_seconds, reduced_motion);
    }

    /// Whether the indicator is still travelling; keeps the frame loop alive.
    pub fn anim_pending(&self, index: usize) -> bool {
        !self.spring.settled(index as f32, 0.01, 0.5)
    }
}

/// Draw a tiered segmented control and report what the pointer did.
///
/// The caller owns all policy: what a tier *means*, whether to dispatch, and how
/// to preview a drag. The widget reports the raw interaction in a
/// [`TieredOutcome`], which supports both a plain click and a press-and-drag:
///
/// - **Click** a segment: `clicked` names it.
/// - **Press and drag** across segments: `dragging` stays true and `hovered`
///   follows the pointer, so the caller can retarget the highlight as a live
///   preview; on release `hovered` names the segment under the pointer (which
///   may be `None` if the pointer left the control — a cancel, exactly like a
///   slider released off its track).
///
/// `indicator` is caller-owned spring state (see [`TieredIndicator`]); pass the
/// same instance every frame. `tiers` must be non-empty.
///
/// The control is built entirely from flow containers (a `stack` of the well,
/// the sliding highlight, and the hit rows), never from absolute placement:
/// `f.place` resolves to display coordinates and would escape the enclosing
/// card's flow (`[INV-ARCH-01]`, ADR-0171).
pub fn tiered_control(
    f: &mut Frame,
    id: &str,
    tiers: &[Tier],
    active: usize,
    indicator: &TieredIndicator,
    hud: ControlCenterColors,
    label_size: f32,
    size: (f32, f32),
) -> TieredOutcome {
    let mut outcome = TieredOutcome::default();
    if tiers.is_empty() || size.0 <= 1.0 || size.1 <= 1.0 {
        return outcome;
    }
    const INSET: f32 = 4.0;
    let inner_w = (size.0 - INSET * 2.0).max(1.0);
    let inner_h = (size.1 - INSET * 2.0).max(1.0);
    let seg_w = inner_w / tiers.len() as f32;
    let active = active.min(tiers.len() - 1);

    // The spring position is clamped to the segment range so an overshoot never
    // paints outside the well.
    let spring_pos = indicator
        .spring
        .value
        .clamp(0.0, (tiers.len() - 1) as f32);

    let mut selected = None;
    f.stack().width(size.0).height(size.1).show(|f| {
        // 1. Recessed well: the control's own track, one step below the card
        //    surface.
        f.row_ex(
            &LayoutOpts {
                width: size.0,
                height: size.1,
                radius: size.1 * 0.5,
                bg: hud.surface_recessed,
                border: hud.border,
                border_width: 1.0,
                ..Default::default()
            },
            |_| {},
        );

        // 2. Sliding highlight, pushed to its spring position by a leading
        //    spacer inside a full-width row.
        f.row_ex(
            &LayoutOpts {
                width: size.0,
                height: size.1,
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                f.spacer(INSET + spring_pos * seg_w);
                f.row_ex(
                    &LayoutOpts {
                        width: seg_w,
                        height: inner_h,
                        radius: inner_h * 0.5,
                        bg: hud.selection_surface,
                        border: hud.accent.with_alpha(80),
                        border_width: 1.0,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        // 3. Hit areas, laid out across the inner width; they sit above the
        //    highlight in the stack so every segment stays reachable, including
        //    the one the highlight covers.
        f.row_ex(
            &LayoutOpts {
                width: size.0,
                height: size.1,
                pad: INSET,
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                for (index, tier) in tiers.iter().enumerate() {
                    let is_active = index == active;
                    let fg = if !tier.enabled {
                        hud.text_muted.with_alpha(140)
                    } else if is_active {
                        hud.accent
                    } else {
                        hud.text_muted
                    };
                    f.set_theme(themes::hud(&hud).with_fg(fg));
                    let (response, _) = f.pressable_row(
                        &format!("{id}-seg-{index}"),
                        &tier.label,
                        &LayoutOpts {
                            width: seg_w,
                            height: inner_h,
                            cross: Align::Center,
                            radius: inner_h * 0.5,
                            bg: Color::TRANSPARENT,
                            ..Default::default()
                        },
                        |f, _| {
                            f.centered(seg_w, inner_h, |f| {
                                f.label_compact_weighted(&tier.label, label_size, DISPLAY_WEIGHT);
                            });
                        },
                    );
                    // `pressed` is reported only by the segment that captured
                    // the press, and stays true for the whole hold — so any
                    // pressed segment means a drag is in flight.
                    if response.pressed {
                        outcome.dragging = true;
                    }
                    if response.hovered && tier.enabled {
                        outcome.hovered = Some(index);
                    }
                    if response.clicked && tier.enabled && !is_active {
                        selected = Some(index);
                    }
                }
            },
        );
    });

    outcome.clicked = selected;
    outcome
}

/// The panel's display label weight (bold sans, the game-HUD voice).
const DISPLAY_WEIGHT: f32 = 700.0;

#[cfg(test)]
mod tests {
    use super::*;
    use lens::{Input, MouseButton, Ui};

    fn tiers() -> Vec<Tier> {
        vec![
            Tier::new("0%", 0),
            Tier::new("50%", 50),
            Tier::new("100%", 100),
        ]
    }

    /// Drive one frame and return the widget's outcome.
    fn frame(
        ui: &mut Ui,
        input: &Input,
        indicator: &TieredIndicator,
        active: usize,
    ) -> TieredOutcome {
        let mut outcome = TieredOutcome::default();
        ui.frame(input, |f| {
            outcome = tiered_control(
                f,
                "t",
                &tiers(),
                active,
                indicator,
                ControlCenterColors::dark(),
                12.0,
                (200.0, 24.0),
            );
        });
        let snap = ui.snapshot().unwrap();
        ui.activate(&snap).unwrap();
        outcome
    }

    #[test]
    fn click_reports_the_released_segment() {
        let mut ui = Ui::headless().unwrap();
        let indicator = TieredIndicator::at(0);
        // First frame resolves the widget's geometry.
        let _ = frame(&mut ui, &Input::new((200.0, 24.0), 0.05), &indicator, 0);
        // Segment centres are at x = 4 + (i + 0.5) * (192/3).
        let seg1_x = 4.0 + 1.5 * 64.0;
        let mut input = Input::new((200.0, 24.0), 0.05);
        input.set_cursor(seg1_x, 12.0);
        let _ = frame(&mut ui, &input, &indicator, 0);
        // Press on segment 1, then release on it.
        input.set_mouse_down(MouseButton::Left, true);
        input.set_mouse_pressed(MouseButton::Left, true);
        let pressed = frame(&mut ui, &input, &indicator, 0);
        assert!(pressed.dragging, "a held press reports a drag in flight");
        input.set_mouse_down(MouseButton::Left, false);
        input.set_mouse_pressed(MouseButton::Left, false);
        input.set_mouse_released(MouseButton::Left, true);
        let released = frame(&mut ui, &input, &indicator, 0);
        assert_eq!(released.clicked, Some(1));
        assert!(!released.dragging, "release ends the drag");
    }

    #[test]
    fn empty_tiers_is_inert() {
        let mut ui = Ui::headless().unwrap();
        let indicator = TieredIndicator::at(0);
        let input = Input::new((200.0, 24.0), 0.05);
        let mut outcome = TieredOutcome::default();
        ui.frame(&input, |f| {
            outcome = tiered_control(
                f,
                "t",
                &[],
                0,
                &indicator,
                ControlCenterColors::dark(),
                12.0,
                (200.0, 24.0),
            );
        });
        assert_eq!(outcome, TieredOutcome::default());
    }
}

