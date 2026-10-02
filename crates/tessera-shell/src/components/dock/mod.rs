//! macOS-style dock: a rounded translucent bar anchored to a screen edge of
//! the output (bottom by default; the left or right edge via
//! `[dock] position`) holding a persistent strip of pinned application
//! icons. Unlike a window list, the dock is populated from launchable
//! `.desktop` entries the binary pins (plus any running window that is not
//! already pinned), so it shows real XDG icons even when nothing is running.
//!
//! Follows the Model-Motion-View (MMV) architecture (ADR-0174):
//! - [`state`]: Headless state machine (apps, selections, drag-reorder, autohide latching).
//! - [`motion`]: Motion dynamics powered by Optics `transit` (reveal & magnification springs).
//! - [`rendering`]: Pure presentation projection onto Lens Frame, glass materials, and backdrops.

pub mod motion;
pub mod rendering;
pub mod state;

#[cfg(test)]
pub mod tests;

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hasher;

pub(crate) use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, Rect};
pub(crate) use tessera_design::materials::{chrome_place, surface_layout};
pub(crate) use tessera_design::{Design, GlassRole, materials};

pub(crate) use crate::component::{
    AppCatalog, AppMenu, BackdropRegion, Chrome, ChromeEvents, ChromeUpdate, CursorShape, IconSet,
    LiquidGlassRegion, LivePreviewPresentation, Localizer, Message, PinAction, PopupSide,
    PreviewCard, Reserved, ellipsize, liquid_glass_region_id, preview,
};
pub(crate) use tessera_desktop::app::Entry;
pub(crate) use tessera_desktop::dock::DockPosition;
pub(crate) use tessera_desktop::window::{SpaceUse, Window};
pub(crate) use tessera_desktop::workspace::WorkspaceSnapshot;
pub(crate) use tessera_primitives::input::{KeyAction, KeyChar, key_action};

pub use motion::{
    AUTOHIDE_CONTENT_DRAIN_END, AUTOHIDE_CONTENT_INTERACTION_MIN, AUTOHIDE_DWELL_THRESHOLD,
    AUTOHIDE_HANDLE_HEIGHT, AUTOHIDE_HANDLE_WIDTH, AUTOHIDE_IDLE_TIMEOUT,
    AUTOHIDE_QUICK_DISMISS_TIMEOUT, DOCK_BASELINE_INSET, DOCK_DOT, DOCK_DOT_STADIUM,
    DOCK_EDGE_MARGIN, DOCK_PAD, DOCK_PANEL_HEIGHT, DOCK_REVEAL_SPRING, DOCK_SECTION_GAP,
    DOCK_SPRING, DOCK_TILE, DOCK_TILE_BIRTH, DOCK_TILE_GAP, DOCK_TILE_MAX, DRAG_LIFT_SCALE,
    DRAG_THRESHOLD, DockMotion, EDGE_DRAG_PROXIMITY, MAGNIFY_RADIUS_TILES, PREVIEW_ASPECT,
    PREVIEW_CARD_GAP, PREVIEW_CARD_MAX_WIDTH, PREVIEW_CARD_MIN_WIDTH, PREVIEW_LABEL_HEIGHT,
    PREVIEW_PANEL_GAP, PREVIEW_PANEL_PAD, PREVIEW_SCREEN_MARGIN, REVEAL_SETTLE_VALUE_EPS,
    REVEAL_SETTLE_VELOCITY_EPS, SETTLED_DRAIN_FRAMES, SPRING_DAMPING, SPRING_OVERSHOOT_MARGIN,
    SPRING_STIFFNESS, TOOLTIP_BAND, TOOLTIP_DWELL, TOOLTIP_FADE_SPEED, TOOLTIP_GAP,
    TOOLTIP_HEIGHT, detect_cursor_retreat,
};
pub use state::{DockApp, DockState, DropSection, PressState, PressTarget};

pub(crate) type SpringState = crate::widgets::motion::Spring;

/// One resolved tile for the current frame: a pinned app or a transient
/// running application.
#[derive(Clone)]
pub(crate) struct Tile {
    pub(crate) key: String,
    pub(crate) icon: Option<*mut c_void>,
    pub(crate) running: bool,
    pub(crate) activated: bool,
    pub(crate) focus: Option<tessera_desktop::window::WindowId>,
    pub(crate) windows: Vec<tessera_desktop::window::WindowId>,
    pub(crate) app: Option<usize>,
    pub(crate) spawn: Option<usize>,
    pub(crate) label: String,
    pub(crate) launchpad: bool,
    pub(crate) pinned: bool,
    pub(crate) pin_entry: Option<String>,
}

