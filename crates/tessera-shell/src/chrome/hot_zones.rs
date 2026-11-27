//! Unified eight-zone screen perimeter Hot Zones (ADR-0254).
//!
//! Renders bezel-flush visual affordances along the four screen edges (right
//! angles against the physical edge, continuous curvature on the desktop-facing
//! corners) and smooth corner-transition arcs at the four screen vertices.
//!
//! Invariants enforced:
//! - `[INV-HOTZONE-01]` Zero idle cost: completely invisible when the pointer
//!   is outside every bound slot's proximity band; submits zero damage and
//!   requests zero animation frames.
//! - `[INV-HOTZONE-02]` Mutual exclusion: edge slots strictly avoid corner
//!   spans, and each slot binds at most one target component.
//! - `[INV-HOTZONE-04]` Fullscreen silence: under `SpaceUse::Fullscreen`, all
//!   eight slots remain hidden and non-interactive.

use optics_ui::{Color, CornerRadii, Frame, Input, LayoutOpts, Rect};
use tessera_design::{Design, materials};
use tessera_desktop::hot_zone::{
    HotZoneBinding, HotZonePolicy, HotZoneSlot, HotZoneTarget, HotZoneTrigger,
};
use tessera_desktop::window::{SpaceUse, Window};
use tessera_desktop::workspace::WorkspaceSnapshot;

use crate::component::{Chrome, ChromeEvents, ChromeUpdate, CursorShape, Localizer};
use crate::widgets::geom::contains;
use crate::widgets::motion::approach;

/// Maximum visual length of a centred edge Hot Zone tab (logical px).
pub const EDGE_TAB_LENGTH: f32 = 128.0;
/// Visual thickness of a revealed edge tab protruding inward from the bezel.
pub const EDGE_TAB_THICKNESS: f32 = 6.0;
/// Interior corner radius for an edge tab (matches full thickness so the
/// exposed inner corners curve continuously into crisp 90° bezel vertices).
pub const EDGE_TAB_INNER_RADIUS: f32 = EDGE_TAB_THICKNESS;
/// Continuous G2 squircle curvature blend for Hot Zone profiles (ADR-0254 / ADR-0267).
const HOT_ZONE_CURVATURE: f32 = 0.6;
/// Physical edge trigger hitbox depth (logical px). Tightly hugs the bezel so
/// Fitts's Law makes blind edge throws effortless without swallowing interior
/// window controls.
pub const EDGE_TRIGGER_DEPTH: f32 = 6.0;

/// Radius of the revealed corner transition arc hugging each screen corner.
pub const CORNER_ARC_RADIUS: f32 = 22.0;
/// Separation kept between corner spans and edge tabs so no two slots overlap.
pub const CORNER_EDGE_CLEARANCE: f32 = 16.0;
/// Trigger hitbox extent at each screen vertex (10×10 px) — small enough to
/// avoid maximized window corner buttons while trivial to hit by throwing the
/// pointer into the corner.
pub const CORNER_TRIGGER_SIZE: f32 = 10.0;

/// Exponential approach speed for proximity reveal/hide.
const REVEAL_RATE: f32 = 16.0;
/// Bounded swapchain retirement drain frames after a zone fully hides (ADR-0227).
const RETIRE_DRAIN_FRAMES: u8 = 4;

#[derive(Debug, Clone, Default)]
struct SlotRuntime {
    reveal: f32,
    in_trigger: bool,
    dwell_secs: f32,
    hover_latched: bool,
    press_armed: bool,
    retire_frames: u8,
    dirty: bool,
}

/// The unified eight-zone Hot Zone chrome component.
pub struct HotZones {
    policy: HotZonePolicy,
    slots: [SlotRuntime; 8],
    space_use: SpaceUse,
    dock_expanded: bool,
    launcher_active: bool,
    reduced_motion: bool,
    prev_down: bool,
    visual_dirty: bool,
    pending_summon: Option<(HotZoneSlot, HotZoneTarget)>,
    design: Design,
}

impl Default for HotZones {
    fn default() -> Self {
        Self::new(HotZonePolicy::default())
    }
}

impl HotZones {
    #[must_use]
    pub fn new(policy: HotZonePolicy) -> Self {
        Self {
            policy,
            slots: std::array::from_fn(|_| SlotRuntime::default()),
            space_use: SpaceUse::Available,
            dock_expanded: false,
            launcher_active: false,
            reduced_motion: false,
            prev_down: false,
            visual_dirty: false,
            pending_summon: None,
            design: Design::dark(),
        }
    }

