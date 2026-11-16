//! The Settings window's model and layout, independent of Wayland, the GPU,
//! and the IPC transport.
//!
//! [`SettingsHost`] owns the module registry, the authoritative snapshot, the
//! navigation and search state, and the outbox of intents waiting to be sent.
//! The binary feeds it snapshots and transport outcomes and drains its outbox;
//! everything here is plain state, so it is exercised by unit tests without a
//! compositor.

use lens::sys::lens_icon_id;
use lens::{Align, Color, Frame, LayoutOpts, TextBuf};
use tessera_design::{ControlCenterColors, Design, themes};
use tessera_desktop::settings::{SettingsAction, SettingsSnapshot};
use tessera_i18n::{Localizer, Message};

use crate::catalog::builtin_modules;
use crate::module::{
    ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata, ModuleRegistry,
};

/// Sidebar width at and above [`COMPACT_BREAKPOINT`].
const SIDEBAR_W: f32 = 248.0;
/// Below this window width the sidebar collapses to an icon rail.
pub const COMPACT_BREAKPOINT: f32 = 640.0;
/// Width of the collapsed icon rail.
const RAIL_W: f32 = 64.0;
/// The page column never stretches past a comfortable line length.
const PAGE_MAX_W: f32 = 760.0;
const SIDEBAR_ROW_H: f32 = 36.0;
const PAGE_PAD: f32 = 24.0;
/// Label of the search field, also used by the host to focus it (Ctrl+F).
pub const SEARCH_FIELD_LABEL: &str = "##settings-search";

/// Whether the compositor link is usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// The first snapshot has not arrived yet.
    Connecting,
    /// Snapshots arrive and intents are accepted.
    Connected,
    /// The link dropped; the worker is retrying. Edits are held, not lost.
    Lost { reason: String },
    /// Settings are readable but this executable may not change them (the
    /// compositor refused it the Settings scope).
    ReadOnly { reason: String },
}

/// A transient message above the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub detail: String,
}

/// One queued intent with the snapshot revision it was drafted against.
pub type Outgoing = (Option<u64>, SettingsAction);

pub struct SettingsHost {
    modules: ModuleRegistry,
    snapshot: Option<SettingsSnapshot>,
    design: Design,
    selected: ModuleId,
    search: TextBuf,
    /// A Ctrl+F request waiting for the next sidebar build, where the search
    /// field's scoped Lens id is known.
    focus_search: bool,
    connection: Connection,
    notice: Option<Notice>,
    outbox: Vec<Outgoing>,
}

impl SettingsHost {
    /// A host showing `initial` when it names a module, otherwise the first
    /// available one.
    pub fn new(initial: Option<&str>) -> Self {
        Self::with_modules(builtin_modules(), initial)
    }

    pub fn with_modules(modules: ModuleRegistry, initial: Option<&str>) -> Self {
        let fallback = modules
            .metadata()
            .find(|module| module.availability == ModuleAvailability::Available)
            .or_else(|| modules.metadata().next())
            .map_or(ModuleId::new(""), |module| module.id);
        let selected = initial
            .and_then(|id| modules.resolve(id))
            .unwrap_or(fallback);
        Self {
            modules,
            snapshot: None,
            design: Design::dark(),
            selected,
            search: TextBuf::new(128, ""),
            focus_search: false,
            connection: Connection::Connecting,
            notice: None,
            outbox: Vec::new(),
        }
    }

    pub fn selected(&self) -> ModuleId {
        self.selected
    }

    pub fn design(&self) -> &Design {
        &self.design
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn snapshot(&self) -> Option<&SettingsSnapshot> {
        self.snapshot.as_ref()
    }

    /// Accessibility text scale from the authoritative preferences.
    pub fn text_scale(&self) -> f32 {
        self.snapshot
            .as_ref()
            .map_or(1.0, |snapshot| snapshot.preferences.text_scale as f32)
    }

    pub fn reduced_motion(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.preferences.reduced_motion)
    }

    /// Navigate to a deep-link target. Returns whether it resolved.
    pub fn navigate(&mut self, id: &str) -> bool {
        match self.modules.resolve(id) {
            Some(id) => {
                self.select(id);
                true
            }
            None => false,
        }
    }

    fn select(&mut self, id: ModuleId) {
        if self.selected != id {
            self.selected = id;
            self.notice = None;
        }
    }

