//! Motion dynamics for the macOS-style dock (ADR-0139, ADR-0172, ADR-0174, ADR-0176).
//!
//! Owns the analytic springs, easing curves, and settle detection for the Dock:
//! - Autohide reveal & collapse travels on a critically damped [`Spring`] (ζ = 1.0, ω₀² = 320.0).
//! - Tile magnification wave travels on an under-damped [`Spring`] (ζ = 0.85, ω₀² = 900.0).
//! - Tooltip alpha decays/approaches via [`approach`].
//! - Swapchain ring drain counter guarantees clean purges across all `FLUX_MAX_FRAMES_IN_FLIGHT` slots.

use std::collections::HashMap;

use lens::Rect;
use tessera_desktop::dock::DockPosition;

use crate::widgets::motion::{Spring, SpringParams, approach, smoothstep};

/// Visual height of the dock bar. Tiles rest inside it; magnified tiles pop
/// above its top edge. On a side edge this is the panel's thickness (width).
pub const DOCK_PANEL_HEIGHT: f32 = 74.0;
/// Gap between the dock surface and the screen edge it is anchored to.
///
/// One value for both states. The expanded bar and the collapsed capsule are the
/// same surface at two sizes, so they hold the same offset: the dock grows and
/// shrinks in place instead of sliding toward the edge as it parks. A floating
/// surface reads as detached chrome, and a capsule that stays put reads as the
/// same object rather than as a second affordance.
///
/// The pointer target is deliberately *not* this value — see
/// [`AUTOHIDE_HANDLE_APPROACH_BAND`], which reaches the physical edge so a
/// pointer thrown at the screen still summons a surface that floats clear of it.
pub const DOCK_EDGE_MARGIN: f32 = 12.0;

/// Gap between the collapsed capsule and the screen edge.
///
/// Defined as [`DOCK_EDGE_MARGIN`] so the collapsed and expanded geometry cannot
/// drift apart through two independent constants.
pub const AUTOHIDE_HANDLE_EDGE_MARGIN: f32 = DOCK_EDGE_MARGIN;
/// Extra depth a pointer may travel *above* the collapsed capsule and still
/// summon the dock, measured inward from the capsule's own top edge.
///
/// This is deliberately independent of [`DOCK_EDGE_MARGIN`] and
/// [`AUTOHIDE_HANDLE_HEIGHT`]: where the pixels sit must not quietly decide how
/// large a target the user has to hit.
///
/// 16 px is chosen on its own merits, in both directions. Deep enough that the
/// capsule can be summoned by reaching the bottom band rather than by aiming at
/// a 4 px pill; shallow enough that it does not swallow clicks on client content
/// sitting just above the parked dock — the trapping failure ADR-0146 exists to
/// prevent. Roughly one tile gap, i.e. a band a pointer crosses deliberately
/// rather than by accident.
pub const AUTOHIDE_HANDLE_APPROACH_BAND: f32 = 16.0;

/// Depth of the collapsed capsule's reveal corridor from the physical screen
/// edge.
///
/// The corridor is the union of three things that must all be true at once: it
/// reaches the screen edge (a pointer thrown at the bottom band summons the dock
/// without being aimed), it covers the capsule's own body, and it extends
/// [`AUTOHIDE_HANDLE_APPROACH_BAND`] past that body. Composing it from those
/// three instead of picking one number is what keeps it correct when the capsule
/// floats clear of the edge: a flat 16 px band would have been *shallower* than
/// the capsule it is supposed to own.
#[inline]
#[must_use]
pub const fn collapsed_reveal_depth() -> f32 {
    DOCK_EDGE_MARGIN + AUTOHIDE_HANDLE_HEIGHT + AUTOHIDE_HANDLE_APPROACH_BAND
}
/// Distance from the bar's bottom edge up to the icon baseline (the bottom of every tile).
pub const DOCK_BASELINE_INSET: f32 = 13.0;
/// Side length of a square dock tile at rest (the icon area).
pub const DOCK_TILE: f32 = 56.0;
/// Side length of a square dock tile at full magnification (1.5× rest).
pub const DOCK_TILE_MAX: f32 = 84.0;
/// Envelope headroom for the underdamped magnification spring: at ζ≈0.85 a
/// tile can overshoot its target by a few percent, and the capture footprint
/// must contain even that peak.
pub const SPRING_OVERSHOOT_MARGIN: f32 = 1.05;
/// How far (in rest-tile widths) the magnification reaches from the cursor.
pub const MAGNIFY_RADIUS_TILES: f32 = 2.0;
/// Spring stiffness (ω₀²) for tile size → target. ~900 gives a period near 0.2s.
pub const SPRING_STIFFNESS: f32 = 900.0;
/// Spring damping ratio for tiles. ~0.85 gives the slight macOS-style bounce-back.
pub const SPRING_DAMPING: f32 = 0.85;
/// Shared motion parameters for tile magnification springs.
pub const DOCK_SPRING: SpringParams = SpringParams::new(SPRING_STIFFNESS, SPRING_DAMPING);