    /// Replace the active Hot Zone policy (startup or live config reload).
    pub fn set_policy(&mut self, policy: HotZonePolicy) {
        self.policy = policy;
        self.pending_summon = None;
        for slot_kind in HotZoneSlot::ALL {
            let idx = Self::slot_index(slot_kind);
            let still_bound = self.policy.binding_for(slot_kind).is_some();
            let slot = &mut self.slots[idx];
            slot.dwell_secs = 0.0;
            slot.hover_latched = false;
            slot.press_armed = false;
            if !still_bound && slot.reveal > 0.0 {
                slot.reveal = 0.0;
                slot.in_trigger = false;
                slot.retire_frames = RETIRE_DRAIN_FRAMES;
            }
        }
        self.visual_dirty = true;
    }

    #[must_use]
    pub fn policy(&self) -> &HotZonePolicy {
        &self.policy
    }

    #[inline]
    const fn slot_index(slot: HotZoneSlot) -> usize {
        match slot {
            HotZoneSlot::Top => 0,
            HotZoneSlot::Bottom => 1,
            HotZoneSlot::Left => 2,
            HotZoneSlot::Right => 3,
            HotZoneSlot::TopLeft => 4,
            HotZoneSlot::TopRight => 5,
            HotZoneSlot::BottomLeft => 6,
            HotZoneSlot::BottomRight => 7,
        }
    }

    /// Whether `binding` is currently suppressed (fullscreen, modal launcher,
    /// or an already-pinned/expanded Dock on that edge).
    fn is_suppressed(&self, binding: HotZoneBinding) -> bool {
        if self.space_use == SpaceUse::Fullscreen || self.launcher_active {
            return true;
        }
        if binding.target == HotZoneTarget::Dock && self.dock_expanded {
            return true;
        }
        false
    }

    /// Effective centred length of an edge tab on `axis_len`, guaranteed never
    /// to enter the corner spans (`[INV-HOTZONE-02]`).
    #[must_use]
    pub fn edge_span(axis_len: f32) -> f32 {
        let reserved = (CORNER_ARC_RADIUS + CORNER_EDGE_CLEARANCE) * 2.0;
        let max_len = (axis_len - reserved).max(0.0);
        EDGE_TAB_LENGTH.min(max_len)
    }

    /// Clipped visible bounding box of a revealed Hot Zone slot inside the
    /// output, used for damage reporting and geometry verification.
    #[must_use]
    pub fn visual_bounds(slot: HotZoneSlot, display: (f32, f32)) -> Rect {
        match slot {
            HotZoneSlot::Top => {
                let w = Self::edge_span(display.0);
                Rect {
                    x: (display.0 - w) * 0.5,
                    y: 0.0,
                    w,
                    h: EDGE_TAB_THICKNESS,
                }
            }
            HotZoneSlot::Bottom => {
                let w = Self::edge_span(display.0);
                Rect {
                    x: (display.0 - w) * 0.5,
                    y: (display.1 - EDGE_TAB_THICKNESS).max(0.0),
                    w,
                    h: EDGE_TAB_THICKNESS,
                }
            }
            HotZoneSlot::Left => {
                let h = Self::edge_span(display.1);
                Rect {
                    x: 0.0,
                    y: (display.1 - h) * 0.5,
                    w: EDGE_TAB_THICKNESS,
                    h,
                }
            }
            HotZoneSlot::Right => {
                let h = Self::edge_span(display.1);
                Rect {
                    x: (display.0 - EDGE_TAB_THICKNESS).max(0.0),
                    y: (display.1 - h) * 0.5,
                    w: EDGE_TAB_THICKNESS,
                    h,
                }
            }
            HotZoneSlot::TopLeft => Rect {
                x: 0.0,
                y: 0.0,
                w: CORNER_ARC_RADIUS,
                h: CORNER_ARC_RADIUS,
            },
            HotZoneSlot::TopRight => Rect {
                x: (display.0 - CORNER_ARC_RADIUS).max(0.0),
                y: 0.0,
                w: CORNER_ARC_RADIUS,
                h: CORNER_ARC_RADIUS,
            },
            HotZoneSlot::BottomLeft => Rect {
                x: 0.0,
                y: (display.1 - CORNER_ARC_RADIUS).max(0.0),
                w: CORNER_ARC_RADIUS,
                h: CORNER_ARC_RADIUS,
            },
            HotZoneSlot::BottomRight => Rect {
                x: (display.0 - CORNER_ARC_RADIUS).max(0.0),
                y: (display.1 - CORNER_ARC_RADIUS).max(0.0),
                w: CORNER_ARC_RADIUS,
                h: CORNER_ARC_RADIUS,
            },
        }
    }