    /// Fold in a newer authoritative snapshot. Older or equal revisions are
    /// ignored so a late reply cannot roll the page back.
    pub fn apply_snapshot(&mut self, snapshot: SettingsSnapshot) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.revision > snapshot.revision)
        {
            return;
        }
        self.design = Design::for_scheme(snapshot.preferences.color_scheme);
        self.modules.update_settings(&snapshot);
        self.snapshot = Some(snapshot);
        if !matches!(self.connection, Connection::ReadOnly { .. }) {
            self.connection = Connection::Connected;
        }
    }

    /// A connection was (re-)established, writable or read-only.
    pub fn linked(&mut self, read_only: Option<String>) {
        self.connection = match read_only {
            Some(reason) => Connection::ReadOnly { reason },
            None => Connection::Connected,
        };
    }

    pub fn connection_lost(&mut self, reason: String) {
        self.connection = Connection::Lost { reason };
    }

    /// A sent intent was refused by the compositor. The worker follows up with
    /// a fresh snapshot, which resets the module's draft.
    pub fn rejected(&mut self, detail: String) {
        self.notice = Some(Notice { detail });
    }

    /// Queue an intent, keeping only the newest draft of each kind: instant
    /// controls emit one intent per change while a slider drags, and only the
    /// final value needs to travel.
    pub fn queue(&mut self, action: SettingsAction) {
        let revision = self.snapshot.as_ref().map(|snapshot| snapshot.revision);
        self.outbox.retain(|(_, queued)| {
            std::mem::discriminant(queued) != std::mem::discriminant(&action)
        });
        self.outbox.push((revision, action));
    }

    /// Drain the intents to send. Nothing leaves while the link is down; the
    /// coalesced drafts wait for the reconnect. A read-only link still
    /// receives them, so each is refused visibly and its draft reset.
    pub fn take_outbox(&mut self) -> Vec<Outgoing> {
        match self.connection {
            Connection::Connected | Connection::ReadOnly { .. } => std::mem::take(&mut self.outbox),
            Connection::Connecting | Connection::Lost { .. } => Vec::new(),
        }
    }

    pub fn has_outbox(&self) -> bool {
        !self.outbox.is_empty()
    }

    /// Modules the sidebar lists for the current query, in registry order.
    pub fn visible_modules(&self, i18n: &Localizer) -> Vec<ModuleMetadata> {
        let query = self.search.as_str();
        self.modules
            .metadata()
            .filter(|module| module.matches(&query, i18n))
            .collect()
    }

    /// Select the first search result (Enter in the search field).
    pub fn activate_first_result(&mut self, i18n: &Localizer) -> bool {
        match self.visible_modules(i18n).first() {
            Some(module) => {
                self.select(module.id);
                true
            }
            None => false,
        }
    }

    /// Focus the search field on the next frame. The request is applied while
    /// the sidebar builds, because Lens widget ids are scoped to their
    /// container and cannot be resolved from outside the frame.
    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    /// Clear the search query (Escape). Returns whether there was one.
    pub fn clear_search(&mut self) -> bool {
        if self.search.as_str().is_empty() {
            return false;
        }
        self.search.set("");
        true
    }

    #[cfg(test)]
    fn set_search(&mut self, query: &str) {
        self.search.set(query);
    }

    #[cfg(test)]
    fn search_query(&self) -> String {
        self.search.as_str().into_owned()
    }

    /// Build one frame of the window at `size` logical pixels.
    pub fn render(&mut self, f: &mut Frame, size: (f32, f32), i18n: &Localizer) {
        let design = self.design;
        let palette = ControlCenterColors::for_scheme(design.scheme);
        let compact = size.0 < COMPACT_BREAKPOINT;
        let sidebar_w = if compact { RAIL_W } else { SIDEBAR_W };
        f.set_theme(themes::application(&design));
        f.row_ex(
            &LayoutOpts {
                width: size.0.max(1.0),
                height: size.1.max(1.0),
                bg: design.colors.application_surface,
                ..Default::default()
            },
            |f| {
                self.render_sidebar(f, (sidebar_w, size.1), compact, &palette, i18n);
                self.render_page(f, (size.0 - sidebar_w, size.1), &palette, i18n);
            },
        );
    }

    fn render_sidebar(
        &mut self,
        f: &mut Frame,
        size: (f32, f32),
        compact: bool,
        palette: &ControlCenterColors,
        i18n: &Localizer,
    ) {
        let design = self.design;
        let mut picked: Option<ModuleId> = None;
        f.column_ex(
            &LayoutOpts {
                width: size.0,
                height: size.1.max(1.0),
                pad: if compact { 8.0 } else { 12.0 },
                gap: 4.0,
                bg: palette.surface_recessed,
                cross: Align::Stretch,
                ..Default::default()
            },
            |f| {
                if !compact {
                    let submitted = f.textfield_placeholder(
                        SEARCH_FIELD_LABEL,
                        &mut self.search,
                        i18n.text(Message::SearchSettings),
                    ) && f.response().submitted;
                    if submitted {
                        self.activate_first_result(i18n);
                    }
                    if self.focus_search
                        && let Ok(label) = std::ffi::CString::new(SEARCH_FIELD_LABEL)
                    {
                        // Resolve the field's id under the sidebar's current
                        // id scope, while that scope is still on the stack.
                        let raw = f.as_raw();
                        // SAFETY: `raw` is live inside the frame, `label` is
                        // NUL-terminated, and `lens_set_focus` only records an
                        // id for the next build.
                        unsafe {
                            let id = lens::sys::lens_current_id(
                                raw as *const lens::sys::lens,
                                label.as_ptr(),
                            );
                            if id != 0 {
                                lens::sys::lens_set_focus(raw, id);
                            }
                        }
                    }
                    f.spacer(8.0);
                }
                self.focus_search = false;
                let visible = self.visible_modules(i18n);
                f.scroll("settings-sidebar-scroll", |f| {
                    f.column_ex(
                        &LayoutOpts {
                            gap: 2.0,
                            cross: Align::Stretch,
                            ..Default::default()
                        },
                        |f| {
                            if visible.is_empty() {
                                f.label_wrapped_sized(
                                    i18n.text(Message::NoMatchingSettings),
                                    design.typography.footnote,
                                    (size.0 - 24.0).max(1.0),
                                );
                                return;
                            }
                            let searching = !self.search.as_str().trim().is_empty();
                            for category in ModuleCategory::ALL {
                                let group: Vec<_> = visible
                                    .iter()
                                    .filter(|module| module.category == category)
                                    .collect();
                                if group.is_empty() {
                                    continue;
                                }
                                if !compact && !searching {
                                    f.spacer(6.0);
                                    let original = f.theme();
                                    f.set_theme(original.with_fg(palette.text_muted));
                                    f.label_sized(
                                        i18n.text(category.title()),
                                        design.typography.caption,
                                    );
                                    f.set_theme(original);
                                }
                                for module in group {
                                    if sidebar_row(
                                        f,
                                        module,
                                        self.selected == module.id,
                                        compact,
                                        palette,
                                        &design,
                                        i18n,
                                    ) {
                                        picked = Some(module.id);
                                    }
                                }
                            }
                        },
                    );
                });
            },
        );
        if let Some(id) = picked {
            self.select(id);
        }
    }

    fn render_page(
        &mut self,
        f: &mut Frame,
        size: (f32, f32),
        palette: &ControlCenterColors,
        i18n: &Localizer,
    ) {
        let design = self.design;
        let page_w = (size.0 - PAGE_PAD * 2.0).clamp(1.0, PAGE_MAX_W);
        let mut events = ModuleEvents::default();
        let mut dismiss_notice = false;
        f.column_ex(
            &LayoutOpts {
                width: size.0.max(1.0),
                height: size.1.max(1.0),
                pad: PAGE_PAD,
                gap: 16.0,
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                // Each page draws its own heading; the window adds only the
                // link state, and only when it is not simply connected.
                if let Some(text) = connection_text(&self.connection, i18n) {
                    f.row_ex(
                        &LayoutOpts {
                            width: page_w,
                            height: 24.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            let original = f.theme();
                            f.set_theme(original.with_fg(palette.text_muted));
                            f.icon_raw(lens_icon_id::LENS_ICON_INFO, 16.0);
                            f.label_sized(text, design.typography.footnote);
                            f.set_theme(original);
                        },
                    );
                }
                if let Some(notice) = &self.notice {
                    dismiss_notice = notice_banner(f, notice, page_w, &design, i18n);
                }
                f.flex(1.0);
                f.scroll("settings-page-scroll", |f| {
                    f.column_ex(
                        &LayoutOpts {
                            width: page_w,
                            gap: 12.0,
                            cross: Align::Stretch,
                            ..Default::default()
                        },
                        |f| {
                            if self.snapshot.is_none() {
                                f.label_sized(
                                    i18n.text(Message::ConnectingToDesktop),
                                    design.typography.body,
                                );
                                return;
                            }
                            self.modules
                                .render(self.selected, f, i18n, &design, &mut events);
                        },
                    );
                });
            },
        );
        if dismiss_notice {
            self.notice = None;
        }
        for action in events.actions {
            self.queue(action);
        }
    }
}

