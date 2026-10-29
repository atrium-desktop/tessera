//! The quick-controls section: toggles and status tiles.

use super::*;

impl ControlCenter {
    /// The first tab: a compact Control Center grid. Four direct-action
    /// tiles occupy a 2×2 cluster while sound and brightness become thick
    /// vertical faders. This keeps glanceable state and manipulation close
    /// together without falling back to a settings-form row stack.
    pub(crate) fn render_quick_controls_section(
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
        if self.bluetooth_expanded {
            self.render_bluetooth_detail_view(f, area, i18n, out);
            return;
        }

        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();
        let tile_gap = 10.0;
        let tile_w = ((area.w - tile_gap) * 0.5).max(1.0);
        let tile_h = 58.0;
        let bento_h = tile_h * 2.0 + tile_gap;
        // Fader card height, derived from the fader's own content: a 24px title
        // row, an 8px gap, and a 20px trough/selector = 52px, plus 10px padding
        // top and bottom = 72px. The grid is three tile rows and three
        // fader/tier rows, so it measures 3*58 + 3*72 + 5*10 = 440px. The old
        // uniform `row_height(58)` clamped the taller fader rows to 58px and
        // silently pushed the trough into the card's bottom rim.
        let fader_h = 72.0;

        let wifi_active = status.wifi_enabled.unwrap_or(false);
        let wifi_sub =
            status
                .wifi_ssid
                .as_deref()
                .unwrap_or(if wifi_active { "On" } else { "Off" });

        let bt_active = status.bluetooth_enabled.unwrap_or(false);
        let bt_sub = Self::bluetooth_tile_subtitle(&status, i18n);

        let dnd_active = status.do_not_disturb;
        let dnd_sub = if dnd_active { "On" } else { "Off" };

        let dark_active = !self.design.is_light();
        let dark_sub = if dark_active { "Dark" } else { "Light" };

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-quick-controls-grid",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        f.grid(4)
                            .width(area.w)
                            .col_gap(tile_gap)
                            .row_gap(tile_gap)
                            .show(|f| {
                                // Wi-Fi (2 cols x 1 row)
                                f.col_span(2);
                                let (wifi_toggle, wifi_expand) =
                                    render_expandable_quick_toggle_tile(
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
                                    self.bluetooth_expanded = false;
                                    out.system_actions.push(SystemAction::ScanWifi);
                                }

                                // MPRIS Now Playing (2 cols x 2 rows, ADR-0104 / ADR-0169)
                                f.col_span(2);
                                f.row_span(2);
                                self.render_bento_media_card(
                                    f,
                                    (tile_w, bento_h),
                                    hud,
                                    type_scale,
                                    i18n,
                                );

                                // Bluetooth (2 cols x 1 row, auto-placed under Wi-Fi in the 4-column grid)
                                f.col_span(2);
                                let (bt_toggle, bt_expand) = render_expandable_quick_toggle_tile(
                                    f,
                                    "tessera-hud-quick-bluetooth",
                                    i18n.text(Message::Bluetooth),
                                    &bt_sub,
                                    lens::sys::lens_icon_id::LENS_ICON_BLUETOOTH,
                                    bt_active,
                                    (tile_w, tile_h),
                                    hud,
                                    type_scale,
                                );
                                if bt_toggle {
                                    out.system_actions.push(SystemAction::SetBluetooth {
                                        enabled: !bt_active,
                                    });
                                }
                                if bt_expand {
                                    self.bluetooth_expanded = true;
                                    out.system_actions.push(SystemAction::ScanBluetooth);
                                }

                                // Do Not Disturb (2 cols x 1 row)
                                f.col_span(2);
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

                                // Dark Mode (2 cols x 1 row)
                                f.col_span(2);
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

                                // Cast status (2 cols x 1 row, ADR-0188). A
                                // read-only status tile: it reports live
                                // extended displays and casting streams but
                                // offers no control until a casting service
                                // exists. Discovery and pairing are out of
                                // process and not yet built, so a toggle here
                                // would be a dead button.
                                f.col_span(2);
                                let cast_active =
                                    status.cast_streams > 0 || status.virtual_displays > 0;
                                let cast_sub = Self::cast_tile_subtitle(&status, i18n);
                                render_status_tile(
                                    f,
                                    i18n.text(Message::Cast),
                                    &cast_sub,
                                    lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                    cast_active,
                                    (tile_w, tile_h),
                                    hud,
                                    type_scale,
                                );

                                // Display Brightness (4 cols x 1 row)
                                f.col_span(4);
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
                                    out.system_actions
                                        .push(SystemAction::SetBrightness { level });
                                }

                                // Sound Volume (4 cols x 1 row)
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
                                f.col_span(4);
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

                                // Keyboard Backlight (4 cols x 1 row, conditional ADR-0168).
                                // Presentation follows the hardware's real
                                // granularity (ADR-0168 amendment): a stepped
                                // backlight gets a tiered segmented control, a
                                // fine-grained dimmer gets a fader, and the row
                                // is omitted entirely when there is no
                                // backlight.
                                if let Some(kbd_val) = status.kbd_brightness {
                                    f.col_span(4);
                                    if let Some(tiers) = status.kbd_brightness_tiers() {
                                        let tiers: Vec<Tier> = tiers
                                            .iter()
                                            .map(|level| Tier::new(format!("{level}%"), *level))
                                            .collect();
                                        let active = self
                                            .kbd_backlight_preview
                                            .unwrap_or_else(|| status.kbd_brightness_tier_index());
                                        let outcome = render_kbd_backlight_tiers(
                                            f,
                                            "tessera-hud-quick-kbd-brightness",
                                            i18n.text(Message::Keyboard),
                                            &tiers,
                                            active,
                                            &self.kbd_backlight_indicator,
                                            (area.w, fader_h),
                                            hud,
                                            type_scale,
                                        );
                                        // A held press scrubs the highlight across
                                        // rungs as the pointer moves; releasing
                                        // off the control cancels the preview
                                        // (like a slider released off its track).
                                        if outcome.dragging {
                                            self.kbd_backlight_preview = outcome.hovered;
                                        } else {
                                            self.kbd_backlight_preview = None;
                                        }
                                        if let Some(index) = outcome.clicked
                                            && let Some(level) = tiers.get(index).map(|t| t.value)
                                        {
                                            out.system_actions.push(
                                                SystemAction::SetKeyboardBrightness { level },
                                            );
                                        }
                                    } else {
                                        let (kbd_level, _) = render_horizontal_fader(
                                            f,
                                            "tessera-hud-quick-kbd-brightness",
                                            i18n.text(Message::Keyboard),
                                            kbd_backlight_icon()
                                                .unwrap_or(lens::sys::lens_icon_id::LENS_ICON_KEY),
                                            false,
                                            Some(kbd_val),
                                            (0, 100),
                                            (area.w, fader_h),
                                            hud.text,
                                            hud,
                                            type_scale,
                                        );
                                        if let Some(level) = kbd_level {
                                            out.system_actions.push(
                                                SystemAction::SetKeyboardBrightness { level },
                                            );
                                        }
                                    }
                                }
                            });
                    },
                );
            },
        );
        f.set_theme(original);
    }
}