    /// Bezel-flush trigger hitbox (`[INV-HOTZONE-02]`): tightly hugs the
    /// physical screen boundary so it never overlaps interior window buttons.
    #[must_use]
    pub fn trigger_hitbox(slot: HotZoneSlot, display: (f32, f32)) -> Rect {
        match slot {
            HotZoneSlot::Top => {
                let w = Self::edge_span(display.0);
                Rect {
                    x: (display.0 - w) * 0.5,
                    y: 0.0,
                    w,
                    h: EDGE_TRIGGER_DEPTH,
                }
            }
            HotZoneSlot::Bottom => {
                let w = Self::edge_span(display.0);
                Rect {
                    x: (display.0 - w) * 0.5,
                    y: (display.1 - EDGE_TRIGGER_DEPTH).max(0.0),
                    w,
                    h: EDGE_TRIGGER_DEPTH,
                }
            }
            HotZoneSlot::Left => {
                let h = Self::edge_span(display.1);
                Rect {
                    x: 0.0,
                    y: (display.1 - h) * 0.5,
                    w: EDGE_TRIGGER_DEPTH,
                    h,
                }
            }
            HotZoneSlot::Right => {
                let h = Self::edge_span(display.1);
                Rect {
                    x: (display.0 - EDGE_TRIGGER_DEPTH).max(0.0),
                    y: (display.1 - h) * 0.5,
                    w: EDGE_TRIGGER_DEPTH,
                    h,
                }
            }
            HotZoneSlot::TopLeft => Rect {
                x: 0.0,
                y: 0.0,
                w: CORNER_TRIGGER_SIZE,
                h: CORNER_TRIGGER_SIZE,
            },
            HotZoneSlot::TopRight => Rect {
                x: (display.0 - CORNER_TRIGGER_SIZE).max(0.0),
                y: 0.0,
                w: CORNER_TRIGGER_SIZE,
                h: CORNER_TRIGGER_SIZE,
            },
            HotZoneSlot::BottomLeft => Rect {
                x: 0.0,
                y: (display.1 - CORNER_TRIGGER_SIZE).max(0.0),
                w: CORNER_TRIGGER_SIZE,
                h: CORNER_TRIGGER_SIZE,
            },
            HotZoneSlot::BottomRight => Rect {
                x: (display.0 - CORNER_TRIGGER_SIZE).max(0.0),
                y: (display.1 - CORNER_TRIGGER_SIZE).max(0.0),
                w: CORNER_TRIGGER_SIZE,
                h: CORNER_TRIGGER_SIZE,
            },
        }
    }

    /// Proximity detection region: entering this region reveals the visual
    /// affordance smoothly without capturing pointer events.
    #[must_use]
    pub fn proximity_contains(
        slot: HotZoneSlot,
        cursor: (f32, f32),
        display: (f32, f32),
        proximity_px: f32,
    ) -> bool {
        let p = proximity_px.max(EDGE_TRIGGER_DEPTH);
        match slot {
            HotZoneSlot::Top => {
                let w = Self::edge_span(display.0);
                let x0 = ((display.0 - w) * 0.5 - p * 0.5).max(CORNER_ARC_RADIUS);
                let x1 = ((display.0 + w) * 0.5 + p * 0.5).min(display.0 - CORNER_ARC_RADIUS);
                cursor.0 >= x0 && cursor.0 <= x1 && cursor.1 >= 0.0 && cursor.1 <= p
            }
            HotZoneSlot::Bottom => {
                let w = Self::edge_span(display.0);
                let x0 = ((display.0 - w) * 0.5 - p * 0.5).max(CORNER_ARC_RADIUS);
                let x1 = ((display.0 + w) * 0.5 + p * 0.5).min(display.0 - CORNER_ARC_RADIUS);
                cursor.0 >= x0
                    && cursor.0 <= x1
                    && cursor.1 >= (display.1 - p).max(0.0)
                    && cursor.1 <= display.1
            }
            HotZoneSlot::Left => {
                let h = Self::edge_span(display.1);
                let y0 = ((display.1 - h) * 0.5 - p * 0.5).max(CORNER_ARC_RADIUS);
                let y1 = ((display.1 + h) * 0.5 + p * 0.5).min(display.1 - CORNER_ARC_RADIUS);
                cursor.1 >= y0 && cursor.1 <= y1 && cursor.0 >= 0.0 && cursor.0 <= p
            }
            HotZoneSlot::Right => {
                let h = Self::edge_span(display.1);
                let y0 = ((display.1 - h) * 0.5 - p * 0.5).max(CORNER_ARC_RADIUS);
                let y1 = ((display.1 + h) * 0.5 + p * 0.5).min(display.1 - CORNER_ARC_RADIUS);
                cursor.1 >= y0
                    && cursor.1 <= y1
                    && cursor.0 >= (display.0 - p).max(0.0)
                    && cursor.0 <= display.0
            }
            HotZoneSlot::TopLeft => {
                let dx = cursor.0.max(0.0);
                let dy = cursor.1.max(0.0);
                dx * dx + dy * dy <= p * p
            }
            HotZoneSlot::TopRight => {
                let dx = (display.0 - cursor.0).max(0.0);
                let dy = cursor.1.max(0.0);
                dx * dx + dy * dy <= p * p
            }
            HotZoneSlot::BottomLeft => {
                let dx = cursor.0.max(0.0);
                let dy = (display.1 - cursor.1).max(0.0);
                dx * dx + dy * dy <= p * p
            }
            HotZoneSlot::BottomRight => {
                let dx = (display.0 - cursor.0).max(0.0);
                let dy = (display.1 - cursor.1).max(0.0);
                dx * dx + dy * dy <= p * p
            }
        }
    }