/// One sidebar entry. Returns whether it was clicked.
fn sidebar_row(
    f: &mut Frame,
    module: &ModuleMetadata,
    selected: bool,
    compact: bool,
    palette: &ControlCenterColors,
    design: &Design,
    i18n: &Localizer,
) -> bool {
    let title = i18n.text(module.title);
    let unavailable = module.availability == ModuleAvailability::BackendUnavailable;
    let (bg, fg, icon_fg) = if selected {
        (palette.selection_surface, palette.text, palette.accent)
    } else if unavailable {
        (Color::TRANSPARENT, palette.text_muted, palette.text_muted)
    } else {
        (Color::TRANSPARENT, palette.text, palette.text_muted)
    };
    let original = f.theme();
    let (response, _) = f.pressable_row(
        &format!("settings-nav-{}", module.id.as_str()),
        title,
        &LayoutOpts {
            height: SIDEBAR_ROW_H,
            pad: 10.0,
            gap: 10.0,
            radius: design.radii.control,
            cross: Align::Center,
            bg,
            ..Default::default()
        },
        |f, _| {
            f.set_theme(original.with_fg(icon_fg));
            f.icon_raw(module.icon, 18.0);
            if !compact {
                f.set_theme(original.with_fg(fg));
                f.label_compact_sized(title, design.typography.body);
            }
        },
    );
    f.set_theme(original);
    response.clicked && !selected
}

