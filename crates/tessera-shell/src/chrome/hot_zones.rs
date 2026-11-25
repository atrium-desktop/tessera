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

use optics_ui::{Color, Frame, Input, LayoutOpts, Rect};
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
/// Interior corner radius for an edge tab (slightly deeper than thickness so
/// the exposed inner curve reads as continuous G2-style curvature while the
/// bezel-facing corners remain crisp 90° right angles).
pub const EDGE_TAB_INNER_RADIUS: f32 = 8.0;
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
            design: Design::dark(),
        }
    }

    /// Replace the active Hot Zone policy (startup or live config reload).
    pub fn set_policy(&mut self, policy: HotZonePolicy) {
        self.policy = policy;
        for slot in &mut self.slots {
            slot.dwell_secs = 0.0;
            slot.hover_latched = false;
            slot.press_armed = false;
        }
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

    /// Paint rectangle for a bezel-flush edge tab or corner arc. Extending the
    /// bezel-touching edge beyond the output boundary lets viewport clipping
    /// produce crisp 90° right angles on the bezel side while exposing smooth
    /// continuous curvature on the desktop-facing side (ADR-0254).
    fn bezel_flush_paint_rect(slot: HotZoneSlot, display: (f32, f32), reveal: f32) -> (Rect, f32) {
        let t = (EDGE_TAB_THICKNESS * reveal).max(1.0);
        let r_edge = EDGE_TAB_INNER_RADIUS;
        match slot {
            HotZoneSlot::Top => {
                let w = Self::edge_span(display.0);
                (
                    Rect {
                        x: (display.0 - w) * 0.5,
                        y: -r_edge,
                        w,
                        h: t + r_edge,
                    },
                    r_edge,
                )
            }
            HotZoneSlot::Bottom => {
                let w = Self::edge_span(display.0);
                (
                    Rect {
                        x: (display.0 - w) * 0.5,
                        y: display.1 - t,
                        w,
                        h: t + r_edge,
                    },
                    r_edge,
                )
            }
            HotZoneSlot::Left => {
                let h = Self::edge_span(display.1);
                (
                    Rect {
                        x: -r_edge,
                        y: (display.1 - h) * 0.5,
                        w: t + r_edge,
                        h,
                    },
                    r_edge,
                )
            }
            HotZoneSlot::Right => {
                let h = Self::edge_span(display.1);
                (
                    Rect {
                        x: display.0 - t,
                        y: (display.1 - h) * 0.5,
                        w: t + r_edge,
                        h,
                    },
                    r_edge,
                )
            }
            HotZoneSlot::TopLeft => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: -r,
                        y: -r,
                        w: r * 2.0,
                        h: r * 2.0,
                    },
                    r,
                )
            }
            HotZoneSlot::TopRight => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: display.0 - r,
                        y: -r,
                        w: r * 2.0,
                        h: r * 2.0,
                    },
                    r,
                )
            }
            HotZoneSlot::BottomLeft => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: -r,
                        y: display.1 - r,
                        w: r * 2.0,
                        h: r * 2.0,
                    },
                    r,
                )
            }
            HotZoneSlot::BottomRight => {
                let r = (CORNER_ARC_RADIUS * reveal).max(2.0);
                (
                    Rect {
                        x: display.0 - r,
                        y: display.1 - r,
                        w: r * 2.0,
                        h: r * 2.0,
                    },
                    r,
                )
            }
        }
    }

    fn advance_slots(&mut self, cursor: (f32, f32), display: (f32, f32), dt: f32) {
        let proximity = self.policy.proximity_px;
        let bindings = self.policy.bindings.clone();
        for slot in HotZoneSlot::ALL {
            let idx = Self::slot_index(slot);
            let binding = bindings.iter().copied().find(|b| b.slot == slot);
            let active = binding.is_some_and(|b| !self.is_suppressed(b));
            let target = if active && Self::proximity_contains(slot, cursor, display, proximity) {
                1.0
            } else {
                0.0
            };
            let in_trigger =
                active && contains(Self::trigger_hitbox(slot, display), cursor.0, cursor.1);
            let runtime = &mut self.slots[idx];
            runtime.in_trigger = in_trigger;
            let was_visible = runtime.reveal > 0.002;
            runtime.reveal = approach(runtime.reveal, target, REVEAL_RATE, dt, self.reduced_motion);
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
                self.space_use = space_use;
                if space_use == SpaceUse::Fullscreen {
                    for slot in &mut self.slots {
                        if slot.reveal > 0.0 {
                            slot.retire_frames = RETIRE_DRAIN_FRAMES;
                        }
                        slot.reveal = 0.0;
                        slot.dwell_secs = 0.0;
                        slot.hover_latched = false;
                        slot.press_armed = false;
                    }
                }
            }
            ChromeUpdate::DockExpanded(expanded) => {
                self.dock_expanded = expanded;
            }
            ChromeUpdate::LauncherActive(active) => {
                self.launcher_active = active;
            }
            ChromeUpdate::ReducedMotion(reduced) => {
                self.reduced_motion = reduced;
            }
            ChromeUpdate::Appearance(design) => {
                self.design = *design;
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
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        if display.0 <= 0.0 || display.1 <= 0.0 {
            return;
        }
        let cursor = (raw.cursor.x, raw.cursor.y);
        let dt = raw.dt_seconds.clamp(0.0, 0.1);
        let down = raw.mouse_down[0];
        let pressed = down && !self.prev_down;
        let released = !down && self.prev_down;
        self.prev_down = down;

        let dwell_threshold = (self.policy.hover_dwell_ms as f32 / 1000.0).max(0.02);
        let bindings = self.policy.bindings.clone();

        for binding in bindings {
            let idx = Self::slot_index(binding.slot);
            if self.is_suppressed(binding) {
                self.slots[idx].dwell_secs = 0.0;
                self.slots[idx].hover_latched = false;
                self.slots[idx].press_armed = false;
                continue;
            }

            let hitbox = Self::trigger_hitbox(binding.slot, display);
            let in_trigger = contains(hitbox, cursor.0, cursor.1);
            let runtime = &mut self.slots[idx];

            match binding.trigger {
                HotZoneTrigger::Hover => {
                    if in_trigger {
                        if !runtime.hover_latched {
                            runtime.dwell_secs += dt;
                            if runtime.dwell_secs >= dwell_threshold {
                                runtime.hover_latched = true;
                                runtime.dwell_secs = 0.0;
                                out.hot_zone_summon = Some((binding.slot, binding.target));
                            }
                        }
                    } else {
                        runtime.dwell_secs = 0.0;
                        runtime.hover_latched = false;
                    }
                }
                HotZoneTrigger::Click => {
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
            }

            let reveal = runtime.reveal;
            if reveal <= 0.002 {
                continue;
            }

            let dwell_boost = if binding.trigger == HotZoneTrigger::Hover && in_trigger {
                (runtime.dwell_secs / dwell_threshold).clamp(0.0, 1.0)
            } else if in_trigger {
                1.0
            } else {
                0.0
            };

            let (paint_rect, radius) = Self::bezel_flush_paint_rect(binding.slot, display, reveal);
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
                        radius,
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
            (rt.reveal > 0.002 && rt.reveal < 0.998) || rt.dwell_secs > 0.0 || hover_waiting
        })
    }

    fn damage_pending(&self) -> bool {
        self.slots.iter().any(|rt| rt.retire_frames > 0)
    }

    fn requires_composition(&self) -> bool {
        self.slots
            .iter()
            .any(|rt| rt.reveal > 0.002 || rt.retire_frames > 0)
    }

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        let mut regions = Vec::new();
        for &binding in &self.policy.bindings {
            let rt = &self.slots[Self::slot_index(binding.slot)];
            if rt.reveal > 0.002 || rt.retire_frames > 0 {
                let mut bounds = Self::visual_bounds(binding.slot, display);
                bounds.x = (bounds.x - 2.0).max(0.0);
                bounds.y = (bounds.y - 2.0).max(0.0);
                bounds.w = (bounds.w + 4.0).min(display.0 - bounds.x);
                bounds.h = (bounds.h + 4.0).min(display.1 - bounds.y);
                regions.push(bounds);
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
    fn idle_hot_zones_have_zero_cost() {
        let mut hz = HotZones::default();
        hz.advance_slots((960.0, 540.0), (1920.0, 1080.0), 0.016);
        assert!(!hz.anim_pending());
        assert!(!hz.requires_composition());
        assert_eq!(hz.damage_regions(&[], (1920.0, 1080.0)), Some(Vec::new()));
    }

    #[test]
    fn proximity_reveals_only_approached_slot_and_fullscreen_silences_all() {
        let mut hz = HotZones::default();
        let display = (1920.0, 1080.0);
        // Move near the top edge (GlanceHud default) across several frames.
        for _ in 0..4 {
            hz.advance_slots((960.0, 10.0), display, 0.05);
        }
        assert!(hz.slots[HotZones::slot_index(HotZoneSlot::Top)].reveal > 0.5);
        assert_eq!(
            hz.slots[HotZones::slot_index(HotZoneSlot::Bottom)].reveal,
            0.0
        );
        assert!(hz.requires_composition());

        // Fullscreen immediately silences all slots.
        hz.update(ChromeUpdate::SpaceUse(SpaceUse::Fullscreen));
        assert_eq!(hz.slots[HotZones::slot_index(HotZoneSlot::Top)].reveal, 0.0);
        assert!(!hz.anim_pending());
    }

    #[test]
    fn click_slot_captures_only_tight_trigger_hitbox_not_proximity() {
        let hz = HotZones::default();
        let display = (1920.0, 1080.0);
        let ws = WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        // TopLeft is Overview (Click).
        assert!(hz.captures_pointer(2.0, 2.0, display, &[], &ws));
        // Inside proximity (30, 30) must NOT capture clicks from windows.
        assert!(!hz.captures_pointer(30.0, 30.0, display, &[], &ws));
        // Bottom is Dock (Hover) — never captures pointer clicks.
        assert!(!hz.captures_pointer(960.0, 1078.0, display, &[], &ws));
    }
}