    /// Exact in-bounds paint rectangle and per-corner radii for a bezel-flush
    /// edge tab or corner arc (ADR-0254 / ADR-0267).
    ///
    /// Every rect lies strictly inside `[0, display.0] × [0, display.1]` and
    /// inside [`Self::visual_bounds`], so `optics-ui` never clamps it inward
    /// and partial-damage scissors never slice the painted shape in half.
    /// Bezel-facing corners carry `0.0` radius (crisp 90° right angles) while
    /// desktop-facing corners carry smooth continuous curvature evaluated
    /// directly by `optics-gfx`'s per-corner analytic squircle SDF.
    fn bezel_flush_geometry(
        slot: HotZoneSlot,
        display: (f32, f32),
        reveal: f32,
    ) -> (Rect, CornerRadii) {
        let t = (EDGE_TAB_THICKNESS * reveal).max(1.0);
        let r_edge = t.min(EDGE_TAB_INNER_RADIUS);
        match slot {
            HotZoneSlot::Top => {
                let w = Self::edge_span(display.0);
                (
                    Rect {
                        x: (display.0 - w) * 0.5,
                        y: 0.0,
                        w,
                        h: t,
                    },
                    CornerRadii::bottom(r_edge),
                )
            }
            HotZoneSlot::Bottom => {
                let w = Self::edge_span(display.0);
                (
                    Rect {
                        x: (display.0 - w) * 0.5,
                        y: (display.1 - t).max(0.0),
                        w,
                        h: t,
                    },
                    CornerRadii::top(r_edge),
                )
            }
            HotZoneSlot::Left => {
                let h = Self::edge_span(display.1);
                (
                    Rect {
                        x: 0.0,
                        y: (display.1 - h) * 0.5,
                        w: t,
                        h,
                    },
                    CornerRadii::right(r_edge),
                )
            }
            HotZoneSlot::Right => {
                let h = Self::edge_span(display.1);
                (
                    Rect {
                        x: (display.0 - t).max(0.0),
                        y: (display.1 - h) * 0.5,
                        w: t,
                        h,
                    },
                    CornerRadii::left(r_edge),
                )
            }
            HotZoneSlot::TopLeft => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        w: r,
                        h: r,
                    },
                    CornerRadii::new(0.0, 0.0, r, 0.0),
                )
            }
            HotZoneSlot::TopRight => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: (display.0 - r).max(0.0),
                        y: 0.0,
                        w: r,
                        h: r,
                    },
                    CornerRadii::new(0.0, 0.0, 0.0, r),
                )
            }
            HotZoneSlot::BottomLeft => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: 0.0,
                        y: (display.1 - r).max(0.0),
                        w: r,
                        h: r,
                    },
                    CornerRadii::new(0.0, r, 0.0, 0.0),
                )
            }
            HotZoneSlot::BottomRight => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: (display.0 - r).max(0.0),
                        y: (display.1 - r).max(0.0),
                        w: r,
                        h: r,
                    },
                    CornerRadii::new(r, 0.0, 0.0, 0.0),
                )
            }
        }
    }

    fn padded_damage_rect(slot: HotZoneSlot, display: (f32, f32)) -> Rect {
        let mut bounds = Self::visual_bounds(slot, display);
        bounds.x = (bounds.x - 2.0).max(0.0);
        bounds.y = (bounds.y - 2.0).max(0.0);
        bounds.w = (bounds.w + 4.0).min(display.0 - bounds.x);
        bounds.h = (bounds.h + 4.0).min(display.1 - bounds.y);
        bounds
    }

    fn dwell_threshold(&self) -> f32 {
        (self.policy.hover_dwell_ms as f32 / 1000.0).max(0.02)
    }

    fn dwell_boost_for(
        binding: HotZoneBinding,
        runtime: &SlotRuntime,
        dwell_threshold: f32,
    ) -> f32 {
        if !runtime.in_trigger {
            return 0.0;
        }
        match binding.trigger {
            HotZoneTrigger::Hover => {
                if runtime.hover_latched {
                    1.0
                } else {
                    (runtime.dwell_secs / dwell_threshold).clamp(0.0, 1.0)
                }
            }
            HotZoneTrigger::Click => 1.0,
        }
    }

    fn advance_slots(&mut self, cursor: (f32, f32), display: (f32, f32), dt: f32) {
        let proximity = self.policy.proximity_px;
        let dwell_threshold = self.dwell_threshold();
        let bindings = self.policy.bindings.clone();
        let visual_dirty = std::mem::take(&mut self.visual_dirty);
        for slot in HotZoneSlot::ALL {
            let idx = Self::slot_index(slot);
            let binding = bindings.iter().copied().find(|b| b.slot == slot);
            let suppressed = binding.is_some_and(|b| self.is_suppressed(b));
            let active = binding.is_some() && !suppressed;
            let runtime = &mut self.slots[idx];
            let prev_reveal = runtime.reveal;
            let prev_in_trigger = runtime.in_trigger;
            let prev_dwell = runtime.dwell_secs;
            let prev_latched = runtime.hover_latched;
            runtime.dirty = false;

            if suppressed {
                // Suppressed slots stop drawing immediately (Dock expanded,
                // modal/launcher active, or fullscreen): retire cleanly in one
                // bounded drain instead of decaying invisibly across ~20 frames.
                if prev_reveal > 0.002 {
                    runtime.retire_frames = RETIRE_DRAIN_FRAMES;
                    runtime.dirty = true;
                } else if runtime.retire_frames > 0 {
                    runtime.retire_frames -= 1;
                }
                runtime.reveal = 0.0;
                runtime.in_trigger = false;
                runtime.dwell_secs = 0.0;
                runtime.hover_latched = false;
                runtime.press_armed = false;
                continue;
            }

            let target = if active && Self::proximity_contains(slot, cursor, display, proximity) {
                1.0
            } else {
                0.0
            };
            let in_trigger =
                active && contains(Self::trigger_hitbox(slot, display), cursor.0, cursor.1);
            runtime.in_trigger = in_trigger;

            let was_visible = prev_reveal > 0.002;
            runtime.reveal = approach(prev_reveal, target, REVEAL_RATE, dt, self.reduced_motion);
            if target == 0.0 && runtime.reveal <= 0.002 {
                runtime.reveal = 0.0;
                if was_visible {
                    runtime.retire_frames = RETIRE_DRAIN_FRAMES;
                } else if runtime.retire_frames > 0 {
                    runtime.retire_frames -= 1;
                }
            } else if runtime.reveal > 0.002 {
                runtime.retire_frames = 0;
            }

            if let Some(binding) = binding {
                if binding.trigger == HotZoneTrigger::Hover {
                    if in_trigger {
                        if !runtime.hover_latched {
                            runtime.dwell_secs += dt;
                            if runtime.dwell_secs >= dwell_threshold {
                                runtime.hover_latched = true;
                                runtime.dwell_secs = dwell_threshold;
                                self.pending_summon = Some((binding.slot, binding.target));
                            }
                        }
                    } else {
                        runtime.dwell_secs = 0.0;
                        runtime.hover_latched = false;
                    }
                }
            } else {
                runtime.dwell_secs = 0.0;
                runtime.hover_latched = false;
                runtime.press_armed = false;
            }

            if (runtime.reveal - prev_reveal).abs() > 1e-4
                || runtime.in_trigger != prev_in_trigger
                || (runtime.dwell_secs - prev_dwell).abs() > 1e-4
                || runtime.hover_latched != prev_latched
                || (visual_dirty && (runtime.reveal > 0.002 || runtime.retire_frames > 0))
            {
                runtime.dirty = true;
            }
        }
    }
}

