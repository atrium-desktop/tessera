//! Bento tiles: the dock's non-icon cells.
//!
//! Status leads the dock: the workspace rail sits at the leading edge as an
//! interactive, progressive segmented control.
//!
//! A cell states its own width at rest and refuses magnification. When only a single
//! workspace exists, it auto-hides (reserving 0.0 width and leaving the dock pure and
//! centered). When multiple workspaces exist, it renders as a sleek, compact capsule
//! that supports direct click-to-switch and scroll-wheel workspace cycling.

use lens::{Frame, LayoutOpts, Rect};
use tessera_design::{Design, materials};

/// What a Bento cell shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    /// The interactive workspace rail: one indicator per workspace, the current one
    /// lit as a capsule. Supports click and scroll-wheel switching. Auto-hides when
    /// only 1 workspace exists.
    WorkspacePager,
}

impl TileKind {
    /// Width at rest, in logical pixels.
    ///
    /// When `workspaces <= 1`, returns 0.0 so the rail auto-hides and reserves no
    /// space on the dock. When `workspaces >= 2`, calculates the compact rail span.
    #[must_use]
    pub fn rest_width(self, workspaces: usize) -> f32 {
        match self {
            TileKind::WorkspacePager => {
                if workspaces <= 1 {
                    0.0
                } else {
                    RAIL_INNER_PAD * 2.0
                        + ACTIVE_TICK
                        + (workspaces - 1) as f32 * TICK
                        + (workspaces - 1) as f32 * TICK_GAP
                }
            }
        }
    }
}

/// Shared rail metrics.
pub const RAIL_INNER_PAD: f32 = 8.0;
pub const TICK: f32 = 6.0;
pub const TICK_GAP: f32 = 5.0;
pub const ACTIVE_TICK: f32 = 16.0;
pub const RAIL_HEIGHT: f32 = 22.0;

/// The gap between the reading section and the launch targets.
pub const READING_BOUNDARY_GAP: f32 = 12.0;

/// One Bento cell in the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BentoCell {
    pub kind: TileKind,
    pub workspaces: usize,
}

impl BentoCell {
    #[must_use]
    pub fn workspace_pager(workspaces: usize) -> BentoCell {
        BentoCell {
            kind: TileKind::WorkspacePager,
            workspaces,
        }
    }

    #[must_use]
    pub fn rest_width(self) -> f32 {
        self.kind.rest_width(self.workspaces)
    }
}