/// Side length a brand-new tile grows in from.
pub const DOCK_TILE_BIRTH: f32 = 6.0;
/// Gap between adjacent rest slots inside the bar.
pub const DOCK_TILE_GAP: f32 = 10.0;
/// Edge-to-edge gap between the pinned strip and the transient section.
pub const DOCK_SECTION_GAP: f32 = 2.0 * DOCK_TILE_GAP;
/// Padding between the bar's edge and the first/last rest slot.
pub const DOCK_PAD: f32 = 10.0;
/// Diameter of a single running-indicator dot.
pub const DOCK_DOT: f32 = 5.0;
/// Width of a running-indicator stadium (pill) for multiple instances.
pub const DOCK_DOT_STADIUM: f32 = 12.0;

/// Reveal spring tuning (ADR-0176 parity). Critically damped (ζ = 1.0) so the
/// panel approaches its collapsed/expanded endpoint without ringing or overshoot.
/// ω₀² = 320.0 settles in ~0.25 s.
pub const DOCK_REVEAL_SPRING: SpringParams = SpringParams::new(320.0, 1.0);
/// Value/velocity tolerances at which the reveal spring is considered at rest.
pub const REVEAL_SETTLE_VALUE_EPS: f32 = 0.002;
pub const REVEAL_SETTLE_VELOCITY_EPS: f32 = 0.02;

/// Frames the dock keeps reporting its maximum footprint damage after reveal settles
/// (ADR-0171 `[INV-ARCH-03]`, ADR-0176). Drains all slots of the 3-frame swapchain ring.
pub const SETTLED_DRAIN_FRAMES: u8 = 3;

/// Inactivity timeout in seconds before an autohiding dock collapses after user interaction.
pub const AUTOHIDE_IDLE_TIMEOUT: f32 = 0.50;
/// Continuous pointer dwell duration in seconds required on the collapsed indicator before expanding.
pub const AUTOHIDE_DWELL_THRESHOLD: f32 = 0.18;
/// Snappy inactivity timeout in seconds when cursor retreats without interacting with the dock.
pub const AUTOHIDE_QUICK_DISMISS_TIMEOUT: f32 = 0.15;
/// Width of the thin stadium handle shown when the Dock is autohidden.
pub const AUTOHIDE_HANDLE_WIDTH: f32 = 140.0;
/// Height of the thin stadium handle shown when the Dock is autohidden.
pub const AUTOHIDE_HANDLE_HEIGHT: f32 = 4.0;
/// Reveal progress below which iconography has completely drained into the collapsing surface.
pub const AUTOHIDE_CONTENT_DRAIN_END: f32 = 0.28;
/// Content at or below this scale is visually drained and no longer hit-testable.
pub const AUTOHIDE_CONTENT_INTERACTION_MIN: f32 = 0.01;

/// Pointer dwell before an application name appears.
pub const TOOLTIP_DWELL: f32 = 0.30;
/// Exponential fade speed for the dock application-name tooltip.
pub const TOOLTIP_FADE_SPEED: f32 = 18.0;
pub const TOOLTIP_HEIGHT: f32 = 28.0;
pub const TOOLTIP_GAP: f32 = 9.0;
pub const TOOLTIP_BAND: f32 = TOOLTIP_HEIGHT + TOOLTIP_GAP;

/// Pointer travel (logical px) that promotes a held press into a drag.
pub const DRAG_THRESHOLD: f32 = 6.0;

