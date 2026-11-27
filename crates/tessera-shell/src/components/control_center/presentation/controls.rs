//! The controls card: header, toggles, faders, now playing, work mode, and
//! the tray row — or, below the header, one detail page.

use super::*;
use tessera_design::icons::Glyph;

impl ControlCenter {
    pub(crate) fn render_controls_card(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        cursor: (f32, f32),
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let design = self.design;
        let body = LayoutOpts {
            corner_radii: optics_ui::CornerRadii::uniform(design.radii.panel_lg),
            ..materials::canvas_panel(&hud)
        };
        materials::place_floating(
            f,
            "control-center-controls",
            SurfaceRole::FloatingPanel,
            &design,
            rect,
            1.0,
            body,
            |f| {
                f.column().with_opts(&sized(rect.w, rect.h)).show(|_| {});
            },
        );

        let inner = Rect {
            x: rect.x + layout::CARD_PAD,
            y: rect.y + layout::CARD_PAD,
            w: (rect.w - layout::CARD_PAD * 2.0).max(1.0),
            h: (rect.h - layout::CARD_PAD * 2.0).max(1.0),
        };
        let mut y = inner.y;
        self.render_header(
            f,
            Rect {
                h: HEADER_H,
                ..inner
            },
            i18n,
            out,
        );
        if !self.open {
            // A header action (Settings, Lock) closed the sheet this frame.
            return;
        }
        y += HEADER_H + layout::SECTION_GAP;
        if self.session_open {
            self.render_session_strip(
                f,
                Rect {
                    y,
                    h: SESSION_STRIP_H,
                    ..inner
                },
                i18n,
                out,
            );
            y += SESSION_STRIP_H + layout::SECTION_GAP;
        }
        let body = Rect {
            y,
            h: (inner.y + inner.h - y).max(1.0),
            ..inner
        };
        match self.page {
            Page::Main => self.render_main_page(f, body, cursor, i18n, out),
            Page::Wifi => self.render_wifi_detail_view(f, body, i18n, out),
            Page::Bluetooth => self.render_bluetooth_detail_view(f, body, i18n, out),
            Page::Sound => self.render_audio_detail_view(f, body, i18n, out),
            Page::Cast => self.render_cast_detail_view(f, body, i18n, out),
        }
    }

    /// The main page. Toggles, faders, and now playing scroll when a short
    /// output caps the card below its natural height, so no control is ever
    /// clipped out of reach. The work-mode switch and the tray row are pinned
    /// to the card's bottom edge, outside the scroll: the switch stays visible
    /// on a short laptop output, and tray cells and popover anchors never move
    /// under the pointer.
    fn render_main_page(
        &mut self,
        f: &mut Frame,
        area: Rect,
        cursor: (f32, f32),
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let cells = self.sni_cells();
        let (content, work_mode, tray) = split_main_footer(area, !cells.is_empty());
        let original = f.theme();
        f.set_theme(themes::hud(&hud));
        // A capped card scrolls; its column then leaves a gutter so the
        // scrollbar never sits on top of a tile's disclosure.
        let scrolls = self.controls_natural_height() > self.sheet.controls.h + 0.5;
        let column_w = if scrolls {
            (content.w - SCROLL_GUTTER).max(1.0)
        } else {
            content.w
        };
        f.place(
            "control-center-main",
            &chrome_place(content, transparent()),
            |f| {
                f.column()
                    .with_opts(&sized(content.w, content.h))
                    .show(|f| {
                        f.flex(1.0);
                        f.scroll("control-center-main-scroll", |f| {
                            f.column()
                                .with_opts(&LayoutOpts {
                                    width: column_w,
                                    gap: layout::SECTION_GAP,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                })
                                .show(|f| {
                                    self.render_toggle_grid(f, column_w, i18n, out);
                                    self.render_faders(f, column_w, hud, type_scale, i18n, out);
                                    if self.media_visible() {
                                        self.render_bento_media_card(
                                            f,
                                            (column_w, MEDIA_H),
                                            hud,
                                            type_scale,
                                            i18n,
                                        );
                                    }
                                });
                        });
                    });
            },
        );
        f.place(
            "control-center-work-mode",
            &chrome_place(work_mode, transparent()),
            |f| self.render_work_mode(f, work_mode.w, i18n, out),
        );
        f.set_theme(original);
        match tray {
            Some(anchor) => self.render_tray_shelf_cells(f, anchor, &cells, 1.0, cursor, i18n),
            None => {
                self.tray_shelf = None;
                self.tray_page = 0;
            }
        }
    }

