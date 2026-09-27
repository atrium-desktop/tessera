//! 2D Bento grid layout engine for modular control surfaces (ADR-0169).
//!
//! Provides variable-span cell placement (1x1, 2x1, 2x2, 4x1) with dense 2D
//! occupancy tracking and automatic reflow for optional/conditional widgets.

use lens::Rect;

/// Multi-column and multi-row cell span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BentoSpan {
    pub cols: u8,
    pub rows: u8,
}

impl BentoSpan {
    pub const fn new(cols: u8, rows: u8) -> Self {
        Self {
            cols: if cols == 0 { 1 } else { cols },
            rows: if rows == 0 { 1 } else { rows },
        }
    }

    /// Single compact 1x1 block (e.g. DND, Dark Mode, Lock).
    pub const TILE_1X1: Self = Self::new(1, 1);
    /// Wide 2x1 pill (e.g. Wi-Fi, Bluetooth).
    pub const WIDE_2X1: Self = Self::new(2, 1);
    /// Large 2x2 card (e.g. MPRIS Now Playing, Weather, Clock).
    pub const CARD_2X2: Self = Self::new(2, 2);
    /// Full-width 4x1 fader strip (e.g. Volume, Brightness).
    pub const STRIP_4X1: Self = Self::new(4, 1);
}

/// Specifications for a 2D Bento layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BentoSpec {
    pub columns: u8,
    pub unit_height: f32,
    pub gap: f32,
}

impl Default for BentoSpec {
    fn default() -> Self {
        Self {
            columns: 4,
            unit_height: 60.0,
            gap: 10.0,
        }
    }
}

/// A requested item to be placed on the grid.
#[derive(Debug, Clone, Copy)]
pub struct BentoItem<T> {
    pub key: T,
    pub span: BentoSpan,
    pub visible: bool,
}

/// A placed cell with concrete geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BentoCell<T> {
    pub key: T,
    pub col: u8,
    pub row: u8,
    pub span: BentoSpan,
    pub rect: Rect,
}

/// Complete resolved layout with all placed cells and total bounds.
#[derive(Debug, Clone)]
pub struct BentoLayout<T> {
    pub cells: Vec<BentoCell<T>>,
    pub total_width: f32,
    pub total_height: f32,
    pub rows_used: u8,
}

