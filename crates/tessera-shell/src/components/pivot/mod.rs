//! Pivot is the unified system intent and action surface (ADR-0151, ADR-0174).
//!
//! Consolidates application discovery, directed search, live window switching,
//! system command palette execution, inline micro-utilities (calculator), and
//! AI Agent / MCP dispatch into a single, unified, modal-free interaction surface.
//!
//! Follows the Model-Motion-View (MMV) architecture:
//! - [`state`]: Headless, deterministic search, catalog, and action execution state machine.
//! - [`motion`]: Progressive disclosure visibility dynamics driven via Optics `transit`.
//! - [`rendering`]: Stateless UI projection using Lens Frame, typography tokens, and glass materials.

pub mod motion;
pub mod rendering;
pub mod state;

use std::ops::{Deref, DerefMut};

pub use motion::PivotMotion;
pub use rendering::{ItemIcon, PivotItem, build_search_items, panel_rect};
pub use state::{AppCategory, PivotAction, PivotState};
pub use tessera_launch_services::{SystemCmd, eval_math};

use crate::component::{
    AppCatalog, BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    IconSet, LiquidGlassRegion, Localizer,
};
use crate::widgets::geom::contains;
use lens::{Frame, Input};
use tessera_design::{Design, GlassRole};
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_primitives::input::{
    KeyChar, XKB_KEY_Down, XKB_KEY_Escape, XKB_KEY_ISO_Left_Tab, XKB_KEY_Left, XKB_KEY_Return,
    XKB_KEY_Right, XKB_KEY_Tab, XKB_KEY_Up, key_action,
};

/// Unified system intent and action surface (Chrome host).
pub struct Pivot {
    pub state: PivotState,
    pub motion: PivotMotion,
    pub icons: IconSet,
    pub prev_down: bool,
    pub reduced_motion: bool,
    pub design: Design,
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
        }
    }

    /// Whether Pivot is currently active (open or animating).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state.is_open() || self.motion.visibility > 0.01
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
        self.state.toggle();
        self.motion.anim_active = true;
    }

    /// Close Pivot and trigger closing animation.
    pub fn close(&mut self) {
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

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if !self.state.is_open() && self.motion.visibility <= 0.01 {
            return Vec::new();
        }
        let fade = self.motion.visibility.clamp(0.0, 1.0);
        vec![BackdropRegion {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: display.1,
            wash: Some(crate::component::backdrop_wash(lens::Color::rgba(
                4,
                6,
                14,
                (54.0 * fade).round() as u8,
            ))),
            opacity: fade,
        }]
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        if !self.state.is_open() && self.motion.visibility <= 0.01 {
            return Vec::new();
        }
        let panel = rendering::panel_rect(&self.state, display, 1, self.motion.visibility);
        vec![LiquidGlassRegion::from_role(
            &self.design,
            GlassRole::ProminentPanel,
            BackdropRegion::from(panel),
            self.design.radii.glass_panel,
            self.motion.visibility,
        )]
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
                if let Some(item) =
                    items.get(self.state.search_selection.min(items.len().saturating_sub(1)))
                {
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

        let target = if self.state.is_open() { 1.0 } else { 0.0 };
        let progress = self
            .motion
            .advance(target, raw.dt_seconds.max(0.0), self.reduced_motion);
        if !self.state.is_open() && progress <= 0.001 {
            self.prev_down = down;
            return;
        }

        let panel = rendering::panel_rect(&self.state, display, 1, progress);
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
        let mut test_win = Window::default();
        test_win.id = WindowId(42);
        test_win.app_id = Some("slack".to_string());
        test_win.title = Some("Slack - Project Atrium".to_string());

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