impl Tile {
    fn launchpad(label: &str) -> Tile {
        Tile {
            key: "launchpad".to_string(),
            icon: None,
            running: false,
            activated: false,
            focus: None,
            windows: Vec::new(),
            app: None,
            spawn: None,
            label: label.to_string(),
            launchpad: true,
            pinned: true,
            pin_entry: None,
        }
    }
}

struct TransientGroup {
    key: String,
    app_id: Option<String>,
    entry: Option<usize>,
    windows: Vec<usize>,
}

pub(crate) struct TileCache {
    signature: Option<u64>,
    label: String,
    window_order: Vec<tessera_desktop::window::WindowId>,
    tiles: Vec<Tile>,
}

/// The macOS-style dock component.
pub struct Dock {
    /// Pinned launchable apps, in dock order.
    pub apps: Vec<DockApp>,
    /// The complete enumerated application catalog, refreshed with every rescan.
    pub all_apps: Vec<Entry>,
    /// Catalog revision counter bumped when app order changes.
    pub catalog_revision: u64,
    /// Pending optimistic reorder list to commit through [`ChromeEvents`].
    pub pending_order: Option<Vec<String>>,
    /// Active press or drag state.
    pub press: Option<PressState>,
    /// Whether autohide mode is enabled by user preference.
    pub autohide: bool,
    /// Inactivity timer tracking elapsed seconds since pointer left the dock.
    pub autohide_idle: f32,
    /// Inactivity timeout before collapsing.
    pub autohide_timeout: f32,
    /// Dwell threshold before expanding.
    pub autohide_dwell_threshold: f32,
    /// Continuous hover dwell time in seconds on collapsed indicator.
    pub autohide_dwell: f32,
    /// Whether user engaged with dock during current reveal session.
    pub dock_interacted: bool,
    /// Anti-rebound latch preventing re-opening while an explicit collapse is in flight.
    pub dismiss_latched: bool,
    /// Whether a visible window intersects the resting dock footprint.
    pub dock_obscured: bool,
    /// Whether dock collapse must complete uninterrupted before re-triggering.
    pub collapse_pending: bool,
    /// Space utilization of windows.
    pub space_use: SpaceUse,
    /// Configured dock screen edge.
    pub position: DockPosition,
    /// Whether collapsed indicator entry is armed.
    pub hidden_trigger_armed: bool,
    /// Previous frame's cursor position.
    pub last_cursor: Option<(f32, f32)>,
    /// Most recently rendered logical display size.
    pub last_display: Option<(f32, f32)>,
    /// Motion dynamics state (Optics `transit` reveal spring and drain ring).
    pub motion: DockMotion,
    /// Per-tile size springs keyed by tile key.
    pub sizes: HashMap<String, SpringState>,
    /// Current reveal progress in `[0.0, 1.0]`.
    pub autohide_reveal: f32,
    /// Number of swapchain drain frames remaining.
    pub settled_drain_frames: u8,
    /// Whether autohide was advanced during `prepare_backdrop` for the current frame.
    pub autohide_stepped_in_prepass: bool,
    /// Whether autohide was moving on the previous frame.
    pub was_autohide_animating: bool,
    /// Whether any animation is currently in flight.
    pub anim_active: bool,
    /// Previous mouse down state.
    pub prev_down: bool,
    /// Application context menu.
    pub app_menu: AppMenu,
    pub menu_tile: Option<String>,
    pub hovered_tile: Option<String>,
    pub hover_elapsed: f32,
    pub tooltip_tile: Option<String>,
    pub tooltip_alpha: f32,
    pub live_preview: Option<LivePreviewPresentation>,
    pub hover_surface_bounds: Option<Rect>,
    pub hover_owner_bounds: Option<Rect>,
    pub hovered_preview: Option<tessera_desktop::window::WindowId>,
    pub reduced_motion: bool,
    pub(crate) tile_cache: RefCell<TileCache>,
    pub all_windows: Vec<Window>,
    pub icons: IconSet,
    pub scale: f32,
    pub design: Design,
}

impl Default for Dock {
    fn default() -> Self {
        Self::new()
    }
}

