//! Pivot is the unified system intent and action surface (ADR-0151, ADR-0174).
//!
//! Consolidates application discovery, directed search, live window switching,
//! system command palette execution, inline micro-utilities (calculator), and
//! AI Agent / MCP dispatch into a single, unified, modal-free interaction surface.
//!
//! Follows the Model-Motion-View (MMV) architecture:
//! - [`state`]: Headless, deterministic search, catalog, and action execution state machine.
//! - [`motion`]: Progressive disclosure visibility dynamics driven via Optics `transit`.
//! - [`rendering`]: Stateless UI projection using Lens Frame, typography tokens, and painted fills.

pub mod motion;
pub mod rendering;
pub mod state;

use std::ops::{Deref, DerefMut};

pub use motion::PivotMotion;
pub use rendering::{ItemIcon, PivotItem, build_search_items, panel_rect};
pub use state::{AppCategory, PivotAction, PivotState};
pub use tessera_launch_services::{SystemCmd, eval_math};

use crate::component::{
    AppCatalog, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, IconSet, Localizer,
};
use crate::widgets::geom::contains;
use lens::{Frame, Input, Rect};
use tessera_design::Design;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_primitives::input::{
    KeyChar, XKB_KEY_Down, XKB_KEY_Escape, XKB_KEY_ISO_Left_Tab, XKB_KEY_Left, XKB_KEY_Return,
    XKB_KEY_Right, XKB_KEY_Tab, XKB_KEY_Up, key_action,
};

/// Frames the vacated panel band keeps being named after the close animation
/// settles (ADR-0227).
///
/// The panel is a transient chrome surface. The frame that removes it repaints
/// only the swapchain image it happened to acquire; the other images in the
/// presentation rotation still carry the panel until they are repaired. A
/// bounded retirement window keeps naming the band for one more frame than the
/// deepest supported rotation, so a closed launcher cannot flash back every
/// two or three frames while the ring cycles.
const RETIRE_FRAMES: u8 = 4;

/// Unified system intent and action surface (Chrome host).
pub struct Pivot {
    pub state: PivotState,
    pub motion: PivotMotion,
    pub icons: IconSet,
    pub prev_down: bool,
    pub reduced_motion: bool,
    pub design: Design,
    /// Set once the compositor pre-pass has advanced this frame's reveal
    /// spring. `render` consults it so the same `dt` is never integrated
    /// twice when both the pre-pass and a direct `render` call run.
    pub frame_prepared: bool,
    /// Whether the last pre-pass observed Pivot open. A close is detected from
    /// this edge rather than only from [`Pivot::close`], so every exit path —
    /// Escape, a clicked action, and the shell's `ClosePivot` command — arms
    /// the same retirement window.
    was_open: bool,
    /// Set when the last preparation observed an open-to-closed transition and
    /// the reveal has not reached zero yet.
    closing: bool,
    /// Union of every panel band painted while the reveal ran, used as the
    /// retirement footprint after the panel is gone.
    last_band: Option<Rect>,
    /// Band the closed panel vacated; painted while `retire_frames` is non-zero.
    retire_band: Option<Rect>,
    /// Damage-only frames remaining after the close animation settles.
    retire_frames: u8,
}

