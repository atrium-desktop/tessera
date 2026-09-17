//! Headless system intent and action dispatch engine (ADR-0157).
//!
//! Provides arithmetic calculation, system command palette resolution, live window
//! matching, application search aggregation, and natural language agent handoff
//! without requiring graphical rendering or UI context.

pub mod calc;
pub mod commands;

pub use calc::eval_math;
pub use commands::SystemCmd;

use tessera_desktop::app::Entry;
use tessera_desktop::window::Window;
use tessera_desktop::window::WindowId;
use tessera_i18n::Localizer;

/// Concrete action triggered by an intent result item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentAction {
    LaunchApp(usize),
    FocusWindow(WindowId),
    SystemCommand(SystemCmd),
    CopyCalculation(String),
    AskAgent(String),
}

/// Category/Domain of an intent item for icon and presentation mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentKind {
    App { app_id: String },
    Window { window_id: WindowId, app_id: String },
    System(SystemCmd),
    Math,
    Agent,
}

/// Lexical prefix mode defining the search target scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryMode {
    /// Standard progressive disclosure matching across all sources.
    Unified,
    /// Prefix `>`: Dedicated command palette mode.
    Command,
    /// Prefix `=`: Dedicated mathematical evaluation mode.
    Math,
    /// Prefix `?` or `/`: Dedicated AI Agent prompt delegation mode.
    Agent,
    /// Prefix `@`: Dedicated live window switching mode.
    Window,
}