impl Dock {
    /// Create a new Dock with default configuration.
    #[must_use]
    pub fn new() -> Self {
        let motion = DockMotion::new(false);
        let reveal_val = motion.reveal_value();
        Self {
            apps: Vec::new(),
            all_apps: Vec::new(),
            catalog_revision: 0,
            pending_order: None,
            press: None,
            autohide: false,
            autohide_idle: 0.0,
            autohide_timeout: AUTOHIDE_IDLE_TIMEOUT,
            autohide_dwell_threshold: AUTOHIDE_DWELL_THRESHOLD,
            autohide_dwell: 0.0,
            dock_interacted: false,
            dismiss_latched: false,
            dock_obscured: false,
            collapse_pending: false,
            space_use: SpaceUse::Available,
            position: DockPosition::Bottom,
            hidden_trigger_armed: true,
            last_cursor: None,
            last_display: None,
            motion,
            sizes: HashMap::new(),
            autohide_reveal: reveal_val,
            settled_drain_frames: 0,
            autohide_stepped_in_prepass: false,
            was_autohide_animating: false,
            anim_active: false,
            prev_down: false,
            app_menu: AppMenu::new("tessera-dock-context-menu", true),
            menu_tile: None,
            hovered_tile: None,
            hover_elapsed: 0.0,
            tooltip_tile: None,
            tooltip_alpha: 0.0,
            live_preview: None,
            hover_surface_bounds: None,
            hover_owner_bounds: None,
            hovered_preview: None,
            reduced_motion: false,
            tile_cache: RefCell::new(TileCache {
                signature: None,
                label: String::new(),
                window_order: Vec::new(),
                tiles: Vec::new(),
            }),
            all_windows: Vec::new(),
            icons: IconSet::default(),
            scale: 1.0,
            design: Design::dark(),
        }
    }

    /// Set whether the dock automatically hides after an inactivity period.
    pub fn set_autohide(&mut self, autohide: bool) {
        self.autohide = autohide;
        if autohide {
            self.autohide_reveal = 0.0;
            self.motion.reveal = SpringState::new(0.0);
            self.motion.settled_drain_frames = 0;
            self.settled_drain_frames = 0;
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = true;
            self.dismiss_latched = false;
        } else if self.space_use == SpaceUse::Available
            && !self.dock_obscured
            && !self.fullscreen_locked()
        {
            self.autohide_reveal = 1.0;
            self.motion.reveal = SpringState::new(1.0);
            self.motion.settled_drain_frames = 0;
            self.settled_drain_frames = 0;
            self.autohide_idle = 0.0;
            self.autohide_dwell = 0.0;
            self.dock_interacted = false;
            self.dismiss_latched = false;
            self.hidden_trigger_armed = true;
        }
    }

    /// Set the inactivity timeout in seconds before the dock autohides.
    pub fn set_autohide_timeout(&mut self, timeout_secs: f32) {
        self.autohide_timeout = timeout_secs.max(0.1);
    }

    /// Set the continuous hover dwell threshold in seconds required before expanding.
    pub fn set_autohide_dwell(&mut self, dwell_secs: f32) {
        self.autohide_dwell_threshold = dwell_secs.clamp(0.01, 1.0);
    }

    /// Toggle autohide mode.
    pub fn toggle_autohide(&mut self) {
        let current = self.autohide;
        self.set_autohide(!current);
    }

    /// Move the panel to a different screen edge.
    pub fn set_position(&mut self, position: DockPosition) {
        if position == self.position {
            return;
        }
        self.position = position;
        self.app_menu.set_side(Self::popup_side_for(position));
        self.anim_active = true;
    }

    /// Update window layout space utilization (Available, Maximized, Fullscreen).
    pub fn set_space_use(&mut self, space_use: SpaceUse) {
        if space_use == self.space_use {
            return;
        }
        let previous = self.space_use;
        self.space_use = space_use;
        if space_use == SpaceUse::Maximized && previous != SpaceUse::Maximized {
            self.dismiss_transient_ui();
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = true;
            self.dismiss_latched = true;
            self.anim_active = true;
        } else if space_use == SpaceUse::Available
            && previous != SpaceUse::Available
            && !self.dock_obscured
        {
            self.collapse_pending = false;
            self.dismiss_latched = false;
            self.anim_active = true;
            if !self.autohide {
                self.autohide_idle = 0.0;
                self.hidden_trigger_armed = true;
            }
        }
    }

