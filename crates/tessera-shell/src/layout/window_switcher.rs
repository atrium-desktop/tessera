//! Shared geometry for the compositor-owned Super+Tab window switcher.
//!
//! A compact list uses fixed cards and a moving selection indicator. Once the
//! standard-width strip would consume more than 65% of the output, the
//! selection indicator stays centred and the cards form a circular carousel.
//! The renderer uses each card's preview rect for live client surfaces while
//! shell chrome uses the same geometry for borders, labels, and hit-testing.

use tessera_primitives::Rect;

const OUTER_MARGIN: i32 = 32;
const PANEL_PAD: i32 = 22;
const CARD_GAP: i32 = 18;
const CARD_MAX_W: i32 = 238;
const CARD_MIN_W: i32 = 88;
const CARD_LABEL_H: i32 = 38;
const PREVIEW_ASPECT: f32 = 0.64;
const FIXED_WIDTH_PERCENT: i32 = 65;
const MAX_HEIGHT_PERCENT: i32 = 34;

/// The spatial model selected for one switcher session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Cards stay in stable positions; only the selection indicator moves.
    Fixed,
    /// The indicator stays centred; the circular card list moves beneath it.
    Carousel,
}

/// One window card in the switcher strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// Complete card including preview and label.
    pub outer: Rect,
    /// Live client-surface target.
    pub preview: Rect,
    /// Title/application label below the preview.
    pub label: Rect,
}

/// Centred switcher panel and its one-to-one window cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub mode: Mode,
    pub panel: Rect,
    pub cards: Vec<Card>,
    /// Signed slot coordinate for each card, parallel to `cards`.
    ///
    /// Fixed mode uses the stable index of the card in the strip. Carousel
    /// mode uses the wrapped offset from the selected card, in `[-n/2, n/2]`.
    /// Chrome animates the *slot*, not the pixel geometry, so a card crossing
    /// the circular seam can be carried continuously through an equivalent
    /// off-panel copy instead of being teleported while still visible.
    pub slots: Vec<i32>,
}

/// Choose the stable-card or centred-carousel model using standard card
/// dimensions. The decision deliberately ignores animation and title width,
/// so neither can change modes while the switcher is visible.
pub fn mode(display: Rect, windows: usize) -> Mode {
    let (card_w, _) = standard_card_size(display);
    let gaps = CARD_GAP * windows.saturating_sub(1) as i32;
    let total_w = card_w * windows as i32 + gaps + PANEL_PAD * 2;
    if total_w <= display.size.w.max(1) * FIXED_WIDTH_PERCENT / 100 {
        Mode::Fixed
    } else {
        Mode::Carousel
    }
}

/// Lay out the switcher in the requested mode.
pub fn layout_for_mode(display: Rect, windows: usize, selected: usize, mode: Mode) -> Layout {
    match mode {
        Mode::Fixed => fixed_layout(display, windows),
        Mode::Carousel => carousel_layout(display, windows, selected),
    }
}

/// Lay out a compact fixed strip. Window cards never change position as the
/// selected index changes.
pub fn fixed_layout(display: Rect, windows: usize) -> Layout {
    let (card_w, card_h) = standard_card_size(display);
    if windows == 0 {
        return empty_layout(display, Mode::Fixed);
    }

    let preview_h = card_h - CARD_LABEL_H;
    let gaps = CARD_GAP * windows.saturating_sub(1) as i32;
    let panel_w = card_w * windows as i32 + gaps + PANEL_PAD * 2;
    let panel_h = card_h + PANEL_PAD * 2;
    let panel = centred_rect(display, panel_w, panel_h);
    let cards = (0..windows)
        .map(|index| {
            let x = panel.origin.x + PANEL_PAD + index as i32 * (card_w + CARD_GAP);
            let y = panel.origin.y + PANEL_PAD;
            card_at(x, y, card_w, preview_h)
        })
        .collect();

    Layout {
        mode: Mode::Fixed,
        panel,
        cards,
        slots: (0..windows as i32).collect(),
    }
}

