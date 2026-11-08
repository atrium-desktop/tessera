//! The SNI tray shelf: a horizontal pager of item cells, plus its dbusmenu
//! popover (ADR-0221).
//!
//! The shelf is a **horizontal, bottom-anchored lane**. Its capacity is derived
//! from the display and the shell's reserved bands ([INV-CHROME-02]), it has no
//! scroll viewport ([INV-CHROME-03]), and when the items exceed the lane the
//! shelf paginates with a chevron at each end whose disabled state is real — the
//! first page cannot go back, the last cannot go forward, and each end renders
//! that fact ([INV-CHROME-07]).
//!
//! Geometry lives in [`super::tray_shelf`] as pure functions; this module only
//! paints and dispatches.

use super::*;

/// A pressable's drawn box when lens resolved it, else the pure layout's
/// declared slot.
///
/// The first frame a cell exists, lens has no snapshot to resolve the flow box
/// from and reports an empty rect. Every consumer of a cell's rect (the menu
/// anchor, the pointer bands) must still get a real box, and the pure layout is
/// the one authority that can supply it — deriving these two from different
/// sources is exactly how a drawn shelf and an interactive shelf drift apart.
fn drawn_or_declared(drawn: Rect, declared: Rect) -> Rect {
    if drawn.w > 0.0 && drawn.h > 0.0 {
        drawn
    } else {
        declared
    }
}

impl ControlCenter {
    /// Render the tray shelf at the bottom-left anchor.
    ///
    /// StatusNotifierItem icons run **left to right** along one row anchored to
    /// the bottom edge. Left-click activates an item; right-click opens the
    /// host-rendered dbusmenu popover (or falls back to `SecondaryActivate` when
    /// the item exposes no `Menu` object). Hover and menu-open are the cell
    /// pressable's own background, never a sibling `place()` overlay
    /// ([INV-CHROME-06]).
    pub(crate) fn render_tray_shelf(
        &mut self,
        f: &mut Frame,
        anchor: Rect,
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
    ) {
        let cells = self.sni_cells();
        self.render_tray_shelf_cells(f, anchor, &cells, progress, cursor, i18n);
    }

