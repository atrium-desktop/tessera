//! Presentation rendering for Pivot (ADR-0151, ADR-0174).
//!
//! Stateless UI projection using Lens Frame, typography tokens, and glass materials.

use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, LayoutOpts, Rect};
use tessera_design::materials::{chrome_place, surface_layout};
use tessera_design::{Design, materials};
use tessera_launch_services::{IntentEngine, IntentKind, SystemCmd};
use tessera_desktop::app::Entry;
use tessera_desktop::window::Window;

use crate::component::{IconSet, Localizer, Message, ellipsize};
use crate::widgets::geom::contains;

use super::motion::PivotMotion;
use super::state::{AppCategory, PivotAction, PivotState};

pub const PANEL_MAX_WIDTH: f32 = 720.0;
pub const PANEL_SIDE_MARGIN: f32 = 20.0;
pub const PANEL_TOP_MIN: f32 = 64.0;
pub const PANEL_TOP_FRACTION: f32 = 0.12;
pub const SEARCH_HEIGHT: f32 = 68.0;
pub const RESULT_HEIGHT: f32 = 56.0;
pub const EMPTY_HEIGHT: f32 = 80.0;
pub const MAX_VISIBLE_RESULTS: usize = 6;
pub const ICON_SIZE: f32 = 36.0;
pub const BROWSE_TILE_W: f32 = 98.0;
pub const BROWSE_TILE_H: f32 = 88.0;
pub const BROWSE_COLS: usize = 6;

/// Icon presentation representation.
pub enum ItemIcon {
    App(Option<*mut c_void>),
    System(Icon),
    Math,
    Agent,
}

/// Displayable search result item.
pub struct PivotItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: ItemIcon,
    pub is_running: bool,
    pub action: PivotAction,
}

#[must_use]
pub fn top(display: (f32, f32)) -> f32 {
    (display.1 * PANEL_TOP_FRACTION)
        .max(PANEL_TOP_MIN)
        .min((display.1 - SEARCH_HEIGHT - PANEL_SIDE_MARGIN).max(PANEL_SIDE_MARGIN))
}

#[must_use]
pub fn panel_rect(
    state: &PivotState,
    display: (f32, f32),
    content_count: usize,
    progress: f32,
) -> Rect {
    let width = PANEL_MAX_WIDTH.min((display.0 - PANEL_SIDE_MARGIN * 2.0).max(1.0));
    let query_empty = state.query().is_empty();

    let content_height = if query_empty {
        let count = state.category_indices(state.active_category).len();
        let rows = count.div_ceil(BROWSE_COLS).clamp(1, 3);
        42.0 + (rows as f32 * BROWSE_TILE_H) + 16.0
    } else {
        let count = content_count.min(MAX_VISIBLE_RESULTS);
        if count == 0 {
            EMPTY_HEIGHT
        } else {
            count as f32 * RESULT_HEIGHT
        }
    };

    Rect {
        x: (display.0 - width) * 0.5,
        y: top(display) - (1.0 - progress.clamp(0.0, 1.0)) * 14.0,
        w: width,
        h: (SEARCH_HEIGHT + content_height).min((display.1 - top(display) - PANEL_SIDE_MARGIN).max(1.0)),
    }
}

pub fn cmd_to_icon(cmd: SystemCmd) -> Icon {
    match cmd {
        SystemCmd::Lock => Icon::Shield,
        SystemCmd::Screenshot => Icon::Zap,
        SystemCmd::Suspend => Icon::Clock,
        SystemCmd::Restart => Icon::RotateCw,
        SystemCmd::PowerOff => Icon::X,
    }
}

pub fn entry_icon(icons: &IconSet, entry: &Entry) -> Option<*mut c_void> {
    let get = |key: &str| {
        let key = key.to_ascii_lowercase();
        (!key.is_empty()).then(|| icons.get(&key)).flatten()
    };
    entry
        .startup_wm_class
        .as_deref()
        .and_then(get)
        .or_else(|| get(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id)))
        .or_else(|| entry.icon.as_deref().and_then(get))
        .or_else(|| icons.default_icon())
}

