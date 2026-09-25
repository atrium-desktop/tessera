use super::*;

use lens::Icon;
use tessera_design::materials::{chrome_place, sized, transparent};

// ---- rendering -----------------------------------------------------------

impl ControlCenter {
    /// Bounds of the currently open dbusmenu popover, if any.
    pub(super) fn open_popover_bounds(&mut self, display: (f32, f32)) -> Option<Rect> {
        let key = self.menu_open_for.clone()?;
        let menu = self.menu_snapshot().filter(|menu| menu.key == key)?;
        tray::visible_children(&menu.root, &self.menu_path)
            .map(|visible| menu_bounds(self.menu_owner, visible, display))
    }

    /// The top-left profile block: user persona (gapped-ring avatar, display
    /// name, `@username · groups`, hostname) drawn frameless — no chip
    /// background or border — straight onto the solid canvas.
    /// Slides in from the top-left.
    pub(super) fn render_profile_panel(
        &self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * -24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let pad = 14.0;
        let center_y = rect.y + rect.h * 0.5;
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let avatar_style = self.design.avatars.for_role(AvatarRole::PersonaHeader);
        let original = f.theme();

        // -- profile zone: 56px avatar with a gapped line ring + name lines --
        let avatar_size = 56.0;
        let avatar_center = (rect.x + pad + avatar_size * 0.5, center_y);
        // The ring floats clear of the avatar edge — the visible gap reads
        // as a deliberate stroke of the identity mark rather than a border.
        let ring_gap = 3.0;
        let ring_diameter = avatar_size + ring_gap * 2.0 + 2.0;
        render_ring(
            f,
            "tessera-hud-avatar-ring",
            avatar_center,
            ring_diameter,
            hud.accent,
            1.5,
        );
        render_disc(
            f,
            "tessera-hud-avatar-backdrop",
            avatar_center,
            avatar_size,
            avatar_style.fallback_surface,
        );
        let avatar_rect = Rect {
            x: avatar_center.0 - avatar_size * 0.5,
            y: avatar_center.1 - avatar_size * 0.5,
            w: avatar_size,
            h: avatar_size,
        };
        match &self.avatar {
            Some(avatar) => {
                let texture = avatar.texture().as_raw();
                f.place(
                    "tessera-hud-avatar",
                    &chrome_place(avatar_rect, transparent()),
                    |f| {
                        f.row_ex(&sized(avatar_size, avatar_size), |f| {
                            unsafe { f.image(texture, avatar_size, avatar_size) };
                        });
                    },
                );
            }
            None => {
                f.set_theme(base_theme.with_fg(avatar_style.fallback_foreground));
                f.place(
                    "tessera-hud-avatar-initials",
                    &chrome_place(avatar_rect, transparent()),
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                width: avatar_size,
                                height: avatar_size,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.flex(1.0);
                                f.spacer(0.0);
                                display_label(
                                    f,
                                    &self.profile.initials,
                                    avatar_rect.w * avatar_style.initials_scale,
                                );
                                f.flex(1.0);
                                f.spacer(0.0);
                            },
                        );
                    },
                );
            }
        }

        let text_x = rect.x + pad + ring_diameter + 14.0;
        let text_w = (rect.x + rect.w - pad - text_x).max(40.0);
        let display_name = truncate(&self.profile.display_name, (text_w / 9.5).max(4.0) as usize);
        f.set_theme(base_theme);
        f.place(
            "tessera-hud-profile-name",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: center_y - 26.0,
                    w: text_w,
                    h: 24.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 24.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| display_label(f, &display_name, type_scale.title),
                );
            },
        );
        let mut sub_line = format!("@{}", self.profile.username);
        if !self.profile.groups.is_empty() {
            sub_line.push_str(" · ");
            sub_line.push_str(&self.profile.groups.join(", "));
        }
        let sub_line = truncate(&sub_line, (text_w / 6.2).max(6.0) as usize);
        f.set_theme(muted_theme);
        f.place(
            "tessera-hud-profile-sub",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: center_y + 1.0,
                    w: text_w,
                    h: 18.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 18.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| display_label(f, &sub_line, type_scale.label),
                );
            },
        );
        // The hostname line answers "which machine": muted, sitting under
        // the account line so the block reads who-on-where.
        if !self.profile.hostname.is_empty() {
            let host_line = truncate(&self.profile.hostname, (text_w / 6.2).max(6.0) as usize);
            f.place(
                "tessera-hud-profile-host",
                &chrome_place(
                    Rect {
                        x: text_x,
                        y: center_y + 20.0,
                        w: text_w,
                        h: 16.0,
                    },
                    transparent(),
                ),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: text_w,
                            height: 16.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| display_label(f, &host_line, type_scale.footnote),
                    );
                },
            );
        }
        f.set_theme(original);
    }

    /// The central command surface. Navigation and page content share one
    /// coherent card so the middle of the screen reads as a single object,
    /// not a loose rail beside an unrelated panel.
    pub(super) fn render_main_panel(
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
                    radius: self.design.radii.glass_panel,
                    border: hud.border,
                    border_width: 1.0,
                    ..materials::hud_panel(&hud)
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
                    radius: (self.design.radii.glass_panel - 4.0).max(8.0),
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
                "input" | "touchpad" | "mouse" | "keyboard" => lens_icon_id::LENS_ICON_MOUSE_POINTER,
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

    pub(super) fn render_nav_rail(&mut self, f: &mut Frame, rect: Rect, i18n: &Localizer) {
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
    pub(super) fn render_settings_tab(
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

    /// The top-right notifications stream: each notification as its own
    /// recessed card — a distinct background, hairline border, and 10px
    /// gaps so individuals read clearly — newest first, with the tail
    /// fading out toward the bottom of the region. The scrollbar stays
    /// hidden until the user wheels over the stream and fades back out
    /// when idle. Slides in from the top-right.
    pub(super) fn render_notifications_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let notifications = self.notification_snapshot();

        // Small muted section header, still frameless.
        f.set_theme(muted_theme);
        f.place(
            "tessera-hud-notifications-header",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: 18.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: 18.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        display_label(f, i18n.text(Message::Notifications), type_scale.label);
                    },
                );
            },
        );

        let body = Rect {
            x: rect.x,
            y: rect.y + 24.0,
            w: rect.w,
            h: (rect.h - 24.0).max(1.0),
        };

        if notifications.is_empty() {
            f.place(
                "tessera-hud-messages-empty",
                &chrome_place(body, transparent()),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: body.w,
                            height: body.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            display_label(f, i18n.text(Message::NoNotifications), type_scale.body);
                        },
                    );
                },
            );
            f.set_theme(original);
            return;
        }

        // Scrollbar only while wheel activity keeps it revealed.
        let scroll_id = "tessera-hud-messages-scroll";
        let scrollbar_theme = if self.notif_scrollbar_reveal > 0.01 {
            base_theme
                .with_scrollbar_width(4.0)
                .with_scrollbar_radius(2.0)
                .with_scrollbar_track_color(hud.background.with_alpha(0))
                .with_scrollbar_thumb_color(hud.surface.with_alpha(150))
                .with_scrollbar_thumb_hover_color(hud.surface.with_alpha(220))
        } else {
            base_theme.with_scrollbar_width(0.0)
        };
        f.set_theme(scrollbar_theme);

        f.place(
            "tessera-hud-messages",
            &chrome_place(body, transparent()),
            |f| {
                f.column_ex(&sized(body.w, body.h), |f| {
                    f.flex(1.0);
                    f.scroll(scroll_id, |f| {
                        f.column_ex(
                            &LayoutOpts {
                                width: body.w,
                                gap: 10.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                for notification in notifications.iter() {
                                    let id = notification.id;
                                    let (response, _) = f.pressable_row(
                                        &format!("tessera-hud-message-{id}"),
                                        "",
                                        &LayoutOpts {
                                            width: body.w,
                                            cross: Align::Center,
                                            gap: 10.0,
                                            pad: 10.0,
                                            radius: 10.0,
                                            bg: hud.surface_recessed,
                                            border: hud.border.with_alpha(60),
                                            border_width: 1.0,
                                            ..Default::default()
                                        },
                                        |f, _| {
                                            // One item per card: a distinct
                                            // recessed background, hairline
                                            // border, and 10px of breathing
                                            // room from its neighbours.
                                            f.icon(Icon::Bell, 14.0);
                                            f.column_ex(
                                                &LayoutOpts {
                                                    flex: 1.0,
                                                    gap: 2.0,
                                                    ..Default::default()
                                                },
                                                |f| {
                                                    f.set_theme(base_theme);
                                                    display_label(
                                                        f,
                                                        &notification.summary,
                                                        type_scale.body,
                                                    );
                                                    f.set_theme(muted_theme);
                                                    display_label(
                                                        f,
                                                        &notification.body,
                                                        type_scale.footnote,
                                                    );
                                                },
                                            );
                                        },
                                    );
                                    if response.clicked {
                                        out.dismissed_notification = Some(id);
                                    }
                                }
                            },
                        );
                    });
                });
            },
        );

        // Tail fade: a canvas-colored gradient plate masks the last stretch
        // of the stream so items dissolve toward the region's bottom edge.
        let fade_h = (body.h * 0.22).clamp(24.0, 72.0).min(body.h);
        f.place(
            "tessera-hud-messages-fade",
            &chrome_place(
                Rect {
                    x: body.x,
                    y: body.y + body.h - fade_h,
                    w: body.w,
                    h: fade_h,
                },
                transparent(),
            ),
            |f| {
                let steps = 10;
                for step in 0..steps {
                    let t = step as f32 / steps as f32;
                    let alpha = (t * t * 255.0) as u8;
                    f.place(
                        &format!("tessera-hud-messages-fade-{step}"),
                        &chrome_place(
                            Rect {
                                x: body.x,
                                y: body.y + body.h - fade_h + fade_h * (step as f32 / steps as f32),
                                w: body.w,
                                h: fade_h / steps as f32 + 0.5,
                            },
                            LayoutOpts {
                                bg: hud.background.with_alpha(alpha),
                                ..Default::default()
                            },
                        ),
                        |f| {
                            f.row_ex(
                                &LayoutOpts {
                                    width: body.w,
                                    height: fade_h / steps as f32 + 0.5,
                                    ..Default::default()
                                },
                                |_| {},
                            );
                        },
                    );
                }
            },
        );
        f.set_theme(original);
    }

    /// The frameless clock surface at top-center: large locale time with
    /// the weekday and date beneath. Redrawn when the wall clock's minute
    /// advances; between minutes the cached strings render unchanged.
    pub(super) fn render_clock_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        if rect.w < 80.0 || rect.h < 40.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let fall = (1.0 - ease_out_cubic(progress)) * 16.0;
        let rect = Rect {
            y: rect.y + fall,
            ..rect
        };
        let (time_text, date_text) = clock_strings();

        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);

        f.set_theme(base_theme);
        let time_h = (rect.h * 0.62).max(24.0);
        f.place(
            "tessera-hud-clock-time",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: time_h,
                },
                transparent(),
            ),
            |f| {
                f.centered(rect.w, time_h, |f| {
                    display_label(f, &time_text, type_scale.hero);
                });
            },
        );
        f.set_theme(muted_theme);
        let date_h = (rect.h - time_h).max(16.0);
        f.place(
            "tessera-hud-clock-date",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y + time_h,
                    w: rect.w,
                    h: date_h,
                },
                transparent(),
            ),
            |f| {
                f.centered(rect.w, date_h, |f| {
                    display_label(f, &date_text, type_scale.headline);
                });
            },
        );
        f.set_theme(original);
    }

    /// The first tab: a compact Control Center grid. Four direct-action
    /// tiles occupy a 2×2 cluster while sound and brightness become thick
    /// vertical faders. This keeps glanceable state and manipulation close
    /// together without falling back to a settings-form row stack.
    pub(super) fn render_quick_controls_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if self.wifi_expanded {
            self.render_wifi_detail_view(f, area, i18n, out);
            return;
        }

        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();
        let gap = 12.0;
        let tile_gap = 10.0;
        let tile_w = ((area.w - tile_gap) * 0.5).max(1.0);
        let tile_h = 58.0;
        let fader_h = 68.0;

        let wifi_active = status.wifi_enabled.unwrap_or(false);
        let wifi_sub = status.wifi_ssid.as_deref().unwrap_or(if wifi_active {
            "On"
        } else {
            "Off"
        });

        let bt_active = status.bluetooth_enabled.unwrap_or(false);
        let bt_sub = if bt_active { "On" } else { "Off" };

        let dnd_active = status.do_not_disturb;
        let dnd_sub = if dnd_active { "On" } else { "Off" };

        let dark_active = !self.design.is_light();
        let dark_sub = if dark_active { "Dark" } else { "Light" };

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-quick",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        gap,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        // Row 1: Wi-Fi + Bluetooth
                        f.row_ex(
                            &LayoutOpts {
                                width: area.w,
                                height: tile_h,
                                gap: tile_gap,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                let (wifi_toggle, wifi_expand) = render_expandable_quick_toggle_tile(
                                    f,
                                    "tessera-hud-quick-wifi",
                                    i18n.text(Message::Wifi),
                                    wifi_sub,
                                    lens::sys::lens_icon_id::LENS_ICON_WIFI,
                                    wifi_active,
                                    (tile_w, tile_h),
                                    hud,
                                    type_scale,
                                );
                                if wifi_toggle {
                                    out.system_actions.push(SystemAction::SetWifi {
                                        enabled: !wifi_active,
                                    });
                                }
                                if wifi_expand {
                                    self.wifi_expanded = true;
                                    out.system_actions.push(SystemAction::ScanWifi);
                                }
                                if render_quick_toggle_tile(
                                    f,
                                    "tessera-hud-quick-bluetooth",
                                    i18n.text(Message::Bluetooth),
                                    bt_sub,
                                    lens::sys::lens_icon_id::LENS_ICON_BLUETOOTH,
                                    bt_active,
                                    (tile_w, tile_h),
                                    hud,
                                    type_scale,
                                ) {
                                    out.system_actions.push(SystemAction::SetBluetooth {
                                        enabled: !bt_active,
                                    });
                                }
                            },
                        );

                        // Row 2: Do Not Disturb + Dark Mode
                        f.row_ex(
                            &LayoutOpts {
                                width: area.w,
                                height: tile_h,
                                gap: tile_gap,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                if render_quick_toggle_tile(
                                    f,
                                    "tessera-hud-quick-dnd",
                                    i18n.text(Message::DoNotDisturb),
                                    dnd_sub,
                                    lens::sys::lens_icon_id::LENS_ICON_BELL,
                                    dnd_active,
                                    (tile_w, tile_h),
                                    hud,
                                    type_scale,
                                ) {
                                    out.system_actions.push(SystemAction::SetDoNotDisturb {
                                        enabled: !dnd_active,
                                    });
                                }
                                if render_quick_toggle_tile(
                                    f,
                                    "tessera-hud-quick-dark-mode",
                                    "Dark Mode",
                                    dark_sub,
                                    if dark_active {
                                        lens::sys::lens_icon_id::LENS_ICON_MOON
                                    } else {
                                        lens::sys::lens_icon_id::LENS_ICON_SUN
                                    },
                                    dark_active,
                                    (tile_w, tile_h),
                                    hud,
                                    type_scale,
                                ) {
                                    let mut preferences = self
                                        .settings
                                        .as_ref()
                                        .map(|s| s.preferences.clone())
                                        .unwrap_or_default();
                                    preferences.color_scheme = if dark_active {
                                        tessera_desktop::settings::ColorScheme::Light
                                    } else {
                                        tessera_desktop::settings::ColorScheme::Dark
                                    };
                                    out.settings_actions.push((
                                        None,
                                        SettingsAction::SetDesktopPreferences { preferences },
                                    ));
                                }
                            },
                        );

                        // Horizontal Fader 1: Display (Brightness)
                        let (bright_level, _) = render_horizontal_fader(
                            f,
                            "tessera-hud-quick-brightness",
                            i18n.text(Message::Brightness),
                            lens::sys::lens_icon_id::LENS_ICON_SUN,
                            false,
                            status.brightness,
                            (1, 100),
                            (area.w, fader_h),
                            hud.text,
                            hud,
                            type_scale,
                        );
                        if let Some(level) = bright_level {
                            out.system_actions.push(SystemAction::SetBrightness { level });
                        }

                        // Horizontal Fader 2: Sound (Volume + Mute)
                        let sound_label = if status.muted {
                            i18n.text(Message::Muted)
                        } else {
                            i18n.text(Message::Sound)
                        };
                        let sound_fill = if status.muted {
                            hud.text_muted
                        } else {
                            hud.accent
                        };
                        let (vol_level, mute_clicked) = render_horizontal_fader(
                            f,
                            "tessera-hud-quick-volume",
                            sound_label,
                            volume_icon_raw(&status),
                            true,
                            status.volume,
                            (0, 100),
                            (area.w, fader_h),
                            sound_fill,
                            hud,
                            type_scale,
                        );
                        if mute_clicked {
                            out.system_actions.push(SystemAction::ToggleMute);
                        }
                        if let Some(level) = vol_level {
                            out.system_actions.push(SystemAction::SetVolume { level });
                        }
                    },
                );
            },
        );
        f.set_theme(original);
    }

    /// Expanded Wi-Fi networks detail view (ADR-0162).
    pub(super) fn render_wifi_detail_view(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();
        let wifi_active = status.wifi_enabled.unwrap_or(false);

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-wifi-detail",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        gap: 12.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        // Header row: [< Back] Title  [↻ Scan] [On/Off]
                        f.row_ex(
                            &LayoutOpts {
                                width: area.w,
                                height: 36.0,
                                cross: Align::Center,
                                gap: 8.0,
                                ..Default::default()
                            },
                            |f| {
                                let (back_resp, _) = f.pressable_row(
                                    "tessera-wifi-back-btn",
                                    "Back",
                                    &LayoutOpts {
                                        width: 80.0,
                                        height: 36.0,
                                        radius: 10.0,
                                        pad: 6.0,
                                        gap: 4.0,
                                        bg: hud.surface_recessed,
                                        cross: Align::Center,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_CHEVRON_LEFT, 16.0);
                                        display_label(f, "Back", type_scale.body);
                                    },
                                );
                                if back_resp.clicked {
                                    self.wifi_expanded = false;
                                    self.wifi_input_ssid = None;
                                }

                                f.spacer(4.0);
                                display_label(f, i18n.text(Message::Wifi), type_scale.headline);

                                f.flex(1.0);
                                f.spacer(0.0);

                                // Refresh / Scan button
                                let (refresh_resp, _) = f.pressable_row(
                                    "tessera-wifi-refresh-btn",
                                    "Scan",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 18.0,
                                        cross: Align::Center,
                                        bg: hud.surface_recessed,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                        f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_REFRESH_CW, 16.0);
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    },
                                );
                                if refresh_resp.clicked {
                                    out.system_actions.push(SystemAction::ScanWifi);
                                }

                                // Radio toggle
                                let (toggle_resp, _) = f.pressable_row(
                                    "tessera-wifi-power-btn",
                                    "Power",
                                    &LayoutOpts {
                                        width: 54.0,
                                        height: 32.0,
                                        radius: 16.0,
                                        cross: Align::Center,
                                        bg: if wifi_active { hud.accent } else { hud.surface_recessed },
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                        let text_color = if wifi_active {
                                            Color::rgba(255, 255, 255, 255)
                                        } else {
                                            hud.text_muted
                                        };
                                        f.set_theme(themes::hud(&hud).with_fg(text_color));
                                        display_label(f, if wifi_active { "On" } else { "Off" }, type_scale.caption);
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    },
                                );
                                if toggle_resp.clicked {
                                    out.system_actions.push(SystemAction::SetWifi {
                                        enabled: !wifi_active,
                                    });
                                }
                            },
                        );

                        // Body list
                        let list_h = (area.h - 48.0).max(1.0);
                        if !wifi_active {
                            f.column_ex(
                                &LayoutOpts {
                                    width: area.w,
                                    height: list_h,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    display_label(f, "Wi-Fi is turned off", type_scale.body);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                },
                            );
                        } else if status.wifi_networks.is_empty() {
                            f.column_ex(
                                &LayoutOpts {
                                    width: area.w,
                                    height: list_h,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    if status.wifi_state == tessera_desktop::system::WifiLinkState::Scanning {
                                        display_label(f, "Scanning for Wi-Fi networks...", type_scale.body);
                                    } else {
                                        display_label(f, "No networks found", type_scale.body);
                                    }
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                },
                            );
                        } else {
                            f.scroll("tessera-wifi-networks-scroll", |f| {
                                f.column_ex(
                                    &LayoutOpts {
                                        width: area.w,
                                        gap: 8.0,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |f| {
                                        for (idx, net) in status.wifi_networks.iter().enumerate() {
                                            let is_curr = net.is_connected
                                                || status.wifi_ssid.as_deref() == Some(&net.ssid);
                                            let is_connecting = status.wifi_state
                                                == tessera_desktop::system::WifiLinkState::Connecting
                                                && is_curr;

                                            let card_bg = if is_curr {
                                                hud.selection_surface
                                            } else {
                                                hud.surface_recessed
                                            };
                                            let card_border = if is_curr {
                                                hud.accent.with_alpha(80)
                                            } else {
                                                hud.border.with_alpha(40)
                                            };

                                            let row_id = format!("tessera-wifi-net-{idx}");
                                            let (row_resp, _) = f.pressable_row(
                                                &row_id,
                                                &net.ssid,
                                                &LayoutOpts {
                                                    width: area.w,
                                                    height: 48.0,
                                                    pad: 8.0,
                                                    radius: 12.0,
                                                    bg: card_bg,
                                                    border: card_border,
                                                    border_width: 1.0,
                                                    cross: Align::Center,
                                                    gap: 10.0,
                                                    ..Default::default()
                                                },
                                                |f, _| {
                                                    // Wifi icon
                                                    let icon_fg = if is_curr {
                                                        hud.accent
                                                    } else {
                                                        hud.text_muted
                                                    };
                                                    f.set_theme(themes::hud(&hud).with_fg(icon_fg));
                                                    f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_WIFI, 18.0);

                                                    // SSID
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                    display_label(f, &net.ssid, type_scale.body);

                                                    // Security lock if encrypted
                                                    if net.security != tessera_desktop::system::WifiSecurity::Open {
                                                        f.spacer(4.0);
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                        f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_LOCK, 14.0);
                                                    }

                                                    f.flex(1.0);
                                                    f.spacer(0.0);

                                                    // Status indicator / action on right
                                                    if is_curr {
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.accent));
                                                        f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_CHECK, 16.0);
                                                        f.spacer(4.0);
                                                        let (disconn_resp, _) = f.pressable_row(
                                                            &format!("tessera-wifi-disconn-{idx}"),
                                                            "Disconnect",
                                                            &LayoutOpts {
                                                                width: 76.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                cross: Align::Center,
                                                                bg: hud.surface,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                display_label(f, "Disconnect", type_scale.footnote);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if disconn_resp.clicked {
                                                            out.system_actions.push(SystemAction::DisconnectWifi);
                                                        }
                                                    } else if is_connecting {
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.accent));
                                                        display_label(f, "Connecting...", type_scale.footnote);
                                                    } else {
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                        let bars_text = match net.signal_bars {
                                                            4 => "••••",
                                                            3 => "•••",
                                                            2 => "••",
                                                            _ => "•",
                                                        };
                                                        display_label(f, bars_text, type_scale.footnote);
                                                    }
                                                },
                                            );

                                            if row_resp.clicked && !is_curr {
                                                if net.security == tessera_desktop::system::WifiSecurity::Open
                                                    || net.is_saved
                                                {
                                                    out.system_actions.push(SystemAction::ConnectWifi {
                                                        ssid: net.ssid.clone(),
                                                        passphrase: None,
                                                    });
                                                } else {
                                                    if self.wifi_input_ssid.as_deref() == Some(&net.ssid) {
                                                        self.wifi_input_ssid = None;
                                                    } else {
                                                        self.wifi_input_ssid = Some(net.ssid.clone());
                                                        self.wifi_input_passphrase.clear();
                                                    }
                                                }
                                            }

                                            // Inline passphrase entry if open for this network
                                            if self.wifi_input_ssid.as_deref() == Some(&net.ssid) {
                                                let masked: String = "•".repeat(self.wifi_input_passphrase.chars().count());
                                                let display_field = if masked.is_empty() {
                                                    "Type password...".to_string()
                                                } else {
                                                    format!("{masked}|")
                                                };
                                                f.row_ex(
                                                    &LayoutOpts {
                                                        width: area.w,
                                                        height: 38.0,
                                                        pad: 6.0,
                                                        radius: 10.0,
                                                        bg: hud.surface,
                                                        cross: Align::Center,
                                                        gap: 8.0,
                                                        ..Default::default()
                                                    },
                                                    |f| {
                                                        f.spacer(4.0);
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.accent));
                                                        f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_LOCK, 14.0);
                                                        f.set_theme(themes::hud(&hud).with_fg(if self.wifi_input_passphrase.is_empty() { hud.text_muted } else { hud.text }));
                                                        display_label(f, &display_field, type_scale.caption);

                                                        f.flex(1.0);
                                                        f.spacer(0.0);

                                                        let (conn_btn, _) = f.pressable_row(
                                                            &format!("tessera-wifi-conn-btn-{idx}"),
                                                            "Connect",
                                                            &LayoutOpts {
                                                                width: 68.0,
                                                                height: 26.0,
                                                                radius: 6.0,
                                                                bg: hud.accent,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(themes::hud(&hud).with_fg(Color::rgba(255, 255, 255, 255)));
                                                                display_label(f, "Connect", type_scale.caption);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if conn_btn.clicked {
                                                            let pass = if self.wifi_input_passphrase.is_empty() {
                                                                None
                                                            } else {
                                                                Some(self.wifi_input_passphrase.clone())
                                                            };
                                                            out.system_actions.push(SystemAction::ConnectWifi {
                                                                ssid: net.ssid.clone(),
                                                                passphrase: pass,
                                                            });
                                                            self.wifi_input_ssid = None;
                                                            self.wifi_input_passphrase.clear();
                                                        }

                                                        let (cancel_btn, _) = f.pressable_row(
                                                            &format!("tessera-wifi-cancel-btn-{idx}"),
                                                            "Cancel",
                                                            &LayoutOpts {
                                                                width: 58.0,
                                                                height: 26.0,
                                                                radius: 6.0,
                                                                bg: hud.surface_recessed,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                                display_label(f, "Cancel", type_scale.caption);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if cancel_btn.clicked {
                                                            self.wifi_input_ssid = None;
                                                            self.wifi_input_passphrase.clear();
                                                        }
                                                    },
                                                );
                                            }
                                        }
                                    },
                                );
                            });
                        }
                    },
                );
            },
        );
        f.set_theme(original);
    }

    /// The tray icon column at the left-middle anchor: StatusNotifierItem
    /// icons stacked vertically with no panel background, compact 22px
    /// glyphs. Left-click activates an item; right-click opens the
    /// host-rendered dbusmenu popover to the icon's right (or
    /// `SecondaryActivate` when the item has no Menu object); hover raises a
    /// rounded accent plate BEHIND the icon — drawn first and larger than
    /// the glyph, so the icon never disappears, only its backing becomes
    /// prominent. The column scrolls when icons overflow its height — the
    /// scrollbar fades in on wheel movement and decays away once idle.
    pub(super) fn render_tray_column(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        _i18n: &Localizer,
    ) {
        if rect.w < 32.0 || rect.h < 32.0 {
            return;
        }
        let hud = self.panel_colors();
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };

        let cells = self.sni_cells();
        if cells.is_empty() {
            return;
        }

        // Distill the per-cell visuals before the layout closures: those
        // capture disjoint borrows, so `self` method calls happen here.
        let fallback_themed = self.themed_icon("application-x-executable-symbolic");
        let cells: Vec<TrayCellVisual> = cells
            .iter()
            .map(|cell| TrayCellVisual {
                key: cell.key.clone(),
                title: truncate(&cell.title, 12),
                has_menu: cell.has_menu,
                texture: if cell.textured {
                    self.tray
                        .as_ref()
                        .and_then(|tray| tray.textures.get(&cell.key))
                        .map(|(_, image)| image.as_raw())
                } else {
                    None
                },
                fallback: fallback_themed.map(|icon| icon as *mut lens::sys::flux_image),
            })
            .collect();

        let cell_w = TRAY_CELL.min(rect.w - TRAY_PAD * 2.0).max(24.0);
        let panel_w = (cell_w + TRAY_PAD * 2.0).min(rect.w);
        let panel_x = rect.x + (rect.w - panel_w) * 0.5;
        let column_x = panel_x + (panel_w - cell_w) * 0.5;

        let content_h =
            cells.len() as f32 * TRAY_CELL + (cells.len().saturating_sub(1) as f32) * TRAY_GAP;
        let needed_h = (content_h + TRAY_PAD * 2.0).min(rect.h);
        let panel_y = rect.y + (rect.h - needed_h) * 0.5;
        let panel_rect = Rect {
            x: panel_x,
            y: panel_y,
            w: panel_w,
            h: needed_h,
        };

        let original = f.theme();
        let base_theme = themes::hud(&hud);

        // Interactions captured inside the layout closures for dispatch
        // after them (opening a popover mutates `self`).
        let mut activations: Vec<String> = Vec::new();
        let mut secondary: Vec<(String, bool)> = Vec::new();
        let mut resolved: Vec<(String, Rect)> = Vec::new();

        let scroll_id = "tessera-hud-tray-column-scroll";
        let scroll_viewport_h = (needed_h - TRAY_PAD * 2.0).max(1.0);
        let needs_scroll = content_h > scroll_viewport_h;
        let scrollbar_theme = if needs_scroll && self.tray_scrollbar_reveal > 0.01 {
            base_theme
                .with_scrollbar_width(4.0)
                .with_scrollbar_radius(2.0)
                .with_scrollbar_track_color(hud.background.with_alpha(0))
                .with_scrollbar_thumb_color(hud.surface.with_alpha(150))
                .with_scrollbar_thumb_hover_color(hud.surface.with_alpha(220))
        } else {
            // Zero width removes the scrollbar entirely when idle.
            base_theme.with_scrollbar_width(0.0)
        };

        // Tray background surface: sleek capsule container matching other HUD panels
        f.place(
            "tessera-hud-tray-surface",
            &chrome_place(
                panel_rect,
                LayoutOpts {
                    radius: panel_w * 0.5,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(&sized(panel_rect.w, panel_rect.h), |_| {});
            },
        );

        f.set_theme(scrollbar_theme);
        f.place(
            "tessera-hud-tray-column",
            &chrome_place(panel_rect, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: panel_rect.w,
                        height: panel_rect.h,
                        pad: TRAY_PAD,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.scroll(scroll_id, |f| {
                            f.column_ex(
                                &LayoutOpts {
                                    width: cell_w,
                                    gap: TRAY_GAP,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    for (index, cell) in cells.iter().enumerate() {
                                        // Hover or active menu-open state raises a plate
                                        // BEHIND the icon.
                                        let est_y = panel_y
                                            + TRAY_PAD
                                            + index as f32 * (TRAY_CELL + TRAY_GAP);
                                        let est_rect = Rect {
                                            x: column_x,
                                            y: est_y,
                                            w: cell_w,
                                            h: TRAY_CELL,
                                        };
                                        let hover = contains(est_rect, cursor.0, cursor.1);
                                        let is_open =
                                            self.menu_open_for.as_deref() == Some(&cell.key);
                                        let plate_radius = cell_w * 0.32;
                                        if is_open {
                                            f.place(
                                                &format!("tessera-hud-tray-active-{}", cell.key),
                                                &chrome_place(
                                                    est_rect,
                                                    LayoutOpts {
                                                        radius: plate_radius,
                                                        bg: hud.selection_surface,
                                                        border: hud.accent.with_alpha(80),
                                                        border_width: 1.0,
                                                        ..Default::default()
                                                    },
                                                ),
                                                |f| {
                                                    f.row_ex(
                                                        &LayoutOpts {
                                                            width: cell_w,
                                                            height: TRAY_CELL,
                                                            ..Default::default()
                                                        },
                                                        |_| {},
                                                    );
                                                },
                                            );
                                        } else if hover {
                                            f.place(
                                                &format!("tessera-hud-tray-plate-{}", cell.key),
                                                &chrome_place(
                                                    est_rect,
                                                    LayoutOpts {
                                                        radius: plate_radius,
                                                        bg: hud.accent_surface_hover,
                                                        border: hud.accent.with_alpha(60),
                                                        border_width: 1.0,
                                                        ..Default::default()
                                                    },
                                                ),
                                                |f| {
                                                    f.row_ex(
                                                        &LayoutOpts {
                                                            width: cell_w,
                                                            height: TRAY_CELL,
                                                            ..Default::default()
                                                        },
                                                        |_| {},
                                                    );
                                                },
                                            );
                                        }
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
                                                bg: Color(0),
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
                                            activations.push(key.clone());
                                        } else if response.right_clicked {
                                            secondary.push((key.clone(), has_menu));
                                        }
                                    }
                                },
                            );
                        });
                        f.spacer(0.0);
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
                        y: panel_y + TRAY_PAD,
                        w: cell_w,
                        h: TRAY_CELL,
                    });
                self.menu_owner = Rect {
                    x: panel_x,
                    y: cell_rect.y,
                    w: panel_w,
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

        // Keep the owner rect fresh against relayout or item movement.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some((_, rect)) = resolved.iter().find(|(k, _)| k == &key) {
                self.menu_owner = Rect {
                    x: panel_x,
                    y: rect.y,
                    w: panel_w,
                    h: rect.h,
                };
            } else {
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// Render the dbusmenu popover. The visible rows come from walking
    /// `menu.root.children` along `self.menu_path`. Submenu rows push onto
    /// `menu_path`, leaf rows send `MenuEvent` and dismiss the popover, and
    /// click-away closes it unless the press falls on the owner tray cell.
    pub(super) fn render_tray_menu(
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
            &chrome_place(popover_bounds, materials::hud_panel(&hud)),
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

    /// Compact MPRIS now-playing card at the left-bottom anchor. The card
    /// remains useful as an honest empty state when no player owns MPRIS;
    /// when one appears, transport actions are forwarded to the worker and
    /// never block the render thread.
    pub(super) fn render_media_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
    ) {
        if rect.w < 150.0 || rect.h < 80.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * -20.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        let snapshot = self
            .media
            .as_ref()
            .map(MediaHandle::snapshot)
            .unwrap_or_default();
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let mut command = None;
        let card_pad = 12.0;
        let icon_w = 18.0;
        let icon_gap = 10.0;
        // Text area occupies the width of the card minus padding, play icon, and gap.
        // Safety margin ensures wide CJK characters and bold display text stay within bounds.
        let text_max_w = (rect.w - card_pad * 2.0 - icon_w - icon_gap - 8.0).max(20.0);

        let identity_raw = if snapshot.available && !snapshot.identity.is_empty() {
            &snapshot.identity
        } else {
            i18n.text(Message::NowPlaying)
        };
        let identity_label = ellipsize(f, identity_raw, type_scale.footnote, text_max_w);

        let title_raw = if snapshot.available && !snapshot.title.is_empty() {
            &snapshot.title
        } else {
            i18n.text(Message::NotPlaying)
        };
        let title_label = ellipsize(f, title_raw, type_scale.body, text_max_w);

        let artist_label = ellipsize(f, &snapshot.artist, type_scale.footnote, text_max_w);

        f.set_theme(base_theme);
        f.place(
            "tessera-hud-media",
            &chrome_place(
                rect,
                LayoutOpts {
                    radius: 18.0,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: 4.0,
                        pad: 12.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                height: 44.0,
                                gap: 10.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.icon(
                                    if snapshot.playing {
                                        Icon::Pause
                                    } else {
                                        Icon::Play
                                    },
                                    18.0,
                                );
                                f.column_ex(
                                    &LayoutOpts {
                                        flex: 1.0,
                                        max_width: text_max_w,
                                        gap: 2.0,
                                        ..Default::default()
                                    },
                                    |f| {
                                        f.set_theme(muted_theme);
                                        display_label(f, &identity_label, type_scale.footnote);
                                        f.set_theme(base_theme);
                                        display_label(f, &title_label, type_scale.body);
                                        if snapshot.available && !snapshot.artist.is_empty() {
                                            f.set_theme(muted_theme);
                                            display_label(f, &artist_label, type_scale.footnote);
                                            f.set_theme(base_theme);
                                        }
                                    },
                                );
                            },
                        );

                        f.row_ex(
                            &LayoutOpts {
                                height: 36.0,
                                gap: 6.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.flex(1.0);
                                f.spacer(0.0);
                                f.set_theme(if snapshot.can_previous {
                                    base_theme
                                } else {
                                    muted_theme
                                });
                                let (previous, _) = f.pressable_row(
                                    "tessera-hud-media-previous",
                                    "Previous",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: Color::TRANSPARENT,
                                        ..Default::default()
                                    },
                                    |f, _| f.centered(36.0, 36.0, |f| f.icon(Icon::SkipBack, 17.0)),
                                );
                                if previous.clicked && snapshot.can_previous {
                                    command = Some(MediaCommand::Previous);
                                }
                                f.set_theme(if snapshot.available {
                                    base_theme
                                } else {
                                    muted_theme
                                });
                                let (play_pause, _) = f.pressable_row(
                                    "tessera-hud-media-play-pause",
                                    "Play or pause",
                                    &LayoutOpts {
                                        width: 40.0,
                                        height: 36.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: hud.selection_surface,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.centered(40.0, 36.0, |f| {
                                            f.icon(
                                                if snapshot.playing {
                                                    Icon::Pause
                                                } else {
                                                    Icon::Play
                                                },
                                                18.0,
                                            )
                                        })
                                    },
                                );
                                if play_pause.clicked && snapshot.available {
                                    command = Some(MediaCommand::PlayPause);
                                }
                                f.set_theme(if snapshot.can_next {
                                    base_theme
                                } else {
                                    muted_theme
                                });
                                let (next, _) = f.pressable_row(
                                    "tessera-hud-media-next",
                                    "Next",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: Color::TRANSPARENT,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.centered(36.0, 36.0, |f| f.icon(Icon::SkipForward, 17.0))
                                    },
                                );
                                if next.clicked && snapshot.can_next {
                                    command = Some(MediaCommand::Next);
                                }
                                f.flex(1.0);
                                f.spacer(0.0);
                            },
                        );
                    },
                );
            },
        );
        f.set_theme(original);
        if let Some(command) = command
            && let Some(media) = &self.media
        {
            media.send(command);
        }
    }

    /// The first of the three right-bottom components: a stable segmented
    /// mode selector. Its visible segments and interaction targets share
    /// the same absolute geometry, preventing the indicator and labels from
    /// drifting into different layout flows.
    pub(super) fn render_work_mode_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if rect.w < 120.0 || rect.h < 32.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let current_mode = self.status.power_mode;
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        const INSET: f32 = 4.0;
        let control = Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };
        let inner = Rect {
            x: control.x + INSET,
            y: control.y + INSET,
            w: (control.w - INSET * 2.0).max(1.0),
            h: (control.h - INSET * 2.0).max(1.0),
        };
        let seg_w = inner.w / PowerMode::ALL.len() as f32;

        f.set_theme(base_theme);
        f.place(
            "tessera-hud-work-mode-surface",
            &chrome_place(
                control,
                LayoutOpts {
                    radius: control.h * 0.5,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(&sized(control.w, control.h), |_| {});
            },
        );

        // Sliding indicator: position from the spring (segment units), so
        // it overshoots slightly and settles with elastic bounce.
        let spring_pos = self
            .work_mode_spring
            .value
            .clamp(0.0, PowerMode::ALL.len().saturating_sub(1) as f32);
        let indicator_x = inner.x + spring_pos * seg_w;
        f.place(
            "tessera-hud-work-mode-indicator",
            &chrome_place(
                Rect {
                    x: indicator_x,
                    y: inner.y,
                    w: seg_w,
                    h: inner.h,
                },
                LayoutOpts {
                    radius: inner.h * 0.5,
                    bg: hud.selection_surface,
                    border: hud.accent.with_alpha(80),
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: seg_w,
                        height: inner.h,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        // Segment hit areas over the indicator.
        self.work_mode_hover = None;
        for (index, mode) in PowerMode::ALL.iter().enumerate() {
            let active = *mode == current_mode;
            let label = match mode {
                PowerMode::Balanced => i18n.text(Message::PowerModeBalanced),
                PowerMode::Awake => i18n.text(Message::PowerModeAwake),
                PowerMode::Secure => i18n.text(Message::PowerModeSecure),
            };
            let hint = power_mode_hint(*mode);
            let seg_rect = Rect {
                x: inner.x + index as f32 * seg_w,
                y: inner.y,
                w: seg_w,
                h: inner.h,
            };
            let hovered = contains(seg_rect, self.cursor_hint.0, self.cursor_hint.1);
            if hovered {
                self.work_mode_hover = Some(index);
            }
            let fg = if active { hud.accent } else { hud.text_muted };
            let seg_theme = themes::hud(&hud).with_fg(fg);
            f.set_theme(seg_theme);
            let mut clicked = false;
            f.place(
                &format!("tessera-hud-work-seg-place-{index}"),
                &chrome_place(seg_rect, transparent()),
                |f| {
                    let (response, _) = f.pressable_row(
                        &format!("tessera-hud-work-seg-{index}"),
                        label,
                        &LayoutOpts {
                            width: seg_rect.w,
                            height: seg_rect.h,
                            cross: Align::Center,
                            radius: inner.h * 0.5,
                            bg: Color::TRANSPARENT,
                            ..Default::default()
                        },
                        |f, _| {
                            f.centered(seg_rect.w, seg_rect.h, |f| {
                                display_label(
                                    f,
                                    &truncate(label, (seg_rect.w / 7.0).max(3.0) as usize),
                                    type_scale.footnote,
                                );
                            });
                        },
                    );
                    clicked = response.clicked;
                },
            );
            if clicked && !active {
                out.system_actions
                    .push(SystemAction::SetPowerMode { mode: *mode });
            }
            // Segment tooltip on hover.
            if hovered && self.work_mode_tooltip_reveal > 0.02 {
                render_tooltip(
                    f,
                    &format!("tessera-hud-work-seg-tip-{index}"),
                    Rect {
                        x: seg_rect.x,
                        y: control.y - 32.0,
                        w: seg_w.max(120.0),
                        h: 26.0,
                    },
                    hint,
                    hud,
                    type_scale,
                    self.work_mode_tooltip_reveal,
                );
            }
        }

        f.set_theme(original);
    }

    /// The second and third right-bottom components: explicit lock and
    /// power buttons. Both the painted button and its hit target occupy the
    /// same fixed rect; destructive power-off still leaves through the
    /// runtime's system-level confirmation flow.
    pub(super) fn render_power_session_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let group_w = SESSION_BUTTON * 2.0 + SESSION_BUTTON_GAP;
        if rect.w < group_w || rect.h < SESSION_BUTTON {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        let original = f.theme();
        let button_y = rect.y + (rect.h - SESSION_BUTTON) * 0.5;
        let group_x = rect.x + (rect.w - group_w) * 0.5;
        let lock_rect = Rect {
            x: group_x,
            y: button_y,
            w: SESSION_BUTTON,
            h: SESSION_BUTTON,
        };
        let power_rect = Rect {
            x: group_x + SESSION_BUTTON + SESSION_BUTTON_GAP,
            y: button_y,
            w: SESSION_BUTTON,
            h: SESSION_BUTTON,
        };
        let lock_hover = contains(lock_rect, cursor.0, cursor.1);
        let power_hover = contains(power_rect, cursor.0, cursor.1);

        let mut lock_clicked = false;
        f.set_theme(themes::hud(&hud).with_fg(if lock_hover {
            hud.accent
        } else {
            hud.text_muted
        }));
        f.place(
            "tessera-hud-session-lock-place",
            &chrome_place(
                lock_rect,
                LayoutOpts {
                    radius: 14.0,
                    bg: hud.surface_recessed,
                    border: if lock_hover { hud.accent } else { hud.border },
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                let (response, _) = f.pressable_row(
                    "tessera-hud-session-lock",
                    i18n.text(Message::LockNow),
                    &LayoutOpts {
                        width: lock_rect.w,
                        height: lock_rect.h,
                        cross: Align::Center,
                        radius: 14.0,
                        bg: Color::TRANSPARENT,
                        ..Default::default()
                    },
                    |f, _| {
                        f.centered(lock_rect.w, lock_rect.h, |f| {
                            f.icon(Icon::Shield, 18.0);
                        });
                    },
                );
                lock_clicked = response.clicked;
            },
        );
        if lock_clicked {
            out.lock = true;
        }

        let mut power_clicked = false;
        f.set_theme(themes::hud(&hud).with_fg(if power_hover { hud.accent } else { hud.text }));
        f.place(
            "tessera-hud-session-power-place",
            &chrome_place(
                power_rect,
                LayoutOpts {
                    radius: 14.0,
                    bg: hud.selection_surface,
                    border: hud.accent.with_alpha(if power_hover { 180 } else { 80 }),
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                let (response, _) = f.pressable_row(
                    "tessera-hud-session-power",
                    i18n.text(Message::PowerOff),
                    &LayoutOpts {
                        width: power_rect.w,
                        height: power_rect.h,
                        cross: Align::Center,
                        radius: 14.0,
                        bg: Color::TRANSPARENT,
                        ..Default::default()
                    },
                    |f, _| {
                        f.centered(power_rect.w, power_rect.h, |f| {
                            f.icon(Icon::Zap, 18.0);
                        });
                    },
                );
                power_clicked = response.clicked;
            },
        );
        if power_clicked {
            self.request_system_confirm(SystemAction::PowerOff, out);
        }

        self.session_hover = if power_hover {
            Some("power")
        } else if lock_hover {
            Some("lock")
        } else {
            None
        };
        if let Some(kind) = self.session_hover
            && self.session_tooltip_reveal > 0.02
        {
            let (anchor, label) = if kind == "power" {
                (power_rect, i18n.text(Message::PowerOff))
            } else {
                (lock_rect, i18n.text(Message::LockNow))
            };
            render_tooltip(
                f,
                &format!("tessera-hud-session-{kind}-tip"),
                Rect {
                    x: anchor.x - 34.0,
                    y: anchor.y - 32.0,
                    w: 120.0,
                    h: 26.0,
                },
                label,
                hud,
                type_scale,
                self.session_tooltip_reveal,
            );
        }
        f.set_theme(original);
    }
    /// Route a destructive session action through the system-level
    /// confirmation. The panel never performs power transitions itself:
    /// like the power notification paths, the request leaves through
    /// `ChromeEvents` and the compositor runtime opens the consent chrome
    /// (`StartConfirmPick`) before executing anything.
    pub(super) fn request_system_confirm(&mut self, action: SystemAction, out: &mut ChromeEvents) {
        if self.power_pending_confirm.is_some() {
            return;
        }
        self.power_pending_confirm = Some(action.clone());
        out.system_actions.push(action);
    }
}

/// The one-line behavior hint under each work-mode segment.
fn power_mode_hint(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Balanced => "Dim → Lock → Blank → Suspend",
        PowerMode::Awake => "Screen stays lit & unlocked",
        PowerMode::Secure => "Lock on idle, screen stays lit",
    }
}

/// Render a small frameless tooltip pill with `reveal` opacity over the
/// anchor's top edge. Purely presentational; hover state is caller-owned.
fn render_tooltip(
    f: &mut Frame,
    id: &str,
    anchor: Rect,
    text: &str,
    hud: ControlCenterColors,
    type_scale: TypeScale,
    reveal: f32,
) {
    let original_opacity = f.opacity();
    f.set_opacity(reveal * original_opacity);
    f.place(
        id,
        &chrome_place(
            anchor,
            LayoutOpts {
                radius: 8.0,
                bg: hud.surface_recessed,
                border: hud.border,
                border_width: 1.0,
                ..Default::default()
            },
        ),
        |f| {
            f.row_ex(
                &LayoutOpts {
                    width: anchor.w,
                    height: anchor.h,
                    cross: Align::Center,
                    pad: 6.0,
                    ..Default::default()
                },
                |f| {
                    display_label(f, text, type_scale.footnote);
                },
            );
        },
    );
    f.set_opacity(original_opacity);
}

/// Locale wall-clock strings for the top-center surface: `("21:47",
/// "Saturday, June 21")`. libc `localtime_r` keeps this off any additional
/// dependency, matching the lock screen's clock.
pub(crate) fn clock_strings() -> (String, String) {
    use std::ffi::{CStr, c_char};

    let mut timestamp = 0;
    unsafe {
        libc::time(&mut timestamp);
    }
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let local = unsafe {
        if libc::localtime_r(&timestamp, local.as_mut_ptr()).is_null() {
            return ("--:--".into(), String::new());
        }
        local.assume_init()
    };
    let strftime = |time: &libc::tm, format: &CStr| -> String {
        let mut output = [0 as c_char; 128];
        let len =
            unsafe { libc::strftime(output.as_mut_ptr(), output.len(), format.as_ptr(), time) };
        if len == 0 {
            String::new()
        } else {
            unsafe { CStr::from_ptr(output.as_ptr()) }
                .to_string_lossy()
                .trim()
                .to_owned()
        }
    };
    (strftime(&local, c"%H:%M"), strftime(&local, c"%A, %B %e"))
}
/// Project the two Quick Controls toggles onto the session power mode
/// (ADR-0140).
///
/// `keep_awake` is the display axis ("never blank the screen"); `auto_lock`
/// is the security axis ("lock on schedule"). The one combination the
/// power pipeline cannot honor — keep awake but never lock — is unsafe on a
/// shared machine (anyone can walk up), so it projects onto
/// [`tessera_desktop::power::PowerMode::Awake`]: with no automatic lock there is nothing to
/// power off or suspend behind, so dimming is the strongest idle response
/// the pipeline may keep. The toggles then read the mode back honestly:
/// "keep awake" shows on (the display indeed never blanks) even though the
/// user turned it off, telling them the security axis won the conflict.
#[allow(dead_code)]
pub(crate) fn power_mode_for(
    keep_awake: bool,
    auto_lock: bool,
) -> tessera_desktop::power::PowerMode {
    use tessera_desktop::power::PowerMode;
    match (keep_awake, auto_lock) {
        (false, true) => PowerMode::Balanced,
        (true, true) => PowerMode::Secure,
        (_, false) => PowerMode::Awake,
    }
}
