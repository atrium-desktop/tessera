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
        optics_ui::sys::optics_ui_icon_id::OPTICS_UI_ICON_WIFI_OFF
    );
    assert_eq!(
        wifi_icon(ReadingTone::Idle),
        optics_ui::sys::optics_ui_icon_id::OPTICS_UI_ICON_WIFI
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
