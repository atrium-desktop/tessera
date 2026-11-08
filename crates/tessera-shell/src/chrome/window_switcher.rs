//! Held-Super window switcher chrome.
//!
//! The compositor renderer paints each live window into the preview rects
//! from `crate::layout::window_switcher`; this component adds the glass panel,
//! selection focus, icons, labels, shared carousel animation, and pointer
//! hit-testing. Releasing Super or clicking a card closes the strip.

use std::collections::{HashMap, HashSet};

use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};
use tessera_design::materials::{chrome_place, surface_layout};
use tessera_design::{Design, PreviewSelectionStyle};

use crate::component::preview;
use crate::component::{
    Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, IconSet, Localizer, Message,
    PreviewCard, WindowSwitcherPresentation, ellipsize,
};
use tessera_desktop::window::Window;
use tessera_desktop::window::WindowId;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_primitives::input::KeyAction;
use tessera_primitives::input::KeyChar;
use tessera_primitives::input::key_action;

const FADE_RATE: f32 = 18.0;
const SLIDE_RATE: f32 = 15.0;

pub struct WindowSwitcher {
    open: bool,
    visibility: f32,
    anim_active: bool,
    reduced_motion: bool,
    icons: IconSet,
    /// Frozen MRU order for the current held-Super session.
    order: Vec<WindowId>,
    layout_key: Option<(usize, i32, i32, i32, i32)>,
    mode: crate::layout::window_switcher::Mode,
    animated_cards: HashMap<WindowId, AnimCard>,
    animated_selection: Option<AnimCard>,
    presentation: Option<WindowSwitcherPresentation>,
    hovered: Option<WindowId>,
    /// The design snapshot the switcher paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
}

impl Default for WindowSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSwitcher {
    pub fn new() -> Self {
        Self {
            open: false,
            visibility: 0.0,
            anim_active: false,
            reduced_motion: false,
            icons: IconSet::default(),
            order: Vec::new(),
            layout_key: None,
            mode: crate::layout::window_switcher::Mode::Fixed,
            animated_cards: HashMap::new(),
            animated_selection: None,
            presentation: None,
            hovered: None,
            design: Design::dark(),
        }
    }

    fn start_window_switcher(&mut self) {
        if !self.open {
            self.order.clear();
            self.layout_key = None;
            self.open = true;
            self.anim_active = true;
        }
    }