    /// Update window obscuration status.
    pub fn set_dock_obscured(&mut self, obscured: bool) {
        if obscured == self.dock_obscured {
            return;
        }
        self.dock_obscured = obscured;
        self.anim_active = true;
        if obscured {
            self.dismiss_transient_ui();
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = true;
            self.dismiss_latched = true;
        } else {
            self.collapse_pending = false;
            self.dismiss_latched = false;
            if !self.effective_autohide() && !self.fullscreen_locked() {
                self.autohide_idle = 0.0;
                self.hidden_trigger_armed = true;
            }
        }
    }

    /// Request explicit dock dismissal (Escape or click outside).
    pub fn request_dismiss(&mut self) {
        self.dismiss_latched = true;
        self.autohide_idle = self.autohide_timeout;
        self.autohide_dwell = 0.0;
        self.press = None;
    }

    /// Whether the dock behaves as autohiding (setting, maximized, or obscured).
    #[inline]
    #[must_use]
    pub fn effective_autohide(&self) -> bool {
        self.autohide || self.space_use == SpaceUse::Maximized || self.dock_obscured
    }

    /// Fullscreen owns the complete output: exposes neither Dock chrome nor a reveal target.
    #[inline]
    #[must_use]
    pub fn fullscreen_locked(&self) -> bool {
        self.space_use == SpaceUse::Fullscreen
    }

    /// The side of a tile its context menu, tooltip, and previews open toward.
    #[must_use]
    pub fn popup_side_for(position: DockPosition) -> PopupSide {
        match position {
            DockPosition::Bottom => PopupSide::Above,
            DockPosition::Left => PopupSide::Right,
            DockPosition::Right => PopupSide::Left,
        }
    }

    /// Associated helpers for compatibility with tests.
    #[inline]
    #[must_use]
    pub fn magnify_factor(dx: f32) -> f32 {
        motion::magnify_factor(dx)
    }

    #[inline]
    #[must_use]
    pub fn smoothstep(progress: f32) -> f32 {
        motion::smoothstep_progress(progress)
    }

    #[inline]
    #[must_use]
    pub fn collapse_surface_progress(reveal: f32) -> f32 {
        motion::collapse_surface_progress(reveal)
    }

    #[inline]
    #[must_use]
    pub fn collapse_content_progress(reveal: f32) -> f32 {
        motion::collapse_content_progress(reveal)
    }

    #[inline]
    #[must_use]
    pub fn rest_bounds(
        tile_count: usize,
        pinned_count: usize,
        position: DockPosition,
        display: (f32, f32),
    ) -> Rect {
        DockState::rest_bounds(tile_count, pinned_count, position, display)
    }

    #[inline]
    #[must_use]
    pub fn collapsed_panel_rect(
        position: DockPosition,
        display: (f32, f32),
        expanded_len: f32,
        reveal: f32,
    ) -> Rect {
        DockState::collapsed_panel_rect(position, display, expanded_len, reveal)
    }

    #[inline]
    #[must_use]
    pub fn collapsed_indicator_bounds(position: DockPosition, display: (f32, f32)) -> Rect {
        DockState::collapsed_indicator_bounds(position, display)
    }

    #[inline]
    #[must_use]
    pub fn collapsed_indicator_contains(
        position: DockPosition,
        cursor: (f32, f32),
        display: (f32, f32),
    ) -> bool {
        DockState::collapsed_indicator_contains(position, cursor, display)
    }

    #[inline]
    #[must_use]
    pub fn expanded_trigger_contains(
        position: DockPosition,
        cursor: (f32, f32),
        rest_bounds: Rect,
        display: (f32, f32),
    ) -> bool {
        DockState::expanded_trigger_contains(position, cursor, rest_bounds, display)
    }

