//! Held-Super window switcher chrome.
//!
//! The compositor renderer paints each live window into the preview rects
//! from `crate::layout::window_switcher`; this component adds the glass panel,
//! selection focus, icons, labels, shared carousel animation, and pointer
//! hit-testing. Releasing Super or clicking a card closes the strip.

use std::collections::{HashMap, HashSet};

use optics_ui::{Align, Color, Frame, Input, LayoutOpts, Rect};
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
    /// The display rectangle the current spatial model was resolved for.
    /// Deliberately excludes the window count: a window closing mid-session
    /// retargets the remaining cards instead of discarding their animation.
    layout_key: Option<(i32, i32, i32, i32)>,
    /// Frozen at session start so a count change cannot swap a fixed strip for
    /// a carousel (or back) while the user is looking at it.
    mode_locked: bool,
    mode: crate::layout::window_switcher::Mode,
    animated_cards: HashMap<WindowId, AnimCard>,
    /// Continuous, unwrapped carousel slot per window. A window crossing the
    /// ring seam steps by one slot like every other card; its off-panel copy is
    /// drawn one ring away, so nothing is teleported while visible.
    animated_slots: HashMap<WindowId, f32>,
    animated_selection: Option<AnimCard>,
    /// The panel body is part of the motion, not a static frame around it: a
    /// closing window narrows a fixed strip and the body must follow the cards.
    animated_panel: Option<AnimRect>,
    /// The panel extent the damage walk may need to repaint this frame and the
    /// one before it. Kept as the union through a transition so the vacated
    /// edge is repainted even while the body shrinks.
    damage_panel: Option<tessera_primitives::Rect>,
    presentation: Option<WindowSwitcherPresentation>,
    hovered: Option<WindowId>,
    /// Whether the card under the pointer changed on the frame most recently
    /// prepared. Damage assessment runs after preparation, so it must be able
    /// to repaint the wash the previous frame left behind even though the new
    /// hover is already `None`.
    hover_dirty: bool,
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
            mode_locked: false,
            mode: crate::layout::window_switcher::Mode::Fixed,
            animated_cards: HashMap::new(),
            animated_slots: HashMap::new(),
            animated_selection: None,
            animated_panel: None,
            damage_panel: None,
            presentation: None,
            hovered: None,
            hover_dirty: false,
            design: Design::dark(),
        }
    }

    fn start_window_switcher(&mut self) {
        if !self.open {
            self.order.clear();
            self.layout_key = None;
            self.mode_locked = false;
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
            self.animated_slots.clear();
            self.animated_selection = None;
            self.animated_panel = None;
            self.damage_panel = None;
            self.hovered = None;
            self.hover_dirty = false;
            self.mode_locked = false;
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

        // The spatial model and the display are session-stable. Window count is
        // deliberately not part of this key: a closing window retargets the
        // remaining cards, while the mode itself stays fixed for the whole
        // session so a threshold crossing cannot swap models mid-flight.
        let display_key = (
            display.origin.x,
            display.origin.y,
            display.size.w,
            display.size.h,
        );
        if self.layout_key != Some(display_key) || !self.mode_locked {
            // A display change re-derives the model so it still fits; only a
            // count change inside one session leaves the locked model alone.
            self.mode = crate::layout::window_switcher::mode(display, self.order.len());
            self.layout_key = Some(display_key);
            self.animated_cards.clear();
            self.animated_slots.clear();
            self.animated_selection = None;
            self.animated_panel = None;
            self.damage_panel = None;
        }
        if self.open {
            self.mode_locked = true;
        }

        let target = crate::layout::window_switcher::layout_for_mode(
            display,
            self.order.len(),
            selected_index,
            self.mode,
        );
        // Reduced motion is a one-frame resolve, expressed once by `optics-motion`
        // (`blend == 1.0`), so no separate branch is needed for the panel or
        // the fixed strip.
        let blend = crate::widgets::motion::blend(SLIDE_RATE, dt, self.reduced_motion);
        let target_panel = AnimRect::from_rect(target.panel);
        let panel_anim = match self.animated_panel {
            Some(mut current) => {
                current = current.lerp(target_panel, blend);
                self.animated_panel = Some(current);
                current
            }
            None => {
                self.animated_panel = Some(target_panel);
                target_panel
            }
        };
        let panel_moving = panel_anim.is_moving(target_panel);
        let panel = panel_anim.to_rect();
        // Keep the footprint of both the painted and the target body while the
        // panel is in flight; a shrinking body must erase the pixels it vacated.
        self.damage_panel = Some(if panel_moving {
            self.damage_panel
                .map_or(panel, |previous| previous.union(panel))
                .union(target.panel)
        } else {
            panel
        });

        let mut cards = Vec::with_capacity(self.order.len());
        let mut cards_moving = false;
        let mut selection_moving = false;

        match self.mode {
            crate::layout::window_switcher::Mode::Fixed => {
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
                for (id, target_card) in
                    self.order.iter().copied().zip(target_cards.iter().copied())
                {
                    let target_anim = AnimCard::from_card(target_card);
                    let current = self.animated_cards.entry(id).or_insert(target_anim);
                    *current = current.lerp(target_anim, blend);
                    cards_moving |= current.is_moving(target_anim);
                    push_card(&mut cards, panel, id, current.to_card(), &design);
                }
                self.animated_cards.retain(|id, _| self.order.contains(id));
                self.animated_slots.clear();

                let target_selection = target_cards.get(selected_index).copied();
                selection_moving = if let Some(target_selection) = target_selection {
                    let target_anim = AnimCard::from_card(target_selection);
                    let current = self.animated_selection.get_or_insert(target_anim);
                    *current = current.lerp(target_anim, blend);
                    current.is_moving(target_anim)
                } else {
                    self.animated_selection = None;
                    false
                };
            }
            crate::layout::window_switcher::Mode::Carousel => {
                // Animate one continuous slot scalar per window. A card crossing
                // the ring seam moves exactly one slot like every other card;
                // its equivalent copy one ring away supplies the visible card
                // on the other side, so nothing is teleported while visible.
                let count = self.order.len();
                let ring = count as f32;
                self.animated_cards.clear();
                for (id, canonical) in self.order.iter().copied().zip(target.slots.iter().copied())
                {
                    let current = self.animated_slots.entry(id).or_insert(canonical as f32);
                    // Pick the ring copy nearest the current position. This
                    // makes the canonical -2 -> +2 representation change on the
                    // seam a -1 slot step instead of a jump.
                    let target_slot =
                        canonical as f32 + ring * ((*current - canonical as f32) / ring).round();
                    *current = crate::widgets::motion::approach(
                        *current,
                        target_slot,
                        SLIDE_RATE,
                        dt,
                        self.reduced_motion,
                    );
                    cards_moving |= (*current - target_slot).abs() > 0.002;

                    for lane in -1..=1 {
                        let slot = *current + lane as f32 * ring;
                        let card =
                            crate::layout::window_switcher::carousel_card(display, count, slot);
                        if !intersects(panel, card.outer) {
                            continue;
                        }
                        let is_selected = Some(id) == selected && slot.abs() < 0.5;
                        let geometry = if is_selected {
                            preview::selected_geometry(card, PreviewSelectionStyle::Staged, &design)
                        } else {
                            card
                        };
                        cards.push(PreviewCard {
                            window: id,
                            geometry,
                            corner_radius: design.radii.control,
                        });
                    }
                }
                self.animated_slots.retain(|id, _| self.order.contains(id));

                self.animated_selection = (count > 0).then(|| {
                    AnimCard::from_card(preview::selected_geometry(
                        crate::layout::window_switcher::carousel_card(display, count, 0.0),
                        PreviewSelectionStyle::Staged,
                        &design,
                    ))
                });
            }
        }

        // The compositor paints previews in this order and chrome paints labels
        // in its own pass; both must agree that the selected card is on top.
        cards.sort_by_key(|card| Some(card.window) == selected);

        let raw = input.as_raw();
        let previous_hovered = self.hovered;
        self.hovered = if self.open && contains(panel, raw.cursor.x, raw.cursor.y) {
            preview::hit_test(&cards, selected, raw.cursor.x, raw.cursor.y)
        } else {
            None
        };

        self.hover_dirty = self.hovered != previous_hovered;
        self.anim_active = cards_moving || selection_moving || panel_moving;
        self.anim_active |= self.hover_dirty;
        if !self.anim_active {
            self.anim_active = (self.visibility - if self.open { 1.0 } else { 0.0 }).abs() > 0.002;
        }

        let presentation = WindowSwitcherPresentation {
            mode: self.mode,
            panel,
            cards,
            selection_indicator: self.animated_selection.map(AnimCard::to_card),
            selected,
            inactive_content_brightness: design.preview.inactive_content_brightness,
            visibility: self.visibility,
        };
        self.presentation = Some(presentation.clone());
        Some(presentation)
    }

    /// The output region the settled switcher may repaint.
    ///
    /// While the scrim is fading the whole output is live, so there is no
    /// narrower answer. Once the fade has resolved, every pixel the switcher
    /// owns is inside the floating panel's body and its painted elevation halo;
    /// the live previews and labels are clipped to that same body. Reporting
    /// this band keeps a settling card, a hover change, or an animated panel
    /// edge from becoming a full-output composite.
    fn damage_band(&self, display: (f32, f32)) -> Option<Vec<Rect>> {
        if !self.open || self.visibility < 0.999 {
            return None;
        }
        let panel = self.damage_panel.or_else(|| {
            self.presentation
                .as_ref()
                .map(|presentation| presentation.panel)
        })?;
        let mut region = panel;
        for (halo, _) in tessera_design::materials::elevation_halo_for_body(
            &self.design,
            tessera_design::SurfaceRole::FloatingPanel,
            to_ui(panel),
            self.design.radii.panel_lg,
            1.0,
        ) {
            region = region.union(tessera_primitives::Rect::new(
                halo.x.floor() as i32,
                halo.y.floor() as i32,
                (halo.x + halo.w).ceil() as i32 - halo.x.floor() as i32,
                (halo.y + halo.h).ceil() as i32 - halo.y.floor() as i32,
            ));
        }
        let display = tessera_primitives::Rect::new(
            0,
            0,
            display.0.max(1.0) as i32,
            display.1.max(1.0) as i32,
        );
        region.intersect(display).map(|region| vec![to_ui(region)])
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
        // Preparation owns the display rectangle and the session model; a
        // fallback prepare from `render` would invent a `(0, 0)` display after
        // the real one and could freeze the wrong spatial model for the session.
        let Some(presentation) = self.presentation.clone() else {
            return;
        };

        let raw = input.as_raw();
        // Hover is resolved in `prepare`, before the damage assessment reads
        // `anim_pending`; a pointer move that crosses a card must repaint the
        // frame that observes it, not the following one.
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

        // The whole strip fades through the optics-ui opacity switch; per-item
        // dimming multiplies on top of it below.
        frame.set_opacity(presentation.visibility);
        let design = self.design;
        let panel = to_ui(presentation.panel);
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

        // The panel body and its elevation halo are painted above; everything
        // below is content that belongs to the strip and must not paint into
        // the desktop outside it. A scoped optics-ui clip stamps every node built
        // here — including the absolutely placed labels and hover washes that
        // escape the panel container's flow — so the boundary is enforced by
        // the renderer, not only by per-rect arithmetic.
        frame.clip_begin(panel);

        if let Some(indicator) = presentation.selection_indicator
            && let Some(visible) = visible_rect(indicator.outer, presentation.panel)
        {
            let selection = to_ui(visible);
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
            // Content brightness dims inactive cards; the hover wash below
            // belongs to a hovered card, whose item opacity is always 1.0.
            let item_opacity = if presentation.selected.is_none() || selected || hovered {
                1.0
            } else {
                presentation.inactive_content_brightness
            };
            frame.set_opacity(presentation.visibility * item_opacity);
            if hovered
                && !selected
                && let Some(visible) = visible_rect(presented.geometry.outer, presentation.panel)
            {
                let outer = to_ui(visible);
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

            // A card at the carousel edge is clipped by the panel body; its
            // label must stop at the same edge instead of spilling into the
            // desktop outside the panel. A label whose left edge is already
            // outside cannot be painted honestly, so it is omitted until the
            // card has entered far enough to show one.
            let Some(label_visible) = clipped_label_rect(presented.geometry, presentation.panel)
            else {
                continue;
            };
            let label_rect = to_ui(label_visible);
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
                                        icon as *mut optics_ui::sys::optics_gfx_image,
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
        frame.clip_end();
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
                let presentation = self.presentation.as_ref()?;
                if !contains(presentation.panel, x, y) {
                    return None;
                }
                presentation.cards.iter().find(|card| {
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

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        self.damage_band(display)
    }

    fn pointer_damage_regions(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        if !self.open {
            return None;
        }
        // Outside the panel nothing is hoverable, so a move that neither left
        // a hovered card nor entered the panel changes no pixels at all.
        let in_panel = self
            .presentation
            .as_ref()
            .is_some_and(|presentation| contains(presentation.panel, x, y));
        if self.hovered.is_none() && !in_panel && !self.hover_dirty {
            return Some(Vec::new());
        }
        self.damage_band(display)
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

fn to_ui(rect: tessera_primitives::Rect) -> Rect {
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

fn contains(rect: tessera_primitives::Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x as f32
        && y >= rect.origin.y as f32
        && x < (rect.origin.x + rect.size.w) as f32
        && y < (rect.origin.y + rect.size.h) as f32
}

/// Push a card only when its body may contribute pixels. Chrome clips the
/// composed body to the panel, so cards wholly outside it are not presented
/// either — keeping hit-testing and paint in one coordinate contract.
fn push_card(
    cards: &mut Vec<PreviewCard>,
    panel: tessera_primitives::Rect,
    window: WindowId,
    geometry: crate::layout::window_switcher::Card,
    design: &Design,
) {
    if intersects(panel, geometry.outer) {
        cards.push(PreviewCard {
            window,
            geometry,
            corner_radius: design.radii.control,
        });
    }
}

/// The part of a rect inside the panel, or `None` when nothing is visible.
fn visible_rect(
    rect: tessera_primitives::Rect,
    panel: tessera_primitives::Rect,
) -> Option<tessera_primitives::Rect> {
    let x = rect.origin.x.max(panel.origin.x);
    let y = rect.origin.y.max(panel.origin.y);
    let right = (rect.origin.x + rect.size.w).min(panel.origin.x + panel.size.w);
    let bottom = (rect.origin.y + rect.size.h).min(panel.origin.y + panel.size.h);
    (right > x && bottom > y).then(|| tessera_primitives::Rect::new(x, y, right - x, bottom - y))
}

/// The label rect narrowed to the panel's right edge, or `None` when the card
/// has not entered far enough to show one.
///
/// optics-ui has no general nested clip for non-scrolling containers, so the card
/// label is the one piece of chrome that must enforce the same boundary as the
/// compositor's preview scissor by construction. The left edge is required to
/// be inside the panel: shifting a left-clipped label to the panel edge would
/// paint a title that is not actually there.
fn clipped_label_rect(
    card: crate::layout::window_switcher::Card,
    panel: tessera_primitives::Rect,
) -> Option<tessera_primitives::Rect> {
    let label = card.label;
    // The label's own leading pad is transparent, so a card may overhang the
    // left edge by exactly that pad before its first painted pixel escapes.
    if label.origin.x + 8 < panel.origin.x || label.origin.y < panel.origin.y {
        return None;
    }
    if label.origin.y + label.size.h > panel.origin.y + panel.size.h {
        return None;
    }
    let right = (label.origin.x + label.size.w).min(panel.origin.x + panel.size.w);
    let width = right - label.origin.x;
    // Below the icon-plus-padding floor the label is unreadable noise, and an
    // icon that does not fit in the narrowed row would itself spill past the
    // panel edge. The card is still represented by its preview and hover wash.
    (width >= 8 + 20 + 7 + 8)
        .then(|| tessera_primitives::Rect::new(label.origin.x, label.origin.y, width, label.size.h))
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
    fn carousel_animation_carries_the_seam_card_by_one_slot() {
        let mut switcher = WindowSwitcher::new();
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(5);
        let input = Input::new((1920.0, 1080.0), 1.0 / 60.0);

        let before = switcher
            .prepare(&input, display, &windows, &order, Some(order[4]))
            .unwrap();
        assert_eq!(before.mode, crate::layout::window_switcher::Mode::Carousel);
        let seam = order[2];
        let seam_before = switcher.animated_slots[&seam];
        assert_eq!(seam_before, -2.0);

        let after = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        let seam_after = switcher.animated_slots[&seam];
        assert!(
            seam_after < seam_before,
            "the seam card moves with the strip instead of jumping to +2"
        );
        assert!(
            seam_after > -3.0,
            "the seam card has not teleported to the far side"
        );
        // While it straddles the seam, the same window is represented by the
        // copy leaving one edge and the copy entering the other.
        let copies = after
            .cards
            .iter()
            .filter(|card| card.window == seam)
            .count();
        assert!((1..=2).contains(&copies));
    }

    #[test]
    fn the_spatial_mode_is_frozen_for_the_session() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let input = Input::default();
        let (windows, order) = windows(5);

        let first = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        assert_eq!(first.mode, crate::layout::window_switcher::Mode::Carousel);

        // A window closing mid-session retargets the remaining cards, but the
        // carousel model stays put; it must not swap to a fixed strip.
        let remaining = &order[..3];
        let remaining_windows = &windows[..3];
        let after = switcher
            .prepare(
                &input,
                display,
                remaining_windows,
                remaining,
                Some(remaining[0]),
            )
            .unwrap();
        assert_eq!(after.mode, crate::layout::window_switcher::Mode::Carousel);
        assert_eq!(
            switcher
                .animated_slots
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            remaining.iter().copied().collect::<HashSet<_>>(),
            "surviving cards keep their animation state"
        );
    }

    #[test]
    fn labels_are_confined_to_the_panel_edge() {
        let panel = tessera_primitives::Rect::new(100, 100, 200, 100);
        let card = |x: i32| crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(x, 120, 100, 60),
            preview: tessera_primitives::Rect::new(x, 120, 100, 22),
            label: tessera_primitives::Rect::new(x, 142, 100, 38),
        };

        // A card entering from the right keeps only the label columns that are
        // inside the panel.
        assert_eq!(
            clipped_label_rect(card(250), panel).map(|rect| rect.size.w),
            Some(50)
        );
        // A card whose left edge is still outside cannot paint a title at all.
        assert!(clipped_label_rect(card(50), panel).is_none());
        // Fully inside is unchanged.
        assert_eq!(clipped_label_rect(card(150), panel), Some(card(150).label));
        // Vertically outside the panel is not visible.
        let below = crate::layout::window_switcher::Card {
            outer: tessera_primitives::Rect::new(150, 220, 100, 60),
            preview: tessera_primitives::Rect::new(150, 220, 100, 22),
            label: tessera_primitives::Rect::new(150, 242, 100, 38),
        };
        assert!(clipped_label_rect(below, panel).is_none());
    }

    #[test]
    fn hover_is_resolved_before_damage_and_inside_the_panel() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(5);
        let first = switcher
            .prepare(&Input::default(), display, &windows, &order, Some(order[0]))
            .unwrap();
        let selected = first
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry
            .outer;

        let mut input = Input::default();
        input.set_cursor(
            (selected.origin.x + selected.size.w / 2) as f32,
            (selected.origin.y + selected.size.h / 2) as f32,
        );
        switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        assert_eq!(switcher.hovered, Some(order[0]));
        assert!(switcher.anim_pending());

        // A pointer outside the panel is not a card hit even when an off-panel
        // copy of a card happens to contain that point.
        input.set_cursor((display.size.w + 40) as f32, (display.size.h / 2) as f32);
        switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        assert_eq!(switcher.hovered, None);
        // The wash the previous frame painted must still be damaged even
        // though the new hover is already `None`.
        assert!(switcher.hover_dirty);
        assert!(
            !switcher
                .pointer_damage_regions(
                    (display.size.w + 40) as f32,
                    (display.size.h / 2) as f32,
                    (display.size.w as f32, display.size.h as f32),
                    &windows,
                    &WorkspaceSnapshot {
                        outputs: Vec::new()
                    },
                )
                .unwrap()
                .is_empty()
        );

        // A second move in the same empty region changes nothing.
        switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        assert!(!switcher.hover_dirty);
        assert!(
            switcher
                .pointer_damage_regions(
                    (display.size.w + 40) as f32,
                    (display.size.h / 2) as f32,
                    (display.size.w as f32, display.size.h as f32),
                    &windows,
                    &WorkspaceSnapshot {
                        outputs: Vec::new()
                    },
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_selected_card_is_presented_last_for_both_passes() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(5);
        let presentation = switcher
            .prepare(&Input::default(), display, &windows, &order, Some(order[0]))
            .unwrap();
        assert_eq!(presentation.cards.last().unwrap().window, order[0]);
    }

    #[test]
    fn a_closing_window_animates_the_panel_body_with_the_cards() {
        let mut switcher = WindowSwitcher::new();
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let input = Input::new((1920.0, 1080.0), 1.0 / 60.0);
        let (windows, order) = windows(4);

        let before = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        assert_eq!(before.mode, crate::layout::window_switcher::Mode::Fixed);

        let remaining = &order[..3];
        let after = switcher
            .prepare(
                &input,
                display,
                &windows[..3],
                remaining,
                Some(remaining[0]),
            )
            .unwrap();
        let target = crate::layout::window_switcher::fixed_layout(display, 3)
            .panel
            .size
            .w;
        assert!(after.panel.size.w < before.panel.size.w);
        assert!(
            after.panel.size.w > target,
            "the body eases toward the narrower target instead of snapping"
        );
        assert!(switcher.anim_pending());
    }

    #[test]
    fn a_settled_switcher_reports_only_its_panel_band() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(4);
        let presentation = switcher
            .prepare(&Input::default(), display, &windows, &order, Some(order[0]))
            .unwrap();

        let regions = switcher.damage_regions(&windows, (1920.0, 1080.0)).unwrap();
        assert_eq!(regions.len(), 1);
        let region = regions[0];
        assert!(region.w < 1920.0 && region.h < 1080.0);
        assert!(region.x <= presentation.panel.origin.x as f32);
        assert!(region.y <= presentation.panel.origin.y as f32);
        assert!(
            region.x + region.w >= (presentation.panel.origin.x + presentation.panel.size.w) as f32
        );

        // The closing fade keeps the full-output scrim live, so no narrower
        // answer is honest.
        switcher.finish_window_switcher();
        assert!(
            switcher
                .damage_regions(&windows, (1920.0, 1080.0))
                .is_none()
        );
    }

    #[test]
    fn pointer_damage_is_empty_outside_the_panel_and_non_empty_inside() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let display = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(4);
        let presentation = switcher
            .prepare(&Input::default(), display, &windows, &order, Some(order[0]))
            .unwrap();
        let panel = presentation.panel;

        let outside = switcher
            .pointer_damage_regions(
                -20.0,
                panel.origin.y as f32 + 10.0,
                (1920.0, 1080.0),
                &windows,
                &WorkspaceSnapshot {
                    outputs: Vec::new(),
                },
            )
            .unwrap();
        assert!(outside.is_empty());

        let inside = switcher
            .pointer_damage_regions(
                (panel.origin.x + panel.size.w / 2) as f32,
                (panel.origin.y + panel.size.h / 2) as f32,
                (1920.0, 1080.0),
                &windows,
                &WorkspaceSnapshot {
                    outputs: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(inside.len(), 1);
        assert!(inside[0].w < 1920.0 && inside[0].h < 1080.0);
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