    fn finish_window_switcher(&mut self) {
        if self.open {
            self.open = false;
            self.anim_active = true;
        }
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn advance_visibility(&mut self, dt: f32) {
        let target = if self.open { 1.0 } else { 0.0 };
        self.visibility = crate::widgets::motion::approach(
            self.visibility,
            target,
            FADE_RATE,
            dt,
            self.reduced_motion,
        );
        self.anim_active = (self.visibility - target).abs() > 0.002;
        if !self.anim_active {
            self.visibility = target;
        }
    }

    fn prepare(
        &mut self,
        input: &Input,
        display: tessera_primitives::Rect,
        windows: &[Window],
        session_order: &[WindowId],
        selected: Option<WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        let dt = input.as_raw().dt_seconds.max(0.0);
        self.advance_visibility(dt);
        if self.visibility <= 0.001 && !self.open {
            self.presentation = None;
            self.animated_cards.clear();
            self.hovered = None;
            return None;
        }

        let live: HashSet<_> = windows.iter().map(|window| window.id).collect();
        let design = self.design;
        if self.open {
            // The compositor owns the frozen eligible set. Replacing from its
            // snapshot removes closed windows but deliberately ignores newly
            // mapped windows until the next switcher session.
            self.order = session_order
                .iter()
                .copied()
                .filter(|id| live.contains(id))
                .collect();
        } else {
            self.order.retain(|id| live.contains(id));
        }

        let selected = selected
            .filter(|id| self.order.contains(id))
            .or_else(|| self.presentation.as_ref().and_then(|state| state.selected))
            .filter(|id| self.order.contains(id))
            .or_else(|| self.order.first().copied());
        let selected_index = selected
            .and_then(|id| self.order.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let layout_key = (
            self.order.len(),
            display.origin.x,
            display.origin.y,
            display.size.w,
            display.size.h,
        );
        if self.layout_key != Some(layout_key) {
            self.layout_key = Some(layout_key);
            self.mode = crate::layout::window_switcher::mode(display, self.order.len());
            self.animated_cards.clear();
            self.animated_selection = None;
        }
        let target = crate::layout::window_switcher::layout_for_mode(
            display,
            self.order.len(),
            selected_index,
            self.mode,
        );
        // Reduced motion is a one-frame resolve, expressed once by `transit`
        // (`blend == 1.0`), so no separate branch is needed below.
        let blend = crate::widgets::motion::blend(SLIDE_RATE, dt, self.reduced_motion);
        let target_cards = target
            .cards
            .iter()
            .copied()
            .enumerate()
            .map(|(index, card)| {
                if index == selected_index {
                    preview::selected_geometry(card, PreviewSelectionStyle::Staged, &design)
                } else {
                    card
                }
            })
            .collect::<Vec<_>>();
        let mut cards = Vec::with_capacity(self.order.len());
        let mut cards_moving = false;
        for (id, target_card) in self.order.iter().copied().zip(target_cards.iter().copied()) {
            let target_anim = AnimCard::from_card(target_card);
            let current = self.animated_cards.entry(id).or_insert(target_anim);
            // A circular list has one off-screen item whose logical slot wraps
            // from one tail to the other. Teleport only that distant item behind
            // the panel clip; every visible item, including the incoming
            // selection, moves exactly one adjacent step.
            if self.mode == crate::layout::window_switcher::Mode::Carousel {
                let current_centre = current.outer.x + current.outer.w * 0.5;
                let target_centre = target_anim.outer.x + target_anim.outer.w * 0.5;
                if (current_centre - target_centre).abs() > target.panel.size.w as f32 * 0.5 {
                    *current = target_anim;
                }
            }
            // `blend` carries reduced motion (`1.0` resolves in one frame), so
            // there is no separate reduced-motion branch here.
            *current = current.lerp(target_anim, blend);
            cards_moving |= current.is_moving(target_anim);
            let settled = current.to_card();
            if intersects(target.panel, settled.outer) {
                cards.push(PreviewCard {
                    window: id,
                    geometry: settled,
                    corner_radius: design.radii.control,
                });
            }
        }
        self.animated_cards.retain(|id, _| self.order.contains(id));

        let target_selection = target_cards.get(selected_index).copied();
        let selection_moving = if let Some(target_selection) = target_selection {
            let target_anim = AnimCard::from_card(target_selection);
            let current = self.animated_selection.get_or_insert(target_anim);
            if self.mode == crate::layout::window_switcher::Mode::Carousel {
                *current = target_anim;
            } else {
                *current = current.lerp(target_anim, blend);
            }
            current.is_moving(target_anim)
        } else {
            self.animated_selection = None;
            false
        };
        self.anim_active = cards_moving || selection_moving;
        if !self.anim_active {
            self.anim_active = (self.visibility - if self.open { 1.0 } else { 0.0 }).abs() > 0.002;
        }

        let presentation = WindowSwitcherPresentation {
            mode: self.mode,
            panel: target.panel,
            cards,
            selection_indicator: self.animated_selection.map(AnimCard::to_card),
            selected,
            inactive_content_brightness: design.preview.inactive_content_brightness,
            visibility: self.visibility,
        };
        self.presentation = Some(presentation.clone());
        Some(presentation)
    }
}

impl Chrome for WindowSwitcher {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let display = tessera_primitives::Rect::new(
            0,
            0,
            input.as_raw().display_size.x.max(1.0) as i32,
            input.as_raw().display_size.y.max(1.0) as i32,
        );
        if self.presentation.is_none() {
            let order = self.order.clone();
            self.prepare(input, display, windows, &order, None);
        }
        let Some(presentation) = self.presentation.clone() else {
            return;
        };

        let raw = input.as_raw();
        let cursor = (raw.cursor.x, raw.cursor.y);
        let previous_hovered = self.hovered;
        self.hovered = self
            .open
            .then(|| {
                preview::hit_test(
                    &presentation.cards,
                    presentation.selected,
                    cursor.0,
                    cursor.1,
                )
            })
            .flatten();
        self.anim_active |= self.hovered != previous_hovered;
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        if self.open && pressed {
            if let Some(id) = self.hovered
                && windows
                    .iter()
                    .find(|window| window.id == id)
                    .is_some_and(|window| !window.read_only)
            {
                out.window_switcher_pick = Some(id);
            } else {
                out.window_switcher_cancel = true;
            }
            self.finish_window_switcher();
        }

        // The whole strip fades through the lens opacity switch; per-item
        // dimming multiplies on top of it below.
        frame.set_opacity(presentation.visibility);
        let design = self.design;
        let panel = to_lens(presentation.panel);
        let panel_material = preview::panel_material(&design);
        tessera_design::materials::place_floating(
            frame,
            "tessera-window-switcher-panel",
            tessera_design::SurfaceRole::FloatingPanel,
            &design,
            panel,
            presentation.visibility,
            panel_material,
            |frame| {
                frame.column_ex(
                    &LayoutOpts {
                        width: panel.w,
                        height: panel.h,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        if let Some(indicator) = presentation.selection_indicator {
            let selection = to_lens(indicator.outer);
            frame.place(
                "tessera-window-switcher-selection",
                &chrome_place(
                    selection,
                    preview::card_material(
                        &design,
                        preview::PreviewCardState::Selected,
                        design.radii.control,
                    ),
                ),
                |frame| {
                    frame.column_ex(&layer_size(selection.w, selection.h), |_| {});
                },
            );
        }

        let mut render_cards: Vec<_> = presentation.cards.iter().collect();
        render_cards.sort_by_key(|card| Some(card.window) == presentation.selected);
        for (index, presented) in render_cards.into_iter().enumerate() {
            let Some(window) = windows.iter().find(|window| window.id == presented.window) else {
                continue;
            };
            let selected = Some(window.id) == presentation.selected;
            let hovered = Some(window.id) == self.hovered && !window.read_only;
            let outer = to_lens(presented.geometry.outer);
            // Content brightness dims inactive cards; the hover wash below
            // belongs to a hovered card, whose item opacity is always 1.0.
            let item_opacity = if presentation.selected.is_none() || selected || hovered {
                1.0
            } else {
                presentation.inactive_content_brightness
            };
            frame.set_opacity(presentation.visibility * item_opacity);
            if hovered && !selected {
                frame.place(
                    &format!("tessera-window-switcher-card-{index}"),
                    &chrome_place(
                        outer,
                        preview::card_material(
                            &design,
                            preview::PreviewCardState::Hovered,
                            presented.corner_radius,
                        ),
                    ),
                    |frame| {
                        frame.column_ex(&layer_size(outer.w, outer.h), |_| {});
                    },
                );
            }

            let label_rect = to_lens(presented.geometry.label);
            let fallback_title = i18n.text(Message::UntitledWindow);
            let hierarchy_title = resolve_switcher_hierarchy_title(window, windows, fallback_title);
            let icon = window
                .app_id
                .as_deref()
                .and_then(|app_id| self.icons.get(&app_id.to_ascii_lowercase()))
                .or_else(|| self.icons.default_icon());
            let occupied_width = 16.0 + if icon.is_some() { 20.0 + 7.0 } else { 0.0 };
            let icon_tint = design.colors.application_text;
            let label = ellipsize(
                frame,
                &hierarchy_title,
                design.typography.label,
                (label_rect.w - occupied_width).max(0.0),
            );
            frame.place(
                &format!("tessera-window-switcher-label-{index}"),
                &chrome_place(
                    label_rect,
                    LayoutOpts {
                        bg: Color::TRANSPARENT,
                        radius: design.radii.control,
                        ..surface_layout()
                    },
                ),
                move |frame| {
                    frame.row_ex(
                        &LayoutOpts {
                            width: label_rect.w,
                            height: label_rect.h,
                            gap: 7.0,
                            pad: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        move |frame| {
                            if let Some(icon) = icon {
                                unsafe {
                                    frame.image_tinted(
                                        icon as *mut lens::sys::flux_image,
                                        20.0,
                                        20.0,
                                        icon_tint,
                                    );
                                }
                            }
                            frame.label_compact_sized(&label, design.typography.label);
                        },
                    );
                },
            );
        }
        frame.set_opacity(1.0);
    }

    fn prepare_window_switcher(
        &mut self,
        input: &Input,
        display: tessera_primitives::Rect,
        windows: &[Window],
        order: &[WindowId],
        selected: Option<WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        self.prepare(input, display, windows, order, selected)
    }

    fn captures_keyboard(&self) -> bool {
        self.open
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if self.open && matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            out.window_switcher_cancel = true;
            self.finish_window_switcher();
        }
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.open
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        _display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        self.open
            .then(|| {
                self.presentation.as_ref()?.cards.iter().find(|card| {
                    card.contains(x, y)
                        && windows
                            .iter()
                            .find(|window| window.id == card.window)
                            .is_some_and(|window| !window.read_only)
                })
            })
            .flatten()
            .map(|_| CursorShape::Pointer)
    }

    fn modal_active(&self) -> bool {
        self.open || self.visibility > 0.01
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn anim_pending(&self) -> bool {
        self.anim_active
    }

    #[cfg(test)]
    fn window_switcher_probe(
        &self,
    ) -> Option<(
        bool,
        bool,
        Option<tessera_desktop::window::WindowId>,
        f32,
        bool,
    )> {
        Some((
            self.anim_active,
            self.open,
            self.hovered,
            self.visibility,
            self.presentation.is_some(),
        ))
    }

    fn requires_composition(&self) -> bool {
        self.open || self.visibility > 0.01
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ReducedMotion(reduced) => self.set_reduced_motion(reduced),
            ChromeUpdate::AppCatalog(catalog) => self.icons = catalog.icons.clone(),
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartWindowSwitcher => self.start_window_switcher(),
            ChromeCommand::FinishWindowSwitcher => self.finish_window_switcher(),
            _ => {}
        }
    }

    fn window_switcher_active(&self) -> bool {
        self.open || self.visibility > 0.01
    }
}

fn to_lens(rect: tessera_primitives::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}

fn intersects(a: tessera_primitives::Rect, b: tessera_primitives::Rect) -> bool {
    a.origin.x < b.origin.x + b.size.w
        && b.origin.x < a.origin.x + a.size.w
        && a.origin.y < b.origin.y + b.size.h
        && b.origin.y < a.origin.y + a.size.h
}

/// A card's geometry while it is still moving.
///
/// The animated state is **floating point**, and it has to be. The layout is
/// integral (`Rect` holds `i32`), so an eased card snapped back to integers
/// every frame had a fixed point one pixel away from its target and never
/// reached it: the blend resolved anything inside 0.75 px to the target, the
/// next step landed on `target ± 1`, and "still moving" stayed true forever. The
/// switcher consequently reported an animation in flight on every frame it was
/// open — a full-output repaint, plus a re-composite of every card's live client
/// surface, at the panel refresh rate, for a strip that was not moving.
///
/// Keeping the eased geometry as `f32` and quantizing only the presented copy
/// makes the eased value converge on the target exactly, so "still moving" is a
/// property of the animation rather than of the rounding grid.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AnimCard {
    outer: AnimRect,
    preview: AnimRect,
    label: AnimRect,
}

impl AnimCard {
    fn from_card(card: crate::layout::window_switcher::Card) -> Self {
        Self {
            outer: AnimRect::from_rect(card.outer),
            preview: AnimRect::from_rect(card.preview),
            label: AnimRect::from_rect(card.label),
        }
    }

    /// The settled layout card this presents as.
    fn to_card(self) -> crate::layout::window_switcher::Card {
        crate::layout::window_switcher::Card {
            outer: self.outer.to_rect(),
            preview: self.preview.to_rect(),
            label: self.label.to_rect(),
        }
    }

    fn lerp(self, to: Self, blend: f32) -> Self {
        Self {
            outer: self.outer.lerp(to.outer, blend),
            preview: self.preview.lerp(to.preview, blend),
            label: self.label.lerp(to.label, blend),
        }
    }

    /// Whether this card is still off its target by more than a rounding step.
    fn is_moving(self, target: Self) -> bool {
        self.outer.is_moving(target.outer)
            || self.preview.is_moving(target.preview)
            || self.label.is_moving(target.label)
    }
}

/// A rectangle mid-flight, in output-logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AnimRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl AnimRect {
    fn from_rect(rect: tessera_primitives::Rect) -> Self {
        Self {
            x: rect.origin.x as f32,
            y: rect.origin.y as f32,
            w: rect.size.w as f32,
            h: rect.size.h as f32,
        }
    }

    fn to_rect(self) -> tessera_primitives::Rect {
        tessera_primitives::Rect::new(
            self.x.round() as i32,
            self.y.round() as i32,
            (self.w.round() as i32).max(1),
            (self.h.round() as i32).max(1),
        )
    }

    fn lerp(self, to: Self, blend: f32) -> Self {
        let blend = blend.clamp(0.0, 1.0);
        let mix = |from: f32, to: f32| from + (to - from) * blend;
        Self {
            x: mix(self.x, to.x),
            y: mix(self.y, to.y),
            w: mix(self.w, to.w),
            h: mix(self.h, to.h),
        }
    }

    /// Half a pixel: the finest difference that survives the quantisation the
    /// paint applies. A smaller one is the grid, not movement.
    fn is_moving(self, target: Self) -> bool {
        const EPSILON: f32 = 0.5;
        (self.x - target.x).abs() > EPSILON
            || (self.y - target.y).abs() > EPSILON
            || (self.w - target.w).abs() > EPSILON
            || (self.h - target.h).abs() > EPSILON
    }
}

fn layer_size(width: f32, height: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        ..Default::default()
    }
}

fn resolve_switcher_hierarchy_title(window: &Window, windows: &[Window], fallback: &str) -> String {
    let mut chain = Vec::new();
    let mut current_id = window.parent_id;
    let mut depth = 0;
    while let Some(pid) = current_id {
        if depth >= 10 {
            break;
        }
        if let Some(parent) = windows.iter().find(|w| w.id == pid) {
            let p_title = parent
                .title
                .as_deref()
                .or(parent.app_id.as_deref())
                .unwrap_or(fallback);
            chain.push(p_title);
            current_id = parent.parent_id;
            depth += 1;
        } else {
            break;
        }
    }
    let own_title = window
        .title
        .as_deref()
        .or(window.app_id.as_deref())
        .unwrap_or(fallback);

    if chain.is_empty() {
        if window.suspended_by_modal {
            format!("{own_title} ⧗")
        } else {
            own_title.to_string()
        }
    } else {
        chain.reverse();
        let mut full = chain.join(" › ");
        full.push_str(" › ");
        full.push_str(own_title);
        if window.suspended_by_modal {
            full.push_str(" ⧗");
        }
        full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switcher_hierarchy_breadcrumb_resolves_nested_chain() {
        let parent = Window {
            id: WindowId(1),
            title: Some("GIMP".into()),
            ..Default::default()
        };
        let dialog = Window {
            id: WindowId(2),
            title: Some("Color Picker".into()),
            parent_id: Some(WindowId(1)),
            ..Default::default()
        };
        let leaf = Window {
            id: WindowId(3),
            title: Some("Select Palette".into()),
            parent_id: Some(WindowId(2)),
            ..Default::default()
        };
        let windows = vec![parent.clone(), dialog.clone(), leaf.clone()];

        assert_eq!(
            resolve_switcher_hierarchy_title(&parent, &windows, "Untitled"),
            "GIMP"
        );
        assert_eq!(
            resolve_switcher_hierarchy_title(&dialog, &windows, "Untitled"),
            "GIMP › Color Picker"
        );
        assert_eq!(
            resolve_switcher_hierarchy_title(&leaf, &windows, "Untitled"),
            "GIMP › Color Picker › Select Palette"
        );
    }

    #[test]
    fn switcher_hierarchy_indicates_suspended_modal_state() {
        let parent = Window {
            id: WindowId(1),
            title: Some("Settings".into()),
            suspended_by_modal: true,
            ..Default::default()
        };
        let windows = vec![parent.clone()];
        assert_eq!(
            resolve_switcher_hierarchy_title(&parent, &windows, "Untitled"),
            "Settings ⧗"
        );
    }

    fn windows(count: u64) -> (Vec<Window>, Vec<WindowId>) {
        let order: Vec<_> = (1..=count).map(WindowId).collect();
        let windows = order.iter().copied().map(Window::new).collect();
        (windows, order)
    }

    #[test]
    fn held_switcher_opens_and_modifier_release_closes_it() {
        let mut switcher = WindowSwitcher::new();
        switcher.start_window_switcher();
        assert!(switcher.window_switcher_active());
        switcher.finish_window_switcher();
        assert!(!switcher.open);
        assert!(switcher.anim_pending());
    }

    #[test]
    fn card_animation_converges_without_overshoot() {
        let from = crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(0, 10, 100, 80),
            preview: tessera_primitives::Rect::new(0, 10, 100, 50),
            label: tessera_primitives::Rect::new(0, 60, 100, 30),
        };
        let to = crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(200, 10, 100, 80),
            preview: tessera_primitives::Rect::new(200, 10, 100, 50),
            label: tessera_primitives::Rect::new(200, 60, 100, 30),
        };
        let card = AnimCard::from_card(from).lerp(AnimCard::from_card(to), 0.25);
        assert_eq!(card.outer.x, 50.0);
        assert!(card.outer.x < to.outer.origin.x as f32);
    }

    /// The eased geometry is floating point, so it converges on its target
    /// exactly instead of stalling one pixel short of it.
    ///
    /// Quantising the animation state every frame gave the switcher a stable
    /// fixed point one pixel away from the layout target: `lerp` resolved
    /// anything inside 0.75 px to the target, the next blend landed on
    /// `target ± 1`, and the pair was a cycle. Every open frame then reported an
    /// animation in flight — a full-output repaint plus a re-composite of each
    /// card's live client surface, at the panel refresh rate, for a strip that
    /// was standing still.
    #[test]
    fn animated_card_geometry_reaches_its_target_exactly() {
        let target = crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(457, 445, 238, 190),
            preview: tessera_primitives::Rect::new(457, 445, 238, 152),
            label: tessera_primitives::Rect::new(457, 597, 238, 38),
        };
        let from = AnimCard::from_card(crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(453, 441, 242, 194),
            preview: tessera_primitives::Rect::new(453, 441, 242, 156),
            label: tessera_primitives::Rect::new(453, 595, 242, 38),
        });
        let target_anim = AnimCard::from_card(target);
        let mut current = from;
        let mut frames = 0;
        let blend = crate::widgets::motion::blend(SLIDE_RATE, 1.0 / 120.0, false);
        while current.is_moving(target_anim) && frames < 600 {
            current = current.lerp(target_anim, blend);
            frames += 1;
        }
        assert!(
            frames < 600,
            "the eased card must converge, or the switcher animates forever"
        );
        assert!(!current.is_moving(target_anim));
        assert_eq!(
            current.to_card(),
            target,
            "the presented card is the target"
        );
    }

    #[test]
    fn fixed_mode_stages_only_the_selected_card() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let input = Input::default();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(4);

        let first = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        let last = switcher
            .prepare(&input, display, &windows, &order, Some(order[3]))
            .unwrap();
        let base = crate::layout::window_switcher::fixed_layout(display, order.len());

        assert_eq!(first.mode, crate::layout::window_switcher::Mode::Fixed);
        let first_selected = first
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry;
        assert!(first_selected.outer.size.w > base.cards[0].outer.size.w);
        assert!(first_selected.outer.origin.y < base.cards[0].outer.origin.y);
        assert_eq!(
            last.cards
                .iter()
                .find(|card| card.window == order[0])
                .unwrap()
                .geometry,
            base.cards[0]
        );
        let last_selected = last
            .cards
            .iter()
            .find(|card| card.window == order[3])
            .unwrap()
            .geometry;
        assert_eq!(
            last_selected,
            preview::selected_geometry(
                base.cards[3],
                PreviewSelectionStyle::Staged,
                &Design::dark(),
            )
        );
        assert_eq!(last.selection_indicator.unwrap().outer, last_selected.outer);
        assert_eq!(
            last.inactive_content_brightness,
            Design::dark().preview.inactive_content_brightness
        );
    }

    #[test]
    fn carousel_wraps_through_the_adjacent_slot_with_a_centred_indicator() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let input = Input::default();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(5);

        let before = switcher
            .prepare(&input, display, &windows, &order, Some(order[4]))
            .unwrap();
        let after = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        let centre = display.size.w / 2;
        let incoming_before = before
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry
            .outer;
        let incoming_after = after
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry
            .outer;

        assert_eq!(after.mode, crate::layout::window_switcher::Mode::Carousel);
        assert!(incoming_before.origin.x + incoming_before.size.w / 2 > centre);
        assert_eq!(incoming_after.origin.x + incoming_after.size.w / 2, centre);
        let indicator = after.selection_indicator.unwrap().outer;
        assert_eq!(indicator.origin.x + indicator.size.w / 2, centre);
    }

    #[test]
    fn escape_requests_cancellation_without_selecting_a_window() {
        let mut switcher = WindowSwitcher::new();
        switcher.start_window_switcher();
        let mut out = ChromeEvents::default();
        switcher.key_char(
            &KeyChar {
                keysym: tessera_primitives::input::XKB_KEY_Escape,
                ch: None,
                mods: tessera_primitives::input::Mods::SUPER,
            },
            &mut out,
        );

        assert!(out.window_switcher_cancel);
        assert!(out.window_switcher_pick.is_none());
        assert!(!switcher.open);
    }

    #[test]
    fn the_visible_switcher_panel_is_one_painted_body() {
        let mut switcher = WindowSwitcher::new();
        switcher.presentation = Some(WindowSwitcherPresentation {
            mode: crate::layout::window_switcher::Mode::Fixed,
            panel: tessera_primitives::Rect::new(200, 160, 640, 240),
            cards: Vec::new(),
            selection_indicator: None,
            selected: None,
            inactive_content_brightness: Design::dark().preview.inactive_content_brightness,
            visibility: 0.6,
        });

        // The panel is one flat painted body at the floating-panel radius; no
        // frost rect sits beneath it and no material body backs it.
        let design = Design::dark();
        let style = design
            .surfaces
            .for_role(tessera_design::SurfaceRole::FloatingPanel);
        assert_eq!(style.radius, design.radii.panel_lg);
        assert!(
            style.shadow_alpha > 0.0,
            "the panel casts a separating shadow"
        );
        assert!(style.body.components().3 >= 236, "the panel body is opaque");
    }

    #[test]
    fn the_selected_switcher_card_reads_through_a_painted_wash() {
        let selected = WindowId(7);
        let card = crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(240, 182, 220, 170),
            preview: tessera_primitives::Rect::new(240, 182, 220, 132),
            label: tessera_primitives::Rect::new(240, 314, 220, 38),
        };
        let design = Design::dark();
        // Selection is a neutral wash on the parent body, at the shared
        // control radius. It never becomes a second body or an accent outline.
        let selected_wash = crate::component::preview::card_material(
            &design,
            crate::component::preview::PreviewCardState::Selected,
            design.radii.control,
        );
        assert_eq!(selected_wash.bg, design.selection.selected_tint);
        assert_eq!(selected_wash.border_width, 0.0);
        assert_eq!(selected_wash.radius, design.radii.control);
        assert_eq!(card.outer.size.w, 220);
        assert_eq!(selected, WindowId(7));
    }
}
