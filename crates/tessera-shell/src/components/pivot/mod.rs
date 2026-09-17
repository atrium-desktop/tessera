//! Pivot is the unified system intent and action surface (ADR-0151).
//!
//! It consolidates application discovery, directed search, live window switching,
//! system command palette execution, inline micro-utilities (calculator), and
//! AI Agent / MCP dispatch into a single, unified, modal-free interaction surface.

use std::ffi::c_void;

use crate::component::{
    AppCatalog, BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    IconSet, LiquidGlassRegion, Localizer, Message, ellipsize,
};
use crate::widgets::geom::contains;
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, Rect};
use tessera_apps::{IntentAction, IntentEngine, IntentKind};
pub use tessera_apps::{SystemCmd, eval_math};
use tessera_design::materials::{chrome_place, surface_layout};
use tessera_design::{Design, GlassRole, materials};
use tessera_desktop::app::BuiltInApplication;
use tessera_desktop::app::Entry;
use tessera_desktop::launcher::Launch;
use tessera_desktop::launcher::Launcher as SearchBrain;
use tessera_desktop::system::SystemAction;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_types::input::KeyChar;
use tessera_types::input::XKB_KEY_Down;
use tessera_types::input::XKB_KEY_Escape;
use tessera_types::input::XKB_KEY_ISO_Left_Tab;
use tessera_types::input::XKB_KEY_Left;
use tessera_types::input::XKB_KEY_Return;
use tessera_types::input::XKB_KEY_Right;
use tessera_types::input::XKB_KEY_Tab;
use tessera_types::input::XKB_KEY_Up;
use tessera_types::input::key_action;

const PANEL_MAX_WIDTH: f32 = 720.0;
const PANEL_SIDE_MARGIN: f32 = 20.0;
const PANEL_TOP_MIN: f32 = 64.0;
const PANEL_TOP_FRACTION: f32 = 0.12;
const SEARCH_HEIGHT: f32 = 68.0;
const RESULT_HEIGHT: f32 = 56.0;
const EMPTY_HEIGHT: f32 = 80.0;
const MAX_VISIBLE_RESULTS: usize = 6;
const ICON_SIZE: f32 = 36.0;
const BROWSE_TILE_W: f32 = 98.0;
const BROWSE_TILE_H: f32 = 88.0;
const BROWSE_COLS: usize = 6;
const ANIMATION_SPEED: f32 = 22.0;

/// Standard categories for visual application inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategory {
    All,
    Development,
    Office,
    Graphics,
    Media,
    Network,
    System,
    Utilities,
}

impl AppCategory {
    pub const ALL: [AppCategory; 8] = [
        AppCategory::All,
        AppCategory::Development,
        AppCategory::Office,
        AppCategory::Graphics,
        AppCategory::Media,
        AppCategory::Network,
        AppCategory::System,
        AppCategory::Utilities,
    ];

    #[must_use]
    pub const fn message(self) -> Message {
        match self {
            Self::All => Message::CategoryAll,
            Self::Development => Message::CategoryDevelopment,
            Self::Office => Message::CategoryOffice,
            Self::Graphics => Message::CategoryGraphics,
            Self::Media => Message::CategoryMedia,
            Self::Network => Message::CategoryNetwork,
            Self::System => Message::CategorySystem,
            Self::Utilities => Message::CategoryUtilities,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Development => "Dev",
            Self::Office => "Office",
            Self::Graphics => "Graphics",
            Self::Media => "Media",
            Self::Network => "Network",
            Self::System => "System",
            Self::Utilities => "Utilities",
        }
    }

