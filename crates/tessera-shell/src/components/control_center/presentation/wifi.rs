//! Wi-Fi detail page: networks, connection, and credentials.

use super::*;

impl ControlCenter {
    /// Expanded Wi-Fi networks detail view (ADR-0162).
    pub(crate) fn render_wifi_detail_view(
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
        let wifi_active = status.wifi_enabled.unwrap_or(false) || status.wifi_ssid.is_some();

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
                                    i18n.text(Message::Back),
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
                                        display_label(f, i18n.text(Message::Back), type_scale.body);
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
                                            let mut row_action_consumed = false;
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

                                                    // Frequency band badge if known
                                                    if let Some(freq) = net.frequency_mhz {
                                                        f.spacer(4.0);
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.text_muted));
                                                        let band = if freq >= 5000 { "5 GHz" } else { "2.4 GHz" };
                                                        display_label(f, band, type_scale.caption);
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
                                                            row_action_consumed = true;
                                                            out.system_actions.push(SystemAction::DisconnectWifi);
                                                        }
                                                        if net.is_saved {
                                                            f.spacer(4.0);
                                                            let (forget_resp, _) = f.pressable_row(
                                                                &format!("tessera-wifi-forget-{idx}"),
                                                                "Forget",
                                                                &LayoutOpts {
                                                                    width: 58.0,
                                                                    height: 28.0,
                                                                    radius: 8.0,
                                                                    cross: Align::Center,
                                                                    bg: hud.surface,
                                                                    ..Default::default()
                                                                },
                                                                |f, _| {
                                                                    f.flex(1.0);
                                                                    f.spacer(0.0);
                                                                    display_label(f, "Forget", type_scale.footnote);
                                                                    f.flex(1.0);
                                                                    f.spacer(0.0);
                                                                },
                                                            );
                                                            if forget_resp.clicked {
                                                                row_action_consumed = true;
                                                                out.system_actions.push(SystemAction::ForgetWifi {
                                                                    ssid: net.ssid.clone(),
                                                                });
                                                            }
                                                        }
                                                    } else if is_connecting {
                                                        f.set_theme(themes::hud(&hud).with_fg(hud.accent));
                                                        display_label(f, "Connecting...", type_scale.footnote);
                                                    } else {
                                                        if net.is_saved {
                                                            let (auto_resp, _) = f.pressable_row(
                                                                &format!("tessera-wifi-autoconn-{idx}"),
                                                                "Auto",
                                                                &LayoutOpts {
                                                                    width: 44.0,
                                                                    height: 28.0,
                                                                    radius: 8.0,
                                                                    cross: Align::Center,
                                                                    bg: if net.auto_connect {
                                                                        hud.accent.with_alpha(50)
                                                                    } else {
                                                                        hud.surface
                                                                    },
                                                                    ..Default::default()
                                                                },
                                                                |f, _| {
                                                                    f.flex(1.0);
                                                                    f.spacer(0.0);
                                                                    f.set_theme(themes::hud(&hud).with_fg(if net.auto_connect {
                                                                        hud.accent
                                                                    } else {
                                                                        hud.text_muted
                                                                    }));
                                                                    display_label(f, "Auto", type_scale.caption);
                                                                    f.flex(1.0);
                                                                    f.spacer(0.0);
                                                                },
                                                            );
                                                            if auto_resp.clicked {
                                                                row_action_consumed = true;
                                                                out.system_actions.push(SystemAction::SetWifiAutoConnect {
                                                                    ssid: net.ssid.clone(),
                                                                    auto_connect: !net.auto_connect,
                                                                });
                                                            }
                                                            f.spacer(4.0);
                                                            let (forget_resp, _) = f.pressable_row(
                                                                &format!("tessera-wifi-forget-{idx}"),
                                                                "Forget",
                                                                &LayoutOpts {
                                                                    width: 58.0,
                                                                    height: 28.0,
                                                                    radius: 8.0,
                                                                    cross: Align::Center,
                                                                    bg: hud.surface,
                                                                    ..Default::default()
                                                                },
                                                                |f, _| {
                                                                    f.flex(1.0);
                                                                    f.spacer(0.0);
                                                                    display_label(f, "Forget", type_scale.footnote);
                                                                    f.flex(1.0);
                                                                    f.spacer(0.0);
                                                                },
                                                            );
                                                            if forget_resp.clicked {
                                                                row_action_consumed = true;
                                                                out.system_actions.push(SystemAction::ForgetWifi {
                                                                    ssid: net.ssid.clone(),
                                                                });
                                                            }
                                                            f.spacer(4.0);
                                                        }
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

                                            if row_resp.clicked && !row_action_consumed && !is_curr {
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
}
