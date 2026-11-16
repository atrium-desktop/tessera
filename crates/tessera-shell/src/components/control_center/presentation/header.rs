//! The controls card's header: who is signed in, and the three session-level
//! actions — open Settings, lock, and the power menu — plus the session strip
//! the power button discloses.

use super::*;

/// One icon-only header action.
#[derive(Clone, Copy)]
enum HeaderAction {
    Settings,
    Lock,
    Power,
}

impl ControlCenter {
    /// The header row: a compact persona (portrait, display name, account and
    /// host) and the Settings, Lock, and Power buttons, right-aligned.
    pub(crate) fn render_header(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let base = themes::hud(&hud);
        let muted = themes::hud_muted(base, &hud);
        let original = f.theme();

        self.render_avatar(f, (rect.x + AVATAR_SIZE * 0.5, rect.y + rect.h * 0.5));

        let buttons_w = HEADER_BUTTON * 3.0 + 6.0 * 2.0;
        let text_x = rect.x + AVATAR_SIZE + 12.0;
        let text_w = (rect.x + rect.w - buttons_w - 12.0 - text_x).max(1.0);
        let name = ellipsize(f, &self.profile.display_name, type_scale.headline, text_w);
        let mut account = format!("@{}", self.profile.username);
        if !self.profile.hostname.is_empty() {
            account.push_str(" · ");
            account.push_str(&self.profile.hostname);
        }
        let account = ellipsize(f, &account, type_scale.footnote, text_w);
        f.set_theme(base);
        f.place(
            "control-center-persona",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: rect.y,
                    w: text_w,
                    h: rect.h,
                },
                transparent(),
            ),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: rect.h,
                        gap: 2.0,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        display_label(f, &name, type_scale.headline);
                        f.set_theme(muted);
                        f.label_sized(&account, type_scale.footnote);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            },
        );

        let mut clicked: Option<HeaderAction> = None;
        let mut hovered: Option<(Tip, Rect)> = None;
        let buttons = [
            (
                HeaderAction::Settings,
                Tip::Settings,
                lens::sys::lens_icon_id::LENS_ICON_SETTINGS,
                i18n.text(Message::Settings),
            ),
            (
                HeaderAction::Lock,
                Tip::Lock,
                // A padlock, not a shield: this locks the session; it does not
                // vouch for security posture (ADR-0080).
                lens::sys::lens_icon_id::LENS_ICON_LOCK,
                i18n.text(Message::LockNow),
            ),
            (
                HeaderAction::Power,
                Tip::Power,
                lens::sys::lens_icon_id::LENS_ICON_POWER,
                i18n.text(Message::PowerAndSession),
            ),
        ];
        let buttons_x = rect.x + rect.w - buttons_w;
        let button_y = rect.y + (rect.h - HEADER_BUTTON) * 0.5;
        for (index, (action, tip, icon, label)) in buttons.into_iter().enumerate() {
            let button = Rect {
                x: buttons_x + index as f32 * (HEADER_BUTTON + 6.0),
                y: button_y,
                w: HEADER_BUTTON,
                h: HEADER_BUTTON,
            };
            let active = matches!(action, HeaderAction::Power) && self.session_open;
            let (bg, fg) = if active {
                (hud.selection_surface, hud.accent)
            } else {
                (hud.surface_recessed, hud.text)
            };
            f.set_theme(base.with_fg(fg));
            f.place(
                &format!("control-center-header-action-{index}"),
                &chrome_place(button, transparent()),
                |f| {
                    let (response, _) = f.pressable_row(
                        &format!("control-center-header-button-{index}"),
                        label,
                        &LayoutOpts {
                            width: button.w,
                            height: button.h,
                            radius: button.h * 0.5,
                            bg,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f, _| {
                            f.centered(button.w, button.h, |f| f.icon_raw(icon, 18.0));
                        },
                    );
                    if response.clicked {
                        clicked = Some(action);
                    }
                    if response.hovered {
                        hovered = Some((tip, button));
                    }
                },
            );
        }
        f.set_theme(original);

        if let Some((tip, anchor)) = hovered {
            self.hover_tip(tip, anchor);
        }
        match clicked {
            Some(HeaderAction::Settings) => {
                out.spawn = Some(tessera_desktop::app::settings_app::entry(None));
                self.close(out);
            }
            Some(HeaderAction::Lock) => {
                out.lock = true;
                self.close(out);
            }
            Some(HeaderAction::Power) => self.session_open = !self.session_open,
            None => {}
        }
    }

    /// The portrait disc: the configured photo or VRM, else initials on a
    /// flat disc, inside a thin accent ring.
    fn render_avatar(&self, f: &mut Frame, center: (f32, f32)) {
        let hud = self.panel_colors();
        let style = self.design.avatars.for_role(AvatarRole::PersonaHeader);
        let ring = AVATAR_SIZE + 6.0;
        render_ring(
            f,
            "control-center-avatar-ring",
            center,
            ring,
            hud.accent.with_alpha(160),
            1.5,
        );
        let inner = AVATAR_SIZE - 2.0;
        render_disc(
            f,
            "control-center-avatar-disc",
            center,
            inner,
            style.fallback_surface,
        );
        let rect = Rect {
            x: center.0 - inner * 0.5,
            y: center.1 - inner * 0.5,
            w: inner,
            h: inner,
        };
        match &self.avatar {
            Some(avatar) => {
                let texture = avatar.texture().as_raw();
                f.place(
                    "control-center-avatar",
                    &chrome_place(rect, transparent()),
                    |f| {
                        f.row_ex(&sized(inner, inner), |f| {
                            // SAFETY: the portrait owns the texture for the
                            // panel's lifetime and is replaced only between
                            // frames (`reload_avatar_if_ready`).
                            unsafe { f.image(texture, inner, inner) };
                        });
                    },
                );
            }
            None => {
                let original = f.theme();
                f.set_theme(themes::hud(&hud).with_fg(style.fallback_foreground));
                f.place(
                    "control-center-avatar-initials",
                    &chrome_place(rect, transparent()),
                    |f| {
                        f.centered(inner, inner, |f| {
                            display_label(f, &self.profile.initials, inner * style.initials_scale);
                        });
                    },
                );
                f.set_theme(original);
            }
        }
    }

    /// Suspend · Restart · Power off. Every action leaves through the
    /// system-level confirmation; the sheet never performs one itself.
    pub(crate) fn render_session_strip(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let actions = [
            (
                SystemAction::Suspend,
                lens::sys::lens_icon_id::LENS_ICON_MOON,
                i18n.text(Message::Suspend),
            ),
            (
                SystemAction::Reboot,
                lens::sys::lens_icon_id::LENS_ICON_ROTATE_CW,
                i18n.text(Message::Restart),
            ),
            (
                SystemAction::PowerOff,
                lens::sys::lens_icon_id::LENS_ICON_POWER,
                i18n.text(Message::PowerOff),
            ),
        ];
        let gap = 8.0;
        let button_w = ((rect.w - gap * 2.0) / 3.0).max(1.0);
        let mut chosen: Option<SystemAction> = None;
        f.set_theme(themes::hud(&hud));
        f.place(
            "control-center-session-strip",
            &chrome_place(rect, transparent()),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        for (index, (action, icon, label)) in actions.into_iter().enumerate() {
                            let destructive = matches!(action, SystemAction::PowerOff);
                            let (response, _) = f.pressable_row(
                                &format!("control-center-session-{index}"),
                                label,
                                &LayoutOpts {
                                    width: button_w,
                                    height: rect.h,
                                    radius: 10.0,
                                    gap: 6.0,
                                    pad: 8.0,
                                    cross: Align::Center,
                                    bg: if destructive {
                                        hud.selection_surface
                                    } else {
                                        hud.surface_recessed
                                    },
                                    ..Default::default()
                                },
                                |f, _| {
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    f.icon_raw(icon, 16.0);
                                    display_label(f, label, type_scale.footnote);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                },
                            );
                            if response.clicked {
                                chosen = Some(action);
                            }
                        }
                    },
                );
            },
        );
        f.set_theme(original);
        if let Some(action) = chosen {
            self.request_system_confirm(action, out);
        }
    }
}