/// How much closer a neighbouring tile must be before it takes the hover.
///
/// A pointer resting on a magnified bar sits, by construction, within float
/// noise of the midpoint between two equally-sized tiles. Without this margin
/// the winner alternates with the noise, and every flip restarts the name
/// bubble's dwell — so the dwell never completes and the bubble never appears
/// (ADR-0177, ADR-0217). The margin is one logical pixel: far larger than the
/// noise it must absorb, and far smaller than the travel of a deliberate move,
/// so pointing at a neighbour still switches the hover on the first frame.
pub const HIT_HYSTERESIS_PX: f32 = 1.0;
/// Distance from a screen edge within which an edge drag snaps the dock there.
pub const EDGE_DRAG_PROXIMITY: f32 = 96.0;
pub const DRAG_LIFT_SCALE: f32 = 1.12;

/// Geometry for preview cards shown above a running Dock tile.
pub const PREVIEW_CARD_MAX_WIDTH: f32 = 224.0;
pub const PREVIEW_CARD_MIN_WIDTH: f32 = 112.0;
pub const PREVIEW_ASPECT: f32 = 0.62;
pub const PREVIEW_LABEL_HEIGHT: f32 = 34.0;
pub const PREVIEW_PANEL_PAD: f32 = 12.0;
pub const PREVIEW_CARD_GAP: f32 = 10.0;
pub const PREVIEW_PANEL_GAP: f32 = 12.0;
pub const PREVIEW_SCREEN_MARGIN: f32 = 8.0;

/// Motion dynamics state for the Dock.
#[derive(Debug, Clone, PartialEq)]
pub struct DockMotion {
    /// Autohide reveal spring: 0.0 = collapsed capsule, 1.0 = expanded bar.
    pub reveal: Spring,
    /// Number of settled frames remaining in the swapchain ring drain.
    pub settled_drain_frames: u8,
    /// Per-tile size springs keyed by tile key.
    pub sizes: HashMap<String, Spring>,
    /// Application name tooltip alpha in `[0.0, 1.0]`.
    pub tooltip_alpha: f32,
    /// Whether any animation (reveal, magnification wave, or tooltip fade) is active.
    pub anim_active: bool,
}

impl DockMotion {
    /// Construct motion state with initial reveal state.
    #[must_use]
    pub fn new(autohide: bool) -> Self {
        let initial_reveal = if autohide { 0.0 } else { 1.0 };
        Self {
            reveal: Spring::new(initial_reveal),
            settled_drain_frames: 0,
            sizes: HashMap::new(),
            tooltip_alpha: 0.0,
            anim_active: false,
        }
    }

    /// Current reveal value clamped to `[0.0, 1.0]`.
    #[inline]
    #[must_use]
    pub fn reveal_value(&self) -> f32 {
        self.reveal.value.clamp(0.0, 1.0)
    }

    /// Whether the reveal spring is currently traveling toward `target`.
    #[inline]
    #[must_use]
    pub fn reveal_animating(&self, target: f32) -> bool {
        !self
            .reveal
            .settled(target, REVEAL_SETTLE_VALUE_EPS, REVEAL_SETTLE_VELOCITY_EPS)
    }

    /// Advance the reveal spring toward `target` and manage the swapchain drain ring.
    ///
    /// The drain counter (`settled_drain_frames`) is primed to `SETTLED_DRAIN_FRAMES` (3)
    /// on the exact frame the spring settles, and decrements once per subsequent frame.
    /// It is non-preemptible by secondary animations.
    pub fn advance_reveal(&mut self, target: f32, dt: f32, reduced_motion: bool) {
        let was_animating = self.reveal_animating(target);
        self.reveal
            .advance_substep(target, DOCK_REVEAL_SPRING, dt, 1.0 / 60.0, reduced_motion);
        if (self.reveal.value - target).abs() <= REVEAL_SETTLE_VALUE_EPS {
            self.reveal.snap_to(target);
        }
        let is_settled = !self.reveal_animating(target);

        if !is_settled {
            self.settled_drain_frames = 0;
        } else if was_animating {
            // First frame of rest: snap to target and prime the swapchain ring drain.
            self.reveal.snap_to(target);
            self.settled_drain_frames = SETTLED_DRAIN_FRAMES;
        } else if self.settled_drain_frames > 0 {
            // Successive drain frames across the swapchain ring.
            self.settled_drain_frames -= 1;
        }
    }

