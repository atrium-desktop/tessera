use super::*;

/// A deferred row click captured during the menu frame's column closure.
pub(super) enum MenuRowAction {
    Back,
    Descend(i32),
    Click(i32),
}

pub(super) use crate::widgets::geom::contains;
pub(super) use crate::widgets::motion::ease_out_cubic;
pub(super) use crate::widgets::motion::stagger;
pub(super) use crate::widgets::shapes::render_disc;
pub(super) use crate::widgets::shapes::render_ring;

/// Two settings actions are the same kind when they mutate the same
/// settings section; the queue keeps only the newest draft of each kind
/// (instant modules emit one action per control change while dragging).
pub(super) fn same_action_kind(left: &SettingsAction, right: &SettingsAction) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

/// The HUD flourish: short L-shaped strokes (12px arms, 1.5px thick) just
/// inside the four corners of a panel rect, like a VR visor's frame
/// markers. The color arrives unfaded — the frame's context opacity fades
/// the brackets like everything else built under it.
#[allow(dead_code)]
pub(super) fn render_corner_brackets(frame: &mut Frame, id: &str, rect: Rect, color: Color) {
    const ARM: f32 = 12.0;
    const THICK: f32 = 1.5;
    const INSET: f32 = 6.0;
    let mut bar = |suffix: &str, x: f32, y: f32, w: f32, h: f32| {
        frame.place(
            &format!("{id}-{suffix}"),
            &materials::chrome_place(
                Rect { x, y, w, h },
                LayoutOpts {
                    bg: color,
                    border: Color::TRANSPARENT,
                    radius: 0.0,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    };
    let left = rect.x + INSET;
    let right = rect.x + rect.w - INSET;
    let top = rect.y + INSET;
    let bottom = rect.y + rect.h - INSET;
    bar("tl-h", left, top, ARM, THICK);
    bar("tl-v", left, top, THICK, ARM);
    bar("tr-h", right - ARM, top, ARM, THICK);
    bar("tr-v", right - THICK, top, THICK, ARM);
    bar("bl-h", left, bottom - THICK, ARM, THICK);
    bar("bl-v", left, bottom - THICK, THICK, ARM);
    bar("br-h", right - ARM, bottom - THICK, ARM, THICK);
    bar("br-v", right - THICK, bottom - THICK, THICK, ARM);
}

pub(super) fn volume_icon_raw(status: &SystemStatus) -> lens::sys::lens_icon_id {
    if status.muted || status.volume.unwrap_or(0) == 0 {
        lens::sys::lens_icon_id::LENS_ICON_VOLUME_X
    } else if status.volume.unwrap_or(0) < 55 {
        lens::sys::lens_icon_id::LENS_ICON_VOLUME_1
    } else {
        lens::sys::lens_icon_id::LENS_ICON_VOLUME_2
    }
}

pub(super) fn render_quick_toggle_tile(
    f: &mut Frame,
    id: &str,
    title: &str,
    subtitle: &str,
    icon: lens::sys::lens_icon_id,
    active: bool,
    size: (f32, f32),
    hud: ControlCenterColors,
    type_scale: TypeScale,
) -> bool {
    let original = f.theme();
    let (bg, border, border_width, icon_bg, icon_fg, text_fg, sub_fg) = if active {
        (
            hud.selection_surface,
            hud.accent.with_alpha(80),
            1.0,
            hud.accent,
            Color::rgba(255, 255, 255, 255),
            hud.text,
            hud.accent,
        )
    } else {
        (
            hud.surface_recessed,
            hud.border,
            1.0,
            hud.surface,
            hud.text_muted,
            hud.text,
            hud.text_muted,
        )
    };

    f.set_theme(themes::hud(&hud).with_fg(text_fg));
    let (response, _) = f.pressable_row(
        id,
        title,
        &LayoutOpts {
            width: size.0,
            height: size.1,
            pad: 10.0,
            radius: 16.0,
            cross: Align::Center,
            gap: 10.0,
            bg,
            border,
            border_width,
            ..Default::default()
        },
        |f, _| {
            f.column_ex(
                &LayoutOpts {
                    width: 36.0,
                    height: 36.0,
                    radius: 18.0,
                    bg: icon_bg,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.set_theme(themes::hud(&hud).with_fg(icon_fg));
                    f.icon_raw(icon, 18.0);
                    f.flex(1.0);
                    f.spacer(0.0);
                },
            );

            f.column_ex(
                &LayoutOpts {
                    width: (size.0 - 20.0 - 36.0 - 10.0).max(1.0),
                    height: 36.0,
                    gap: 1.0,
                    cross: Align::Start,
                    ..Default::default()
                },
                |f| {
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.set_theme(themes::hud(&hud).with_fg(text_fg));
                    display_label(f, title, type_scale.body);
                    f.set_theme(themes::hud(&hud).with_fg(sub_fg));
                    display_label(f, subtitle, type_scale.footnote);
                    f.flex(1.0);
                    f.spacer(0.0);
                },
            );
        },
    );
    f.set_theme(original);
    response.clicked
}

pub(super) fn render_expandable_quick_toggle_tile(
    f: &mut Frame,
    id: &str,
    title: &str,
    subtitle: &str,
    icon: lens::sys::lens_icon_id,
    active: bool,
    size: (f32, f32),
    hud: ControlCenterColors,
    type_scale: TypeScale,
) -> (bool, bool) {
    let original = f.theme();
    let (bg, border, border_width, icon_bg, icon_fg, text_fg, sub_fg) = if active {
        (
            hud.selection_surface,
            hud.accent.with_alpha(80),
            1.0,
            hud.accent,
            Color::rgba(255, 255, 255, 255),
            hud.text,
            hud.accent,
        )
    } else {
        (
            hud.surface_recessed,
            hud.border,
            1.0,
            hud.surface,
            hud.text_muted,
            hud.text,
            hud.text_muted,
        )
    };

    f.set_theme(themes::hud(&hud).with_fg(text_fg));
    let chevron_id = format!("{id}-chevron");
    let mut chevron_clicked = false;

    let (response, _) = f.pressable_row(
        id,
        title,
        &LayoutOpts {
            width: size.0,
            height: size.1,
            pad: 10.0,
            radius: 16.0,
            cross: Align::Center,
            gap: 8.0,
            bg,
            border,
            border_width,
            ..Default::default()
        },
        |f, _| {
            // Icon
            f.column_ex(
                &LayoutOpts {
                    width: 36.0,
                    height: 36.0,
                    radius: 18.0,
                    bg: icon_bg,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.set_theme(themes::hud(&hud).with_fg(icon_fg));
                    f.icon_raw(icon, 18.0);
                    f.flex(1.0);
                    f.spacer(0.0);
                },
            );

            // Labels
            let label_w = (size.0 - 20.0 - 36.0 - 8.0 - 28.0 - 8.0).max(1.0);
            f.column_ex(
                &LayoutOpts {
                    width: label_w,
                    height: 36.0,
                    gap: 1.0,
                    cross: Align::Start,
                    ..Default::default()
                },
                |f| {
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.set_theme(themes::hud(&hud).with_fg(text_fg));
                    display_label(f, title, type_scale.body);
                    f.set_theme(themes::hud(&hud).with_fg(sub_fg));
                    display_label(f, subtitle, type_scale.footnote);
                    f.flex(1.0);
                    f.spacer(0.0);
                },
            );

            // Chevron expand button
            let (chev_resp, _) = f.pressable_row(
                &chevron_id,
                "Expand",
                &LayoutOpts {
                    width: 28.0,
                    height: 36.0,
                    radius: 8.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f, _| {
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.set_theme(themes::hud(&hud).with_fg(sub_fg));
                    f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_CHEVRON_RIGHT, 16.0);
                    f.flex(1.0);
                    f.spacer(0.0);
                },
            );
            if chev_resp.clicked {
                chevron_clicked = true;
            }
        },
    );
    f.set_theme(original);

    let toggle_clicked = response.clicked && !chevron_clicked;
    (toggle_clicked, chevron_clicked)
}

pub(super) fn render_horizontal_fader(
    f: &mut Frame,
    id: &str,
    label: &str,
    icon: lens::sys::lens_icon_id,
    icon_clickable: bool,
    value: Option<u8>,
    range: (u8, u8),
    size: (f32, f32),
    fill: Color,
    hud: ControlCenterColors,
    type_scale: TypeScale,
) -> (Option<u8>, bool) {
    let original = f.theme();
    let enabled = value.is_some();
    let mut level = value.unwrap_or(range.0) as f32;
    let theme = themes::hud(&hud)
        .with_fg(if enabled { hud.text } else { hud.text_muted })
        .with_slider_track_color(hud.control_track)
        .with_slider_fill_color(if enabled { fill } else { hud.text_muted })
        .with_slider_knob_color(if enabled {
            hud.control_knob
        } else {
            hud.text_muted
        })
        .with_slider_track_thickness(10.0)
        .with_slider_knob_size(16.0);
    f.set_theme(theme);
    let mut changed = false;
    let mut icon_clicked = false;

    f.column_ex(
        &LayoutOpts {
            width: size.0,
            height: size.1,
            gap: 6.0,
            pad: 12.0,
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
                    width: (size.0 - 24.0).max(1.0),
                    height: 24.0,
                    cross: Align::Center,
                    gap: 8.0,
                    ..Default::default()
                },
                |f| {
                    if icon_clickable {
                        let (resp, _) = f.pressable_row(
                            &format!("{id}-icon-btn"),
                            label,
                            &LayoutOpts {
                                width: 28.0,
                                height: 24.0,
                                cross: Align::Center,
                                radius: 6.0,
                                bg: Color::TRANSPARENT,
                                ..Default::default()
                            },
                            |f, _| {
                                f.icon_raw(icon, 18.0);
                            },
                        );
                        if resp.clicked {
                            icon_clicked = true;
                        }
                    } else {
                        f.icon_raw(icon, 18.0);
                    }
                    display_label(f, label, type_scale.body);
                    f.flex(1.0);
                    f.spacer(0.0);
                    display_label(
                        f,
                        &value
                            .map(|v| format!("{v}%"))
                            .unwrap_or_else(|| "--".to_string()),
                        type_scale.footnote,
                    );
                },
            );

            f.size_next((size.0 - 24.0).max(40.0), 20.0);
            changed = f.slider(
                &format!("##{id}"),
                &mut level,
                range.0 as f32,
                range.1 as f32,
            );
        },
    );
    f.set_theme(original);
    (
        (changed && enabled).then(|| level.round().clamp(range.0 as f32, range.1 as f32) as u8),
        icon_clicked,
    )
}

// ---- dbusmenu popover helpers -------------------------------------------

/// Decorate a row label with a toggle glyph (when present) and a submenu
/// chevron suffix (when the row opens a submenu).
pub(super) fn menu_row_label(row: &MenuNode) -> String {
    let mut label = row.label.clone();
    if row.toggle.is_on() {
        let glyph = match row.toggle {
            tray::MenuToggle::Checkmark(_) => "✓ ",
            tray::MenuToggle::Radio(_) => "● ",
            _ => "",
        };
        label.insert_str(0, glyph);
    }
    if row.has_submenu {
        label.push(' ');
        label.push('▸');
    }
    label
}

/// Compute the visible popover bounds from the owner cell rect, the visible
/// rows (already truncated to the current `menu_path`), and the display. The
/// height counts every visible row + separator and the optional Back header.
/// The popover opens directly to the right of the tray owner.
pub(super) fn menu_bounds(owner: Rect, visible: &[MenuNode], display: (f32, f32)) -> Rect {
    let separator_count = visible
        .iter()
        .filter(|row| row.visible && row.kind == tray::MenuEntryKind::Separator)
        .count();
    let row_count = visible
        .iter()
        .filter(|row| row.visible && row.kind != tray::MenuEntryKind::Separator)
        .count();
    // Overestimating is safe: this is used for click-away hit-testing, and
    // the render pass recomputes the exact height from the same rows.
    let height = MENU_PAD * 2.0
        + MENU_HEADER_HEIGHT
        + row_count as f32 * MENU_ROW_HEIGHT
        + separator_count as f32 * MENU_SEPARATOR_HEIGHT;
    place_popup_side(owner, (MENU_WIDTH, height), display, PopupSide::Right)
}

// ---- display typography (ADR-0080 refresh) -------------------------------

/// Weight of the panel's display typography. lens draws text at the theme's
/// regular weight (`text_weight = 0`); the bold 600–700 range reads as the
/// game-HUD voice the panel wants for its labels.
pub(super) const DISPLAY_WEIGHT: f32 = 700.0;

/// A bold compact label: measures and draws at [`DISPLAY_WEIGHT`] in the
/// engine's default sans-serif family, so centered and right-aligned runs
/// stay geometrically exact. This is the panel's standard label call —
/// plain `label_compact_sized` inside the panel is a review flag.
pub(super) fn display_label(f: &mut Frame, text: &str, size: f32) {
    f.label_compact_weighted(text, size, DISPLAY_WEIGHT);
}

// ---- network surface charts ----------------------------------------------
