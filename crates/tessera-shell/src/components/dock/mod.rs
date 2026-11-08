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
//! - [`rendering`]: Pure presentation projection onto Lens Frame, painted fills, and elevation.

pub mod bento;
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
pub(crate) use tessera_design::{Design, SurfaceRole, materials};

pub(crate) use crate::component::{
    AppCatalog, AppMenu, Chrome, ChromeEvents, ChromeUpdate, CursorShape, IconSet,
    LivePreviewPresentation, Localizer, Message, PinAction, PopupSide, PreviewCard, Reserved,
    ellipsize, preview,
};
pub(crate) use tessera_desktop::app::Entry;
pub(crate) use tessera_desktop::dock::DockPosition;
pub(crate) use tessera_desktop::system::SystemStatus;
pub(crate) use tessera_desktop::window::{SpaceUse, Window};
pub(crate) use tessera_desktop::workspace::WorkspaceSnapshot;
pub(crate) use tessera_primitives::input::{KeyAction, KeyChar, key_action};

pub use bento::{
    BentoColumn, BentoPager, COLUMN_UNITS, ColumnPiece, Gauge, MAX_ROWS, MIN_ROW_UNITS, Reading,
    ReadingTone, StripCell,
};
pub use motion::{
    AUTOHIDE_CONTENT_DRAIN_END, AUTOHIDE_CONTENT_INTERACTION_MIN, AUTOHIDE_DWELL_THRESHOLD,
    AUTOHIDE_HANDLE_APPROACH_BAND, AUTOHIDE_HANDLE_EDGE_MARGIN, AUTOHIDE_HANDLE_HEIGHT,
    AUTOHIDE_HANDLE_WIDTH, AUTOHIDE_IDLE_TIMEOUT, AUTOHIDE_QUICK_DISMISS_TIMEOUT,
    DOCK_BASELINE_INSET, DOCK_DOT, DOCK_DOT_STADIUM, DOCK_EDGE_MARGIN, DOCK_PAD, DOCK_PANEL_HEIGHT,
    DOCK_REVEAL_SPRING, DOCK_SECTION_GAP, DOCK_SPRING, DOCK_TILE, DOCK_TILE_BIRTH, DOCK_TILE_GAP,
    DOCK_TILE_MAX, DRAG_LIFT_SCALE, DRAG_THRESHOLD, DockMotion, EDGE_DRAG_PROXIMITY,
    HIT_HYSTERESIS_PX, MAGNIFY_RADIUS_TILES, PREVIEW_ASPECT, PREVIEW_CARD_GAP,
    PREVIEW_CARD_MAX_WIDTH, PREVIEW_CARD_MIN_WIDTH, PREVIEW_LABEL_HEIGHT, PREVIEW_PANEL_GAP,
    PREVIEW_PANEL_PAD, PREVIEW_SCREEN_MARGIN, REVEAL_SETTLE_VALUE_EPS, REVEAL_SETTLE_VELOCITY_EPS,
    SETTLED_DRAIN_FRAMES, SPRING_DAMPING, SPRING_OVERSHOOT_MARGIN, SPRING_STIFFNESS, TOOLTIP_BAND,
    TOOLTIP_DWELL, TOOLTIP_FADE_SPEED, TOOLTIP_GAP, TOOLTIP_HEIGHT, collapsed_reveal_depth,
    detect_cursor_retreat, edge_margin_for_surface_progress,
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
    /// How many workspaces the output the dock is drawn on has.
    ///
    /// The pager's width depends on it, so it participates in the strip length
    /// and therefore in the bar's resting bounds. Refreshed from the workspace
    /// snapshot each frame the dock paints.
    pub workspace_tick_count: usize,
    /// Index of the workspace that output is currently showing.
    pub workspace_current: usize,
    /// Stable identifiers of the workspaces observed from the output snapshot.
    pub workspace_ids: Vec<tessera_desktop::workspace::WorkspaceId>,
    /// Currently hovered workspace tick index, if pointer is over the workspace rail.
    pub hovered_workspace_tick: Option<usize>,
    /// The latest host system snapshot the reading column resolves from.
    ///
    /// The column is a *reading*, not a control: it draws whatever the host last
    /// reported and never infers a state from chrome's own behaviour. Retained
    /// whole rather than distilled into a resolved column so a re-resolve (the
    /// strip length, the paint, and the hit test each need it) cannot disagree
    /// with a snapshot that arrived between two of them.
    pub system_status: SystemStatus,
    /// Row index under the pointer inside the reading column, if any.
    pub hovered_reading_row: Option<usize>,
    /// Whether collapsed indicator entry is armed.
    pub hidden_trigger_armed: bool,
    /// Previous frame's cursor position.
    pub last_cursor: Option<(f32, f32)>,
    /// Most recently rendered logical display size.
    pub last_display: Option<(f32, f32)>,
    /// Motion dynamics state (Optics `transit` reveal spring and drain ring).
    ///
    /// The single owner of every value that advances with `dt`: the reveal
    /// spring, the swapchain drain counter, and the bubble's fade. `Dock` reads
    /// them through accessors rather than mirroring them.
    pub motion: DockMotion,
    /// Per-tile size springs keyed by tile key.
    pub sizes: HashMap<String, SpringState>,
    /// Whether autohide was advanced during `prepare_frame` for the current frame.
    pub autohide_stepped_in_prepass: bool,
    /// Whether tile magnification springs were advanced during `prepare_frame` for the current frame.
    pub sizes_stepped_in_prepass: bool,
    /// Whether autohide was moving on the previous frame.
    pub was_autohide_animating: bool,
    /// Whether the tile magnification wave was moving on the previous frame.
    pub was_magnific_animating: bool,
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
    pub live_preview: Option<LivePreviewPresentation>,
    pub hover_surface_bounds: Option<Rect>,
    pub hover_owner_bounds: Option<Rect>,
    pub hovered_preview: Option<tessera_desktop::window::WindowId>,
    /// Retained damage footprint for active or recently dismissed hover surfaces (tooltip / preview).
    pub hover_damage_rect: Option<tessera_primitives::Rect>,
    /// Damage footprint of the tile context menu: the popup rect plus the band it
    /// vacated on a previous frame. Repainted until the menu closes so a moving
    /// anchor never strands pixels.
    pub menu_damage_rect: Option<tessera_primitives::Rect>,
    /// Swapchain ring drain frames remaining after a hover surface closes or moves.
    pub hover_drain_frames: u8,
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
    /// Current reveal progress in `[0.0, 1.0]`, owned by [`DockMotion`].
    ///
    /// This used to be a second `f32` field on `Dock`, resynchronised from the
    /// spring once per `prepare_frame`. Two copies of one fact need a
    /// reconciliation step, and a reconciliation step is a place for the two to
    /// disagree — the geometry would then be derived from a value the spring had
    /// already moved past. The spring is the only owner; this reads it.
    #[must_use]
    pub fn autohide_reveal(&self) -> f32 {
        self.motion.reveal_value()
    }

    /// Create a new Dock with default configuration.
    #[must_use]
    pub fn new() -> Self {
        let motion = DockMotion::new(false);
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
            workspace_tick_count: 1,
            workspace_current: 0,
            workspace_ids: Vec::new(),
            hovered_workspace_tick: None,
            system_status: SystemStatus::default(),
            hovered_reading_row: None,
            hidden_trigger_armed: true,
            last_cursor: None,
            last_display: None,
            motion,
            sizes: HashMap::new(),
            autohide_stepped_in_prepass: false,
            sizes_stepped_in_prepass: false,
            was_autohide_animating: false,
            was_magnific_animating: false,
            anim_active: false,
            prev_down: false,
            app_menu: AppMenu::new("tessera-dock-context-menu", true),
            menu_tile: None,
            hovered_tile: None,
            hover_elapsed: 0.0,
            tooltip_tile: None,
            live_preview: None,
            hover_surface_bounds: None,
            hover_owner_bounds: None,
            hovered_preview: None,
            hover_damage_rect: None,
            menu_damage_rect: None,
            hover_drain_frames: 0,
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
            self.motion.reveal = SpringState::new(0.0);
            self.motion.settled_drain_frames = 0;
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = true;
            self.dismiss_latched = false;
        } else if self.space_use == SpaceUse::Available
            && !self.dock_obscured
            && !self.fullscreen_locked()
        {
            self.motion.reveal = SpringState::new(1.0);
            self.motion.settled_drain_frames = 0;
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

    /// The dock's leading (non-icon) cells, in strip order.
    ///
    /// The machine's own readings lead, then the workspace pager, then the
    /// icons (ADR-0218). The order is semantic rather than decorative: the
    /// reading column is a plate the user presses to reach the machine, the
    /// pager is navigation, and the icons are targets.
    ///
    /// Both cells auto-hide with zero footprint when they have nothing to say —
    /// a machine that reports no radios and no battery resolves an empty column,
    /// and a single workspace resolves an empty pager — so a bare dock stays
    /// pure and centered instead of carrying a placeholder.
    #[must_use]
    pub fn strip_cells(&self) -> Vec<StripCell> {
        let mut cells = Vec::with_capacity(2);
        let readings = BentoColumn::resolve(&self.system_status);
        if !readings.is_empty() {
            cells.push(StripCell::Readings(readings));
        }
        if self.workspace_tick_count > 1 {
            cells.push(StripCell::WorkspacePager(BentoPager::new(
                self.workspace_tick_count,
            )));
        }
        cells
    }

    /// The reading column, resolved from the latest status snapshot.
    #[must_use]
    pub fn readings(&self) -> BentoColumn {
        BentoColumn::resolve(&self.system_status)
    }

    /// Fold this frame's workspace snapshot into the pager's readings.
    ///
    /// The dock paints whichever output the presentation domain draws on, and
    /// the snapshot is keyed by output; taking the first output keeps the pager
    /// honest on a multi-output session instead of inventing a global count.
    pub fn observe_workspaces(&mut self, workspaces: &WorkspaceSnapshot) {
        let Some(output) = workspaces.outputs.first() else {
            self.workspace_tick_count = 1;
            self.workspace_current = 0;
            self.workspace_ids.clear();
            return;
        };
        self.workspace_tick_count = output.workspaces.len().max(1);
        self.workspace_current = output
            .current
            .and_then(|current| output.workspaces.iter().position(|ws| ws.id == current))
            .unwrap_or(0);
        self.workspace_ids = output.workspaces.iter().map(|ws| ws.id).collect();
    }

    /// The resting bar for `tile_count` icons with no Bento cells.
    ///
    /// Kept for callers that reason about the icon strip alone (geometry tests,
    /// the damage envelope). The live layout goes through
    /// `Dock::rest_bounds_with_cells` so the pager's width is included.
    pub fn rest_bounds(
        tile_count: usize,
        pinned_count: usize,
        position: DockPosition,
        display: (f32, f32),
    ) -> Rect {
        DockState::rest_bounds_with_cells(tile_count, pinned_count, &[], position, display)
    }

    /// The resting bar including this dock's Bento cells.
    #[must_use]
    pub fn live_rest_bounds(
        &self,
        tile_count: usize,
        pinned_count: usize,
        display: (f32, f32),
    ) -> Rect {
        DockState::rest_bounds_with_cells(
            tile_count,
            pinned_count,
            &self.strip_cells(),
            self.position,
            display,
        )
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
    pub fn collapsed_reveal_corridor(position: DockPosition, display: (f32, f32)) -> Rect {
        DockState::collapsed_reveal_corridor(position, display)
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

    /// The resting centre of icon `i` on this dock's actual bar, which leads with
    /// its Bento reading section.
    ///
    /// Every live call site goes through this rather than the cells-free
    /// estimate: the magnification springs settle on these centres, so a bar
    /// length that forgets the reading section would park every icon slightly off
    /// its own paint.
    #[must_use]
    pub fn live_rest_centre(&self, i: usize, n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        DockState::rest_centre_estimate_with_cells(
            i,
            n,
            pinned_count,
            &self.strip_cells(),
            axis_len,
        )
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
        self.live_rest_bounds(tiles.len(), pinned_count, display)
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
        // One resolved length for the whole bar: the leading Bento reading
        // section, one section gap, then the icon strip at its current widths.
        // Deriving the visual extent from its own arithmetic is how the reading
        // section would silently go missing from the hit-test.
        let cells = self.strip_cells();
        let bar_len =
            crate::components::dock::bento::strip_length(widths, &cells, DOCK_TILE_GAP, DOCK_PAD);
        Self::panel_rect_for(self.position, bar_len, display)
    }

    /// Animation-stable damage envelope of the panel's painted body.
    ///
    /// One envelope must contain every body the morph can produce: the widest
    /// magnified strip *and* both edge offsets. The strip envelope stays
    /// anchored at the expanded bar's float ([`DOCK_EDGE_MARGIN`]), so the
    /// footprint extends toward the anchored edge until the collapsed capsule's
    /// tuck ([`AUTOHIDE_HANDLE_EDGE_MARGIN`]) is covered too. Deriving it from
    /// the two endpoints — never from the live reveal — keeps the compositor's
    /// capture and damage stable across the whole animation (ADR-0171,
    /// ADR-0180).
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
        // The envelope is the widest body the morph can produce, so it measures
        // the *maximum* tile width — not the resting one — and it includes the
        // leading reading section, whose width does not scale with the tile
        // count at all. Deriving it from the icons alone let a wide reading
        // section push the painted bar outside the region that gets repainted,
        // and those pixels then never updated.
        let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
        let max_widths = std::iter::repeat_n(DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN, tiles.len());
        let max_len = crate::components::dock::bento::strip_length(
            max_widths,
            &self.strip_cells(),
            DOCK_TILE_GAP,
            DOCK_PAD,
        );
        let panel =
            Self::panel_rect_for(self.position, max_len.max(AUTOHIDE_HANDLE_WIDTH), display);
        let _ = pinned_count;
        // Both states hold `DOCK_EDGE_MARGIN` (ADR-0213 unification), so the
        // capsule is not tucked closer to the edge than the bar and there is no
        // extra band to grow. The expression stays because it is the invariant
        // that matters: the envelope covers whichever endpoint reaches furthest.
        let tuck = (DOCK_EDGE_MARGIN - AUTOHIDE_HANDLE_EDGE_MARGIN).max(0.0);
        match self.position {
            DockPosition::Bottom => Rect {
                h: panel.h + tuck,
                ..panel
            },
            DockPosition::Left => Rect {
                x: panel.x - tuck,
                w: panel.w + tuck,
                ..panel
            },
            DockPosition::Right => Rect {
                w: panel.w + tuck,
                ..panel
            },
        }
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
        let Some(display) = self.last_display else {
            return;
        };
        // Obscuration is geometric and independent of who owns the output, so it
        // is recomputed on every window snapshot. While a client owns the output
        // the Dock is locked out and the flag is *cleared* rather than frozen: a
        // stale `true` would survive the ownership release and suppress the
        // restore path in `set_space_use`.
        let obscured = !self.fullscreen_locked() && self.obscured_by_windows(windows, display);
        self.set_dock_obscured(obscured);
    }

    /// Adopt the server-resolved space use (ADR-0183).
    ///
    /// The Dock never infers this from the window snapshot: a window that
    /// covers the output without declaring `xdg_toplevel.set_fullscreen` owns
    /// the presentation just like one that did.
    pub fn apply_space_use(&mut self, space_use: SpaceUse) {
        self.set_space_use(space_use);
        if space_use == SpaceUse::Fullscreen {
            self.dismiss_transient_ui();
            self.motion.reveal = SpringState::new(0.0);
            self.motion.settled_drain_frames = 0;
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = false;
            self.anim_active = false;
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

    /// The application-name bubble's fade, advanced by [`DockMotion`].
    ///
    /// Read-only here on purpose: `DockMotion` is the single owner of the value
    /// that the fade integrates. A second copy on `Dock` is how the fade came to
    /// have two implementations that disagreed about whether a settled fade also
    /// clears `tooltip_tile`.
    #[must_use]
    pub fn tooltip_alpha(&self) -> f32 {
        self.motion.tooltip_alpha
    }

    /// Clear the bubble's fade. The only mutation callers need; every advance
    /// goes through [`DockMotion::advance_tooltip`].
    pub fn reset_tooltip_fade(&mut self) {
        self.motion.tooltip_alpha = 0.0;
    }

    /// Dismiss popups, preview panels, and tooltips.
    pub fn dismiss_transient_ui(&mut self) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.press = None;
        self.hovered_tile = None;
        self.hover_elapsed = 0.0;
        self.tooltip_tile = None;
        self.reset_tooltip_fade();
        self.dismiss_hover_surface();
    }

    /// Append a damage region, dropping degenerate ones and collapsing exact
    /// duplicates. Anchored bands can coincide (the tooltip band contains the
    /// strip), and a repeated region would make the host walk the same scissor
    /// twice.
    pub(crate) fn push_damage(regions: &mut Vec<lens::Rect>, x: f32, y: f32, w: f32, h: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        /* Outward on both edges, so the far edge is never short. */
        let x0 = x.floor();
        let y0 = y.floor();
        let rect = lens::Rect {
            x: x0,
            y: y0,
            w: (x + w).ceil() - x0,
            h: (y + h).ceil() - y0,
        };
        if !regions.contains(&rect) {
            regions.push(rect);
        }
    }

    /// Record the damage footprint of the dock's tile context menu.
    ///
    /// A popup pixel belongs to this component, so it must appear in
    /// [`Chrome::damage_regions`]. Omitting it left the menu outside the frame's
    /// render area: it only appeared on frames where an unrelated backdrop
    /// refresh happened to widen the damage, which is the "sometimes renders,
    /// sometimes flickers" popup behaviour.
    ///
    /// The union keeps a moving anchor repaintable: while a tile rides the
    /// magnification spring the menu travels with it, and the band it vacated has
    /// to be repainted or the previous frame's popup lingers behind the new one.
    pub(crate) fn record_menu_damage(
        menu_damage_rect: &mut Option<tessera_primitives::Rect>,
        bounds: Rect,
    ) {
        /* Outward on both edges: flooring the origin while ceiling only the
         * extent leaves the far edge short by the fractional part. */
        let x0 = bounds.x.floor();
        let y0 = bounds.y.floor();
        let x1 = (bounds.x + bounds.w).ceil();
        let y1 = (bounds.y + bounds.h).ceil();
        let padded = tessera_primitives::Rect::new(
            x0 as i32,
            y0 as i32,
            (x1 - x0).max(1.0) as i32,
            (y1 - y0).max(1.0) as i32,
        );
        *menu_damage_rect = Some(match *menu_damage_rect {
            Some(existing) => existing.union(padded),
            None => padded,
        });
    }

    /// Record damage footprint for a hover surface (tooltip or live preview)
    /// so all swapchain ring slots repaint its area when it moves or closes.
    pub(crate) fn record_hover_surface_damage(
        hover_damage_rect: &mut Option<tessera_primitives::Rect>,
        hover_drain_frames: &mut u8,
        bounds: Rect,
    ) {
        let pad = 48;
        let padded = tessera_primitives::Rect::new(
            (bounds.x.floor() as i32 - pad).max(0),
            (bounds.y.floor() as i32 - pad).max(0),
            bounds.w.ceil().max(1.0) as i32 + pad * 2,
            bounds.h.ceil().max(1.0) as i32 + pad * 2,
        );
        *hover_damage_rect = Some(match *hover_damage_rect {
            Some(existing) if *hover_drain_frames > 0 => existing.union(padded),
            _ => padded,
        });
        *hover_drain_frames = SETTLED_DRAIN_FRAMES;
    }

    /// Dismiss active preview popover.
    pub fn dismiss_hover_surface(&mut self) {
        if let Some(bounds) = self.hover_surface_bounds.take() {
            Self::record_hover_surface_damage(
                &mut self.hover_damage_rect,
                &mut self.hover_drain_frames,
                bounds,
            );
        }
        self.live_preview = None;
        self.hover_owner_bounds = None;
        self.hovered_preview = None;
    }

    /// Check if cursor is over preview surface or bridge.
    pub fn hover_surface_contains(&self, x: f32, y: f32) -> bool {
        let Some(surface) = self.hover_surface_bounds else {
            return false;
        };
        let contains =
            |rect: Rect| x >= rect.x && x <= rect.x + rect.w && y >= rect.y && y <= rect.y + rect.h;
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
            let centre = self.live_rest_centre(i, n, pinned_count, axis_len);
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
