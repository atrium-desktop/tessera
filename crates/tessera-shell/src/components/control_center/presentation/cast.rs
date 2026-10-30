//! Wireless display and casting detail page (ADR-0188, ADR-0192).
//!
//! Exposes active display casting sessions, virtual extend outputs,
//! and interactive discovery of nearby wireless display (Miracast / WFD) receivers.

use super::*;

impl ControlCenter {
    /// Expanded Cast detail view: live casting sessions, virtual extend outputs,
    /// and interactive discovery of nearby wireless display sinks (Miracast / Wi-Fi Display).
    pub(crate) fn render_cast_detail_view(
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
        let cast_active = status.cast_streams > 0
            || status.virtual_displays > 0
            || status.cast_devices.iter().any(|d| d.connected);

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-cast-detail",
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
                        // Header row: [< Back] Title  [↻ Scan]
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
                                    "tessera-cast-back-btn",
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
                                        f.icon_raw(
                                            lens::sys::lens_icon_id::LENS_ICON_CHEVRON_LEFT,
                                            16.0,
                                        );
                                        display_label(f, "Back", type_scale.body);
                                    },
                                );
                                if back_resp.clicked {
                                    self.cast_expanded = false;
                                }

                                f.spacer(4.0);
                                display_label(f, i18n.text(Message::Cast), type_scale.headline);

                                f.flex(1.0);
                                f.spacer(0.0);

                                // Refresh / Scan button
                                let is_scanning = status.cast_scanning || self.cast_scanning;
                                let (refresh_resp, _) = f.pressable_row(
                                    "tessera-cast-refresh-btn",
                                    "Scan",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 18.0,
                                        cross: Align::Center,
                                        bg: if is_scanning {
                                            hud.selection_surface
                                        } else {
                                            hud.surface_recessed
                                        },
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                        f.icon_raw(
                                            lens::sys::lens_icon_id::LENS_ICON_REFRESH_CW,
                                            16.0,
                                        );
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    },
                                );
                                if refresh_resp.clicked {
                                    self.cast_scanning = true;
                                    out.system_actions.push(SystemAction::ScanCast);
                                }
                            },
                        );

                        let list_h = (area.h - 36.0 - 12.0).max(1.0);
                        f.scroll("tessera-cast-scroll", |f| {
                            f.column_ex(
                                &LayoutOpts {
                                    width: area.w,
                                    height: list_h,
                                    gap: 12.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |f| {
                                    // Section: Current Session
                                    display_label(f, "Current Session", type_scale.headline);

                                    let (bg, border, icon_fg) = if cast_active {
                                        (hud.selection_surface, hud.accent.with_alpha(80), hud.accent)
                                    } else {
                                        (hud.surface_recessed, hud.border, hud.text_muted)
                                    };

                                    f.row_ex(
                                        &LayoutOpts {
                                            width: area.w,
                                            height: 52.0,
                                            pad: 10.0,
                                            radius: 14.0,
                                            bg,
                                            border,
                                            border_width: 1.0,
                                            cross: Align::Center,
                                            gap: 10.0,
                                            ..Default::default()
                                        },
                                        |f| {
                                            f.set_theme(themes::hud(&hud).with_fg(icon_fg));
                                            f.icon_raw(
                                                lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                20.0,
                                            );
                                            f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                            f.column_ex(
                                                &LayoutOpts {
                                                    cross: Align::Start,
                                                    gap: 2.0,
                                                    ..Default::default()
                                                },
                                                |f| {
                                                    let title = if cast_active {
                                                        "Wireless Display Active"
                                                    } else {
                                                        "No Active Display Session"
                                                    };
                                                    display_label(f, title, type_scale.body);

                                                    let sub = if status.cast_streams > 0 {
                                                        format!("{} stream(s) rendering to virtual display", status.cast_streams)
                                                    } else if status.virtual_displays > 0 {
                                                        format!("{} virtual extended display(s) configured", status.virtual_displays)
                                                    } else {
                                                        "Ready to cast desktop or mirror screen to nearby displays".to_string()
                                                    };
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                    display_label(f, &sub, type_scale.footnote);
                                                },
                                            );

                                            f.flex(1.0);
                                            f.spacer(0.0);

                                            if cast_active {
                                                let (disc_resp, _) = f.pressable_row(
                                                    "tessera-cast-disconnect-btn",
                                                    "Disconnect",
                                                    &LayoutOpts {
                                                        width: 90.0,
                                                        height: 30.0,
                                                        radius: 8.0,
                                                        bg: hud.surface_recessed,
                                                        border: hud.border,
                                                        border_width: 1.0,
                                                        cross: Align::Center,
                                                        ..Default::default()
                                                    },
                                                    |f, _| {
                                                        f.flex(1.0);
                                                        f.spacer(0.0);
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                        display_label(f, "Disconnect", type_scale.caption);
                                                        f.flex(1.0);
                                                        f.spacer(0.0);
                                                    },
                                                );
                                                if disc_resp.clicked {
                                                    let addr = status
                                                        .cast_devices
                                                        .iter()
                                                        .find(|d| d.connected)
                                                        .map(|d| d.address.clone())
                                                        .unwrap_or_default();
                                                    out.system_actions.push(
                                                        SystemAction::DisconnectCast { address: addr },
                                                    );
                                                }
                                            }
                                        },
                                    );

                                    f.spacer(4.0);

                                    // Section: Nearby Displays
                                    display_label(f, "Nearby Displays", type_scale.headline);

                                    if status.cast_scanning || self.cast_scanning {
                                        f.row_ex(
                                            &LayoutOpts {
                                                width: area.w,
                                                height: 40.0,
                                                pad: 8.0,
                                                radius: 10.0,
                                                bg: hud.surface_recessed,
                                                cross: Align::Center,
                                                gap: 8.0,
                                                ..Default::default()
                                            },
                                            |f| {
                                                f.icon_raw(
                                                    lens::sys::lens_icon_id::LENS_ICON_REFRESH_CW,
                                                    16.0,
                                                );
                                                f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                display_label(
                                                    f,
                                                    "Searching for nearby Wi-Fi Display (Miracast) & AirPlay receivers…",
                                                    type_scale.footnote,
                                                );
                                            },
                                        );
                                    }

                                    if status.cast_devices.is_empty() {
                                        f.column_ex(
                                            &LayoutOpts {
                                                width: area.w,
                                                pad: 16.0,
                                                radius: 12.0,
                                                bg: hud.surface_recessed,
                                                cross: Align::Center,
                                                gap: 6.0,
                                                ..Default::default()
                                            },
                                            |f| {
                                                f.icon_raw(
                                                    lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                    24.0,
                                                );
                                                f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                display_label(f, "No wireless displays found", type_scale.body);
                                                f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                display_label(
                                                    f,
                                                    "Ensure screen mirroring is enabled on your display, then click Scan.",
                                                    type_scale.footnote,
                                                );
                                            },
                                        );
                                    } else {
                                        for (idx, device) in status.cast_devices.iter().enumerate() {
                                            let (row_resp, _) = f.pressable_row(
                                                &format!("tessera-cast-device-{idx}"),
                                                &device.name,
                                                &LayoutOpts {
                                                    width: area.w,
                                                    height: 52.0,
                                                    pad: 8.0,
                                                    radius: 12.0,
                                                    bg: hud.surface_recessed,
                                                    cross: Align::Center,
                                                    gap: 10.0,
                                                    ..Default::default()
                                                },
                                                |f, _| {
                                                    f.set_theme(
                                                        themes::hud(&hud).with_fg(if device.connected {
                                                            hud.accent
                                                        } else {
                                                            hud.text
                                                        }),
                                                    );
                                                    f.icon_raw(
                                                        lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                        20.0,
                                                    );
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                    f.column_ex(
                                                        &LayoutOpts {
                                                            cross: Align::Start,
                                                            gap: 1.0,
                                                            ..Default::default()
                                                        },
                                                        |f| {
                                                            display_label(f, &device.name, type_scale.body);
                                                            f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                            let proto_label = if device.protocol.is_empty() {
                                                                "Miracast (Wi-Fi Display)"
                                                            } else {
                                                                &device.protocol
                                                            };
                                                            display_label(f, proto_label, type_scale.footnote);
                                                        },
                                                    );

                                                    f.flex(1.0);
                                                    f.spacer(0.0);

                                                    if device.connected {
                                                        let (d, _) = f.pressable_row(
                                                            &format!("tessera-cast-disconnect-sink-{idx}"),
                                                            "Disconnect",
                                                            &LayoutOpts {
                                                                width: 88.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                cross: Align::Center,
                                                                bg: hud.surface,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                                display_label(f, "Disconnect", type_scale.caption);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if d.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::DisconnectCast {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    } else {
                                                        let (c, _) = f.pressable_row(
                                                            &format!("tessera-cast-connect-sink-{idx}"),
                                                            "Connect",
                                                            &LayoutOpts {
                                                                width: 80.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                cross: Align::Center,
                                                                bg: hud.accent,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(
                                                                    themes::hud(&hud).with_fg(Color::rgba(255, 255, 255, 255)),
                                                                );
                                                                display_label(f, "Connect", type_scale.caption);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if c.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::ConnectCast {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                },
                                            );
                                            let _ = row_resp;
                                        }
                                    }

                                    // Tips and guidance card
                                    f.row_ex(
                                        &LayoutOpts {
                                            width: area.w,
                                            pad: 12.0,
                                            radius: 12.0,
                                            bg: hud.surface_recessed,
                                            border: hud.border,
                                            border_width: 1.0,
                                            cross: Align::Start,
                                            gap: 10.0,
                                            ..Default::default()
                                        },
                                        |f| {
                                            f.icon_raw(
                                                lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                                18.0,
                                            );
                                            f.column_ex(
                                                &LayoutOpts {
                                                    cross: Align::Start,
                                                    gap: 4.0,
                                                    ..Default::default()
                                                },
                                                |f| {
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                    display_label(
                                                        f,
                                                        "How to connect to a wireless display",
                                                        type_scale.body,
                                                    );
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                    display_label(
                                                        f,
                                                        "1. Ensure 'Screen Mirroring' or 'Wireless Display' is enabled on your TV or projector.",
                                                        type_scale.footnote,
                                                    );
                                                    display_label(
                                                        f,
                                                        "2. Wi-Fi Direct (P2P) negotiates a low-latency H.264 stream directly via tessera-cast.",
                                                        type_scale.footnote,
                                                    );
                                                    display_label(
                                                        f,
                                                        "3. Discovered sinks will appear here automatically when discovery is active.",
                                                        type_scale.footnote,
                                                    );
                                                },
                                            );
                                        },
                                    );
                                },
                            );
                        });
                    },
                );
            },
        );
        f.set_theme(original);
    }
}
