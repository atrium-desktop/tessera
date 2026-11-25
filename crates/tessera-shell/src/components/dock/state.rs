//! Headless state machine for the pure application Dock (ADR-0174, ADR-0254, ADR-0255).
//!
//! Owns pure functional state and transitions:
//! - Pinned applications, drag & drop reorder states, press tracking.
//! - Four-edge horizontal (`Top`/`Bottom`) and vertical (`Left`/`Right`) layout.
//! - Clean Hot Zone reveal/hide without collapsed capsule morphing.
//! - 100% headless testable with zero GPU or display dependencies.

use optics_ui::Rect;
use tessera_desktop::app::Entry;
use tessera_desktop::dock::DockPosition;
use tessera_desktop::window::{SpaceUse, Window};

use super::motion::{
    AUTOHIDE_IDLE_TIMEOUT, DOCK_EDGE_MARGIN, DOCK_PAD, DOCK_PANEL_HEIGHT, DOCK_REVEAL_SLIDE_OFFSET,
    DOCK_SECTION_GAP, DOCK_TILE, DOCK_TILE_GAP, DRAG_THRESHOLD, EDGE_DRAG_PROXIMITY,
    smoothstep_progress,
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
    /// Empty panel space (padding or gaps) — supports edge-drag repositioning.
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
    pub apps: Vec<DockApp>,
    pub all_apps: Vec<Entry>,
    pub catalog_revision: u64,
    pub pending_order: Option<Vec<String>>,
    pub press: Option<PressState>,
    pub autohide: bool,
    pub autohide_idle: f32,
    pub autohide_timeout: f32,
    pub dock_interacted: bool,
    pub dismiss_latched: bool,
    pub dock_obscured: bool,
    pub collapse_pending: bool,
    pub space_use: SpaceUse,
    pub position: DockPosition,
    pub last_cursor: Option<(f32, f32)>,
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
            dock_interacted: false,
            dismiss_latched: false,
            dock_obscured: false,
            collapse_pending: false,
            space_use: SpaceUse::Available,
            position: DockPosition::Bottom,
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
            self.dismiss_latched = false;
        }
    }

    /// Set the inactivity timeout in seconds before an autohiding dock hides.
    pub fn set_autohide_timeout(&mut self, timeout_secs: f32) {
        self.autohide_timeout = timeout_secs.max(0.1);
    }

    /// Set the dock position (`Top`, `Bottom`, `Left`, `Right`).
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
            self.collapse_pending = true;
            self.dismiss_latched = true;
        } else {
            self.collapse_pending = false;
            self.dismiss_latched = false;
            if !self.effective_autohide() {
                self.autohide_idle = 0.0;
            }
        }
    }

    /// Request explicit dock dismissal (Escape or click outside).
    pub fn request_dismiss(&mut self) {
        self.dismiss_latched = true;
        self.autohide_idle = self.autohide_timeout;
        self.press = None;
    }

    /// Resolve whether the pointer position keeps an already-revealed Dock open.
    ///
    /// When `reveal <= 0.001` (hidden), the Dock does not self-reveal — it waits
    /// for its bound Hot Zone to emit `ChromeCommand::SummonDock` (ADR-0254).
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn pointer_keeps_revealed(
        &self,
        effective_autohide: bool,
        reveal: f32,
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
            return false;
        }
        if reveal < 0.999 {
            return Self::live_panel_contains(position, cursor, current_panel, display);
        }
        Self::expanded_trigger_contains(position, cursor, rest_bounds, display)
    }

    /// Panel rectangle during edge slide reveal (`0.0` = slid toward edge,
    /// `1.0` = resting at `DOCK_EDGE_MARGIN`). No capsule morphing (ADR-0254).
    #[must_use]
    pub fn revealed_panel_rect(
        position: DockPosition,
        display: (f32, f32),
        bar_len: f32,
        reveal: f32,
    ) -> Rect {
        let mut rect = Self::panel_rect_for(position, bar_len, display);
        let offset = DOCK_REVEAL_SLIDE_OFFSET * (1.0 - smoothstep_progress(reveal));
        match position {
            DockPosition::Top => rect.y -= offset,
            DockPosition::Bottom => rect.y += offset,
            DockPosition::Left => rect.x -= offset,
            DockPosition::Right => rect.x += offset,
        }
        rect
    }

    /// Whether cursor is inside the expanded dock's approach corridor (reaching
    /// all the way to the anchored physical screen edge).
    #[must_use]
    pub fn expanded_trigger_contains(
        position: DockPosition,
        cursor: (f32, f32),
        rest_bounds: Rect,
        display: (f32, f32),
    ) -> bool {
        match position {
            DockPosition::Top => {
                cursor.0 >= rest_bounds.x
                    && cursor.0 < rest_bounds.x + rest_bounds.w
                    && cursor.1 >= 0.0
                    && cursor.1 < rest_bounds.y + rest_bounds.h
            }
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

    /// Whether cursor is inside the live sliding panel and its anchored edge gap.
    #[must_use]
    pub fn live_panel_contains(
        position: DockPosition,
        cursor: (f32, f32),
        current_panel: Rect,
        display: (f32, f32),
    ) -> bool {
        match position {
            DockPosition::Top => {
                cursor.0 >= current_panel.x
                    && cursor.0 < current_panel.x + current_panel.w
                    && cursor.1 >= 0.0
                    && cursor.1 < current_panel.y + current_panel.h
            }
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

    /// Pure 1D strip length for application tiles only (`[INV-DOCK-PURE-01]`).
    #[must_use]
    pub fn strip_length(widths: impl IntoIterator<Item = f32>, tile_gap: f32, pad: f32) -> f32 {
        let mut count = 0usize;
        let mut sum = 0.0f32;
        for w in widths {
            sum += w;
            count += 1;
        }
        if count == 0 {
            return pad * 2.0;
        }
        pad * 2.0 + sum + (count - 1) as f32 * tile_gap
    }

    /// Calculate the stable resting bounds of the pure application dock bar.
    #[must_use]
    pub fn rest_bounds(
        tile_count: usize,
        pinned_count: usize,
        position: DockPosition,
        display: (f32, f32),
    ) -> Rect {
        let bar_len = Self::strip_length(
            std::iter::repeat_n(DOCK_TILE, tile_count),
            DOCK_TILE_GAP,
            DOCK_PAD,
        ) + Self::section_extra(pinned_count, tile_count);
        Self::panel_rect_for(position, bar_len, display)
    }

    /// Construct a panel rectangle centered along `position` edge.
    #[must_use]
    pub fn panel_rect_for(position: DockPosition, bar_len: f32, display: (f32, f32)) -> Rect {
        match position {
            DockPosition::Top => Rect {
                x: (display.0 - bar_len) * 0.5,
                y: DOCK_EDGE_MARGIN,
                w: bar_len,
                h: DOCK_PANEL_HEIGHT,
            },
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

    /// Edge drag target screen edge (supports all four edges).
    #[must_use]
    pub fn edge_drag_target(cursor: (f32, f32), display: (f32, f32)) -> Option<DockPosition> {
        let d_top = cursor.1.max(0.0);
        let d_left = cursor.0.max(0.0);
        let d_right = (display.0 - cursor.0).max(0.0);
        let d_bottom = (display.1 - cursor.1).max(0.0);
        let min = d_top.min(d_bottom).min(d_left).min(d_right);
        if min > EDGE_DRAG_PROXIMITY {
            return None;
        }
        if min == d_bottom {
            Some(DockPosition::Bottom)
        } else if min == d_top {
            Some(DockPosition::Top)
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

    /// Estimated rest centre of tile `i` in a pure application strip of `n` tiles.
    #[must_use]
    pub fn rest_centre_estimate(i: usize, n: usize, pinned_count: usize, axis_len: f32) -> f32 {
        let section_extra = Self::section_extra(pinned_count, n);
        let bar_w = Self::strip_length(std::iter::repeat_n(DOCK_TILE, n), DOCK_TILE_GAP, DOCK_PAD)
            + section_extra;
        let bar_x = (axis_len - bar_w) * 0.5;
        let extra = if i >= pinned_count {
            section_extra
        } else {
            0.0
        };
        bar_x + DOCK_PAD + i as f32 * (DOCK_TILE + DOCK_TILE_GAP) + extra + DOCK_TILE * 0.5
    }
}
