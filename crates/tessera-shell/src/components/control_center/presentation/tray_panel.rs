//! The SNI tray shelf and its dbusmenu popover (ADR-0197).
//!
//! The tray is a **shelf**, not a scrolling column. Its height is derived from
//! the display and the shell's reserved bands ([INV-CHROME-02]), it has no
//! scroll viewport ([INV-CHROME-03]), and overflow is *disclosed* by a trailing
//! `+N` cell that expands into a grid popover rather than hidden behind a
//! scroll or a page turn ([INV-CHROME-04]).
//!
//! Geometry lives in [`super::tray_shelf`] as pure functions; this module only
//! paints and dispatches.

use super::*;

/// Which cell of the shelf a draw is producing.
enum ShelfCell<'a> {
    /// A resident SNI item.
    Item(&'a TrayCellVisual),
    /// The trailing disclosure cell showing how many items are hidden.
    Disclosure { hidden: usize },
}

impl ControlCenter {
    /// Render the tray shelf at the bottom-left anchor.
    ///
    /// StatusNotifierItem icons stack **upward** from the anchor's bottom edge,
    /// so a newly-registered item grows the shelf without displacing the icons
    /// already nearest the pointer's approach path. Left-click activates an
    /// item; right-click opens the host-rendered dbusmenu popover (or falls
    /// back to `SecondaryActivate` when the item exposes no `Menu` object).
    /// Hover and menu-open are the cell pressable's own background, never a
    /// sibling `place()` overlay ([INV-CHROME-06]).
    pub(crate) fn render_tray_shelf(
        &mut self,
        f: &mut Frame,
        anchor: Rect,
        display: (f32, f32),
        progress: f32,
        cursor: (f32, f32),
        pressed: bool,
        _i18n: &Localizer,
    ) {
        let cells = self.sni_cells();
        self.render_tray_shelf_cells(f, anchor, display, &cells, progress, cursor, pressed);
    }

    /// Paint an explicit cell list into the shelf.
    ///
    /// Split from [`Self::render_tray_shelf`] so the shelf's behaviour is
    /// drivable with a known item set: overflow disclosure is a function of
    /// `(anchor, item count)`, and a caller that already holds the cells should
    /// not have to re-derive them through the projection to exercise it.
    pub(crate) fn render_tray_shelf_cells(
        &mut self,
        f: &mut Frame,
        anchor: Rect,
        display: (f32, f32),
        cells: &[SniCell],
        progress: f32,
        cursor: (f32, f32),
        pressed: bool,
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
            // Nothing to show, so nothing is disclosed either.
            self.tray_shelf = None;
            self.tray_overflow_open = false;
            return;
        }

