//! The dock's leading cells: the machine's own readings, and the workspace pager.
//!
//! Two cells lead the icon strip, and both are `Bento` in the sense that
//! matters: their extent is *computed* from what they hold rather than assumed
//! from a constant (ADR-0218).
//!
//! * [`BentoColumn`] is the reading column. It sits across the bar's
//!   *thickness*, not along its length: the bar is 74 px tall and a resting tile
//!   is 56 px, so a plate as tall as a tile fits inside the bar with margin to
//!   spare. Its thickness is divided into [`COLUMN_UNITS`] units, every reading
//!   spans `COLUMN_UNITS / n` of them, and no reading may span fewer than
//!   [`MIN_ROW_UNITS`]. Six is divisible by every row count that floor permits,
//!   so the rows tile the plate exactly for one, two, or three readings — and
//!   three is both the arithmetic ceiling (`6 / 2`) and the perceptual one.
//! * [`BentoPager`] is the interactive workspace rail. It sits along the bar's
//!   length as a compact segmented capsule, auto-hides when only one workspace
//!   exists, and supports click-to-switch and wheel cycling.
//!
//! The two cells are the two things a leading cell can be, so the strip length,
//! the hit test, and the paint each match on [`StripCell`] instead of growing a
//! flag per cell kind.

use lens::{Frame, LayoutOpts, Rect};
use tessera_design::{Design, materials};
use tessera_desktop::system::{BatteryStatus, BluetoothLinkState, SystemStatus, WifiLinkState};

/// How many units the reading column's thickness is divided into.
///
/// Six, because six is the smallest unit count divisible by every row count the
/// minimum-span rule permits: `6`, `3 + 3`, `2 + 2 + 2`. There is no remainder
/// to distribute and no fractional pixel in any case.
pub const COLUMN_UNITS: u32 = 6;

/// The fewest units a single reading may span.
///
/// Two, which makes the row-count ceiling `COLUMN_UNITS / MIN_ROW_UNITS = 3`.
/// The floor is what keeps a reading legible: an unconstrained subdivision of a
/// 56 px plate eventually produces a 9 px row that is neither readable nor
/// pointable.
pub const MIN_ROW_UNITS: u32 = 2;

/// The most readings the column can hold, derived from the algebra rather than
/// written beside it.
pub const MAX_ROWS: u32 = COLUMN_UNITS / MIN_ROW_UNITS;

/// The reading column's length along the bar: one narrow plate, as tall as a
/// resting tile.
///
/// The column costs this span *whatever it holds*. Adding a radio changes what
/// is inside the plate and never moves an icon, so the bar's length stays a
/// function of what the user pinned rather than of hardware inventory.
pub const COLUMN_WIDTH: f32 = 30.0;

/// Inner padding between the plate's edge and its rows.
pub const COLUMN_PAD: f32 = 3.0;

/// A battery gauge's track thickness.
pub const GAUGE_THICKNESS: f32 = 3.0;

/// How far the gauge band sits inside the row's rim.
pub const BAND_INSET: f32 = 2.0;

/// Diameter of the connected-peripheral dot on a live Bluetooth row.
pub const BADGE_DOT: f32 = 3.0;

/// Shared pager metrics.
pub const RAIL_INNER_PAD: f32 = 8.0;
pub const TICK: f32 = 6.0;
pub const TICK_GAP: f32 = 5.0;
pub const ACTIVE_TICK: f32 = 16.0;
pub const RAIL_HEIGHT: f32 = 22.0;

/// The gap between the leading cells and the launch targets.
pub const READING_BOUNDARY_GAP: f32 = 12.0;

/// What a reading reports about the machine.
///
/// Three states, and each one draws a different mark: a linked radio is not an
/// idle one, and neither is an off one. Collapsing them would let "Bluetooth on,
/// nothing connected" and "Bluetooth off" draw the same glyph, which is the
/// single most common misreading a status surface can invite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingTone {
    /// The radio is up and carrying a session (or the battery is charging).
    Linked,
    /// The radio is on and idle, scanning, or associated but down.
    Idle,
    /// The radio is off, or the service behind it is unavailable.
    Off,
}

impl ReadingTone {
    /// Whether this tone reads as live. Used for the foreground ramp; `Off` is
    /// the muted one.
    #[must_use]
    pub fn is_live(self) -> bool {
        matches!(self, ReadingTone::Linked)
    }
}

/// What one row of the reading column shows.
///
/// Every field here has a consumer in the paint: the tone picks the glyph and the
/// ink ramp, and the Bluetooth count picks whether the connected dot is drawn.
/// Signal strength is deliberately absent — the row answers "is the link up", and
/// the exact bars, SSID, and RSSI belong to the surface one press away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// The wireless link.
    Wifi { tone: ReadingTone },
    /// The Bluetooth radio and how many peripherals it is carrying.
    Bluetooth { tone: ReadingTone, connected: u8 },
    /// The battery, as a magnitude.
    Battery { status: BatteryStatus },
}

impl Reading {
    /// The row's tone.
    #[must_use]
    pub fn tone(self) -> ReadingTone {
        match self {
            Reading::Wifi { tone, .. } | Reading::Bluetooth { tone, .. } => tone,
            Reading::Battery { status } => {
                if status.charging {
                    ReadingTone::Linked
                } else {
                    ReadingTone::Idle
                }
            }
        }
    }

    /// Stable key for the row's placed-node id, so a row that changes tone or
    /// value keeps its node identity across frames.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Reading::Wifi { .. } => "wifi",
            Reading::Bluetooth { .. } => "bluetooth",
            Reading::Battery { .. } => "battery",
        }
    }
}

/// The machine's readings, resolved from one status snapshot.
///
/// A reading is present exactly when the machine reports the *capability*, never
/// when a value merely happens to be non-default: a desktop with no battery must
/// not carry a battery row, and a machine whose wireless service never answered
/// must not claim Wi-Fi is off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BentoColumn {
    rows: Vec<Reading>,
}