    /// Wi-Fi and Bluetooth (half width, with a disclosure to their page), Cast
    /// (half width), Do Not Disturb and the color scheme (a quarter each).
    fn render_toggle_grid(
        &mut self,
        f: &mut Frame,
        width: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let status = self.status.clone();
        let half = ((width - TILE_GAP) * 0.5).max(1.0);
        let quarter = ((width - TILE_GAP * 3.0) * 0.25).max(1.0);

        let wifi_on = status.wifi_enabled.unwrap_or(false) || status.wifi_ssid.is_some();
        let wifi_sub = match status.wifi_ssid.as_deref() {
            Some(ssid) => ssid.to_owned(),
            None => i18n
                .text(if wifi_on { Message::On } else { Message::Off })
                .to_owned(),
        };
        let bt_on = status.bluetooth_enabled.unwrap_or(false);
        let bt_sub = Self::bluetooth_tile_subtitle(&status, i18n);
        let cast_on =
            status.cast_streams > 0 || status.virtual_displays > 0 || status.cast.is_casting();
        let cast_sub = Self::cast_tile_subtitle(&status, i18n);
        let dnd_on = status.do_not_disturb;
        let dark_on = !self.design.is_light();

        let mut next_page: Option<Page> = None;
        let mut hovered: Option<(Tip, Rect)> = None;
        f.grid(4)
            .width(width)
            .col_gap(TILE_GAP)
            .row_gap(TILE_GAP)
            .show(|f| {
                f.col_span(2);
                let (toggle, expand) = render_expandable_quick_toggle_tile(
                    f,
                    "control-center-wifi",
                    i18n.text(Message::Wifi),
                    &wifi_sub,
                    Glyph::Wifi.id(),
                    wifi_on,
                    (half, TILE_H),
                    hud,
                    type_scale,
                );
                if toggle {
                    out.system_actions
                        .push(SystemAction::SetWifi { enabled: !wifi_on });
                }
                if expand {
                    next_page = Some(Page::Wifi);
                }

                f.col_span(2);
                let (toggle, expand) = render_expandable_quick_toggle_tile(
                    f,
                    "control-center-bluetooth",
                    i18n.text(Message::Bluetooth),
                    &bt_sub,
                    Glyph::Bluetooth.id(),
                    bt_on,
                    (half, TILE_H),
                    hud,
                    type_scale,
                );
                if toggle {
                    out.system_actions
                        .push(SystemAction::SetBluetooth { enabled: !bt_on });
                }
                if expand {
                    next_page = Some(Page::Bluetooth);
                }

                // Casting has no meaningful on/off of its own: both halves of
                // the tile open the receiver list.
                f.col_span(2);
                let (toggle, expand) = render_expandable_quick_toggle_tile(
                    f,
                    "control-center-cast",
                    i18n.text(Message::Cast),
                    &cast_sub,
                    Glyph::Cast.id(),
                    cast_on,
                    (half, TILE_H),
                    hud,
                    type_scale,
                );
                if toggle || expand {
                    next_page = Some(Page::Cast);
                }

                f.col_span(1);
                let dnd = render_compact_quick_toggle_tile(
                    f,
                    "control-center-dnd",
                    i18n.text(Message::DoNotDisturb),
                    None,
                    if dnd_on {
                        Glyph::BellOff.id()
                    } else {
                        Glyph::Bell.id()
                    },
                    dnd_on,
                    (quarter, TILE_H),
                    hud,
                    type_scale,
                );
                if dnd.clicked {
                    out.system_actions
                        .push(SystemAction::SetDoNotDisturb { enabled: !dnd_on });
                }
                if dnd.hovered {
                    hovered = Some((Tip::DoNotDisturb, dnd.rect));
                }

                f.col_span(1);
                let dark = render_compact_quick_toggle_tile(
                    f,
                    "control-center-dark-mode",
                    i18n.text(Message::DarkMode),
                    None,
                    if dark_on {
                        Glyph::Moon.id()
                    } else {
                        Glyph::Sun.id()
                    },
                    dark_on,
                    (quarter, TILE_H),
                    hud,
                    type_scale,
                );
                if dark.clicked
                    && let Some(action) = self.color_scheme_toggle(dark_on)
                {
                    out.settings_actions.push((None, action));
                }
                if dark.hovered {
                    hovered = Some((Tip::DarkMode, dark.rect));
                }
            });
        if let Some((tip, anchor)) = hovered {
            self.hover_tip(tip, anchor);
        }
        if let Some(page) = next_page {
            self.show_page(page, out);
        }
    }

