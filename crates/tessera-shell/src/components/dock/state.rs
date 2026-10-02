//! Headless state machine for the macOS-style dock (ADR-0174).
//!
//! Owns pure functional state and transitions:
//! - Pinned applications, drag & drop reorder states, press tracking.
//! - Autohide intent dwell, idle timers, retreat vectors, and dismiss latching.
//! - Obscurity collision tracking with client windows and workspace mode.
//! - 100% headless testable with zero GPU or display dependencies.

use lens::Rect;
use tessera_desktop::app::Entry;
use tessera_desktop::dock::DockPosition;
use tessera_desktop::window::{SpaceUse, Window};

use super::motion::{
    AUTOHIDE_DWELL_THRESHOLD, AUTOHIDE_HANDLE_HEIGHT, AUTOHIDE_HANDLE_WIDTH, AUTOHIDE_IDLE_TIMEOUT,
    DOCK_EDGE_MARGIN, DOCK_PAD, DOCK_PANEL_HEIGHT, DOCK_SECTION_GAP, DOCK_TILE, DOCK_TILE_GAP,
    DRAG_THRESHOLD, EDGE_DRAG_PROXIMITY, collapse_surface_progress,
};

/// One application pinned to the dock: the launchable entry plus the lowercased
/// `app_id`s a running toplevel might report, used to fold a running window
/// into its pinned tile.
#[derive(Clone, Debug, PartialEq)]
pub struct DockApp {
    /// The entry spawned when the tile is clicked and no window matches.
    pub entry: Entry,
    /// Lowercased ids this app may run as (`StartupWMClass`, the desktop-id
    /// stem, the icon name). Matched against a window's `app_id`.
    pub keys: Vec<String>,
}

/// What a left-button press on the dock panel landed on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PressTarget {
    /// A pinned application tile (identified by its stable tile key).
    PinnedTile(String),
    /// A transient application tile or the Launchpad.
    OtherTile(String),
    /// Empty panel space (padding, gaps, or the collapsed autohide handle).
    Panel,
}

/// The strip section under a dragged tile: the kept (pinned) strip, or the
/// transient running section past the section divider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropSection {
    Pinned,
    Transient,
}

/// A held left-button press on the dock.
#[derive(Clone, Debug, PartialEq)]
pub struct PressState {
    /// Cursor position at the press; the drag-threshold reference.
    pub origin: (f32, f32),
    /// What the press landed on.
    pub target: PressTarget,
    /// Whether the press has been promoted to a drag.
    pub dragging: bool,
    /// The strip section the drag currently hovers.
    pub section: DropSection,
    /// The latest reorder insertion slot previewed during a tile drag.
    pub insert: Option<usize>,
    /// The dock position when the press started.
    pub start_position: DockPosition,
}

/// Headless state of the Dock component.
#[derive(Clone, Debug, PartialEq)]
pub struct DockState {
    /// Pinned launchable apps, in dock order.
    pub apps: Vec<DockApp>,
    /// The complete enumerated application catalog, refreshed with every rescan.
    pub all_apps: Vec<Entry>,
    /// Monotonically increasing revision bumped when app ordering changes.
    pub catalog_revision: u64,
    /// Pending reorder to commit through `ChromeEvents`.
    pub pending_order: Option<Vec<String>>,
    /// Active mouse press or drag state.
    pub press: Option<PressState>,
    /// Whether autohide mode is enabled by user configuration.
    pub autohide: bool,
    /// Inactivity timer tracking elapsed seconds since pointer left the dock area.
    pub autohide_idle: f32,
    /// Configurable inactivity timeout in seconds before an autohiding dock collapses.
    pub autohide_timeout: f32,
    /// Configurable dwell threshold in seconds before collapsed indicator begins expanding.
    pub autohide_dwell_threshold: f32,
    /// Continuous hover dwell time in seconds on the collapsed indicator before expanding.
    pub autohide_dwell: f32,
    /// Whether the user engaged with the Dock during the current reveal session.
    pub dock_interacted: bool,
    /// Anti-rebound latch: once an explicit dismiss is triggered (click-outside, Escape,
    /// or window obscuration), the dock is latched into collapsing and CANNOT be re-opened
    /// by dynamic panel hitboxes mid-transition. It only unlatches after reaching complete rest (0.0).
    pub dismiss_latched: bool,
    /// Whether a visible window intersects the Dock's resting footprint.
    pub dock_obscured: bool,
    /// Entering an obscured state must complete one uninterrupted collapse to 0.0.
    pub collapse_pending: bool,
    /// Compositor-derived window space use (Available, Maximized, Fullscreen).
    pub space_use: SpaceUse,
    /// The screen edge the dock is anchored to.
    pub position: DockPosition,
    /// Whether a pointer entry may reveal the collapsed Dock.
    pub hidden_trigger_armed: bool,
    /// Previous frame's cursor position used to detect retreat vectors.
    pub last_cursor: Option<(f32, f32)>,
    /// Most recently rendered logical output size.
    pub last_display: Option<(f32, f32)>,
}

