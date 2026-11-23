use super::*;

#[test]
fn drag_rect_normalizes_negative_sizes() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.anchor = Some(Point { x: 100.0, y: 100.0 });
    s.current = Point { x: 50.0, y: 60.0 };
    let rect = s.drag_rect().unwrap();
    assert_eq!(rect.origin.x, 50);
    assert_eq!(rect.origin.y, 60);
    assert_eq!(rect.size.w, 50);
    assert_eq!(rect.size.h, 40);
}

#[test]
fn shown_rect_is_none_without_anchor_or_selection() {
    let s = ScreenshotSelector::new();
    assert!(s.shown_rect().is_none());
}

#[test]
fn start_toggles_active_state_and_clears_prior_selection() {
    let mut s = ScreenshotSelector::new();
    s.start();
    assert!(s.active());
    s.anchor = Some(Point { x: 10.0, y: 10.0 });
    s.start();
    assert!(!s.active());
    assert!(s.anchor.is_none());
}

#[test]
fn release_stages_selection_without_closing() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
    s.update_pointer(Point { x: 80.0, y: 90.0 }, false, false);
    s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);
    assert_eq!(
        s.confirmed,
        Some(tessera_primitives::Rect::new(20, 30, 90, 40))
    );
    assert!(s.active(), "release must keep the selector open");
    assert!(s.anchor.is_none());
}

#[test]
fn confirm_key_emits_staged_region_and_closes() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
    s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);

    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Return,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert_eq!(
        out.screenshot_region,
        Some(tessera_primitives::Rect::new(20, 30, 90, 40))
    );
    assert!(!s.active());
}

#[test]
fn new_press_replaces_staged_selection() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
    s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);
    assert!(s.confirmed.is_some());

    s.update_pointer(Point { x: 200.0, y: 200.0 }, true, false);
    assert!(s.confirmed.is_none());
    assert_eq!(
        s.drag_rect(),
        Some(tessera_primitives::Rect::new(200, 200, 0, 0))
    );
}

#[test]
fn escape_cancels_staged_selection_without_emitting() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
    s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);

    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Escape,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert!(out.screenshot_region.is_none());
    assert!(!out.pick_cancelled, "the Print-key flow stays silent");
    assert!(!s.active());
}

#[test]
fn tiny_release_keeps_selector_waiting() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.update_pointer(Point { x: 50.0, y: 50.0 }, true, false);
    s.update_pointer(Point { x: 52.0, y: 53.0 }, false, true);
    assert!(s.confirmed.is_none());
    assert!(s.active());
}

#[test]
fn selector_captures_input_with_a_visible_live_cursor() {
    let mut s = ScreenshotSelector::new();
    s.start();
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    assert!(s.captures_keyboard());
    assert!(s.captures_pointer(0.0, 0.0, (100.0, 100.0), &[], &workspaces));
    assert_eq!(
        s.cursor_shape_at(0.0, 0.0, (100.0, 100.0), &[], &workspaces),
        Some(CursorShape::Default)
    );
}

#[test]
fn region_selection_is_a_painted_marker_not_a_surface() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
    s.update_pointer(Point { x: 220.0, y: 150.0 }, false, true);

    // A region selection frames pixels the user is about to capture. It is
    // a border plus a scrim hole, and the hole matches the staged rect
    // exactly, square corners included — nothing overlays the framed area.
    let hole = s
        .scrim_hole((800.0, 600.0), &[])
        .expect("staged rect is the hole");
    let staged = s.shown_rect().expect("selection is staged");
    assert_eq!(hole.x, staged.origin.x as f32);
    assert_eq!(hole.y, staged.origin.y as f32);
    assert_eq!(hole.w, staged.size.w as f32);
    assert_eq!(hole.h, staged.size.h as f32);
    assert_eq!(s.loupe_body_rect((800.0, 600.0)), None);
}

#[test]
fn the_pixel_loupe_is_a_painted_body_over_the_pixels_it_frames() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Pixel);
    s.update_pointer(Point { x: 400.0, y: 300.0 }, false, false);

    // The loupe is a painted body at the panel radius. It samples nothing:
    // the magnified pixels arrive as a texture the selector draws itself,
    // so the loupe's own body is a flat fill that cannot obscure the
    // reading inside it.
    let rect = s
        .loupe_body_rect((800.0, 600.0))
        .expect("the loupe has a body");
    assert_eq!(rect.w, PIXEL_LENS_SIZE);
    let design = Design::dark();
    let style = design
        .surfaces
        .for_role(tessera_design::SurfaceRole::FloatingPanel);
    assert_eq!(style.radius, design.radii.panel_lg);
    assert!(style.body.components().3 >= 236, "the loupe body is opaque");
}