        // Resolve the shelf geometry before any painting, so the overflow
        // decision is a pure function of the anchor and the item count
        // ([INV-CHROME-05]).
        let layout = tray_shelf::shelf_layout(anchor, cells.len());
        // Cache the resolved layout: click-away hit-testing and the disclosure
        // popover read this instead of re-deriving the geometry, so the drawn
        // shelf and the interactive shelf cannot disagree.
        self.tray_shelf = Some(layout);
        if layout.visible == 0 {
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

        // The disclosure cell owns the last slot; items fill the rest. When
        // nothing overflows every drawn cell is an item.
        let own_items = layout.visible - usize::from(layout.has_disclosure());
        let shelf_cells: Vec<ShelfCell<'_>> = visuals
            .iter()
            .take(own_items)
            .map(ShelfCell::Item)
            .chain(layout.has_disclosure().then_some(ShelfCell::Disclosure {
                hidden: layout.overflow,
            }))
            .collect();

        let cell_w = layout.cell_width();
        let surface = layout.surface;
        let column_x = surface.x + (surface.w - cell_w) * 0.5;
        let plate_radius = cell_w * 0.32;

        let original = f.theme();

        // Interactions are captured inside the layout closures and dispatched
        // after them: opening a popover mutates `self`.
        let mut activations: Vec<String> = Vec::new();
        let mut secondary: Vec<(String, bool)> = Vec::new();
        let mut resolved: Vec<(String, Rect)> = Vec::new();
        let mut disclosure_clicked = false;
        let mut disclosure_rect = Rect {
            x: column_x,
            y: surface.y + TRAY_PAD,
            w: cell_w,
            h: TRAY_CELL,
        };

        // The capsule: one recessed surface sized to the *occupied* slots, so a
        // two-icon tray is a small pill rather than a tall empty column.
        f.place(
            "tessera-hud-tray-surface",
            &chrome_place(
                surface,
                LayoutOpts {
                    radius: surface.w * 0.5,
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
                f.column_ex(
                    &LayoutOpts {
                        width: surface.w,
                        height: surface.h,
                        pad: TRAY_PAD,
                        gap: TRAY_GAP,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        for cell in shelf_cells.iter() {
                            match cell {
                                ShelfCell::Item(cell) => {
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
                                    resolved.push((key.clone(), response.rect));
                                    if response.clicked {
                                        activations.push(key);
                                    } else if response.right_clicked {
                                        secondary.push((key, has_menu));
                                    }
                                }
                                ShelfCell::Disclosure { hidden } => {
                                    // The disclosure is a cell like any other:
                                    // same slot geometry, same hover treatment,
                                    // same pressable semantics. It is never a
                                    // navigation control ([INV-CHROME-04]).
                                    let label = format!("+{hidden}");
                                    let (response, _) = f.pressable_row(
                                        "tessera-hud-tray-overflow",
                                        &label,
                                        &LayoutOpts {
                                            width: cell_w,
                                            height: TRAY_CELL,
                                            cross: Align::Center,
                                            radius: plate_radius,
                                            bg: Color::TRANSPARENT,
                                            ..Default::default()
                                        },
                                        |f, _| {
                                            f.centered(cell_w, TRAY_CELL, |f| {
                                                display_label(
                                                    f,
                                                    &label,
                                                    self.design.typography.footnote,
                                                );
                                            });
                                        },
                                    );
                                    disclosure_rect = response.rect;
                                    disclosure_clicked = response.clicked;
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
                    .unwrap_or(Rect {
                        x: column_x,
                        y: surface.y + TRAY_PAD,
                        w: cell_w,
                        h: TRAY_CELL,
                    });
                self.menu_owner = Rect {
                    x: surface.x,
                    y: cell_rect.y,
                    w: surface.w,
                    h: cell_rect.h,
                };
                self.menu_open_for = Some(key.clone());
                self.menu_path.clear();
                self.menu_just_opened = true;
                self.send_tray_command(TrayCommand::FetchMenu { key });
            } else {
                self.send_tray_command(TrayCommand::SecondaryActivate { key, x, y });
            }
        }

        // A disclosed tray is an open popover on the same footing as a menu:
        // both float above the shelf and both re-anchor every frame.
        if disclosure_clicked {
            self.tray_overflow_open = !self.tray_overflow_open;
        }
        self.tray_overflow_anchor = disclosure_rect;
        if !layout.has_disclosure() {
            // The last disclosed item fit: the popover has nothing left to
            // show and must not linger as an empty surface.
            self.tray_overflow_open = false;
        }
        if self.tray_overflow_open {
            self.render_tray_overflow(f, &visuals[own_items..], display, cursor, pressed);
        }

        // Keep the owner rect fresh against relayout or item movement.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some((_, rect)) = resolved.iter().find(|(k, _)| k == &key) {
                self.menu_owner = Rect {
                    x: surface.x,
                    y: rect.y,
                    w: surface.w,
                    h: rect.h,
                };
            } else {
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// The overflow popover: every item the shelf had to disclose, as a grid.
    ///
    /// This is disclosure, not navigation: it holds the *complete* hidden set,
    /// so reaching an item is one click from the shelf rather than a paged
    /// walk ([INV-CHROME-04]). Cells carry the same activate / context-menu
    /// affordances as shelf cells so the overflow set is not second-class.
    fn render_tray_overflow(
        &mut self,
        f: &mut Frame,
        items: &[TrayCellVisual],
        display: (f32, f32),
        cursor: (f32, f32),
        pressed: bool,
    ) {
        if items.is_empty() {
            return;
        }
        const COLUMNS: u32 = 4;
        const PAD: f32 = 8.0;
        const GAP: f32 = 6.0;
        let hud = self.panel_colors();

        let cols = COLUMNS.min(items.len() as u32).max(1);
        // One shared pure function owns the popover's extent, so this render
        // pass and click-away hit-testing cannot drift apart.
        let bounds = tray_shelf::overflow_bounds(self.tray_overflow_anchor, items.len(), display);

        // Light-dismiss, symmetric with the dbusmenu popover: a press outside
        // both the popover and the disclosure cell that opened it closes the
        // popover rather than leaving a stale surface on screen. The panel's
        // own click-away runs first and already treats the popover as inside,
        // so this cannot double-handle the same press.
        let in_anchor = contains(self.tray_overflow_anchor, cursor.0, cursor.1);
        let in_popover = contains(bounds, cursor.0, cursor.1);
        if pressed && !in_anchor && !in_popover {
            self.tray_overflow_open = false;
            return;
        }

        let original = f.theme();
        let base = themes::hud(&hud);
        let mut activations: Vec<String> = Vec::new();
        let mut secondary: Vec<(String, bool)> = Vec::new();
        let mut cell_rects: Vec<(String, Rect)> = Vec::new();

        f.set_theme(base);
        f.place(
            "tessera-hud-tray-overflow-popover",
            &chrome_place(bounds, materials::canvas_panel(&hud)),
            |f| {
                f.grid(cols)
                    .col_gap(GAP)
                    .row_gap(GAP)
                    .pad(PAD)
                    .row_height(TRAY_CELL)
                    .show(|f| {
                        for item in items.iter() {
                            let texture = item.texture;
                            let fallback = item.fallback;
                            let title = item.title.clone();
                            let key = item.key.clone();
                            let has_menu = item.has_menu;
                            let (response, _) = f.pressable_row(
                                &format!("tessera-hud-tray-overflow-{key}"),
                                &title,
                                &LayoutOpts {
                                    width: TRAY_CELL,
                                    height: TRAY_CELL,
                                    cross: Align::Center,
                                    radius: TRAY_CELL * 0.32,
                                    bg: Color::TRANSPARENT,
                                    ..Default::default()
                                },
                                |f, _| {
                                    f.centered(TRAY_CELL, TRAY_CELL, |f| match texture {
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
                            cell_rects.push((key.clone(), response.rect));
                            if response.clicked {
                                activations.push(key);
                            } else if response.right_clicked {
                                secondary.push((key, has_menu));
                            }
                        }
                    });
            },
        );
        f.set_theme(original);

        let (x, y) = (cursor.0 as i32, cursor.1 as i32);
        for key in activations {
            self.send_tray_command(TrayCommand::Activate { key, x, y });
            // An activated item has done its job; leaving the popover open
            // would occlude the shelf it belongs to.
            self.tray_overflow_open = false;
        }
        for (key, has_menu) in secondary {
            if has_menu {
                let cell_rect = cell_rects
                    .iter()
                    .find(|(k, _)| k == &key)
                    .map(|(_, rect)| *rect)
                    .unwrap_or(self.tray_overflow_anchor);
                self.menu_owner = Rect {
                    x: bounds.x,
                    y: cell_rect.y,
                    w: bounds.w,
                    h: cell_rect.h,
                };
                self.menu_open_for = Some(key.clone());
                self.menu_path.clear();
                self.menu_just_opened = true;
                self.tray_overflow_open = false;
                self.send_tray_command(TrayCommand::FetchMenu { key });
            } else {
                self.send_tray_command(TrayCommand::SecondaryActivate { key, x, y });
            }
        }
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
