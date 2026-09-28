//! Headless state machine for Pivot (ADR-0157, ADR-0174).
//!
//! Owns application filtering, query state, category selections, and action execution
//! with zero graphical, GPU, or Lens dependencies.

use crate::component::{ChromeEvents, Message};
use tessera_desktop::app::{BuiltInApplication, Entry};
use tessera_desktop::launcher::{Launch, Launcher as SearchBrain};
use tessera_desktop::system::SystemAction;
use tessera_launch_services::{IntentAction, SystemCmd};

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

/// Action to execute when a search item is activated.
pub type PivotAction = IntentAction;

/// Headless state for Pivot search, catalog, and action routing.
pub struct PivotState {
    pub brain: SearchBrain,
    pub active_category: AppCategory,
    pub browse_selection: usize,
    pub search_selection: usize,
    pub pinned_ids: Vec<String>,
}

impl Default for PivotState {
    fn default() -> Self {
        Self::new()
    }
}

impl PivotState {
    /// Construct a new empty Pivot state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            brain: SearchBrain::new(Vec::new()),
            active_category: AppCategory::All,
            browse_selection: 0,
            search_selection: 0,
            pinned_ids: Vec::new(),
        }
    }

    /// Toggle Pivot open or closed.
    pub fn toggle(&mut self) {
        if self.brain.is_open() {
            self.close();
        } else {
            self.open();
        }
    }

    /// Open Pivot and reset selection indices.
    pub fn open(&mut self) {
        self.brain.open();
        self.browse_selection = 0;
        self.search_selection = 0;
    }

    /// Close Pivot and reset selection indices.
    pub fn close(&mut self) {
        self.brain.close();
        self.browse_selection = 0;
        self.search_selection = 0;
    }

    /// Whether Pivot is currently logically open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.brain.is_open()
    }

    /// The current search query string.
    #[must_use]
    pub fn query(&self) -> &str {
        self.brain.query()
    }

    /// Filtered indices for an application category.
    #[must_use]
    pub fn category_indices(&self, category: AppCategory) -> Vec<usize> {
        self.brain
            .apps()
            .iter()
            .enumerate()
            .filter(|(_, entry)| category.matches(entry))
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Update application catalog and pinned identities.
    pub fn update_apps(&mut self, apps: Vec<Entry>, pinned_ids: Vec<String>) {
        self.brain.replace_apps(apps);
        self.pinned_ids = pinned_ids;
    }

    /// Execute a chosen PivotAction, mutating system events and closing Pivot.
    pub fn execute_action(&mut self, action: PivotAction, out: &mut ChromeEvents) {
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

    /// Emit a launch outcome into ChromeEvents.
    pub fn emit(outcome: Option<Launch>, out: &mut ChromeEvents) {
        match outcome {
            Some(Launch::Spawn(entry)) => out.activate_entry(*entry),
            Some(Launch::Focus(window)) => out.clicked = Some(window),
            Some(Launch::BuiltIn(app)) => out.open_builtin = Some(app),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_app(id: &str, name: &str, cat: &[&str]) -> Entry {
        Entry {
            id: id.to_string(),
            name: name.to_string(),
            categories: cat.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn headless_state_lifecycle_and_category_filtering() {
        let mut state = PivotState::new();
        assert!(!state.is_open());

        state.toggle();
        assert!(state.is_open());

        state.update_apps(
            vec![
                make_app("editor.desktop", "Code Editor", &["Development"]),
                make_app("player.desktop", "Media Player", &["AudioVideo"]),
            ],
            vec![],
        );

        assert_eq!(state.category_indices(AppCategory::All).len(), 2);
        assert_eq!(state.category_indices(AppCategory::Development).len(), 1);
        assert_eq!(state.category_indices(AppCategory::Media).len(), 1);
        assert_eq!(state.category_indices(AppCategory::Network).len(), 0);

        state.close();
        assert!(!state.is_open());
    }

    #[test]
    fn headless_state_action_dispatch() {
        let mut state = PivotState::new();
        state.open();

        let mut events = ChromeEvents::default();
        state.execute_action(
            PivotAction::SystemCommand(SystemCmd::Lock),
            &mut events,
        );
        assert!(events.lock);
        assert!(!state.is_open());

        state.open();
        state.execute_action(
            PivotAction::CopyCalculation("42".into()),
            &mut events,
        );
        assert_eq!(events.clipboard_copy.as_deref(), Some("42"));
        assert!(!state.is_open());
    }
}