pub fn app_icon_by_id(icons: &IconSet, app_id: &str) -> Option<*mut c_void> {
    let get = |key: &str| {
        let key = key.to_ascii_lowercase();
        (!key.is_empty()).then(|| icons.get(&key)).flatten()
    };
    get(app_id.strip_suffix(".desktop").unwrap_or(app_id)).or_else(|| icons.default_icon())
}

pub fn build_search_items(
    state: &PivotState,
    icons: &IconSet,
    query: &str,
    windows: &[Window],
    i18n: &Localizer,
) -> Vec<PivotItem> {
    let intent_items = IntentEngine::query(
        query,
        state.brain.apps(),
        &state.brain.filtered(),
        |idx| state.brain.is_running(idx),
        windows,
        i18n,
    );

    intent_items
        .into_iter()
        .map(|it| {
            let icon = match &it.kind {
                IntentKind::App { app_id } => ItemIcon::App(app_icon_by_id(icons, app_id)),
                IntentKind::Window { app_id, .. } => ItemIcon::App(app_icon_by_id(icons, app_id)),
                IntentKind::System(cmd) => ItemIcon::System(cmd_to_icon(*cmd)),
                IntentKind::Math => ItemIcon::Math,
                IntentKind::Agent => ItemIcon::Agent,
            };
            PivotItem {
                title: it.title,
                subtitle: it.subtitle,
                icon,
                is_running: it.is_running,
                action: it.action,
            }
        })
        .collect()
}