impl BentoSpec {
    /// Resolve dense 2D cell placement for a sequence of widgets.
    pub fn layout<T: Copy>(&self, area: Rect, items: &[BentoItem<T>]) -> BentoLayout<T> {
        let columns = self.columns.max(1);
        let gap = self.gap;
        let cell_w = ((area.w - gap * (columns as f32 - 1.0)) / columns as f32).max(1.0);
        let cell_h = self.unit_height;

        // 2D occupancy tracking: each row is represented as a bitmask (u64 supports up to 64 cols)
        let mut occupancy: Vec<u64> = Vec::new();

        let is_occupied = |occ: &[u64], c: u8, r: usize, span: BentoSpan| -> bool {
            for row_idx in r..(r + span.rows as usize) {
                if row_idx < occ.len() {
                    let mask = ((1u64 << span.cols) - 1) << c;
                    if (occ[row_idx] & mask) != 0 {
                        return true;
                    }
                }
            }
            false
        };

        let mark_occupied = |occ: &mut Vec<u64>, c: u8, r: usize, span: BentoSpan| {
            let required_len = r + span.rows as usize;
            if occ.len() < required_len {
                occ.resize(required_len, 0);
            }
            let mask = ((1u64 << span.cols) - 1) << c;
            for row_idx in r..required_len {
                occ[row_idx] |= mask;
            }
        };

        let mut cells = Vec::new();
        let mut max_row = 0;

        for item in items {
            if !item.visible {
                continue;
            }
            let span = BentoSpan::new(item.span.cols.min(columns), item.span.rows);

            // Find first available slot (row, col) that can accommodate (span.cols, span.rows)
            let mut placed = false;
            let mut r = 0;
            while !placed {
                for c in 0..=(columns - span.cols) {
                    if !is_occupied(&occupancy, c, r, span) {
                        mark_occupied(&mut occupancy, c, r, span);
                        let x = area.x + c as f32 * (cell_w + gap);
                        let y = area.y + r as f32 * (cell_h + gap);
                        let w = span.cols as f32 * cell_w + (span.cols as f32 - 1.0) * gap;
                        let h = span.rows as f32 * cell_h + (span.rows as f32 - 1.0) * gap;
                        cells.push(BentoCell {
                            key: item.key,
                            col: c,
                            row: r as u8,
                            span,
                            rect: Rect { x, y, w, h },
                        });
                        max_row = max_row.max(r + span.rows as usize);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    r += 1;
                }
            }
        }

        let total_h = if max_row == 0 {
            0.0
        } else {
            max_row as f32 * cell_h + (max_row as f32 - 1.0) * gap
        };

        BentoLayout {
            cells,
            total_width: area.w,
            total_height: total_h,
            rows_used: max_row as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bento_grid_dense_packing_and_geometry() {
        let spec = BentoSpec {
            columns: 4,
            unit_height: 60.0,
            gap: 10.0,
        };
        let area = Rect {
            x: 10.0,
            y: 20.0,
            w: 430.0,
            h: 600.0,
        };

        // 430px wide with 3 gaps of 10px = 400px / 4 = 100px per unit column
        // 2 cols = 100 * 2 + 10 = 210px
        let items = [
            // (0,0) 2x1
            BentoItem {
                key: "wifi",
                span: BentoSpan::WIDE_2X1,
                visible: true,
            },
            // (2,0) to (3,1) 2x2
            BentoItem {
                key: "media",
                span: BentoSpan::CARD_2X2,
                visible: true,
            },
            // (0,1) 2x1
            BentoItem {
                key: "bluetooth",
                span: BentoSpan::WIDE_2X1,
                visible: true,
            },
            // Invisible item skipped
            BentoItem {
                key: "hidden_item",
                span: BentoSpan::WIDE_2X1,
                visible: false,
            },
            // (0,2) 2x1
            BentoItem {
                key: "dnd",
                span: BentoSpan::WIDE_2X1,
                visible: true,
            },
            // (2,2) 2x1
            BentoItem {
                key: "dark_mode",
                span: BentoSpan::WIDE_2X1,
                visible: true,
            },
            // (0,3) 4x1 full width
            BentoItem {
                key: "brightness",
                span: BentoSpan::STRIP_4X1,
                visible: true,
            },
        ];

        let layout = spec.layout(area, &items);
        assert_eq!(layout.cells.len(), 6);
        assert_eq!(layout.rows_used, 4);

        // Check wifi at (col: 0, row: 0)
        assert_eq!(layout.cells[0].key, "wifi");
        assert_eq!(layout.cells[0].col, 0);
        assert_eq!(layout.cells[0].row, 0);
        assert_eq!(layout.cells[0].rect.x, 10.0);
        assert_eq!(layout.cells[0].rect.y, 20.0);
        assert_eq!(layout.cells[0].rect.w, 210.0);
        assert_eq!(layout.cells[0].rect.h, 60.0);

        // Check media at (col: 2, row: 0) with span 2x2
        assert_eq!(layout.cells[1].key, "media");
        assert_eq!(layout.cells[1].col, 2);
        assert_eq!(layout.cells[1].row, 0);
        assert_eq!(layout.cells[1].rect.x, 10.0 + 210.0 + 10.0);
        assert_eq!(layout.cells[1].rect.y, 20.0);
        assert_eq!(layout.cells[1].rect.w, 210.0);
        assert_eq!(layout.cells[1].rect.h, 60.0 * 2.0 + 10.0); // 130.0

        // Check bluetooth packed at (col: 0, row: 1)
        assert_eq!(layout.cells[2].key, "bluetooth");
        assert_eq!(layout.cells[2].col, 0);
        assert_eq!(layout.cells[2].row, 1);
        assert_eq!(layout.cells[2].rect.x, 10.0);
        assert_eq!(layout.cells[2].rect.y, 20.0 + 60.0 + 10.0);

        // Check brightness at (col: 0, row: 3) full width
        assert_eq!(layout.cells[5].key, "brightness");
        assert_eq!(layout.cells[5].col, 0);
        assert_eq!(layout.cells[5].row, 3);
        assert_eq!(layout.cells[5].rect.w, 430.0);
    }
}