impl Deref for Pivot {
    type Target = PivotState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for Pivot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Pivot {
    /// Construct a new empty Pivot component.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PivotState::new(),
            motion: PivotMotion::new(),
            icons: IconSet::default(),
            prev_down: false,
            reduced_motion: false,
            design: Design::dark(),
            frame_prepared: false,
            was_open: false,
            closing: false,
            last_band: None,
            retire_band: None,
            retire_frames: 0,
        }
    }

    /// Advance the progressive-disclosure spring for the current frame.
    ///
    /// The compositor runs [`Chrome::prepare_frame`] before damage assessment,
    /// backdrop capture, and `render`, so `motion.visibility` must already
    /// reflect this frame's `dt` before [`Chrome::damage_regions`] and
    /// [`Chrome::pointer_damage_regions`] are queried — otherwise the repaint
    /// band describes frame T-1 geometry while the foreground panel draws
    /// frame T (ADR-0171, ADR-0217).
    fn prepare_frame_state(&mut self, input: &Input) {
        let open = self.state.is_open();
        if open && !self.was_open {
            // A fresh open owns a fresh footprint; never inherit the band the
            // previous session retired.
            self.last_band = None;
            self.retire_band = None;
            self.retire_frames = 0;
            self.closing = false;
        } else if !open && self.was_open {
            // Covers every close path, including actions that close the state
            // machine from `key_char`/`render` without calling [`Pivot::close`].
            self.closing = true;
        }
        self.was_open = open;

        let target = if open { 1.0 } else { 0.0 };
        self.motion.advance(
            target,
            input.as_raw().dt_seconds.max(0.0),
            self.reduced_motion,
        );

        if self.closing && self.motion.visibility <= 0.001 {
            self.retire_band = self.last_band;
            self.retire_frames = RETIRE_FRAMES;
            self.closing = false;
        } else if self.retire_frames > 0 {
            self.retire_frames -= 1;
        }

        self.frame_prepared = true;
    }

    /// Whether Pivot is currently active (open or animating). The retirement
    /// window is deliberately excluded: once the panel is gone it must not
    /// keep capturing the keyboard or the pointer while its pixels drain.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state.is_open() || self.motion.visibility > 0.01
    }

    /// The painted envelope of one panel placement: the body plus the
    /// floating material's shadow halo, clamped to the output.
    fn panel_band(&self, display: (f32, f32), panel: Rect) -> Option<Rect> {
        if display.0 <= 0.0 || display.1 <= 0.0 {
            return None;
        }
        let style = self
            .design
            .surfaces
            .for_role(tessera_design::SurfaceRole::ProminentPanel);
        let fringe = 2.0 * style.shadow_blur.max(0.0) + style.shadow_offset_y.abs() + 1.0;
        let x0 = (panel.x - fringe).floor().max(0.0);
        let y0 = (panel.y - fringe).floor().max(0.0);
        let x1 = (panel.x + panel.w + fringe).ceil().min(display.0);
        let y1 = (panel.y + panel.h + fringe + style.shadow_offset_y.max(0.0))
            .ceil()
            .min(display.1);
        (x1 > x0 && y1 > y0).then_some(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    fn union_band(&mut self, band: Rect) {
        self.last_band = Some(match self.last_band {
            Some(existing) => {
                let x0 = existing.x.min(band.x);
                let y0 = existing.y.min(band.y);
                let x1 = (existing.x + existing.w).max(band.x + band.w);
                let y1 = (existing.y + existing.h).max(band.y + band.h);
                Rect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                }
            }
            None => band,
        });
    }

    /// Update application catalog and cached icons.
    pub fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        let pinned = catalog.pinned.iter().map(|e| e.id.clone()).collect();
        self.state.update_apps(catalog.apps.clone(), pinned);
        self.icons = catalog.icons.clone();
    }

    /// Build unified search items (delegates to [`rendering::build_search_items`]).
    #[must_use]
    pub fn build_search_items(
        &self,
        query: &str,
        windows: &[Window],
        i18n: &Localizer,
    ) -> Vec<PivotItem> {
        rendering::build_search_items(&self.state, &self.icons, query, windows, i18n)
    }

    /// Toggle Pivot open or closed.
    pub fn toggle(&mut self) {
        let opening = !self.state.is_open();
        self.state.toggle();
        if opening {
            self.last_band = None;
            self.retire_band = None;
            self.retire_frames = 0;
            self.closing = false;
        } else {
            self.closing = true;
        }
        self.motion.anim_active = true;
    }

    /// Close Pivot and trigger closing animation.
    pub fn close(&mut self) {
        if self.state.is_open() {
            self.closing = true;
        }
        self.state.close();
        self.motion.anim_active = true;
    }
}

impl Default for Pivot {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for Pivot {
    fn anim_pending(&self) -> bool {
        self.motion.anim_active
    }

    fn damage_pending(&self) -> bool {
        self.retire_frames > 0
    }

    /// The panel plus its painted halo is the only surface the reveal can
    /// touch: it slides 14 px upward and fades in place. Localizing the
    /// animation keeps a close from repainting the whole output at the panel
    /// rate; the retirement window keeps naming the vacated band after the
    /// panel itself has gone (ADR-0227).
    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        if !self.state.is_open() && self.motion.visibility <= 0.001 && self.retire_frames == 0 {
            return Some(Vec::new());
        }
        if let Some(band) = self.retire_band {
            return Some(vec![band]);
        }
        // `last_band` is the union of every placement the reveal has painted,
        // so it also covers the strip a closing panel vacates when it slides
        // upward. Recomputing the band from the current frame alone would
        // leave that strip attached to the previous frame.
        if let Some(band) = self.last_band {
            return Some(vec![band]);
        }
        let panel = rendering::panel_rect(&self.state, display, usize::MAX, 1.0);
        self.panel_band(display, panel).map(|band| vec![band])
    }