    #[must_use]
    pub fn matches(self, entry: &Entry) -> bool {
        if self == Self::All {
            return true;
        }
        let categories = &entry.categories;
        match self {
            Self::Development => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("development") || s.eq_ignore_ascii_case("ide")
            }),
            Self::Office => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("office") || s.eq_ignore_ascii_case("texteditor")
            }),
            Self::Graphics => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("graphics")
                    || s.eq_ignore_ascii_case("photography")
                    || s.eq_ignore_ascii_case("2dgraphics")
            }),
            Self::Media => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("audiovideo")
                    || s.eq_ignore_ascii_case("audio")
                    || s.eq_ignore_ascii_case("video")
                    || s.eq_ignore_ascii_case("player")
            }),
            Self::Network => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("network")
                    || s.eq_ignore_ascii_case("webbrowser")
                    || s.eq_ignore_ascii_case("email")
            }),
            Self::System => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("system")
                    || s.eq_ignore_ascii_case("settings")
                    || s.eq_ignore_ascii_case("filemanager")
            }),
            Self::Utilities => categories.iter().any(|c| {
                let s = c.as_str();
                s.eq_ignore_ascii_case("utility") || s.eq_ignore_ascii_case("accessories")
            }),
            Self::All => true,
        }
    }
}

/// Action to execute when a search item is activated (re-exported from tessera-apps).
pub type PivotAction = IntentAction;

/// Displayable search result item.
pub struct PivotItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: ItemIcon,
    pub is_running: bool,
    pub action: PivotAction,
}

pub enum ItemIcon {
    App(Option<*mut c_void>),
    System(Icon),
    Math,
    Agent,
}

/// Unified system intent and action surface.
pub struct Pivot {
    brain: SearchBrain,
    icons: IconSet,
    active_category: AppCategory,
    browse_selection: usize,
    search_selection: usize,
    pinned_ids: Vec<String>,
    visibility: f32,
    anim_active: bool,
    prev_down: bool,
    reduced_motion: bool,
    design: Design,
}

impl Pivot {
    /// Construct a new empty Pivot component.
    #[must_use]
    pub fn new() -> Self {
        Self {
            brain: SearchBrain::new(Vec::new()),
            icons: IconSet::default(),
            active_category: AppCategory::All,
            browse_selection: 0,
            search_selection: 0,
            pinned_ids: Vec::new(),
            visibility: 0.0,
            anim_active: false,
            prev_down: false,
            reduced_motion: false,
            design: Design::dark(),
        }
    }

    /// Toggle Pivot open or closed.
    pub fn toggle(&mut self) {
        if self.brain.is_open() {
            self.close();
        } else {
            self.brain.open();
            self.browse_selection = 0;
            self.search_selection = 0;
            self.anim_active = true;
        }
    }

    pub fn close(&mut self) {
        self.brain.close();
        self.browse_selection = 0;
        self.search_selection = 0;
        self.anim_active = true;
    }

    pub fn is_active(&self) -> bool {
        self.brain.is_open() || self.visibility > 0.01
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.brain.replace_apps(catalog.apps.clone());
        self.icons = catalog.icons.clone();
        self.pinned_ids = catalog.pinned.iter().map(|e| e.id.clone()).collect();
    }

    fn advance_visibility(&mut self, target: f32, dt: f32) -> f32 {
        if self.reduced_motion {
            self.visibility = target;
            self.anim_active = false;
            return target;
        }
        let blend = 1.0 - (-ANIMATION_SPEED * dt.clamp(0.0, 1.0 / 30.0)).exp();
        self.visibility += (target - self.visibility) * blend;
        self.anim_active = (self.visibility - target).abs() > 0.002;
        if !self.anim_active {
            self.visibility = target;
        }
        self.visibility.clamp(0.0, 1.0)
    }

