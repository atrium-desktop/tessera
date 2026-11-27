//! The bento media card (MPRIS now-playing).

use super::*;
use tessera_design::icons::Glyph;

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
        f.column()
            .with_opts(&LayoutOpts {
                width: size.0,
                height: size.1,
                gap: 4.0,
                pad: card_pad,
                corner_radii: optics_ui::CornerRadii::uniform(16.0),
                bg: hud.surface_recessed,
                border: hud.border,
                border_width: 1.0,
                cross: Align::Stretch,
                ..Default::default()
            })
            .show(|f| {
                f.row()
                    .with_opts(&LayoutOpts {
                        height: 48.0,
                        gap: 10.0,
                        cross: Align::Center,
                        ..Default::default()
                    })
                    .show(|f| {
                        f.icon(
                            if snapshot.playing {
                                Glyph::Pause.id()
                            } else {
                                Glyph::Play.id()
                            },
                            18.0,
                        );
                        f.column()
                            .with_opts(&LayoutOpts {
                                flex: 1.0,
                                max_width: text_max_w,
                                gap: 2.0,
                                ..Default::default()
                            })
                            .show(|f| {
                                f.set_theme(muted_theme);
                                display_label(f, &identity_label, type_scale.footnote);
                                f.set_theme(base_theme);
                                display_label(f, &title_label, type_scale.body);
                                if snapshot.available && !snapshot.artist.is_empty() {
                                    f.set_theme(muted_theme);
                                    display_label(f, &artist_label, type_scale.footnote);
                                    f.set_theme(base_theme);
                                }
                            });
                    });

                f.flex(1.0);
                f.spacer(0.0);

                f.row()
                    .with_opts(&LayoutOpts {
                        height: 36.0,
                        gap: 6.0,
                        cross: Align::Center,
                        ..Default::default()
                    })
                    .show(|f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.set_theme(if snapshot.can_previous {
                            base_theme
                        } else {
                            muted_theme
                        });
                        let (previous, _) = f
                            .row()
                            .with_opts(&LayoutOpts {
                                width: 36.0,
                                height: 36.0,
                                corner_radii: optics_ui::CornerRadii::uniform(12.0),
                                cross: Align::Center,
                                bg: Color::TRANSPARENT,
                                ..Default::default()
                            })
                            .id_label("tessera-hud-media-previous", "Previous")
                            .show(|f| {
                                f.centered(36.0, 36.0, |f| f.icon(Glyph::SkipBack.id(), 17.0))
                            });
                        if previous.clicked && snapshot.can_previous {
                            command = Some(MediaCommand::Previous);
                        }
                        f.set_theme(if snapshot.available {
                            base_theme
                        } else {
                            muted_theme
                        });
                        let (play_pause, _) = f
                            .row()
                            .with_opts(&LayoutOpts {
                                width: 40.0,
                                height: 36.0,
                                corner_radii: optics_ui::CornerRadii::uniform(12.0),
                                cross: Align::Center,
                                bg: hud.selection_surface,
                                ..Default::default()
                            })
                            .id_label("tessera-hud-media-play-pause", "Play or pause")
                            .show(|f| {
                                f.centered(40.0, 36.0, |f| {
                                    f.icon(
                                        if snapshot.playing {
                                            Glyph::Pause.id()
                                        } else {
                                            Glyph::Play.id()
                                        },
                                        18.0,
                                    )
                                })
                            });
                        if play_pause.clicked && snapshot.available {
                            command = Some(MediaCommand::PlayPause);
                        }
                        f.set_theme(if snapshot.can_next {
                            base_theme
                        } else {
                            muted_theme
                        });
                        let (next, _) = f
                            .row()
                            .with_opts(&LayoutOpts {
                                width: 36.0,
                                height: 36.0,
                                corner_radii: optics_ui::CornerRadii::uniform(12.0),
                                cross: Align::Center,
                                bg: Color::TRANSPARENT,
                                ..Default::default()
                            })
                            .id_label("tessera-hud-media-next", "Next")
                            .show(|f| {
                                f.centered(36.0, 36.0, |f| f.icon(Glyph::SkipForward.id(), 17.0))
                            });
                        if next.clicked && snapshot.can_next {
                            command = Some(MediaCommand::Next);
                        }
                        f.flex(1.0);
                        f.spacer(0.0);
                    });
            });
        if let Some(command) = command
            && let Some(media) = &self.media
        {
            media.send(command);
        }
    }
}