    /// The color-scheme flip as a complete preferences transaction, or `None`
    /// before the first settings snapshot (flipping from defaults would reset
    /// the user's other preferences).
    pub(crate) fn color_scheme_toggle(
        &self,
        dark_on: bool,
    ) -> Option<tessera_desktop::settings::SettingsAction> {
        let mut preferences = self.preferences.clone()?;
        preferences.color_scheme = if dark_on {
            tessera_desktop::settings::ColorScheme::Light
        } else {
            tessera_desktop::settings::ColorScheme::Dark
        };
        Some(tessera_desktop::settings::SettingsAction::SetDesktopPreferences { preferences })
    }

    /// Sound, brightness, microphone, and keyboard backlight. A row exists only
    /// when its hardware or service answered: a fader pinned at zero would
    /// claim the machine lacks a device it has (ADR-0216).
    fn render_faders(
        &mut self,
        f: &mut Frame,
        width: f32,
        hud: ControlCenterColors,
        type_scale: TypeScale,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let status = self.status.clone();
        let disclosure = Self::audio_output_disclosure_visible(&status);
        let mut open_sound = false;
        f.column()
            .with_opts(&LayoutOpts {
                width,
                gap: TILE_GAP,
                cross: Align::Stretch,
                ..Default::default()
            })
            .show(|f| {
                if status.volume.is_some() {
                    let sound = render_horizontal_fader(
                        f,
                        "control-center-volume",
                        i18n.text(if status.muted {
                            Message::Muted
                        } else {
                            Message::Sound
                        }),
                        volume_icon_raw(&status),
                        true,
                        status.volume,
                        (0, 100),
                        (width, FADER_H),
                        if status.muted {
                            hud.text_muted
                        } else {
                            hud.accent
                        },
                        hud,
                        type_scale,
                        disclosure.then_some(i18n.text(Message::Output)),
                    );
                    if sound.icon_clicked {
                        out.system_actions.push(SystemAction::ToggleMute);
                    }
                    if let Some(level) = sound.level {
                        out.system_actions.push(SystemAction::SetVolume { level });
                    }
                    open_sound |= sound.expand_clicked;
                }

                if status.brightness.is_some() {
                    let brightness = render_horizontal_fader(
                        f,
                        "control-center-brightness",
                        i18n.text(Message::Brightness),
                        Glyph::Sun.id(),
                        false,
                        status.brightness,
                        (1, 100),
                        (width, FADER_H),
                        hud.text,
                        hud,
                        type_scale,
                        None,
                    );
                    if let Some(level) = brightness.level {
                        out.system_actions
                            .push(SystemAction::SetBrightness { level });
                    }
                }

                // The capture fader is its own row with its own mute: silencing
                // a microphone is a privacy action, never one misclick from
                // silencing playback (ADR-0216).
                if self.microphone_visible() {
                    let label = if status.microphone_active {
                        i18n.text(Message::MicrophoneInUse)
                    } else {
                        status
                            .default_audio_input()
                            .map(|input| input.description.as_str())
                            .unwrap_or_else(|| i18n.text(Message::Microphone))
                    };
                    let mic = render_horizontal_fader(
                        f,
                        "control-center-microphone",
                        label,
                        microphone_icon_raw(&status),
                        true,
                        status.microphone_volume,
                        (0, 100),
                        (width, FADER_H),
                        if status.microphone_muted {
                            hud.text_muted
                        } else {
                            hud.accent
                        },
                        hud,
                        type_scale,
                        disclosure.then_some(i18n.text(Message::Inputs)),
                    );
                    if mic.icon_clicked {
                        out.system_actions.push(SystemAction::ToggleMicrophoneMute);
                    }
                    if let Some(level) = mic.level {
                        out.system_actions
                            .push(SystemAction::SetMicrophoneVolume { level });
                    }
                    open_sound |= mic.expand_clicked;
                }

                // The keyboard backlight follows the hardware's granularity
                // (ADR-0168): stepped hardware gets tiers, a fine dimmer a
                // fader.
                if let Some(level) = status.kbd_brightness {
                    if let Some(tiers) = status.kbd_brightness_tiers() {
                        let tiers: Vec<Tier> = tiers
                            .iter()
                            .map(|level| Tier::new(format!("{level}%"), *level))
                            .collect();
                        let outcome = render_kbd_backlight_tiers(
                            f,
                            "control-center-kbd-backlight",
                            i18n.text(Message::Keyboard),
                            &tiers,
                            self.kbd_backlight_index(),
                            &self.kbd_backlight_indicator,
                            (width, FADER_H),
                            hud,
                            type_scale,
                        );
                        // A held press scrubs the highlight; releasing off the
                        // control cancels the preview.
                        self.kbd_backlight_preview = if outcome.dragging {
                            outcome.hovered
                        } else {
                            None
                        };
                        if let Some(index) = outcome.clicked
                            && let Some(level) = tiers.get(index).map(|tier| tier.value)
                        {
                            out.system_actions
                                .push(SystemAction::SetKeyboardBrightness { level });
                        }
                    } else {
                        let kbd = render_horizontal_fader(
                            f,
                            "control-center-kbd-backlight",
                            i18n.text(Message::Keyboard),
                            kbd_backlight_icon().unwrap_or(Glyph::Key.id()),
                            false,
                            Some(level),
                            (0, 100),
                            (width, FADER_H),
                            hud.text,
                            hud,
                            type_scale,
                            None,
                        );
                        if let Some(level) = kbd.level {
                            out.system_actions
                                .push(SystemAction::SetKeyboardBrightness { level });
                        }
                    }
                }
            });
        if open_sound {
            self.show_page(Page::Sound, out);
        }
    }
}