#[test]
fn status_geometry_stays_inside_a_roomy_selection_and_centres_when_outside() {
    let roomy = UiRect {
        x: 100.0,
        y: 80.0,
        w: 500.0,
        h: 300.0,
    };
    assert_eq!(
        status_rect(roomy, (180.0, 32.0), (800.0, 600.0)),
        UiRect {
            x: 112.0,
            y: 92.0,
            w: 180.0,
            h: 32.0,
        }
    );

    let narrow = UiRect {
        x: 100.0,
        y: 80.0,
        w: 80.0,
        h: 40.0,
    };
    assert_eq!(
        status_rect(narrow, (180.0, 32.0), (800.0, 600.0)),
        UiRect {
            x: 50.0,
            y: 132.0,
            w: 180.0,
            h: 32.0,
        }
    );
}

#[test]
fn scrim_regions_leave_the_selection_body_undimmed() {
    let full = UiRect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    };
    let hole = UiRect {
        x: 100.0,
        y: 80.0,
        w: 200.0,
        h: 120.0,
    };
    let regions = scrim_regions(full, Some(hole));
    let area: f32 = regions.iter().map(|rect| rect.w * rect.h).sum();
    assert_eq!(area, full.w * full.h - hole.w * hole.h);
    assert!(regions.iter().all(|rect| {
        rect.x + rect.w <= hole.x
            || rect.x >= hole.x + hole.w
            || rect.y + rect.h <= hole.y
            || rect.y >= hole.y + hole.h
    }));
}

#[test]
fn rounded_scrim_corner_bands_fill_only_the_body_corner_cutouts() {
    let radius = Design::dark().radii.panel_lg;
    let bands = rounded_corner_bands(radius);
    assert!(!bands.is_empty());
    assert!(bands.windows(2).all(|pair| pair[0].inset > pair[1].inset));
    assert!((bands.iter().map(|band| band.height).sum::<f32>() - radius).abs() < 0.001);

    // Four mirrored corners should approximate the exact area between a
    // square-cornered rectangle and the matching rounded rectangle.
    let sampled_area = bands
        .iter()
        .map(|band| band.inset * band.height * 4.0)
        .sum::<f32>();
    let analytic_area = (4.0 - std::f32::consts::PI) * radius * radius;
    assert!((sampled_area - analytic_area).abs() < 1.0);
}

fn window(id: u64, x: i32, y: i32, w: i32, h: i32) -> Window {
    let mut window = Window::new(WindowId(id));
    window.position = tessera_primitives::Point { x, y };
    window.size = tessera_primitives::Size { w, h };
    window
}

#[test]
fn window_at_prefers_the_topmost_containing_window() {
    // z-order: the last window is topmost.
    let windows = vec![window(1, 0, 0, 400, 300), window(2, 100, 100, 200, 150)];
    assert_eq!(
        ScreenshotSelector::window_at(&windows, Point { x: 150.0, y: 150.0 }),
        Some(WindowId(2))
    );
    assert_eq!(
        ScreenshotSelector::window_at(&windows, Point { x: 20.0, y: 20.0 }),
        Some(WindowId(1))
    );
    assert_eq!(
        ScreenshotSelector::window_at(&windows, Point { x: 500.0, y: 20.0 }),
        None
    );
}

#[test]
fn picker_escape_emits_pick_cancelled() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Pixel);
    assert!(s.active());
    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Escape,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert!(out.pick_cancelled);
    assert!(!s.active());
}

#[test]
fn picker_region_confirm_without_selection_cancels() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Region);
    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Return,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert!(out.screenshot_region.is_none());
    assert!(out.pick_cancelled);
    assert!(!s.active());
}

#[test]
fn cancel_pick_only_closes_picker_sessions() {
    let mut s = ScreenshotSelector::new();
    s.start();
    s.cancel_pick();
    assert!(s.active(), "the Print-key session is not interrupted");
    s.reset();

    s.start_pick(PickerMode::Window);
    s.cancel_pick();
    assert!(!s.active());
}

#[test]
fn window_mode_enter_chooses_the_whole_output() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Window);
    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Return,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert!(out.pick_output);
    assert!(!s.active());
}

