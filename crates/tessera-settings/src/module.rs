//! The settings-module contract and its registry.
//!
//! A module owns one settings domain: its page, its local draft state, and the
//! typed intents it emits. The host — the Settings application — owns
//! discovery, navigation, search, and the transport that carries intents to the
//! compositor. Modules never read configuration, probe hardware, call system
//! services, or pick a transport.
//!
//! Modules are linked statically. Rust has no stable dynamic-library ABI, so a
//! future third-party module boundary is a versioned process protocol, never an
//! in-process `.so`; the metadata and state model here are what such a
//! protocol would carry.

use optics_lens::Frame;
use optics_lens::sys::lens_icon_id;
use tessera_design::Design;
use tessera_desktop::settings::{SettingsAction, SettingsSnapshot};
use tessera_i18n::{Localizer, Message};

/// Stable identifier for a settings module; also the deep-link segment
/// (`tessera-settings display`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(&'static str);

impl ModuleId {
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Sidebar group. Categories are presentation metadata, never owners of
/// settings or persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleCategory {
    Hardware,
    Personalization,
    System,
}

impl ModuleCategory {
    /// Sidebar order.
    pub const ALL: [ModuleCategory; 3] = [
        ModuleCategory::Hardware,
        ModuleCategory::Personalization,
        ModuleCategory::System,
    ];

    pub fn title(self) -> Message {
        match self {
            ModuleCategory::Hardware => Message::SettingsCategoryHardware,
            ModuleCategory::Personalization => Message::SettingsCategoryPersonalization,
            ModuleCategory::System => Message::SettingsCategorySystem,
        }
    }
}

/// Whether edits take effect as the user changes a control or only after an
/// explicit apply inside the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPolicy {
    Instant,
    Explicit,
}

/// Whether the module's authoritative backend is usable in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleAvailability {
    Available,
    BackendUnavailable,
}

/// Immutable discovery metadata owned by one module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleMetadata {
    pub id: ModuleId,
    pub title: Message,
    pub icon: lens_icon_id,
    pub category: ModuleCategory,
    /// Lower-case search terms beyond the localized title.
    pub keywords: &'static [&'static str],
    pub apply_policy: ApplyPolicy,
    pub availability: ModuleAvailability,
}

impl ModuleMetadata {
    /// Whether this module answers a search query. Matching is a
    /// case-insensitive substring test over the localized title, the stable
    /// id, and the keywords; an empty query matches everything.
    pub fn matches(&self, query: &str, i18n: &Localizer) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        i18n.text(self.title).to_lowercase().contains(&query)
            || self.id.as_str().contains(&query)
            || self.keywords.iter().any(|keyword| keyword.contains(&query))
    }
}

/// Typed intents emitted by a settings page during one frame. The host
/// decides how they travel; modules never touch a transport.
#[derive(Debug, Default)]
pub struct ModuleEvents {
    pub actions: Vec<SettingsAction>,
}

/// One independently stateful settings page.
pub trait SettingsModule {
    fn metadata(&self) -> ModuleMetadata;

    fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        out: &mut ModuleEvents,
    );

    /// Replace the module's authoritative snapshot. Modules keep local editor
    /// state only while it is dirty; each decides how a newer snapshot
    /// reconciles with its draft.
    fn update_settings(&mut self, snapshot: &SettingsSnapshot);
}

/// Ordered module registry used for navigation, search, and deep links.
#[derive(Default)]
pub struct ModuleRegistry {
    modules: Vec<Box<dyn SettingsModule>>,
}

impl ModuleRegistry {
    /// Register a module. A second module claiming an id that is already
    /// registered is refused (and logged) rather than shadowing the first:
    /// deep links and navigation must resolve to exactly one page.
    pub fn register(&mut self, module: impl SettingsModule + 'static) -> bool {
        let id = module.metadata().id;
        if self.contains(id) {
            log::error!("settings: duplicate module id {:?} refused", id.as_str());
            return false;
        }
        self.modules.push(Box::new(module));
        true
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn metadata(&self) -> impl Iterator<Item = ModuleMetadata> + '_ {
        self.modules.iter().map(|module| module.metadata())
    }

    pub fn get(&self, id: ModuleId) -> Option<ModuleMetadata> {
        self.metadata().find(|module| module.id == id)
    }

    pub fn contains(&self, id: ModuleId) -> bool {
        self.get(id).is_some()
    }

    /// Resolve a deep-link segment, case-insensitively.
    pub fn resolve(&self, id: &str) -> Option<ModuleId> {
        let id = id.trim();
        self.metadata()
            .find(|module| module.id.as_str().eq_ignore_ascii_case(id))
            .map(|module| module.id)
    }

    /// Render one module's page. Returns `false` when no module has `id`.
    pub fn render(
        &mut self,
        id: ModuleId,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        out: &mut ModuleEvents,
    ) -> bool {
        let Some(module) = self
            .modules
            .iter_mut()
            .find(|module| module.metadata().id == id)
        else {
            return false;
        };
        module.render(frame, i18n, design, out);
        true
    }

    pub fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        for module in &mut self.modules {
            module.update_settings(snapshot);
        }
    }
}
