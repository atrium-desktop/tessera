//! The lock/power session panel (ADR-0140).

use super::*;

impl ControlCenter {
    /// The second and third right-bottom components: explicit lock and
    /// power buttons. Both the painted button and its hit target occupy the
    /// same fixed rect; destructive power-off still leaves through the
    /// runtime's system-level confirmation flow.
    pub(crate) fn render_power_session_panel(
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
                            // A padlock, not a shield: this locks the session,
                            // it does not vouch for security posture (ADR-0080).
                            f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_LOCK, 18.0);
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
                            // The universal power glyph, not a lightning bolt:
                            // Zap reads as "energy/charging", not "shut down".
                            f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_POWER, 18.0);
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
}