/// Lay out a circular carousel whose selected card is always centred.
///
/// `cards` stays in caller order; only its positions rotate. At most one copy
/// of each window is emitted, so small lists never repeat an item. Cards two
/// or more steps from the centre are slightly reduced and the fixed-width
/// panel clips the distant tail of large lists.
pub fn carousel_layout(display: Rect, windows: usize, selected: usize) -> Layout {
    let (card_w, card_h) = standard_card_size(display);
    if windows == 0 {
        return empty_layout(display, Mode::Carousel);
    }

    let selected = selected.min(windows - 1);
    let centre_x = display.origin.x + display.size.w / 2;
    let centre_y = display.origin.y + display.size.h / 2;
    let max_panel_w = (display.size.w * FIXED_WIDTH_PERCENT / 100)
        .max(card_w + PANEL_PAD * 2)
        .min((display.size.w - OUTER_MARGIN * 2).max(1));
    let panel = Rect::new(
        centre_x - max_panel_w / 2,
        centre_y - (card_h + PANEL_PAD * 2) / 2,
        max_panel_w,
        card_h + PANEL_PAD * 2,
    );

    let slots: Vec<i32> = (0..windows)
        .map(|index| {
            let forward = (index + windows - selected) % windows;
            if forward <= windows / 2 {
                forward as i32
            } else {
                forward as i32 - windows as i32
            }
        })
        .collect();
    let cards = slots
        .iter()
        .map(|offset| carousel_card_at_offset(display, windows, *offset))
        .collect();

    Layout {
        mode: Mode::Carousel,
        panel,
        cards,
        slots,
    }
}

/// The card a carousel window occupies at a continuous signed slot.
///
/// Unlike [`carousel_layout`], `slot` may lie outside `[-n/2, n/2]`: the
/// geometry is placed at that unwrapped position, which lets an animation carry
/// a card across the ring seam at constant speed. Renderers draw every
/// equivalent copy (`slot ± k·n`) that intersects the panel, so the card leaves
/// one edge and enters the other instead of teleporting while visible.
pub fn carousel_card(display: Rect, windows: usize, slot: f32) -> Card {
    if windows == 0 {
        return card_at(display.origin.x, display.origin.y, CARD_MIN_W, 56);
    }
    let lower = slot.floor();
    let fraction = slot - lower;
    let from = carousel_card_at_offset(display, windows, lower as i32);
    let to = carousel_card_at_offset(display, windows, lower as i32 + 1);
    lerp_card(from, to, fraction)
}

/// The carousel card for one integer, possibly unwrapped, slot offset.
fn carousel_card_at_offset(display: Rect, windows: usize, offset: i32) -> Card {
    let (card_w, card_h) = standard_card_size(display);
    let step = card_w + CARD_GAP;
    let centre_x = display.origin.x + display.size.w / 2;
    let centre_y = display.origin.y + display.size.h / 2;
    let distance = wrapped_distance(offset, windows);
    let scale_percent = match distance {
        0 => 100,
        1 => 93,
        _ => 86,
    };
    let scaled_w = card_w * scale_percent / 100;
    let scaled_h = card_h * scale_percent / 100;
    let preview_h = (scaled_h - CARD_LABEL_H).max(1);
    let x = centre_x + offset * step - scaled_w / 2;
    let y = centre_y - scaled_h / 2;
    card_at(x, y, scaled_w, preview_h)
}

/// The distance in slots from the selected card, following the short way
/// around the ring. `offset` may be unwrapped.
fn wrapped_distance(offset: i32, windows: usize) -> u32 {
    if windows == 0 {
        return offset.unsigned_abs();
    }
    let n = windows as i32;
    let wrapped = offset - n * ((offset as f32) / (n as f32)).round() as i32;
    wrapped.unsigned_abs()
}

