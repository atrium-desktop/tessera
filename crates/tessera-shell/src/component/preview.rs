//! Shared presentation contract for compositor-rendered live previews.
//!
//! Components own layout, animation, and interaction lifecycles. This module
//! owns the common card model and the pure derivations that must agree across
//! compositor scene rendering, shell chrome, hit-testing, and Liquid Glass
//! focus.

use lens::LayoutOpts;
use tessera_design::{Design, PreviewSelectionStyle, materials};
use tessera_desktop::window::WindowId;

/// One live client-preview card shared by compositor and chrome rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewCard {
    pub window: WindowId,
    pub geometry: crate::layout::window_switcher::Card,
    pub corner_radius: f32,
}

impl PreviewCard {
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        let rect = self.geometry.outer;
        x >= rect.origin.x as f32
            && y >= rect.origin.y as f32
            && x < (rect.origin.x + rect.size.w) as f32
            && y < (rect.origin.y + rect.size.h) as f32
    }
}

/// Shared switcher presentation prepared once per frame.
///
/// The executable uses these exact targets for live client previews and the
/// shell uses them for focus, labels, hit-testing, and animation. Keeping a
/// single snapshot prevents the chrome and client scene from drifting apart.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSwitcherPresentation {
    pub mode: crate::layout::window_switcher::Mode,
    pub panel: tessera_primitives::Rect,
    pub cards: Vec<PreviewCard>,
    /// Independently animated focus geometry. In fixed mode this moves
    /// between cards; in carousel mode it remains at the output centre.
    pub selection_indicator: Option<crate::layout::window_switcher::Card>,
    pub selected: Option<WindowId>,
    pub inactive_content_brightness: f32,
    pub visibility: f32,
}

/// One compositor-rendered live-preview popover contributed by ordinary
/// chrome, such as the group of running windows above a hovered Dock tile.
#[derive(Debug, Clone, PartialEq)]
pub struct LivePreviewPresentation {
    pub panel: tessera_primitives::Rect,
    pub cards: Vec<PreviewCard>,
    pub focused: Option<WindowId>,
    pub inactive_content_brightness: f32,
    pub visibility: f32,
}

/// Visual state of content hosted inside one preview panel body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewCardState {
    Rest,
    Hovered,
    Selected,
}

/// Minimal painted foreground for the parent preview panel.
///
/// Enter/exit fades belong to the lens opacity switch (`Frame::set_opacity`),
/// so the material stays at full alpha.
#[must_use]
pub fn panel_material(design: &Design) -> LayoutOpts {
    let mut material = materials::panel(design);
    material.radius = design.radii.panel_lg;
    material
}

/// Shared foreground treatment for one preview card.
#[must_use]
pub fn card_material(design: &Design, state: PreviewCardState, corner_radius: f32) -> LayoutOpts {
    let mut material = match state {
        PreviewCardState::Rest => materials::surface_layout(),
        PreviewCardState::Hovered => materials::selection(design, false),
        PreviewCardState::Selected => materials::selection(design, true),
    };
    material.radius = corner_radius;
    material.pad = 0.0;
    material
}

/// Resolve the topmost card under a point, optionally preferring a staged
/// card that may overlap its stationary siblings.
#[must_use]
pub fn hit_test(
    cards: &[PreviewCard],
    preferred: Option<WindowId>,
    x: f32,
    y: f32,
) -> Option<WindowId> {
    preferred
        .and_then(|preferred| {
            cards
                .iter()
                .find(|card| card.window == preferred && card.contains(x, y))
        })
        .or_else(|| cards.iter().rev().find(|card| card.contains(x, y)))
        .map(|card| card.window)
}

/// Keep preview pixels opaque and express hierarchy through brightness.
#[must_use]
pub fn content_brightness(focused: Option<WindowId>, candidate: WindowId, inactive: f32) -> f32 {
    if focused.is_none() || focused == Some(candidate) {
        1.0
    } else {
        inactive.clamp(0.0, 1.0)
    }
}