    fn prepare_frame(
        &mut self,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
        self.prepare_frame_state(input);
    }

    fn pivot_active(&self) -> bool {
        self.is_active()
    }

    fn captures_keyboard(&self) -> bool {
        self.state.is_open()
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.is_active()
    }

    /// The pointer's reaction to Pivot is confined to its panel.
    ///
    /// Pivot takes the pointer across the whole output (a press outside the
    /// panel closes it), but the only pixels a *move* can change are the row
    /// and tile hover washes inside the panel body and the halo under it. The
    /// panel is resolved at its maximum content height so the band is a
    /// superset of every state the movement can occur in — a bound that is
    /// wrong in the small direction would strand stale pixels.
    fn pointer_damage_regions(
        &self,
        _x: f32,
        _y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        if !self.is_active() {
            return Some(Vec::new());
        }
        let panel = rendering::panel_rect(&self.state, display, usize::MAX, 1.0);
        let style = self
            .design
            .surfaces
            .for_role(tessera_design::SurfaceRole::ProminentPanel);
        let fringe = 2.0 * style.shadow_blur + 1.0;
        Some(vec![Rect {
            x: panel.x - fringe,
            y: panel.y - fringe + style.shadow_offset_y,
            w: panel.w + fringe * 2.0,
            h: panel.h + fringe * 2.0,
        }])
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        let panel = rendering::panel_rect(&self.state, display, 1, self.motion.visibility);
        if !contains(panel, x, y) {
            return Some(CursorShape::Default);
        }
        if y < panel.y + rendering::SEARCH_HEIGHT {
            Some(CursorShape::Text)
        } else {
            Some(CursorShape::Pointer)
        }
    }

    fn modal_active(&self) -> bool {
        self.is_active()
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::TogglePivot => {
                self.toggle();
            }
            ChromeCommand::ClosePivot => {
                self.close();
            }
            _ => {}
        }
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::AppCatalog(catalog) => self.update_app_catalog(catalog),
            ChromeUpdate::ReducedMotion(reduced) => self.reduced_motion = reduced,
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    #[allow(non_upper_case_globals)]
    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.state.is_open() {
            return;
        }

        // Global Escape / Tab
        match key.keysym {
            XKB_KEY_Escape => {
                if !self.state.query().is_empty() {
                    self.state.brain.open(); // clears query and selection
                    self.state.search_selection = 0;
                } else {
                    self.close();
                }
                self.motion.anim_active = true;
                return;
            }
            XKB_KEY_Tab | XKB_KEY_ISO_Left_Tab => {
                let forward = key.keysym == XKB_KEY_Tab;
                if self.state.query().is_empty() {
                    let cats = AppCategory::ALL;
                    let cur_pos = cats
                        .iter()
                        .position(|&c| c == self.state.active_category)
                        .unwrap_or(0);
                    let next_pos = if forward {
                        (cur_pos + 1) % cats.len()
                    } else {
                        (cur_pos + cats.len() - 1) % cats.len()
                    };
                    self.state.active_category = cats[next_pos];
                    self.state.browse_selection = 0;
                } else if forward {
                    self.state.search_selection += 1;
                } else {
                    self.state.search_selection = self.state.search_selection.saturating_sub(1);
                }
                self.motion.anim_active = true;
                return;
            }
            _ => {}
        }