    /// Advance one tile spring toward its target size.
    pub fn advance_tile_spring(
        spring: &mut Spring,
        target: f32,
        dt: f32,
        reduced_motion: bool,
    ) -> f32 {
        spring.advance_substep(target, DOCK_SPRING, dt, 1.0 / 60.0, reduced_motion)
    }

    /// Advance the tooltip fade towards `target` (`0.0` or `1.0`).
    pub fn advance_tooltip(&mut self, target: f32, dt: f32, reduced_motion: bool) {
        self.tooltip_alpha = approach(
            self.tooltip_alpha,
            target,
            TOOLTIP_FADE_SPEED,
            dt,
            reduced_motion,
        );
        if target == 0.0 && self.tooltip_alpha < 0.01 {
            self.tooltip_alpha = 0.0;
        }
    }

    /// Whether any chrome animation is pending and requires another frame.
    ///
    /// `settled_drain_frames` is deliberately excluded: it is a *swapchain
    /// ring* bookkeeping counter ("repaint this slot once more so every
    /// in-flight image shows the settled state"), not an animation. Letting
    /// it gate frame production creates a self-sustaining loop — a drain
    /// count keeps a frame requested, the frame re-primes the drain, and the
    /// compositor repaints a fully static dock at the panel refresh rate
    /// forever. The drain still drives damage; it must not drive the clock.
    #[must_use]
    pub fn anim_pending(&self, target_reveal: f32, dwell_active: bool, drag_active: bool) -> bool {
        self.reveal_animating(target_reveal) || self.anim_active || dwell_active || drag_active
    }
}

/// Hermite smoothstep curve for morph transitions.
#[inline]
#[must_use]
pub fn smoothstep_progress(progress: f32) -> f32 {
    smoothstep(progress)
}

/// Geometric expansion of the single Dock surface: 0.0 = stadium handle, 1.0 = full glass panel.
#[inline]
#[must_use]
pub fn collapse_surface_progress(reveal: f32) -> f32 {
    smoothstep(reveal)
}

/// Distance the single Dock surface keeps from its anchored screen edge.
///
/// Constant across the morph, because both states hold [`DOCK_EDGE_MARGIN`]. The
/// `surface_progress` parameter is retained so callers keep one call site for
/// the question "how far from the edge is the body now", and so the answer
/// cannot silently diverge between the two states again.
#[inline]
#[must_use]
pub fn edge_margin_for_surface_progress(_surface_progress: f32) -> f32 {
    DOCK_EDGE_MARGIN
}

/// Content drains earlier than containing surface: icons fade and shrink into the sink by 0.28.
#[inline]
#[must_use]
pub fn collapse_content_progress(reveal: f32) -> f32 {
    let normalized = (reveal - AUTOHIDE_CONTENT_DRAIN_END) / (1.0 - AUTOHIDE_CONTENT_DRAIN_END);
    smoothstep(normalized)
}

/// Cosine-bell magnification factor in `[0, 1]` for a tile whose rest centre is `dx` px from cursor.
#[must_use]
pub fn magnify_factor(dx: f32) -> f32 {
    let radius = MAGNIFY_RADIUS_TILES * DOCK_TILE;
    let d = dx.abs();
    if d >= radius {
        return 0.0;
    }
    0.5 * (1.0 + (std::f32::consts::PI * d / radius).cos())
}

/// Check if cursor is retreating away from the anchored screen edge toward the client work area.
#[must_use]
pub fn detect_cursor_retreat(
    position: DockPosition,
    cursor: (f32, f32),
    last_cursor: Option<(f32, f32)>,
    rest_bounds: Rect,
) -> bool {
    let Some((last_x, last_y)) = last_cursor else {
        return false;
    };
    match position {
        DockPosition::Bottom => cursor.1 < rest_bounds.y && cursor.1 < last_y,
        DockPosition::Left => cursor.0 > rest_bounds.x + rest_bounds.w && cursor.0 > last_x,
        DockPosition::Right => cursor.0 < rest_bounds.x && cursor.0 < last_x,
    }
}
