//! Desktop session composition and application use cases.
use std::os::fd::AsRawFd;

use tessera_platform::Backend;
use tessera_platform::drm::DrmError;
use tessera_platform::host::{BackendKind, HardwareCursor, Host, HostError};

mod cursor;
mod runtime;

pub use runtime::run;

/// Persistent (level) input state carried across frames. Per-frame edges
/// (mouse pressed/released, scroll, text, key events) are *not* held here;
/// they are built fresh each frame from backend events and live only for the
/// iteration. This matches lens's contract that the host owns edge derivation
/// (see the `lens::Input` docstring) and mirrors iris's wayland host
/// `drain_input` pattern. Keeping level state separate from per-frame edges
/// guarantees a press/release edge can never leak into the next frame and
/// trigger phantom clicks in immediate-mode widgets.
#[derive(Default)]
struct InputAccumulator {
    cursor: (f32, f32),
    mouse_down: [bool; 3],
    display_size: (f32, f32),
}

impl InputAccumulator {
    /// Mirror of `lens::Input::set_mouse_down` so callers can update the
    /// level state alongside the per-frame snapshot through the same
    /// `lens::MouseButton` key.
    fn set_mouse_down(&mut self, b: lens::MouseButton, down: bool) {
        let idx = match b {
            lens::MouseButton::Left => 0,
            lens::MouseButton::Right => 1,
            lens::MouseButton::Middle => 2,
        };
        self.mouse_down[idx] = down;
    }
}

mod engine;
mod registry;

mod host_system;
