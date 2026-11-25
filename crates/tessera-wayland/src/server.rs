mod clipboard;
pub mod input;
mod interaction_domain;
mod launch;
mod lifecycle;
mod scene;
mod window_manager;

pub use clipboard::ClipboardError;
pub(crate) use clipboard::queue_owned_clipboard_write;
pub use input::accessibility::{FilterOutcome, KeyboardAccessibilityFilter, StickyState};
pub(crate) use launch::{PendingLaunchPlacement, take_pending_launch_placement};
#[cfg(test)]
pub(crate) use scene::client_cursor_surface_from_rec;
pub use scene::{ClientCursorSurface, DesktopFrameSets, PresentedSurfaces};