/// The rejection banner. Returns whether its dismiss button was pressed.
fn notice_banner(
    f: &mut Frame,
    notice: &Notice,
    width: f32,
    design: &Design,
    i18n: &Localizer,
) -> bool {
    let mut dismissed = false;
    f.row_ex(
        &LayoutOpts {
            width,
            pad: 12.0,
            gap: 10.0,
            cross: Align::Center,
            radius: design.radii.card,
            bg: Color::rgba(255, 159, 10, 40),
            border: Color::rgba(255, 159, 10, 120),
            border_width: design.strokes.hairline,
            ..Default::default()
        },
        |f| {
            f.icon_raw(lens_icon_id::LENS_ICON_ALERT_TRIANGLE, 18.0);
            f.column_ex(
                &LayoutOpts {
                    flex: 1.0,
                    gap: 2.0,
                    ..Default::default()
                },
                |f| {
                    f.label_compact_weighted(
                        i18n.text(Message::SettingsChangeRejected),
                        design.typography.label,
                        600.0,
                    );
                    f.label_wrapped_sized(
                        &notice.detail,
                        design.typography.footnote,
                        (width - 96.0).max(1.0),
                    );
                },
            );
            dismissed = f.icon_button_raw(lens_icon_id::LENS_ICON_X);
        },
    );
    dismissed
}

fn connection_text(connection: &Connection, i18n: &Localizer) -> Option<&'static str> {
    match connection {
        Connection::Connected => None,
        Connection::Connecting => Some(i18n.text(Message::ConnectingToDesktop)),
        Connection::Lost { .. } => Some(i18n.text(Message::SettingsConnectionLost)),
        Connection::ReadOnly { .. } => Some(i18n.text(Message::SettingsReadOnly)),
    }
}

#[cfg(test)]
mod tests {
    use tessera_desktop::settings::{DockSettings, IdleSettings};

    use super::*;

    fn snapshot(revision: u64) -> SettingsSnapshot {
        SettingsSnapshot {
            revision,
            ..SettingsSnapshot::default()
        }
    }

    fn connected() -> SettingsHost {
        let mut host = SettingsHost::new(None);
        host.apply_snapshot(snapshot(1));
        host
    }

    #[test]
    fn opens_on_the_deep_linked_module() {
        assert_eq!(
            SettingsHost::new(Some("power")).selected(),
            ModuleId::new("power")
        );
    }

    #[test]
    fn an_unknown_deep_link_falls_back_to_the_first_available_module() {
        assert_eq!(
            SettingsHost::new(Some("nope")).selected(),
            ModuleId::new("display")
        );
    }

