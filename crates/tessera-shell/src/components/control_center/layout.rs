//! Sheet geometry: where the Control Center's two cards sit on an output.
//!
//! The Control Center is a sheet anchored to the right edge: a controls card
//! and a notifications card. Wide outputs place them side by side
//! (notifications to the left of controls, both top-aligned); narrow outputs
//! stack notifications under controls. Each card is sized to its content and
//! capped by the space the shell's persistent decorations leave free.
//!
//! Everything here is a pure function of the display, the reserved edges, and
//! the cards' natural heights, so hit-testing, damage, and painting all read
//! one answer, and the behaviour is unit-tested without a GPU.

use lens::Rect;

use crate::component::Reserved;

/// Controls card width.
pub(super) const CONTROLS_W: f32 = 380.0;
/// Notifications card width.
pub(super) const NOTIFICATIONS_W: f32 = 360.0;
/// Space between the two cards.
pub(super) const CARD_GAP: f32 = 12.0;
/// Space between a card and the output edge (beyond any reserved band).
pub(super) const EDGE_MARGIN: f32 = 12.0;
/// Inner padding of a card.
pub(super) const CARD_PAD: f32 = 16.0;
/// Vertical rhythm between a card's sections.
pub(super) const SECTION_GAP: f32 = 12.0;
/// Outputs at least this wide show both cards side by side.
pub(super) const TWO_COLUMN_MIN_W: f32 = 1024.0;
/// A stacked notifications card shorter than this is not worth drawing.
pub(super) const MIN_STACKED_NOTIFICATIONS_H: f32 = 120.0;
/// Stacked controls never shrink below this to make room for notifications.
pub(super) const MIN_STACKED_CONTROLS_H: f32 = 360.0;
/// Distance the cards travel while revealing.
pub(super) const REVEAL_SLIDE: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SheetLayout {
    pub controls: Rect,
    /// Zero-sized when the output has no room for it.
    pub notifications: Rect,
    pub two_column: bool,
}

impl SheetLayout {
    pub fn notifications_visible(&self) -> bool {
        self.notifications.w > 0.0 && self.notifications.h > 0.0
    }

    /// Whether `(x, y)` lands on either card.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        inside(self.controls, x, y)
            || (self.notifications_visible() && inside(self.notifications, x, y))
    }

    /// Every pixel the cards can touch: the cards, their elevation halo
    /// (`halo` on every side), and the strip a sliding reveal crosses.
    pub fn footprint(&self, halo: f32) -> Vec<Rect> {
        let mut rects = vec![slid(self.controls, halo)];
        if self.notifications_visible() {
            rects.push(slid(self.notifications, halo));
        }
        rects
    }
}

fn inside(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

/// A card grown by its halo and widened to the right by the reveal slide,
/// rounded outward to whole pixels so a fractional edge is never
/// under-covered.
fn slid(rect: Rect, halo: f32) -> Rect {
    let x0 = (rect.x - halo).floor();
    let y0 = (rect.y - halo).floor();
    let x1 = (rect.x + rect.w + REVEAL_SLIDE + halo).ceil();
    let y1 = (rect.y + rect.h + halo).ceil();
    Rect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0.0),
        h: (y1 - y0).max(0.0),
    }
}

