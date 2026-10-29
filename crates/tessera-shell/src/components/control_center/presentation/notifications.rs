//! The notifications panel.

use super::*;

impl ControlCenter {
    /// The top-right notifications stream: each notification as its own
    /// recessed card — a distinct background, hairline border, and 10px
    /// gaps so individuals read clearly — newest first, with the tail
    /// fading out toward the bottom of the region. The scrollbar stays
    /// hidden until the user wheels over the stream and fades back out
    /// when idle. Slides in from the top-right.
    pub(crate) fn render_notifications_panel(
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
}
