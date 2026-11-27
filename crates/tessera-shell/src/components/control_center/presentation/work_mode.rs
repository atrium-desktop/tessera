//! The work-mode switch: how the session behaves when idle.

use super::*;

impl ControlCenter {
    /// A segmented control over the session power modes. The selection
    /// highlight follows the shared under-damped indicator spring: each
    /// segment's highlight strength is its closeness to the spring's position,
    /// so a switch cross-fades from the old segment to the new one with the
    /// same light settle every segmented control in the shell has.
    pub(crate) fn render_work_mode(
        &mut self,
        f: &mut Frame,
        width: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let current = self.status.power_mode;
        let position = self.work_mode_indicator.spring.value;
        let inset = 4.0;
        let segment_w = ((width - inset * 2.0) / PowerMode::ALL.len() as f32).max(1.0);
        let segment_h = WORK_MODE_H - inset * 2.0;
        let original = f.theme();
        let mut chosen: Option<PowerMode> = None;
        let mut hovered: Option<(Tip, Rect)> = None;
        f.row()
            .with_opts(&LayoutOpts {
                width,
                height: WORK_MODE_H,
                pad: inset,
                corner_radii: optics_ui::CornerRadii::uniform(WORK_MODE_H * 0.5),
                bg: hud.surface_recessed,
                cross: Align::Center,
                ..Default::default()
            })
            .show(|f| {
                for (index, mode) in PowerMode::ALL.iter().enumerate() {
                    let strength = (1.0 - (position - index as f32).abs()).clamp(0.0, 1.0);
                    let label = i18n.text(match mode {
                        PowerMode::Balanced => Message::PowerModeBalanced,
                        PowerMode::Awake => Message::PowerModeAwake,
                        PowerMode::Secure => Message::PowerModeSecure,
                    });
                    let fg = if *mode == current {
                        hud.accent
                    } else {
                        hud.text_muted
                    };
                    f.set_theme(themes::hud(&hud).with_fg(fg));
                    let (response, _) = f
                        .row()
                        .with_opts(&LayoutOpts {
                            width: segment_w,
                            height: segment_h,
                            corner_radii: optics_ui::CornerRadii::uniform(segment_h * 0.5),
                            cross: Align::Center,
                            bg: hud
                                .selection_surface
                                .with_alpha((strength * 255.0).round() as u8),
                            ..Default::default()
                        })
                        .id_label(&format!("control-center-work-mode-{index}"), label)
                        .show(|f| {
                            f.centered(segment_w, segment_h, |f| {
                                display_label(
                                    f,
                                    &truncate(label, (segment_w / 7.0).max(3.0) as usize),
                                    type_scale.footnote,
                                );
                            });
                        });
                    if response.clicked && *mode != current {
                        chosen = Some(*mode);
                    }
                    if response.hovered {
                        hovered = Some((Tip::WorkMode(index), response.rect));
                    }
                }
            });
        f.set_theme(original);
        if let Some((tip, anchor)) = hovered {
            self.hover_tip(tip, anchor);
        }
        if let Some(mode) = chosen {
            out.system_actions.push(SystemAction::SetPowerMode { mode });
        }
    }
}