        // Empty query: directional navigation in categorical grid
        if self.state.query().is_empty() {
            let cat_items = self.state.category_indices(self.state.active_category);
            let len = cat_items.len();
            match key.keysym {
                XKB_KEY_Left => {
                    if len > 0 {
                        self.state.browse_selection = self.state.browse_selection.saturating_sub(1);
                    }
                    return;
                }
                XKB_KEY_Right => {
                    if len > 0 && self.state.browse_selection + 1 < len {
                        self.state.browse_selection += 1;
                    }
                    return;
                }
                XKB_KEY_Up => {
                    if self.state.browse_selection >= rendering::BROWSE_COLS {
                        self.state.browse_selection -= rendering::BROWSE_COLS;
                    }
                    return;
                }
                XKB_KEY_Down => {
                    if len > 0 && self.state.browse_selection + rendering::BROWSE_COLS < len {
                        self.state.browse_selection += rendering::BROWSE_COLS;
                    }
                    return;
                }
                XKB_KEY_Return => {
                    if let Some(&app_index) = cat_items.get(self.state.browse_selection) {
                        self.state
                            .execute_action(PivotAction::LaunchApp(app_index), out);
                        self.motion.anim_active = true;
                    }
                    return;
                }
                _ => {}
            }
        }

        // In search mode with text
        match key.keysym {
            XKB_KEY_Up => {
                self.state.search_selection = self.state.search_selection.saturating_sub(1);
                return;
            }
            XKB_KEY_Down => {
                self.state.search_selection += 1;
                return;
            }
            XKB_KEY_Return => {
                let dummy_windows: Vec<Window> = Vec::new();
                let items = self.build_search_items(
                    self.state.query(),
                    &dummy_windows,
                    &Localizer::from_env(),
                );
                if let Some(item) = items.get(
                    self.state
                        .search_selection
                        .min(items.len().saturating_sub(1)),
                ) {
                    self.state.execute_action(item.action.clone(), out);
                    self.motion.anim_active = true;
                    return;
                }
            }
            _ => {}
        }

        // Append text to query
        let outcome = self.state.brain.handle(key_action(key.keysym, key.ch));
        self.state.search_selection = 0;
        if outcome.is_some() || !self.state.is_open() {
            self.motion.anim_active = true;
        }
        PivotState::emit(outcome, out);
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;

        let running: Vec<(String, _)> = windows
            .iter()
            .rev()
            .filter(|w| w.state.activated)
            .chain(windows.iter().rev().filter(|w| !w.state.activated))
            .filter_map(|w| w.app_id.as_ref().map(|id| (id.clone(), w.id)))
            .collect();
        self.state.brain.set_running(running);

        if !self.frame_prepared {
            self.prepare_frame_state(input);
        }
        self.frame_prepared = false;
        let progress = self.motion.visibility;
        if !self.state.is_open() && progress <= 0.001 {
            self.prev_down = down;
            return;
        }

        let panel = rendering::panel_rect(&self.state, display, 1, progress);
        // Remember every band the panel actually paints while the reveal runs.
        // When the panel closes, its query is cleared, so recomputing the band
        // from the closed state could shrink it below the search-result panel
        // the user was just looking at. The union is the only safe footprint.
        if let Some(band) = self.panel_band(display, panel) {
            self.union_band(band);
        }
        let cursor = (raw.cursor.x, raw.cursor.y);

        if pressed && self.state.is_open() && !contains(panel, cursor.0, cursor.1) {
            self.close();
        }

        let action = rendering::render_pivot(
            &mut self.state,
            &self.motion,
            &self.icons,
            &self.design,
            frame,
            display,
            cursor,
            pressed,
            windows,
            i18n,
        );

        if let Some(act) = action {
            self.state.execute_action(act, out);
            self.motion.anim_active = true;
        }

        self.prev_down = down;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::app::ApplicationTarget;
    use tessera_desktop::app::Entry;
    use tessera_desktop::window::WindowId;

