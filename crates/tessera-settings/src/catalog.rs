//! The built-in module set, in stable sidebar order.

use lens::sys::lens_icon_id;
use tessera_i18n::Message;

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleId, ModuleMetadata, ModuleRegistry,
};
use crate::modules::{
    AccessibilityModule, AppearanceModule, DisplayModule, DockModule, InputModule, PowerModule,
    UnavailableModule,
};

/// Construct the built-in module set in stable navigation order.
pub fn builtin_modules() -> ModuleRegistry {
    let mut modules = ModuleRegistry::default();
    modules.register(DisplayModule::new());
    modules.register(InputModule::new());
    modules.register(PowerModule::new());
    modules.register(AppearanceModule::new());
    modules.register(DockModule::new());
    modules.register(AccessibilityModule::new());
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("users"),
            title: Message::UserAccounts,
            icon: lens_icon_id::LENS_ICON_USERS,
            category: ModuleCategory::System,
            keywords: &["users", "accounts", "password", "avatar", "administrator"],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::UserAccountsDescription,
    ));
    modules.register(UnavailableModule::new(
        ModuleMetadata {
            id: ModuleId::new("window-rules"),
            title: Message::WindowRules,
            icon: lens_icon_id::LENS_ICON_GRID,
            category: ModuleCategory::System,
            keywords: &["window", "rules", "application", "workspace", "floating"],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::BackendUnavailable,
        },
        Message::WindowRulesDescription,
    ));
    modules
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn built_in_modules_have_stable_routes() {
        let modules = builtin_modules();
        assert_eq!(
            modules
                .metadata()
                .map(|module| module.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "display",
                "input",
                "power",
                "appearance",
                "dock",
                "accessibility",
                "users",
                "window-rules"
            ]
        );
    }

    #[test]
    fn every_built_in_registration_is_accepted() {
        // `register` refuses duplicates instead of panicking; the built-in set
        // must never trip that refusal.
        let modules = builtin_modules();
        let ids: HashSet<_> = modules.metadata().map(|module| module.id).collect();
        assert_eq!(ids.len(), modules.len());
        assert_eq!(modules.len(), 8);
    }

    #[test]
    fn a_duplicate_registration_is_refused_not_shadowing() {
        let mut modules = builtin_modules();
        let before = modules.len();
        let accepted = modules.register(UnavailableModule::new(
            ModuleMetadata {
                id: ModuleId::new("display"),
                title: Message::Display,
                icon: lens_icon_id::LENS_ICON_MONITOR,
                category: ModuleCategory::Hardware,
                keywords: &[],
                apply_policy: ApplyPolicy::Explicit,
                availability: ModuleAvailability::BackendUnavailable,
            },
            Message::DisplayDescription,
        ));
        assert!(!accepted);
        assert_eq!(modules.len(), before);
        assert_eq!(
            modules
                .get(ModuleId::new("display"))
                .map(|m| m.availability),
            Some(ModuleAvailability::Available)
        );
    }

    #[test]
    fn deep_links_resolve_case_insensitively() {
        let modules = builtin_modules();
        assert_eq!(modules.resolve("Display"), Some(ModuleId::new("display")));
        assert_eq!(
            modules.resolve(" window-rules "),
            Some(ModuleId::new("window-rules"))
        );
        assert_eq!(modules.resolve("nonexistent"), None);
    }

    #[test]
    fn every_category_hosts_at_least_one_module() {
        let modules = builtin_modules();
        for category in ModuleCategory::ALL {
            assert!(
                modules.metadata().any(|module| module.category == category),
                "{category:?} would render an empty sidebar group"
            );
        }
    }
}