impl QueryMode {
    /// Parse query mode and stripped rest from raw input string.
    #[must_use]
    pub fn parse_prefix(raw: &str) -> (Self, &str) {
        let trimmed = raw.trim_start();
        if let Some(rest) = trimmed.strip_prefix('>') {
            (Self::Command, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix('=') {
            (Self::Math, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix('?') {
            (Self::Agent, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix('/') {
            (Self::Agent, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix('@') {
            (Self::Window, rest.trim())
        } else {
            (Self::Unified, trimmed)
        }
    }
}

/// Headless search result item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: IntentKind,
    pub is_running: bool,
    pub action: IntentAction,
}

/// Headless query pipeline resolving intents from active system sources.
pub struct IntentEngine;

impl IntentEngine {
    /// Aggregate search items across all levels for the given `query`:
    /// 1. Level 3: Inline math calculation
    /// 2. Level 2: System command palette
    /// 3. Level 1+: Live window switcher matching window title or app_id
    /// 4. Level 1: Application catalog entries
    /// 5. Level 4: Agent prompt delegation
    pub fn query(
        query: &str,
        apps: &[Entry],
        filtered_app_indices: &[usize],
        is_app_running: impl Fn(usize) -> bool,
        windows: &[Window],
        i18n: &Localizer,
    ) -> Vec<IntentItem> {
        let (mode, q) = QueryMode::parse_prefix(query);
        if q.is_empty() && mode == QueryMode::Unified {
            return Vec::new();
        }

        let mut items = Vec::new();
        let q_lower = q.to_lowercase();

        // Mode 1: Dedicated Command Mode (Prefix `>`)
        if mode == QueryMode::Command {
            for cmd in SystemCmd::ALL {
                if q.is_empty() || cmd.matches(q) {
                    items.push(IntentItem {
                        title: cmd.title(i18n).to_string(),
                        subtitle: Some(cmd.subtitle(i18n).to_string()),
                        kind: IntentKind::System(cmd),
                        is_running: false,
                        action: IntentAction::SystemCommand(cmd),
                    });
                }
            }
            return items;
        }

        // Mode 2: Dedicated Math Mode (Prefix `=`)
        if mode == QueryMode::Math {
            if let Some(res) = eval_math(q) {
                items.push(IntentItem {
                    title: format!("= {res}"),
                    subtitle: Some("Calculation result · Press Enter to copy".into()),
                    kind: IntentKind::Math,
                    is_running: false,
                    action: IntentAction::CopyCalculation(res),
                });
            }
            return items;
        }

        // Mode 3: Dedicated Agent Mode (Prefix `?` or `/`)
        if mode == QueryMode::Agent {
            if !q.is_empty() {
                items.push(IntentItem {
                    title: format!("Ask Agent: \"{q}\""),
                    subtitle: Some("Dispatch instruction to Agent Broker & MCP Gateway".into()),
                    kind: IntentKind::Agent,
                    is_running: false,
                    action: IntentAction::AskAgent(q.to_string()),
                });
            }
            return items;
        }

        // Mode 4: Dedicated Window Switcher Mode (Prefix `@`)
        if mode == QueryMode::Window {
            for window in windows {
                let matches = q.is_empty()
                    || window
                        .title
                        .as_ref()
                        .is_some_and(|t| t.to_lowercase().contains(&q_lower))
                    || window
                        .app_id
                        .as_ref()
                        .is_some_and(|id| id.to_lowercase().contains(&q_lower));

                if matches {
                    let win_title = window.title.clone().unwrap_or_else(|| {
                        window.app_id.clone().unwrap_or_else(|| "Window".into())
                    });
                    let win_sub = format!(
                        "Window · {}",
                        window.app_id.as_deref().unwrap_or("Toplevel")
                    );
                    let app_id = window.app_id.clone().unwrap_or_default();

                    items.push(IntentItem {
                        title: win_title,
                        subtitle: Some(win_sub),
                        kind: IntentKind::Window {
                            window_id: window.id,
                            app_id,
                        },
                        is_running: true,
                        action: IntentAction::FocusWindow(window.id),
                    });
                }
            }
            return items;
        }

        // Standard Mode: Unified Progressive Disclosure
        // 1. Inline math calculator (Level 3)
        if let Some(res) = eval_math(q) {
            items.push(IntentItem {
                title: format!("= {res}"),
                subtitle: Some("Calculation result · Press Enter to copy".into()),
                kind: IntentKind::Math,
                is_running: false,
                action: IntentAction::CopyCalculation(res),
            });
        }

        // 2. System Command Palette (Level 2)
        for cmd in SystemCmd::ALL {
            if cmd.matches(q) {
                items.push(IntentItem {
                    title: cmd.title(i18n).to_string(),
                    subtitle: Some(cmd.subtitle(i18n).to_string()),
                    kind: IntentKind::System(cmd),
                    is_running: false,
                    action: IntentAction::SystemCommand(cmd),
                });
            }
        }

        // 3. Live Window Switcher (Level 1+)
        for window in windows {
            let title_match = window
                .title
                .as_ref()
                .is_some_and(|t| t.to_lowercase().contains(&q_lower));
            let app_match = window
                .app_id
                .as_ref()
                .is_some_and(|id| id.to_lowercase().contains(&q_lower));

            if title_match || app_match {
                let win_title = window
                    .title
                    .clone()
                    .unwrap_or_else(|| window.app_id.clone().unwrap_or_else(|| "Window".into()));
                let win_sub = format!(
                    "Window · {}",
                    window.app_id.as_deref().unwrap_or("Toplevel")
                );
                let app_id = window.app_id.clone().unwrap_or_default();

                items.push(IntentItem {
                    title: win_title,
                    subtitle: Some(win_sub),
                    kind: IntentKind::Window {
                        window_id: window.id,
                        app_id,
                    },
                    is_running: true,
                    action: IntentAction::FocusWindow(window.id),
                });
            }
        }

        // 4. Applications (Level 1)
        for &app_index in filtered_app_indices {
            if let Some(entry) = apps.get(app_index) {
                let subtitle = entry.generic_name.clone().or_else(|| entry.comment.clone());
                let running = is_app_running(app_index);
                items.push(IntentItem {
                    title: entry.name.clone(),
                    subtitle,
                    kind: IntentKind::App {
                        app_id: entry.id.clone(),
                    },
                    is_running: running,
                    action: IntentAction::LaunchApp(app_index),
                });
            }
        }

        // 5. Agent & MCP Gateway (Level 4)
        if q.len() >= 2 {
            items.push(IntentItem {
                title: format!("Ask Agent: \"{q}\""),
                subtitle: Some("Dispatch natural-language instruction to Agent Broker".into()),
                kind: IntentKind::Agent,
                is_running: false,
                action: IntentAction::AskAgent(q.to_string()),
            });
        }

        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_math_intent() {
        let i18n = Localizer::from_env();
        let items = IntentEngine::query("1920 * 1080", &[], &[], |_| false, &[], &i18n);
        assert!(!items.is_empty());
        assert_eq!(items[0].kind, IntentKind::Math);
        assert_eq!(
            items[0].action,
            IntentAction::CopyCalculation("2073600".into())
        );
    }

    #[test]
    fn query_system_command_intent() {
        let i18n = Localizer::from_env();
        let items = IntentEngine::query("lock", &[], &[], |_| false, &[], &i18n);
        assert!(!items.is_empty());
        assert_eq!(items[0].kind, IntentKind::System(SystemCmd::Lock));
        assert_eq!(
            items[0].action,
            IntentAction::SystemCommand(SystemCmd::Lock)
        );
    }

    #[test]
    fn query_window_and_agent_intents() {
        let i18n = Localizer::from_env();
        let mut win = Window::new(WindowId(42));
        win.title = Some("Slack - general".into());
        win.app_id = Some("slack".into());

        let items = IntentEngine::query("slack", &[], &[], |_| false, &[win], &i18n);
        let win_item = items
            .iter()
            .find(|it| matches!(it.kind, IntentKind::Window { .. }));
        assert!(win_item.is_some());

        let agent_item = items.iter().find(|it| matches!(it.kind, IntentKind::Agent));
        assert!(agent_item.is_some());
    }

    #[test]
    fn prefix_routing_modes() {
        let i18n = Localizer::from_env();
        let win = Window::new(WindowId(1));

        // 1. Prefix `>` lists all commands when rest is empty
        let all_cmds = IntentEngine::query(">", &[], &[], |_| false, &[], &i18n);
        assert_eq!(all_cmds.len(), SystemCmd::ALL.len());

        // 2. Prefix `>` filters commands
        let filtered_cmd = IntentEngine::query("> lock", &[], &[], |_| false, &[], &i18n);
        assert_eq!(filtered_cmd.len(), 1);
        assert_eq!(filtered_cmd[0].kind, IntentKind::System(SystemCmd::Lock));

        // 3. Prefix `=` forces math mode
        let math_items = IntentEngine::query("= 128 * 8", &[], &[], |_| false, &[], &i18n);
        assert_eq!(math_items.len(), 1);
        assert_eq!(math_items[0].title, "= 1024");

        // 4. Prefix `?` forces agent mode
        let agent_items =
            IntentEngine::query("? write a rust macro", &[], &[], |_| false, &[], &i18n);
        assert_eq!(agent_items.len(), 1);
        assert_eq!(agent_items[0].kind, IntentKind::Agent);

        // 5. Prefix `@` lists all windows when rest is empty
        let all_wins = IntentEngine::query("@", &[], &[], |_| false, &[win], &i18n);
        assert_eq!(all_wins.len(), 1);
    }
}