fn lerp_card(from: Card, to: Card, fraction: f32) -> Card {
    let fraction = fraction.clamp(0.0, 1.0);
    let mix = |a: i32, b: i32| a as f32 + (b - a) as f32 * fraction;
    let x = mix(from.outer.origin.x, to.outer.origin.x).round() as i32;
    let y = mix(from.outer.origin.y, to.outer.origin.y).round() as i32;
    let w = mix(from.outer.size.w, to.outer.size.w).round() as i32;
    let preview_h = mix(from.preview.size.h, to.preview.size.h).round() as i32;
    Card {
        outer: Rect::new(x, y, w.max(1), preview_h.max(1) + CARD_LABEL_H),
        preview: Rect::new(x, y, w.max(1), preview_h.max(1)),
        label: Rect::new(x, y + preview_h.max(1), w.max(1), CARD_LABEL_H),
    }
}

/// Compatibility entry point for fixed strips used by live-preview callers.
pub fn layout(display: Rect, windows: usize) -> Layout {
    fixed_layout(display, windows)
}

fn standard_card_size(display: Rect) -> (i32, i32) {
    let max_card_h =
        (display.size.h.max(1) * MAX_HEIGHT_PERCENT / 100 - PANEL_PAD * 2).max(CARD_LABEL_H + 56);
    let height_limited_w =
        (((max_card_h - CARD_LABEL_H) as f32 / PREVIEW_ASPECT).round() as i32).max(CARD_MIN_W);
    let card_w = CARD_MAX_W.min(height_limited_w);
    let preview_h = ((card_w as f32 * PREVIEW_ASPECT).round() as i32).max(56);
    (card_w, preview_h + CARD_LABEL_H)
}

fn card_at(x: i32, y: i32, width: i32, preview_h: i32) -> Card {
    Card {
        outer: Rect::new(x, y, width, preview_h + CARD_LABEL_H),
        preview: Rect::new(x, y, width, preview_h),
        label: Rect::new(x, y + preview_h, width, CARD_LABEL_H),
    }
}

fn centred_rect(display: Rect, width: i32, height: i32) -> Rect {
    Rect::new(
        display.origin.x + (display.size.w - width) / 2,
        display.origin.y + (display.size.h - height) / 2,
        width,
        height,
    )
}