    fn entry_icon(&self, entry: &Entry) -> Option<*mut c_void> {
        let get = |key: &str| {
            let key = key.to_ascii_lowercase();
            (!key.is_empty()).then(|| self.icons.get(&key)).flatten()
        };
        entry
            .startup_wm_class
            .as_deref()
            .and_then(get)
            .or_else(|| get(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id)))
            .or_else(|| entry.icon.as_deref().and_then(get))
            .or_else(|| self.icons.default_icon())
    }

    fn app_icon_by_id(&self, app_id: &str) -> Option<*mut c_void> {
        let get = |key: &str| {
            let key = key.to_ascii_lowercase();
            (!key.is_empty()).then(|| self.icons.get(&key)).flatten()
        };
        get(app_id.strip_suffix(".desktop").unwrap_or(app_id)).or_else(|| self.icons.default_icon())
    }

    fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.activate_entry(*entry),
            Some(Launch::Focus(window)) => out.clicked = Some(window),
            Some(Launch::BuiltIn(app)) => out.open_builtin = Some(app),
            None => {}
        }
    }

    fn top(display: (f32, f32)) -> f32 {
        (display.1 * PANEL_TOP_FRACTION)
            .max(PANEL_TOP_MIN)
            .min((display.1 - SEARCH_HEIGHT - PANEL_SIDE_MARGIN).max(PANEL_SIDE_MARGIN))
    }

    fn category_indices(&self, category: AppCategory) -> Vec<usize> {
        self.brain
            .apps()
            .iter()
            .enumerate()
            .filter(|(_, entry)| category.matches(entry))
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Build unified search items (Math + Commands + Windows + Apps + Agent) for a query.
    pub fn build_search_items(
        &self,
        query: &str,
        windows: &[Window],
        i18n: &Localizer,
    ) -> Vec<PivotItem> {
        let intent_items = IntentEngine::query(
            query,
            self.brain.apps(),
            &self.brain.filtered(),
            |idx| self.brain.is_running(idx),
            windows,
            i18n,
        );

        intent_items
            .into_iter()
            .map(|it| {
                let icon = match &it.kind {
                    IntentKind::App { app_id } => ItemIcon::App(self.app_icon_by_id(app_id)),
                    IntentKind::Window { app_id, .. } => ItemIcon::App(self.app_icon_by_id(app_id)),
                    IntentKind::System(cmd) => ItemIcon::System(Self::cmd_to_icon(*cmd)),
                    IntentKind::Math => ItemIcon::Math,
                    IntentKind::Agent => ItemIcon::Agent,
                };
                PivotItem {
                    title: it.title,
                    subtitle: it.subtitle,
                    icon,
                    is_running: it.is_running,
                    action: it.action,
                }
            })
            .collect()
    }

    fn cmd_to_icon(cmd: SystemCmd) -> Icon {
        match cmd {
            SystemCmd::Lock => Icon::Shield,
            SystemCmd::Screenshot => Icon::Zap,
            SystemCmd::Suspend => Icon::Clock,
            SystemCmd::Restart => Icon::RotateCw,
            SystemCmd::PowerOff => Icon::X,
        }
    }

    fn panel_rect(&self, display: (f32, f32), content_count: usize, progress: f32) -> Rect {
        let width = PANEL_MAX_WIDTH.min((display.0 - PANEL_SIDE_MARGIN * 2.0).max(1.0));
        let query_empty = self.brain.query().is_empty();

        let content_height = if query_empty {
            let count = self.category_indices(self.active_category).len();
            let rows = count.div_ceil(BROWSE_COLS).clamp(1, 3);
            42.0 + (rows as f32 * BROWSE_TILE_H) + 16.0
        } else {
            let count = content_count.min(MAX_VISIBLE_RESULTS);
            if count == 0 {
                EMPTY_HEIGHT
            } else {
                count as f32 * RESULT_HEIGHT
            }
        };

        Rect {
            x: (display.0 - width) * 0.5,
            y: Self::top(display) - (1.0 - progress.clamp(0.0, 1.0)) * 14.0,
            w: width,
            h: (SEARCH_HEIGHT + content_height)
                .min((display.1 - Self::top(display) - PANEL_SIDE_MARGIN).max(1.0)),
        }
    }

    fn execute_action(&mut self, action: PivotAction, out: &mut ChromeEvents) {
        match action {
            PivotAction::LaunchApp(app_index) => {
                let app = &self.brain.apps()[app_index];
                if let Some(&window_id) = self.brain.running_surfaces(app_index).first() {
                    Self::emit(Some(Launch::Focus(window_id)), out);
                } else {
                    Self::emit(Some(Launch::Spawn(Box::new(app.clone()))), out);
                }
            }
            PivotAction::FocusWindow(window_id) => {
                out.clicked = Some(window_id);
            }
            PivotAction::SystemCommand(cmd) => match cmd {
                SystemCmd::Lock => {
                    out.lock = true;
                }
                SystemCmd::Screenshot => {
                    out.open_builtin = Some(BuiltInApplication::ScreenshotSelector);
                }
                SystemCmd::Suspend => {
                    out.system_actions.push(SystemAction::Suspend);
                }
                SystemCmd::Restart => {
                    out.system_actions.push(SystemAction::Reboot);
                }
                SystemCmd::PowerOff => {
                    out.system_actions.push(SystemAction::PowerOff);
                }
            },
            PivotAction::CopyCalculation(res) => {
                out.clipboard_copy = Some(res);
            }
            PivotAction::AskAgent(prompt) => {
                out.agent_prompt = Some(prompt);
            }
        }
        self.close();
    }
}

