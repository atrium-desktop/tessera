//! Cluster chrome: the nav rail, tab icons, popover bounds, and the main panel.

use super::*;

impl ControlCenter {
    /// Bounds of the tray's dbusmenu popover, if one is open.
    ///
    /// Click-away has to see this surface: a press inside it must not be read
    /// as "outside every cluster surface" and dismiss the whole panel.
    pub(crate) fn open_popover_bounds(&mut self, display: (f32, f32)) -> Option<Rect> {
        let key = self.menu_open_for.clone()?;
        let menu = self.menu_snapshot().filter(|menu| menu.key == key)?;
        tray::visible_children(&menu.root, &self.menu_path)
            .map(|visible| menu_bounds(self.menu_owner, visible, display))
    }

    /// The navigation list inside the main card's recessed rail.
    /// Returns the dedicated icon for a navigation tab.
    pub(super) fn tab_icon_raw(tab: Tab) -> lens::sys::lens_icon_id {
        use lens::sys::lens_icon_id;
        match tab {
            Tab::QuickControls => lens_icon_id::LENS_ICON_SLIDERS,
            Tab::Settings(id) => match id.as_str() {
                "display" => lens_icon_id::LENS_ICON_MONITOR,
                "appearance" => lens_icon_id::LENS_ICON_IMAGE,
                "dock" => lens_icon_id::LENS_ICON_LAYOUT,
                "power" => lens_icon_id::LENS_ICON_BATTERY_CHARGING,
                "input" | "touchpad" | "mouse" | "keyboard" => {
                    lens_icon_id::LENS_ICON_MOUSE_POINTER
                }
                "keybindings" => lens_icon_id::LENS_ICON_EDIT,
                "users" | "persona" => lens_icon_id::LENS_ICON_USERS,
                "window-rules" => lens_icon_id::LENS_ICON_GRID,
                "network" | "wifi" => lens_icon_id::LENS_ICON_WIFI,
                "sound" | "audio" => lens_icon_id::LENS_ICON_VOLUME_2,
                "bluetooth" => lens_icon_id::LENS_ICON_BLUETOOTH,
                _ => lens_icon_id::LENS_ICON_SETTINGS,
            },
        }
    }

