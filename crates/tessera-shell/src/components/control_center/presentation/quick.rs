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
        if self.audio_expanded {
            self.render_audio_detail_view(f, area, i18n, out);
            return;
        }
        if self.cast_expanded {
            self.render_cast_detail_view(f, area, i18n, out);
            return;
        }

        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();
        let tile_gap = 10.0;
        let tile_w = ((area.w - tile_gap) * 0.5).max(1.0);
        let col_w = ((area.w - 3.0 * tile_gap) / 4.0).max(1.0);
        let tile_h = 58.0;
        let bento_h = tile_h * 2.0 + tile_gap;
        // Fader card height, derived from the fader's own content: a 24px title
        // row, an 8px gap, and a 20px trough/selector = 52px, plus 10px padding
        // top and bottom = 72px. The grid is three tile rows and three
        // fader/tier rows, so it measures 3*58 + 3*72 + 5*10 = 440px. The old
        // uniform `row_height(58)` clamped the taller fader rows to 58px and
        // silently pushed the trough into the card's bottom rim.
        let fader_h = 72.0;

        let wifi_active = status.wifi_enabled.unwrap_or(false) || status.wifi_ssid.is_some();
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
                // The grid is taller than a short output's panel (MAIN_H is
                // clamped to the display), and rows are content-sized rather
                // than compressed: clipping the last fader would hide a control
                // with no way to reach it. Scrolling is what every other page
                // in this panel already does, so a short display loses nothing
                // but a gesture.
                f.scroll("tessera-hud-quick-controls-scroll", |f| {
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
                                        self.audio_expanded = false;
                                        self.cast_expanded = false;
                                        out.system_actions.push(SystemAction::StopCastDiscovery);
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
                                    let (bt_toggle, bt_expand) =
                                        render_expandable_quick_toggle_tile(
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
                                        self.wifi_expanded = false;
                                        self.audio_expanded = false;
                                        self.cast_expanded = false;
                                        out.system_actions.push(SystemAction::StopCastDiscovery);
                                        out.system_actions.push(SystemAction::ScanBluetooth);
                                    }

                                    // Cast status & interactive discovery (2 cols x 1 row, ADR-0188 / ADR-0192)
                                    f.col_span(2);
                                    let cast_active = status.cast_streams > 0
                                        || status.virtual_displays > 0
                                        || status.cast.is_casting();
                                    let cast_sub = Self::cast_tile_subtitle(&status, i18n);
                                    let (cast_toggle, cast_expand) =
                                        render_expandable_quick_toggle_tile(
                                            f,
                                            "tessera-hud-quick-cast",
                                            i18n.text(Message::Cast),
                                            &cast_sub,
                                            lens::sys::lens_icon_id::LENS_ICON_MONITOR,
                                            cast_active,
                                            (tile_w, tile_h),
                                            hud,
                                            type_scale,
                                        );
                                    if cast_toggle || cast_expand {
                                        self.cast_expanded = true;
                                        self.cast_scanning = true;
                                        self.wifi_expanded = false;
                                        self.bluetooth_expanded = false;
                                        self.audio_expanded = false;
                                        out.system_actions.push(SystemAction::ScanCast);
                                    }

                                    // Do Not Disturb (1 col x 1 row) - Modern compact quick toggle
                                    f.col_span(1);
                                    if render_compact_quick_toggle_tile(
                                        f,
                                        "tessera-hud-quick-dnd",
                                        i18n.text(Message::DoNotDisturb),
                                        Some(dnd_sub),
                                        lens::sys::lens_icon_id::LENS_ICON_BELL,
                                        dnd_active,
                                        (col_w, tile_h),
                                        hud,
                                        type_scale,
                                    ) {
                                        out.system_actions.push(SystemAction::SetDoNotDisturb {
                                            enabled: !dnd_active,
                                        });
                                    }

                                    // Dark Mode (1 col x 1 row) - Modern compact quick toggle
                                    f.col_span(1);
                                    if render_compact_quick_toggle_tile(
                                        f,
                                        "tessera-hud-quick-dark-mode",
                                        "Dark Mode",
                                        Some(dark_sub),
                                        if dark_active {
                                            lens::sys::lens_icon_id::LENS_ICON_MOON
                                        } else {
                                            lens::sys::lens_icon_id::LENS_ICON_SUN
                                        },
                                        dark_active,
                                        (col_w, tile_h),
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

                                    // Display Brightness (4 cols x 1 row)
                                    f.col_span(4);
                                    let brightness = render_horizontal_fader(
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
                                        None,
                                    );
                                    if let Some(level) = brightness.level {
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
                                    let sound = render_horizontal_fader(
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
                                        // A machine with a sound server gets the
                                        // disclosure; one with none gets nothing,
                                        // because the page would list nothing
                                        // (ADR-0215, ADR-0216).
                                        Self::audio_output_disclosure_visible(&status)
                                            .then_some(i18n.text(Message::Output)),
                                    );
                                    if sound.icon_clicked {
                                        out.system_actions.push(SystemAction::ToggleMute);
                                    }
                                    if let Some(level) = sound.level {
                                        out.system_actions.push(SystemAction::SetVolume { level });
                                    }
                                    if sound.expand_clicked {
                                        self.audio_expanded = true;
                                        self.wifi_expanded = false;
                                        self.bluetooth_expanded = false;
                                        self.cast_expanded = false;
                                        out.system_actions.push(SystemAction::StopCastDiscovery);
                                    }

                                    // Microphone (4 cols x 1 row, conditional
                                    // ADR-0216). The capture fader is its own row
                                    // with its own icon-mute, never merged into the
                                    // sound fader: silencing a microphone is a
                                    // privacy action, and a shared control would put
                                    // it one misclick from silencing playback.
                                    //
                                    // Omitted entirely when no sound server answered:
                                    // a fader pinned at zero would claim the machine
                                    // has no microphone.
                                    if status.microphone_volume.is_some()
                                        || !status.audio_inputs.is_empty()
                                    {
                                        f.col_span(4);
                                        // The label is the reading a user glances at:
                                        // the device in use, or the fact that
                                        // something is capturing right now — which
                                        // outranks the device name, because it is the
                                        // urgent half of the same fact.
                                        let mic_label = if status.microphone_active {
                                            i18n.text(Message::MicrophoneInUse)
                                        } else {
                                            status
                                                .default_audio_input()
                                                .map(|input| input.description.as_str())
                                                .unwrap_or_else(|| i18n.text(Message::Microphone))
                                        };
                                        let mic = render_horizontal_fader(
                                            f,
                                            "tessera-hud-quick-microphone",
                                            mic_label,
                                            microphone_icon_raw(&status),
                                            true,
                                            status.microphone_volume,
                                            (0, 100),
                                            (area.w, fader_h),
                                            if status.microphone_muted {
                                                hud.text_muted
                                            } else {
                                                hud.accent
                                            },
                                            hud,
                                            type_scale,
                                            // The disclosure is the only way into
                                            // the input picker, so it appears when
                                            // that picker has something to offer even
                                            // if the output list does not. Its name
                                            // is the input half's, because that is
                                            // what this fader is about.
                                            Self::audio_output_disclosure_visible(&status)
                                                .then_some(i18n.text(Message::Inputs)),
                                        );
                                        if mic.icon_clicked {
                                            out.system_actions
                                                .push(SystemAction::ToggleMicrophoneMute);
                                        }
                                        if let Some(level) = mic.level {
                                            out.system_actions
                                                .push(SystemAction::SetMicrophoneVolume { level });
                                        }
                                        if mic.expand_clicked {
                                            self.audio_expanded = true;
                                            self.wifi_expanded = false;
                                            self.bluetooth_expanded = false;
                                            self.cast_expanded = false;
                                            out.system_actions
                                                .push(SystemAction::StopCastDiscovery);
                                        }
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
                                            let active =
                                                self.kbd_backlight_preview.unwrap_or_else(|| {
                                                    status.kbd_brightness_tier_index()
                                                });
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
                                                && let Some(level) =
                                                    tiers.get(index).map(|t| t.value)
                                            {
                                                out.system_actions.push(
                                                    SystemAction::SetKeyboardBrightness { level },
                                                );
                                            }
                                        } else {
                                            let kbd = render_horizontal_fader(
                                                f,
                                                "tessera-hud-quick-kbd-brightness",
                                                i18n.text(Message::Keyboard),
                                                kbd_backlight_icon().unwrap_or(
                                                    lens::sys::lens_icon_id::LENS_ICON_KEY,
                                                ),
                                                false,
                                                Some(kbd_val),
                                                (0, 100),
                                                (area.w, fader_h),
                                                hud.text,
                                                hud,
                                                type_scale,
                                                None,
                                            );
                                            if let Some(level) = kbd.level {
                                                out.system_actions.push(
                                                    SystemAction::SetKeyboardBrightness { level },
                                                );
                                            }
                                        }
                                    }
                                });
                        },
                    );
                });
            },
        );
        f.set_theme(original);
    }
}