fn empty_layout(display: Rect, mode: Mode) -> Layout {
    Layout {
        mode,
        panel: centred_rect(display, 200, 80),
        cards: Vec::new(),
        slots: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_strip_uses_fixed_mode_and_stable_positions() {
        let display = Rect::new(0, 0, 1920, 1080);
        assert_eq!(mode(display, 4), Mode::Fixed);
        let first = layout_for_mode(display, 4, 0, Mode::Fixed);
        let last = layout_for_mode(display, 4, 3, Mode::Fixed);
        assert_eq!(first.cards, last.cards);
        assert_eq!(first.panel, last.panel);
    }

    #[test]
    fn wide_strip_crosses_the_sixty_five_percent_threshold() {
        let display = Rect::new(0, 0, 1920, 1080);
        assert_eq!(mode(display, 5), Mode::Carousel);
        let layout = layout_for_mode(display, 5, 0, Mode::Carousel);
        assert!(layout.panel.size.w <= display.size.w * 65 / 100);
    }

    #[test]
    fn fixed_cards_are_centred_and_stay_on_screen() {
        let display = Rect::new(0, 0, 1920, 1080);
        let layout = fixed_layout(display, 4);
        assert_eq!(layout.cards.len(), 4);
        assert!(layout.panel.origin.x >= 0);
        assert!(layout.panel.origin.y >= 0);
        assert!(layout.panel.origin.x + layout.panel.size.w <= display.size.w);
        assert!(layout.panel.origin.y + layout.panel.size.h <= display.size.h);
        for card in &layout.cards {
            assert_eq!(card.preview.size.w, card.outer.size.w);
            assert_eq!(card.preview.size.h + card.label.size.h, card.outer.size.h);
        }
    }

    #[test]
    fn carousel_keeps_every_selection_at_the_visual_centre() {
        let display = Rect::new(0, 0, 1920, 1080);
        for selected in 0..8 {
            let layout = carousel_layout(display, 8, selected);
            let card = layout.cards[selected];
            assert_eq!(
                card.outer.origin.x + card.outer.size.w / 2,
                display.size.w / 2
            );
            assert_eq!(
                card.outer.origin.y + card.outer.size.h / 2,
                display.size.h / 2
            );
        }
    }

    #[test]
    fn carousel_wrap_is_one_adjacent_step() {
        let display = Rect::new(0, 0, 1920, 1080);
        let before = carousel_layout(display, 5, 4);
        let after = carousel_layout(display, 5, 0);
        let centre = display.size.w / 2;
        let incoming_before = before.cards[0].outer.origin.x + before.cards[0].outer.size.w / 2;
        let incoming_after = after.cards[0].outer.origin.x + after.cards[0].outer.size.w / 2;
        assert!(incoming_before > centre);
        assert_eq!(incoming_after, centre);
    }

    #[test]
    fn tiny_outputs_keep_the_panel_inside_the_available_width() {
        let display = Rect::new(100, 20, 480, 320);
        let layout = carousel_layout(display, 10, 0);
        assert!(layout.panel.origin.x >= display.origin.x);
        assert!(layout.panel.origin.x + layout.panel.size.w <= display.origin.x + display.size.w);
    }

    #[test]
    fn carousel_slots_are_signed_and_centred_on_the_selection() {
        let display = Rect::new(0, 0, 1920, 1080);
        let layout = carousel_layout(display, 8, 5);
        assert_eq!(
            layout.slots,
            vec![3, 4, -3, -2, -1, 0, 1, 2],
            "the selected card sits at slot 0 and the seam is one step wide"
        );
        assert_eq!(layout.cards[5], carousel_card(display, 8, 0.0));
    }

    /// The seam is crossed at a continuous position: the card that leaves one
    /// side is the same card that enters the other, one ring away.
    #[test]
    fn carousel_cards_cross_the_seam_continuously() {
        let display = Rect::new(0, 0, 1920, 1080);
        let before = carousel_layout(display, 5, 4);
        let after = carousel_layout(display, 5, 0);

        // The seam card is index 2: canonical slots jump from -2 to +2, a
        // four-slot representation change. Animated slots instead carry it one
        // step left, from -2 to -3, with the +2 copy entering from the right.
        assert_eq!(before.slots[2], -2);
        assert_eq!(after.slots[2], 2);
        let left = carousel_card(display, 5, -3.0);
        let right = carousel_card(display, 5, 2.0);
        assert!(left.outer.origin.x + left.outer.size.w < display.size.w / 2);
        assert!(right.outer.origin.x > display.size.w / 2);
        assert_eq!(right, after.cards[2]);

        // Every other card moves exactly one adjacent slot.
        for index in [0, 1, 3, 4] {
            assert_eq!(after.slots[index] - before.slots[index], -1);
        }
    }

    #[test]
    fn unwrapped_copy_steps_continuously_across_the_seam() {
        let display = Rect::new(0, 0, 1920, 1080);
        let a = carousel_card(display, 5, 1.8);
        let b = carousel_card(display, 5, 2.0);
        let c = carousel_card(display, 5, 2.2);
        assert!(a.outer.origin.x < b.outer.origin.x);
        assert!(b.outer.origin.x < c.outer.origin.x);

        // The equivalent copy on the other side reaches the same screen
        // position one ring later, without the card itself changing identity.
        let step = 238 + CARD_GAP;
        let across = carousel_card(display, 5, 2.2 - 5.0);
        assert!((across.outer.origin.x - (c.outer.origin.x - 5 * step)).abs() <= 2);
    }
}
