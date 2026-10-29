//! The work-mode panel and its behavior hints.

use super::*;

impl ControlCenter {
    /// The first of the three right-bottom components: a stable segmented
    /// mode selector. Its visible segments and interaction targets share
    /// the same absolute geometry, preventing the indicator and labels from
    /// drifting into different layout flows.
    pub(crate) fn render_work_mode_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if rect.w < 120.0 || rect.h < 32.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let current_mode = self.status.power_mode;
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        const INSET: f32 = 4.0;
        let control = Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };
        let inner = Rect {
            x: control.x + INSET,
            y: control.y + INSET,
            w: (control.w - INSET * 2.0).max(1.0),
            h: (control.h - INSET * 2.0).max(1.0),
        };
        let seg_w = inner.w / PowerMode::ALL.len() as f32;

        f.set_theme(base_theme);
        f.place(
            "tessera-hud-work-mode-surface",
            &chrome_place(
                control,
                LayoutOpts {
                    radius: control.h * 0.5,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(&sized(control.w, control.h), |_| {});
            },
        );

        // Sliding indicator: position from the spring (segment units), so
        // it overshoots slightly and settles with elastic bounce.
        let spring_pos = self
            .work_mode_indicator
            .spring
            .value
            .clamp(0.0, PowerMode::ALL.len().saturating_sub(1) as f32);
        let indicator_x = inner.x + spring_pos * seg_w;
        f.place(
            "tessera-hud-work-mode-indicator",
            &chrome_place(
                Rect {
                    x: indicator_x,
                    y: inner.y,
                    w: seg_w,
                    h: inner.h,
                },
                LayoutOpts {
                    radius: inner.h * 0.5,
                    bg: hud.selection_surface,
                    border: hud.accent.with_alpha(80),
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: seg_w,
                        height: inner.h,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        // Segment hit areas over the indicator.
        self.work_mode_hover = None;
        for (index, mode) in PowerMode::ALL.iter().enumerate() {
            let active = *mode == current_mode;
            let label = match mode {
                PowerMode::Balanced => i18n.text(Message::PowerModeBalanced),
                PowerMode::Awake => i18n.text(Message::PowerModeAwake),
                PowerMode::Secure => i18n.text(Message::PowerModeSecure),
            };
            let hint = power_mode_hint(*mode);
            let seg_rect = Rect {
                x: inner.x + index as f32 * seg_w,
                y: inner.y,
                w: seg_w,
                h: inner.h,
            };
            let hovered = contains(seg_rect, self.cursor_hint.0, self.cursor_hint.1);
            if hovered {
                self.work_mode_hover = Some(index);
            }
            let fg = if active { hud.accent } else { hud.text_muted };
            let seg_theme = themes::hud(&hud).with_fg(fg);
            f.set_theme(seg_theme);
            let mut clicked = false;
            f.place(
                &format!("tessera-hud-work-seg-place-{index}"),
                &chrome_place(seg_rect, transparent()),
                |f| {
                    let (response, _) = f.pressable_row(
                        &format!("tessera-hud-work-seg-{index}"),
                        label,
                        &LayoutOpts {
                            width: seg_rect.w,
                            height: seg_rect.h,
                            cross: Align::Center,
                            radius: inner.h * 0.5,
                            bg: Color::TRANSPARENT,
                            ..Default::default()
                        },
                        |f, _| {
                            f.centered(seg_rect.w, seg_rect.h, |f| {
                                display_label(
                                    f,
                                    &truncate(label, (seg_rect.w / 7.0).max(3.0) as usize),
                                    type_scale.footnote,
                                );
                            });
                        },
                    );
                    clicked = response.clicked;
                },
            );
            if clicked && !active {
                out.system_actions
                    .push(SystemAction::SetPowerMode { mode: *mode });
            }
            // Segment tooltip on hover.
            if hovered && self.work_mode_tooltip_reveal > 0.02 {
                render_tooltip(
                    f,
                    &format!("tessera-hud-work-seg-tip-{index}"),
                    Rect {
                        x: seg_rect.x,
                        y: control.y - 32.0,
                        w: seg_w.max(120.0),
                        h: 26.0,
                    },
                    hint,
                    hud,
                    type_scale,
                    self.work_mode_tooltip_reveal,
                );
            }
        }

        f.set_theme(original);
    }
}

/// The one-line behavior hint under each work-mode segment.
fn power_mode_hint(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Balanced => "Dim → Lock → Blank → Suspend",
        PowerMode::Awake => "Screen stays lit & unlocked",
        PowerMode::Secure => "Lock on idle, screen stays lit",
    }
}