/// Split the main page's area into the scrolling content, the work-mode
/// switch, and — when the tray has items — the tray row. Both footers stay
/// pinned to the bottom edge, so a capped card never scrolls either out of
/// view; only the toggles, faders, and now-playing card scroll.
pub(crate) fn split_main_footer(area: Rect, has_tray: bool) -> (Rect, Rect, Option<Rect>) {
    let (above_tray, tray) = split_tray_row(area, has_tray);
    let work_h = WORK_MODE_H.min(above_tray.h);
    let content_h = (above_tray.h - work_h - layout::SECTION_GAP).max(1.0);
    (
        Rect {
            h: content_h,
            ..above_tray
        },
        Rect {
            y: above_tray.y + above_tray.h - work_h,
            h: work_h,
            ..above_tray
        },
        tray,
    )
}

/// Split the main page's area into the scrolling content and, when the tray
/// has items, the tray row pinned to the bottom edge.
fn split_tray_row(area: Rect, has_tray: bool) -> (Rect, Option<Rect>) {
    if !has_tray {
        return (area, None);
    }
    let tray_h = TRAY_ROW_H.min(area.h);
    let content_h = (area.h - tray_h - layout::SECTION_GAP).max(1.0);
    (
        Rect {
            h: content_h,
            ..area
        },
        Some(Rect {
            y: area.y + area.h - tray_h,
            h: tray_h,
            ..area
        }),
    )
}
