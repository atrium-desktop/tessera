//! Pure geometry for the control-center tray shelf (ADR-0197).
//!
//! The shelf replaced a scrolling column. Everything here is a pure function of
//! `(anchor, item_count)` — never of the item list, the render pass, or the GPU
//! — so the overflow decision is unit-testable without a frame, and the layout
//! is identical in tests, previews and production ([INV-CHROME-05]).
//!
//! Three rules drive the arithmetic:
//!
//! - Capacity is derived, never capped ([INV-CHROME-02]): only the anchor's
//!   height bounds the slot count.
//! - There is no scroll viewport ([INV-CHROME-03]): overflow is a disclosure
//!   cell, not hidden content ([INV-CHROME-04]).
//! - The capsule is sized to the *occupied* slots, not to the available height,
//!   so a two-icon tray reads as a small pill rather than a tall empty column.

use super::{TRAY_CELL, TRAY_GAP, TRAY_PAD};
use lens::Rect;

/// The vertical pitch of one shelf slot: the cell plus the gap after it.
pub(super) const SLOT_PITCH: f32 = TRAY_CELL + TRAY_GAP;

/// A resolved shelf layout for one item count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TrayShelfLayout {
    /// The full anchor available to the shelf, bottom-left of the display.
    /// Bottom-anchored: the shelf grows upward from `anchor.y + anchor.h`.
    pub anchor: Rect,
    /// Whole slots the anchor admits. Never below 1.
    pub slots: usize,
    /// Cells drawn this frame, including the disclosure cell when present.
    /// `0` only when there are no items at all.
    pub visible: usize,
    /// Items hidden behind the disclosure cell. `0` when everything fits.
    pub overflow: usize,
    /// The recessed capsule, sized to `visible` slots and bottom-aligned
    /// inside the anchor.
    pub surface: Rect,
}

impl TrayShelfLayout {
    /// Whether the last drawn cell is the overflow disclosure.
    pub(super) fn has_disclosure(&self) -> bool {
        self.overflow > 0
    }

    /// Cell width for this shelf, given the anchor's width. Never narrower than
    /// a 24px target, never wider than the nominal cell.
    pub(super) fn cell_width(&self) -> f32 {
        TRAY_CELL.min(self.anchor.w - TRAY_PAD * 2.0).max(24.0)
    }

    /// The capsule's width: one cell plus the shelf's own inset on both sides.
    pub(super) fn surface_width(&self) -> f32 {
        (self.cell_width() + TRAY_PAD * 2.0).min(self.anchor.w)
    }

    /// The capsule, bottom-aligned in the anchor and sized to the occupied
    /// slots. Width comes from [`Self::surface_width`], so the capsule and the
    /// cells it contains cannot disagree about the horizontal metric.
    pub(super) fn surface_rect(&self) -> Rect {
        let h = if self.visible == 0 {
            0.0
        } else {
            shelf_height(self.visible).min(self.anchor.h)
        };
        Rect {
            x: self.anchor.x,
            y: self.anchor.y + self.anchor.h - h,
            w: self.surface_width(),
            h,
        }
    }
}

/// Height occupied by `slots` whole slots, including both paddings.
pub(super) fn shelf_height(slots: usize) -> f32 {
    let slots = slots.max(1) as f32;
    slots * TRAY_CELL + (slots - 1.0) * TRAY_GAP + TRAY_PAD * 2.0
}

/// Whole slots an anchor admits.
///
/// Inverts [`shelf_height`]: the largest `n` whose occupied height fits the
/// anchor. Clamped to at least one slot so an extremely short anchor still
/// yields a usable (if disclosing) shelf instead of a division-by-zero layout.
pub(super) fn shelf_slots(anchor_h: f32) -> usize {
    // `+ TRAY_GAP` because `n` cells have `n - 1` gaps between them.
    let fit = ((anchor_h - TRAY_PAD * 2.0 + TRAY_GAP) / SLOT_PITCH).floor();
    // A degenerate (zero, negative, or NaN) anchor still yields one usable
    // slot rather than a zero-slot layout that would hide the tray entirely.
    if fit < 1.0 { 1 } else { fit as usize }
}