impl Default for Pivot {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for Pivot {
    fn anim_pending(&self) -> bool {
        self.anim_active
    }

    fn pivot_active(&self) -> bool {
        self.is_active()
    }

    fn captures_keyboard(&self) -> bool {
        self.brain.is_open()
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
        let panel = self.panel_rect(display, 1, self.visibility);
        if !contains(panel, x, y) {
            return Some(CursorShape::Default);
        }
        if y < panel.y + SEARCH_HEIGHT {
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
            ChromeCommand::TogglePivot
            | ChromeCommand::ToggleLauncher
            | ChromeCommand::TogglePrism => {
                self.toggle();
            }
            ChromeCommand::ClosePivot
            | ChromeCommand::CloseLauncher
            | ChromeCommand::ClosePrism => {
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
        if !self.brain.is_open() && self.visibility <= 0.01 {
            return Vec::new();
        }
        let fade = self.visibility.clamp(0.0, 1.0);
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
        if !self.brain.is_open() && self.visibility <= 0.01 {
            return Vec::new();
        }
        let panel = self.panel_rect(display, 1, self.visibility);
        vec![LiquidGlassRegion::from_role(
            &self.design,
            GlassRole::ProminentPanel,
            BackdropRegion::from(panel),
            self.design.radii.glass_panel,
            self.visibility,
        )]
    }

    #[allow(non_upper_case_globals)]
    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.brain.is_open() {
            return;
        }

        // Global Escape / Tab
        match key.keysym {
            XKB_KEY_Escape => {
                if !self.brain.query().is_empty() {
                    self.brain.open(); // clears query and selection
                    self.search_selection = 0;
                } else {
                    self.close();
                }
                self.anim_active = true;
                return;
            }
            XKB_KEY_Tab | XKB_KEY_ISO_Left_Tab => {
                let forward = key.keysym == XKB_KEY_Tab;
                if self.brain.query().is_empty() {
                    let cats = AppCategory::ALL;
                    let cur_pos = cats
                        .iter()
                        .position(|&c| c == self.active_category)
                        .unwrap_or(0);
                    let next_pos = if forward {
                        (cur_pos + 1) % cats.len()
                    } else {
                        (cur_pos + cats.len() - 1) % cats.len()
                    };
                    self.active_category = cats[next_pos];
                    self.browse_selection = 0;
                } else {
                    if forward {
                        self.search_selection += 1;
                    } else {
                        self.search_selection = self.search_selection.saturating_sub(1);
                    }
                }
                self.anim_active = true;
                return;
            }
            _ => {}
        }

        // Empty query: directional navigation in categorical grid
        if self.brain.query().is_empty() {
            let cat_items = self.category_indices(self.active_category);
            let len = cat_items.len();
            match key.keysym {
                XKB_KEY_Left => {
                    if len > 0 {
                        self.browse_selection = self.browse_selection.saturating_sub(1);
                    }
                    return;
                }
                XKB_KEY_Right => {
                    if len > 0 && self.browse_selection + 1 < len {
                        self.browse_selection += 1;
                    }
                    return;
                }
                XKB_KEY_Up => {
                    if self.browse_selection >= BROWSE_COLS {
                        self.browse_selection -= BROWSE_COLS;
                    }
                    return;
                }
                XKB_KEY_Down => {
                    if len > 0 && self.browse_selection + BROWSE_COLS < len {
                        self.browse_selection += BROWSE_COLS;
                    }
                    return;
                }
                XKB_KEY_Return => {
                    if let Some(&app_index) = cat_items.get(self.browse_selection) {
                        self.execute_action(PivotAction::LaunchApp(app_index), out);
                    }
                    return;
                }
                _ => {}
            }
        }

        // In search mode with text
        match key.keysym {
            XKB_KEY_Up => {
                self.search_selection = self.search_selection.saturating_sub(1);
                return;
            }
            XKB_KEY_Down => {
                self.search_selection += 1;
                return;
            }
            XKB_KEY_Return => {
                // Enter activation will be resolved in render or via brain
                let dummy_windows: Vec<Window> = Vec::new();
                let items = self.build_search_items(
                    self.brain.query(),
                    &dummy_windows,
                    &Localizer::from_env(),
                );
                if let Some(item) =
                    items.get(self.search_selection.min(items.len().saturating_sub(1)))
                {
                    self.execute_action(item.action.clone(), out);
                    return;
                }
            }
            _ => {}
        }

        // Append text to query
        let outcome = self.brain.handle(key_action(key.keysym, key.ch));
        self.search_selection = 0;
        if outcome.is_some() || !self.brain.is_open() {
            self.anim_active = true;
        }
        Self::emit(outcome, out);
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
        let display = raw.display_size;
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;

        let running: Vec<(String, _)> = windows
            .iter()
            .rev()
            .filter(|w| w.state.activated)
            .chain(windows.iter().rev().filter(|w| !w.state.activated))
            .filter_map(|w| w.app_id.as_ref().map(|id| (id.clone(), w.id)))
            .collect();
        self.brain.set_running(running);

        let target = if self.brain.is_open() { 1.0 } else { 0.0 };
        let progress = self.advance_visibility(target, raw.dt_seconds.max(0.0));
        if !self.brain.is_open() && progress <= 0.001 {
            self.prev_down = down;
            return;
        }

        let query = self.brain.query();
        let query_empty = query.is_empty();
        let search_items = if !query_empty {
            self.build_search_items(query, windows, i18n)
        } else {
            Vec::new()
        };

        let panel = self.panel_rect((display.x, display.y), search_items.len(), progress);
        let cursor = raw.cursor;

        if pressed && self.brain.is_open() && !contains(panel, cursor.x, cursor.y) {
            self.close();
        }

        let design = self.design;
        let type_scale = design.typography;
        let mut panel_mat = materials::glass_panel(&design);
        panel_mat.bg = design.colors.glass_surface;

        frame.place(
            "tessera-pivot-panel",
            &chrome_place(panel, panel_mat),
            |_| {},
        );

        // 1. Search Query Bar
        let search_rect = Rect {
            x: panel.x,
            y: panel.y,
            w: panel.w,
            h: SEARCH_HEIGHT,
        };
        let search_text_width = (search_rect.w - 80.0).max(0.0);
        let shown_placeholder = ellipsize(
            frame,
            i18n.text(Message::SearchApplications),
            type_scale.title,
            search_text_width,
        );
        let shown_query = ellipsize(
            frame,
            self.brain.query(),
            type_scale.title,
            search_text_width,
        );
        let query_metrics = frame.measure_text(&shown_query, type_scale.title);

        frame.place(
            "tessera-pivot-search-bar",
            &chrome_place(
                search_rect,
                LayoutOpts {
                    bg: Color::TRANSPARENT,
                    pad: 0.0,
                    cross: Align::Center,
                    ..surface_layout()
                },
            ),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: search_rect.w,
                        height: search_rect.h,
                        gap: 12.0,
                        pad: 20.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.icon(Icon::Search, 22.0);
                        if query_empty {
                            frame.label_compact_sized(&shown_placeholder, type_scale.title);
                        } else {
                            frame.label_compact_sized(&shown_query, type_scale.title);
                        }
                    },
                );
            },
        );