    #[inline]
    #[must_use]
    pub fn live_panel_contains(
        position: DockPosition,
        cursor: (f32, f32),
        current_panel: Rect,
        display: (f32, f32),
    ) -> bool {
        DockState::live_panel_contains(position, cursor, current_panel, display)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    #[must_use]
    pub fn pointer_keeps_revealed(
        effective_autohide: bool,
        reveal: f32,
        capsule_entry: bool,
        cursor: (f32, f32),
        current_panel: Rect,
        rest_bounds: Rect,
        position: DockPosition,
        display: (f32, f32),
    ) -> bool {
        DockState::default().pointer_keeps_revealed(
            effective_autohide,
            reveal,
            capsule_entry,
            cursor,
            current_panel,
            rest_bounds,
            position,
            display,
        )
    }

    #[inline]
    pub fn step_hidden_reveal(
        position: DockPosition,
        armed: &mut bool,
        dwell: &mut f32,
        threshold: f32,
        cursor: (f32, f32),
        display: (f32, f32),
        dt: f32,
    ) -> bool {
        DockState::step_hidden_reveal(position, armed, dwell, threshold, cursor, display, dt)
    }

    #[inline]
    #[must_use]
    pub fn window_overlaps_bounds(window: &Window, bounds: Rect) -> bool {
        DockState::window_overlaps_bounds(window, bounds)
    }

    #[inline]
    #[must_use]
    pub fn rest_centre_estimate(i: usize, n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        DockState::rest_centre_estimate(i, n, pinned_count, axis_len)
    }

    #[inline]
    #[must_use]
    pub fn section_extra(pinned_count: usize, n: usize) -> f32 {
        DockState::section_extra(pinned_count, n)
    }

    #[inline]
    #[must_use]
    pub fn panel_rect_for(position: DockPosition, bar_len: f32, display: (f32, f32)) -> Rect {
        DockState::panel_rect_for(position, bar_len, display)
    }

    #[inline]
    pub fn move_element<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
        DockState::move_element(items, from, to)
    }

    #[inline]
    #[must_use]
    pub fn drag_threshold_exceeded(origin: (f32, f32), cursor: (f32, f32)) -> bool {
        DockState::drag_threshold_exceeded(origin, cursor)
    }

    #[inline]
    #[must_use]
    pub fn drop_insert_index(pinned_centres: &[f32], cursor_axis: f32) -> usize {
        DockState::drop_insert_index(pinned_centres, cursor_axis)
    }

    #[inline]
    #[must_use]
    pub fn edge_drag_target(cursor: (f32, f32), display: (f32, f32)) -> Option<DockPosition> {
        DockState::edge_drag_target(cursor, display)
    }

    #[inline]
    pub fn spring(state: &mut SpringState, target: f32, dt: f32, reduced_motion: bool) -> f32 {
        DockMotion::advance_tile_spring(state, target, dt, reduced_motion)
    }

    /// Strip section under cursor: pinned or transient.
    pub(crate) fn drop_section_at(
        cursor_axis: f32,
        n: usize,
        pinned_count: usize,
        axis_len: f32,
    ) -> DropSection {
        let divider_centre = Self::section_boundary_axis(n, pinned_count, axis_len);
        if cursor_axis < divider_centre {
            DropSection::Pinned
        } else {
            DropSection::Transient
        }
    }

    /// Strip-axis position of section divider under resting geometry.
    pub(crate) fn section_boundary_axis(n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        Self::rest_centre_estimate(pinned_count - 1, n, pinned_count, axis_len)
            + DOCK_TILE * 0.5
            + DOCK_SECTION_GAP * 0.5
    }

    /// Magnification axis coordinate along the strip's long axis.
    pub(crate) fn magnification_axis(
        over_hover_surface: bool,
        over_rest_bounds: bool,
        owner_rest_centre: Option<f32>,
        cursor_axis: f32,
    ) -> f32 {
        if over_hover_surface && !over_rest_bounds {
            owner_rest_centre.unwrap_or(cursor_axis)
        } else {
            cursor_axis
        }
    }

    /// Magnification target size for a tile given its centre and active axis.
    pub(crate) fn magnification_target(in_band: bool, magnify_axis: f32, tile_centre: f32) -> f32 {
        let factor = if in_band {
            Self::magnify_factor(magnify_axis - tile_centre)
        } else {
            0.0
        };
        DOCK_TILE + (DOCK_TILE_MAX - DOCK_TILE) * factor
    }

    /// Stable pointer hit bounds of the resting dock.
    #[must_use]
    pub fn pointer_bounds(&self, display: (f32, f32)) -> Rect {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        Self::rest_bounds(tiles.len(), pinned_count, self.position, display)
    }

    /// Bounds of the animated panel material.
    #[must_use]
    pub fn visual_panel_bounds(&self, display: (f32, f32)) -> Rect {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let widths = tiles.iter().map(|tile| {
            self.sizes
                .get(&tile.key)
                .map_or(DOCK_TILE, |state| state.value.max(DOCK_TILE))
        });
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        let gaps = tiles.len().saturating_sub(1) as f32 * DOCK_TILE_GAP
            + Self::section_extra(pinned_count, tiles.len());
        let bar_len = widths.sum::<f32>() + gaps + 2.0 * DOCK_PAD;
        Self::panel_rect_for(self.position, bar_len, display)
    }

