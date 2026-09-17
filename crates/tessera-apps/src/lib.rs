//! Desktop application discovery, icon-theme lookup, process launching, and
//! system intent search for Tessera (ADR-0160).

pub mod entries;
pub mod icons;
pub mod launcher;
pub mod search;

// Top-level re-exports for unified application lifecycle consumption:
pub use entries::{
    AppsError, Entry, enumerate, enumerate_in, enumerate_with_theme,
    enumerate_with_theme_and_scale, expand_exec, expand_exec_tokens, parse_str,
};
pub use icons::{
    DEFAULT_ICON_SIZE, DEFAULT_ICON_THEME, icon_search_bases, resolve_icon, resolve_icon_scaled,
    xdg_data_dirs,
};
pub use launcher::{
    InteractionDomainResourceLimits, InteractionDomainSandbox, LaunchOpts, LaunchSource,
    ManagedLaunch, launch, launch_managed, prepare_interaction_domain_host,
};
pub use search::{
    IntentAction, IntentEngine, IntentItem, IntentKind, QueryMode, SystemCmd, eval_math,
};