/// The overflow disclosure popover's bounds for `hidden` disclosed items.
///
/// Pure so click-away hit-testing and the renderer cannot disagree about where
/// the popover is. Four columns, matching the popover's grid.
pub(super) fn overflow_bounds(anchor: Rect, hidden: usize, display: (f32, f32)) -> Rect {
    const COLUMNS: usize = 4;
    const PAD: f32 = 8.0;
    const GAP: f32 = 6.0;
    let cols = COLUMNS.min(hidden).max(1);
    let rows = hidden.div_ceil(cols);
    crate::component::place_popup_side(
        anchor,
        (
            cols as f32 * (TRAY_CELL + GAP) - GAP + PAD * 2.0,
            rows as f32 * (TRAY_CELL + GAP) - GAP + PAD * 2.0,
        ),
        display,
        crate::component::PopupSide::Right,
    )
}

/// Resolve the shelf for `item_count` items inside `anchor`.
///
/// When the items exceed the anchor's slots, the final slot becomes the
/// disclosure cell and one slot's worth of items moves behind it. That is why
/// the overflow count is `item_count - (slots - 1)`: the disclosure cell itself
/// consumes a slot but shows no item.
pub(super) fn shelf_layout(anchor: Rect, item_count: usize) -> TrayShelfLayout {
    let slots = shelf_slots(anchor.h);
    let (visible, overflow) = if item_count == 0 {
        (0, 0)
    } else if item_count <= slots {
        (item_count, 0)
    } else {
        // The last visible cell is the disclosure, so only `slots - 1` items
        // get their own cell.
        (slots, item_count - (slots - 1))
    };

    let mut layout = TrayShelfLayout {
        anchor,
        slots,
        visible,
        overflow,
        surface: Rect {
            x: anchor.x,
            y: anchor.y,
            w: 0.0,
            h: 0.0,
        },
    };
    // Bottom-aligned: the capsule's bottom edge sits on the anchor's bottom
    // edge, so adding an icon grows the shelf upward and never displaces the
    // icons already nearest the pointer's approach path.
    layout.surface = layout.surface_rect();
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(h: f32) -> Rect {
        Rect {
            x: 48.0,
            y: 200.0,
            w: 56.0,
            h,
        }
    }

    #[test]
    fn capacity_scales_monotonically_with_the_anchor() {
        // [INV-CHROME-02]: no constant caps the slot count.
        let mut previous = 0;
        for h in [100.0_f32, 200.0, 300.0, 400.0, 500.0, 700.0, 1000.0, 1400.0] {
            let slots = shelf_slots(h);
            assert!(
                slots >= previous,
                "capacity must not shrink as the anchor grows: {h}px yielded {slots} after {previous}"
            );
            previous = slots;
        }
        assert!(previous > 20, "a tall anchor must admit many slots");
    }

    #[test]
    fn capacity_is_never_below_one() {
        // A degenerate anchor still yields a usable shelf rather than a
        // division-by-zero or a zero-slot layout that hides the tray entirely.
        for h in [0.0_f32, -5.0, 1.0, 10.0, TRAY_PAD * 2.0] {
            assert_eq!(shelf_slots(h), 1, "anchor {h}px must clamp to one slot");
        }
    }

    #[test]
    fn capacity_inverts_the_occupied_height_exactly() {
        // The round trip is the contract: `n` slots determine a height, and
        // that height admits exactly `n` slots. Off-by-one here would make the
        // shelf clip its own last cell.
        for slots in 1..=30 {
            let h = shelf_height(slots);
            assert_eq!(
                shelf_slots(h),
                slots,
                "{slots} slots occupy {h}px, which must admit exactly {slots}"
            );
        }
    }

    #[test]
    fn everything_fitting_needs_no_disclosure() {
        let layout = shelf_layout(anchor(shelf_height(5)), 5);
        assert_eq!(layout.slots, 5);
        assert_eq!(layout.visible, 5);
        assert_eq!(layout.overflow, 0);
        assert!(!layout.has_disclosure());
    }

    #[test]
    fn overflow_reserves_exactly_one_disclosure_slot() {
        let layout = shelf_layout(anchor(shelf_height(5)), 9);
        assert_eq!(layout.slots, 5);
        assert_eq!(layout.visible, 5, "the shelf fills every slot");
        assert_eq!(layout.overflow, 5, "9 items - 4 own cells = 5 behind +N");
        assert!(layout.has_disclosure());
        // Every item is accounted for: own cells plus disclosed items.
        assert_eq!((layout.visible - 1) + layout.overflow, 9);
    }

    #[test]
    fn every_overflow_size_is_fully_accounted_for() {
        for slots in 1..=12 {
            for item_count in 1..=(slots + 8) {
                let layout = shelf_layout(anchor(shelf_height(slots)), item_count);
                let own_cells = layout.visible - usize::from(layout.has_disclosure());
                assert_eq!(
                    own_cells + layout.overflow,
                    item_count,
                    "{item_count} items in {slots} slots must all be reachable"
                );
                assert!(
                    layout.visible <= layout.slots,
                    "{item_count} items in {slots} slots overflowed to {} cells",
                    layout.visible
                );
            }
        }
    }

    #[test]
    fn a_single_slot_still_discloses_everything() {
        // The pathological case: one slot, two items. The slot becomes the
        // disclosure cell itself, and it must count both items.
        let layout = shelf_layout(anchor(shelf_height(1)), 2);
        assert_eq!(layout.slots, 1);
        assert_eq!(layout.visible, 1);
        assert_eq!(layout.overflow, 2);
        assert!(layout.has_disclosure());
    }

    #[test]
    fn an_empty_shelf_occupies_nothing() {
        let layout = shelf_layout(anchor(shelf_height(6)), 0);
        assert_eq!(layout.visible, 0);
        assert_eq!(layout.overflow, 0);
        assert_eq!(layout.surface.h, 0.0, "no items means no capsule");
    }

    #[test]
    fn the_capsule_is_bottom_aligned_and_sized_to_occupied_slots() {
        let tall = anchor(shelf_height(12));
        let layout = shelf_layout(tall, 2);
        // Sized to two slots, not to the twelve the anchor admits.
        assert_eq!(layout.surface.h, shelf_height(2));
        assert!(
            layout.surface.h < layout.anchor.h,
            "a two-icon tray must not paint a twelve-slot capsule"
        );
        // Bottom-aligned: the capsule's bottom edge is the anchor's.
        assert_eq!(
            layout.surface.y + layout.surface.h,
            layout.anchor.y + layout.anchor.h,
            "the shelf grows upward, so the bottom edge stays put"
        );
    }

    #[test]
    fn the_anchor_never_gains_slots_from_the_item_list() {
        // [INV-CHROME-05]: the slot count depends only on the anchor.
        let a = anchor(shelf_height(7));
        let counts: Vec<usize> = [0, 1, 3, 7, 20, 100]
            .into_iter()
            .map(|n| shelf_layout(a, n).slots)
            .collect();
        assert!(
            counts.windows(2).all(|w| w[0] == w[1]),
            "slot count varied with the item list: {counts:?}"
        );
    }

    #[test]
    fn a_narrow_anchor_still_yields_a_usable_cell() {
        let mut narrow = anchor(shelf_height(4));
        narrow.w = 20.0;
        let layout = shelf_layout(narrow, 2);
        assert!(
            layout.cell_width() >= 24.0,
            "the cell must keep a 24px minimum target even in a narrow anchor"
        );
        assert!(
            layout.surface_width() <= narrow.w,
            "the capsule must never overflow its anchor"
        );
    }
}