    #[test]
    fn navigation_reports_whether_the_target_resolved() {
        let mut host = connected();
        assert!(host.navigate("Dock"));
        assert_eq!(host.selected(), ModuleId::new("dock"));
        assert!(!host.navigate("missing"));
        assert_eq!(host.selected(), ModuleId::new("dock"));
    }

    #[test]
    fn older_snapshots_never_roll_the_page_back() {
        let mut host = connected();
        host.apply_snapshot(snapshot(5));
        host.apply_snapshot(snapshot(3));
        assert_eq!(host.snapshot().map(|s| s.revision), Some(5));
    }

    #[test]
    fn the_outbox_keeps_the_newest_draft_of_each_kind() {
        let mut host = connected();
        let idle = |dim| SettingsAction::SetIdle {
            settings: IdleSettings {
                dim_after_seconds: dim,
                ..IdleSettings::default()
            },
        };
        host.queue(idle(10));
        host.queue(SettingsAction::SetDock {
            settings: DockSettings::default(),
        });
        host.queue(idle(20));
        let out = host.take_outbox();
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].1, SettingsAction::SetDock { .. }));
        assert_eq!(out[1], (Some(1), idle(20)));
        assert!(host.take_outbox().is_empty());
    }

    #[test]
    fn nothing_leaves_while_the_link_is_down_and_drafts_survive() {
        let mut host = connected();
        host.connection_lost("socket closed".into());
        host.queue(SettingsAction::SetDock {
            settings: DockSettings::default(),
        });
        assert!(host.take_outbox().is_empty());
        assert!(host.has_outbox());
        host.apply_snapshot(snapshot(2));
        assert_eq!(host.take_outbox().len(), 1);
    }

    #[test]
    fn a_read_only_link_survives_snapshots_and_still_drains_drafts() {
        let mut host = SettingsHost::new(None);
        host.linked(Some("scope refused".into()));
        host.apply_snapshot(snapshot(1));
        assert_eq!(
            host.connection(),
            &Connection::ReadOnly {
                reason: "scope refused".into()
            }
        );
        host.queue(SettingsAction::SetDock {
            settings: DockSettings::default(),
        });
        assert_eq!(host.take_outbox().len(), 1, "the link refuses it visibly");
        host.linked(None);
        assert_eq!(host.connection(), &Connection::Connected);
    }

    #[test]
    fn focus_search_targets_the_scoped_text_field() {
        let i18n = Localizer::new("en-US");
        let mut host = connected();
        let mut ui = lens::Ui::headless().unwrap();
        let size = (960.0, 680.0);
        let idle = lens::Input::new(size, 0.016);
        host.focus_search();
        ui.frame(&idle, |f| host.render(f, size, &i18n));
        let mut typed = lens::Input::new(size, 0.016);
        typed.set_text("dock");
        ui.frame(&typed, |f| host.render(f, size, &i18n));
        assert_eq!(host.search_query(), "dock");
    }

    #[test]
    fn search_filters_by_title_id_and_keyword() {
        let i18n = Localizer::new("en-US");
        let mut host = connected();
        host.set_search("mon");
        let ids: Vec<_> = host
            .visible_modules(&i18n)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert!(ids.contains(&"display"), "{ids:?}");
        host.set_search("zzz-nothing");
        assert!(host.visible_modules(&i18n).is_empty());
        assert!(!host.activate_first_result(&i18n));
        assert!(host.clear_search());
        assert_eq!(host.visible_modules(&i18n).len(), 8);
        assert!(!host.clear_search());
    }

    #[test]
    fn enter_in_search_opens_the_first_result() {
        let i18n = Localizer::new("en-US");
        let mut host = connected();
        host.set_search("dock");
        assert!(host.activate_first_result(&i18n));
        assert_eq!(host.selected(), ModuleId::new("dock"));
    }

    #[test]
    fn a_rejection_raises_a_notice_that_navigation_clears() {
        let mut host = connected();
        host.rejected("revision conflict".into());
        assert_eq!(
            host.notice().map(|n| n.detail.as_str()),
            Some("revision conflict")
        );
        host.navigate("power");
        assert!(host.notice().is_none());
    }

    #[test]
    fn the_snapshot_drives_the_scheme_and_text_scale() {
        let mut host = SettingsHost::new(None);
        let mut light = snapshot(1);
        light.preferences.color_scheme = tessera_desktop::settings::ColorScheme::Light;
        light.preferences.text_scale = 1.25;
        host.apply_snapshot(light);
        assert!(host.design().is_light());
        assert!((host.text_scale() - 1.25).abs() < f32::EPSILON);
    }
}