    /// Animation-stable capture footprint of the panel's glass body.
    #[must_use]
    pub fn capture_footprint(&self, display: (f32, f32)) -> Rect {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        let gaps = tiles.len().saturating_sub(1) as f32 * DOCK_TILE_GAP
            + Self::section_extra(pinned_count, tiles.len());
        let max_len =
            tiles.len() as f32 * (DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN) + gaps + 2.0 * DOCK_PAD;
        Self::panel_rect_for(self.position, max_len.max(AUTOHIDE_HANDLE_WIDTH), display)
    }

    /// Whether a visible window overlaps the resting dock rectangle.
    #[must_use]
    pub fn obscured_by_windows(&self, windows: &[Window], display: (f32, f32)) -> bool {
        let bounds = self.pointer_bounds(display);
        windows
            .iter()
            .any(|window| Self::window_overlaps_bounds(window, bounds))
    }

    /// Retain workspace windows.
    pub fn update_windows(&mut self, windows: &[Window]) {
        let space_use = SpaceUse::from_windows(windows);
        self.set_space_use(space_use);
        if space_use == SpaceUse::Fullscreen {
            self.dismiss_transient_ui();
            self.autohide_reveal = 0.0;
            self.motion.reveal = SpringState::new(0.0);
            self.motion.settled_drain_frames = 0;
            self.settled_drain_frames = 0;
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = false;
            self.anim_active = false;
            return;
        }

        if let Some(display) = self.last_display {
            let obscured = self.obscured_by_windows(windows, display);
            self.set_dock_obscured(obscured);
        }
    }

    /// Retain all windows across all workspaces.
    pub fn update_all_windows(&mut self, windows: &[Window]) {
        if self.live_preview.as_ref().is_some_and(|presentation| {
            presentation
                .cards
                .iter()
                .any(|card| !windows.iter().any(|window| window.id == card.window))
        }) {
            self.dismiss_hover_surface();
        }
        self.all_windows = windows.to_vec();
        if !self.app_menu.is_open() {
            self.menu_tile = None;
        }
    }

