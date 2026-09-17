//! List pickers, candidate selection, and scroll-window calculations.

use std::time::Duration;

/// Standard duration threshold for classifying consecutive presses as a double-click.
pub const DEFAULT_DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(400);

/// Standard number of rows scrolled per wheel detent.
pub const DEFAULT_WHEEL_SCROLL_ROWS: f32 = 3.0;

/// Standard row height for list picker candidate items.
pub const DEFAULT_PICKER_ROW_HEIGHT: f32 = 36.0;