    fn make_app(id: &str, name: &str, cat: &[&str]) -> Entry {
        Entry {
            target: ApplicationTarget::External,
            id: id.to_string(),
            name: name.to_string(),
            categories: cat.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn frame_input(dt: f32) -> Input {
        let mut input = Input::default();
        input.set_dt(dt);
        input
    }

    #[test]
    fn a_closed_launcher_retires_its_band_and_stops_animating() {
        let mut pivot = Pivot::new();
        pivot.state.open();
        let input = frame_input(1.0 / 60.0);
        for _ in 0..240 {
            pivot.prepare_frame_state(&input);
        }
        assert!(pivot.motion.visibility >= 0.999);
        assert!(!pivot.anim_pending());

        // The band the open panel painted is owned through the close.
        pivot.last_band = Some(Rect {
            x: 100.0,
            y: 100.0,
            w: 400.0,
            h: 300.0,
        });
        pivot.close();
        for _ in 0..120 {
            pivot.prepare_frame_state(&input);
            if pivot.retire_frames > 0 {
                break;
            }
        }
        assert!(!pivot.is_active(), "the panel itself is gone");
        assert!(pivot.retire_frames > 0, "the vacated band is still owned");
        assert!(
            pivot
                .damage_regions(&[], (1280.0, 800.0))
                .is_some_and(|regions| regions.len() == 1),
            "a retired launcher localizes its clear to the panel band"
        );

        for _ in 0..RETIRE_FRAMES {
            pivot.prepare_frame_state(&input);
        }
        assert_eq!(pivot.retire_frames, 0);
        assert!(!pivot.damage_pending(), "the retirement window is finite");
        assert_eq!(
            pivot.damage_regions(&[], (1280.0, 800.0)),
            Some(Vec::new()),
            "a quiescent closed launcher owns no pixels"
        );
    }

    #[test]
    fn pivot_toggle_and_visibility_lifecycle() {
        let mut pivot = Pivot::new();
        assert!(!pivot.is_active());

        // Toggle open
        pivot.toggle();
        assert!(pivot.is_active());
        assert!(pivot.brain.is_open());

        // Toggle closed
        pivot.toggle();
        assert!(!pivot.brain.is_open());
    }

    #[test]
    fn pivot_category_filtering() {
        let apps = vec![
            make_app("code.desktop", "VS Code", &["Development", "IDE"]),
            make_app("writer.desktop", "LibreOffice Writer", &["Office"]),
            make_app("gimp.desktop", "GIMP", &["Graphics"]),
            make_app("vlc.desktop", "VLC", &["AudioVideo", "Player"]),
        ];

        let mut pivot = Pivot::new();
        pivot.update_app_catalog(&AppCatalog {
            apps,
            pinned: Vec::new(),
            icons: IconSet::default(),
            ..Default::default()
        });

        assert_eq!(pivot.category_indices(AppCategory::All).len(), 4);
        assert_eq!(pivot.category_indices(AppCategory::Development).len(), 1);
        assert_eq!(pivot.category_indices(AppCategory::Office).len(), 1);
        assert_eq!(pivot.category_indices(AppCategory::Graphics).len(), 1);
        assert_eq!(pivot.category_indices(AppCategory::Media).len(), 1);
        assert_eq!(pivot.category_indices(AppCategory::Network).len(), 0);
    }

    #[test]
    fn pivot_unified_intent_query_integration() {
        let apps = vec![make_app(
            "slack.desktop",
            "Slack",
            &["Network", "InstantMessaging"],
        )];
        let mut pivot = Pivot::new();
        pivot.update_app_catalog(&AppCatalog {
            apps,
            pinned: Vec::new(),
            icons: IconSet::default(),
            ..Default::default()
        });

        let i18n = Localizer::from_env();

        // 1. Math computation
        let math_items = pivot.build_search_items("1920 * 1080", &[], &i18n);
        assert!(!math_items.is_empty());
        assert!(math_items[0].title.contains("2073600"));

        // 2. System commands
        let cmd_items = pivot.build_search_items("lock", &[], &i18n);
        assert!(!cmd_items.is_empty());
        assert_eq!(
            cmd_items[0].action,
            PivotAction::SystemCommand(SystemCmd::Lock)
        );

        // 3. AI Agent queries
        let agent_items = pivot.build_search_items("how to write a rust macro", &[], &i18n);
        assert!(
            agent_items
                .iter()
                .any(|i| matches!(i.action, PivotAction::AskAgent(_)))
        );

        // 4. Live window switching
        let test_win = Window {
            id: WindowId(42),
            app_id: Some("slack".to_string()),
            title: Some("Slack - Project Atrium".to_string()),
            ..Default::default()
        };

        let win_items = pivot.build_search_items("slack", &[test_win], &i18n);
        assert!(
            win_items
                .iter()
                .any(|i| matches!(i.action, PivotAction::FocusWindow(WindowId(42))))
        );

        // 5. Action executions
        let mut events = ChromeEvents::default();
        pivot.execute_action(PivotAction::CopyCalculation("2073600".into()), &mut events);
        assert_eq!(events.clipboard_copy.as_deref(), Some("2073600"));

        pivot.execute_action(PivotAction::AskAgent("summarize repo".into()), &mut events);
        assert_eq!(events.agent_prompt.as_deref(), Some("summarize repo"));
    }
}