pub fn render_pivot(
    state: &mut PivotState,
    motion: &PivotMotion,
    icons: &IconSet,
    design: &Design,
    frame: &mut Frame,
    display: (f32, f32),
    cursor: (f32, f32),
    pressed: bool,
    windows: &[Window],
    i18n: &Localizer,
) -> Option<PivotAction> {
    let progress = motion.visibility;
    let query = state.query();
    let query_empty = query.is_empty();
    let search_items = if !query_empty {
        build_search_items(state, icons, query, windows, i18n)
    } else {
        Vec::new()
    };

    let panel = panel_rect(state, display, search_items.len(), progress);

    let type_scale = design.typography;
    let mut panel_mat = materials::glass_panel(design);
    panel_mat.bg = design.colors.glass_surface;

    frame.place(
        "tessera-pivot-panel",
        &chrome_place(panel, panel_mat),
        |_| {},
    );

    // 1. Search Query Bar
    let search_rect = Rect {
        x: panel.x,
        y: panel.y,
        w: panel.w,
        h: SEARCH_HEIGHT,
    };
    let search_text_width = (search_rect.w - 80.0).max(0.0);
    let shown_placeholder = ellipsize(
        frame,
        i18n.text(Message::SearchApplications),
        type_scale.title,
        search_text_width,
    );
    let shown_query = ellipsize(frame, state.query(), type_scale.title, search_text_width);
    let query_metrics = frame.measure_text(&shown_query, type_scale.title);

    frame.place(
        "tessera-pivot-search-bar",
        &chrome_place(
            search_rect,
            LayoutOpts {
                bg: Color::TRANSPARENT,
                pad: 0.0,
                cross: Align::Center,
                ..surface_layout()
            },
        ),
        |frame| {
            frame.row_ex(
                &LayoutOpts {
                    width: search_rect.w,
                    height: search_rect.h,
                    gap: 12.0,
                    pad: 20.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.icon(Icon::Search, 22.0);
                    if query_empty {
                        frame.label_compact_sized(&shown_placeholder, type_scale.title);
                    } else {
                        frame.label_compact_sized(&shown_query, type_scale.title);
                    }
                },
            );
        },
    );

    // Active Caret
    if state.is_open() {
        let caret_x = if query_empty {
            search_rect.x + 54.0
        } else {
            (search_rect.x + 54.0 + query_metrics.width).min(search_rect.x + search_rect.w - 24.0)
        };
        frame.place(
            "tessera-pivot-caret",
            &chrome_place(
                Rect {
                    x: caret_x,
                    y: search_rect.y + 22.0,
                    w: 2.0,
                    h: 24.0,
                },
                LayoutOpts {
                    bg: design.colors.application_text,
                    radius: 1.0,
                    pad: 0.0,
                    ..surface_layout()
                },
            ),
            |_| {},
        );
    }

    // Separator divider
    frame.place(
        "tessera-pivot-divider",
        &chrome_place(
            Rect {
                x: panel.x + 16.0,
                y: panel.y + SEARCH_HEIGHT - 1.0,
                w: (panel.w - 32.0).max(0.0),
                h: 1.0,
            },
            LayoutOpts {
                bg: design.colors.application_border,
                pad: 0.0,
                ..surface_layout()
            },
        ),
        |_| {},
    );

    let mut chosen_action: Option<PivotAction> = None;

    if !query_empty {
        let capacity = MAX_VISIBLE_RESULTS;
        let range = 0..search_items.len().min(capacity);

        if search_items.is_empty() {
            frame.place(
                "tessera-pivot-empty",
                &chrome_place(
                    Rect {
                        x: panel.x,
                        y: panel.y + SEARCH_HEIGHT,
                        w: panel.w,
                        h: EMPTY_HEIGHT,
                    },
                    LayoutOpts {
                        bg: Color::TRANSPARENT,
                        pad: 0.0,
                        cross: Align::Center,
                        ..surface_layout()
                    },
                ),
                |frame| {
                    frame.centered(panel.w, EMPTY_HEIGHT, |frame| {
                        frame.label_compact_sized(
                            i18n.text(Message::NoApplicationsFound),
                            type_scale.body,
                        );
                    });
                },
            );
        } else {
            let selection_clamped = state.search_selection.min(search_items.len().saturating_sub(1));
            for (visible_pos, item) in search_items[range].iter().enumerate() {
                let row = Rect {
                    x: panel.x,
                    y: panel.y + SEARCH_HEIGHT + visible_pos as f32 * RESULT_HEIGHT,
                    w: panel.w,
                    h: RESULT_HEIGHT,
                };
                let hovered = state.is_open() && contains(row, cursor.0, cursor.1);
                if pressed && hovered {
                    chosen_action = Some(item.action.clone());
                }
                let selected = visible_pos == selection_clamped;
                let text_w = (row.w - 190.0).max(1.0);
                let name = ellipsize(frame, &item.title, type_scale.body, text_w);
                let subtitle = item
                    .subtitle
                    .as_deref()
                    .map(|t| ellipsize(frame, t, type_scale.footnote, text_w));
                let is_running = item.is_running;
                let action_badge = match &item.action {
                    PivotAction::LaunchApp(_) => {
                        if is_running {
                            "↵ Focus"
                        } else {
                            "↵ Open"
                        }
                    }
                    PivotAction::FocusWindow(_) => "↵ Switch",
                    PivotAction::SystemCommand(_) => "↵ Run",
                    PivotAction::CopyCalculation(_) => "↵ Copy",
                    PivotAction::AskAgent(_) => "↵ Ask Agent",
                };

                frame.place(
                    &format!("tessera-pivot-result-{visible_pos}"),
                    &chrome_place(
                        row,
                        LayoutOpts {
                            bg: if selected {
                                design.colors.application_surface_active
                            } else if hovered {
                                design.colors.application_surface_hover
                            } else {
                                Color::TRANSPARENT
                            },
                            pad: 0.0,
                            ..surface_layout()
                        },
                    ),
                    |frame| {
                        frame.row_ex(
                            &LayoutOpts {
                                width: row.w,
                                height: row.h,
                                gap: 12.0,
                                pad: 10.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |frame| {
                                match &item.icon {
                                    ItemIcon::App(ptr) => render_icon(frame, *ptr, progress),
                                    ItemIcon::System(ic) => {
                                        frame.icon(*ic, 22.0);
                                    }
                                    ItemIcon::Math => {
                                        frame.icon(Icon::Sliders, 22.0);
                                    }
                                    ItemIcon::Agent => {
                                        frame.icon(Icon::Zap, 22.0);
                                    }
                                }
                                frame.column_ex(
                                    &LayoutOpts {
                                        width: text_w,
                                        height: 40.0,
                                        gap: 2.0,
                                        ..Default::default()
                                    },
                                    |frame| {
                                        frame.label_compact_sized(&name, type_scale.body);
                                        if let Some(sub) = &subtitle {
                                            frame.label_compact_sized(sub, type_scale.footnote);
                                        }
                                    },
                                );
                                frame.flex(1.0);
                                if is_running && !selected {
                                    frame.label_compact_sized("●", type_scale.caption);
                                }
                                if selected {
                                    frame.row_ex(
                                        &LayoutOpts {
                                            pad: 4.0,
                                            radius: 5.0,
                                            bg: design.colors.menu_border,
                                            cross: Align::Center,
                                            ..Default::default()
                                        },
                                        |frame| {
                                            frame.label_compact_sized(
                                                action_badge,
                                                type_scale.caption,
                                            );
                                        },
                                    );
                                }
                            },
                        );
                    },
                );
            }
        }
    } else {
        let cat_row_y = panel.y + SEARCH_HEIGHT + 8.0;
        let pill_w = 74.0;
        for (i, &cat) in AppCategory::ALL.iter().enumerate() {
            let pill = Rect {
                x: panel.x + 20.0 + i as f32 * (pill_w + 6.0),
                y: cat_row_y,
                w: pill_w,
                h: 28.0,
            };
            let is_active = state.active_category == cat;
            let hovered = state.is_open() && contains(pill, cursor.0, cursor.1);
            if pressed && hovered {
                state.active_category = cat;
                state.browse_selection = 0;
            }
            frame.place(
                &format!("tessera-pivot-cat-{i}"),
                &chrome_place(
                    pill,
                    LayoutOpts {
                        bg: if is_active {
                            design.colors.application_surface_active
                        } else if hovered {
                            design.colors.application_surface_hover
                        } else {
                            Color::TRANSPARENT
                        },
                        radius: 14.0,
                        pad: 0.0,
                        cross: Align::Center,
                        ..surface_layout()
                    },
                ),
                |frame| {
                    frame.centered(pill.w, pill.h, |frame| {
                        frame.label_compact_sized(i18n.text(cat.message()), type_scale.footnote);
                    });
                },
            );
        }

        let cat_items = state.category_indices(state.active_category);
        let grid_y = cat_row_y + 36.0;
        for (pos, &app_index) in cat_items.iter().take(BROWSE_COLS * 3).enumerate() {
            let col = pos % BROWSE_COLS;
            let row = pos / BROWSE_COLS;
            let tile = Rect {
                x: panel.x + 20.0 + col as f32 * (BROWSE_TILE_W + 12.0),
                y: grid_y + row as f32 * BROWSE_TILE_H,
                w: BROWSE_TILE_W,
                h: BROWSE_TILE_H,
            };
            let hovered = state.is_open() && contains(tile, cursor.0, cursor.1);
            let selected = pos == state.browse_selection;
            if pressed && hovered {
                chosen_action = Some(PivotAction::LaunchApp(app_index));
            }
            let entry = &state.brain.apps()[app_index];
            let icon = entry_icon(icons, entry);
            let name = ellipsize(frame, &entry.name, type_scale.caption, BROWSE_TILE_W - 8.0);

            frame.place(
                &format!("tessera-pivot-browse-{pos}"),
                &chrome_place(
                    tile,
                    LayoutOpts {
                        bg: if selected {
                            design.colors.application_surface_active
                        } else if hovered {
                            design.colors.application_surface_hover
                        } else {
                            Color::TRANSPARENT
                        },
                        radius: 8.0,
                        pad: 0.0,
                        ..surface_layout()
                    },
                ),
                |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: tile.w,
                            height: tile.h,
                            gap: 6.0,
                            pad: 6.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            render_icon(frame, icon, progress);
                            frame.label_compact_sized(&name, type_scale.caption);
                        },
                    );
                },
            );
        }
    }

    frame.set_opacity(1.0);
    chosen_action
}

pub fn render_icon(frame: &mut Frame, icon: Option<*mut c_void>, progress: f32) {
    let size = ICON_SIZE * (0.92 + progress.clamp(0.0, 1.0) * 0.08);
    frame.centered(ICON_SIZE, ICON_SIZE, |frame| match icon {
        Some(pointer) => unsafe {
            frame.image(pointer as *mut lens::sys::flux_image, size, size);
        },
        None => {
            frame.column_ex(
                &LayoutOpts {
                    width: size,
                    height: size,
                    bg: Color::rgba(78, 88, 120, 230),
                    radius: 9.0,
                    ..Default::default()
                },
                |frame| {
                    frame.centered(size, size, |frame| {
                        frame.icon(Icon::FileText, 20.0);
                    });
                },
            );
        }
    });
}
