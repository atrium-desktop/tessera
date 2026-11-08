//! Pure geometry for the control-center tray shelf (ADR-0221).
//!
//! The shelf is a **horizontal, bottom-anchored lane** of StatusNotifierItem
//! cells. It is not a scroll viewport: the cells are laid out along one row, and
//! when they exceed the row's capacity the shelf becomes a **pager** with a
//! chevron at each end.
//!
//! Everything here is a pure function of `(anchor, item_count, page)` — never of
//! the item list, the render pass, or the GPU — so capacity, page count, the
//! per-page slice and each chevron's reachability are all unit-testable without
//! a frame ([INV-CHROME-05]). The renderer and the hit-testing paths read the
//! *same* resolved layout, so the drawn shelf and the interactive shelf cannot
//! disagree.
//!
//! Four rules drive the arithmetic:
//!
//! - Capacity is derived, never capped ([INV-CHROME-02]): only the anchor's
//!   width bounds the slot count.
//! - There is no scroll viewport ([INV-CHROME-03]): a page is a whole-number
//!   slice of the lane, never a fractional scroll offset.
//! - Paging is bounded and reversible ([INV-CHROME-07]): the page index can
//!   never address a page that does not exist, and each chevron reports its own
//!   reachability so the two ends can render a disabled state instead of a dead
//!   button.
//! - The capsule is sized to the *occupied* slots, not to the available width,
//!   so a two-icon tray reads as a small pill rather than a wide empty bar.

use super::{TRAY_CELL, TRAY_GAP, TRAY_PAD};
use lens::Rect;

/// The horizontal pitch of one shelf slot: the cell plus the gap after it.
pub(super) const SLOT_PITCH: f32 = TRAY_CELL + TRAY_GAP;

/// Whether one slot of the lane is an item cell or a pager chevron.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShelfSlot {
    /// A resident SNI item, indexing the tray's cell list.
    Item(usize),
    /// The leading chevron: "there are earlier items".
    Previous,
    /// The trailing chevron: "there are later items".
    Next,
}

/// A resolved shelf layout for one item count and one page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TrayShelfLayout {
    /// The full anchor available to the shelf: a bottom-left strip whose height
    /// fits exactly one row of cells.
    pub anchor: Rect,
    /// Slots the anchor admits. Never below 1.
    pub slots: usize,
    /// Total pages for this item count. Always at least 1.
    pub pages: usize,
    /// The current page, clamped into `0..pages`. The resolver owns this clamp
    /// so a stale page index can never address a page that no longer exists —
    /// items disappearing is the common case (a tray client exits) and it
    /// shrinks the page count under the reader's feet.
    pub page: usize,
    /// Items this page has room for when the pager is present, `slots - 2`;
    /// equal to `slots` when everything fits and no pager is drawn.
    pub items_per_page: usize,
    /// Index into the item list of the first item drawn on this page.
    pub first_item: usize,
    /// Items actually drawn on this page: `items_per_page` except on the last
    /// page, which holds only what is left.
    on_page: usize,
    /// Whether the pager is present at all (more items than fit in one row).
    pub paged: bool,
    /// The recessed capsule, sized to the slots occupied on this page and
    /// bottom-aligned inside the anchor.
    pub surface: Rect,
}

impl TrayShelfLayout {
    /// What slot `index` of this page draws, or `None` past the last slot.
    ///
    /// The chevrons bracket the item band, so a page is always
    /// `[previous?, items…, next?]`. The chevrons are present on *every* page
    /// while the shelf is paged, including a disabled end: the lane does not
    /// change length as the user turns pages, which is what keeps the icons from
    /// sliding under the pointer.
    ///
    /// This is the anti-allocation form of the slot walk: `render` calls it once
    /// per slot, per frame, and a `Vec` per frame would be heap churn on the
    /// chrome path for a shelf that changes at most when a tray client comes or
    /// goes.
    pub(super) fn slot_at(&self, index: usize) -> Option<ShelfSlot> {
        if index >= self.visible_slots() {
            return None;
        }
        if !self.paged {
            return Some(ShelfSlot::Item(self.first_item + index));
        }
        match index {
            0 => Some(ShelfSlot::Previous),
            i if i == self.visible_slots() - 1 => Some(ShelfSlot::Next),
            i => Some(ShelfSlot::Item(self.first_item + i - 1)),
        }
    }