/// Apply a named selection treatment while preserving the shared card
/// geometry contract.
#[must_use]
pub fn selected_geometry(
    card: crate::layout::window_switcher::Card,
    style: PreviewSelectionStyle,
    design: &Design,
) -> crate::layout::window_switcher::Card {
    let selection = design.preview.selection(style);
    if selection.scale == 1.0 && selection.lift == 0.0 {
        return card;
    }
    let scale = |value: i32| ((value as f32 * selection.scale).round() as i32).max(1);
    let width = scale(card.outer.size.w);
    let preview_height = scale(card.preview.size.h);
    let label_height = card.label.size.h;
    let height = preview_height + label_height;
    let centre_x = card.outer.origin.x + card.outer.size.w / 2;
    let x = centre_x - width / 2;
    let y = card.outer.origin.y - (height - card.outer.size.h) / 2 - selection.lift.round() as i32;
    crate::layout::window_switcher::Card {
        outer: tessera_primitives::Rect::new(x, y, width, height),
        preview: tessera_primitives::Rect::new(x, y, width, preview_height),
        label: tessera_primitives::Rect::new(x, y + preview_height, width, label_height),
    }
}

#[cfg(test)]
mod tests {
    use lens::Color;

    use super::*;

    fn card(window: u64, x: i32) -> PreviewCard {
        PreviewCard {
            window: WindowId(window),
            geometry: crate::layout::window_switcher::Card {
                outer: tessera_primitives::Rect::new(x, 10, 100, 70),
                preview: tessera_primitives::Rect::new(x, 10, 100, 50),
                label: tessera_primitives::Rect::new(x, 60, 100, 20),
            },
            corner_radius: Design::dark().radii.control,
        }
    }

    #[test]
    fn preferred_card_wins_in_an_overlap() {
        let cards = [card(1, 0), card(2, 50)];
        assert_eq!(
            hit_test(&cards, Some(WindowId(1)), 75.0, 30.0),
            Some(WindowId(1))
        );
        assert_eq!(hit_test(&cards, None, 75.0, 30.0), Some(WindowId(2)));
    }

    #[test]
    fn selection_targets_the_complete_card() {
        // Selection is a wash over the whole card, label included — a partial
        // highlight would read as a stripe rather than a chosen target.
        let design = Design::dark();
        let cards = [card(7, 20)];
        let selected = card_material(&design, PreviewCardState::Selected, cards[0].corner_radius);
        assert_eq!(selected.radius, design.radii.control);
        assert_eq!(selected.bg, design.selection.selected_tint);
        assert_eq!(cards[0].geometry.label.size.h, 20);
    }

    #[test]
    fn staged_selection_scales_and_lifts_without_resizing_the_label() {
        let design = Design::dark();
        let base = card(1, 100).geometry;
        let staged = selected_geometry(base, PreviewSelectionStyle::Staged, &design);
        assert!(staged.outer.size.w > base.outer.size.w);
        assert!(staged.outer.origin.y < base.outer.origin.y);
        assert_eq!(staged.label.size.h, base.label.size.h);
        assert_eq!(
            selected_geometry(base, PreviewSelectionStyle::Focused, &design),
            base
        );
    }

    #[test]
    fn card_states_stay_inside_the_parent_material() {
        let design = Design::dark();
        let rest = card_material(&design, PreviewCardState::Rest, 9.0);
        let hovered = card_material(&design, PreviewCardState::Hovered, 9.0);
        let selected = card_material(&design, PreviewCardState::Selected, 9.0);
        assert_eq!(rest.bg, Color::TRANSPARENT);
        assert_eq!(hovered.bg, design.selection.hover_tint);
        assert_eq!(selected.bg, design.selection.selected_tint);
        assert_eq!(hovered.border_width, 0.0);
        assert_eq!(selected.border_width, 0.0);
        assert_eq!(selected.radius, 9.0);
    }
}