impl Chrome for HotZones {
    fn persistent_decoration(&self) -> bool {
        true
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::SpaceUse(space_use) => {
                if self.space_use != space_use {
                    self.space_use = space_use;
                    self.visual_dirty = true;
                }
                if space_use == SpaceUse::Fullscreen {
                    self.pending_summon = None;
                    for slot in &mut self.slots {
                        if slot.reveal > 0.0 {
                            slot.retire_frames = RETIRE_DRAIN_FRAMES;
                        }
                        slot.reveal = 0.0;
                        slot.in_trigger = false;
                        slot.dwell_secs = 0.0;
                        slot.hover_latched = false;
                        slot.press_armed = false;
                    }
                }
            }
            ChromeUpdate::DockExpanded(expanded) => {
                if self.dock_expanded != expanded {
                    self.dock_expanded = expanded;
                    self.visual_dirty = true;
                }
            }
            ChromeUpdate::LauncherActive(active) => {
                if self.launcher_active != active {
                    self.launcher_active = active;
                    self.visual_dirty = true;
                }
            }
            ChromeUpdate::ReducedMotion(reduced) => {
                self.reduced_motion = reduced;
            }
            ChromeUpdate::Appearance(design) => {
                self.design = *design;
                self.visual_dirty = true;
            }
            ChromeUpdate::HotZones(policy) => {
                self.set_policy(policy.clone());
            }
            _ => {}
        }
    }

    fn prepare_frame(
        &mut self,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        if display.0 <= 0.0 || display.1 <= 0.0 {
            return;
        }
        let cursor = (raw.cursor.x, raw.cursor.y);
        let dt = raw.dt_seconds.clamp(0.0, 0.1);
        self.advance_slots(cursor, display, dt);
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if let Some(summon) = self.pending_summon.take() {
            out.hot_zone_summon = Some(summon);
        }

        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        if display.0 <= 0.0 || display.1 <= 0.0 {
            return;
        }
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down[0];
        let pressed = down && !self.prev_down;
        let released = !down && self.prev_down;
        self.prev_down = down;

        let dwell_threshold = self.dwell_threshold();
        let bindings = self.policy.bindings.clone();

        for binding in bindings {
            let idx = Self::slot_index(binding.slot);
            if self.is_suppressed(binding) {
                self.slots[idx].press_armed = false;
                continue;
            }

            let in_trigger = contains(
                Self::trigger_hitbox(binding.slot, display),
                cursor.0,
                cursor.1,
            );
            let runtime = &mut self.slots[idx];

            if binding.trigger == HotZoneTrigger::Click {
                if pressed && in_trigger {
                    runtime.press_armed = true;
                }
                if released {
                    if runtime.press_armed && in_trigger {
                        out.hot_zone_summon = Some((binding.slot, binding.target));
                    }
                    runtime.press_armed = false;
                }
            }

            let reveal = runtime.reveal;
            if reveal <= 0.002 {
                continue;
            }

            let (paint_rect, corner_radii) =
                Self::bezel_flush_geometry(binding.slot, display, reveal);
            if paint_rect.w <= 0.0 || paint_rect.h <= 0.0 {
                continue;
            }

            let dwell_boost = Self::dwell_boost_for(binding, runtime, dwell_threshold);
            let base = if dwell_boost > 0.01 {
                self.design.colors.application_accent
            } else {
                self.design.colors.menu_text
            };
            let (r, g, b, _) = base.components();
            let alpha = ((0.42 + 0.48 * dwell_boost) * reveal * 255.0).round() as u8;
            let fill = Color::rgba(r, g, b, alpha);

            frame.place(
                &format!("tessera-hot-zone-{}", binding.slot.name()),
                &materials::chrome_place(
                    paint_rect,
                    LayoutOpts {
                        bg: fill,
                        corner_radii,
                        corner_curvature: HOT_ZONE_CURVATURE,
                        border_width: 0.0,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |_| {},
            );
        }
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        if self.space_use == SpaceUse::Fullscreen || self.launcher_active {
            return false;
        }
        // Only Click-triggered slots capture pointer presses inside their tight
        // bezel-flush trigger hitbox; proximity bands and Hover slots never
        // steal clicks from windows beneath.
        self.policy.bindings.iter().any(|&binding| {
            binding.trigger == HotZoneTrigger::Click
                && !self.is_suppressed(binding)
                && contains(Self::trigger_hitbox(binding.slot, display), x, y)
        })
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        windows: &[Window],
        workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        self.captures_pointer(x, y, display, windows, workspaces)
            .then_some(CursorShape::Pointer)
    }

    fn anim_pending(&self) -> bool {
        if self.space_use == SpaceUse::Fullscreen || self.launcher_active {
            return false;
        }
        self.policy.bindings.iter().any(|&binding| {
            if self.is_suppressed(binding) {
                return false;
            }
            let rt = &self.slots[Self::slot_index(binding.slot)];
            let hover_waiting =
                binding.trigger == HotZoneTrigger::Hover && rt.in_trigger && !rt.hover_latched;
            (rt.reveal > 0.002 && rt.reveal < 0.998)
                || (rt.dwell_secs > 0.0 && !rt.hover_latched)
                || hover_waiting
        })
    }

    fn damage_pending(&self) -> bool {
        self.slots.iter().any(|rt| rt.retire_frames > 0 || rt.dirty)
            || self.pending_summon.is_some()
    }

    fn requires_composition(&self) -> bool {
        self.slots
            .iter()
            .any(|rt| rt.reveal > 0.002 || rt.retire_frames > 0 || rt.dirty)
    }

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        let mut regions = Vec::new();
        for slot in HotZoneSlot::ALL {
            let rt = &self.slots[Self::slot_index(slot)];
            if rt.reveal > 0.002 || rt.retire_frames > 0 || rt.dirty {
                regions.push(Self::padded_damage_rect(slot, display));
            }
        }
        Some(regions)
    }

    fn pointer_damage_regions(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        if self.space_use == SpaceUse::Fullscreen || self.launcher_active {
            return Some(Vec::new());
        }
        let mut regions = Vec::new();
        for &binding in &self.policy.bindings {
            if !self.is_suppressed(binding)
                && (Self::proximity_contains(
                    binding.slot,
                    (x, y),
                    display,
                    self.policy.proximity_px,
                ) || contains(Self::trigger_hitbox(binding.slot, display), x, y))
            {
                regions.push(Self::padded_damage_rect(binding.slot, display));
            }
        }
        Some(regions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects_overlap(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
    }

    fn rect_contained_in(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x - 1e-4
            && inner.y >= outer.y - 1e-4
            && inner.x + inner.w <= outer.x + outer.w + 1e-4
            && inner.y + inner.h <= outer.y + outer.h + 1e-4
    }

    #[test]
    fn all_eight_slots_are_pairwise_disjoint() {
        let display = (1920.0, 1080.0);
        for (i, &a) in HotZoneSlot::ALL.iter().enumerate() {
            for &b in &HotZoneSlot::ALL[i + 1..] {
                assert!(
                    !rects_overlap(
                        HotZones::visual_bounds(a, display),
                        HotZones::visual_bounds(b, display)
                    ),
                    "visual bounds of {a:?} and {b:?} must not overlap"
                );
                assert!(
                    !rects_overlap(
                        HotZones::trigger_hitbox(a, display),
                        HotZones::trigger_hitbox(b, display)
                    ),
                    "trigger hitboxes of {a:?} and {b:?} must not overlap"
                );
            }
        }
    }

    #[test]
    fn bezel_flush_geometry_stays_strictly_inside_visual_bounds_and_viewport() {
        let display = (1920.0, 1080.0);
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: display.1,
        };
        for slot in HotZoneSlot::ALL {
            let bounds = HotZones::visual_bounds(slot, display);
            for reveal in [0.05_f32, 0.25, 0.5, 1.0] {
                let (rect, radii) = HotZones::bezel_flush_geometry(slot, display, reveal);
                assert!(
                    rect_contained_in(rect, viewport),
                    "{slot:?} at reveal={reveal} overflowed viewport: {rect:?}"
                );
                assert!(
                    rect_contained_in(rect, bounds),
                    "{slot:?} at reveal={reveal} overflowed visual_bounds {bounds:?}: {rect:?}"
                );
                match slot {
                    HotZoneSlot::Top => {
                        assert_eq!((radii.tl, radii.tr), (0.0, 0.0));
                        assert!(radii.bl > 0.0 && radii.br > 0.0);
                    }
                    HotZoneSlot::Bottom => {
                        assert_eq!((radii.bl, radii.br), (0.0, 0.0));
                        assert!(radii.tl > 0.0 && radii.tr > 0.0);
                    }
                    HotZoneSlot::Left => {
                        assert_eq!((radii.tl, radii.bl), (0.0, 0.0));
                        assert!(radii.tr > 0.0 && radii.br > 0.0);
                    }
                    HotZoneSlot::Right => {
                        assert_eq!((radii.tr, radii.br), (0.0, 0.0));
                        assert!(radii.tl > 0.0 && radii.bl > 0.0);
                    }
                    HotZoneSlot::TopLeft => {
                        assert_eq!((radii.tl, radii.tr, radii.bl), (0.0, 0.0, 0.0));
                        assert!(radii.br > 0.0);
                    }
                    HotZoneSlot::TopRight => {
                        assert_eq!((radii.tl, radii.tr, radii.br), (0.0, 0.0, 0.0));
                        assert!(radii.bl > 0.0);
                    }
                    HotZoneSlot::BottomLeft => {
                        assert_eq!((radii.tl, radii.br, radii.bl), (0.0, 0.0, 0.0));
                        assert!(radii.tr > 0.0);
                    }
                    HotZoneSlot::BottomRight => {
                        assert_eq!((radii.tr, radii.br, radii.bl), (0.0, 0.0, 0.0));
                        assert!(radii.tl > 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn idle_hot_zones_have_zero_cost() {
        let mut hz = HotZones::default();
        hz.advance_slots((960.0, 540.0), (1920.0, 1080.0), 0.016);
        assert!(!hz.anim_pending());
        assert!(!hz.damage_pending());
        assert!(!hz.requires_composition());
        assert_eq!(hz.damage_regions(&[], (1920.0, 1080.0)), Some(Vec::new()));
    }

    #[test]
    fn proximity_reveals_only_approached_slot_and_fullscreen_silences_all() {
        let mut hz = HotZones::default();
        let display = (1920.0, 1080.0);
        for _ in 0..4 {
            hz.advance_slots((960.0, 10.0), display, 0.05);
        }
        assert!(hz.slots[HotZones::slot_index(HotZoneSlot::Top)].reveal > 0.5);
        assert_eq!(
            hz.slots[HotZones::slot_index(HotZoneSlot::Bottom)].reveal,
            0.0
        );
        assert!(hz.requires_composition());

        hz.update(ChromeUpdate::SpaceUse(SpaceUse::Fullscreen));
        assert_eq!(hz.slots[HotZones::slot_index(HotZoneSlot::Top)].reveal, 0.0);
        assert!(!hz.anim_pending());
    }

    #[test]
    fn suppression_retires_immediately_without_invisible_decay_frames() {
        let mut hz = HotZones::default();
        let display = (1920.0, 1080.0);
        for _ in 0..8 {
            hz.advance_slots((960.0, 1070.0), display, 0.05);
        }
        assert!(hz.slots[HotZones::slot_index(HotZoneSlot::Bottom)].reveal > 0.8);

        // Expanding the Dock suppresses the Bottom hot zone immediately.
        hz.update(ChromeUpdate::DockExpanded(true));
        hz.advance_slots((960.0, 1070.0), display, 0.016);
        let rt = &hz.slots[HotZones::slot_index(HotZoneSlot::Bottom)];
        assert_eq!(rt.reveal, 0.0);
        assert_eq!(rt.retire_frames, RETIRE_DRAIN_FRAMES);
        assert!(!hz.anim_pending());

        // Bounded drain finishes in exactly RETIRE_DRAIN_FRAMES additional frames.
        for _ in 0..RETIRE_DRAIN_FRAMES {
            hz.advance_slots((960.0, 1070.0), display, 0.016);
        }
        assert!(!hz.damage_pending());
        assert!(!hz.requires_composition());
    }

    #[test]
    fn hover_dwell_advances_in_prepare_frame_and_keeps_accent_boost_when_latched() {
        let mut hz = HotZones::default();
        let display = (1920.0, 1080.0);
        let binding = hz.policy.binding_for(HotZoneSlot::Top).unwrap();
        let threshold = hz.dwell_threshold();

        // Dwell inside top edge trigger past threshold.
        hz.advance_slots((960.0, 2.0), display, threshold * 0.6);
        assert!(hz.pending_summon.is_none());
        assert!(hz.anim_pending());

        hz.advance_slots((960.0, 2.0), display, threshold * 0.6);
        assert_eq!(
            hz.pending_summon,
            Some((HotZoneSlot::Top, HotZoneTarget::GlanceHud))
        );
        // Accent boost remains 1.0 on the summon frame (no colour flicker back to neutral).
        let rt = &hz.slots[HotZones::slot_index(HotZoneSlot::Top)];
        assert!(rt.hover_latched);
        assert_eq!(HotZones::dwell_boost_for(binding, rt, threshold), 1.0);
    }

    #[test]
    fn click_slot_captures_only_tight_trigger_hitbox_and_bounds_pointer_damage() {
        let hz = HotZones::default();
        let display = (1920.0, 1080.0);
        let ws = WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(hz.captures_pointer(2.0, 2.0, display, &[], &ws));
        let ptr_dmg = hz
            .pointer_damage_regions(2.0, 2.0, display, &[], &ws)
            .expect("HotZones must localize pointer hover damage");
        assert_eq!(ptr_dmg.len(), 1);
        assert!(ptr_dmg[0].w <= CORNER_ARC_RADIUS + 4.0);
        assert!(ptr_dmg[0].h <= CORNER_ARC_RADIUS + 4.0);
        assert!(!hz.captures_pointer(30.0, 30.0, display, &[], &ws));
        assert!(!hz.captures_pointer(960.0, 1078.0, display, &[], &ws));
    }

    #[test]
    fn reduced_motion_and_trigger_toggle_emit_exact_damage_and_settle_idle() {
        let mut hz = HotZones::default();
        let display = (1920.0, 1080.0);
        hz.update(ChromeUpdate::ReducedMotion(true));

        hz.advance_slots((20.0, 20.0), display, 0.016);
        assert_eq!(
            hz.slots[HotZones::slot_index(HotZoneSlot::TopLeft)].reveal,
            1.0
        );
        assert!(hz.damage_pending());
        assert_eq!(hz.damage_regions(&[], display).unwrap().len(), 1);

        hz.advance_slots((20.0, 20.0), display, 0.016);
        assert!(!hz.damage_pending());

        hz.advance_slots((3.0, 3.0), display, 0.016);
        assert!(hz.damage_pending());

        hz.advance_slots((20.0, 20.0), display, 0.016);
        assert!(hz.damage_pending());

        for _ in 0..=RETIRE_DRAIN_FRAMES + 1 {
            hz.advance_slots((960.0, 540.0), display, 0.016);
        }
        assert!(!hz.anim_pending());
        assert!(!hz.damage_pending());
        assert!(!hz.requires_composition());
    }
}