impl BentoColumn {
    /// Resolve the column from a status snapshot.
    ///
    /// Order is fixed: Wi-Fi, Bluetooth, battery. The order is a reading order
    /// (network, then peripherals, then power) and not a priority — the rows
    /// never reorder, so a value changing never moves a row.
    #[must_use]
    pub fn resolve(status: &SystemStatus) -> BentoColumn {
        let mut rows = Vec::new();
        if let Some(enabled) = status.wifi_enabled {
            rows.push(Reading::Wifi {
                tone: wifi_tone(enabled, status.wifi_state),
            });
        }
        if let Some(enabled) = status.bluetooth_enabled {
            rows.push(Reading::Bluetooth {
                tone: bluetooth_tone(enabled, status.bluetooth_state),
                connected: status
                    .bluetooth_devices
                    .iter()
                    .filter(|device| device.connected)
                    .count()
                    .min(u8::MAX as usize) as u8,
            });
        }
        if let Some(battery) = status.battery {
            rows.push(Reading::Battery { status: battery });
        }
        // The ceiling is the algebra's, not the hardware's: a machine that grew a
        // fourth radio would need `MIN_ROW_UNITS` raised, and silently drawing a
        // fourth row at 1.5 units would break the tiling invariant instead.
        rows.truncate(MAX_ROWS as usize);
        BentoColumn { rows }
    }

    /// The rows, in fixed reading order.
    #[must_use]
    pub fn rows(&self) -> &[Reading] {
        &self.rows
    }

    /// How many readings are present.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the machine reports none of the readings this column draws.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Units one row spans: `COLUMN_UNITS / n`.
    ///
    /// Exact for every `n` the minimum-span rule permits, which is why the
    /// division needs no remainder handling.
    #[must_use]
    pub fn row_units(&self) -> u32 {
        match self.rows.len() {
            0 => 0,
            n => COLUMN_UNITS / n as u32,
        }
    }

    /// The row rects inside a plate, in absolute coordinates.
    ///
    /// Rows stack across the bar's *thickness*: down the plate on a bottom or top
    /// dock, across it on a side dock. `vertical` means the bar runs along the
    /// output's y axis (a left or right dock), so the plate is tall and narrow
    /// there and the rows divide its width.
    ///
    /// The union of the rects is the plate's padded interior, with no overlap and
    /// no remainder — the tiling invariant, expressed once here so the paint and
    /// the hit test cannot disagree about where a row ends.
    #[must_use]
    pub fn row_rects(&self, plate: Rect, vertical: bool) -> Vec<Rect> {
        let count = self.rows.len();
        if count == 0 {
            return Vec::new();
        }
        let units = COLUMN_UNITS as f32;
        let span = self.row_units() as f32;
        // The pad is removed from the plate's thickness *before* it is divided,
        // so the rows tile the padded interior and the pad stays a pad.
        let thickness = if vertical { plate.w } else { plate.h };
        let inner = thickness - 2.0 * COLUMN_PAD;
        if inner <= 0.0 {
            return Vec::new();
        }
        (0..count)
            .map(|index| {
                let start = COLUMN_PAD + inner * (index as f32 * span / units);
                let extent = inner * (span / units);
                if vertical {
                    // Side dock: the plate is tall, so rows divide its width and
                    // each spans the padded height.
                    Rect {
                        x: plate.x + start,
                        y: plate.y + COLUMN_PAD,
                        w: extent,
                        h: (plate.h - 2.0 * COLUMN_PAD).max(0.0),
                    }
                } else {
                    // Bottom/top dock: the plate is wide and short, so rows
                    // divide its height and each spans the padded width.
                    Rect {
                        x: plate.x + COLUMN_PAD,
                        y: plate.y + start,
                        w: (plate.w - 2.0 * COLUMN_PAD).max(0.0),
                        h: extent,
                    }
                }
            })
            .collect()
    }

    /// The row index under `cursor`, or `None` when the pointer is outside the
    /// plate.
    ///
    /// One algebra for the tiling and the hit test: the row boundaries come from
    /// [`Self::row_rects`], so a hit cannot land in a row the paint did not draw.
    #[must_use]
    pub fn row_at(&self, plate: Rect, cursor: (f32, f32), vertical: bool) -> Option<usize> {
        if cursor.0 < plate.x
            || cursor.0 >= plate.x + plate.w
            || cursor.1 < plate.y
            || cursor.1 >= plate.y + plate.h
        {
            return None;
        }
        self.row_rects(plate, vertical).iter().position(|row| {
            cursor.0 >= row.x
                && cursor.0 < row.x + row.w
                && cursor.1 >= row.y
                && cursor.1 < row.y + row.h
        })
    }
}

/// The tone for the wireless link.
fn wifi_tone(enabled: bool, state: WifiLinkState) -> ReadingTone {
    if !enabled {
        return ReadingTone::Off;
    }
    match state {
        WifiLinkState::Connected => ReadingTone::Linked,
        WifiLinkState::Disabled => ReadingTone::Off,
        WifiLinkState::Disconnected | WifiLinkState::Scanning | WifiLinkState::Connecting => {
            ReadingTone::Idle
        }
    }
}

/// The tone for the Bluetooth radio.
fn bluetooth_tone(enabled: bool, state: BluetoothLinkState) -> ReadingTone {
    if !enabled || state == BluetoothLinkState::Unavailable {
        return ReadingTone::Off;
    }
    match state {
        BluetoothLinkState::Connected => ReadingTone::Linked,
        BluetoothLinkState::Disabled | BluetoothLinkState::Unavailable => ReadingTone::Off,
        BluetoothLinkState::Idle | BluetoothLinkState::Scanning => ReadingTone::Idle,
    }
}

/// The interactive workspace pager's own span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BentoPager {
    pub workspaces: usize,
}

impl BentoPager {
    #[must_use]
    pub fn new(workspaces: usize) -> BentoPager {
        BentoPager { workspaces }
    }

    /// Width at rest, in logical pixels.
    ///
    /// When `workspaces <= 1`, returns 0.0 so the rail auto-hides and reserves no
    /// space on the dock. When `workspaces >= 2`, calculates the compact rail
    /// span.
    #[must_use]
    pub fn rest_width(self) -> f32 {
        if self.workspaces <= 1 {
            0.0
        } else {
            RAIL_INNER_PAD * 2.0
                + ACTIVE_TICK
                + (self.workspaces - 1) as f32 * TICK
                + (self.workspaces - 1) as f32 * TICK_GAP
        }
    }
}