    pub(crate) fn render_nav_rail(&mut self, f: &mut Frame, rect: Rect, i18n: &Localizer) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let mut tabs: Vec<(Tab, &'static str)> =
            vec![(Tab::QuickControls, i18n.text(Message::QuickControls))];
        tabs.extend(
            self.modules
                .metadata()
                .filter(|module| module.availability == ModuleAvailability::Available)
                .map(|module| (Tab::Settings(module.id), i18n.text(module.title))),
        );

        let mut action: Option<TabAction> = None;
        let original = f.theme();

        const ROW_H: f32 = 42.0;
        const ROW_GAP: f32 = 4.0;
        const RAIL_PAD: f32 = 8.0;

        let tab_theme = themes::hud(&hud);

        f.place(
            "tessera-hud-nav-rail",
            &chrome_place(rect, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: (rect.w - RAIL_PAD * 2.0).max(1.0),
                        height: (rect.h - RAIL_PAD * 2.0).max(1.0),
                        gap: ROW_GAP,
                        pad: RAIL_PAD,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        for (index, (tab, label)) in tabs.iter().enumerate() {
                            let selected = self.tab == *tab;

                            let (bg, border, text_color, icon_color) = if selected {
                                (
                                    hud.selection_surface,
                                    hud.accent.with_alpha(70),
                                    hud.text,
                                    hud.accent,
                                )
                            } else {
                                (
                                    Color::TRANSPARENT,
                                    Color::TRANSPARENT,
                                    hud.text,
                                    hud.text_muted,
                                )
                            };

                            let icon = Self::tab_icon_raw(*tab);
                            let label_text =
                                truncate(label, ((rect.w - 54.0) / 7.0).max(3.0) as usize);

                            f.set_theme(tab_theme.with_fg(text_color));
                            let (response, _) = f.pressable_row(
                                &format!("tessera-hud-tab-{index}"),
                                &label_text,
                                &LayoutOpts {
                                    height: ROW_H,
                                    pad: 10.0,
                                    radius: 10.0,
                                    cross: Align::Center,
                                    gap: 10.0,
                                    bg,
                                    border,
                                    border_width: if selected { 1.0 } else { 0.0 },
                                    ..Default::default()
                                },
                                |f, _| {
                                    f.set_theme(tab_theme.with_fg(icon_color));
                                    f.icon_raw(icon, 18.0);
                                    f.set_theme(tab_theme.with_fg(text_color));
                                    display_label(f, &label_text, type_scale.body);
                                },
                            );
                            if response.clicked && !selected {
                                action = Some(TabAction::Select(*tab));
                            }
                        }
                    },
                );
            },
        );
        f.set_theme(original);

        match action {
            Some(TabAction::Select(tab)) => self.select_tab(tab),
            None => {}
        }
    }

    /// A settings module tab's body: the module's page inside a scroll
    /// area, painted with the theme matching the stored design snapshot.
    /// Emitted `SettingsAction`s are forwarded to the shell tagged with the
    /// current snapshot revision, coalesced to the newest draft per action
    /// kind (instant modules emit per change while a control drags). Until
    /// the first settings snapshot arrives the tab shows a muted
    /// placeholder instead.
    pub(crate) fn render_settings_tab(
        &mut self,
        f: &mut Frame,
        id: ModuleId,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        if self.settings.is_none() {
            let original = f.theme();
            let muted = themes::hud_muted(themes::hud(&hud), &hud);
            f.set_theme(muted);
            f.place(
                "tessera-hud-settings-empty",
                &chrome_place(area, transparent()),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: area.w,
                            height: area.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.flex(1.0);
                            f.spacer(0.0);
                            display_label(
                                f,
                                i18n.text(Message::ConnectingToDesktop),
                                type_scale.body,
                            );
                            f.flex(1.0);
                            f.spacer(0.0);
                        },
                    );
                },
            );
            f.set_theme(original);
            return;
        }
        let design = self.design;
        let mut events = ModuleEvents::default();
        let original = f.theme();
        f.set_theme(themes::application(&design));
        f.place(
            "tessera-hud-settings",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(&sized(area.w, area.h), |f| {
                    f.flex(1.0);
                    f.scroll("tessera-hud-settings-scroll", |f| {
                        f.column_ex(
                            &LayoutOpts {
                                gap: 12.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                self.modules.render(id, f, i18n, &design, &mut events);
                            },
                        );
                    });
                });
            },
        );
        f.set_theme(original);
        let revision = self.settings.as_ref().map(|settings| settings.revision);
        for action in events.actions {
            out.settings_actions
                .retain(|(_, queued)| !same_action_kind(queued, &action));
            out.settings_actions.push((revision, action));
        }
    }

    /// The central command surface. Navigation and page content share one
    /// coherent card so the middle of the screen reads as a single object,
    /// not a loose rail beside an unrelated panel.
    pub(crate) fn render_main_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };

        const SHELL_PAD: f32 = 12.0;
        const INNER_GAP: f32 = 12.0;
        let nav_w: f32 = if rect.w < 620.0 { 140.0 } else { 210.0 };
        let nav_w = nav_w.min((rect.w * 0.36).max(100.0));
        let inner_h = (rect.h - SHELL_PAD * 2.0).max(1.0);
        let view_w = (rect.w - SHELL_PAD * 2.0 - nav_w - INNER_GAP).max(1.0);

        // One strong silhouette owns the centre. The recessed navigation
        // well supplies hierarchy without creating a second floating card.
        f.place(
            "tessera-hud-main-surface",
            &chrome_place(
                rect,
                LayoutOpts {
                    radius: self.design.radii.panel_lg,
                    border: hud.border,
                    border_width: 1.0,
                    ..materials::canvas_panel(&hud)
                },
            ),
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        let nav_rect = Rect {
            x: rect.x + SHELL_PAD,
            y: rect.y + SHELL_PAD,
            w: nav_w,
            h: inner_h,
        };
        let view_rect = Rect {
            x: nav_rect.x + nav_w + INNER_GAP,
            y: rect.y + SHELL_PAD,
            w: view_w,
            h: inner_h,
        };

        f.place(
            "tessera-hud-nav-surface",
            &chrome_place(
                nav_rect,
                LayoutOpts {
                    radius: (self.design.radii.panel_lg - 4.0).max(8.0),
                    bg: hud.surface_recessed,
                    ..Default::default()
                },
            ),
            |f| {
                f.column_ex(&sized(nav_rect.w, nav_rect.h), |_| {});
            },
        );

        self.render_nav_rail(f, nav_rect, i18n);

        let pad_h = 14.0;
        let pad_v = 10.0;
        let header_h = 42.0;
        let active_title = match self.tab {
            Tab::QuickControls => i18n.text(Message::QuickControls),
            Tab::Settings(id) => self
                .modules
                .metadata()
                .find(|m| m.id == id)
                .map(|m| i18n.text(m.title))
                .unwrap_or("Settings"),
        };

        // Header inside Right View
        let original = f.theme();
        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-view-header",
            &chrome_place(
                Rect {
                    x: view_rect.x + pad_h,
                    y: view_rect.y + pad_v,
                    w: (view_rect.w - pad_h * 2.0).max(1.0),
                    h: header_h,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: (view_rect.w - pad_h * 2.0).max(1.0),
                        height: header_h,
                        cross: Align::Center,
                        gap: 12.0,
                        ..Default::default()
                    },
                    |f| {
                        f.icon_raw(Self::tab_icon_raw(self.tab), 20.0);
                        display_label(f, active_title, type_scale.title);
                    },
                );
            },
        );
        f.set_theme(original);

        let body_area = Rect {
            x: view_rect.x + pad_h,
            y: view_rect.y + pad_v + header_h + 8.0,
            w: (view_rect.w - pad_h * 2.0).max(1.0),
            h: (view_rect.h - pad_v * 2.0 - header_h - 8.0).max(1.0),
        };
        match self.tab {
            Tab::QuickControls => self.render_quick_controls_section(f, body_area, i18n, out),
            Tab::Settings(id) => self.render_settings_tab(f, id, body_area, i18n, out),
        }
    }
}
