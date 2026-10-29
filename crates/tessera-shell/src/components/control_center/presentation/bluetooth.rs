//! Bluetooth detail page: adapter state, paired devices, and pairing controls.

use super::*;

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
                                    "tessera-bluetooth-back-btn",
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
                                    self.bluetooth_expanded = false;
                                }

                                f.spacer(4.0);
                                display_label(f, i18n.text(Message::Bluetooth), type_scale.headline);

                                f.flex(1.0);
                                f.spacer(0.0);

                                // Refresh / Scan button
                                let (refresh_resp, _) = f.pressable_row(
                                    "tessera-bluetooth-refresh-btn",
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
                                        f.icon_raw(
                                            lens::sys::lens_icon_id::LENS_ICON_REFRESH_CW,
                                            16.0,
                                        );
                                        f.flex(1.0);
                                        f.spacer(0.0);
                                    },
                                );
                                if refresh_resp.clicked {
                                    out.system_actions.push(SystemAction::ScanBluetooth);
                                }

                                // Radio toggle
                                let (toggle_resp, _) = f.pressable_row(
                                    "tessera-bluetooth-power-btn",
                                    "Power",
                                    &LayoutOpts {
                                        width: 54.0,
                                        height: 32.0,
                                        radius: 16.0,
                                        cross: Align::Center,
                                        bg: if bt_active {
                                            hud.accent
                                        } else {
                                            hud.surface_recessed
                                        },
                                        ..Default::default()
                                    },
                                    |f, _| {
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
                                    },
                                );
                                if toggle_resp.clicked {
                                    out.system_actions.push(SystemAction::SetBluetooth {
                                        enabled: !bt_active,
                                    });
                                }
                            },
                        );

                        // Body list
                        let list_h = (area.h - 48.0).max(1.0);
                        if !bt_active {
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
                                    display_label(f, "Bluetooth is turned off", type_scale.body);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                },
                            );
                        } else if status.bluetooth_devices.is_empty() {
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
                                },
                            );
                        } else {
                            f.scroll("tessera-bluetooth-devices-scroll", |f| {
                                f.column_ex(
                                    &LayoutOpts {
                                        width: area.w,
                                        gap: 8.0,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |f| {
                                        for (idx, device) in
                                            status.bluetooth_devices.iter().enumerate()
                                        {
                                            let (connect_resp, _) = f.pressable_row(
                                                &format!("tessera-bluetooth-device-{idx}"),
                                                &device.name,
                                                &LayoutOpts {
                                                    width: area.w,
                                                    height: 48.0,
                                                    pad: 8.0,
                                                    radius: 12.0,
                                                    bg: hud.surface_recessed,
                                                    cross: Align::Center,
                                                    gap: 8.0,
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
                                                        lens::sys::lens_icon_id::LENS_ICON_BLUETOOTH,
                                                        18.0,
                                                    );
                                                    f.set_theme(themes::hud(&hud).with_fg(hud.text));
                                                    display_label(f, &device.name, type_scale.body);
                                                    f.flex(1.0);
                                                    f.spacer(0.0);
                                                    if device.connected {
                                                        let (d, _) = f.pressable_row(
                                                            &format!("tessera-bluetooth-disconnect-{idx}"),
                                                            "Disconnect",
                                                            &LayoutOpts {
                                                                width: 84.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                bg: hud.surface,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                display_label(
                                                                    f,
                                                                    "Disconnect",
                                                                    type_scale.footnote,
                                                                );
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if d.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::DisconnectBluetooth {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    } else if !device.paired {
                                                        let (p, _) = f.pressable_row(
                                                            &format!("tessera-bluetooth-pair-{idx}"),
                                                            "Pair",
                                                            &LayoutOpts {
                                                                width: 56.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                bg: hud.surface,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                display_label(f, "Pair", type_scale.footnote);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if p.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::PairBluetooth {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                    if device.paired {
                                                        let (b, _) = f.pressable_row(
                                                            &format!("tessera-bluetooth-forget-{idx}"),
                                                            "Forget",
                                                            &LayoutOpts {
                                                                width: 60.0,
                                                                height: 28.0,
                                                                radius: 8.0,
                                                                bg: hud.surface,
                                                                cross: Align::Center,
                                                                ..Default::default()
                                                            },
                                                            |f, _| {
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                                f.set_theme(
                                                                    themes::hud(&hud)
                                                                        .with_fg(hud.text_muted),
                                                                );
                                                                display_label(f, "Forget", type_scale.footnote);
                                                                f.flex(1.0);
                                                                f.spacer(0.0);
                                                            },
                                                        );
                                                        if b.clicked {
                                                            out.system_actions.push(
                                                                SystemAction::ForgetBluetooth {
                                                                    address: device.address.clone(),
                                                                },
                                                            );
                                                        }
                                                    }
                                                },
                                            );
                                            if connect_resp.clicked {
                                                out.system_actions.push(
                                                    SystemAction::ConnectBluetooth {
                                                        address: device.address.clone(),
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
}
