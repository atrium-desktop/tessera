//! The notifications card: the wall clock over the notification list.

use super::*;

/// Locale wall-clock strings: `("21:47", "Saturday, June 21")`.
pub(crate) use crate::clock::clock_strings;

impl ControlCenter {
    pub(crate) fn render_notifications_card(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let design = self.design;
        let type_scale = design.typography;
        let base = themes::hud(&hud);
        let muted = themes::hud_muted(base, &hud);
        let notifications = self.notification_snapshot();
        let (time, date) = clock_strings();
        let inner_w = (rect.w - layout::CARD_PAD * 2.0).max(1.0);

        let mut dismissed: Vec<u64> = Vec::new();
        let original = f.theme();
        f.set_theme(base);
        materials::place_floating(
            f,
            "control-center-notifications",
            SurfaceRole::FloatingPanel,
            &design,
            rect,
            1.0,
            LayoutOpts {
                radius: design.radii.panel_lg,
                ..materials::canvas_panel(&hud)
            },
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        pad: layout::CARD_PAD,
                        gap: layout::SECTION_GAP,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        // The clock answers "what time is it" for a desktop
                        // with no top bar.
                        f.column_ex(
                            &LayoutOpts {
                                width: inner_w,
                                height: CLOCK_BLOCK_H,
                                gap: 2.0,
                                ..Default::default()
                            },
                            |f| {
                                f.set_theme(base);
                                display_label(f, &time, type_scale.hero);
                                f.set_theme(muted);
                                display_label(f, &date, type_scale.body);
                            },
                        );

                        f.row_ex(
                            &LayoutOpts {
                                width: inner_w,
                                height: LIST_HEADER_H,
                                gap: 8.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.set_theme(base);
                                display_label(
                                    f,
                                    i18n.text(Message::Notifications),
                                    type_scale.headline,
                                );
                                f.flex(1.0);
                                f.spacer(0.0);
                                if !notifications.is_empty() {
                                    f.set_theme(base.with_fg(hud.accent));
                                    if f.button_subtle(i18n.text(Message::ClearAllNotifications)) {
                                        dismissed.extend(notifications.iter().map(|n| n.id));
                                    }
                                }
                            },
                        );

                        if notifications.is_empty() {
                            f.set_theme(muted);
                            f.row_ex(
                                &LayoutOpts {
                                    width: inner_w,
                                    height: EMPTY_LIST_H,
                                    gap: 8.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    f.icon_raw(
                                        optics_ui::sys::optics_ui_icon_id::OPTICS_UI_ICON_BELL_OFF,
                                        16.0,
                                    );
                                    display_label(
                                        f,
                                        i18n.text(Message::NoNotifications),
                                        type_scale.body,
                                    );
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                },
                            );
                            return;
                        }

                        f.flex(1.0);
                        f.scroll("control-center-notification-list", |f| {
                            f.column_ex(
                                &LayoutOpts {
                                    width: inner_w,
                                    gap: NOTIFICATION_GAP,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |f| {
                                    for notification in notifications.iter() {
                                        if notification_card(
                                            f,
                                            notification,
                                            inner_w,
                                            hud,
                                            type_scale,
                                        ) {
                                            dismissed.push(notification.id);
                                        }
                                    }
                                },
                            );
                        });
                    },
                );
            },
        );
        f.set_theme(original);
        out.dismissed_notifications.extend(dismissed);
    }
}

/// One notification: the sending app, the summary, and the first line of the
/// body. Pressing the card dismisses it. Returns whether it was pressed.
fn notification_card(
    f: &mut Frame,
    notification: &Notification,
    width: f32,
    hud: ControlCenterColors,
    type_scale: TypeScale,
) -> bool {
    let base = themes::hud(&hud);
    let muted = themes::hud_muted(base, &hud);
    let text_w = (width - 24.0 - 16.0 - 10.0).max(1.0);
    let app = notification
        .app_id
        .as_deref()
        .filter(|app| !app.is_empty())
        .map(app_label)
        .unwrap_or_default();
    let summary = ellipsize(f, &notification.summary, type_scale.body, text_w);
    let first_line = notification.body.lines().next().unwrap_or_default();
    let body = ellipsize(f, first_line, type_scale.footnote, text_w);
    let app = ellipsize(f, &app, type_scale.caption, text_w);
    let (response, _) = f.pressable_row(
        &format!("control-center-notification-{}", notification.id),
        &notification.summary,
        &LayoutOpts {
            width,
            height: NOTIFICATION_H,
            pad: 12.0,
            gap: 10.0,
            radius: 12.0,
            bg: hud.surface_recessed,
            cross: Align::Center,
            ..Default::default()
        },
        |f, _| {
            f.set_theme(muted);
            f.icon_raw(optics_ui::sys::optics_ui_icon_id::OPTICS_UI_ICON_BELL, 16.0);
            f.column_ex(
                &LayoutOpts {
                    flex: 1.0,
                    gap: 1.0,
                    ..Default::default()
                },
                |f| {
                    if !app.is_empty() {
                        f.set_theme(muted);
                        f.label_sized(&app, type_scale.caption);
                    }
                    f.set_theme(base);
                    display_label(f, &summary, type_scale.body);
                    if !body.is_empty() {
                        f.set_theme(muted);
                        f.label_sized(&body, type_scale.footnote);
                    }
                },
            );
        },
    );
    f.set_theme(base);
    response.clicked
}

/// A readable app name from an application id: the last reverse-DNS segment
/// without a `.desktop` suffix (`org.gnome.Nautilus.desktop` → `Nautilus`).
pub(crate) fn app_label(app_id: &str) -> String {
    let trimmed = app_id.strip_suffix(".desktop").unwrap_or(app_id);
    trimmed
        .rsplit('.')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}