        // Active Caret
        if self.brain.is_open() {
            let caret_x = if query_empty {
                search_rect.x + 54.0
            } else {
                (search_rect.x + 54.0 + query_metrics.width)
                    .min(search_rect.x + search_rect.w - 24.0)
            };
            frame.place(
                "tessera-pivot-caret",
                &chrome_place(
                    Rect {
                        x: caret_x,
                        y: search_rect.y + 22.0,
                        w: 2.0,
                        h: 24.0,
                    },
                    LayoutOpts {
                        bg: design.colors.application_text,
                        radius: 1.0,
                        pad: 0.0,
                        ..surface_layout()
                    },
                ),
                |_| {},
            );
        }

        // Separator divider
        frame.place(
            "tessera-pivot-divider",
            &chrome_place(
                Rect {
                    x: panel.x + 16.0,
                    y: panel.y + SEARCH_HEIGHT - 1.0,
                    w: (panel.w - 32.0).max(0.0),
                    h: 1.0,
                },
                LayoutOpts {
                    bg: design.colors.application_border,
                    pad: 0.0,
                    ..surface_layout()
                },
            ),
            |_| {},
        );

        // 2. Body Area
        let mut chosen_action: Option<PivotAction> = None;

        if !query_empty {
            // High-throughput Unified Intent List
            let capacity = MAX_VISIBLE_RESULTS;
            let range = 0..search_items.len().min(capacity);

            if search_items.is_empty() {
                frame.place(
                    "tessera-pivot-empty",
                    &chrome_place(
                        Rect {
                            x: panel.x,
                            y: panel.y + SEARCH_HEIGHT,
                            w: panel.w,
                            h: EMPTY_HEIGHT,
                        },
                        LayoutOpts {
                            bg: Color::TRANSPARENT,
                            pad: 0.0,
                            cross: Align::Center,
                            ..surface_layout()
                        },
                    ),
                    |frame| {
                        frame.centered(panel.w, EMPTY_HEIGHT, |frame| {
                            frame.label_compact_sized(
                                i18n.text(Message::NoApplicationsFound),
                                type_scale.body,
                            );
                        });
                    },
                );
            } else {
                let selection_clamped = self
                    .search_selection
                    .min(search_items.len().saturating_sub(1));
                for (visible_pos, item) in search_items[range].iter().enumerate() {
                    let row = Rect {
                        x: panel.x,
                        y: panel.y + SEARCH_HEIGHT + visible_pos as f32 * RESULT_HEIGHT,
                        w: panel.w,
                        h: RESULT_HEIGHT,
                    };
                    let hovered = self.brain.is_open() && contains(row, cursor.x, cursor.y);
                    if pressed && hovered {
                        chosen_action = Some(item.action.clone());
                    }
                    let selected = visible_pos == selection_clamped;
                    let text_w = (row.w - 190.0).max(1.0);
                    let name = ellipsize(frame, &item.title, type_scale.body, text_w);
                    let subtitle = item
                        .subtitle
                        .as_deref()
                        .map(|t| ellipsize(frame, t, type_scale.footnote, text_w));
                    let is_running = item.is_running;
                    let action_badge = match &item.action {
                        PivotAction::LaunchApp(_) => {
                            if is_running {
                                "↵ Focus"
                            } else {
                                "↵ Open"
                            }
                        }
                        PivotAction::FocusWindow(_) => "↵ Switch",
                        PivotAction::SystemCommand(_) => "↵ Run",
                        PivotAction::CopyCalculation(_) => "↵ Copy",
                        PivotAction::AskAgent(_) => "↵ Ask Agent",
                    };

                    frame.place(
                        &format!("tessera-pivot-result-{visible_pos}"),
                        &chrome_place(
                            row,
                            LayoutOpts {
                                bg: if selected {
                                    design.colors.application_surface_active
                                } else if hovered {
                                    design.colors.application_surface_hover
                                } else {
                                    Color::TRANSPARENT
                                },
                                pad: 0.0,
                                ..surface_layout()
                            },
                        ),
                        |frame| {
                            frame.row_ex(
                                &LayoutOpts {
                                    width: row.w,
                                    height: row.h,
                                    gap: 12.0,
                                    pad: 10.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |frame| {
                                    match &item.icon {
                                        ItemIcon::App(ptr) => render_icon(frame, *ptr, progress),
                                        ItemIcon::System(ic) => {
                                            frame.icon(*ic, 22.0);
                                        }
                                        ItemIcon::Math => {
                                            frame.icon(Icon::Sliders, 22.0);
                                        }
                                        ItemIcon::Agent => {
                                            frame.icon(Icon::Zap, 22.0);
                                        }
                                    }
                                    frame.column_ex(
                                        &LayoutOpts {
                                            width: text_w,
                                            height: 40.0,
                                            gap: 2.0,
                                            ..Default::default()
                                        },
                                        |frame| {
                                            frame.label_compact_sized(&name, type_scale.body);
                                            if let Some(sub) = &subtitle {
                                                frame.label_compact_sized(sub, type_scale.footnote);
                                            }
                                        },
                                    );
                                    frame.flex(1.0);
                                    if is_running && !selected {
                                        frame.label_compact_sized("●", type_scale.caption);
                                    }
                                    if selected {
                                        frame.row_ex(
                                            &LayoutOpts {
                                                pad: 4.0,
                                                radius: 5.0,
                                                bg: design.colors.menu_border,
                                                cross: Align::Center,
                                                ..Default::default()
                                            },
                                            |frame| {
                                                frame.label_compact_sized(
                                                    action_badge,
                                                    type_scale.caption,
                                                );
                                            },
                                        );
                                    }
                                },
                            );
                        },
                    );
                }
            }
        } else {
            // Category selector pills row
            let cat_row_y = panel.y + SEARCH_HEIGHT + 8.0;
            let pill_w = 74.0;
            for (i, &cat) in AppCategory::ALL.iter().enumerate() {
                let pill = Rect {
                    x: panel.x + 20.0 + i as f32 * (pill_w + 6.0),
                    y: cat_row_y,
                    w: pill_w,
                    h: 28.0,
                };
                let is_active = self.active_category == cat;
                let hovered = self.brain.is_open() && contains(pill, cursor.x, cursor.y);
                if pressed && hovered {
                    self.active_category = cat;
                    self.browse_selection = 0;
                }
                frame.place(
                    &format!("tessera-pivot-cat-{i}"),
                    &chrome_place(
                        pill,
                        LayoutOpts {
                            bg: if is_active {
                                design.colors.application_surface_active
                            } else if hovered {
                                design.colors.application_surface_hover
                            } else {
                                Color::TRANSPARENT
                            },
                            radius: 14.0,
                            pad: 0.0,
                            cross: Align::Center,
                            ..surface_layout()
                        },
                    ),
                    |frame| {
                        frame.centered(pill.w, pill.h, |frame| {
                            frame
                                .label_compact_sized(i18n.text(cat.message()), type_scale.footnote);
                        });
                    },
                );
            }

            // Categorized application grid
            let cat_items = self.category_indices(self.active_category);
            let grid_y = cat_row_y + 36.0;
            for (pos, &app_index) in cat_items.iter().take(BROWSE_COLS * 3).enumerate() {
                let col = pos % BROWSE_COLS;
                let row = pos / BROWSE_COLS;
                let tile = Rect {
                    x: panel.x + 20.0 + col as f32 * (BROWSE_TILE_W + 12.0),
                    y: grid_y + row as f32 * BROWSE_TILE_H,
                    w: BROWSE_TILE_W,
                    h: BROWSE_TILE_H,
                };
                let hovered = self.brain.is_open() && contains(tile, cursor.x, cursor.y);
                let selected = pos == self.browse_selection;
                if pressed && hovered {
                    chosen_action = Some(PivotAction::LaunchApp(app_index));
                }
                let entry = &self.brain.apps()[app_index];
                let icon = self.entry_icon(entry);
                let name = ellipsize(frame, &entry.name, type_scale.caption, BROWSE_TILE_W - 8.0);

                frame.place(
                    &format!("tessera-pivot-browse-{pos}"),
                    &chrome_place(
                        tile,
                        LayoutOpts {
                            bg: if selected {
                                design.colors.application_surface_active
                            } else if hovered {
                                design.colors.application_surface_hover
                            } else {
                                Color::TRANSPARENT
                            },
                            radius: 8.0,
                            pad: 0.0,
                            ..surface_layout()
                        },
                    ),
                    |frame| {
                        frame.column_ex(
                            &LayoutOpts {
                                width: tile.w,
                                height: tile.h,
                                gap: 6.0,
                                pad: 6.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |frame| {
                                render_icon(frame, icon, progress);
                                frame.label_compact_sized(&name, type_scale.caption);
                            },
                        );
                    },
                );
            }
        }

        frame.set_opacity(1.0);

        if let Some(act) = chosen_action {
            self.execute_action(act, out);
            self.anim_active = true;
        }

        self.prev_down = down;
    }
}

fn render_icon(frame: &mut Frame, icon: Option<*mut c_void>, progress: f32) {
    let size = ICON_SIZE * (0.92 + progress.clamp(0.0, 1.0) * 0.08);
    frame.centered(ICON_SIZE, ICON_SIZE, |frame| match icon {
        Some(pointer) => unsafe {
            frame.image(pointer as *mut lens::sys::flux_image, size, size);
        },
        None => {
            frame.column_ex(
                &LayoutOpts {
                    width: size,
                    height: size,
                    bg: Color::rgba(78, 88, 120, 230),
                    radius: 9.0,
                    ..Default::default()
                },
                |frame| {
                    frame.centered(size, size, |frame| {
                        frame.icon(Icon::FileText, 20.0);
                    });
                },
            );
        }
    });
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
    fn category_matching_and_i18n_works() {
        let dev_app = make_app("code.desktop", "Code", &["Development", "IDE"]);
        let media_app = make_app("vlc.desktop", "VLC", &["AudioVideo", "Player"]);
        let sys_app = make_app("settings.desktop", "Settings", &["System", "Settings"]);

        assert!(AppCategory::All.matches(&dev_app));
        assert!(AppCategory::Development.matches(&dev_app));
        assert!(!AppCategory::Media.matches(&dev_app));

        assert!(AppCategory::Media.matches(&media_app));
        assert!(AppCategory::System.matches(&sys_app));

        let en = Localizer::new("en-US");
        let zh = Localizer::new("zh-CN");
        assert_eq!(en.text(AppCategory::Development.message()), "Dev");
        assert_eq!(zh.text(AppCategory::Development.message()), "开发");
        assert_eq!(en.text(AppCategory::All.message()), "All");
        assert_eq!(zh.text(AppCategory::All.message()), "全部");
    }

    #[test]
    fn unified_search_includes_math_commands_and_agent() {
        let mut pivot = Pivot::new();
        pivot.update_app_catalog(&AppCatalog {
            apps: vec![make_app("firefox.desktop", "Firefox", &["Network"])],
            ..Default::default()
        });
        let i18n = Localizer::new("en-US");

        // 1. Math query
        let math_items = pivot.build_search_items("1920 * 1080", &[], &i18n);
        assert!(!math_items.is_empty());
        assert_eq!(math_items[0].title, "= 2073600");

        // 2. Command query
        let cmd_items = pivot.build_search_items("lock", &[], &i18n);
        assert!(
            cmd_items
                .iter()
                .any(|i| matches!(i.action, PivotAction::SystemCommand(SystemCmd::Lock)))
        );

        // 3. Agent fallback
        let agent_items = pivot.build_search_items("how to write a rust macro", &[], &i18n);
        assert!(
            agent_items
                .iter()
                .any(|i| matches!(i.action, PivotAction::AskAgent(_)))
        );

        // 4. Live Window Match
        let mut test_win = Window::new(WindowId(42));
        test_win.title = Some("Compiler Error Discussion - Slack".into());
        test_win.app_id = Some("slack".into());
        let win_items = pivot.build_search_items("slack", &[test_win], &i18n);
        assert!(
            win_items
                .iter()
                .any(|i| matches!(i.action, PivotAction::FocusWindow(WindowId(42))))
        );

        // 5. Execution dispatches clipboard copy & agent prompt
        let mut events = ChromeEvents::default();
        pivot.execute_action(PivotAction::CopyCalculation("2073600".into()), &mut events);
        assert_eq!(events.clipboard_copy.as_deref(), Some("2073600"));

        pivot.execute_action(PivotAction::AskAgent("summarize repo".into()), &mut events);
        assert_eq!(events.agent_prompt.as_deref(), Some("summarize repo"));
    }
}