    /// The slot index the first *item* occupies on this page: 1 when the pager
    /// brackets the band, else 0. Anchoring a popover to "slot 0" would put it
    /// on the leading chevron.
    pub(super) fn first_item_slot(&self) -> usize {
        usize::from(self.paged)
    }

    /// Every slot of this page, left to right. Test and preview convenience;
    /// the render path uses [`Self::slot_at`] so it allocates nothing.
    #[cfg(test)]
    pub(super) fn page_slots(&self) -> Vec<ShelfSlot> {
        (0..self.visible_slots())
            .filter_map(|index| self.slot_at(index))
            .collect()
    }

    /// Slots drawn this page, chevrons included. `0` only for an empty tray.
    pub(super) fn visible_slots(&self) -> usize {
        if self.paged {
            self.on_page + 2
        } else {
            self.on_page
        }
    }

    /// Whether a page exists before this one.
    pub(super) fn has_previous(&self) -> bool {
        self.paged && self.page > 0
    }

    /// Whether a page exists after this one.
    pub(super) fn has_next(&self) -> bool {
        self.paged && self.page + 1 < self.pages
    }

    /// Cell width for this shelf, given the anchor's width. Never narrower than
    /// a 24px target, never wider than the nominal cell.
    pub(super) fn cell_width(&self) -> f32 {
        TRAY_CELL.min(self.anchor.w - TRAY_PAD * 2.0).max(24.0)
    }

    /// The capsule's width: exactly the occupied slots plus both paddings,
    /// never wider than the anchor. Derived from [`shelf_width`] so the capsule
    /// and the slot metrics cannot disagree.
    pub(super) fn surface_width(&self) -> f32 {
        let slots = self.visible_slots();
        if slots == 0 {
            return 0.0;
        }
        // A narrow anchor shrinks the cell, and the capsule must shrink with
        // it rather than assume the nominal width.
        let tightened =
            slots as f32 * self.cell_width() + (slots as f32 - 1.0) * TRAY_GAP + TRAY_PAD * 2.0;
        tightened.min(shelf_width(slots)).min(self.anchor.w)
    }

    /// The capsule's height: one cell row plus the shelf's own inset.
    pub(super) fn row_height(&self) -> f32 {
        (TRAY_CELL + TRAY_PAD * 2.0).min(self.anchor.h)
    }

    /// The capsule, bottom-aligned in the anchor and sized to the occupied
    /// slots. Width comes from [`Self::surface_width`], so the capsule and the
    /// slots it contains cannot disagree about the horizontal metric.
    pub(super) fn surface_rect(&self) -> Rect {
        Rect {
            x: self.anchor.x,
            y: self.anchor.y + self.anchor.h - self.row_height(),
            w: self.surface_width(),
            h: if self.visible_slots() == 0 {
                0.0
            } else {
                self.row_height()
            },
        }
    }

    /// The rect of slot `index` on this page, in output coordinates.
    ///
    /// Slots share one uniform pitch: a chevron slot is the same hit target as
    /// an item slot, so the pager never demands more precision than the icons
    /// beside it. Every consumer — the renderer, the pressable ids, the popover
    /// anchors — derives from this, so a slot is never drawn somewhere it cannot
    /// be hit.
    pub(super) fn slot_rect(&self, index: usize) -> Rect {
        Rect {
            x: self.surface.x + TRAY_PAD + index as f32 * SLOT_PITCH,
            y: self.surface.y + (self.surface.h - TRAY_CELL) * 0.5,
            w: self.cell_width(),
            h: TRAY_CELL,
        }
    }
}

/// Whole slots an anchor admits in one row.
///
/// Inverts the occupied width of `slots` uniform cells: the largest `n` whose
/// width fits the anchor. Clamped to at least one slot so an extremely narrow
/// anchor still yields a usable (if paged) shelf instead of a division-by-zero
/// layout.
pub(super) fn shelf_slots(anchor_w: f32) -> usize {
    // `+ TRAY_GAP` because `n` cells have `n - 1` gaps between them.
    let fit = ((anchor_w - TRAY_PAD * 2.0 + TRAY_GAP) / SLOT_PITCH).floor();
    if fit < 1.0 { 1 } else { fit as usize }
}

/// Width occupied by `slots` whole slots in one row, including both paddings.
pub(super) fn shelf_width(slots: usize) -> f32 {
    let slots = slots.max(1) as f32;
    slots * TRAY_CELL + (slots - 1.0) * TRAY_GAP + TRAY_PAD * 2.0
}