    /// Dismiss popups, preview panels, and tooltips.
    pub fn dismiss_transient_ui(&mut self) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.press = None;
        self.hovered_tile = None;
        self.hover_elapsed = 0.0;
        self.tooltip_tile = None;
        self.tooltip_alpha = 0.0;
        self.dismiss_hover_surface();
    }

    /// Dismiss active preview popover.
    pub fn dismiss_hover_surface(&mut self) {
        self.live_preview = None;
        self.hover_surface_bounds = None;
        self.hover_owner_bounds = None;
        self.hovered_preview = None;
    }

    /// Check if cursor is over preview surface or bridge.
    pub fn hover_surface_contains(&self, x: f32, y: f32) -> bool {
        let Some(surface) = self.hover_surface_bounds else {
            return false;
        };
        let contains = |rect: Rect| {
            x >= rect.x && x <= rect.x + rect.w && y >= rect.y && y <= rect.y + rect.h
        };
        if contains(surface) {
            return true;
        }
        let Some(owner) = self.hover_owner_bounds else {
            return false;
        };
        if contains(owner) {
            return true;
        }
        gap_bridge(surface, owner).is_some_and(contains)
    }

    /// The current frame's tile strip.
    pub(crate) fn frame_tiles<'a>(
        tile_cache: &'a RefCell<TileCache>,
        apps: &[DockApp],
        all_apps: &[Entry],
        icons: &IconSet,
        catalog_revision: u64,
        windows: &[Window],
        application_label: Option<&str>,
    ) -> Ref<'a, Vec<Tile>> {
        let signature = Self::tile_signature(catalog_revision, windows);
        let stale = {
            let cache = tile_cache.borrow();
            cache.signature != Some(signature)
                || application_label.is_some_and(|label| label != cache.label)
        };
        if stale {
            let label = application_label
                .map(str::to_string)
                .unwrap_or_else(|| tile_cache.borrow().label.clone());
            let mut cache = tile_cache.borrow_mut();
            cache
                .window_order
                .retain(|id| windows.iter().any(|window| window.id == *id));
            for window in windows {
                if !cache.window_order.contains(&window.id) {
                    cache.window_order.push(window.id);
                }
            }
            cache.tiles =
                Self::build_tiles(apps, all_apps, icons, windows, &cache.window_order, &label);
            cache.signature = Some(signature);
            cache.label = label;
        }
        Ref::map(tile_cache.borrow(), |cache| &cache.tiles)
    }

    fn tile_signature(catalog_revision: u64, windows: &[Window]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write_u64(catalog_revision);
        for w in windows {
            hasher.write_u64(w.id.0);
            match &w.app_id {
                Some(app_id) => {
                    hasher.write_u8(1);
                    hasher.write(app_id.as_bytes());
                }
                None => hasher.write_u8(0),
            }
            match &w.title {
                Some(title) => {
                    hasher.write_u8(1);
                    hasher.write(title.as_bytes());
                }
                None => hasher.write_u8(0),
            }
            hasher.write_u8(w.state.activated as u8);
            hasher.write_u8(w.read_only as u8);
        }
        hasher.finish()
    }

    fn build_tiles(
        apps: &[DockApp],
        all_apps: &[Entry],
        icons: &IconSet,
        windows: &[Window],
        window_order: &[tessera_desktop::window::WindowId],
        application_label: &str,
    ) -> Vec<Tile> {
        let win_appid: Vec<Option<String>> = windows
            .iter()
            .map(|w| w.app_id.as_ref().map(|a| a.to_ascii_lowercase()))
            .collect();
        let mut claimed = vec![false; windows.len()];
        let mut tiles = Vec::with_capacity(apps.len() + windows.len() + 1);
        tiles.push(Tile::launchpad(application_label));

        for (i, app) in apps.iter().enumerate() {
            let mut running = false;
            let mut activated = false;
            let mut focus = None;
            let mut window_ids = Vec::new();
            for (wi, w) in windows.iter().enumerate() {
                let Some(a) = &win_appid[wi] else { continue };
                if app.keys.iter().any(|k| k == a) {
                    claimed[wi] = true;
                    running = true;
                    if w.state.activated {
                        window_ids.insert(0, w.id);
                    } else {
                        window_ids.push(w.id);
                    }
                    if !w.read_only && w.state.activated {
                        activated = true;
                        focus = Some(w.id);
                    } else if !w.read_only && focus.is_none() {
                        focus = Some(w.id);
                    }
                }
            }
            let icon = app
                .keys
                .iter()
                .find_map(|k| icons.get(k))
                .or_else(|| icons.default_icon());
            tiles.push(Tile {
                key: format!("app:{}", app.entry.id),
                icon,
                running,
                activated,
                focus,
                windows: window_ids,
                app: Some(i),
                spawn: if running { None } else { Some(i) },
                label: app.entry.name.clone(),
                launchpad: false,
                pinned: true,
                pin_entry: None,
            });
        }

        let mut groups: Vec<TransientGroup> = Vec::new();
        for window_id in window_order {
            let Some((wi, w)) = windows
                .iter()
                .enumerate()
                .find(|(_, window)| window.id == *window_id)
            else {
                continue;
            };
            if claimed[wi] {
                continue;
            }
            let app_id = win_appid[wi].clone();
            let entry = app_id.as_ref().and_then(|a| {
                all_apps
                    .iter()
                    .position(|entry| rendering::entry_matches_app_id(entry, a))
            });
            let key = match (entry, &app_id) {
                (Some(i), _) => format!("transient:{}", all_apps[i].id),
                (None, Some(a)) => format!("transient:{a}"),
                (None, None) => format!("win:{}", w.id.0),
            };
            match groups.iter_mut().find(|group| group.key == key) {
                Some(group) => group.windows.push(wi),
                None => groups.push(TransientGroup {
                    key,
                    app_id,
                    entry,
                    windows: vec![wi],
                }),
            }
        }

        for group in groups {
            let mut activated = false;
            let mut focus = None;
            let mut window_ids = Vec::new();
            for &wi in &group.windows {
                let w = &windows[wi];
                if w.state.activated {
                    window_ids.insert(0, w.id);
                } else {
                    window_ids.push(w.id);
                }
                if !w.read_only && w.state.activated {
                    activated = true;
                    focus = Some(w.id);
                } else if !w.read_only && focus.is_none() {
                    focus = Some(w.id);
                }
            }
            let first = &windows[group.windows[0]];
            let icon = group
                .app_id
                .as_ref()
                .and_then(|a| icons.get(a))
                .or_else(|| icons.default_icon());
            let label = match group.entry {
                Some(i) => all_apps[i].name.clone(),
                None if group.windows.len() == 1 => first
                    .title
                    .clone()
                    .or_else(|| first.app_id.clone())
                    .unwrap_or_else(|| application_label.to_string()),
                None => first
                    .app_id
                    .clone()
                    .or_else(|| first.title.clone())
                    .unwrap_or_else(|| application_label.to_string()),
            };
            tiles.push(Tile {
                key: group.key,
                icon,
                running: true,
                activated,
                focus,
                windows: window_ids,
                app: None,
                spawn: None,
                label,
                launchpad: false,
                pinned: false,
                pin_entry: group.entry.map(|i| all_apps[i].id.clone()),
            });
        }
        tiles
    }

    /// The strip without the leading Launchpad tile — the view the unit tests
    /// assert against.
    #[cfg(test)]
    pub(crate) fn tiles(&self, windows: &[Window]) -> Vec<Tile> {
        Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            windows,
            None,
        )
        .iter()
        .skip(1)
        .cloned()
        .collect()
    }

    /// Minimization targets in output coordinates.
    pub fn minimize_targets(
        &self,
        display: (f32, f32),
    ) -> Vec<(tessera_desktop::window::WindowId, tessera_primitives::Rect)> {
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            None,
        );
        let n = tiles.len();
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        let axis_len = if self.position.is_vertical() {
            display.1
        } else {
            display.0
        };
        let mut out = Vec::new();
        for (i, tile) in tiles.iter().enumerate() {
            if tile.launchpad || tile.windows.is_empty() {
                continue;
            }
            let centre = Self::rest_centre_estimate(i, n, pinned_count, axis_len);
            let rect = self.minimize_target(centre, display);
            for &window in &tile.windows {
                out.push((window, rect));
            }
        }
        out
    }

    fn minimize_target(&self, centre: f32, display: (f32, f32)) -> tessera_primitives::Rect {
        let s = DOCK_TILE;
        let rect = match self.position {
            DockPosition::Bottom => Rect {
                x: centre - s * 0.5,
                y: display.1 - DOCK_EDGE_MARGIN - DOCK_BASELINE_INSET - s,
                w: s,
                h: s,
            },
            DockPosition::Left => Rect {
                x: DOCK_EDGE_MARGIN + DOCK_BASELINE_INSET,
                y: centre - s * 0.5,
                w: s,
                h: s,
            },
            DockPosition::Right => Rect {
                x: display.0 - DOCK_EDGE_MARGIN - DOCK_BASELINE_INSET - s,
                y: centre - s * 0.5,
                w: s,
                h: s,
            },
        };
        tessera_primitives::Rect::new(
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.w.round() as i32,
            rect.h.round() as i32,
        )
    }

    /// Update catalog with newly pushed entries.
    pub fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.dismiss_hover_surface();
        self.apps = catalog
            .pinned
            .iter()
            .map(|entry| DockApp {
                keys: entry.match_keys(),
                entry: entry.clone(),
            })
            .collect();
        self.all_apps = catalog.apps.clone();
        self.icons = catalog.icons.clone();
        self.pending_order = None;
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
        if !self
            .press
            .as_ref()
            .is_some_and(|press| press.dragging && matches!(press.target, PressTarget::Panel))
        {
            self.set_position(catalog.position);
        }
    }
}

/// The rectangular air gap between two rects separated along one axis.
fn gap_bridge(a: Rect, b: Rect) -> Option<Rect> {
    let horizontal_span = |x0: f32, x1: f32, y: f32, h: f32| Rect {
        x: x0,
        y,
        w: x1 - x0,
        h,
    };
    let min_x = a.x.min(b.x);
    let max_x = (a.x + a.w).max(b.x + b.w);
    let min_y = a.y.min(b.y);
    let max_y = (a.y + a.h).max(b.y + b.h);

    if a.y + a.h <= b.y {
        Some(horizontal_span(min_x, max_x, a.y + a.h, b.y - (a.y + a.h)))
    } else if b.y + b.h <= a.y {
        Some(horizontal_span(min_x, max_x, b.y + b.h, a.y - (b.y + b.h)))
    } else if a.x + a.w <= b.x {
        Some(Rect {
            x: a.x + a.w,
            y: min_y,
            w: b.x - (a.x + a.w),
            h: max_y - min_y,
        })
    } else if b.x + b.w <= a.x {
        Some(Rect {
            x: b.x + b.w,
            y: min_y,
            w: a.x - (b.x + b.w),
            h: max_y - min_y,
        })
    } else {
        None
    }
}
