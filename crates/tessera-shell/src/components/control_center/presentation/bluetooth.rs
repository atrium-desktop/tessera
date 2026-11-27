//! Bluetooth detail page: adapter state, paired devices, and pairing controls.

use super::*;
use tessera_design::icons::Glyph;

impl ControlCenter {
    /// Expanded Bluetooth detail view (ADR-0175): the paired/known peripherals
    /// and the in-range ones, with per-device connect/disconnect/pair/forget
    /// actions — the parity of [`Self::render_wifi_detail_view`].
    pub(crate) fn render_bluetooth_detail_view(
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
        let bt_active = status.bluetooth_enabled.unwrap_or(false);

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-bluetooth-detail",
            &chrome_place(area, transparent()),
            |f| {
                f.column().with_opts(&LayoutOpts {
                        width: area.w,
                        height: area.h,
                        gap: 12.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    }).show(|f| {
                        // Header row: [< Back] Title  [↻ Scan] [On/Off]
                        f.row().with_opts(&LayoutOpts {
                                width: area.w,
                                height: 36.0,
                                cross: Align::Center,
                                gap: 8.0,
                                ..Default::default()
                            }).show(|f| {
                                let (back_resp, _) = f.row().with_opts(&LayoutOpts {
                                        width: 80.0,
                                        height: 36.0,
                                        corner_radii: optics_ui::CornerRadii::uniform(10.0),
                                        pad: 6.0,
                                        gap: 4.0,
                                        bg: hud.surface_recessed,
                                        cross: Align::Center,
                                        ..Default::default()
                                    }).id_label("tessera-bluetooth-back-btn", i18n.text(Message::Back)).show(|f| {
                                        f.icon(optics_ui::sys::optics_ui_icon_id::OPTICS_UI_ICON_CHEVRON_LEFT, 16.0);
                                        display_label(f, i18n.text(Message::Back), type_scale.body);
                                    });
                                if back_resp.clicked {
                                    self.show_page(Page::Main, out);
                                }

                                f.spacer(4.0);
                                display_label(f, i18n.text(Message::Bluetooth), type_scale.headline);

                                f.flex(1.0);
                                f.spacer(0.0);

                                // Refresh / Scan button
                                let (refresh_resp, _) = f.row().with_opts(&LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        corner_radii: optics_ui::CornerRadii::uniform(18.0),
                                        cross: Align::Center,
                                        bg: hud.surface_recessed,
                                        ..Default::default()
                                    }).id_label("tessera-bluetooth-refresh-btn", "Scan").show(|f| {
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                        f.icon(Glyph::RefreshCw.id(), 16.0);
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    });
                                if refresh_resp.clicked {
                                    out.system_actions.push(SystemAction::ScanBluetooth);
                                }

                                // Radio toggle
                                let (toggle_resp, _) = f.row().with_opts(&LayoutOpts {
                                        width: 54.0,
                                        height: 32.0,
                                        corner_radii: optics_ui::CornerRadii::uniform(16.0),
                                        cross: Align::Center,
                                        bg: if bt_active {
                                            hud.accent
                                        } else {
                                            hud.surface_recessed
                                        },
                                        ..Default::default()
                                    }).id_label("tessera-bluetooth-power-btn", "Power").show(|f| {
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                        let text_color = if bt_active {
                                            Color::rgba(255, 255, 255, 255)
                                        } else {
                                            hud.text_muted
                                        };
                                        f.set_theme(themes::hud(&hud).with_fg(text_color));
                                        display_label(
                                            f,
                                            if bt_active { "On" } else { "Off" },
                                            type_scale.caption,
                                        );
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    });
                                if toggle_resp.clicked {
                                    out.system_actions.push(SystemAction::SetBluetooth {
                                        enabled: !bt_active,
                                    });
                                }
                            });

                        // Body list
                        let list_h = (area.h - 48.0).max(1.0);
                        if !bt_active {
                            f.column().with_opts(&LayoutOpts {
                                    width: area.w,
                                    height: list_h,
                                    cross: Align::Center,
                                    ..Default::default()
                                }).show(|f| {
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    display_label(f, "Bluetooth is turned off", type_scale.body);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                });
                        } else if status.bluetooth_devices.is_empty() {
                            f.column().with_opts(&LayoutOpts {
                                    width: area.w,
                                    height: list_h,
                                    cross: Align::Center,
                                    ..Default::default()
                                }).show(|f| {
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    if status.bluetooth_state
                                        == tessera_desktop::system::BluetoothLinkState::Scanning
                                    {
                                        display_label(
                                            f,
                                            "Scanning for devices…",
                                            type_scale.body,
                                        );
                                    } else {
                                        display_label(f, "No devices found", type_scale.body);
                                    }
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                });
                        } else {
                            f.scroll("tessera-bluetooth-devices-scroll", |f| {
                                f.column().with_opts(&LayoutOpts {
                                        width: area.w,
                                        gap: 8.0,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    }).show(|f| {
                                        for (idx, device) in
                                            status.bluetooth_devices.iter().enumerate()
                                        {
                                            let (connect_resp, _) = f.row().with_opts(&LayoutOpts {
                                                    width: area.w,
                                                    height: 48.0,
                                                    pad: 8.0,
                                                    corner_radii: optics_ui::CornerRadii::uniform(12.0),
                                                    bg: hud.surface_recessed,
                                                    cross: Align::Center,
                                                    gap: 8.0,
                                                    ..Default::default()
                                                }).id_label(&format!("tessera-bluetooth-device-{idx}"), &device.name).show(|f| {
                                                    f.set_theme(
                                                        themes::hud(&hud).with_fg(if device.connected {
                                                            hud.accent
                                                        } else {
                                                            hud.text
                                                        }),
                                                    );
                                                    f.icon(Glyph::Bluetooth.id(), 18.0);
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                    display_label(f, &device.name, type_scale.body);
                                                    f.flex(1.0);
                                                    f.spacer(0.0);
                                                    if device.connected {
                                                        let (d, _) = f.row().with_opts(&LayoutOpts {
                                                                width: 84.0,
                                                                height: 28.0,
                                                                corner_radii: optics_ui::CornerRadii::uniform(8.0),
                                                                bg: hud.surface,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            }).id_label(&format!("tessera-bluetooth-disconnect-{idx}"), "Disconnect").show(|f| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                display_label(
                                                                    f,
                                                                    "Disconnect",
                                                                    type_scale.footnote,
                                                                );
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            });
                                                        if d.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::DisconnectBluetooth {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    } else if !device.paired {
                                                        let (p, _) = f.row().with_opts(&LayoutOpts {
                                                                width: 56.0,
                                                                height: 28.0,
                                                                corner_radii: optics_ui::CornerRadii::uniform(8.0),
                                                                bg: hud.surface,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            }).id_label(&format!("tessera-bluetooth-pair-{idx}"), "Pair").show(|f| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                display_label(f, "Pair", type_scale.footnote);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            });
                                                        if p.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::PairBluetooth {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                    if device.paired {
                                                        let (b, _) = f.row().with_opts(&LayoutOpts {
                                                                width: 60.0,
                                                                height: 28.0,
                                                                corner_radii: optics_ui::CornerRadii::uniform(8.0),
                                                                bg: hud.surface,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            }).id_label(&format!("tessera-bluetooth-forget-{idx}"), "Forget").show(|f| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(
                                                                    themes::hud(&hud)
                                                                        .with_fg(hud.text_muted),
                                                                );
                                                                display_label(f, "Forget", type_scale.footnote);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            });
                                                        if b.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::ForgetBluetooth {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                });
                                            if connect_resp.clicked {
                                                out.system_actions.push(
                                                    SystemAction::ConnectBluetooth {
                                                        address: device.address.clone(),
                                                    },
                                                );
                                            }
                                        }
                                    });
                            });
                        }
                    });
            },
        );
        f.set_theme(original);
    }
}