#[test]
fn window_mode_clicking_empty_desktop_picks_hovered_output() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Window);
    s.outputs = vec![
        test_output("HDMI-A-1", 0, 0, 1920, 1080),
        test_output("DP-1", 1920, 0, 2560, 1440),
    ];
    let cursor = Point { x: 2000.0, y: 50.0 };
    s.current = cursor;
    s.hovered = ScreenshotSelector::window_at(&[], cursor);
    let mut out = ChromeEvents::default();
    match s.hovered {
        Some(id) => out.picked_window = Some(id),
        None => {
            s.hovered_output = ScreenshotSelector::output_at(&s.outputs, cursor);
            s.confirm_output(&mut out);
        }
    }
    assert_eq!(out.picked_output.as_deref(), Some("DP-1"));
    assert!(!s.active());
}

fn test_output(
    connector: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> tessera_desktop::output::OutputInfo {
    tessera_desktop::output::OutputInfo {
        connector: connector.into(),
        geometry: tessera_desktop::output::OutputGeometry {
            mode: tessera_desktop::output::OutputMode {
                width: w,
                height: h,
                refresh_mhz: 60_000,
            },
            scale: tessera_desktop::output::Scale(1.0),
            transform: tessera_primitives::Transform::Normal,
            logical_origin: tessera_primitives::Point { x, y },
            logical_size_override: None,
        },
        kind: tessera_desktop::output::OutputKind::External,
        available_modes: Vec::new(),
        color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        role: tessera_desktop::output::OutputRole::Extend,
        role_source: tessera_desktop::output::OutputRoleSource::Auto,
    }
}

#[test]
fn output_mode_hit_tests_against_the_output_rects() {
    let outputs = vec![
        test_output("HDMI-A-1", 0, 0, 1920, 1080),
        test_output("DP-1", 1920, 0, 2560, 1440),
    ];
    assert_eq!(
        ScreenshotSelector::output_at(&outputs, Point { x: 20.0, y: 20.0 }),
        Some("HDMI-A-1".to_owned())
    );
    assert_eq!(
        ScreenshotSelector::output_at(&outputs, Point { x: 2000.0, y: 20.0 }),
        Some("DP-1".to_owned())
    );
    assert_eq!(
        ScreenshotSelector::output_at(&outputs, Point { x: 5000.0, y: 20.0 }),
        None
    );
    // Edge: the right/bottom edge is exclusive, like window hit-testing.
    assert_eq!(
        ScreenshotSelector::output_at(&outputs, Point { x: 1920.0, y: 20.0 }),
        Some("DP-1".to_owned())
    );
}

#[test]
fn output_mode_enter_emits_the_hovered_connector() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Output);
    s.outputs = vec![
        test_output("HDMI-A-1", 0, 0, 1920, 1080),
        test_output("DP-1", 1920, 0, 2560, 1440),
    ];
    s.hovered_output = Some("DP-1".to_owned());
    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Return,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert_eq!(out.picked_output.as_deref(), Some("DP-1"));
    assert!(!out.pick_output);
    assert!(!s.active());
}

#[test]
fn output_mode_enter_without_a_hover_falls_back_to_bare_output() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Output);
    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Return,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert!(out.pick_output);
    assert!(out.picked_output.is_none());
    assert!(!s.active());
}

#[test]
fn output_mode_escape_cancels() {
    let mut s = ScreenshotSelector::new();
    s.start_pick(PickerMode::Output);
    let mut out = ChromeEvents::default();
    s.key_char(
        &KeyChar {
            keysym: tessera_primitives::input::XKB_KEY_Escape,
            ch: None,
            mods: tessera_primitives::input::Mods::NONE,
        },
        &mut out,
    );
    assert!(out.pick_cancelled);
    assert!(out.picked_output.is_none());
    assert!(!s.active());
}

#[test]
fn output_mode_status_updates_refresh_the_hit_test_snapshot() {
    let mut s = ScreenshotSelector::new();
    let status = crate::SystemStatus {
        display: tessera_desktop::settings::DisplayStatus {
            outputs: vec![test_output("HDMI-A-1", 0, 0, 1920, 1080)],
            pending_revert_seconds: None,
            ..tessera_desktop::settings::DisplayStatus::default()
        },
        ..crate::SystemStatus::default()
    };
    s.update(ChromeUpdate::SystemStatus(&status));
    assert_eq!(s.outputs.len(), 1);
    assert_eq!(s.outputs[0].connector, "HDMI-A-1");
}