/// Resolve the dock's rest layout length, including its Bento cells.
///
/// Pure, so the strip length the interaction model assumes and the length the
/// paint produces cannot disagree. Cells with 0 rest width (e.g. single workspace)
/// add no width and insert no boundary gap.
#[must_use]
pub fn strip_length(
    tile_widths: impl IntoIterator<Item = f32>,
    bento: &[BentoCell],
    tile_gap: f32,
    pad: f32,
) -> f32 {
    let mut total = 0.0_f32;
    let mut count = 0usize;
    let mut active_bento = 0usize;
    for cell in bento {
        let w = cell.rest_width();
        if w > 0.0 {
            if active_bento > 0 {
                total += tile_gap;
            }
            total += w;
            active_bento += 1;
            count += 1;
        }
    }
    for width in tile_widths {
        if count > 0 {
            total += if active_bento > 0 && count == active_bento {
                READING_BOUNDARY_GAP
            } else {
                tile_gap
            };
        }
        total += width;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    total + 2.0 * pad
}

/// The cell's recessed stadium plate.
fn place_plate(frame: &mut Frame, design: &Design, id: &str, rect: Rect) {
    frame.place(
        &format!("{id}-plate"),
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: design.colors.card_surface,
                border: design.colors.panel_border,
                border_width: 0.5,
                radius: rect.h.min(rect.w) * 0.5,
                pad: 0.0,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
}

/// Draw the interactive workspace rail.
pub fn render_workspace_rail(
    frame: &mut Frame,
    design: &Design,
    id: &str,
    slot_rect: Rect,
    workspaces: usize,
    current: usize,
    hovered: Option<usize>,
    progress: f32,
    vertical: bool,
) {
    if progress <= 0.001 || workspaces <= 1 || slot_rect.w <= 0.0 || slot_rect.h <= 0.0 {
        return;
    }

    let total_ticks = workspaces;
    let current = current.min(total_ticks.saturating_sub(1));
    let total_ticks_span =
        ACTIVE_TICK + (total_ticks - 1) as f32 * TICK + (total_ticks - 1) as f32 * TICK_GAP;
    let rail_span = RAIL_INNER_PAD * 2.0 + total_ticks_span;
    let rail_thickness = (RAIL_HEIGHT * progress).min(slot_rect.w).min(slot_rect.h);

    let (rail_rect, rail_x, rail_y) = if vertical {
        let rx = slot_rect.x + (slot_rect.w - rail_thickness) * 0.5;
        let ry = slot_rect.y + (slot_rect.h - rail_span) * 0.5;
        (
            Rect {
                x: rx,
                y: ry,
                w: rail_thickness,
                h: rail_span,
            },
            rx,
            ry,
        )
    } else {
        let rx = slot_rect.x + (slot_rect.w - rail_span) * 0.5;
        let ry = slot_rect.y + (slot_rect.h - rail_thickness) * 0.5;
        (
            Rect {
                x: rx,
                y: ry,
                w: rail_span,
                h: rail_thickness,
            },
            rx,
            ry,
        )
    };

    place_plate(frame, design, id, rail_rect);

    let mut offset = 0.0_f32;
    for index in 0..total_ticks {
        let active = index == current;
        let is_hovered = hovered == Some(index);
        let span = if active { ACTIVE_TICK } else { TICK };

        let tick_rect = if vertical {
            Rect {
                x: rail_x + (rail_thickness - TICK) * 0.5,
                y: rail_y + RAIL_INNER_PAD + offset,
                w: TICK,
                h: span,
            }
        } else {
            Rect {
                x: rail_x + RAIL_INNER_PAD + offset,
                y: rail_y + (rail_thickness - TICK) * 0.5,
                w: span,
                h: TICK,
            }
        };

        let bg = if active {
            design.floating_foreground.primary
        } else if is_hovered {
            design.floating_foreground.primary.with_alpha(217)
        } else {
            design.dock.running_dot_inactive
        };

        frame.place(
            &format!("{id}-tick-{index}"),
            &materials::chrome_place(
                tick_rect,
                LayoutOpts {
                    bg,
                    radius: TICK * 0.5,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );

        offset += span + TICK_GAP;
    }
}

/// Hit test cursor against the workspace rail ticks.
pub fn hit_test_workspace_rail(
    slot_rect: Rect,
    cursor: (f32, f32),
    workspaces: usize,
    current: usize,
    vertical: bool,
) -> Option<usize> {
    if workspaces <= 1 {
        return None;
    }
    let total_ticks = workspaces;
    let current = current.min(total_ticks.saturating_sub(1));
    let total_ticks_span =
        ACTIVE_TICK + (total_ticks - 1) as f32 * TICK + (total_ticks - 1) as f32 * TICK_GAP;
    let rail_span = RAIL_INNER_PAD * 2.0 + total_ticks_span;
    let rail_thickness = RAIL_HEIGHT;

    let (rail_rect, rail_x, rail_y) = if vertical {
        let rx = slot_rect.x + (slot_rect.w - rail_thickness) * 0.5;
        let ry = slot_rect.y + (slot_rect.h - rail_span) * 0.5;
        (
            Rect {
                x: rx,
                y: ry,
                w: rail_thickness,
                h: rail_span,
            },
            rx,
            ry,
        )
    } else {
        let rx = slot_rect.x + (slot_rect.w - rail_span) * 0.5;
        let ry = slot_rect.y + (slot_rect.h - rail_thickness) * 0.5;
        (
            Rect {
                x: rx,
                y: ry,
                w: rail_span,
                h: rail_thickness,
            },
            rx,
            ry,
        )
    };

    let hit_pad = 4.0_f32;
    if cursor.0 < rail_rect.x - hit_pad
        || cursor.0 >= rail_rect.x + rail_rect.w + hit_pad
        || cursor.1 < rail_rect.y - hit_pad
        || cursor.1 >= rail_rect.y + rail_rect.h + hit_pad
    {
        return None;
    }

    let mut offset = 0.0_f32;
    for index in 0..total_ticks {
        let active = index == current;
        let span = if active { ACTIVE_TICK } else { TICK };
        let (slot_start, slot_end) = if vertical {
            let start = rail_y + RAIL_INNER_PAD + offset - TICK_GAP * 0.5;
            let end = start + span + TICK_GAP;
            (start, end)
        } else {
            let start = rail_x + RAIL_INNER_PAD + offset - TICK_GAP * 0.5;
            let end = start + span + TICK_GAP;
            (start, end)
        };

        let cursor_axis = if vertical { cursor.1 } else { cursor.0 };
        if cursor_axis >= slot_start && cursor_axis < slot_end {
            return Some(index);
        }

        offset += span + TICK_GAP;
    }

    let cursor_axis = if vertical { cursor.1 } else { cursor.0 };
    let start_axis = if vertical { rail_y } else { rail_x };
    if cursor_axis < start_axis + RAIL_INNER_PAD {
        Some(0)
    } else {
        Some(total_ticks.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_workspace_auto_hides_with_zero_width() {
        let zero = BentoCell::workspace_pager(0).rest_width();
        let one = BentoCell::workspace_pager(1).rest_width();
        assert_eq!(zero, 0.0);
        assert_eq!(one, 0.0);
    }

    #[test]
    fn multiple_workspaces_scale_compactly() {
        let two = BentoCell::workspace_pager(2).rest_width();
        let three = BentoCell::workspace_pager(3).rest_width();
        let four = BentoCell::workspace_pager(4).rest_width();
        assert_eq!(two, RAIL_INNER_PAD * 2.0 + ACTIVE_TICK + TICK + TICK_GAP);
        assert!(two < three);
        assert!(three < four);
        assert!(
            four <= 80.0,
            "four workspaces stay compact, not a massive panel"
        );
    }

    #[test]
    fn single_workspace_adds_zero_to_strip_length() {
        let icons = [48.0_f32, 48.0];
        let cells = [BentoCell::workspace_pager(1)];

        let without = strip_length(icons, &[], 6.0, 8.0);
        let with = strip_length(icons, &cells, 6.0, 8.0);
        assert_eq!(with, without, "single workspace reserves zero extra width");
    }

    #[test]
    fn multiple_workspaces_extend_strip_length_with_boundary_gap() {
        let icons = [48.0_f32, 48.0];
        let cells = [BentoCell::workspace_pager(3)];

        let without = strip_length(icons, &[], 6.0, 8.0);
        let with = strip_length(icons, &cells, 6.0, 8.0);
        assert_eq!(with - without, READING_BOUNDARY_GAP + cells[0].rest_width());
    }

    #[test]
    fn hit_test_finds_correct_workspace_tick() {
        let width = BentoCell::workspace_pager(3).rest_width();
        let slot = Rect {
            x: 100.0,
            y: 50.0,
            w: width,
            h: 48.0,
        };
        let hit0 = hit_test_workspace_rail(slot, (105.0, 60.0), 3, 0, false);
        assert_eq!(hit0, Some(0));

        let hit2 = hit_test_workspace_rail(slot, (100.0 + width - 5.0, 60.0), 3, 0, false);
        assert_eq!(hit2, Some(2));

        let outside = hit_test_workspace_rail(slot, (50.0, 60.0), 3, 0, false);
        assert_eq!(outside, None);
    }
}
