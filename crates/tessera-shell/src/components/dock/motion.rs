//! Motion dynamics for the pure application Dock (ADR-0139, ADR-0174, ADR-0254, ADR-0256).
//!
//! Owns the analytic springs, easing curves, and settle detection for the Dock:
//! - Reveal & hide travels on a critically damped [`Spring`] (ζ = 1.0, ω₀² = 320.0)
//!   driving a clean edge slide and opacity fade (no collapsed capsule morph).
//! - Tile magnification wave travels on an under-damped [`Spring`] (ζ = 0.85, ω₀² = 900.0).
//! - Tooltip/preview alpha decays/approaches via [`approach`].
//! - Swapchain ring drain counter guarantees clean purges across all frames in flight.

use std::collections::HashMap;

use optics_ui::Rect;
use tessera_desktop::dock::DockPosition;

use crate::widgets::motion::{Spring, SpringParams, approach, smoothstep};

/// Visual thickness of the dock bar (height on Top/Bottom, width on Left/Right).
pub const DOCK_PANEL_HEIGHT: f32 = 74.0;
/// Gap between the expanded dock surface and the screen edge it is anchored to (ADR-0251).
pub const DOCK_EDGE_MARGIN: f32 = 4.0;
/// Slide travel distance (logical px) during hide/reveal transitions (ADR-0254).
pub const DOCK_REVEAL_SLIDE_OFFSET: f32 = 28.0;
/// Distance from the bar's anchored edge inward to the icon baseline.
pub const DOCK_BASELINE_INSET: f32 = 13.0;
/// Side length of a square dock tile at rest.
pub const DOCK_TILE: f32 = 56.0;
/// Side length of a square dock tile at full magnification (1.5× rest).
pub const DOCK_TILE_MAX: f32 = 84.0;
/// Envelope headroom for the underdamped magnification spring.
pub const SPRING_OVERSHOOT_MARGIN: f32 = 1.05;
/// How far (in rest-tile widths) the magnification reaches from the cursor.
pub const MAGNIFY_RADIUS_TILES: f32 = 2.0;
/// Spring stiffness (ω₀²) for tile size → target.
pub const SPRING_STIFFNESS: f32 = 900.0;
/// Spring damping ratio for tiles.
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

/// Reveal spring tuning: critically damped (ζ = 1.0) so the panel slides in/out
/// cleanly without overshoot.
pub const DOCK_REVEAL_SPRING: SpringParams = SpringParams::new(320.0, 1.0);
/// Value/velocity tolerances at which the reveal spring is considered at rest.
pub const REVEAL_SETTLE_VALUE_EPS: f32 = 0.002;
pub const REVEAL_SETTLE_VELOCITY_EPS: f32 = 0.02;

/// Frames the dock keeps reporting its maximum footprint damage after reveal settles.
pub const SETTLED_DRAIN_FRAMES: u8 = 3;

/// Inactivity timeout in seconds before an autohiding dock hides after interaction.
pub const AUTOHIDE_IDLE_TIMEOUT: f32 = 0.50;
/// Snappy inactivity timeout in seconds when cursor leaves without interacting.
pub const AUTOHIDE_QUICK_DISMISS_TIMEOUT: f32 = 0.15;
/// Exit grace window (seconds) before dismissing an active Live Preview when
/// the pointer briefly skims outside its boundary (ADR-0256).
pub const PREVIEW_EXIT_GRACE_SECS: f32 = 0.22;
/// Forgiveness margin (logical px) around an active Live Preview panel for
/// keep-alive hit-testing (ADR-0256).
pub const PREVIEW_FORGIVENESS_MARGIN: f32 = 10.0;

/// Pointer dwell before an application name or live preview appears.
pub const TOOLTIP_DWELL: f32 = 0.30;
/// Exponential fade speed for the dock tooltip / preview popover.
pub const TOOLTIP_FADE_SPEED: f32 = 18.0;
pub const TOOLTIP_HEIGHT: f32 = 28.0;
pub const TOOLTIP_GAP: f32 = 9.0;
pub const TOOLTIP_BAND: f32 = TOOLTIP_HEIGHT + TOOLTIP_GAP;

/// Pointer travel (logical px) that promotes a held press into a drag.
pub const DRAG_THRESHOLD: f32 = 6.0;
/// Hysteresis margin preventing float-noise oscillation between adjacent tiles.
pub const HIT_HYSTERESIS_PX: f32 = 1.0;
/// Distance from a screen edge within which an edge drag snaps the dock there.
pub const EDGE_DRAG_PROXIMITY: f32 = 96.0;
pub const DRAG_LIFT_SCALE: f32 = 1.12;

/// Geometry for preview cards shown beside a running Dock tile.
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
    /// Reveal spring: 0.0 = hidden (0 pixels), 1.0 = fully revealed.
    pub reveal: Spring,
    /// Number of settled frames remaining in the swapchain ring drain.
    pub settled_drain_frames: u8,
    /// Per-tile size springs keyed by tile key.
    pub sizes: HashMap<String, Spring>,
    /// Application name tooltip / preview alpha in `[0.0, 1.0]`.
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
            self.reveal.snap_to(target);
            self.settled_drain_frames = SETTLED_DRAIN_FRAMES;
        } else if self.settled_drain_frames > 0 {
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

    /// Advance the tooltip/preview fade towards `target` (`0.0` or `1.0`).
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
    #[must_use]
    pub fn anim_pending(&self, target_reveal: f32, drag_active: bool) -> bool {
        self.reveal_animating(target_reveal) || self.anim_active || drag_active
    }
}

/// Hermite smoothstep curve for reveal transitions.
#[inline]
#[must_use]
pub fn smoothstep_progress(progress: f32) -> f32 {
    smoothstep(progress)
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
        DockPosition::Top => cursor.1 > rest_bounds.y + rest_bounds.h && cursor.1 > last_y,
        DockPosition::Bottom => cursor.1 < rest_bounds.y && cursor.1 < last_y,
        DockPosition::Left => cursor.0 > rest_bounds.x + rest_bounds.w && cursor.0 > last_x,
        DockPosition::Right => cursor.0 < rest_bounds.x && cursor.0 < last_x,
    }
}
