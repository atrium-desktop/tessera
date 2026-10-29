//! The identity profile panel.

use super::*;

impl ControlCenter {
    /// The top-left profile block: user persona (gapped-ring avatar, display
    /// name, `@username · groups`, hostname) drawn frameless — no chip
    /// background or border — straight onto the solid canvas.
    /// Slides in from the top-left.
    pub(crate) fn render_profile_panel(
        &self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * -24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let pad = 14.0;
        let center_y = rect.y + rect.h * 0.5;
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let avatar_style = self.design.avatars.for_role(AvatarRole::PersonaHeader);
        let original = f.theme();

        // -- profile zone: 56px avatar with a gapped line ring + name lines --
        let avatar_size = 56.0;
        let avatar_center = (rect.x + pad + avatar_size * 0.5, center_y);
        // The ring floats clear of the avatar edge — the visible gap reads
        // as a deliberate stroke of the identity mark rather than a border.
        let ring_gap = 3.0;
        let ring_diameter = avatar_size + ring_gap * 2.0 + 2.0;
        render_ring(
            f,
            "tessera-hud-avatar-ring",
            avatar_center,
            ring_diameter,
            hud.accent,
            1.5,
        );
        render_disc(
            f,
            "tessera-hud-avatar-backdrop",
            avatar_center,
            avatar_size,
            avatar_style.fallback_surface,
        );
        let avatar_rect = Rect {
            x: avatar_center.0 - avatar_size * 0.5,
            y: avatar_center.1 - avatar_size * 0.5,
            w: avatar_size,
            h: avatar_size,
        };
        match &self.avatar {
            Some(avatar) => {
                let texture = avatar.texture().as_raw();
                f.place(
                    "tessera-hud-avatar",
                    &chrome_place(avatar_rect, transparent()),
                    |f| {
                        f.row_ex(&sized(avatar_size, avatar_size), |f| {
                            unsafe { f.image(texture, avatar_size, avatar_size) };
                        });
                    },
                );
            }
            None => {
                f.set_theme(base_theme.with_fg(avatar_style.fallback_foreground));
                f.place(
                    "tessera-hud-avatar-initials",
                    &chrome_place(avatar_rect, transparent()),
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                width: avatar_size,
                                height: avatar_size,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.flex(1.0);
                                f.spacer(0.0);
                                display_label(
                                    f,
                                    &self.profile.initials,
                                    avatar_rect.w * avatar_style.initials_scale,
                                );
                                f.flex(1.0);
                                f.spacer(0.0);
                            },
                        );
                    },
                );
            }
        }

        let text_x = rect.x + pad + ring_diameter + 14.0;
        let text_w = (rect.x + rect.w - pad - text_x).max(40.0);
        let display_name = truncate(&self.profile.display_name, (text_w / 9.5).max(4.0) as usize);
        f.set_theme(base_theme);
        f.place(
            "tessera-hud-profile-name",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: center_y - 26.0,
                    w: text_w,
                    h: 24.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 24.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| display_label(f, &display_name, type_scale.title),
                );
            },
        );
        let mut sub_line = format!("@{}", self.profile.username);
        if !self.profile.groups.is_empty() {
            sub_line.push_str(" · ");
            sub_line.push_str(&self.profile.groups.join(", "));
        }
        let sub_line = truncate(&sub_line, (text_w / 6.2).max(6.0) as usize);
        f.set_theme(muted_theme);
        f.place(
            "tessera-hud-profile-sub",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: center_y + 1.0,
                    w: text_w,
                    h: 18.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 18.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| display_label(f, &sub_line, type_scale.label),
                );
            },
        );
        // The hostname line answers "which machine": muted, sitting under
        // the account line so the block reads who-on-where.
        if !self.profile.hostname.is_empty() {
            let host_line = truncate(&self.profile.hostname, (text_w / 6.2).max(6.0) as usize);
            f.place(
                "tessera-hud-profile-host",
                &chrome_place(
                    Rect {
                        x: text_x,
                        y: center_y + 20.0,
                        w: text_w,
                        h: 16.0,
                    },
                    transparent(),
                ),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: text_w,
                            height: 16.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| display_label(f, &host_line, type_scale.footnote),
                    );
                },
            );
        }
        f.set_theme(original);
    }
}