impl Default for DockState {
    fn default() -> Self {
        Self::new()
    }
}

impl DockState {
    /// Create a new default headless Dock state.
    #[must_use]
    pub fn new() -> Self {
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
        }
    }

    /// Whether the dock behaves as autohiding (user setting, maximized, or obscured).
    #[inline]
    #[must_use]
    pub fn effective_autohide(&self) -> bool {
        self.autohide || self.space_use == SpaceUse::Maximized || self.dock_obscured
    }

    /// Set whether the dock automatically hides after an inactivity period.
    pub fn set_autohide(&mut self, enabled: bool) {
        if self.autohide == enabled {
            return;
        }
        self.autohide = enabled;
        if !enabled && self.space_use == SpaceUse::Available && !self.dock_obscured {
            self.autohide_idle = 0.0;
            self.hidden_trigger_armed = true;
            self.dismiss_latched = false;
        }
    }

    /// Set the inactivity timeout in seconds before the dock autohides.
    pub fn set_autohide_timeout(&mut self, timeout_secs: f32) {
        self.autohide_timeout = timeout_secs.max(0.1);
    }

    /// Set the dwell threshold in seconds before the collapsed indicator expands.
    pub fn set_autohide_dwell(&mut self, dwell_secs: f32) {
        self.autohide_dwell_threshold = dwell_secs.clamp(0.05, 1.0);
    }

    /// Set the dock position (Bottom, Left, Right).
    pub fn set_position(&mut self, position: DockPosition) {
        self.position = position;
    }

    /// Update window layout space utilization (Available, Maximized, Fullscreen).
    pub fn set_space_use(&mut self, space_use: SpaceUse) {
        if space_use == self.space_use {
            return;
        }
        let previous = self.space_use;
        self.space_use = space_use;
        if space_use == SpaceUse::Maximized && previous != SpaceUse::Maximized {
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = true;
            self.dismiss_latched = true;
        } else if space_use == SpaceUse::Available
            && previous != SpaceUse::Available
            && !self.dock_obscured
        {
            self.collapse_pending = false;
            self.dismiss_latched = false;
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
        if obscured {
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = true;
            self.dismiss_latched = true;
        } else {
            self.collapse_pending = false;
            self.dismiss_latched = false;
            if !self.effective_autohide() {
                self.autohide_idle = 0.0;
                self.hidden_trigger_armed = true;
            }
        }
    }

    /// Request explicit dock dismissal (e.g. click outside or Escape pressed).
    /// Latches dismissal until the dock completely settles at 0.0.
    pub fn request_dismiss(&mut self) {
        self.dismiss_latched = true;
        self.autohide_idle = self.autohide_timeout;
        self.autohide_dwell = 0.0;
        self.press = None;
    }

    /// Step intent dwell timer for the collapsed indicator.
    pub fn step_hidden_reveal(
        position: DockPosition,
        armed: &mut bool,
        dwell: &mut f32,
        threshold: f32,
        cursor: (f32, f32),
        display: (f32, f32),
        dt: f32,
    ) -> bool {
        if !Self::collapsed_indicator_contains(position, cursor, display) {
            *armed = true;
            *dwell = 0.0;
            return false;
        }
        if !*armed {
            *dwell = 0.0;
            return false;
        }
        *dwell += dt;
        if *dwell >= threshold {
            *armed = false;
            *dwell = 0.0;
            true
        } else {
            false
        }
    }

    /// Resolve whether the pointer position keeps the Dock revealed.
    ///
    /// If [`dismiss_latched`] is active, this unconditionally returns `false`
    /// to guarantee smooth, uninterrupted collapse without limit-cycle rebounding.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn pointer_keeps_revealed(
        &self,
        effective_autohide: bool,
        reveal: f32,
        capsule_entry: bool,
        cursor: (f32, f32),
        current_panel: Rect,
        rest_bounds: Rect,
        position: DockPosition,
        display: (f32, f32),
    ) -> bool {
        if self.dismiss_latched {
            return false;
        }
        if !effective_autohide {
            return cursor.0 >= rest_bounds.x
                && cursor.1 >= rest_bounds.y
                && cursor.0 < rest_bounds.x + rest_bounds.w
                && cursor.1 < rest_bounds.y + rest_bounds.h;
        }
        if reveal <= 0.001 {
            return capsule_entry;
        }
        if reveal < 0.999 {
            return capsule_entry
                || Self::collapsed_indicator_contains(position, cursor, display)
                || Self::live_panel_contains(position, cursor, current_panel, display);
        }
        Self::expanded_trigger_contains(position, cursor, rest_bounds, display)
    }

    /// Geometric bounding box for the collapsed panel handle.
    #[must_use]
    pub fn collapsed_panel_rect(
        position: DockPosition,
        display: (f32, f32),
        expanded_len: f32,
        reveal: f32,
    ) -> Rect {
        let progress = collapse_surface_progress(reveal);
        let len = AUTOHIDE_HANDLE_WIDTH + (expanded_len - AUTOHIDE_HANDLE_WIDTH) * progress;
        let thick =
            AUTOHIDE_HANDLE_HEIGHT + (DOCK_PANEL_HEIGHT - AUTOHIDE_HANDLE_HEIGHT) * progress;
        match position {
            DockPosition::Bottom => Rect {
                x: (display.0 - len) * 0.5,
                y: display.1 - DOCK_EDGE_MARGIN - thick,
                w: len,
                h: thick,
            },
            DockPosition::Left => Rect {
                x: DOCK_EDGE_MARGIN,
                y: (display.1 - len) * 0.5,
                w: thick,
                h: len,
            },
            DockPosition::Right => Rect {
                x: display.0 - DOCK_EDGE_MARGIN - thick,
                y: (display.1 - len) * 0.5,
                w: thick,
                h: len,
            },
        }
    }

    /// Bounding box of the collapsed stadium indicator at rest.
    #[inline]
    #[must_use]
    pub fn collapsed_indicator_bounds(position: DockPosition, display: (f32, f32)) -> Rect {
        Self::collapsed_panel_rect(position, display, AUTOHIDE_HANDLE_WIDTH, 0.0)
    }

    /// Whether `cursor` is inside the collapsed stadium indicator.
    #[must_use]
    pub fn collapsed_indicator_contains(
        position: DockPosition,
        cursor: (f32, f32),
        display: (f32, f32),
    ) -> bool {
        let indicator = Self::collapsed_indicator_bounds(position, display);
        match position {
            DockPosition::Bottom => {
                cursor.0 >= indicator.x
                    && cursor.0 < indicator.x + indicator.w
                    && cursor.1 >= indicator.y
                    && cursor.1 < display.1
            }
            DockPosition::Left => {
                cursor.0 >= 0.0
                    && cursor.0 < indicator.x + indicator.w
                    && cursor.1 >= indicator.y
                    && cursor.1 < indicator.y + indicator.h
            }
            DockPosition::Right => {
                cursor.0 >= indicator.x
                    && cursor.0 < display.0
                    && cursor.1 >= indicator.y
                    && cursor.1 < indicator.y + indicator.h
            }
        }
    }

    /// Whether cursor is inside the expanded dock's approach corridor.
    #[must_use]
    pub fn expanded_trigger_contains(
        position: DockPosition,
        cursor: (f32, f32),
        rest_bounds: Rect,
        display: (f32, f32),
    ) -> bool {
        match position {
            DockPosition::Bottom => {
                cursor.0 >= rest_bounds.x
                    && cursor.1 >= rest_bounds.y
                    && cursor.0 < rest_bounds.x + rest_bounds.w
                    && cursor.1 < display.1
            }
            DockPosition::Left => {
                cursor.0 >= 0.0
                    && cursor.1 >= rest_bounds.y
                    && cursor.0 < rest_bounds.x + rest_bounds.w
                    && cursor.1 < rest_bounds.y + rest_bounds.h
            }
            DockPosition::Right => {
                cursor.0 >= rest_bounds.x
                    && cursor.1 >= rest_bounds.y
                    && cursor.0 < display.0
                    && cursor.1 < rest_bounds.y + rest_bounds.h
            }
        }
    }

    /// Whether cursor is inside the live morphing panel and its anchored edge gap.
    #[must_use]
    pub fn live_panel_contains(
        position: DockPosition,
        cursor: (f32, f32),
        current_panel: Rect,
        display: (f32, f32),
    ) -> bool {
        match position {
            DockPosition::Bottom => {
                cursor.0 >= current_panel.x
                    && cursor.1 >= current_panel.y
                    && cursor.0 < current_panel.x + current_panel.w
                    && cursor.1 < display.1
            }
            DockPosition::Left => {
                cursor.0 >= 0.0
                    && cursor.0 < current_panel.x + current_panel.w
                    && cursor.1 >= current_panel.y
                    && cursor.1 < current_panel.y + current_panel.h
            }
            DockPosition::Right => {
                cursor.0 >= current_panel.x
                    && cursor.0 < display.0
                    && cursor.1 >= current_panel.y
                    && cursor.1 < current_panel.y + current_panel.h
            }
        }
    }

    /// Calculate the stable resting bounds of the dock bar.
    #[must_use]
    pub fn rest_bounds(
        tile_count: usize,
        pinned_count: usize,
        position: DockPosition,
        display: (f32, f32),
    ) -> Rect {
        let gaps = tile_count.saturating_sub(1) as f32 * DOCK_TILE_GAP
            + Self::section_extra(pinned_count, tile_count);
        let bar_len = tile_count as f32 * DOCK_TILE + gaps + 2.0 * DOCK_PAD;
        Self::panel_rect_for(position, bar_len, display)
    }

    /// Construct a panel rectangle centered along `position` edge.
    #[must_use]
    pub fn panel_rect_for(position: DockPosition, bar_len: f32, display: (f32, f32)) -> Rect {
        match position {
            DockPosition::Bottom => Rect {
                x: (display.0 - bar_len) * 0.5,
                y: display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN,
                w: bar_len,
                h: DOCK_PANEL_HEIGHT,
            },
            DockPosition::Left => Rect {
                x: DOCK_EDGE_MARGIN,
                y: (display.1 - bar_len) * 0.5,
                w: DOCK_PANEL_HEIGHT,
                h: bar_len,
            },
            DockPosition::Right => Rect {
                x: display.0 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN,
                y: (display.1 - bar_len) * 0.5,
                w: DOCK_PANEL_HEIGHT,
                h: bar_len,
            },
        }
    }

    /// Additional spacing allocated for the pinned/transient divider.
    #[inline]
    #[must_use]
    pub fn section_extra(pinned_count: usize, total_count: usize) -> f32 {
        if pinned_count < total_count {
            (DOCK_SECTION_GAP - DOCK_TILE_GAP).max(0.0)
        } else {
            0.0
        }
    }

    /// Check if cursor movement exceeded drag promotion threshold.
    #[must_use]
    pub fn drag_threshold_exceeded(origin: (f32, f32), cursor: (f32, f32)) -> bool {
        let dx = cursor.0 - origin.0;
        let dy = cursor.1 - origin.1;
        dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD
    }

    /// Drop insertion index in the pinned section along `cursor_axis`.
    #[must_use]
    pub fn drop_insert_index(pinned_centres: &[f32], cursor_axis: f32) -> usize {
        pinned_centres
            .iter()
            .filter(|centre| cursor_axis > **centre)
            .count()
    }

    /// Reorder helper to move an item from `from` to `insert`.
    pub fn move_element<T>(items: &mut Vec<T>, from: usize, insert: usize) -> bool {
        if from == insert || from >= items.len() || insert > items.len().saturating_sub(1) {
            return false;
        }
        let item = items.remove(from);
        items.insert(insert.min(items.len()), item);
        true
    }

    /// Edge drag target screen edge.
    #[must_use]
    pub fn edge_drag_target(cursor: (f32, f32), display: (f32, f32)) -> Option<DockPosition> {
        let d_left = cursor.0;
        let d_right = (display.0 - cursor.0).max(0.0);
        let d_bottom = (display.1 - cursor.1).max(0.0);
        let min = d_left.min(d_right).min(d_bottom);
        if min > EDGE_DRAG_PROXIMITY {
            return None;
        }
        if min == d_bottom {
            Some(DockPosition::Bottom)
        } else if min == d_left {
            Some(DockPosition::Left)
        } else {
            Some(DockPosition::Right)
        }
    }

    /// Check whether a window overlaps resting dock bounds.
    #[must_use]
    pub fn window_overlaps_bounds(window: &Window, bounds: Rect) -> bool {
        if window.minimized || window.size.w <= 0 || window.size.h <= 0 {
            return false;
        }
        let left = window.position.x as f32;
        let top = window.position.y as f32;
        let right = left + window.size.w as f32;
        let bottom = top + window.size.h as f32;
        left < bounds.x + bounds.w
            && right > bounds.x
            && top < bounds.y + bounds.h
            && bottom > bounds.y
    }

    /// Estimated rest centre of tile `i` in row of `n`.
    #[must_use]
    pub fn rest_centre_estimate(i: usize, n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        let section_extra = Self::section_extra(pinned_count, n);
        let bar_w = n as f32 * DOCK_TILE
            + (n as f32 - 1.0) * DOCK_TILE_GAP
            + section_extra
            + 2.0 * DOCK_PAD;
        let bar_x = (axis_len - bar_w) * 0.5;
        let extra = if i >= pinned_count {
            section_extra
        } else {
            0.0
        };
        bar_x + DOCK_PAD + i as f32 * (DOCK_TILE + DOCK_TILE_GAP) + extra + DOCK_TILE * 0.5
    }
}