/// One cell leading the dock's strip.
#[derive(Debug, Clone, PartialEq)]
pub enum StripCell {
    /// The machine's own readings, across the bar's thickness (ADR-0218).
    Readings(BentoColumn),
    /// The interactive workspace pager, along the bar's length.
    WorkspacePager(BentoPager),
}

impl StripCell {
    /// Length at rest along the bar's long axis.
    ///
    /// The reading column costs one constant span whatever it holds; the pager
    /// costs its computed rail span. A cell that reserves zero length (an empty
    /// column, a single workspace) inserts no boundary gap either.
    #[must_use]
    pub fn rest_width(&self) -> f32 {
        match self {
            StripCell::Readings(column) => {
                if column.is_empty() {
                    0.0
                } else {
                    COLUMN_WIDTH
                }
            }
            StripCell::WorkspacePager(pager) => pager.rest_width(),
        }
    }
}

/// Resolve the dock's rest layout length, including its leading cells.
///
/// Pure, so the strip length the interaction model assumes and the length the
/// paint produces cannot disagree. Cells with 0 rest width (an empty reading
/// column, a single workspace) add no width and insert no boundary gap.
#[must_use]
pub fn strip_length(
    tile_widths: impl IntoIterator<Item = f32>,
    cells: &[StripCell],
    tile_gap: f32,
    pad: f32,
) -> f32 {
    let mut total = 0.0_f32;
    let mut count = 0usize;
    let mut active_cells = 0usize;
    for cell in cells {
        let w = cell.rest_width();
        if w > 0.0 {
            if active_cells > 0 {
                total += tile_gap;
            }
            total += w;
            active_cells += 1;
            count += 1;
        }
    }
    for width in tile_widths {
        if count > 0 {
            total += if active_cells > 0 && count == active_cells {
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

/// One painted piece of the reading column, resolved without a frame.
///
/// The layout is pure so it can be asserted directly: a test that re-ran the
/// geometry arithmetic would pass on a renderer that placed nothing, and a test
/// that ran a frame would be asserting on a GPU context. The plan is the middle
/// ground — exactly the rects the paint will place, in paint order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnPiece {
    /// The recessed plate the whole column sits on.
    Plate { rect: Rect },
    /// The hover tint of one row.
    RowHover { index: usize, rect: Rect },
    /// One reading's mark (a glyph, or a battery's glyph and gauge).
    ///
    /// The row's own extent is deliberately absent: it is [`BentoColumn::row_rects`],
    /// and carrying a second copy here would let a piece disagree with the tiling
    /// it came from.
    Reading {
        index: usize,
        glyph: Rect,
        /// The battery's gauge track and its proportional fill, in leading-edge
        /// order. `None` for a radio reading, which has no magnitude to show.
        gauge: Option<Gauge>,
        /// The connected-peripheral dot of a live Bluetooth row, if it has one.
        /// `None` for every other reading, and for a Bluetooth radio with nothing
        /// connected — a dot that says "something is connected" must not be drawn
        /// when nothing is.
        badge: Option<Rect>,
    },
    /// A hairline inside the shared boundary between two adjacent rows.
    Separator { index: usize, rect: Rect },
}

/// A battery gauge's track and the fill that carries its level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gauge {
    pub track: Rect,
    pub fill: Rect,
}

/// Resolve everything the reading column will paint, in paint order.
///
/// `hovered` is the row index under the pointer. `progress` is the bar's content
/// reveal, so a column that is still draining paints nothing and a partially
/// revealed one paints at the size it actually is.
#[must_use]
pub fn column_plan(
    plate: Rect,
    column: &BentoColumn,
    hovered: Option<usize>,
    progress: f32,
    vertical: bool,
) -> Vec<ColumnPiece> {
    if progress <= 0.001 || column.is_empty() || plate.w <= 0.0 || plate.h <= 0.0 {
        return Vec::new();
    }
    let rows = column.row_rects(plate, vertical);
    if rows.is_empty() {
        return Vec::new();
    }
    let mut plan = Vec::with_capacity(rows.len() * 3 + 1);
    plan.push(ColumnPiece::Plate { rect: plate });

    for (index, row) in rows.iter().enumerate() {
        let reading = column.rows()[index];
        if hovered == Some(index) {
            plan.push(ColumnPiece::RowHover { index, rect: *row });
        }
        // A radio mark is centred in the row and sized from the row's short axis:
        // at three rows a 24 x 16.67 row draws a 10.3 px mark, at one row a
        // 24 x 50 row draws a 14.9 px one. A battery row sizes its own glyph,
        // because the gauge band claims part of the row first.
        let (glyph, gauge) = match reading {
            Reading::Battery { status } => battery_parts(*row, status),
            Reading::Wifi { .. } | Reading::Bluetooth { .. } => {
                let short = row.w.min(row.h);
                let glyph_size = (short * 0.62).clamp(9.0, 16.0);
                (radio_glyph_rect(*row, glyph_size), None)
            }
        };
        // A linked radio carries its count as a dot, not a digit: at 11 px a
        // numeral is a smudge, and "something is connected" is the answer the row
        // exists to give (ADR-0218).
        let badge = match reading {
            Reading::Bluetooth {
                tone,
                connected: count,
            } if tone.is_live() && count > 0 && row.w >= BADGE_DOT && row.h >= BADGE_DOT => {
                Some(Rect {
                    x: row.x + (row.w - BADGE_DOT - 1.0).max(0.0),
                    y: row.y + (row.h - BADGE_DOT - 1.0).max(0.0),
                    w: BADGE_DOT,
                    h: BADGE_DOT,
                })
            }
            _ => None,
        };
        plan.push(ColumnPiece::Reading {
            index,
            glyph,
            gauge,
            badge,
        });
    }

    // Separators are paint, not content: a hairline centred in the shared
    // boundary between two rows. Spending a grid unit on a divider would take 17%
    // of the column to draw a line, and would make the three-row case need eight
    // units (ADR-0218).
    for (index, row) in rows.iter().enumerate().skip(1) {
        plan.push(ColumnPiece::Separator {
            index,
            rect: if vertical {
                Rect {
                    x: row.x,
                    y: plate.y + COLUMN_PAD,
                    w: 1.0,
                    h: (plate.h - 2.0 * COLUMN_PAD).max(0.0),
                }
            } else {
                Rect {
                    x: plate.x + COLUMN_PAD,
                    y: row.y,
                    w: (plate.w - 2.0 * COLUMN_PAD).max(0.0),
                    h: 1.0,
                }
            },
        });
    }
    plan
}

/// A radio reading's glyph: centred on *both* axes of the row.
///
/// Centred, not edge-hugging. A radio mark has no second element to sit beside,
/// so hugging the leading edge leaves it visibly off-centre — at three rows a
/// 24 x 16.67 row would put a 10.33 px glyph 5.8 px left of the row's centre,
/// which reads as a misaligned column rather than as a deliberate one. Only the
/// battery row pairs its glyph with a gauge, and that pairing is what gives a
/// glyph a leading edge to sit on.
fn radio_glyph_rect(row: Rect, glyph: f32) -> Rect {
    let glyph = glyph.max(0.0).min(row.w.min(row.h).max(0.0));
    Rect {
        x: row.x + (row.w - glyph).max(0.0) * 0.5,
        y: row.y + (row.h - glyph).max(0.0) * 0.5,
        w: glyph,
        h: glyph,
    }
}

/// A battery row's glyph and gauge.
///
/// The gauge is a **band along the row's own long axis**, hugging its trailing
/// edge, and the glyph is centred in the space that band leaves. That shape is
/// chosen by measurement, not by taste: laying the glyph and the gauge end to end
/// along the row gives the gauge `row_long - glyph - 4`, which is 5.1 px at one
/// row and 9.7 px at three — a 40% charge would be a 2 px sliver in every
/// geometry the column can produce. A band costs the row nothing on its long
/// axis, so the gauge is 20 px at three rows and 46 px at one.
///
/// Orientation comes from the row's own aspect, never from the dock's edge. A
/// bottom dock with one reading has a *tall* row (24 x 50) because rows stack
/// down the plate's height, so keying the gauge on "the dock is at the bottom"
/// laid a horizontal gauge in a 24 px row. The row knows its own shape; the dock
/// edge does not.
///
/// The fill runs from the leading edge — left on a wide row, bottom on a tall one,
/// the way a level reads.
fn battery_parts(row: Rect, status: BatteryStatus) -> (Rect, Option<Gauge>) {
    let wide = row.w >= row.h;
    // The band hugs the trailing long edge, inset from the row's own rim so it
    // does not touch the plate's hairline.
    let band = if wide {
        Rect {
            x: row.x + BAND_INSET,
            y: row.y + (row.h - BAND_INSET - GAUGE_THICKNESS).max(0.0),
            w: (row.w - 2.0 * BAND_INSET).max(0.0),
            h: GAUGE_THICKNESS.min(row.h.max(0.0)),
        }
    } else {
        Rect {
            x: row.x + (row.w - BAND_INSET - GAUGE_THICKNESS).max(0.0),
            y: row.y + BAND_INSET,
            w: GAUGE_THICKNESS.min(row.w.max(0.0)),
            h: (row.h - 2.0 * BAND_INSET).max(0.0),
        }
    };
    // The glyph takes the space the band leaves, sized from *that* space's short
    // dimension so it cannot bleed into the band or its neighbour.
    let (avail_w, avail_h) = if wide {
        (
            row.w.max(0.0),
            (row.h - GAUGE_THICKNESS - BAND_INSET).max(0.0),
        )
    } else {
        (
            (row.w - GAUGE_THICKNESS - BAND_INSET).max(0.0),
            row.h.max(0.0),
        )
    };
    let short_dim = avail_w.min(avail_h);
    let min_glyph = if short_dim >= 10.0 {
        9.0
    } else {
        short_dim * 0.62
    };
    let glyph_size = (short_dim * 0.62).clamp(min_glyph, 16.0).max(0.0);
    let glyph = Rect {
        x: row.x + (avail_w - glyph_size).max(0.0) * 0.5,
        y: row.y + (avail_h - glyph_size).max(0.0) * 0.5,
        w: glyph_size,
        h: glyph_size,
    };

    if band.w <= 0.0 || band.h <= 0.0 {
        return (glyph, None);
    }
    let fraction = (f32::from(status.percent) / 100.0).clamp(0.0, 1.0);
    let fill = if wide {
        Rect {
            w: (band.w * fraction).max(0.0),
            ..band
        }
    } else {
        Rect {
            y: band.y + band.h * (1.0 - fraction),
            h: (band.h * fraction).max(0.0),
            ..band
        }
    };
    (glyph, Some(Gauge { track: band, fill }))
}

/// Draw the reading column from its resolved plan.
///
/// The plate is placed once and the rows are placed inside it, so the column
/// reads as one machine-state surface rather than as a stack of unrelated chips.
/// A hovered row is tinted to say it is pressable; the tint is its own fill and
/// not a sibling overlay, because an ABS node drawn after the rows would swallow
/// their hit test (Optics ADR-0060).
pub fn render_reading_column(
    frame: &mut Frame,
    design: &Design,
    id: &str,
    plate: Rect,
    column: &BentoColumn,
    hovered: Option<usize>,
    progress: f32,
    vertical: bool,
) {
    let plan = column_plan(plate, column, hovered, progress, vertical);
    if plan.is_empty() {
        return;
    }
    let alpha = (progress * 255.0).clamp(0.0, 255.0) as u8;

    for piece in plan {
        match piece {
            ColumnPiece::Plate { rect } => place_plate(frame, design, id, rect),
            ColumnPiece::RowHover { index, rect } => {
                let reading = column.rows()[index];
                frame.place(
                    &format!("{id}-row-{}-hover", reading.key()),
                    &materials::chrome_place(
                        rect,
                        LayoutOpts {
                            bg: design.selection.hover_tint.with_alpha(alpha),
                            radius: 5.0,
                            pad: 0.0,
                            ..materials::surface_layout()
                        },
                    ),
                    |_| {},
                );
            }
            ColumnPiece::Separator { index, rect } => {
                frame.place(
                    &format!("{id}-sep-{index}"),
                    &materials::chrome_place(
                        rect,
                        LayoutOpts {
                            bg: design.dock.section_divider.with_alpha(alpha),
                            radius: 0.0,
                            pad: 0.0,
                            ..materials::surface_layout()
                        },
                    ),
                    |_| {},
                );
            }
            ColumnPiece::Reading {
                index,
                glyph,
                gauge,
                badge,
            } => {
                let reading = column.rows()[index];
                paint_reading(frame, design, id, glyph, gauge, badge, reading, alpha);
            }
        }
    }
}

/// Paint one row from its resolved plan.
///
/// The plan already decided every rect, so this is only ink: the glyph, the
/// battery's gauge, and the connected-peripheral dot. Nothing here computes a
/// position, which is what keeps the layout testable without a frame.
fn paint_reading(
    frame: &mut Frame,
    design: &Design,
    id: &str,
    glyph_rect: Rect,
    gauge: Option<Gauge>,
    badge: Option<Rect>,
    reading: Reading,
    alpha: u8,
) {
    let tone = reading.tone();
    let ink = if tone.is_live() {
        design.floating_foreground.primary
    } else {
        design.dock.running_dot_inactive
    }
    .with_alpha(alpha);

    let original = frame.theme();
    frame.set_theme(original.with_fg(ink));

    // A bolt when charging, a plain battery otherwise, the wifi mark when the
    // radio is on and the struck-through one when it is off, and the Bluetooth
    // rune likewise. Every glyph is either a built-in lens icon or a
    // runtime-registered product SVG resolved by `widgets::icons`.
    let icon = match reading {
        Reading::Wifi { .. } => wifi_icon(tone),
        Reading::Bluetooth { .. } => bluetooth_icon(tone),
        Reading::Battery { status } => {
            if status.charging {
                lens::sys::lens_icon_id::LENS_ICON_BATTERY_CHARGING
            } else {
                lens::sys::lens_icon_id::LENS_ICON_BATTERY
            }
        }
    };
    let glyph_size = glyph_rect.w.min(glyph_rect.h);
    frame.place(
        &format!("{id}-mark-{}", reading.key()),
        &materials::chrome_place(
            glyph_rect,
            LayoutOpts {
                pad: 0.0,
                cross: lens::Align::Center,
                ..materials::surface_layout()
            },
        ),
        |frame| frame.icon_raw(icon, glyph_size),
    );

    // The gauge is the battery's magnitude: a muted track and a proportional
    // fill. It is never a printed percentage — at a 30 px plate the digits are
    // illegible, and an illegible number looks like information (ADR-0218).
    if let Some(gauge) = gauge {
        let fill_ink = if reading.tone().is_live() {
            design.floating_foreground.primary
        } else {
            design.dock.running_dot_inactive
        }
        .with_alpha(alpha);
        frame.place(
            &format!("{id}-gauge-track"),
            &materials::chrome_place(
                gauge.track,
                LayoutOpts {
                    bg: design.dock.running_dot_inactive.with_alpha(alpha / 2),
                    radius: GAUGE_THICKNESS * 0.5,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
        if gauge.fill.w > 0.0 && gauge.fill.h > 0.0 {
            frame.place(
                &format!("{id}-gauge-fill"),
                &materials::chrome_place(
                    gauge.fill,
                    LayoutOpts {
                        bg: fill_ink,
                        radius: GAUGE_THICKNESS * 0.5,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |_| {},
            );
        }
    }

    // The plan decided whether a connected-peripheral dot belongs here and where
    // it goes; this only inks it.
    if let Some(badge) = badge {
        frame.place(
            &format!("{id}-badge-{}", reading.key()),
            &materials::chrome_place(
                badge,
                LayoutOpts {
                    bg: ink,
                    radius: BADGE_DOT * 0.5,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    }

    frame.set_theme(original);
}

/// The wireless glyph for a tone.
///
/// `Linked` and `Idle` share the mark: an associated link and an idle radio are
/// both "the radio is on", and the tone ramp already says which. `Off` is the
/// struck-through glyph, because "off" and "on but idle" must never draw the same
/// shape.
fn wifi_icon(tone: ReadingTone) -> lens::sys::lens_icon_id {
    match tone {
        ReadingTone::Linked | ReadingTone::Idle => lens::sys::lens_icon_id::LENS_ICON_WIFI,
        ReadingTone::Off => lens::sys::lens_icon_id::LENS_ICON_WIFI_OFF,
    }
}

/// The Bluetooth glyph for a tone: the rune, struck through when off.
fn bluetooth_icon(tone: ReadingTone) -> lens::sys::lens_icon_id {
    let enabled = !matches!(tone, ReadingTone::Off);
    crate::widgets::icons::bluetooth(enabled)
        .unwrap_or(lens::sys::lens_icon_id::LENS_ICON_BLUETOOTH)
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
    use tessera_desktop::system::BluetoothDevice;

    /// A status whose wireless radio reports `state`.
    fn wifi_status(state: WifiLinkState) -> SystemStatus {
        SystemStatus {
            wifi_enabled: Some(state != WifiLinkState::Disabled),
            wifi_state: state,
            ..Default::default()
        }
    }

    fn full_status() -> SystemStatus {
        let mut status = wifi_status(WifiLinkState::Connected);
        status.bluetooth_enabled = Some(true);
        status.bluetooth_state = BluetoothLinkState::Connected;
        status.battery = Some(BatteryStatus {
            percent: 80,
            charging: false,
        });
        status
    }

    fn plate() -> Rect {
        Rect {
            x: 100.0,
            y: 50.0,
            w: COLUMN_WIDTH,
            h: 56.0,
        }
    }

    #[test]
    fn column_resolves_only_reported_capabilities() {
        let empty = BentoColumn::resolve(&SystemStatus::default());
        assert!(empty.is_empty(), "a machine with no radios has no column");

        let wifi_only = BentoColumn::resolve(&wifi_status(WifiLinkState::Connected));
        assert_eq!(wifi_only.len(), 1);
        assert_eq!(wifi_only.rows()[0].key(), "wifi");

        let all = BentoColumn::resolve(&full_status());
        assert_eq!(all.len(), 3);
        assert_eq!(
            all.rows().iter().map(|r| r.key()).collect::<Vec<_>>(),
            vec!["wifi", "bluetooth", "battery"],
            "order is fixed: network, peripherals, power"
        );
    }

    #[test]
    fn row_units_tile_the_column_exactly() {
        let column = BentoColumn::resolve(&full_status());
        assert_eq!(column.row_units(), 2, "three rows span 6 / 3 = 2 units");

        let rows = column.row_rects(plate(), false);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].y, 50.0 + COLUMN_PAD);
        let last = &rows[2];
        assert!(
            (last.y + last.h - (50.0 + 56.0 - COLUMN_PAD)).abs() < 1e-4,
            "the last row must reach the padded interior's far edge"
        );
        for pair in rows.windows(2) {
            assert!(
                (pair[0].y + pair[0].h - pair[1].y).abs() < 1e-4,
                "rows must abut with no gap and no overlap"
            );
        }
    }

    #[test]
    fn one_reading_spans_the_whole_column() {
        let column = BentoColumn::resolve(&wifi_status(WifiLinkState::Connected));
        assert_eq!(column.row_units(), COLUMN_UNITS);
        let rows = column.row_rects(plate(), false);
        assert_eq!(rows.len(), 1);
        assert!((rows[0].h - (56.0 - 2.0 * COLUMN_PAD)).abs() < 1e-4);
    }

    #[test]
    fn side_dock_rows_divide_the_plate_width() {
        let column = BentoColumn::resolve(&full_status());
        let side_plate = Rect {
            x: 20.0,
            y: 100.0,
            w: 56.0,
            h: COLUMN_WIDTH,
        };
        let rows = column.row_rects(side_plate, true);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert!((row.h - (COLUMN_WIDTH - 2.0 * COLUMN_PAD)).abs() < 1e-4);
        }
        for pair in rows.windows(2) {
            assert!((pair[0].x + pair[0].w - pair[1].x).abs() < 1e-4);
        }
    }

    #[test]
    fn row_count_ceiling_is_derived_from_the_floor() {
        assert_eq!(MAX_ROWS, 3);
        assert_eq!(MAX_ROWS * MIN_ROW_UNITS, COLUMN_UNITS);
    }

    #[test]
    fn off_and_idle_radios_draw_different_marks() {
        assert_eq!(
            wifi_icon(ReadingTone::Off),
            lens::sys::lens_icon_id::LENS_ICON_WIFI_OFF
        );
        assert_eq!(
            wifi_icon(ReadingTone::Idle),
            lens::sys::lens_icon_id::LENS_ICON_WIFI
        );
        assert_ne!(
            bluetooth_icon(ReadingTone::Off),
            bluetooth_icon(ReadingTone::Idle),
            "an off radio and an idle one must never share a glyph"
        );
    }

    #[test]
    fn wifi_tone_follows_the_link_state() {
        assert_eq!(wifi_tone(false, WifiLinkState::Connected), ReadingTone::Off);
        assert_eq!(
            wifi_tone(true, WifiLinkState::Connected),
            ReadingTone::Linked
        );
        assert_eq!(
            wifi_tone(true, WifiLinkState::Connecting),
            ReadingTone::Idle
        );
        assert_eq!(wifi_tone(true, WifiLinkState::Disabled), ReadingTone::Off);
    }

    #[test]
    fn bluetooth_tone_follows_the_link_state() {
        assert_eq!(
            bluetooth_tone(true, BluetoothLinkState::Connected),
            ReadingTone::Linked
        );
        assert_eq!(
            bluetooth_tone(true, BluetoothLinkState::Scanning),
            ReadingTone::Idle
        );
        assert_eq!(
            bluetooth_tone(true, BluetoothLinkState::Unavailable),
            ReadingTone::Off
        );
        assert_eq!(
            bluetooth_tone(false, BluetoothLinkState::Connected),
            ReadingTone::Off
        );
    }

    #[test]
    fn bluetooth_row_counts_connected_peripherals() {
        let mut status = SystemStatus {
            bluetooth_enabled: Some(true),
            bluetooth_state: BluetoothLinkState::Connected,
            ..Default::default()
        };
        status.bluetooth_devices = vec![
            BluetoothDevice {
                connected: true,
                ..Default::default()
            },
            BluetoothDevice {
                connected: false,
                ..Default::default()
            },
            BluetoothDevice {
                connected: true,
                ..Default::default()
            },
        ];
        let column = BentoColumn::resolve(&status);
        match column.rows()[0] {
            Reading::Bluetooth { connected, .. } => assert_eq!(connected, 2),
            other => panic!("expected a Bluetooth row, got {other:?}"),
        }
    }

    #[test]
    fn battery_tone_reports_charging() {
        let column = BentoColumn::resolve(&SystemStatus {
            battery: Some(BatteryStatus {
                percent: 40,
                charging: true,
            }),
            ..Default::default()
        });
        assert_eq!(column.rows()[0].tone(), ReadingTone::Linked);
    }

    #[test]
    fn empty_column_reserves_zero_length() {
        let cell = StripCell::Readings(BentoColumn::default());
        assert_eq!(cell.rest_width(), 0.0);
        let icons = [48.0_f32, 48.0];
        let without = strip_length(icons, &[], 6.0, 8.0);
        let with = strip_length(icons, &[cell], 6.0, 8.0);
        assert_eq!(with, without, "an empty column costs no length");
    }

    #[test]
    fn column_costs_one_constant_span_whatever_it_holds() {
        let three = StripCell::Readings(BentoColumn::resolve(&full_status()));
        let one = StripCell::Readings(BentoColumn::resolve(&wifi_status(WifiLinkState::Connected)));
        assert_eq!(three.rest_width(), one.rest_width());
        assert_eq!(one.rest_width(), COLUMN_WIDTH);
    }

    #[test]
    fn multiple_workspaces_scale_compactly() {
        let two = BentoPager::new(2).rest_width();
        let three = BentoPager::new(3).rest_width();
        let four = BentoPager::new(4).rest_width();
        assert_eq!(two, RAIL_INNER_PAD * 2.0 + ACTIVE_TICK + TICK + TICK_GAP);
        assert!(two < three);
        assert!(three < four);
        assert!(
            four <= 80.0,
            "four workspaces stay compact, not a massive panel"
        );
    }

    #[test]
    fn single_workspace_auto_hides_with_zero_width() {
        assert_eq!(BentoPager::new(0).rest_width(), 0.0);
        assert_eq!(BentoPager::new(1).rest_width(), 0.0);
    }

    #[test]
    fn cells_extend_strip_length_with_boundary_gap() {
        let icons = [48.0_f32, 48.0];
        let cells = [StripCell::WorkspacePager(BentoPager::new(3))];

        let without = strip_length(icons, &[], 6.0, 8.0);
        let with = strip_length(icons, &cells, 6.0, 8.0);
        assert_eq!(with - without, READING_BOUNDARY_GAP + cells[0].rest_width());
    }

    #[test]
    fn both_cells_take_one_tile_gap_between_them() {
        let cells = [
            StripCell::Readings(BentoColumn::resolve(&full_status())),
            StripCell::WorkspacePager(BentoPager::new(4)),
        ];
        let icons = [48.0_f32];
        let expected = 2.0 * 8.0
            + cells[0].rest_width()
            + 6.0
            + cells[1].rest_width()
            + READING_BOUNDARY_GAP
            + 48.0;
        assert_eq!(strip_length(icons, &cells, 6.0, 8.0), expected);
    }

    #[test]
    fn row_at_matches_the_painted_tiling() {
        let column = BentoColumn::resolve(&full_status());
        let plate = plate();
        let rows = column.row_rects(plate, false);
        for (index, row) in rows.iter().enumerate() {
            let inside = (row.x + row.w * 0.5, row.y + row.h * 0.5);
            assert_eq!(column.row_at(plate, inside, false), Some(index));
        }
        assert_eq!(column.row_at(plate, (0.0, 0.0), false), None);
    }

    #[test]
    fn row_at_works_across_the_thickness_on_a_side_dock() {
        let column = BentoColumn::resolve(&full_status());
        let side_plate = Rect {
            x: 20.0,
            y: 100.0,
            w: 56.0,
            h: COLUMN_WIDTH,
        };
        let rows = column.row_rects(side_plate, true);
        for (index, row) in rows.iter().enumerate() {
            let inside = (row.x + row.w * 0.5, row.y + row.h * 0.5);
            assert_eq!(column.row_at(side_plate, inside, true), Some(index));
        }
    }

    #[test]
    fn the_plan_paints_one_plate_one_mark_per_row_and_n_minus_one_separators() {
        let column = BentoColumn::resolve(&full_status());
        let plan = column_plan(plate(), &column, None, 1.0, false);
        let count = |wanted: fn(&ColumnPiece) -> bool| plan.iter().filter(|p| wanted(p)).count();
        assert_eq!(count(|p| matches!(p, ColumnPiece::Plate { .. })), 1);
        assert_eq!(count(|p| matches!(p, ColumnPiece::Reading { .. })), 3);
        assert_eq!(
            count(|p| matches!(p, ColumnPiece::Separator { .. })),
            2,
            "three rows have two shared boundaries, and a separator is paint"
        );
        assert_eq!(count(|p| matches!(p, ColumnPiece::RowHover { .. })), 0);
    }

    #[test]
    fn the_plan_is_empty_when_the_column_draws_nothing() {
        assert!(column_plan(plate(), &BentoColumn::default(), None, 1.0, false).is_empty());
        let column = BentoColumn::resolve(&full_status());
        assert!(
            column_plan(plate(), &column, None, 0.0, false).is_empty(),
            "a fully drained bar paints no leading content"
        );
    }

    #[test]
    fn the_plan_marks_only_the_hovered_row() {
        let column = BentoColumn::resolve(&full_status());
        let plan = column_plan(plate(), &column, Some(1), 1.0, false);
        let hovered: Vec<usize> = plan
            .iter()
            .filter_map(|piece| match piece {
                ColumnPiece::RowHover { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(hovered, vec![1]);
    }

    /// The gauge is a band along the row's long axis, and its fill is the charge.
    ///
    /// Orientation comes from the row's own aspect: with one reading on a bottom
    /// dock the row is 24 x 50, so the band runs *down* it. Keying this on the
    /// dock's edge instead laid a horizontal gauge in a 24 px row.
    #[test]
    fn the_gauge_band_runs_along_the_rows_long_axis() {
        let battery = BentoColumn::resolve(&SystemStatus {
            battery: Some(BatteryStatus {
                percent: 40,
                charging: false,
            }),
            ..Default::default()
        });
        let plan = column_plan(plate(), &battery, None, 1.0, false);
        let row = battery.row_rects(plate(), false)[0];
        let gauge = plan
            .iter()
            .find_map(|piece| match piece {
                ColumnPiece::Reading { gauge, .. } => *gauge,
                _ => None,
            })
            .expect("a battery row carries a gauge");

        assert!(
            row.h > row.w,
            "one reading on a bottom dock is a tall row: {row:?}"
        );
        assert!(
            gauge.track.h > gauge.track.w,
            "the band must follow the row's long axis, not the dock's edge: {gauge:?}"
        );
        assert!(
            (gauge.fill.h - gauge.track.h * 0.4).abs() < 1e-4,
            "the fill is the charge's proportion of the band"
        );
        assert!(
            (gauge.fill.y + gauge.fill.h - (gauge.track.y + gauge.track.h)).abs() < 1e-4,
            "a tall band fills upward from its bottom edge, the way a level reads"
        );
    }

    #[test]
    fn a_wide_gauge_fills_from_its_left_edge() {
        let mut status = full_status();
        status.battery = Some(BatteryStatus {
            percent: 25,
            charging: false,
        });
        // Three rows on a bottom dock: each row is 24 x 16.67, so the band is
        // horizontal.
        let column = BentoColumn::resolve(&status);
        let plan = column_plan(plate(), &column, None, 1.0, false);
        let gauge = plan
            .iter()
            .find_map(|piece| match piece {
                ColumnPiece::Reading { gauge, .. } => *gauge,
                _ => None,
            })
            .expect("a battery row carries a gauge");
        assert!(
            gauge.track.w > gauge.track.h,
            "a short row takes a wide band"
        );
        assert!((gauge.fill.w - gauge.track.w * 0.25).abs() < 1e-4);
        assert!((gauge.fill.x - gauge.track.x).abs() < 1e-4);
    }

    /// The gauge must be long enough for a proportion to mean anything.
    ///
    /// Laying the glyph and the gauge end to end along the row gave the gauge
    /// 5.1 px at one row and 9.7 px at three, so a 40% charge was a 2 px sliver in
    /// every geometry the column can produce. A band costs the row nothing on its
    /// long axis, so the band is 20 px at three rows and 46 px at one.
    #[test]
    fn the_gauge_is_never_a_sliver_in_any_geometry() {
        // One, two, and three readings, built from the tail of a full status so
        // the battery row is present in each.
        let mut statuses = Vec::new();
        for bluetooth in [false, true] {
            let mut status = wifi_status(WifiLinkState::Connected);
            if bluetooth {
                status.bluetooth_enabled = Some(true);
            }
            status.battery = Some(BatteryStatus {
                percent: 40,
                charging: false,
            });
            statuses.push(status);
        }
        statuses.push(full_status());

        let plates = [
            (
                false,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: COLUMN_WIDTH,
                    h: 56.0,
                },
            ),
            (
                true,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 56.0,
                    h: COLUMN_WIDTH,
                },
            ),
        ];

        let mut checked = 0;
        for status in &statuses {
            let column = BentoColumn::resolve(status);
            for (vertical, plate) in plates {
                let plan = column_plan(plate, &column, None, 1.0, vertical);
                let Some(gauge) = plan.iter().find_map(|piece| match piece {
                    ColumnPiece::Reading {
                        gauge: Some(gauge), ..
                    } => Some(*gauge),
                    _ => None,
                }) else {
                    continue;
                };
                let long = gauge.track.w.max(gauge.track.h);
                assert!(
                    long >= 14.0,
                    "a {} -row {vertical} column gives only a {long} px band: {gauge:?}",
                    column.len()
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 6, "three reading counts x two orientations");
    }

    /// `[INV-DOCK-BENTO-8]`: the inner layout follows the row, not the edge.
    ///
    /// Enumerates every (reading count, dock orientation) pair the algebra can
    /// produce and asserts the inner layout is decided by the row's own aspect.
    #[test]
    fn row_orientation_comes_from_the_row_not_the_dock_edge() {
        let plates = [
            (
                false,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: COLUMN_WIDTH,
                    h: 56.0,
                },
            ),
            (
                true,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 56.0,
                    h: COLUMN_WIDTH,
                },
            ),
        ];
        for status in [wifi_status(WifiLinkState::Connected), full_status()] {
            let column = BentoColumn::resolve(&status);
            for (vertical, plate) in plates {
                let plan = column_plan(plate, &column, None, 1.0, vertical);
                let rows = column.row_rects(plate, vertical);
                for piece in &plan {
                    let ColumnPiece::Reading {
                        index,
                        glyph,
                        gauge,
                        ..
                    } = piece
                    else {
                        continue;
                    };
                    let row = rows[*index];
                    // The glyph is centred on both axes of its row.
                    assert!(
                        (glyph.x + glyph.w * 0.5 - (row.x + row.w * 0.5)).abs() < 0.01
                            && (glyph.y + glyph.h * 0.5 - (row.y + row.h * 0.5)).abs() < 0.01
                            || gauge.is_some(),
                        "a radio mark must be centred in its row: glyph {glyph:?} row {row:?}"
                    );
                    if let Some(gauge) = gauge {
                        let row_wide = row.w >= row.h;
                        let band_wide = gauge.track.w >= gauge.track.h;
                        assert_eq!(
                            row_wide, band_wide,
                            "the band must follow the row's long axis: row {row:?} band {:?}",
                            gauge.track
                        );
                    }
                }
            }
        }
    }

    /// `[INV-DOCK-BENTO-9]`: no reading shrinks below legibility.
    #[test]
    fn every_mark_clears_the_legibility_floor_in_every_geometry() {
        let plates = [
            (
                false,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: COLUMN_WIDTH,
                    h: 56.0,
                },
            ),
            (
                true,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 56.0,
                    h: COLUMN_WIDTH,
                },
            ),
        ];
        for status in [wifi_status(WifiLinkState::Connected), full_status()] {
            let column = BentoColumn::resolve(&status);
            for (vertical, plate) in plates {
                let plan = column_plan(plate, &column, None, 1.0, vertical);
                for piece in &plan {
                    if let ColumnPiece::Reading { glyph, .. } = piece {
                        assert!(
                            glyph.w >= 9.0 - 0.01 && glyph.h >= 9.0 - 0.01,
                            "a mark below 9 px is not a mark: {glyph:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_radio_row_carries_no_gauge() {
        let column = BentoColumn::resolve(&wifi_status(WifiLinkState::Connected));
        let plan = column_plan(plate(), &column, None, 1.0, false);
        for piece in &plan {
            if let ColumnPiece::Reading { gauge, .. } = piece {
                assert!(gauge.is_none(), "a radio has no magnitude to show");
            }
        }
    }

    #[test]
    fn every_planned_rect_stays_inside_the_plate() {
        let column = BentoColumn::resolve(&full_status());
        let plate = plate();
        let plan = column_plan(plate, &column, Some(2), 1.0, false);
        for piece in plan {
            let rects: Vec<Rect> = match piece {
                ColumnPiece::Plate { rect } => vec![rect],
                ColumnPiece::RowHover { rect, .. } | ColumnPiece::Separator { rect, .. } => {
                    vec![rect]
                }
                ColumnPiece::Reading {
                    glyph,
                    gauge,
                    badge,
                    ..
                } => {
                    let mut rects = vec![glyph];
                    if let Some(badge) = badge {
                        rects.push(badge);
                    }
                    if let Some(gauge) = gauge {
                        rects.push(gauge.track);
                        rects.push(gauge.fill);
                    }
                    rects
                }
            };
            for rect in rects {
                assert!(
                    rect.x >= plate.x - 0.01
                        && rect.y >= plate.y - 0.01
                        && rect.x + rect.w <= plate.x + plate.w + 0.01
                        && rect.y + rect.h <= plate.y + plate.h + 0.01,
                    "a planned rect escaped the plate: {rect:?} outside {plate:?}"
                );
            }
        }
    }

    #[test]
    fn hit_test_finds_correct_workspace_tick() {
        let width = BentoPager::new(3).rest_width();
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
