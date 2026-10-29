//! The bento media card (MPRIS now-playing).

use super::*;

impl ControlCenter {
    /// Integrated MPRIS now-playing card inside the Quick Controls Bento Grid (ADR-0169).
    pub(crate) fn render_bento_media_card(
        &mut self,
        f: &mut Frame,
        size: (f32, f32),
        hud: ControlCenterColors,
        type_scale: TypeScale,
        i18n: &Localizer,
    ) {
        let snapshot = self
            .media
            .as_ref()
            .map(MediaHandle::snapshot)
            .unwrap_or_default();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let mut command = None;
        let card_pad = 12.0;
        let icon_w = 18.0;
        let icon_gap = 10.0;
        let text_max_w = (size.0 - card_pad * 2.0 - icon_w - icon_gap - 8.0).max(20.0);

        let identity_raw = if snapshot.available && !snapshot.identity.is_empty() {
            &snapshot.identity
        } else {
            i18n.text(Message::NowPlaying)
        };
        let identity_label = ellipsize(f, identity_raw, type_scale.footnote, text_max_w);

        let title_raw = if snapshot.available && !snapshot.title.is_empty() {
            &snapshot.title
        } else {
            i18n.text(Message::NotPlaying)
        };
        let title_label = ellipsize(f, title_raw, type_scale.body, text_max_w);

        let artist_label = ellipsize(f, &snapshot.artist, type_scale.footnote, text_max_w);

        f.set_theme(base_theme);
        f.column_ex(
            &LayoutOpts {
                width: size.0,
                height: size.1,
                gap: 4.0,
                pad: card_pad,
                radius: 16.0,
                bg: hud.surface_recessed,
                border: hud.border,
                border_width: 1.0,
                cross: Align::Stretch,
                ..Default::default()
            },
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        height: 48.0,
                        gap: 10.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.icon(
                            if snapshot.playing {
                                Icon::Pause
                            } else {
                                Icon::Play
                            },
                            18.0,
                        );
                        f.column_ex(
                            &LayoutOpts {
                                flex: 1.0,
                                max_width: text_max_w,
                                gap: 2.0,
                                ..Default::default()
                            },
                            |f| {
                                f.set_theme(muted_theme);
                                display_label(f, &identity_label, type_scale.footnote);
                                f.set_theme(base_theme);
                                display_label(f, &title_label, type_scale.body);
                                if snapshot.available && !snapshot.artist.is_empty() {
                                    f.set_theme(muted_theme);
                                    display_label(f, &artist_label, type_scale.footnote);
                                    f.set_theme(base_theme);
                                }
                            },
                        );
                    },
                );

                f.flex(1.0);
                f.spacer(0.0);

                f.row_ex(
                    &LayoutOpts {
                        height: 36.0,
                        gap: 6.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.set_theme(if snapshot.can_previous {
                            base_theme
                        } else {
                            muted_theme
                        });
                        let (previous, _) = f.pressable_row(
                            "tessera-hud-media-previous",
                            "Previous",
                            &LayoutOpts {
                                width: 36.0,
                                height: 36.0,
                                radius: 12.0,
                                cross: Align::Center,
                                bg: Color::TRANSPARENT,
                                ..Default::default()
                            },
                            |f, _| f.centered(36.0, 36.0, |f| f.icon(Icon::SkipBack, 17.0)),
                        );
                        if previous.clicked && snapshot.can_previous {
                            command = Some(MediaCommand::Previous);
                        }
                        f.set_theme(if snapshot.available {
                            base_theme
                        } else {
                            muted_theme
                        });
                        let (play_pause, _) = f.pressable_row(
                            "tessera-hud-media-play-pause",
                            "Play or pause",
                            &LayoutOpts {
                                width: 40.0,
                                height: 36.0,
                                radius: 12.0,
                                cross: Align::Center,
                                bg: hud.selection_surface,
                                ..Default::default()
                            },
                            |f, _| {
                                f.centered(40.0, 36.0, |f| {
                                    f.icon(
                                        if snapshot.playing {
                                            Icon::Pause
                                        } else {
                                            Icon::Play
                                        },
                                        18.0,
                                    )
                                })
                            },
                        );
                        if play_pause.clicked && snapshot.available {
                            command = Some(MediaCommand::PlayPause);
                        }
                        f.set_theme(if snapshot.can_next {
                            base_theme
                        } else {
                            muted_theme
                        });
                        let (next, _) = f.pressable_row(
                            "tessera-hud-media-next",
                            "Next",
                            &LayoutOpts {
                                width: 36.0,
                                height: 36.0,
                                radius: 12.0,
                                cross: Align::Center,
                                bg: Color::TRANSPARENT,
                                ..Default::default()
                            },
                            |f, _| f.centered(36.0, 36.0, |f| f.icon(Icon::SkipForward, 17.0)),
                        );
                        if next.clicked && snapshot.can_next {
                            command = Some(MediaCommand::Next);
                        }
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            },
        );
        if let Some(command) = command
            && let Some(media) = &self.media
        {
            media.send(command);
        }
    }
}