/// Resolve the shelf for `item_count` items inside `anchor`, showing `page`.
///
/// When the items fit in one row the pager is absent and every item is drawn.
/// Otherwise the lane spends one slot on each chevron and paginates the
/// remainder: `items_per_page = slots - 2`, and `pages` is the exact ceiling of
/// `item_count / items_per_page`. `page` is clamped into range.
pub(super) fn shelf_layout(anchor: Rect, item_count: usize, page: usize) -> TrayShelfLayout {
    let slots = shelf_slots(anchor.w);
    let paged = item_count > slots;
    // With the pager present, two of the slots are chevrons. The floor of one
    // item per page guarantees forward progress even on a one-slot anchor.
    let items_per_page = if paged {
        slots.saturating_sub(2).max(1)
    } else {
        slots
    };
    let pages = if item_count == 0 {
        1
    } else {
        item_count.div_ceil(items_per_page)
    };
    let page = page.min(pages.saturating_sub(1));
    let first_item = page * items_per_page;
    let on_page = item_count.saturating_sub(first_item).min(items_per_page);

    let mut layout = TrayShelfLayout {
        anchor,
        slots,
        pages,
        page,
        items_per_page,
        first_item,
        paged,
        on_page,
        surface: Rect {
            x: anchor.x,
            y: anchor.y,
            w: 0.0,
            h: 0.0,
        },
    };
    // Bottom-aligned: the capsule's bottom edge sits on the anchor's bottom
    // edge, so the shelf hugs the display's corner.
    layout.surface = layout.surface_rect();
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(w: f32) -> Rect {
        Rect {
            x: 48.0,
            y: 900.0,
            w,
            h: TRAY_CELL + TRAY_PAD * 2.0,
        }
    }

    /// Comfortably wide: enough slots for a real pager test.
    fn wide() -> Rect {
        anchor(760.0)
    }

    #[test]
    fn capacity_scales_monotonically_with_the_anchor() {
        // [INV-CHROME-02]: no constant caps the slot count.
        let mut previous = 0;
        for w in [40.0_f32, 100.0, 200.0, 400.0, 800.0, 1600.0, 3000.0] {
            let slots = shelf_slots(w);
            assert!(
                slots >= previous,
                "capacity must not shrink as the anchor grows: {w}px yielded {slots} after {previous}"
            );
            previous = slots;
        }
        assert!(previous > 20, "a wide anchor must admit many slots");
    }

    #[test]
    fn capacity_is_never_below_one() {
        for w in [0.0_f32, -5.0, 1.0, 10.0, TRAY_PAD * 2.0] {
            assert_eq!(shelf_slots(w), 1, "a degenerate anchor is one slot: {w}");
        }
    }

    #[test]
    fn capacity_inverts_the_occupied_width_exactly() {
        for slots in 1..24_usize {
            let width = shelf_width(slots);
            assert_eq!(
                shelf_slots(width),
                slots,
                "{slots} slots occupy {width}px but resolve back to a different count"
            );
            if slots > 1 {
                assert!(
                    shelf_slots(width - SLOT_PITCH) < slots,
                    "{slots} slots must not fit in {}px",
                    width - SLOT_PITCH
                );
            }
        }
    }

    #[test]
    fn everything_fitting_needs_no_pager() {
        let layout = shelf_layout(wide(), 4, 0);
        assert!(!layout.paged, "four items fit a wide shelf");
        assert_eq!(layout.pages, 1);
        assert_eq!(layout.first_item, 0);
        assert_eq!(layout.visible_slots(), 4);
        assert!(!layout.has_previous() && !layout.has_next());
        // Without a pager every slot is an item, in order.
        assert_eq!(
            layout.page_slots(),
            (0..4).map(ShelfSlot::Item).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_empty_shelf_occupies_nothing() {
        let layout = shelf_layout(wide(), 0, 0);
        assert_eq!(layout.visible_slots(), 0);
        assert_eq!(layout.pages, 1);
        assert_eq!(layout.surface.w, 0.0, "no items means no capsule");
        assert!(layout.page_slots().is_empty());
    }

    #[test]
    fn the_capsule_is_bottom_aligned_and_sized_to_occupied_slots() {
        let tall = anchor(1400.0);
        let layout = shelf_layout(tall, 2, 0);
        // Sized to two slots, not to the width the anchor admits.
        assert_eq!(layout.surface.w, shelf_width(2));
        assert!(
            layout.surface.w < layout.anchor.w,
            "a two-icon tray must not paint a full-width capsule"
        );
        // Bottom-aligned: the capsule's bottom edge is the anchor's.
        assert_eq!(
            layout.surface.y + layout.surface.h,
            layout.anchor.y + layout.anchor.h,
            "the shelf hugs the bottom edge"
        );
    }

    #[test]
    fn the_anchor_never_gains_slots_from_the_item_list() {
        // [INV-CHROME-05]: the slot count depends only on the anchor.
        let a = wide();
        let counts: Vec<usize> = [0, 1, 3, 7, 20, 100]
            .into_iter()
            .map(|n| shelf_layout(a, n, 0).slots)
            .collect();
        assert!(
            counts.windows(2).all(|w| w[0] == w[1]),
            "slot count varied with the item list: {counts:?}"
        );
    }

    #[test]
    fn a_narrow_anchor_still_yields_a_usable_cell() {
        let narrow = anchor(20.0);
        let layout = shelf_layout(narrow, 2, 0);
        assert!(
            layout.cell_width() >= 24.0,
            "the cell must keep a 24px minimum target even in a narrow anchor"
        );
        assert!(
            layout.surface_width() <= narrow.w,
            "the capsule must never overflow its anchor"
        );
    }

    #[test]
    fn an_oversized_tray_pages_with_two_chevrons() {
        // The lane spends one slot on each chevron, so the per-page item
        // capacity is `slots - 2`.
        let a = wide();
        let slots = shelf_slots(a.w);
        let layout = shelf_layout(a, slots + 5, 0);
        assert!(layout.paged, "more items than slots must paginate");
        assert_eq!(layout.items_per_page, slots - 2);
        assert_eq!(layout.visible_slots(), slots, "the page fills the lane");
        assert_eq!(layout.first_item, 0);
        assert_eq!(layout.pages, (slots + 5).div_ceil(slots - 2));
        assert!(!layout.has_previous(), "the first page has no previous");
        assert!(layout.has_next(), "the first page of many has a next");
    }

    #[test]
    fn the_page_index_is_clamped_into_range() {
        // [INV-CHROME-07]: a stale page index can never address a page that no
        // longer exists.
        let a = wide();
        let slots = shelf_slots(a.w);
        let over = shelf_layout(a, slots + 5, 99);
        assert_eq!(over.page, over.pages - 1, "an out-of-range page clamps");
        assert!(over.has_previous());
        assert!(!over.has_next(), "the last page has no next");

        // An empty tray has exactly one (empty) page and no pager.
        let empty = shelf_layout(a, 0, 7);
        assert_eq!(empty.page, 0);
        assert_eq!(empty.pages, 1);
        assert_eq!(empty.first_item, 0);
    }

    #[test]
    fn every_item_is_reachable_and_exactly_once() {
        let a = wide();
        let slots = shelf_slots(a.w);
        let per_page = slots - 2;
        for item_count in 1..(per_page * 3 + 2) {
            let mut seen = Vec::new();
            let pages = shelf_layout(a, item_count, 0).pages;
            for page in 0..pages {
                let layout = shelf_layout(a, item_count, page);
                assert_eq!(layout.page, page);
                for slot in layout.page_slots() {
                    if let ShelfSlot::Item(index) = slot {
                        seen.push(index);
                    }
                }
            }
            let expected: Vec<usize> = (0..item_count).collect();
            assert_eq!(
                seen, expected,
                "{item_count} items paged across {pages} pages did not land exactly once each"
            );
        }
    }

    #[test]
    fn turning_pages_is_reversible_and_moves_by_whole_pages() {
        // [INV-CHROME-07]: forward and back are exact inverses, and a page step
        // never lands between two pages.
        let a = wide();
        let slots = shelf_slots(a.w);
        let per_page = slots - 2;
        let item_count = per_page * 3 + 2;
        let pages = shelf_layout(a, item_count, 0).pages;
        assert_eq!(pages, 4);

        for page in 0..pages {
            let here = shelf_layout(a, item_count, page);
            assert_eq!(here.first_item, page * per_page);
            if here.has_next() {
                let next = shelf_layout(a, item_count, page + 1);
                assert_eq!(next.first_item, here.first_item + per_page);
                // And back again is the exact same page.
                let back = shelf_layout(a, item_count, next.page - 1);
                assert_eq!(back.page, here.page);
                assert_eq!(back.first_item, here.first_item);
                assert_eq!(back.page_slots(), here.page_slots());
            }
        }
    }

    #[test]
    fn the_last_page_may_be_short_but_keeps_its_chevrons() {
        let a = wide();
        let slots = shelf_slots(a.w);
        let item_count = (slots - 2) * 2 + 1;
        let layout = shelf_layout(a, item_count, 2);
        assert_eq!(
            layout.visible_slots(),
            3,
            "two chevrons plus the one remaining item"
        );
        assert!(layout.has_previous());
        assert!(!layout.has_next());
        // The capsule shrinks with the short page instead of painting a lane
        // full of empty slots.
        assert!(layout.surface.w < a.w);
    }

    #[test]
    fn both_ends_report_their_own_reachability() {
        // [INV-CHROME-07]: a disabled end is a property of the layout, not a
        // judgement the renderer makes on its own.
        let a = wide();
        let slots = shelf_slots(a.w);
        // Enough items for three pages so a middle page exists.
        let item_count = (slots - 2) * 3;
        let pages = shelf_layout(a, item_count, 0).pages;
        assert_eq!(pages, 3);

        let first = shelf_layout(a, item_count, 0);
        assert_eq!((first.has_previous(), first.has_next()), (false, true));
        let middle = shelf_layout(a, item_count, 1);
        assert_eq!(
            (middle.has_previous(), middle.has_next()),
            (true, true),
            "a middle page can go both ways"
        );
        let last = shelf_layout(a, item_count, pages - 1);
        assert_eq!((last.has_previous(), last.has_next()), (true, false));
    }

    #[test]
    fn the_chevrons_bracket_the_item_band_on_every_page() {
        let a = wide();
        let slots = shelf_slots(a.w);
        let layout = shelf_layout(a, slots + 4, 1);
        let page = layout.page_slots();
        assert_eq!(page.first(), Some(&ShelfSlot::Previous));
        assert_eq!(page.last(), Some(&ShelfSlot::Next));

        let previous = layout.slot_rect(0);
        let next = layout.slot_rect(layout.visible_slots() - 1);
        let first_item = layout.slot_rect(1);
        let last_item = layout.slot_rect(layout.visible_slots() - 2);
        assert!(
            previous.x + previous.w <= first_item.x,
            "the leading chevron precedes the first item: {previous:?} {first_item:?}"
        );
        assert!(
            next.x >= last_item.x + last_item.w,
            "the trailing chevron follows the last item: {next:?} {last_item:?}"
        );
        // The chevrons keep the same hit target as an item cell.
        assert_eq!(previous.w, layout.cell_width());
        assert_eq!(next.w, layout.cell_width());
    }

    #[test]
    fn slots_are_laid_out_left_to_right_on_one_row() {
        let layout = shelf_layout(wide(), 3, 0);
        let rects: Vec<Rect> = (0..layout.visible_slots())
            .map(|index| layout.slot_rect(index))
            .collect();
        for pair in rects.windows(2) {
            assert!(
                pair[1].x >= pair[0].x + pair[0].w,
                "slots must advance rightward without overlapping: {pair:?}"
            );
        }
        assert!(rects.iter().all(|r| r.y == rects[0].y && r.h == rects[0].h));
        for rect in &rects {
            assert!(rect.x >= layout.surface.x);
            assert!(rect.x + rect.w <= layout.surface.x + layout.surface.w + 0.01);
            assert!(rect.y >= layout.surface.y);
            assert!(rect.y + rect.h <= layout.surface.y + layout.surface.h + 0.01);
        }
    }

    #[test]
    fn a_single_slot_anchor_still_pages_forward() {
        // The pathological case: one slot, two items. The pager keeps at least
        // one item per page, so the shelf cannot livelock on page 0. The lane
        // still draws both chevrons, however short it is — the ends are what
        // tell the reader which way pages exist.
        let layout = shelf_layout(anchor(20.0), 2, 0);
        assert!(layout.paged);
        assert_eq!(layout.items_per_page, 1);
        assert_eq!(layout.pages, 2);
        assert_eq!(
            layout.page_slots(),
            vec![ShelfSlot::Previous, ShelfSlot::Item(0), ShelfSlot::Next]
        );
        assert_eq!(
            (layout.has_previous(), layout.has_next()),
            (false, true),
            "the leading end is dead and the trailing one is live"
        );
        let last = shelf_layout(anchor(20.0), 2, 1);
        assert_eq!(last.page, 1);
        assert_eq!(
            (last.has_previous(), last.has_next()),
            (true, false),
            "the ends swap at the other extreme"
        );
    }
}