/// Resolve the sheet for one output.
pub(super) fn sheet_layout(
    display: (f32, f32),
    reserved: Reserved,
    controls_natural_h: f32,
    notifications_natural_h: f32,
) -> SheetLayout {
    let left = EDGE_MARGIN + reserved.left as f32;
    let right = (display.0 - EDGE_MARGIN - reserved.right as f32).max(left + 1.0);
    let top = EDGE_MARGIN + reserved.top as f32;
    let bottom = (display.1 - EDGE_MARGIN - reserved.bottom as f32).max(top + 1.0);
    let available_w = right - left;
    let available_h = bottom - top;

    let controls_w = CONTROLS_W.min(available_w);
    let two_column =
        display.0 >= TWO_COLUMN_MIN_W && available_w >= CONTROLS_W + CARD_GAP + NOTIFICATIONS_W;

    if two_column {
        let controls = Rect {
            x: right - controls_w,
            y: top,
            w: controls_w,
            h: controls_natural_h.min(available_h).max(1.0),
        };
        let notifications = Rect {
            x: controls.x - CARD_GAP - NOTIFICATIONS_W,
            y: top,
            w: NOTIFICATIONS_W,
            h: notifications_natural_h.min(available_h).max(1.0),
        };
        return SheetLayout {
            controls,
            notifications,
            two_column,
        };
    }

    // Stacked: controls come first. They give up height to the notifications
    // below them only down to a floor, so a short output keeps its controls
    // usable and simply has no room for the notification card.
    let controls_floor = controls_natural_h.min(MIN_STACKED_CONTROLS_H);
    let squeezed = (available_h - MIN_STACKED_NOTIFICATIONS_H - CARD_GAP).max(controls_floor);
    let controls_h = controls_natural_h.min(available_h).min(squeezed).max(1.0);
    let controls = Rect {
        x: right - controls_w,
        y: top,
        w: controls_w,
        h: controls_h,
    };
    let below = controls.y + controls.h + CARD_GAP;
    let notifications_h = notifications_natural_h.min(bottom - below);
    let notifications = if notifications_h >= MIN_STACKED_NOTIFICATIONS_H {
        Rect {
            x: controls.x,
            y: below,
            w: controls_w,
            h: notifications_h,
        }
    } else {
        Rect {
            x: controls.x,
            y: below,
            w: 0.0,
            h: 0.0,
        }
    };
    SheetLayout {
        controls,
        notifications,
        two_column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: (f32, f32) = (1920.0, 1080.0);
    const NARROW: (f32, f32) = (800.0, 1280.0);

    fn dock_bottom(px: i32) -> Reserved {
        Reserved {
            bottom: px,
            ..Reserved::default()
        }
    }

    #[test]
    fn wide_outputs_place_notifications_left_of_controls() {
        let sheet = sheet_layout(WIDE, Reserved::default(), 500.0, 300.0);
        assert!(sheet.two_column);
        assert_eq!(sheet.controls.x + sheet.controls.w, WIDE.0 - EDGE_MARGIN);
        assert_eq!(sheet.controls.y, EDGE_MARGIN);
        assert_eq!(sheet.notifications.y, sheet.controls.y);
        assert_eq!(
            sheet.notifications.x + sheet.notifications.w + CARD_GAP,
            sheet.controls.x
        );
        assert_eq!(sheet.controls.h, 500.0);
        assert_eq!(sheet.notifications.h, 300.0);
    }

    #[test]
    fn cards_never_cross_a_reserved_band() {
        let sheet = sheet_layout(WIDE, dock_bottom(96), 5000.0, 5000.0);
        let floor = WIDE.1 - 96.0 - EDGE_MARGIN;
        assert!(sheet.controls.y + sheet.controls.h <= floor);
        assert!(sheet.notifications.y + sheet.notifications.h <= floor);
        let side = sheet_layout(
            WIDE,
            Reserved {
                right: 80,
                ..Reserved::default()
            },
            400.0,
            200.0,
        );
        assert_eq!(
            side.controls.x + side.controls.w,
            WIDE.0 - 80.0 - EDGE_MARGIN
        );
    }

    #[test]
    fn narrow_outputs_stack_notifications_under_controls() {
        let sheet = sheet_layout(NARROW, Reserved::default(), 520.0, 400.0);
        assert!(!sheet.two_column);
        assert_eq!(sheet.notifications.x, sheet.controls.x);
        assert_eq!(
            sheet.notifications.y,
            sheet.controls.y + sheet.controls.h + CARD_GAP
        );
        assert_eq!(sheet.notifications.h, 400.0);
    }

    #[test]
    fn stacked_controls_leave_room_for_a_usable_notification_card() {
        let short = (800.0, 700.0);
        let sheet = sheet_layout(short, Reserved::default(), 2000.0, 400.0);
        assert!(sheet.notifications_visible());
        assert!(sheet.notifications.h >= MIN_STACKED_NOTIFICATIONS_H);
        assert!(sheet.notifications.y + sheet.notifications.h <= short.1 - EDGE_MARGIN);
    }

    #[test]
    fn a_tiny_output_drops_the_notifications_card_instead_of_squashing_it() {
        let tiny = (420.0, 300.0);
        let sheet = sheet_layout(tiny, Reserved::default(), 600.0, 400.0);
        assert!(!sheet.notifications_visible());
        assert!(sheet.controls.w <= tiny.0 - EDGE_MARGIN * 2.0);
        assert!(sheet.controls.h <= tiny.1 - EDGE_MARGIN * 2.0);
    }

    #[test]
    fn the_footprint_covers_the_reveal_slide_and_whole_pixels() {
        let sheet = sheet_layout((1500.5, 900.0), Reserved::default(), 333.3, 222.2);
        let halo = 24.0;
        for (card, rect) in [sheet.controls, sheet.notifications]
            .into_iter()
            .zip(sheet.footprint(halo))
        {
            assert!(rect.x <= card.x - halo);
            assert!(rect.y <= card.y - halo);
            assert!(rect.x + rect.w >= card.x + card.w + REVEAL_SLIDE + halo);
            assert!(rect.y + rect.h >= card.y + card.h + halo);
            assert_eq!(rect.x.fract(), 0.0);
            assert_eq!(rect.w.fract(), 0.0);
        }
    }

    #[test]
    fn hit_testing_ignores_a_hidden_notifications_card() {
        let sheet = sheet_layout((420.0, 300.0), Reserved::default(), 600.0, 400.0);
        assert!(sheet.contains(sheet.controls.x + 1.0, sheet.controls.y + 1.0));
        assert!(!sheet.contains(sheet.notifications.x, sheet.notifications.y));
        assert!(!sheet.contains(1.0, 1.0));
    }
}