    /// Paint an explicit cell list into the shelf.
    ///
    /// Split from [`Self::render_tray_shelf`] so the shelf's behaviour is
    /// drivable with a known item set: paging is a function of
    /// `(anchor, item count, page)`, and a caller that already holds the cells
    /// should not have to re-derive them through the projection to exercise it.
    pub(crate) fn render_tray_shelf_cells(
        &mut self,
        f: &mut Frame,
        anchor: Rect,
        cells: &[SniCell],
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
    ) {
        if anchor.w < 32.0 || anchor.h < TRAY_CELL {
            return;
        }
        let hud = self.panel_colors();
        let rise = (1.0 - progress) * 16.0;
        let anchor = Rect {
            y: anchor.y + rise,
            ..anchor
        };

        if cells.is_empty() {
            // Nothing to show, so nothing is paged either.
            self.tray_shelf = None;
            self.tray_page = 0;
            return;
        }

        // Resolve the shelf geometry before any painting, so capacity, page
        // count and each chevron's reachability are a pure function of the
        // anchor and the item count ([INV-CHROME-05], [INV-CHROME-07]).
        let layout = tray_shelf::shelf_layout(anchor, cells.len(), self.tray_page);
        // The resolver owns the clamp: if the current page stopped existing
        // (items left), the resolved page is the truth from here on.
        self.tray_page = layout.page;
        // Cache the resolved layout: click-away hit-testing and the popover
        // anchors read this instead of re-deriving the geometry, so the drawn
        // shelf and the interactive shelf cannot disagree.
        self.tray_shelf = Some(layout);
        if layout.visible_slots() == 0 {
            return;
        }

        // Distill the per-cell visuals before the layout closures: those
        // capture disjoint borrows, so `self` method calls happen here.
        let fallback_themed = self.themed_icon("application-x-executable-symbolic");
        let visuals: Vec<TrayCellVisual> = cells
            .iter()
            .map(|cell| TrayCellVisual {
                key: cell.key.clone(),
                title: truncate(&cell.title, 12),
                has_menu: cell.has_menu,
                texture: if cell.textured {
                    self.tray
                        .as_ref()
                        .and_then(|tray| tray.texture(&cell.key))
                        .map(|image| image.as_raw())
                } else {
                    None
                },
                fallback: fallback_themed.map(|icon| icon as *mut lens::sys::flux_image),
            })
            .collect();

        let cell_w = layout.cell_width();
        let surface = layout.surface;
        let plate_radius = cell_w * 0.32;

        let original = f.theme();

        // Interactions are captured inside the layout closures and dispatched
        // after them: opening a popover or turning a page mutates `self`.
        let mut activations: Vec<String> = Vec::new();
        let mut secondary: Vec<(String, bool)> = Vec::new();
        let mut resolved: Vec<(String, Rect)> = Vec::new();
        let mut page_step: i32 = 0;

        // The capsule: one recessed surface sized to the *occupied* slots, so a
        // two-icon tray is a small pill rather than a wide empty bar.
        f.place(
            "tessera-hud-tray-surface",
            &chrome_place(
                surface,
                LayoutOpts {
                    radius: surface.h * 0.5,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(&sized(surface.w, surface.h), |_| {});
            },
        );

        f.place(
            "tessera-hud-tray-shelf",
            &chrome_place(surface, transparent()),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: surface.w,
                        height: surface.h,
                        pad: TRAY_PAD,
                        gap: TRAY_GAP,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        // Walk the slots by index rather than over a `Vec`:
                        // this runs once per slot per frame, so a heap
                        // allocation here would be per-frame churn on the
                        // chrome path.
                        for index in 0..layout.visible_slots() {
                            let Some(slot) = layout.slot_at(index) else {
                                break;
                            };
                            match slot {
                                tray_shelf::ShelfSlot::Item(offset) => {
                                    let Some(cell) = visuals.get(offset) else {
                                        continue;
                                    };
                                    let is_open = self.menu_open_for.as_deref() == Some(&cell.key);
                                    // The plate is the pressable's own
                                    // background ([INV-CHROME-06]). Hover needs
                                    // no explicit colour: the pressable's theme
                                    // emphasis lerps `accent_surface_hover` in
                                    // behind the glyph. Only the menu-open
                                    // "selected" state is a resting background.
                                    let (plate_bg, plate_border) = if is_open {
                                        (hud.selection_surface, hud.accent.with_alpha(80))
                                    } else {
                                        (Color::TRANSPARENT, Color::TRANSPARENT)
                                    };
                                    let texture = cell.texture;
                                    let fallback = cell.fallback;
                                    let title = cell.title.clone();
                                    let key = cell.key.clone();
                                    let has_menu = cell.has_menu;
                                    let (response, _) = f.pressable_row(
                                        &format!("tessera-hud-tray-item-{key}"),
                                        &title,
                                        &LayoutOpts {
                                            width: cell_w,
                                            height: TRAY_CELL,
                                            cross: Align::Center,
                                            radius: plate_radius,
                                            bg: plate_bg,
                                            border: plate_border,
                                            border_width: if is_open { 1.0 } else { 0.0 },
                                            ..Default::default()
                                        },
                                        |f, _| {
                                            f.centered(cell_w, TRAY_CELL, |f| match texture {
                                                Some(texture) => unsafe {
                                                    f.image(texture, TRAY_ICON, TRAY_ICON)
                                                },
                                                None => match fallback {
                                                    Some(icon) => unsafe {
                                                        f.image(icon, TRAY_ICON, TRAY_ICON)
                                                    },
                                                    None => f.icon(Icon::FileText, TRAY_ICON),
                                                },
                                            });
                                        },
                                    );
                                    // Trust the drawn box when lens resolved
                                    // it; fall back to the pure layout's slot
                                    // on the first frame, so a menu opened by a
                                    // press can never anchor to a zero rect.
                                    resolved.push((
                                        key.clone(),
                                        drawn_or_declared(response.rect, layout.slot_rect(index)),
                                    ));
                                    if response.clicked {
                                        activations.push(key);
                                    } else if response.right_clicked {
                                        secondary.push((key, has_menu));
                                    }
                                }
                                tray_shelf::ShelfSlot::Previous => {
                                    // The layout owns each end's reachability:
                                    // the renderer never decides on its own
                                    // whether an end is live ([INV-CHROME-07]).
                                    let enabled = layout.has_previous();
                                    let clicked = Self::pager_cell(
                                        f,
                                        "tessera-hud-tray-page-previous",
                                        Icon::ChevronLeft,
                                        i18n.text(Message::TrayPreviousPage),
                                        enabled,
                                        cell_w,
                                        hud,
                                    );
                                    if clicked {
                                        page_step = -1;
                                    }
                                }
                                tray_shelf::ShelfSlot::Next => {
                                    let enabled = layout.has_next();
                                    let clicked = Self::pager_cell(
                                        f,
                                        "tessera-hud-tray-page-next",
                                        Icon::ChevronRight,
                                        i18n.text(Message::TrayNextPage),
                                        enabled,
                                        cell_w,
                                        hud,
                                    );
                                    if clicked {
                                        page_step = 1;
                                    }
                                }
                            }
                        }
                    },
                );
            },
        );

        f.set_theme(original);
        let (x, y) = (cursor.0 as i32, cursor.1 as i32);
        for key in activations {
            self.send_tray_command(TrayCommand::Activate { key, x, y });
        }
        for (key, has_menu) in secondary {
            // Items that expose a Menu object path get the host-rendered
            // popover; everything else keeps the SNI `SecondaryActivate`.
            if has_menu {
                let cell_rect = resolved
                    .iter()
                    .find(|(k, _)| k == &key)
                    .map(|(_, rect)| *rect)
                    // The first item slot, not slot 0: with the pager present
                    // slot 0 is the leading chevron.
                    .unwrap_or_else(|| layout.slot_rect(layout.first_item_slot()));
                self.menu_owner = cell_rect;
                self.menu_open_for = Some(key.clone());
                self.menu_path.clear();
                self.menu_just_opened = true;
                self.send_tray_command(TrayCommand::FetchMenu { key });
            } else {
                self.send_tray_command(TrayCommand::SecondaryActivate { key, x, y });
            }
        }

        // A page turn is applied after the frame: the shelf repaints on the next
        // one with the new slice, so the icons the pointer is over cannot move
        // under it mid-press.
        if page_step != 0 {
            let target = if page_step < 0 {
                layout
                    .page
                    .saturating_sub(page_step.unsigned_abs() as usize)
            } else {
                // `has_next` already gated the click, and `pages` is exact, so
                // the target can never exceed the last page.
                (layout.page + page_step as usize).min(layout.pages - 1)
            };
            self.tray_page = target;
            // The cell the popover was anchored to is not on the new page at
            // all, so it must go with the turn rather than one frame later.
            if let Some(key) = self.menu_open_for.take() {
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }

        // Keep the owner rect fresh against relayout or item movement.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some((_, rect)) = resolved.iter().find(|(k, _)| k == &key) {
                self.menu_owner = *rect;
            } else {
                // The owning cell scrolled to another page: the popover has no
                // anchor and must close rather than float over the wrong icon.
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// One pager chevron.
    ///
    /// The reachable state is the layout's, not the renderer's: `enabled` comes
    /// from [`tray_shelf::TrayShelfLayout::has_previous`]/`has_next`, so the
    /// first page's back chevron and the last page's forward chevron are drawn
    /// dimmed and refuse the press ([INV-CHROME-07]). That is the whole point:
    /// a chevron that looks live but does nothing is worse than no chevron.
    fn pager_cell(
        f: &mut Frame,
        id: &str,
        icon: Icon,
        label: &str,
        enabled: bool,
        cell_w: f32,
        hud: ControlCenterColors,
    ) -> bool {
        let original = f.theme();
        let fg = if enabled {
            hud.text
        } else {
            // A disabled end is a reading, not an invitation: muted, and with
            // no hover plate to suggest it can be pressed.
            hud.text_muted.with_alpha(90)
        };
        f.set_theme(themes::hud(&hud).with_fg(fg));
        // A disabled pressable would still capture the pointer and swallow the
        // click, so the disabled end is an inert box with the same geometry —
        // the lane keeps its length and the cell keeps its hit target.
        let clicked = if enabled {
            f.pressable_row(
                id,
                label,
                &LayoutOpts {
                    width: cell_w,
                    height: TRAY_CELL,
                    cross: Align::Center,
                    radius: cell_w * 0.32,
                    bg: Color::TRANSPARENT,
                    ..Default::default()
                },
                |f, _| {
                    f.centered(cell_w, TRAY_CELL, |f| f.icon(icon, TRAY_ICON * 0.7));
                },
            )
            .0
            .clicked
        } else {
            f.push_id(id);
            f.row_ex(
                &LayoutOpts {
                    width: cell_w,
                    height: TRAY_CELL,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.centered(cell_w, TRAY_CELL, |f| f.icon(icon, TRAY_ICON * 0.7));
                },
            );
            f.pop_id();
            // No pressable, so nothing to press: the disabled end is drawn and
            // then refuses the pointer entirely.
            false
        };
        f.set_theme(original);
        clicked
    }

    /// Render the dbusmenu popover. The visible rows come from walking
    /// `menu.root.children` along `self.menu_path`. Submenu rows push onto
    /// `menu_path`, leaf rows send `MenuEvent` and dismiss the popover, and
    /// click-away closes it unless the press falls on the owner tray cell.
    pub(crate) fn render_tray_menu(
        &mut self,
        f: &mut Frame,
        menu: &MenuState,
        display: (f32, f32),
        cursor: (f32, f32),
        pressed: bool,
    ) {
        // If a targeted submenu id no longer exists (the worker truncated the
        // tree on `LayoutUpdated`), pop back to the nearest valid level.
        while tray::visible_children(&menu.root, &self.menu_path).is_none()
            && self.menu_path.len() > 1
        {
            self.menu_path.pop();
        }
        let visible = match tray::visible_children(&menu.root, &self.menu_path) {
            Some(rows) => rows,
            None => return,
        };

        let popover_bounds = menu_bounds(self.menu_owner, visible, display);
        let in_owner = contains(self.menu_owner, cursor.0, cursor.1);
        let in_popover = contains(popover_bounds, cursor.0, cursor.1);
        if !self.menu_just_opened && pressed && !in_owner && !in_popover {
            self.close_menu(menu.key.clone());
            return;
        }
        self.menu_just_opened = false;

        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original_theme = f.theme();
        let menu_theme = themes::hud(&hud);
        let dim_theme = themes::hud_muted(menu_theme, &hud);

        let header_visible = self.menu_path.len() > 1;
        let mut action: Option<MenuRowAction> = None;
        f.set_theme(menu_theme);
        f.place(
            "tessera-hud-sni-menu",
            &chrome_place(popover_bounds, materials::canvas_panel(&hud)),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: popover_bounds.w,
                        height: popover_bounds.h,
                        gap: 0.0,
                        pad: MENU_PAD,
                        ..Default::default()
                    },
                    |f| {
                        let inner_w = popover_bounds.w - MENU_PAD * 2.0;
                        if header_visible {
                            f.size_next(inner_w, MENU_HEADER_HEIGHT);
                            f.push_id("hud-menu-back");
                            if f.selectable("‹ Back", false) {
                                action = Some(MenuRowAction::Back);
                            }
                            f.pop_id();
                        }
                        for row in visible.iter() {
                            if !row.visible {
                                continue;
                            }
                            if row.kind == tray::MenuEntryKind::Separator {
                                f.spacer(MENU_SEP_GAP);
                                f.size_next(inner_w, 1.0);
                                f.separator();
                                f.spacer(MENU_SEP_GAP);
                                continue;
                            }
                            f.size_next(inner_w, MENU_ROW_HEIGHT);
                            f.push_id(&format!("hud-menu-row-{}", row.id));
                            if !row.enabled {
                                // Disabled rows render as inert labels with a
                                // dim foreground — selectable would still
                                // capture the click, which the dbusmenu spec
                                // forbids.
                                f.set_theme(dim_theme);
                                display_label(
                                    f,
                                    &truncate(&menu_row_label(row), 32),
                                    type_scale.body,
                                );
                                f.set_theme(menu_theme);
                            } else if f.selectable(&truncate(&menu_row_label(row), 32), false) {
                                if row.has_submenu {
                                    action = Some(MenuRowAction::Descend(row.id));
                                } else {
                                    action = Some(MenuRowAction::Click(row.id));
                                }
                            }
                            f.pop_id();
                        }
                    },
                );
            },
        );
        f.set_theme(original_theme);

        match action {
            Some(MenuRowAction::Back) => {
                self.menu_path.pop();
            }
            Some(MenuRowAction::Descend(id)) => {
                self.menu_path.push(id);
            }
            Some(MenuRowAction::Click(id)) => {
                self.send_tray_command(TrayCommand::MenuEvent {
                    key: menu.key.clone(),
                    id,
                });
                self.close_menu(menu.key.clone());
            }
            None => {}
        }
    }
}
