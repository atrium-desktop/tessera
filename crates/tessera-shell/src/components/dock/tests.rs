use super::rendering::{
    entry_matches_app_id, hit_test_tiles, live_preview_hit, live_preview_layout, snapped_hairline,
    strip_centres,
};
use super::*;

fn app(id: &str) -> Entry {
    Entry {
        id: id.to_string(),
        ..Default::default()
    }
}

/// Register `pinned` entries the way the composition root does: one
/// catalog push that rebuilds the pinned list from [`Entry::match_keys`].
fn dock_with(pinned: Vec<Entry>) -> Dock {
    let mut dock = Dock::new();
    dock.update_app_catalog(&AppCatalog {
        apps: pinned.clone(),
        pinned,
        icons: IconSet::default(),
        position: DockPosition::Bottom,
    });
    dock
}

fn window(id: u64, app_id: &str, activated: bool) -> Window {
    let mut w = Window {
        id: tessera_desktop::window::WindowId(id),
        app_id: Some(app_id.to_string()),
        ..Default::default()
    };
    w.state.activated = activated;
    w
}

fn workspace_snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        outputs: Vec::new(),
    }
}

#[test]
fn dock_is_persistent_decoration() {
    let dock = Dock::new();
    assert!(dock.persistent_decoration());
    assert!(dock.visible_during_modal());
}

#[test]
fn magnify_factor_is_one_at_cursor() {
    assert!(Dock::magnify_factor(0.0) > 0.999);
}

#[test]
fn magnify_factor_is_zero_outside_radius() {
    let radius = MAGNIFY_RADIUS_TILES * DOCK_TILE;
    assert_eq!(Dock::magnify_factor(radius), 0.0);
    assert_eq!(Dock::magnify_factor(radius + 1.0), 0.0);
    assert_eq!(Dock::magnify_factor(-radius), 0.0);
}

#[test]
fn magnify_factor_is_symmetric() {
    for d in [5.0, 12.0, 33.0, 50.0] {
        let r = MAGNIFY_RADIUS_TILES * DOCK_TILE;
        if d < r {
            assert!((Dock::magnify_factor(d) - Dock::magnify_factor(-d)).abs() < 1e-5);
        }
    }
}

#[test]
fn spring_approaches_target_at_rest() {
    // No time elapses → nothing moves.
    let mut s = SpringState {
        value: 10.0,
        velocity: 0.0,
    };
    assert_eq!(Dock::spring(&mut s, 20.0, 0.0, false), 10.0);
}

#[test]
fn spring_settles_on_target() {
    // Many small steps from rest must converge to the target.
    let mut s = SpringState {
        value: 10.0,
        velocity: 0.0,
    };
    for _ in 0..2000 {
        Dock::spring(&mut s, 20.0, 1.0 / 120.0, false);
    }
    assert!((s.value - 20.0).abs() < 0.01, "settled at {}", s.value);
}

#[test]
fn spring_overshoots_then_settles() {
    // Under-damped: from rest it should cross past the target at least once
    // before settling (the macOS lift-and-bounce).
    let mut s = SpringState {
        value: 0.0,
        velocity: 0.0,
    };
    let mut overshot = false;
    for _ in 0..2000 {
        Dock::spring(&mut s, 100.0, 1.0 / 120.0, false);
        if s.value > 100.0 {
            overshot = true;
        }
    }
    assert!(overshot, "spring never overshot the target");
    assert!((s.value - 100.0).abs() < 0.01, "settled at {}", s.value);
}

#[test]
fn spring_is_dt_stable() {
    // A single large step (a long frame stall) must not blow up.
    let mut s = SpringState {
        value: 0.0,
        velocity: 0.0,
    };
    let v = Dock::spring(&mut s, 100.0, 1.0 / 5.0, false);
    assert!(v.is_finite(), "value diverged: {v}");
    assert!(s.velocity.is_finite(), "velocity diverged: {}", s.velocity);
}

#[test]
fn spring_remains_bounded_and_settles_at_thirty_fps() {
    let mut s = SpringState {
        value: DOCK_TILE,
        velocity: 0.0,
    };
    for _ in 0..300 {
        Dock::spring(&mut s, DOCK_TILE_MAX, 1.0 / 30.0, false);
        assert!(
            s.value >= 0.0 && s.value <= DOCK_TILE_MAX * 2.0,
            "spring escaped its visual range: {}",
            s.value
        );
    }
    assert!((s.value - DOCK_TILE_MAX).abs() < 0.01);
    assert!(s.velocity.abs() < 0.01);
}

#[test]
fn pinned_apps_show_without_any_running_window() {
    let dock = dock_with(vec![app("firefox.desktop"), app("term.desktop")]);
    let tiles = dock.tiles(&[]);
    assert_eq!(
        tiles.len(),
        2,
        "both pinned apps are tiles even with no windows"
    );
    assert!(tiles.iter().all(|t| !t.running));
    // No running window → clicking launches (spawn), not focus.
    assert!(tiles.iter().all(|t| t.spawn.is_some() && t.focus.is_none()));
}

#[test]
fn running_window_folds_into_its_pinned_tile() {
    let dock = dock_with(vec![app("firefox.desktop")]);
    let tiles = dock.tiles(&[window(7, "firefox", true)]);
    assert_eq!(
        tiles.len(),
        1,
        "the window folds into the pinned tile, not a new one"
    );
    assert!(tiles[0].running);
    assert!(tiles[0].activated);
    assert_eq!(
        tiles[0].focus,
        Some(tessera_desktop::window::WindowId(7)),
        "clicking focuses the running window"
    );
    assert!(tiles[0].spawn.is_none());
}

#[test]
fn multiple_running_windows_fold_into_one_tile_with_multiple_instances() {
    let dock = dock_with(vec![app("firefox.desktop")]);
    let tiles = dock.tiles(&[window(7, "firefox", true), window(8, "firefox", false)]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].windows.len(), 2);
}

#[test]
fn unpinned_running_window_is_appended() {
    let dock = dock_with(vec![app("firefox.desktop")]);
    let tiles = dock.tiles(&[window(3, "gimp", false)]);
    assert_eq!(
        tiles.len(),
        2,
        "pinned firefox plus the unpinned gimp window"
    );
    let gimp = tiles
        .iter()
        .find(|t| t.key == "transient:gimp")
        .expect("gimp tile");
    assert!(gimp.running);
    assert!(!gimp.pinned, "the window tile is transient, not kept");
    assert_eq!(gimp.focus, Some(tessera_desktop::window::WindowId(3)));
}

#[test]
fn tile_strip_uses_the_workspace_global_window_list() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    let visible = window(7, "firefox", true);
    let hidden = window(3, "gimp", false);

    // The visible-set push feeds only the autohide/SpaceUse policy; the strip
    // must not grow from it.
    dock.update(ChromeUpdate::Windows(std::slice::from_ref(&visible)));
    assert_eq!(
        dock.tiles(&dock.all_windows).len(),
        1,
        "only the pinned app before any global snapshot"
    );

    // The workspace-global push builds the strip: the pinned tile folds its
    // running window and a window on another workspace gets a transient tile.
    dock.update(ChromeUpdate::AllWindows(&[visible.clone(), hidden]));
    let tiles = dock.tiles(&dock.all_windows);
    assert_eq!(tiles.len(), 2);
    let firefox = tiles
        .iter()
        .find(|t| t.key == "app:firefox.desktop")
        .unwrap();
    assert!(firefox.running && firefox.activated);
    let gimp = tiles.iter().find(|t| t.key == "transient:gimp").unwrap();
    assert!(gimp.running && !gimp.pinned);
    assert_eq!(gimp.focus, Some(tessera_desktop::window::WindowId(3)));

    // A later visible-set push without the hidden window must not drop its
    // tile; only the global list owns strip membership.
    dock.update(ChromeUpdate::Windows(std::slice::from_ref(&visible)));
    assert!(
        dock.tiles(&dock.all_windows)
            .iter()
            .any(|t| t.key == "transient:gimp")
    );

    // The transient tile leaves when the window leaves the global list.
    dock.update(ChromeUpdate::AllWindows(std::slice::from_ref(&visible)));
    assert!(
        !dock
            .tiles(&dock.all_windows)
            .iter()
            .any(|t| t.key == "transient:gimp")
    );
}

#[test]
fn scale_update_is_retained_for_hairline_snapping() {
    let mut dock = Dock::new();
    assert_eq!(dock.scale, 1.0);
    dock.update(ChromeUpdate::Scale(2.0));
    assert_eq!(dock.scale, 2.0);
}

#[test]
fn hairline_snaps_to_the_device_pixel_grid() {
    // Scale 2: a fractional center lands on a device-pixel boundary and the
    // width is exactly one device pixel.
    let (center, width) = snapped_hairline(100.3, 2.0);
    assert_eq!((center, width), (100.5, 0.5));
    // Scale 1: a 1 logical px line at (within half a pixel of) the same
    // center — the pre-snap appearance.
    let (center, width) = snapped_hairline(100.3, 1.0);
    assert_eq!((center, width), (100.0, 1.0));
    // An already-aligned center is untouched.
    let (center, width) = snapped_hairline(100.0, 2.0);
    assert_eq!((center, width), (100.0, 0.5));
}

#[test]
fn unpinned_windows_keep_open_order_when_focus_reorders_snapshot() {
    let dock = Dock::new();
    let first = dock.tiles(&[window(1, "first", false), window(2, "second", true)]);
    assert_eq!(
        first
            .iter()
            .map(|tile| tile.key.as_str())
            .collect::<Vec<_>>(),
        vec!["transient:first", "transient:second"]
    );

    let reordered = dock.tiles(&[window(2, "second", false), window(1, "first", true)]);
    assert_eq!(
        reordered
            .iter()
            .map(|tile| tile.key.as_str())
            .collect::<Vec<_>>(),
        vec!["transient:first", "transient:second"],
        "focus/stacking order must not move a transient Dock tile"
    );
    assert!(reordered[0].activated);
    assert!(!reordered[1].activated);

    let after_close_and_open = dock.tiles(&[window(3, "third", true), window(2, "second", false)]);
    assert_eq!(
        after_close_and_open
            .iter()
            .map(|tile| tile.key.as_str())
            .collect::<Vec<_>>(),
        vec!["transient:second", "transient:third"],
        "a newly opened application is appended after surviving transient tiles"
    );
}

#[test]
fn unpinned_windows_of_one_app_fold_into_one_tile() {
    let dock = Dock::new();
    let tiles = dock.tiles(&[
        window(1, "gimp", false),
        window(2, "gimp", true),
        window(3, "other", false),
    ]);
    assert_eq!(tiles.len(), 2, "one tile per application, pinned or not");
    let gimp = &tiles[0];
    assert_eq!(gimp.key, "transient:gimp");
    assert!(gimp.running && gimp.activated);
    assert_eq!(
        gimp.windows,
        vec![
            tessera_desktop::window::WindowId(2),
            tessera_desktop::window::WindowId(1)
        ],
        "the activated window leads the group"
    );
    assert_eq!(gimp.focus, Some(tessera_desktop::window::WindowId(2)));
    assert_eq!(gimp.pin_entry, None, "no desktop entry in the catalog");
    assert!(!tiles[1].activated);
}

#[test]
fn unpinned_app_resolves_its_desktop_entry_for_pinning() {
    let mut dock = Dock::new();
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("gimp.desktop")],
        pinned: vec![],
        icons: IconSet::default(),
        position: DockPosition::Bottom,
    });
    let tiles = dock.tiles(&[window(1, "Gimp", false)]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].key, "transient:gimp.desktop");
    assert_eq!(tiles[0].pin_entry.as_deref(), Some("gimp.desktop"));
}

#[test]
fn a_window_without_app_id_stands_alone() {
    let dock = Dock::new();
    let mut lone = window(1, "gimp", false);
    lone.app_id = None;
    lone.title = Some("Untitled".to_string());
    let tiles = dock.tiles(&[lone]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].key, "win:1");
    assert_eq!(tiles[0].pin_entry, None);
    assert_eq!(tiles[0].label, "Untitled");
}

#[test]
fn pointer_bounds_stay_at_rest_while_tiles_are_magnified() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    dock.sizes.insert(
        "launchpad".into(),
        SpringState {
            value: DOCK_TILE_MAX,
            velocity: 0.0,
        },
    );
    dock.sizes.insert(
        "app:firefox.desktop".into(),
        SpringState {
            value: DOCK_TILE_MAX,
            velocity: 0.0,
        },
    );

    let display = (1920.0, 1080.0);
    let bounds = dock.pointer_bounds(display);
    let visual_bounds = dock.visual_panel_bounds(display);
    // The bar leads with its reading section, so its width is that section plus
    // the icon strip — not the icons alone. The expected value comes from the
    // strip's single length source: a hand-rolled sum here is how a newly added
    // reading silently falls out of the assertion.
    let cells = dock.strip_cells();
    let expected_width = crate::components::dock::bento::strip_length(
        [DOCK_TILE; 2],
        &cells,
        DOCK_TILE_GAP,
        DOCK_PAD,
    );
    let expected_visual = crate::components::dock::bento::strip_length(
        [DOCK_TILE_MAX; 2],
        &cells,
        DOCK_TILE_GAP,
        DOCK_PAD,
    );
    assert_eq!(bounds.w, expected_width);
    assert_eq!(visual_bounds.w, expected_visual);
    assert_eq!(bounds.y, display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
    assert_eq!(bounds.h, DOCK_PANEL_HEIGHT);
    assert!(!dock.captures_pointer(
        display.0 * 0.5,
        bounds.y - 1.0,
        display,
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn read_only_mirror_has_no_physical_focus_action() {
    let dock = Dock::new();
    let mut mirror = window(7, "org.example.App", false);
    mirror.read_only = true;
    let tiles = dock.tiles(&[mirror]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].focus, None);
    assert_eq!(tiles[0].windows, vec![tessera_desktop::window::WindowId(7)]);
}

#[test]
fn releasing_output_ownership_restores_a_dock_that_was_obscured_before_it() {
    // Regression for the ownership split: a window that overlaps the Dock and
    // then takes the output must not leave a frozen `dock_obscured` behind,
    // because the release path in `set_space_use` refuses to restore while the
    // flag is set. Production order is ownership first, then the window
    // snapshot.
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut game = window(7, "org.example.Game", true);
    let bounds = dock.pointer_bounds(display);
    game.position = tessera_primitives::Point {
        x: bounds.x.round() as i32 + 10,
        y: bounds.y.round() as i32 - 10,
    };
    game.size = tessera_primitives::Size { w: 120, h: 40 };
    dock.update_windows(std::slice::from_ref(&game));
    assert!(dock.dock_obscured, "the window starts on top of the Dock");

    // The client takes the whole output (a bordered-less game growing to the
    // monitor), then releases it.
    dock.apply_space_use(SpaceUse::Fullscreen);
    dock.update_windows(std::slice::from_ref(&game));
    assert!(!dock.dock_obscured, "ownership clears obscuration");
    assert!(dock.fullscreen_locked());

    dock.apply_space_use(SpaceUse::Available);
    assert!(!dock.fullscreen_locked());
    assert!(!dock.effective_autohide(), "the Dock comes back");
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
    );
}

#[test]
fn window_invading_rest_bounds_starts_an_animated_collapse() {
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut invading = window(7, "org.example.Game", true);
    let bounds = dock.pointer_bounds(display);
    invading.position = tessera_primitives::Point {
        x: bounds.x.round() as i32 + 10,
        y: bounds.y.round() as i32 - 10,
    };
    invading.size = tessera_primitives::Size { w: 120, h: 40 };
    dock.update_windows(std::slice::from_ref(&invading));

    assert!(dock.dock_obscured);
    assert!(dock.effective_autohide());
    assert_eq!(
        dock.autohide_reveal(),
        1.0,
        "collision starts an animation instead of teleporting the Dock"
    );
    assert!(dock.collapse_pending);
    assert!(!dock.hidden_trigger_armed);
    assert_eq!(dock.reserved(), Reserved::default());

    invading.position.y = bounds.y.round() as i32 - 80;
    invading.size.h = 40;
    dock.update_windows(&[invading]);
    assert!(!dock.dock_obscured);
    assert!(!dock.effective_autohide());
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
    );
}

#[test]
fn maximized_state_forces_an_animated_collapse_without_geometric_invasion() {
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut maximized = window(7, "org.example.Game", true);
    maximized.state.maximized = true;
    maximized.position = tessera_primitives::Point { x: 100, y: 100 };
    maximized.size = tessera_primitives::Size { w: 1000, h: 700 };
    dock.update_windows(std::slice::from_ref(&maximized));
    dock.apply_space_use(SpaceUse::Maximized);

    assert_eq!(dock.space_use, SpaceUse::Maximized);
    assert!(!dock.dock_obscured);
    assert!(dock.effective_autohide());
    assert_eq!(dock.autohide_reveal(), 1.0);
    assert!(dock.collapse_pending);
    assert!(!dock.hidden_trigger_armed);
    assert!(dock.anim_pending());
    assert_eq!(dock.reserved(), Reserved::default());

    let mut restored = maximized;
    restored.state.maximized = false;
    dock.update_windows(&[restored]);
    dock.apply_space_use(SpaceUse::Available);
    assert_eq!(dock.space_use, SpaceUse::Available);
    assert!(!dock.effective_autohide());
    assert!(!dock.collapse_pending);
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
    );
}

#[test]
fn disabling_user_autohide_does_not_override_maximized_collapse() {
    let mut dock = Dock::new();
    let mut maximized = window(7, "org.example.Game", true);
    maximized.state.maximized = true;
    dock.update_windows(&[maximized]);
    dock.apply_space_use(SpaceUse::Maximized);
    dock.motion.reveal = SpringState::new(0.0);

    dock.set_autohide(true);
    dock.set_autohide(false);

    assert!(dock.effective_autohide());
    assert_eq!(dock.autohide_reveal(), 0.0);
    assert_eq!(dock.reserved(), Reserved::default());
}

#[test]
fn minimized_window_does_not_count_as_a_dock_invasion() {
    let bounds = Dock::rest_bounds(1, 1, DockPosition::Bottom, (1920.0, 1080.0));
    let mut minimized = window(7, "org.example.Game", false);
    minimized.position = tessera_primitives::Point {
        x: bounds.x.round() as i32,
        y: bounds.y.round() as i32,
    };
    minimized.size = tessera_primitives::Size { w: 100, h: 50 };
    minimized.minimized = true;
    assert!(!Dock::window_overlaps_bounds(&minimized, bounds));
}

#[test]
fn dock_surface_morphs_between_panel_and_exact_handle_geometry() {
    let display = (1920.0, 1080.0);
    let expanded_width = 320.0;
    let expanded = Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, 1.0);
    assert_eq!(expanded.w, expanded_width);
    assert_eq!(expanded.h, DOCK_PANEL_HEIGHT);
    assert_eq!(expanded.y, display.1 - DOCK_EDGE_MARGIN - DOCK_PANEL_HEIGHT);

    let collapsed = Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, 0.0);
    assert_eq!(collapsed.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(collapsed.h, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(
        collapsed.y,
        display.1 - AUTOHIDE_HANDLE_EDGE_MARGIN - AUTOHIDE_HANDLE_HEIGHT
    );
}

#[test]
fn both_dock_states_hold_the_same_distance_from_the_screen_edge() {
    let display = (1920.0, 1080.0);
    let expanded_width = 320.0;
    let expanded = Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, 1.0);
    let collapsed = Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, 0.0);

    // The dock is one surface at two sizes, so it holds one offset: it grows and
    // shrinks in place instead of sliding toward the edge as it parks. Two
    // independent constants let the collapsed and expanded geometry drift apart,
    // which is exactly what this pins shut.
    assert_eq!(
        display.1 - (collapsed.y + collapsed.h),
        DOCK_EDGE_MARGIN,
        "the collapsed capsule holds the bar's float"
    );
    assert_eq!(
        display.1 - (expanded.y + expanded.h),
        DOCK_EDGE_MARGIN,
        "the expanded bar holds it too"
    );
    assert_eq!(
        AUTOHIDE_HANDLE_EDGE_MARGIN, DOCK_EDGE_MARGIN,
        "the two names resolve to one value"
    );

    // Because the offset is constant, the bottom edge does not move at all
    // across the morph: only the top edge travels as the surface changes size.
    let mut previous_bottom = f32::MAX;
    for step in 0..=20 {
        let reveal = step as f32 / 20.0;
        let rect =
            Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, reveal);
        let bottom = rect.y + rect.h;
        assert!(
            (bottom - previous_bottom).abs() <= f32::EPSILON || previous_bottom == f32::MAX,
            "the anchored edge must not move during the morph: {bottom} vs {previous_bottom}"
        );
        previous_bottom = bottom;
    }
    assert_eq!(previous_bottom, expanded.y + expanded.h);
}

#[test]
fn the_edge_margin_is_constant_across_the_morph() {
    // One answer for every reveal, including a spring overshoot past either
    // endpoint: the body cannot leave its offset.
    for progress in [-0.5, 0.0, 0.25, 0.5, 0.75, 1.0, 1.5] {
        assert_eq!(
            edge_margin_for_surface_progress(progress),
            DOCK_EDGE_MARGIN,
            "reveal {progress} holds the dock's one offset"
        );
    }
}

#[test]
fn the_reveal_corridor_still_reaches_the_physical_screen_edge() {
    // The pointer target and the painted body are separate concerns: the capsule
    // now floats clear of the edge, and a pointer thrown at the screen must
    // still summon it without being aimed.
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        let display = (1920.0, 1080.0);
        let corridor = Dock::collapsed_reveal_corridor(position, display);
        let visual = Dock::collapsed_indicator_bounds(position, display);
        match position {
            DockPosition::Bottom => {
                assert_eq!(
                    corridor.y + corridor.h,
                    display.1,
                    "the corridor reaches the bottom screen edge"
                );
                assert!(
                    corridor.y < visual.y,
                    "the corridor extends above the floating capsule"
                );
            }
            DockPosition::Left => {
                assert_eq!(corridor.x, 0.0, "the corridor reaches the left screen edge");
                assert!(corridor.x < visual.x);
            }
            DockPosition::Right => {
                assert_eq!(
                    corridor.x + corridor.w,
                    display.0,
                    "the corridor reaches the right screen edge"
                );
                assert!(corridor.x + corridor.w > visual.x + visual.w);
            }
        }
    }
}

#[test]
fn dock_content_finishes_draining_before_surface_becomes_handle() {
    assert_eq!(Dock::collapse_content_progress(1.0), 1.0);
    assert_eq!(
        Dock::collapse_content_progress(AUTOHIDE_CONTENT_DRAIN_END),
        0.0
    );
    assert_eq!(Dock::collapse_content_progress(0.0), 0.0);
    assert!(
        Dock::collapse_surface_progress(AUTOHIDE_CONTENT_DRAIN_END) > 0.0,
        "the surface must still be visibly morphing after its icons drain"
    );
}

#[test]
fn collapsed_handle_has_no_tile_hover_target() {
    let display = (1920.0, 1080.0);
    let rest_bounds = Dock::rest_bounds(1, 1, DockPosition::Bottom, display);
    let panel_rect = Dock::collapsed_panel_rect(DockPosition::Bottom, display, rest_bounds.w, 0.0);
    let old_icon_rect = Rect {
        x: display.0 * 0.5 - DOCK_TILE * 0.5,
        y: rest_bounds.y + 5.0,
        w: DOCK_TILE,
        h: DOCK_TILE,
    };
    let cursor = (display.0 * 0.5, old_icon_rect.y + old_icon_rect.h * 0.5);

    assert_eq!(
        hit_test_tiles(
            cursor,
            rest_bounds,
            rest_bounds,
            1.0,
            &[old_icon_rect],
            false,
            None,
        ),
        Some(0),
        "the same point owns the visible tile while the Dock is expanded"
    );
    assert!(
        hit_test_tiles(
            cursor,
            rest_bounds,
            panel_rect,
            Dock::collapse_content_progress(0.0),
            &[old_icon_rect],
            false,
            None,
        )
        .is_none()
    );

    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    assert!(!dock.captures_pointer(cursor.0, cursor.1, display, &[], &workspace_snapshot(),));
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    assert!(dock.captures_pointer(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
        display,
        &[],
        &workspace_snapshot(),
    ));
    // Physical bottom edge directly under the capsule corridor captures pointer to eliminate edge dead zone.
    assert!(dock.captures_pointer(
        display.0 * 0.5,
        display.1 - 1.0,
        display,
        &[],
        &workspace_snapshot(),
    ));
    // Bottom edge laterally outside the capsule corridor remains client-owned.
    assert!(!dock.captures_pointer(
        indicator.x - 20.0,
        display.1 - 1.0,
        display,
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn capsule_is_the_only_collapsed_reveal_target() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let corridor = Dock::collapsed_reveal_corridor(DockPosition::Bottom, display);
    let workspaces = workspace_snapshot();

    assert!(dock.captures_pointer(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
        display,
        &[],
        &workspaces,
    ));
    // Corridor extends to physical edge:
    assert!(dock.captures_pointer(
        indicator.x + indicator.w * 0.5,
        display.1 - 1.0,
        display,
        &[],
        &workspaces,
    ));
    // The reveal corridor reaches past the floating capsule to the physical
    // edge, so the reachable strip is a deliberate band rather than an aim at a
    // 4 px pill hovering 12 px clear of the screen.
    assert!(
        corridor.y < indicator.y,
        "the corridor must reach above the capsule's float"
    );
    assert!(dock.captures_pointer(
        indicator.x + indicator.w * 0.5,
        corridor.y + 1.0,
        display,
        &[],
        &workspaces,
    ));
    // Points outside the corridor corridor (left, right, above, or distant bottom edge) remain client-owned:
    for point in [
        (indicator.x - 1.0, indicator.y + indicator.h * 0.5),
        (
            indicator.x + indicator.w + 1.0,
            indicator.y + indicator.h * 0.5,
        ),
        (indicator.x + indicator.w * 0.5, corridor.y - 1.0),
        (indicator.x - 20.0, display.1 - 1.0),
        (indicator.x + indicator.w + 20.0, display.1 - 1.0),
    ] {
        assert!(
            !dock.captures_pointer(point.0, point.1, display, &[], &workspaces),
            "point {point:?} outside the capsule corridor must remain client-owned"
        );
    }
}

#[test]
fn tucked_capsule_keeps_its_reveal_corridor_deeper_than_the_capsule() {
    let display = (1920.0, 1080.0);
    let corridor = Dock::collapsed_reveal_corridor(DockPosition::Bottom, display);
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);

    // ADR-0146's dwell verification tests against this rectangle, so its depth
    // is a first-class contract: the capsule can be summoned by reaching the
    // bottom band, not by aiming at a 4 px pill. The depth is the union of the
    // edge float, the capsule body, and the approach band — a flat band would be
    // shallower than the capsule it owns once that capsule floats clear.
    assert_eq!(display.1 - corridor.y, collapsed_reveal_depth());
    assert_eq!(
        collapsed_reveal_depth(),
        DOCK_EDGE_MARGIN + AUTOHIDE_HANDLE_HEIGHT + AUTOHIDE_HANDLE_APPROACH_BAND
    );
    assert!(corridor.w >= indicator.w);
    assert!(
        corridor.y < indicator.y,
        "the corridor is deeper than the capsule"
    );
    assert!(corridor.y + corridor.h <= display.1);
    assert_eq!(corridor.x, indicator.x);
    assert_eq!(corridor.w, indicator.w);

    // It stays shallow enough to leave client content just above the parked
    // Dock clickable — the trapping failure ADR-0146 exists to prevent.
    const {
        assert!(
            collapsed_reveal_depth() < DOCK_PANEL_HEIGHT,
            "the corridor must not span a resting bar's depth"
        );
    }

    // Side docks get the same contract on their own axis, with the capsule's
    // long axis preserved.
    let left = Dock::collapsed_reveal_corridor(DockPosition::Left, display);
    assert_eq!(left.w, collapsed_reveal_depth());
    assert_eq!(left.x, 0.0);
    let right = Dock::collapsed_reveal_corridor(DockPosition::Right, display);
    assert_eq!(right.x + right.w, display.0);
    assert_eq!(right.w, collapsed_reveal_depth());
}

#[test]
fn collapsed_dock_does_not_reuse_its_old_resting_region_as_a_trigger() {
    let display = (1920.0, 1080.0);
    let dock = Dock::new();
    let rest = dock.pointer_bounds(display);
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let old_dock_point = (rest.x + rest.w * 0.5, rest.y + 8.0);
    let capsule_point = (
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );

    assert!(!Dock::collapsed_indicator_contains(
        DockPosition::Bottom,
        old_dock_point,
        display
    ));
    let collapsed_panel = Dock::collapsed_panel_rect(DockPosition::Bottom, display, rest.w, 0.0);
    assert!(!Dock::pointer_keeps_revealed(
        true,
        0.0,
        false,
        old_dock_point,
        collapsed_panel,
        rest,
        DockPosition::Bottom,
        display,
    ));
    assert!(Dock::pointer_keeps_revealed(
        true,
        0.0,
        true,
        capsule_point,
        collapsed_panel,
        rest,
        DockPosition::Bottom,
        display,
    ));
    assert!(Dock::pointer_keeps_revealed(
        true,
        1.0,
        false,
        old_dock_point,
        rest,
        rest,
        DockPosition::Bottom,
        display,
    ));
}

#[test]
fn expanded_autohide_dock_keeps_pointer_across_bottom_gap() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![
        app("org.example.Editor.desktop"),
        app("org.example.Terminal.desktop"),
    ]);
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(1.0);
    let rest = dock.pointer_bounds(display);
    let gap_y = rest.y + rest.h + DOCK_EDGE_MARGIN * 0.5;
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);

    assert!(Dock::expanded_trigger_contains(
        DockPosition::Bottom,
        (display.0 * 0.5, gap_y),
        rest,
        display,
    ));
    assert!(dock.captures_pointer(display.0 * 0.5, gap_y, display, &[], &workspace_snapshot(),));
    // The bar leads with its reading cells, so a hardcoded x from the icon-only
    // layout is now *inside* the bar. The invariant the test exists for is that
    // the pointer stops at the bar's edge, so the probe is one pixel past it —
    // and the capsule, being narrower, is inside that edge too.
    assert!(
        indicator.x > rest.x,
        "the expanded bar reaches further left than the collapsed capsule"
    );
    assert!(!dock.captures_pointer(
        rest.x - 1.0,
        indicator.y + indicator.h * 0.5,
        display,
        &[],
        &workspace_snapshot(),
    ));
    assert!(!Dock::expanded_trigger_contains(
        DockPosition::Bottom,
        (rest.x - 1.0, gap_y),
        rest,
        display,
    ));
}

#[test]
fn fullscreen_window_locks_dock_hidden_without_hot_edge() {
    let mut dock = Dock::new();
    dock.apply_space_use(SpaceUse::Fullscreen);

    assert_eq!(dock.space_use, SpaceUse::Fullscreen);
    assert_eq!(dock.autohide_reveal(), 0.0);
    assert_eq!(dock.reserved(), Reserved::default());
    assert!(!dock.requires_composition());
    assert!(!dock.captures_pointer(960.0, 1079.0, (1920.0, 1080.0), &[], &workspace_snapshot(),));
}

#[test]
fn the_dock_body_is_one_painted_strip_inside_a_stable_footprint() {
    let dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let display = (1920.0, 1080.0);

    // The body is painted by the dock itself. Its footprint is the stable
    // envelope the damage path repaints, so reveal and magnification morphs
    // never force a full-output repaint.
    let (bounds, material) = dock
        .body_material(display)
        .expect("a settled dock paints a body");
    let footprint = dock.capture_footprint(display);
    assert!(footprint.x <= bounds.x && footprint.y <= bounds.y);
    assert!(footprint.x + footprint.w >= bounds.x + bounds.w);
    assert!(footprint.y + footprint.h >= bounds.y + bounds.h);
    assert_eq!(material.radius, Design::dark().radii.panel_lg);
    let style = Design::dark().surfaces.for_role(SurfaceRole::Dock);
    assert!(material.bg.components().3 >= 236, "the bar body is opaque");
    assert!(style.shadow_alpha > 0.0, "the bar casts a shadow");
}

#[test]
fn the_collapsed_capsule_is_a_painted_pill_at_rest() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    let display = (1920.0, 1080.0);

    assert!(dock.requires_composition());
    let (bounds, material) = dock
        .body_material(display)
        .expect("the resting capsule is still a body");
    // The resting capsule is a small pill, not a flat hairline: a control
    // sitting on the screen edge has to read as a control.
    assert_eq!(bounds.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(bounds.h, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(material.radius, AUTOHIDE_HANDLE_HEIGHT * 0.5);
    // The resting capsule takes the palette's collapsed tone. The dwell glow
    // is a separate micro-interaction, so it is cleared here to pin the rest
    // value itself.
    dock.autohide_dwell = 0.0;
    let (_, material) = dock.body_material(display).expect("resting body");
    assert_eq!(material.bg, Design::dark().dock.bar_surface_collapsed);
    assert!(material.bg.components().3 >= 236, "the capsule is opaque");
}

#[test]
fn settled_maximized_collapse_keeps_the_capsule_painted() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let mut maximized = window(7, "org.example.Editor", true);
    maximized.state.maximized = true;
    dock.update_windows(std::slice::from_ref(&maximized));
    dock.apply_space_use(SpaceUse::Maximized);
    dock.motion.reveal = SpringState::new(0.0);
    dock.collapse_pending = false;

    assert_eq!(dock.space_use, SpaceUse::Maximized);
    assert!(dock.requires_composition());
    let display = (1920.0, 1080.0);
    let (rest, _) = dock.body_material(display).expect("the capsule is painted");
    assert_eq!(rest.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(rest.h, AUTOHIDE_HANDLE_HEIGHT);

    dock.motion.reveal = SpringState::new(0.25);
    assert!(dock.requires_composition());
    // Mid-reveal the body morphs inside one stable footprint, so no frame of
    // the morph paints outside what the damage path already covers.
    let (morphing, _) = dock.body_material(display).expect("mid-reveal body");
    let footprint = dock.capture_footprint(display);
    assert!(footprint.w >= AUTOHIDE_HANDLE_WIDTH);
    assert!(footprint.x <= morphing.x && footprint.y <= morphing.y);
    assert!(footprint.x + footprint.w >= morphing.x + morphing.w);
    assert!(footprint.y + footprint.h >= morphing.y + morphing.h);
    // The envelope spans from the bar's float down to the capsule's tuck, and
    // reaches inward far enough for a fully magnified tile. No frame of the
    // morph or of the hover wave paints outside its own footprint
    // (ADR-0171, ADR-0225).
    assert!(
        footprint.h > DOCK_PANEL_HEIGHT + (DOCK_EDGE_MARGIN - AUTOHIDE_HANDLE_EDGE_MARGIN),
        "the envelope must include the magnified-tile overshoot: {}",
        footprint.h
    );
    assert_eq!(
        footprint.y + footprint.h,
        display.1 - AUTOHIDE_HANDLE_EDGE_MARGIN,
        "the envelope must reach the collapsed capsule's edge tuck"
    );
}

/// Moving the pointer over the dock repaints the dock's own band, not the
/// output.
///
/// A pointer move has no pixel footprint of its own; it matters only because
/// chrome under the cursor reacts to it. The dock localizes everything it can
/// react with — magnified tiles, the name bubble, live previews, the context
/// menu — so a mouse crossing the bar must not become a full-output composite.
#[test]
fn pointer_over_the_dock_localizes_to_its_own_band() {
    let display = (1920.0, 1080.0);
    let dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let workspaces = workspace_snapshot();
    let bar = dock.pointer_bounds(display);
    let regions = dock
        .pointer_damage_regions(
            bar.x + bar.w * 0.5,
            bar.y + bar.h * 0.5,
            display,
            &[],
            &workspaces,
        )
        .expect("the dock states its hover footprint");
    assert!(!regions.is_empty(), "a visible dock owns pixels");
    for region in &regions {
        assert!(
            region.y >= display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN - 64.0,
            "every hover region sits on the dock's edge, not mid-screen: {region:?}"
        );
    }
}

/// A held drag is not bounded by the strip: it previews across the bar and an
/// edge drag travels to another screen edge. That keeps the conservative
/// full-output answer rather than a claim the dock cannot honour.
#[test]
fn a_dragging_dock_refuses_to_bound_its_own_damage() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let workspaces = workspace_snapshot();
    dock.press = Some(PressState {
        origin: (960.0, 1040.0),
        target: PressTarget::Panel,
        dragging: true,
        section: DropSection::Pinned,
        insert: None,
        start_position: DockPosition::Bottom,
    });
    assert!(
        dock.pointer_damage_regions(960.0, 1040.0, display, &[], &workspaces)
            .is_none(),
        "an in-flight drag keeps the full-output verdict"
    );
}

#[test]
fn backdrop_prepass_reveals_only_from_the_capsule_body() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let mut maximized = window(7, "org.example.Editor", true);
    maximized.state.maximized = true;
    dock.update_windows(std::slice::from_ref(&maximized));
    dock.apply_space_use(SpaceUse::Maximized);
    dock.motion.reveal = SpringState::new(0.0);
    dock.collapse_pending = false;
    dock.hidden_trigger_armed = true;
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();

    // The bottom edge laterally outside the visible capsule corridor remains client-owned.
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let mut outside = Input::new(display, 0.016);
    outside.set_cursor(indicator.x - 30.0, display.1 - 1.0);
    dock.prepare_frame(&outside, std::slice::from_ref(&maximized), &workspaces);
    assert_eq!(dock.autohide_idle, dock.autohide_timeout);
    assert!(dock.requires_composition());

    // Skimming the capsule corridor for a single brief frame (e.g. 16ms transit) keeps
    // dwell pending without prematurely triggering dock expansion.
    let mut capsule = Input::new(display, 0.016);
    capsule.set_cursor(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );
    dock.prepare_frame(&capsule, std::slice::from_ref(&maximized), &workspaces);
    assert!(
        dock.anim_pending(),
        "dwell accumulation keeps animation ticking"
    );
    assert_eq!(
        dock.autohide_idle, dock.autohide_timeout,
        "brief transit skim must not prematurely start reveal"
    );

    // Once pointer dwell satisfies the intent threshold, reveal formally engages.
    let mut sustained_capsule = Input::new(display, AUTOHIDE_DWELL_THRESHOLD);
    sustained_capsule.set_cursor(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );
    dock.prepare_frame(&sustained_capsule, &[maximized], &workspaces);
    assert_eq!(
        dock.autohide_idle, 0.0,
        "sustained dwell confirms reveal intent"
    );
    assert!(dock.anim_pending());
}

#[test]
fn running_app_preview_layout_exposes_every_window_inside_the_output() {
    let owner = lens::Rect {
        x: 910.0,
        y: 980.0,
        w: 84.0,
        h: 84.0,
    };
    let windows: Vec<_> = (1..=7).map(tessera_desktop::window::WindowId).collect();
    let presentation = live_preview_layout(
        &Design::dark(),
        (1920.0, 1080.0),
        owner,
        &windows,
        1.0,
        DockPosition::Bottom,
    );

    assert_eq!(presentation.cards.len(), windows.len());
    assert!(presentation.panel.origin.x >= PREVIEW_SCREEN_MARGIN as i32);
    assert!(presentation.panel.origin.y >= PREVIEW_SCREEN_MARGIN as i32);
    assert!(presentation.panel.origin.x + presentation.panel.size.w <= 1920);
    for card in &presentation.cards {
        assert!(windows.contains(&card.window));
        assert_eq!(card.geometry.preview.size.w, card.geometry.outer.size.w);
        assert_eq!(
            card.geometry.preview.size.h + card.geometry.label.size.h,
            card.geometry.outer.size.h
        );
    }

    for card in &presentation.cards {
        let centre_x =
            card.geometry.outer.origin.x as f32 + card.geometry.outer.size.w as f32 * 0.5;
        let centre_y =
            card.geometry.outer.origin.y as f32 + card.geometry.outer.size.h as f32 * 0.5;
        assert_eq!(
            live_preview_hit(&presentation, centre_x, centre_y),
            Some(card.window),
            "each preview card resolves to its own focus target"
        );
    }
}

#[test]
fn live_preview_adds_one_panel_body_and_keeps_its_pointer_bridge() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let owner = lens::Rect {
        x: 900.0,
        y: 970.0,
        w: 84.0,
        h: 84.0,
    };
    let presentation = live_preview_layout(
        &Design::dark(),
        (1920.0, 1080.0),
        owner,
        &[
            tessera_desktop::window::WindowId(7),
            tessera_desktop::window::WindowId(8),
        ],
        1.0,
        DockPosition::Bottom,
    );
    let panel = presentation.panel;
    dock.motion.tooltip_alpha = 1.0;
    dock.hover_owner_bounds = Some(owner);
    dock.hover_surface_bounds = Some(lens::Rect {
        x: panel.origin.x as f32,
        y: panel.origin.y as f32,
        w: panel.size.w as f32,
        h: panel.size.h as f32,
    });
    dock.live_preview = Some(presentation);

    // The preview panel is a second painted body above the bar, at the
    // floating-panel radius.
    let (_, hover) = dock
        .hover_body_material()
        .expect("the preview panel is painted");
    assert_eq!(hover.radius, Design::dark().radii.panel_lg);
    let bridge_y = (panel.origin.y + panel.size.h) as f32 + PREVIEW_PANEL_GAP * 0.5;
    assert!(dock.captures_pointer(
        owner.x + owner.w * 0.5,
        bridge_y,
        (1920.0, 1080.0),
        &[],
        &workspace_snapshot(),
    ));
    assert!(dock.captures_pointer(
        owner.x + owner.w * 0.5,
        owner.y + owner.h * 0.25,
        (1920.0, 1080.0),
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn hovered_live_preview_uses_one_parent_body_with_an_optical_focus_field() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let owner = lens::Rect {
        x: 900.0,
        y: 970.0,
        w: 84.0,
        h: 84.0,
    };
    let window = tessera_desktop::window::WindowId(7);
    let presentation = live_preview_layout(
        &Design::dark(),
        (1920.0, 1080.0),
        owner,
        &[window],
        1.0,
        DockPosition::Bottom,
    );
    let panel = presentation.panel;
    dock.motion.tooltip_alpha = 1.0;
    dock.hover_surface_bounds = Some(lens::Rect {
        x: panel.origin.x as f32,
        y: panel.origin.y as f32,
        w: panel.size.w as f32,
        h: panel.size.h as f32,
    });
    let mut focused_presentation = presentation.clone();
    focused_presentation.focused = Some(window);
    dock.hovered_preview = Some(window);
    dock.live_preview = Some(focused_presentation);

    // Selection is a painted wash over the focused card, inside the preview
    // panel's single body — never a nested body or an outline.
    let design = Design::dark();
    let focused_card = presentation.cards[0].geometry.outer;
    let focus = preview::card_material(
        &design,
        preview::PreviewCardState::Selected,
        presentation.cards[0].corner_radius,
    );
    assert_eq!(focus.bg, design.selection.selected_tint);
    assert_eq!(focus.border_width, 0.0);
    assert!(focused_card.size.w > 0 && focused_card.size.h > 0);
}

#[test]
fn a_client_owning_the_output_locks_the_dock_and_release_restores_it() {
    let mut dock = Dock::new();
    dock.apply_space_use(SpaceUse::Fullscreen);
    assert_eq!(dock.space_use, SpaceUse::Fullscreen);

    dock.apply_space_use(SpaceUse::Available);
    assert_eq!(dock.space_use, SpaceUse::Available);
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
    );
}

#[test]
fn entry_matches_app_id_like_the_launcher_heuristic() {
    let mut e = Entry {
        id: "org.mozilla.firefox.desktop".to_string(),
        icon: Some("firefox-icon".to_string()),
        ..Default::default()
    };
    e.startup_wm_class = Some("Firefox".to_string());
    assert!(entry_matches_app_id(&e, "firefox")); // WM class, case-insensitive
    assert!(entry_matches_app_id(&e, "org.mozilla.firefox")); // desktop-id stem
    assert!(entry_matches_app_id(&e, "Firefox-Icon")); // icon name
    assert!(!entry_matches_app_id(&e, "chromium"));
    assert!(!entry_matches_app_id(&e, ""));
}

#[test]
fn rest_centres_include_the_section_gap_for_transient_tiles() {
    // 2 pinned tiles (incl. launchpad) + 1 transient application tile.
    let pinned = Dock::rest_centre_estimate(1, 3, 2, 1920.0);
    let transient = Dock::rest_centre_estimate(2, 3, 2, 1920.0);
    let pitch = DOCK_TILE + DOCK_TILE_GAP;
    assert!(
        (transient - pinned - DOCK_TILE - DOCK_SECTION_GAP).abs() < 1e-5,
        "the section boundary replaces the ordinary gap with the wider section gap"
    );
    // No transient tiles → no extra gap.
    let a = Dock::rest_centre_estimate(1, 2, 2, 1920.0);
    let b = Dock::rest_centre_estimate(0, 2, 2, 1920.0);
    assert!((a - b - pitch).abs() < 1e-5);
}

#[test]
fn live_strip_centres_settle_onto_the_rest_estimates() {
    // Launchpad + 2 pinned apps (one running) + 1 transient application. With
    // every spring at rest and no drag permutation, the live strip geometry
    // must reproduce the resting estimates the magnification factor and the
    // pointer bounds are computed from — including the section boundary gap.
    let dock = dock_with(vec![app("firefox.desktop"), app("terminal.desktop")]);
    let windows = [window(1, "firefox", true), window(2, "unknown-app", false)];
    let tiles = Dock::frame_tiles(
        &dock.tile_cache,
        &dock.apps,
        &dock.all_apps,
        &dock.icons,
        dock.catalog_revision,
        &windows,
        Some("Applications"),
    );
    let n = tiles.len();
    assert_eq!(n, 4);
    let pinned_count = tiles.iter().filter(|t| t.pinned).count();
    assert_eq!(pinned_count, 3);

    let order: Vec<usize> = (0..n).collect();
    let eased = vec![DOCK_TILE; n];
    // Both sides of this assertion read the strip's single length source: the
    // bar leads with the dock's Bento reading section, and comparing against a
    // hand-rolled icon-only sum would compare two different bars.
    let bento = dock.strip_cells();
    let bar_len = crate::components::dock::bento::strip_length(
        eased.iter().copied(),
        &bento,
        DOCK_TILE_GAP,
        DOCK_PAD,
    );
    let origin = (1920.0 - bar_len) * 0.5 + DockState::reading_extent(&bento);
    let centres = strip_centres(&eased, &order, origin, pinned_count, DOCK_SECTION_GAP);
    for (i, centre) in centres.iter().enumerate() {
        let rest = dock.live_rest_centre(i, n, pinned_count, 1920.0);
        assert!(
            (centre - rest).abs() < 1e-4,
            "tile {i}: live centre {centre} != rest estimate {rest}"
        );
    }

    // The section boundary replaces the ordinary tile gap with the wider
    // section gap, so the divider at its edge-to-edge midpoint keeps exactly
    // one ordinary gap of clearance on each side.
    let last_pinned_edge = centres[pinned_count - 1] + eased[pinned_count - 1] * 0.5;
    let first_transient_edge = centres[pinned_count] - eased[pinned_count] * 0.5;
    assert!((first_transient_edge - last_pinned_edge - DOCK_SECTION_GAP).abs() < 1e-4);
}

#[test]
fn strip_centres_place_the_section_gap_at_the_preview_boundary() {
    // Pin preview: the transient tile (index 3) previews inside the pinned
    // strip at slot 2, so the boundary gap lands right after it.
    let eased = vec![DOCK_TILE; 4];
    let order = vec![0, 1, 3, 2];
    let centres = strip_centres(&eased, &order, 0.0, 3, DOCK_SECTION_GAP);
    let pitch = DOCK_TILE + DOCK_TILE_GAP;
    let first = DOCK_PAD + DOCK_TILE * 0.5;
    assert!((centres[0] - first).abs() < 1e-4);
    assert!((centres[1] - (first + pitch)).abs() < 1e-4);
    // Slot 2 holds the dragged transient tile: the ordinary gap.
    assert!((centres[3] - (first + 2.0 * pitch)).abs() < 1e-4);
    // Slot 3: the wider section gap replaces the ordinary gap.
    assert!((centres[2] - (first + 2.0 * pitch + DOCK_TILE + DOCK_SECTION_GAP)).abs() < 1e-4);
}

#[test]
fn drop_section_follows_the_section_divider() {
    // Launchpad + 2 pinned apps + 1 transient app on a 1920-wide bottom dock.
    let (n, pinned_count, axis_len) = (4, 3, 1920.0);
    let boundary = Dock::section_boundary_axis(n, pinned_count, axis_len);
    let last_pinned = Dock::rest_centre_estimate(2, n, pinned_count, axis_len);
    assert!((boundary - (last_pinned + DOCK_TILE * 0.5 + DOCK_SECTION_GAP * 0.5)).abs() < 1e-4);
    assert_eq!(
        Dock::drop_section_at(boundary - 1.0, n, pinned_count, axis_len),
        DropSection::Pinned
    );
    assert_eq!(
        Dock::drop_section_at(boundary + 1.0, n, pinned_count, axis_len),
        DropSection::Transient
    );
}

#[test]
fn drop_insert_index_swaps_once_the_cursor_passes_a_tile_centre() {
    // Rest centres of the pinned strip with the dragged tile removed.
    let centres = [100.0, 166.0, 232.0];
    assert_eq!(Dock::drop_insert_index(&centres, 50.0), 0);
    assert_eq!(Dock::drop_insert_index(&centres, 100.0), 0);
    // Past the next tile's centre (its midpoint) the insertion slot moves
    // past it — one swap per centre crossed.
    assert_eq!(Dock::drop_insert_index(&centres, 100.1), 1);
    assert_eq!(Dock::drop_insert_index(&centres, 166.0), 1);
    assert_eq!(Dock::drop_insert_index(&centres, 166.1), 2);
    assert_eq!(Dock::drop_insert_index(&centres, 400.0), 3);
}

#[test]
fn reorder_commit_stays_inside_the_pinned_range() {
    // The insertion index ranges over the pinned sequence (excluding the
    // dragged tile) by construction, so a committed reorder can neither
    // displace the leading Launchpad tile nor cross the pinned/transient
    // separator.
    let mut ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    // Drag "a" past "b"'s centre: one swap.
    assert!(Dock::move_element(&mut ids, 0, 1));
    assert_eq!(ids, vec!["b", "a", "c"]);
    // Drag "a" (now at 1) to the end of the pinned strip.
    assert!(Dock::move_element(&mut ids, 1, 2));
    assert_eq!(ids, vec!["b", "c", "a"]);
    // Drag "a" back before every pinned tile.
    assert!(Dock::move_element(&mut ids, 2, 0));
    assert_eq!(ids, vec!["a", "b", "c"]);
    // Dropping on the tile's own slot is a no-op.
    assert!(!Dock::move_element(&mut ids, 1, 1));
    assert_eq!(ids, vec!["a", "b", "c"]);
    // Out-of-range indices never panic or mutate.
    assert!(!Dock::move_element(&mut ids, 3, 0));
    assert!(!Dock::move_element(&mut ids, 0, 4));
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn drag_threshold_promotes_only_past_the_threshold() {
    let origin = (100.0, 100.0);
    assert!(!Dock::drag_threshold_exceeded(origin, (103.0, 104.0)));
    assert!(!Dock::drag_threshold_exceeded(origin, origin));
    assert!(Dock::drag_threshold_exceeded(origin, (107.0, 100.0)));
    // Diagonal travel past the same distance also promotes.
    assert!(Dock::drag_threshold_exceeded(origin, (105.0, 105.0)));
    assert!(!Dock::drag_threshold_exceeded(origin, (104.0, 104.0)));
}

#[test]
fn edge_drag_target_prefers_the_nearest_in_zone_edge() {
    let display = (1920.0, 1080.0);
    assert_eq!(
        Dock::edge_drag_target((50.0, 540.0), display),
        Some(DockPosition::Left)
    );
    assert_eq!(
        Dock::edge_drag_target((960.0, 1000.0), display),
        Some(DockPosition::Bottom)
    );
    assert_eq!(
        Dock::edge_drag_target((1870.0, 540.0), display),
        Some(DockPosition::Right)
    );
    // In a corner the nearer edge wins.
    assert_eq!(
        Dock::edge_drag_target((20.0, 1040.0), display),
        Some(DockPosition::Left)
    );
    assert_eq!(
        Dock::edge_drag_target((80.0, 1060.0), display),
        Some(DockPosition::Bottom)
    );
    // The screen centre and the top edge are outside every zone; the dock
    // keeps its current edge there (top is deliberately not a target).
    assert_eq!(Dock::edge_drag_target((960.0, 540.0), display), None);
    assert_eq!(Dock::edge_drag_target((960.0, 4.0), display), None);
}

#[test]
fn rest_bounds_and_reserved_follow_the_configured_edge() {
    let display = (1920.0, 1080.0);
    // Launchpad + one pinned tile: 2 tiles, 1 pinned (both pinned here; the
    // pinned count only matters for the section gap).
    let bar_len = 2.0 * DOCK_TILE + DOCK_TILE_GAP + 2.0 * DOCK_PAD;

    let bottom = Dock::rest_bounds(2, 2, DockPosition::Bottom, display);
    assert_eq!(bottom.y, display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
    assert_eq!(bottom.h, DOCK_PANEL_HEIGHT);
    assert_eq!(bottom.w, bar_len);

    let left = Dock::rest_bounds(2, 2, DockPosition::Left, display);
    assert_eq!(left.x, DOCK_EDGE_MARGIN);
    assert_eq!(left.w, DOCK_PANEL_HEIGHT);
    assert_eq!(left.y, (display.1 - bar_len) * 0.5);
    assert_eq!(left.h, bar_len);

    let right = Dock::rest_bounds(2, 2, DockPosition::Right, display);
    assert_eq!(right.x, display.0 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
    assert_eq!(right.w, DOCK_PANEL_HEIGHT);
    assert_eq!(right.h, bar_len);

    let extent = (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32;
    let mut dock = Dock::new();
    assert_eq!(dock.reserved().bottom, extent);
    assert_eq!(dock.reserved().left, 0);
    dock.set_position(DockPosition::Left);
    assert_eq!(dock.reserved().left, extent);
    assert_eq!(dock.reserved().bottom, 0);
    dock.set_position(DockPosition::Right);
    assert_eq!(dock.reserved().right, extent);
    assert_eq!(dock.reserved().left, 0);
}

#[test]
fn side_dock_collapsed_geometry_anchors_to_its_edge() {
    let display = (1920.0, 1080.0);
    let expanded_len = 320.0;

    let expanded = Dock::collapsed_panel_rect(DockPosition::Left, display, expanded_len, 1.0);
    assert_eq!(expanded.x, DOCK_EDGE_MARGIN);
    assert_eq!(expanded.w, DOCK_PANEL_HEIGHT);
    assert_eq!(expanded.h, expanded_len);
    assert_eq!(expanded.y, (display.1 - expanded_len) * 0.5);

    let collapsed = Dock::collapsed_panel_rect(DockPosition::Left, display, expanded_len, 0.0);
    assert_eq!(collapsed.x, AUTOHIDE_HANDLE_EDGE_MARGIN);
    assert_eq!(collapsed.w, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(collapsed.h, AUTOHIDE_HANDLE_WIDTH);
    // The handle stays centred on the edge, like the bottom one.
    assert_eq!(
        collapsed.y + collapsed.h * 0.5,
        display.1 * 0.5,
        "the side handle is centred vertically"
    );
    assert_eq!(
        collapsed.x, expanded.x,
        "the side capsule holds the bar's offset: the dock grows in place"
    );

    let right = Dock::collapsed_panel_rect(DockPosition::Right, display, expanded_len, 1.0);
    assert_eq!(right.x, display.0 - DOCK_EDGE_MARGIN - DOCK_PANEL_HEIGHT);
    let right_collapsed =
        Dock::collapsed_panel_rect(DockPosition::Right, display, expanded_len, 0.0);
    assert_eq!(
        right_collapsed.x,
        display.0 - AUTOHIDE_HANDLE_EDGE_MARGIN - AUTOHIDE_HANDLE_HEIGHT
    );
    assert_eq!(
        right_collapsed.x + right_collapsed.w,
        right.x + right.w,
        "the right capsule holds the bar's anchored edge"
    );
}

#[test]
fn catalog_push_updates_position_and_reconciles_optimistic_order() {
    let mut dock = dock_with(vec![app("a.desktop"), app("b.desktop")]);
    assert_eq!(dock.position, DockPosition::Bottom);

    // A push carrying a new edge moves the dock (a live config edit).
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("a.desktop"), app("b.desktop")],
        pinned: vec![app("a.desktop"), app("b.desktop")],
        icons: IconSet::default(),
        position: DockPosition::Right,
    });
    assert_eq!(dock.position, DockPosition::Right);

    // A queued optimistic reorder is superseded by the reconciling push.
    dock.pending_order = Some(vec!["b.desktop".to_string(), "a.desktop".to_string()]);
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("a.desktop"), app("b.desktop")],
        pinned: vec![app("a.desktop"), app("b.desktop")],
        icons: IconSet::default(),
        position: DockPosition::Left,
    });
    assert!(dock.pending_order.is_none());
    assert_eq!(dock.position, DockPosition::Left);

    // While an edge drag holds the panel, a catalog push must not yank the
    // dock back to the configured edge mid-gesture.
    dock.press = Some(PressState {
        origin: (10.0, 500.0),
        target: PressTarget::Panel,
        dragging: true,
        section: DropSection::Pinned,
        insert: None,
        start_position: DockPosition::Left,
    });
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("a.desktop"), app("b.desktop")],
        pinned: vec![app("a.desktop"), app("b.desktop")],
        icons: IconSet::default(),
        position: DockPosition::Bottom,
    });
    assert_eq!(
        dock.position,
        DockPosition::Left,
        "an in-flight edge drag owns the live position"
    );
}

#[test]
fn minimize_targets_map_windows_to_their_resting_tile_icons() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    let pinned_window = window(1, "firefox", true);
    let transient = window(2, "terminal", false);
    dock.update(ChromeUpdate::AllWindows(&[
        pinned_window.clone(),
        transient.clone(),
    ]));
    let targets = dock.minimize_targets((1920.0, 1080.0));
    assert_eq!(targets.len(), 2, "one target per running window");

    let firefox = targets
        .iter()
        .find(|(id, _)| *id == pinned_window.id)
        .expect("pinned window has a target")
        .1;
    let terminal = targets
        .iter()
        .find(|(id, _)| *id == transient.id)
        .expect("transient window has a target")
        .1;
    // Bottom dock: resting icons are DOCK_TILE squares inside the bottom panel.
    assert_eq!(firefox.size.w, DOCK_TILE as i32);
    assert_eq!(firefox.size.h, DOCK_TILE as i32);
    assert!(firefox.origin.y + firefox.size.h > 1080 - 100);
    assert!(firefox.origin.y + firefox.size.h <= 1080);
    assert_ne!(firefox, terminal, "different tiles give different targets");

    // A second window of the same app folds into the same tile: same target.
    // The transient stays around so the strip length (and therefore every
    // icon's resting position) is unchanged.
    let second_firefox = window(3, "Firefox", false);
    dock.update(ChromeUpdate::AllWindows(&[
        pinned_window.clone(),
        second_firefox.clone(),
        transient,
    ]));
    let targets = dock.minimize_targets((1920.0, 1080.0));
    let first = targets
        .iter()
        .find(|(id, _)| *id == pinned_window.id)
        .map(|(_, rect)| *rect);
    let second = targets
        .iter()
        .find(|(id, _)| *id == second_firefox.id)
        .map(|(_, rect)| *rect);
    assert_eq!(first, Some(firefox));
    assert_eq!(second, Some(firefox));
}

#[test]
fn minimize_targets_follow_the_dock_edge() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    dock.set_position(DockPosition::Left);
    let w = window(1, "firefox", false);
    dock.update(ChromeUpdate::AllWindows(std::slice::from_ref(&w)));
    let targets = dock.minimize_targets((1920.0, 1080.0));
    let rect = targets
        .iter()
        .find(|(id, _)| *id == w.id)
        .expect("window has a target")
        .1;
    // Left dock: the resting icon sits inside the left panel, vertically centred.
    assert!(rect.origin.x < 100, "icon hugs the left edge: {rect:?}");
    let centre_y = rect.origin.y + rect.size.h / 2;
    assert!(
        (centre_y - 540).abs() < 100,
        "icon near vertical centre: {rect:?}"
    );
}

#[test]
fn dock_tiles_fall_back_to_default_icon_when_app_has_no_icon() {
    let dummy_ptr = 0x1234 as *mut std::ffi::c_void;
    let mut dock = Dock::new();
    let app_without_icon = app("custom-app.desktop");
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app_without_icon.clone()],
        pinned: vec![app_without_icon],
        icons: IconSet::from_raw_with_default(HashMap::new(), Some(dummy_ptr)),
        position: DockPosition::Bottom,
    });
    let running_unknown = window(1, "unknown-unregistered-window", false);
    dock.update(ChromeUpdate::AllWindows(std::slice::from_ref(
        &running_unknown,
    )));
    let tiles = Dock::frame_tiles(
        &dock.tile_cache,
        &dock.apps,
        &dock.all_apps,
        &dock.icons,
        dock.catalog_revision,
        &[running_unknown],
        Some("Applications"),
    );
    // Pinned tile without an icon gets the default icon
    let pinned_tile = tiles
        .iter()
        .find(|t| t.key == "app:custom-app.desktop")
        .unwrap();
    assert_eq!(pinned_tile.icon, Some(dummy_ptr));
    // Transient running window without an icon also gets the default icon
    let transient_tile = tiles.iter().find(|t| !t.pinned && !t.launchpad).unwrap();
    assert_eq!(transient_tile.icon, Some(dummy_ptr));
}

#[test]
fn transit_skim_does_not_reveal_collapsed_dock() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    dock.collapse_pending = false;
    dock.hidden_trigger_armed = true;

    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let inside_point = (
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );
    let outside_work_point = (indicator.x + indicator.w * 0.5, indicator.y - 60.0);

    // Frame 1: cursor skims into indicator (16ms)
    let confirmed_1 = Dock::step_hidden_reveal(
        DockPosition::Bottom,
        &mut dock.hidden_trigger_armed,
        &mut dock.autohide_dwell,
        dock.autohide_dwell_threshold,
        inside_point,
        display,
        0.016,
    );
    assert!(
        !confirmed_1,
        "first 16ms transit frame must not confirm intent"
    );
    assert_eq!(dock.autohide_dwell, 0.016);

    // Frame 2: cursor still in indicator (another 16ms, total 32ms < 180ms)
    let confirmed_2 = Dock::step_hidden_reveal(
        DockPosition::Bottom,
        &mut dock.hidden_trigger_armed,
        &mut dock.autohide_dwell,
        dock.autohide_dwell_threshold,
        inside_point,
        display,
        0.016,
    );
    assert!(!confirmed_2, "32ms transit frame must not confirm intent");

    // Frame 3: cursor leaves indicator back into client work area
    let confirmed_3 = Dock::step_hidden_reveal(
        DockPosition::Bottom,
        &mut dock.hidden_trigger_armed,
        &mut dock.autohide_dwell,
        dock.autohide_dwell_threshold,
        outside_work_point,
        display,
        0.016,
    );
    assert!(!confirmed_3);
    assert_eq!(
        dock.autohide_dwell, 0.0,
        "leaving indicator resets dwell timer"
    );
}

#[test]
fn sustained_dwell_triggers_dock_reveal() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    dock.collapse_pending = false;
    dock.hidden_trigger_armed = true;

    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let inside_point = (
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );

    // Dwell for 100ms: not yet enough
    let confirmed_partial = Dock::step_hidden_reveal(
        DockPosition::Bottom,
        &mut dock.hidden_trigger_armed,
        &mut dock.autohide_dwell,
        dock.autohide_dwell_threshold,
        inside_point,
        display,
        0.100,
    );
    assert!(!confirmed_partial);

    // Dwell another 90ms (total 190ms >= AUTOHIDE_DWELL_THRESHOLD of 180ms): intent confirmed!
    let confirmed_full = Dock::step_hidden_reveal(
        DockPosition::Bottom,
        &mut dock.hidden_trigger_armed,
        &mut dock.autohide_dwell,
        dock.autohide_dwell_threshold,
        inside_point,
        display,
        0.090,
    );
    assert!(
        confirmed_full,
        "dwelling past threshold confirms user reveal intent"
    );
}

#[test]
fn live_morphing_panel_prevents_hitbox_inflation_trap() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    // Partially revealed transition state (25% progress)
    dock.motion.reveal = SpringState::new(0.25);

    let rest = dock.pointer_bounds(display);
    let rest_len = rest.w;
    let live_panel = Dock::collapsed_panel_rect(DockPosition::Bottom, display, rest_len, 0.25);

    // A cursor position inside the eventual resting rectangle, but well above the live panel
    let work_area_point = (display.0 * 0.5, rest.y + 10.0);
    assert!(
        work_area_point.1 < live_panel.y,
        "the cursor must sit in the client area above the morphing panel"
    );

    // Crucial UX guarantee: the client area above the live panel must NOT be captured!
    assert!(
        !dock.captures_pointer(
            work_area_point.0,
            work_area_point.1,
            display,
            &[],
            &workspace_snapshot(),
        ),
        "the morphing dock must not inflate its hitbox to capture client pixels ahead of its body"
    );

    // Pixels inside the live morphing panel ARE captured
    let inside_live_point = (
        live_panel.x + live_panel.w * 0.5,
        live_panel.y + live_panel.h * 0.5,
    );
    assert!(dock.captures_pointer(
        inside_live_point.0,
        inside_live_point.1,
        display,
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn quick_dismiss_timeout_is_snappy_for_uninteracted_reveal() {
    const {
        assert!(
            AUTOHIDE_QUICK_DISMISS_TIMEOUT <= 0.20,
            "uninteracted reveal must exit rapidly to avoid frustrating the user"
        );
    }
    const {
        assert!(
            AUTOHIDE_DWELL_THRESHOLD >= 0.15 && AUTOHIDE_DWELL_THRESHOLD <= 0.25,
            "dwell threshold must filter transit flings without feeling sluggish"
        );
    }
}

#[test]
fn set_autohide_dwell_clamps_and_customizes_threshold() {
    let mut dock = Dock::new();
    assert_eq!(dock.autohide_dwell_threshold, AUTOHIDE_DWELL_THRESHOLD);

    dock.set_autohide_dwell(0.35);
    assert_eq!(dock.autohide_dwell_threshold, 0.35);

    dock.set_autohide_dwell(0.0001);
    assert_eq!(dock.autohide_dwell_threshold, 0.01);
}

#[test]
fn cursor_retreat_detection_handles_all_dock_edges() {
    use crate::components::dock::rendering::detect_cursor_retreat;

    let panel = Rect {
        x: 800.0,
        y: 1040.0,
        w: 320.0,
        h: 40.0,
    };

    // Bottom dock: moving upward (y decreases) outside panel detects retreat
    assert!(detect_cursor_retreat(
        DockPosition::Bottom,
        (900.0, 1020.0),
        Some((900.0, 1030.0)),
        panel,
    ));
    // Moving downward toward bottom dock is approaching, not retreating
    assert!(!detect_cursor_retreat(
        DockPosition::Bottom,
        (900.0, 1035.0),
        Some((900.0, 1020.0)),
        panel,
    ));

    // Left dock: panel on left edge (x: 0..40). Moving rightward (x increases) outside panel detects retreat
    let left_panel = Rect {
        x: 0.0,
        y: 400.0,
        w: 40.0,
        h: 280.0,
    };
    assert!(detect_cursor_retreat(
        DockPosition::Left,
        (60.0, 500.0),
        Some((50.0, 500.0)),
        left_panel,
    ));
    // Moving leftward toward left dock is approaching
    assert!(!detect_cursor_retreat(
        DockPosition::Left,
        (45.0, 500.0),
        Some((60.0, 500.0)),
        left_panel,
    ));
}

#[test]
fn multi_frame_continuous_dwell_expands_dock_from_hidden_state() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    dock.autohide_idle = dock.autohide_timeout;
    let workspaces = workspace_snapshot();

    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let mut input = Input::new(display, 0.016);
    input.set_cursor(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );

    // Dwell for 10 frames (160ms < 180ms threshold)
    for _ in 0..10 {
        dock.prepare_frame(&input, &[], &workspaces);
        assert!(
            dock.anim_pending(),
            "dwell accumulation keeps animation ticking"
        );
        assert_eq!(dock.autohide_idle, dock.autohide_timeout);
    }
    assert!(dock.autohide_dwell > 0.15 && dock.autohide_dwell < dock.autohide_dwell_threshold);

    // Next 2 frames (accumulating to > 180ms)
    dock.prepare_frame(&input, &[], &workspaces);
    dock.prepare_frame(&input, &[], &workspaces);
    assert_eq!(
        dock.autohide_idle, 0.0,
        "sustained multi-frame dwell confirms reveal intent"
    );
    assert!(dock.anim_pending());
}

#[test]
fn physical_screen_edge_hover_dwell_expands_dock() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);
    dock.autohide_idle = dock.autohide_timeout;
    let workspaces = workspace_snapshot();

    // Cursor is parked right on the bottom physical edge under the capsule
    let mut input = Input::new(display, 0.016);
    input.set_cursor(display.0 * 0.5, display.1 - 1.0);

    for _ in 0..12 {
        dock.prepare_frame(&input, &[], &workspaces);
    }
    assert_eq!(
        dock.autohide_idle, 0.0,
        "dwelling at physical screen edge confirms reveal intent"
    );
}

#[test]
fn cursor_navigating_toward_dock_tiles_does_not_abort_reveal() {
    use crate::components::dock::rendering::detect_cursor_retreat;
    let display = (1920.0, 1080.0);
    let rest_bounds = Dock::rest_bounds(5, 5, DockPosition::Bottom, display);

    // Cursor moves upward from the bottom edge into the resting dock tile zone
    let last_cursor = Some((display.0 * 0.5, display.1 - 2.0));
    let cursor = (display.0 * 0.5, rest_bounds.y + 20.0);

    assert!(
        !detect_cursor_retreat(DockPosition::Bottom, cursor, last_cursor, rest_bounds),
        "moving within rest_bounds toward dock icons is normal navigation, not retreat"
    );

    // Only moving above rest_bounds toward the client area is retreat
    let outside_cursor = (display.0 * 0.5, rest_bounds.y - 10.0);
    let previous = Some((display.0 * 0.5, rest_bounds.y - 2.0));
    assert!(
        detect_cursor_retreat(DockPosition::Bottom, outside_cursor, previous, rest_bounds),
        "moving outside rest_bounds toward client window is retreat"
    );
}

#[test]
fn preview_hover_locks_magnification_axis_to_owner_tile_centre() {
    let display = (1920.0, 1080.0);
    let n = 5;
    let pinned_count = 3;
    let owner_idx = 2;
    let owner_centre = Dock::rest_centre_estimate(owner_idx, n, pinned_count, display.0);
    let neighbour_idx = 1;
    let neighbour_centre = Dock::rest_centre_estimate(neighbour_idx, n, pinned_count, display.0);

    // 1. Hovering directly on the dock bar: axis tracks the cursor coordinate.
    let cursor_on_bar = owner_centre + 8.0;
    let axis_on_bar = Dock::magnification_axis(false, true, Some(owner_centre), cursor_on_bar);
    assert_eq!(axis_on_bar, cursor_on_bar);

    // 2. Hovering inside the preview card / bridge above the dock:
    // over_rest_bounds is false, over_hover_surface is true.
    // The axis locks onto the owner tile's centre.
    let cursor_in_preview_card_a = owner_centre - 45.0;
    let axis_preview_a =
        Dock::magnification_axis(true, false, Some(owner_centre), cursor_in_preview_card_a);
    assert_eq!(
        axis_preview_a, owner_centre,
        "magnification axis must lock to owner tile centre when hovering preview"
    );

    // Moving horizontally across preview cards must not wobble or shift the dock.
    let cursor_in_preview_card_b = owner_centre + 60.0;
    let axis_preview_b =
        Dock::magnification_axis(true, false, Some(owner_centre), cursor_in_preview_card_b);
    assert_eq!(
        axis_preview_b, owner_centre,
        "magnification axis stays locked as cursor traverses preview cards"
    );

    // 3. Peak magnification is preserved on the owner tile while hovering its preview.
    let owner_target = Dock::magnification_target(true, axis_preview_a, owner_centre);
    assert_eq!(
        owner_target, DOCK_TILE_MAX,
        "owner tile must stay at maximum magnification while preview is hovered"
    );

    // 4. Neighbouring tiles receive smooth cosine-bell attenuation centered at the owner tile.
    let neighbour_target = Dock::magnification_target(true, axis_preview_a, neighbour_centre);
    assert!(
        neighbour_target > DOCK_TILE && neighbour_target < DOCK_TILE_MAX,
        "neighbour tiles stay smoothly curved around the owner tile"
    );

    // 5. When the cursor leaves the preview into open desktop space:
    // over_hover_surface and over_rest_bounds both become false, in_band drops to false.
    let dismissed_owner_target = Dock::magnification_target(false, axis_preview_a, owner_centre);
    assert_eq!(
        dismissed_owner_target, DOCK_TILE,
        "tiles must smoothly return to resting size when preview is dismissed"
    );
}

#[test]
fn app_menu_follows_dock_dark_and_light_theme() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let owner = lens::Rect {
        x: 900.0,
        y: 970.0,
        w: 84.0,
        h: 84.0,
    };
    dock.app_menu.open(
        "Editor",
        None,
        [tessera_desktop::window::WindowId(7)],
        owner,
        None,
    );
    assert!(dock.app_menu.is_open());

    let dark_menu_body = Design::dark().surfaces.for_role(SurfaceRole::Menu).body;
    dock.update(ChromeUpdate::Appearance(&Design::light()));
    let light_menu_body = Design::light().surfaces.for_role(SurfaceRole::Menu).body;
    assert_ne!(
        light_menu_body, dark_menu_body,
        "the dock's menu re-tones with the appearance"
    );
}

#[test]
fn dock_morph_keeps_capture_and_shadow_damage_stable() {
    let display = (1919.0, 1079.0);
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
        dock.set_position(position);
        dock.set_autohide(true);
        let footprint = dock.capture_footprint(display);
        let damage = dock.damage_regions(&[], display).unwrap();
        for progress in [0.0, 0.001, 0.25, 0.75, 1.0] {
            for active in [false, true] {
                dock.motion.reveal = SpringState::new(progress);
                dock.anim_active = active;
                assert_eq!(dock.capture_footprint(display), footprint);
                assert_eq!(dock.damage_regions(&[], display), Some(damage.clone()));
                let body = dock.body_rect(display).expect("a morphing dock paints");
                assert!(footprint.x <= body.x && footprint.y <= body.y);
                assert!(footprint.x + footprint.w >= body.x + body.w);
                assert!(footprint.y + footprint.h >= body.y + body.h);
                // The body's shadow reaches twice its blur radius, and the
                // shadow scales with the panel thickness as it collapses.
                let style = Design::dark().surfaces.for_role(SurfaceRole::Dock);
                let shadow_factor = ((if dock.position.is_vertical() {
                    body.w
                } else {
                    body.h
                }) / DOCK_PANEL_HEIGHT)
                    .clamp(0.35, 1.0);
                let blur = style.shadow_blur * shadow_factor;
                let offset = style.shadow_offset_y * shadow_factor;
                let fringe = blur * 2.0;
                let x0 = (body.x - fringe).max(0.0).floor() as i32;
                let y0 = (body.y - fringe + offset.min(0.0)).max(0.0).floor() as i32;
                let x1 = (body.x + body.w + fringe).min(display.0).ceil() as i32;
                let y1 = (body.y + body.h + fringe + offset.max(0.0))
                    .min(display.1)
                    .ceil() as i32;
                /* The strip region must already cover the painted body plus
                 * its shadow envelope: the popup bands are separate regions, so
                 * this is asserted per-region rather than on a union. */
                let strip = damage.first().expect("the strip is always reported");
                assert!(strip.x <= x0 as f32);
                assert!(strip.y <= y0 as f32);
                assert!(strip.x + strip.w >= x1 as f32);
                assert!(strip.y + strip.h >= y1 as f32);
            }
        }
    }
}

#[test]
fn autohide_collapse_settle_drains_swapchain_ring_frames() {
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(1.0);
    dock.autohide_idle = dock.autohide_timeout;
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();
    let windows = Vec::new();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();

    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);
    input.set_cursor(display.0 * 0.5, 100.0);

    // Advance collapse over several frames until it settles to 0.0
    for _ in 0..60 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
        });
        if (dock.autohide_reveal() - 0.0).abs() <= 0.002 {
            break;
        }
    }
    assert_eq!(dock.autohide_reveal(), 0.0);
    // When the animation settles, it primes 3 drain frames. The drain is
    // swapchain-ring *damage* bookkeeping, not an animation: it must not keep
    // requesting frames, or a settled dock is repainted at the output refresh
    // rate forever (the self-sustaining repaint loop this test now guards).
    assert_eq!(dock.motion.settled_drain_frames, 3);
    assert!(
        !dock.anim_pending(),
        "a settled dock must not pump the frame loop just to drain the ring"
    );

    // Frame 1 of drain: the counter advances, and the dock still asks for no
    // frames -- the ring drains across the frames real work produces.
    ui.frame(&input, |f| {
        dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
    });
    assert_eq!(dock.motion.settled_drain_frames, 2);
    assert!(!dock.anim_pending());

    // Frame 2 of drain
    ui.frame(&input, |f| {
        dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
    });
    assert_eq!(dock.motion.settled_drain_frames, 1);
    assert!(!dock.anim_pending());

    // Frame 3 of drain
    ui.frame(&input, |f| {
        dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
    });
    assert_eq!(dock.motion.settled_drain_frames, 0);
    assert!(
        !dock.anim_pending(),
        "all 3 ring slots drained, dock at complete rest"
    );
}

#[test]
fn dismiss_latch_prevents_rebound_oscillation_during_collapse() {
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(1.0);
    dock.autohide_idle = 0.0;
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();
    let windows = Vec::new();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();

    let mut ui = lens::Ui::headless().unwrap();
    // Cursor parked in the terminal area above the expanded dock
    let mut input = Input::new(display, 0.016);
    input.set_cursor(display.0 * 0.5, 950.0);

    // Click outside to trigger Escape hatch dismiss
    input.set_mouse_down(lens::MouseButton::Left, true);
    ui.frame(&input, |f| {
        dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
    });
    assert!(dock.dismiss_latched, "click outside engages dismiss latch");

    input.set_mouse_down(lens::MouseButton::Left, false);

    // Advance collapse partially. While collapsing, move cursor into the morphing panel area.
    for _ in 0..5 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
        });
    }
    assert!(dock.autohide_reveal() < 0.9 && dock.autohide_reveal() > 0.05);

    // Cursor moves inside the active panel area (which previously caused rebound loop)
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    input.set_cursor(indicator.x + indicator.w * 0.5, display.1 - 2.0);

    for _ in 0..30 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
        });
        if (dock.autohide_reveal() - 0.0).abs() <= 0.002 {
            break;
        }
    }

    assert_eq!(
        dock.autohide_reveal(),
        0.0,
        "dock must collapse completely without rebounding"
    );

    // Move cursor away into client work area: latch clears
    input.set_cursor(display.0 * 0.5, 500.0);
    dock.prepare_frame(&input, &windows, &workspaces);
    assert!(
        !dock.dismiss_latched,
        "latch clears once rested and cursor departs"
    );
}

#[test]
fn obscured_dock_clears_collapse_pending_once_rested_so_dwell_can_reopen() {
    let mut dock = Dock::new();
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();

    // A window overlapping the bottom resting dock area
    let mut invading = window(10, "terminal", true);
    invading.position = tessera_primitives::Point {
        x: 100,
        y: (display.1 - 60.0) as i32,
    };
    invading.size = tessera_primitives::Size { w: 1000, h: 200 };

    let mut input = Input::new(display, 0.016);
    input.set_cursor(display.0 * 0.5, 500.0);

    dock.prepare_frame(&input, std::slice::from_ref(&invading), &workspaces);
    assert!(dock.dock_obscured, "window overlapping dock sets obscured");
    assert!(
        dock.collapse_pending,
        "obscured dock initiates collapse_pending"
    );

    // Advance until fully collapsed
    for _ in 0..40 {
        dock.prepare_frame(&input, std::slice::from_ref(&invading), &workspaces);
        ui.frame(&input, |f| {
            dock.render(
                f,
                &input,
                std::slice::from_ref(&invading),
                &workspaces,
                &i18n,
                &mut out,
            );
        });
        if (dock.autohide_reveal() - 0.0).abs() <= 0.002 {
            break;
        }
    }
    assert_eq!(dock.autohide_reveal(), 0.0);
    assert!(
        dock.dock_obscured,
        "dock_obscured remains true while window overlaps resting dock bounds"
    );
    assert!(
        !dock.collapse_pending,
        "collapse_pending clears once initial collapse to 0.0 completes so dwell can reopen"
    );

    // Dwelling on the collapsed indicator while still obscured must expand the dock
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    input.set_cursor(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );
    for _ in 0..15 {
        dock.prepare_frame(&input, std::slice::from_ref(&invading), &workspaces);
        ui.frame(&input, |f| {
            dock.render(
                f,
                &input,
                std::slice::from_ref(&invading),
                &workspaces,
                &i18n,
                &mut out,
            );
        });
    }
    assert!(
        dock.autohide_reveal() > 0.05,
        "obscured dock must re-expand when pointer dwells on collapsed indicator"
    );

    // Now window moves away
    invading.position = tessera_primitives::Point { x: 100, y: 100 };
    dock.prepare_frame(&input, std::slice::from_ref(&invading), &workspaces);
    assert!(!dock.dock_obscured, "window moved away: no longer obscured");
    assert!(
        !dock.collapse_pending,
        "collapse_pending stays clear when window leaves"
    );
}

#[test]
fn dock_interaction_sets_interacted_flag_and_standard_timeout() {
    let mut dock = dock_with(vec![app("term.desktop")]);
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(1.0);
    dock.autohide_idle = 0.0;
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();
    let windows = Vec::new();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();

    assert!(!dock.dock_interacted);

    // Hover over a tile and dwell. The bar leads with its reading cells now, so
    // the target comes from the live geometry rather than a cells-free estimate
    // that would land on a reading instead of an icon.
    let mut input = Input::new(display, 0.016);
    let rest_bounds = dock.pointer_bounds(display);
    let reading = crate::components::dock::DockState::reading_extent(&dock.strip_cells());
    input.set_cursor(
        rest_bounds.x + reading + DOCK_TILE * 0.5,
        rest_bounds.y + rest_bounds.h * 0.5,
    );

    for _ in 0..30 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
        });
    }

    assert!(
        dock.dock_interacted,
        "hover dwell / interaction sets dock_interacted"
    );
}

/// The pre-pass and `render` must resolve the *same* strip geometry: if they
/// disagree about which tile the cursor is over, the pre-pass tears down a
/// tooltip that `render` believes is still live (or vice versa), which is the
/// one-frame flicker this pre-pass exists to prevent (ADR-0171, ADR-0180).
#[test]
fn prepass_and_render_agree_on_the_hovered_tile() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![
        app("one.desktop"),
        app("two.desktop"),
        app("three.desktop"),
    ]);
    dock.motion.reveal = SpringState::new(1.0);
    let workspaces = workspace_snapshot();
    let windows = Vec::new();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();

    // First frame primes the tile spring table; the pre-pass intentionally
    // skips an empty table so a never-shown Dock does not allocate springs.
    let mut input = Input::new(display, 0.016);
    input.set_cursor(display.0 * 0.5, display.1 - 20.0);
    dock.prepare_frame(&input, &windows, &workspaces);
    ui.frame(&input, |f| {
        dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
    });

    let tiles = dock.tiles(&dock.all_windows);
    let keys: Vec<String> = tiles.iter().map(|tile| tile.key.clone()).collect();
    assert!(keys.len() >= 3, "three pinned apps plus the launchpad");
    let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();

    // Resolve the strip at rest width — the exact geometry the pre-pass
    // publishes — and aim at the centre of tile 1. The dock's pager reading is
    // part of that geometry, so the observation the pre-pass makes must be in
    // place before this resolves the bar.
    dock.observe_workspaces(&workspaces);
    let eased = vec![DOCK_TILE; keys.len()];
    let order: Vec<usize> = (0..keys.len()).collect();
    let geometry = dock.resolve_strip_geometry(&eased, &order, pinned_count, display.0, display);
    let target = geometry.icon_rects[1];
    let cursor = (target.x + target.w * 0.5, target.y + target.h * 0.5);

    // Settle the magnification springs on the resting width, so the geometry the
    // pre-pass publishes is the geometry this test aims at. A tooltip that is
    // already open is a settled state by definition.
    for key in &keys {
        dock.sizes.insert(
            key.clone(),
            SpringState {
                value: DOCK_TILE,
                velocity: 0.0,
            },
        );
    }

    // Model a tooltip already open over tile 1.
    dock.hovered_tile = Some(keys[1].clone());
    dock.tooltip_tile = Some(keys[1].clone());
    dock.motion.tooltip_alpha = 1.0;
    dock.hover_owner_bounds = Some(target);
    dock.hover_surface_bounds = Some(lens::Rect {
        x: target.x,
        y: target.y,
        w: target.w,
        h: target.h,
    });

    let mut input = Input::new(display, 0.016);
    input.set_cursor(cursor.0, cursor.1);
    dock.prepare_frame(&input, &windows, &workspaces);
    assert_eq!(
        dock.tooltip_tile.as_deref(),
        Some(keys[1].as_str()),
        "the pre-pass hit-test must keep the tooltip on the tile under the cursor",
    );

    ui.frame(&input, |f| {
        dock.render(f, &input, &windows, &workspaces, &i18n, &mut out);
    });
    assert_eq!(
        dock.tooltip_tile.as_deref(),
        Some(keys[1].as_str()),
        "render must agree with the pre-pass about the hovered tile",
    );
}

/// `resolve_strip_geometry` is the single source of truth for the strip
/// layout: at rest it must land every tile centre and icon rect exactly on the
/// interaction model's resting geometry, on every edge (ADR-0171, ADR-0180).
#[test]
fn resolved_strip_geometry_lands_icons_on_the_resting_model() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![
        app("one.desktop"),
        app("two.desktop"),
        app("three.desktop"),
    ]);
    // One transient running window so the section gap participates.
    let windows = vec![window(1, "org.example.Transient.desktop", true)];
    let tiles = dock.tiles(&windows);
    let n = tiles.len();
    let pinned_count = tiles.iter().filter(|tile| tile.pinned).count();
    assert!(
        pinned_count < n,
        "a transient tile must exercise the section boundary"
    );

    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        dock.set_position(position);
        let vertical = position.is_vertical();
        let axis_disp = if vertical { display.1 } else { display.0 };
        let eased = vec![DOCK_TILE; n];
        let order: Vec<usize> = (0..n).collect();
        let geometry =
            dock.resolve_strip_geometry(&eased, &order, pinned_count, axis_disp, display);

        // The expected length comes from the strip's own single source rather
        // than a second copy of the arithmetic: a hand-rolled sum here is how
        // the leading Bento section would silently fall out of the layout.
        let bar_len = crate::components::dock::bento::strip_length(
            eased.iter().copied(),
            &dock.strip_cells(),
            DOCK_TILE_GAP,
            DOCK_PAD,
        );
        let expected_panel = Dock::panel_rect_for(position, bar_len, display);
        assert_eq!(geometry.panel_rect, expected_panel);
        assert_eq!(geometry.section_gap, DOCK_SECTION_GAP);

        for i in 0..n {
            let rest = dock.live_rest_centre(i, n, pinned_count, axis_disp);
            assert!(
                (geometry.centres[i] - rest).abs() < 1e-3,
                "{position:?} tile {i}: centre {} != rest estimate {rest}",
                geometry.centres[i]
            );
            let rect = geometry.icon_rects[i];
            let centre = if vertical {
                rect.y + rect.h * 0.5
            } else {
                rect.x + rect.w * 0.5
            };
            assert!(
                (centre - rest).abs() < 1e-3,
                "{position:?} tile {i}: icon centre {centre} != rest estimate {rest}"
            );
            assert!((rect.w - DOCK_TILE).abs() < 1e-3 && (rect.h - DOCK_TILE).abs() < 1e-3);
        }
    }
}

/// A tile context menu is a popup the dock paints, so it must appear in the
/// dock's damage footprint. Omitting it left the menu outside the frame's render
/// area, and it only appeared on frames where an unrelated backdrop refresh
/// happened to widen the damage.
#[test]
fn the_tile_context_menu_is_part_of_the_dock_damage() {
    let mut dock = dock_with(vec![app("foot")]);
    let display = (1920.0f32, 1080.0f32);

    // No menu: the footprint is the strip plus its shadow, nowhere near mid-screen.
    let quiet = dock
        .damage_regions(&[], display)
        .expect("the dock localizes its own paint");
    assert!(
        quiet.iter().all(|band| band.y > 900.0),
        "a closed menu leaves only the strip band: {quiet:?}"
    );

    // Open one anchored at the strip, then check the popup's band is covered.
    dock.app_menu.open(
        "foot",
        Some(app("foot")),
        std::iter::empty(),
        lens_rect(600.0, 980.0, 84.0, 74.0),
        None,
    );
    let menu = dock
        .app_menu
        .bounds(display)
        .expect("an open menu has bounds");
    Dock::record_menu_damage(&mut dock.menu_damage_rect, menu);

    let with_menu = dock
        .damage_regions(&[], display)
        .expect("the dock localizes its own paint");
    let menu_rect = tessera_primitives::Rect::new(
        menu.x.floor() as i32,
        menu.y.floor() as i32,
        menu.w.ceil() as i32,
        menu.h.ceil() as i32,
    );
    /* The popup is its own region, not folded into the strip: one of the
     * reported bands must cover it exactly, and no band may span the gap between
     * the strip and the popup. */
    assert!(
        with_menu.iter().any(|band| {
            band.x <= menu_rect.origin.x as f32
                && band.y <= menu_rect.origin.y as f32
                && band.x + band.w >= (menu_rect.origin.x + menu_rect.size.w) as f32
                && band.y + band.h >= (menu_rect.origin.y + menu_rect.size.h) as f32
        }),
        "the popup {menu_rect:?} must be a reported region: {with_menu:?}"
    );
    assert!(
        with_menu.len() > quiet.len(),
        "an open popup adds a region instead of widening the strip"
    );
    let strip = quiet.first().expect("strip");
    assert!(
        with_menu
            .iter()
            .any(|band| band.y + band.h == strip.y + strip.h && band.h == strip.h),
        "the strip keeps its own extent while the popup is open: {with_menu:?}"
    );
}

/// A dismissed context menu is removed in one frame, so it must own its
/// vacated band for a bounded retirement window rather than only the frame that
/// happened to be on screen (ADR-0227).
#[test]
fn a_dismissed_context_menu_retires_its_vacated_band() {
    let mut dock = dock_with(vec![app("foot")]);
    let display = (1920.0f32, 1080.0f32);
    dock.last_display = Some(display);
    dock.app_menu.open(
        "foot",
        Some(app("foot")),
        std::iter::empty(),
        lens_rect(600.0, 980.0, 84.0, 74.0),
        None,
    );
    assert!(dock.captures_keyboard());

    dock.dismiss_transient_ui();
    assert!(!dock.app_menu.is_open());
    assert!(
        dock.damage_pending(),
        "the close owes damage-only frames for the ring"
    );
    assert!(
        dock.menu_retire_frames > 0 && dock.menu_damage_rect.is_some(),
        "the vacated popup footprint is retained through the retirement"
    );
    assert!(
        !dock.anim_pending(),
        "a retirement is damage bookkeeping, not motion"
    );
    assert!(
        dock.damage_regions(&[], display)
            .is_some_and(|regions| !regions.is_empty()),
        "the retirement names a repaintable region"
    );
}

/// A popup that follows its tile must repaint the band it vacated: the previous
/// frame's menu is still on screen, and no animation predicate reports the move.
#[test]
fn a_moving_menu_footprint_keeps_the_vacated_band() {
    let mut dock = dock_with(vec![app("foot")]);
    let first = lens_rect(600.0, 980.0, 84.0, 74.0);
    let second = lens_rect(900.0, 980.0, 84.0, 74.0);

    Dock::record_menu_damage(&mut dock.menu_damage_rect, first);
    let after_first = dock.menu_damage_rect.expect("recorded");
    Dock::record_menu_damage(&mut dock.menu_damage_rect, second);
    let after_second = dock.menu_damage_rect.expect("recorded");

    assert!(
        after_second.origin.x <= after_first.origin.x,
        "the vacated band's left edge must stay covered"
    );
    assert!(
        after_second.size.w >= after_first.size.w,
        "the union of both anchor positions must stay covered"
    );
}

/// A `lens::Rect` from corners, so the popup fixtures stay readable.
fn lens_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn the_capsule_body_interpolates_without_double_premultiplying() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    let display = (1920.0, 1080.0);

    // At rest the body is exactly the palette's collapsed tone. The palette is
    // stored premultiplied, so this also pins that the interpolation works on
    // straight channels and repremultiplies once — mixing premultiplied bytes
    // and repremultiplying landed the capsule darker than either endpoint.
    dock.motion.reveal = SpringState::new(0.0);
    dock.autohide_dwell = 0.0;
    let (_, rest) = dock.body_material(display).expect("resting capsule");
    assert_eq!(rest.bg, Design::dark().dock.bar_surface_collapsed);

    // At full reveal it is exactly the palette's expanded tone.
    dock.motion.reveal = SpringState::new(1.0);
    let (_, open) = dock.body_material(display).expect("expanded bar");
    assert_eq!(open.bg, Design::dark().dock.bar_surface_expanded);

    // Mid-morph it sits between the two, in straight-channel terms.
    dock.motion.reveal = SpringState::new(0.5);
    let (_, mid) = dock.body_material(display).expect("morphing body");
    let straight = |c: Color| c.with_alpha(c.components().3).components();
    let (cr, _, _, _) = straight(Design::dark().dock.bar_surface_collapsed);
    let (er, _, _, _) = straight(Design::dark().dock.bar_surface_expanded);
    let (mr, _, _, _) = straight(mid.bg);
    assert!(
        mr >= cr.min(er) && mr <= cr.max(er),
        "mid-morph red {mr} stays inside [{}, {}]",
        cr.min(er),
        cr.max(er)
    );
}

#[test]
fn dwelling_on_the_capsule_lifts_its_body_without_changing_its_alpha() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    dock.motion.reveal = SpringState::new(0.0);

    dock.autohide_dwell = 0.0;
    let (_, calm) = dock.body_material((1920.0, 1080.0)).expect("calm capsule");
    dock.autohide_dwell = dock.autohide_dwell_threshold;
    let (_, dwelling) = dock
        .body_material((1920.0, 1080.0))
        .expect("dwelling capsule");

    // The dwell lifts the tone and leaves the body's coverage alone: the
    // capsule is already opaque, so an alpha lift would be invisible.
    assert_eq!(
        calm.bg.components().3,
        dwelling.bg.components().3,
        "dwell never changes the body's alpha"
    );
    assert!(
        dwelling.bg.components().0 > calm.bg.components().0,
        "dwell brightens the body: {:?} vs {:?}",
        calm.bg.components(),
        dwelling.bg.components()
    );
    assert_eq!(
        calm.radius, dwelling.radius,
        "dwell is a tone change, not a geometry change"
    );
}

#[test]
fn an_open_menu_holds_the_dock_open_past_its_idle_timeout() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    dock.autohide_timeout = 0.01;

    assert!(!dock.interaction_locked(), "a resting dock locks nothing");

    let owner = lens_rect(900.0, 970.0, 84.0, 84.0);
    dock.app_menu.open(
        "Editor",
        None,
        [tessera_desktop::window::WindowId(7)],
        owner,
        None,
    );
    assert!(
        dock.interaction_locked(),
        "a popup the dock owns outranks the idle timer: the pointer is inside \
         the popup, not the bar, and collapsing would destroy the menu mid-use"
    );

    dock.app_menu.dismiss();
    assert!(
        !dock.interaction_locked(),
        "closing the menu releases the veto"
    );
}

#[test]
fn a_held_press_holds_the_dock_open_until_it_resolves() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    dock.autohide_timeout = 0.01;
    let display = (1920.0, 1080.0);

    let _ = display;
    assert!(!dock.interaction_locked());
    // A press that has not resolved yet, exactly as the render pass records it.
    dock.press = Some(PressState {
        origin: (960.0, 1040.0),
        target: PressTarget::Panel,
        dragging: false,
        section: DropSection::Pinned,
        insert: None,
        start_position: DockPosition::Bottom,
    });
    assert!(
        dock.interaction_locked(),
        "a press that ends outside the bar is still the same gesture"
    );

    dock.press = None;
    assert!(
        !dock.interaction_locked(),
        "the resolved press releases the veto"
    );
}

#[test]
fn a_locked_dock_does_not_collapse_no_matter_how_long_it_idles() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    dock.autohide_timeout = 0.01;
    dock.motion.reveal = SpringState::new(1.0);

    let owner = lens_rect(900.0, 970.0, 84.0, 84.0);
    dock.app_menu.open(
        "Editor",
        None,
        [tessera_desktop::window::WindowId(7)],
        owner,
        None,
    );

    // Ten idle timeouts' worth of stepping while the popup is open.
    for _ in 0..200 {
        dock.apply_space_use(SpaceUse::Available);
        dock.autohide_idle = 10.0;
    }
    assert!(
        dock.autohide_reveal() > 0.9,
        "the dock stayed revealed under an open popup: {}",
        dock.autohide_reveal()
    );
}

/// A snapshot with `count` workspaces on one output, `current` active.
fn ws_snapshot(count: usize, current: usize) -> WorkspaceSnapshot {
    use tessera_desktop::workspace::{OutputId, OutputSnapshot, WorkspaceEntry, WorkspaceId};
    let entries: Vec<WorkspaceEntry> = (0..count)
        .map(|i| WorkspaceEntry {
            id: WorkspaceId(i as u64 + 1),
            label: None,
            tiled: true,
            toplevels: Vec::new(),
        })
        .collect();
    WorkspaceSnapshot {
        outputs: vec![OutputSnapshot {
            id: OutputId(0),
            connector: "HEADLESS-1".to_string(),
            current: entries.get(current).map(|w| w.id),
            workspaces: entries,
        }],
    }
}

/// The damage envelope must cover the painted body for every shape the reading
/// section can take.
///
/// The footprint used to be derived from the icon tiles alone, so a wide reading
/// section — many workspaces, few icons — pushed the painted bar outside the
/// region the compositor repaints. The excess pixels simply never updated.
#[test]
fn the_damage_envelope_covers_a_reading_section_of_any_width() {
    let display = (1920.0, 1080.0);
    for workspaces in [1_usize, 4, 9, 20, 40] {
        for running in [0_usize, 1, 3] {
            let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
            if running > 0 {
                let mut w = window(1, "org.example.Editor", true);
                w.app_id = Some("org.example.Editor".to_string());
                let mut windows = vec![w];
                for id in 2..=running as u32 {
                    windows.push(window(id.into(), "org.example.Other", false));
                }
                dock.update_windows(&windows);
            }
            dock.observe_workspaces(&ws_snapshot(workspaces, 0));

            let pointer = dock.pointer_bounds(display);
            let visual = dock.visual_panel_bounds(display);
            let footprint = dock.capture_footprint(display);

            assert!(
                visual.w + 1.0 >= pointer.w,
                "{workspaces} workspaces / {running} running: the visual body {} must cover \
                 the pointer body {}",
                visual.w,
                pointer.w
            );
            assert!(
                footprint.w + 1.0 >= visual.w,
                "{workspaces} workspaces / {running} running: the damage envelope {} must cover \
                 the painted body {}",
                footprint.w,
                visual.w
            );
        }
    }
}

#[test]
fn workspace_rail_auto_hides_on_single_workspace() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.observe_workspaces(&ws_snapshot(1, 0));
    assert!(
        dock.strip_cells().is_empty(),
        "a single workspace leaves the dock pure and centered with no rail"
    );
}

#[test]
fn workspace_rail_shows_on_multiple_workspaces() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.observe_workspaces(&ws_snapshot(4, 0));
    let cells = dock.strip_cells();
    let pager = cells
        .iter()
        .find(|cell| matches!(cell, StripCell::WorkspacePager(_)))
        .expect("carries the workspace rail");
    assert_eq!(pager.rest_width(), 65.0, "compact width for 4 workspaces");
}

#[test]
fn workspace_rail_click_switches_workspace() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let workspaces = ws_snapshot(3, 0);
    let ws_ids: Vec<_> = workspaces.outputs[0]
        .workspaces
        .iter()
        .map(|w| w.id)
        .collect();
    dock.observe_workspaces(&workspaces);

    let display = (1920.0, 1080.0);
    let mut input = Input::new(display, 0.05);

    let bounds = dock.visual_panel_bounds(display);
    let rail_x = bounds.x + DOCK_PAD;
    let rail_y = bounds.y + (bounds.h - DOCK_TILE) * 0.5;

    // Hover over tick 1
    let tick1_x = rail_x
        + crate::components::dock::bento::RAIL_INNER_PAD
        + crate::components::dock::bento::ACTIVE_TICK
        + crate::components::dock::bento::TICK_GAP
        + 2.0;
    let tick1_y = rail_y + DOCK_TILE * 0.5;

    // Hover to let magnification settle
    input.set_cursor(tick1_x, tick1_y);
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();
    let i18n = Localizer::default();
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
    });

    // Press on tick 1
    input.set_mouse_down(lens::MouseButton::Left, true);
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
    });

    // Release on tick 1
    input.set_mouse_down(lens::MouseButton::Left, false);

    let mut out2 = ChromeEvents::default();
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out2);
    });

    assert_eq!(
        out2.switch_workspace,
        Some(ws_ids[1]),
        "clicking tick 1 emits switch to workspace 1"
    );
}

#[test]
fn workspace_rail_wheel_scroll_switches_workspace() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let workspaces = ws_snapshot(3, 0);
    let ws_ids: Vec<_> = workspaces.outputs[0]
        .workspaces
        .iter()
        .map(|w| w.id)
        .collect();
    dock.observe_workspaces(&workspaces);

    let display = (1920.0, 1080.0);
    let mut input = Input::new(display, 0.05);

    let bounds = dock.visual_panel_bounds(display);
    let rail_x = bounds.x + DOCK_PAD + 10.0;
    let rail_y = bounds.y + (bounds.h - DOCK_TILE) * 0.5 + 10.0;

    // Hover over rail to let magnification and hover settle
    input.set_cursor(rail_x, rail_y);
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();
    let i18n = Localizer::default();
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
    });

    // Scroll down (negative scroll_y)
    input.as_raw_mut().scroll_y = -1.0;
    let mut out2 = ChromeEvents::default();
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out2);
    });

    assert_eq!(
        out2.switch_workspace,
        Some(ws_ids[1]),
        "scrolling down switches to next workspace"
    );
}

#[test]
fn workspace_rail_renders_on_vertical_dock_positions() {
    let display = (1920.0, 1080.0);
    let workspaces = ws_snapshot(3, 0);

    for position in [DockPosition::Left, DockPosition::Right] {
        let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
        dock.set_position(position);
        dock.observe_workspaces(&workspaces);

        let bounds = dock.visual_panel_bounds(display);
        assert!(bounds.w > 0.0 && bounds.h > 0.0);
        assert!(bounds.x >= 0.0, "{position:?} dock x is inside display");

        let input = Input::new(display, 0.05);
        let mut ui = lens::Ui::headless().unwrap();
        let mut out = ChromeEvents::default();
        let i18n = Localizer::default();
        ui.frame(&input, |f| {
            dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
        });
    }
}

/// A machine that reports radios and a battery grows a reading column, and the
/// column costs one constant span whatever it holds.
#[test]
fn the_reading_column_costs_one_span_whatever_the_machine_reports() {
    use tessera_desktop::system::{BatteryStatus, BluetoothLinkState, WifiLinkState};

    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let display = (1920.0, 1080.0);
    let bare = dock.pointer_bounds(display).w;

    let wifi_only = SystemStatus {
        wifi_enabled: Some(true),
        wifi_state: WifiLinkState::Connected,
        ..Default::default()
    };
    dock.update(ChromeUpdate::SystemStatus(&wifi_only));
    let one_reading = dock.pointer_bounds(display).w;

    let full = SystemStatus {
        wifi_enabled: Some(true),
        wifi_state: WifiLinkState::Connected,
        bluetooth_enabled: Some(true),
        bluetooth_state: BluetoothLinkState::Connected,
        battery: Some(BatteryStatus {
            percent: 64,
            charging: false,
        }),
        ..Default::default()
    };
    dock.update(ChromeUpdate::SystemStatus(&full));
    let three_readings = dock.pointer_bounds(display).w;

    assert!(
        one_reading > bare,
        "a reported radio must widen the bar by the column"
    );
    assert_eq!(
        one_reading, three_readings,
        "a third reading changes what is inside the plate, never the bar's length"
    );
    assert_eq!(
        three_readings - bare,
        crate::components::dock::bento::COLUMN_WIDTH
            + crate::components::dock::bento::READING_BOUNDARY_GAP,
        "the column costs its constant span plus the boundary gap into the icons"
    );
}

/// A machine that reports nothing keeps a pure, centered dock: no empty plate,
/// no reserved span, no boundary gap.
#[test]
fn a_machine_with_no_readings_keeps_a_bare_dock() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let display = (1920.0, 1080.0);
    let before = dock.pointer_bounds(display);
    dock.update(ChromeUpdate::SystemStatus(&SystemStatus::default()));
    assert_eq!(dock.pointer_bounds(display), before);
    assert!(dock.readings().is_empty());
    assert!(
        !dock
            .strip_cells()
            .iter()
            .any(|cell| matches!(cell, StripCell::Readings(_))),
        "no readings means no reading cell at all, not an empty one"
    );
}

/// Pressing a reading opens the surface that owns the reading, and does not
/// toggle it: a press while the panel is open must not close the panel the user
/// is looking at.
#[test]
fn pressing_a_reading_opens_the_control_center() {
    use tessera_desktop::system::WifiLinkState;

    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.update(ChromeUpdate::SystemStatus(&SystemStatus {
        wifi_enabled: Some(true),
        wifi_state: WifiLinkState::Connected,
        ..Default::default()
    }));

    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();
    let mut input = Input::new(display, 0.05);
    let mut ui = lens::Ui::headless().unwrap();
    let i18n = Localizer::default();

    // The rest plate comes from the same placement rule the frame uses, so the
    // press below targets the pixels the paint produces rather than a
    // hand-rolled box that happens to be close.
    let column = dock.readings();
    let plate = dock
        .rest_leading_cells(display)
        .into_iter()
        .find_map(|(cell, rect)| match cell {
            StripCell::Readings(readings) if readings == column => Some(rect),
            _ => None,
        })
        .expect("the reading column is a leading cell");
    let rows = column.row_rects(plate, false);
    let target = rows[0];
    let cursor = (target.x + target.w * 0.5, target.y + target.h * 0.5);

    let mut out = ChromeEvents::default();
    input.set_cursor(cursor.0, cursor.1);
    // Settle the magnification springs onto their rest geometry: a first-seen
    // tile grows in from a seed size, so the bar is narrower than its rest
    // width until the wave settles.
    for _ in 0..40 {
        ui.frame(&input, |f| {
            dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
        });
    }
    // Make the precondition explicit. The cursor above was derived from rest
    // geometry, so probing a bar that is still moving would test the wrong
    // pixels and pass or fail on where the wave happened to be.
    assert!(
        !dock.anim_pending(),
        "the bar must be at rest before it is probed at a resting coordinate"
    );
    assert_eq!(
        dock.hovered_reading_row,
        Some(0),
        "the row hit test uses the same algebra as the tiling"
    );

    input.set_mouse_down(lens::MouseButton::Left, true);
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
    });
    input.set_mouse_down(lens::MouseButton::Left, false);
    ui.frame(&input, |f| {
        dock.render(f, &input, &[], &workspaces, &i18n, &mut out);
    });

    assert!(
        out.open_control_center,
        "a reading press asks for the surface that owns the reading"
    );
    assert!(
        !out.toggle_pivot && out.clicked.is_none(),
        "a reading press activates nothing else"
    );
}

/// A pointer that comes to rest on a tile must produce the name bubble.
///
/// The dwell timer and the fade were advanced inside `render`, and the clock
/// term that would request the next frame was an `|` into `anim_active` in the
/// same `render`. `Shell::prepare_frame` recomputes `anim_active` on every frame
/// before the damage assessment reads it, so a `|` written during the previous
/// `render` is always overwritten: the timer gated the bubble but nothing
/// advanced it, and a pointer that arrived after the magnification wave settled
/// sat at `hover_elapsed ≈ 0` forever. The bubble simply never appeared.
///
/// This drives only the pre-pass, because that is where the invariant lives: if
/// the bubble depends on `render` to advance, this test cannot reach it.
#[test]
fn a_resting_pointer_raises_the_name_bubble_through_the_pre_pass() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let windows = [window(1, "org.example.Editor", true)];
    dock.update_windows(&windows);
    let workspaces = workspace_snapshot();
    let panel = dock.visual_panel_bounds(display);

    // The hit-test answer `render` would reach for the pointer below: the single
    // pinned tile, which sits centred in the bar once the wave has settled.
    dock.hovered_tile = Some("app:org.example.Editor.desktop".to_string());
    dock.hover_elapsed = 0.0;
    dock.reset_tooltip_fade();

    let mut input = Input::new(display, 1.0 / 120.0);
    input.set_cursor(display.0 * 0.5, panel.y + panel.h * 0.5);

    let mut frames_to_bubble = None;
    for frame in 0..600 {
        dock.prepare_frame(&input, &windows, &workspaces);
        if dock.tooltip_tile.is_some() && frames_to_bubble.is_none() {
            frames_to_bubble = Some(frame);
        }
        if dock.tooltip_alpha() > 0.99 {
            break;
        }
        assert!(
            dock.anim_pending() || dock.tooltip_alpha() > 0.99,
            "the clock must stay awake until the bubble has faded in: \
             alpha={} after {frame} frames",
            dock.tooltip_alpha()
        );
    }

    let frames = frames_to_bubble.expect(
        "a pointer resting past TOOLTIP_DWELL must raise the bubble; the pre-pass \
         never reached the dwell threshold",
    );
    assert!(
        frames <= (motion::TOOLTIP_DWELL * 120.0) as usize + 4,
        "the bubble must appear one dwell after the pointer rests, not {frames} frames later"
    );
    assert!(
        dock.tooltip_alpha() > 0.99,
        "the bubble must finish fading in, alpha={}",
        dock.tooltip_alpha()
    );

    // And leaving must fade it back out through the same pre-pass.
    dock.hovered_tile = None;
    let mut out_frames = 0;
    for frame in 0..600 {
        dock.prepare_frame(&input, &windows, &workspaces);
        out_frames = frame;
        if dock.tooltip_alpha() <= 0.0001 {
            break;
        }
        assert!(
            dock.anim_pending(),
            "the clock must stay awake until the bubble has faded out: \
             alpha={} after {frame} frames",
            dock.tooltip_alpha()
        );
    }
    assert_eq!(
        dock.tooltip_alpha(),
        0.0,
        "the bubble must reach zero, {out_frames} frames after the pointer left"
    );
    assert!(dock.tooltip_tile.is_none(), "a faded bubble is released");
}

/// The reveal value has exactly one owner.
///
/// `Dock` used to carry its own `autohide_reveal: f32`, resynchronised from the
/// spring at the top of every `prepare_frame`. Two copies of a continuously
/// moving value need a reconciliation step, and that step only ran once per
/// frame: anything reading the copy after the spring had advanced saw last
/// frame's progress while the geometry was derived from this frame's. The
/// accessor makes the disagreement unrepresentable.
#[test]
fn the_reveal_value_is_read_from_its_single_owner() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let workspaces = workspace_snapshot();

    // Writing the spring is writing the value; there is no second copy to sync.
    dock.motion.reveal = SpringState::new(0.37);
    assert_eq!(dock.autohide_reveal(), 0.37);

    // And the pre-pass advances that same value — not a mirror of it.
    let mut input = Input::new(display, 1.0 / 120.0);
    input.set_cursor(display.0 * 0.5, display.1 - 3.0);
    let before = dock.autohide_reveal();
    dock.prepare_frame(&input, &[], &workspaces);
    assert!(
        (dock.autohide_reveal() - before).abs() > f32::EPSILON,
        "the spring must advance the value the accessor reads: {before} -> {}",
        dock.autohide_reveal()
    );
}

/// A pointer resting between two equal tiles has a stable hover.
///
/// Magnification makes neighbouring tiles the same size around the pointer, so
/// their centres are equidistant to within float noise. Without hysteresis the
/// winner is decided by which side the noise fell on that frame and the answer
/// alternates. Every hover *change* is a discrete event downstream — it
/// restarts the name bubble's dwell, releases the previous tile's popover, and
/// requests a repaint — so an alternating answer restarts the dwell every other
/// frame and the bubble never completes. Measured before the fix: 121 frames to
/// fade in a bubble whose dwell is 36.
#[test]
fn a_pointer_between_two_equal_tiles_keeps_one_hover() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let windows = [window(1, "org.example.Editor", true)];
    dock.update_windows(&windows);
    let workspaces = workspace_snapshot();
    let mut ui = lens::Ui::headless().unwrap();
    let i18n = Localizer::default();

    // The exact midpoint of the two-tile strip: the geometry that made the
    // floating-point tie reachable.
    let bounds = Dock::rest_bounds(1, 0, DockPosition::Bottom, display);
    let mut input = Input::new(display, 1.0 / 120.0);
    input.set_cursor(
        bounds.x + bounds.w * 0.5,
        bounds.y + bounds.h - DOCK_TILE * 0.5,
    );
    let mut out = ChromeEvents::default();

    let mut flips = 0;
    let mut previous = dock.hovered_tile.clone();
    let mut frames_to_bubble = None;
    for frame in 0..200 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out)
        });
        if dock.hovered_tile != previous {
            flips += 1;
            previous = dock.hovered_tile.clone();
        }
        if dock.tooltip_alpha() > 0.99 && frames_to_bubble.is_none() {
            frames_to_bubble = Some(frame);
            break;
        }
    }

    assert_eq!(
        flips, 1,
        "a resting pointer acquires the hover once, not {flips} times"
    );
    let frames = frames_to_bubble
        .expect("the name bubble must appear while the pointer rests, not be restarted forever");
    // One dwell plus the fade, at the frame rate above, with slack for the
    // spring ramp before the tile is large enough to be hit at all.
    assert!(
        frames < 120,
        "the bubble must appear about one dwell (36 frames) after the pointer \
         rests, not {frames} frames later"
    );
}

/// A raised popover stays raised while the pointer travels into it.
///
/// The popover sits in the air gap above the bar, so a pointer that moves from
/// the tile into the popover is not inside any tile. Dropping the hover on the
/// way makes a live preview unreachable — the popover vanishes exactly as the
/// user reaches for it. `update_hover_surfaces` therefore falls back to the
/// popover's owner while the pointer is inside the surface the popover owns.
///
/// This is the behaviour that a "make the hover a pure function of the hit
/// test" simplification destroys, so it is pinned separately from the hysteresis
/// test that motivated that simplification.
#[test]
fn a_raised_popover_stays_while_the_pointer_enters_it() {
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();
    let i18n = Localizer::default();
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let windows = [window(1, "org.example.Editor", true)];
    dock.update_windows(&windows);
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();

    let bounds = Dock::rest_bounds(1, 0, DockPosition::Bottom, display);
    let icon_pt = (
        bounds.x + bounds.w * 0.5,
        bounds.y + bounds.h - DOCK_TILE * 0.5,
    );
    let mut input = Input::new(display, 1.0 / 120.0);
    input.set_cursor(icon_pt.0, icon_pt.1);
    for _ in 0..200 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out)
        });
        if dock.tooltip_alpha() > 0.99 {
            break;
        }
    }
    assert!(
        dock.tooltip_alpha() > 0.99,
        "the popover must be raised before the bridge can be tested"
    );
    let popover = dock
        .hover_surface_bounds
        .expect("a raised popover owns a surface rect");

    // Travel into the popover in small steps, as a pointer does.
    let target = (popover.x + popover.w * 0.5, popover.y + popover.h * 0.5);
    let mut x = icon_pt.0;
    let mut y = icon_pt.1;
    let mut step = Input::new(display, 1.0 / 120.0);
    while (y - target.1).abs() > 2.0 {
        y += if target.1 < y { -2.0 } else { 2.0 };
        x += (target.0 - x) * 0.25;
        step.set_cursor(x, y);
        dock.prepare_frame(&step, &windows, &workspaces);
        ui.frame(&step, |f| {
            dock.render(f, &step, &windows, &workspaces, &i18n, &mut out)
        });
    }
    step.set_cursor(target.0, target.1);
    for _ in 0..30 {
        dock.prepare_frame(&step, &windows, &workspaces);
        ui.frame(&step, |f| {
            dock.render(f, &step, &windows, &workspaces, &i18n, &mut out)
        });
    }

    assert_eq!(
        dock.tooltip_tile.as_deref(),
        Some("launchpad"),
        "the popover keeps its owner while the pointer is inside it"
    );
    assert!(
        dock.tooltip_alpha() > 0.99,
        "and it stays fully raised, not faded out: alpha={}",
        dock.tooltip_alpha()
    );
}

/// Collapsing into the capsule must never emit a sub-pixel launchpad cell.
///
/// A maximized client makes the Dock autohide (`effective_autohide`), and the
/// launchpad's 3x3 grid is subdivided from a tile that drains to zero. At 2x
/// scale a cell below one device pixel snapped to a negative width in the
/// encoder (`[-0.04, 0.05]`), which rejected the entire chrome scene. Because
/// ADR-0223 retries a failed build only every 250 ms, the collapse then played
/// at roughly 4 fps — the visible stutter. The cells are now omitted while
/// they are sub-pixel; every build in the drain must stay valid.
#[test]
fn collapsing_into_the_capsule_keeps_every_scene_build_valid() {
    let display = (1487.0, 865.0);
    let mut dock = dock_with(vec![app("foot.desktop")]);
    // The reading column widens the bar, as it did in the failing session.
    dock.system_status.battery = Some(tessera_desktop::system::BatteryStatus {
        percent: 21,
        charging: true,
    });
    dock.system_status.bluetooth_enabled = Some(true);
    dock.system_status.wifi_enabled = Some(true);
    dock.system_status.wifi_state = tessera_desktop::system::WifiLinkState::Connected;
    let windows = [window(1, "foot", true)];
    dock.update_windows(&windows);
    dock.update(ChromeUpdate::Scale(2.0));
    dock.apply_space_use(tessera_desktop::window::SpaceUse::Maximized);
    let workspaces = workspace_snapshot();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    ui.set_scale(2.0);
    let mut input = Input::new(display, 1.0 / 120.0);
    input.set_cursor(display.0 * 0.5, display.1 - 20.0);

    let mut collapsed = false;
    for frame in 0..600 {
        dock.prepare_frame(&input, &windows, &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &windows, &workspaces, &i18n, &mut out)
        });
        if let Err(error) = ui.snapshot() {
            panic!("frame {frame} failed during the collapse: {error}");
        }
        if dock.autohide_reveal() <= 0.002 {
            collapsed = true;
            break;
        }
    }
    assert!(collapsed, "the Dock must reach its collapsed capsule");
}

/// The damage envelope must cover the largest tile a magnification spring can
/// produce, on every dock edge.
///
/// The body is only one panel thick, while a tile anchored to the inner
/// baseline grows inward by `DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN`. The
/// envelope used to stop at the body (plus the shadow skirt), so the top rows
/// of a fully magnified icon fell outside the repaint region and retained the
/// previous frame — a horizontal tear across the hovered tile.
#[test]
fn the_damage_envelope_contains_every_tile_at_full_magnification() {
    let display = (1920.0, 1080.0);
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
        dock.set_position(position);
        let n = 2;
        let eased = vec![DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN; n];
        let geometry = dock.resolve_strip_geometry(
            &eased,
            &[0, 1],
            2,
            if position.is_vertical() {
                display.1
            } else {
                display.0
            },
            display,
        );
        let envelope = dock.capture_footprint(display);
        for (index, tile) in geometry.icon_rects.iter().enumerate() {
            assert!(
                envelope.x <= tile.x
                    && envelope.y <= tile.y
                    && envelope.x + envelope.w >= tile.x + tile.w
                    && envelope.y + envelope.h >= tile.y + tile.h,
                "{position:?}: envelope {envelope:?} does not contain magnified tile {index} {tile:?}"
            );
        }
    }
}

#[test]
fn launcher_active_dismisses_live_preview_and_inhibits_pointer_capture() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![app("org.example.Terminal.desktop")]);
    dock.hovered_tile = Some("app:org.example.Terminal.desktop".to_string());
    dock.tooltip_tile = Some("app:org.example.Terminal.desktop".to_string());
    dock.hover_surface_bounds = Some(Rect {
        x: 700.0,
        y: 800.0,
        w: 320.0,
        h: 180.0,
    });
    dock.live_preview = Some(LivePreviewPresentation {
        panel: tessera_primitives::Rect::new(700, 800, 320, 180),
        cards: Vec::new(),
        focused: None,
        visibility: 1.0,
        inactive_content_brightness: 0.72,
    });

    assert!(dock.captures_pointer(960.0, 1050.0, display, &[], &workspace_snapshot()));

    dock.update(ChromeUpdate::LauncherActive(true));
    assert!(dock.live_preview.is_none());
    assert!(dock.tooltip_tile.is_none());
    assert!(dock.hovered_tile.is_none());
    assert!(
        dock.hover_damage_rect.is_some(),
        "dismissing live preview on modal open must record vacated hover damage"
    );
    assert!(
        !dock.captures_pointer(960.0, 1050.0, display, &[], &workspace_snapshot()),
        "Dock must not capture pointer while an immersive modal owns the output"
    );
}

#[test]
fn dock_render_preserves_hover_dwell_and_fade_animation_after_spring_settles() {
    let display = (1920.0, 1080.0);
    let mut dock = dock_with(vec![app("foot.desktop")]);
    let workspaces = workspace_snapshot();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();

    // Place the pointer over the pinned Foot tile and advance with 60 Hz steps.
    // The magnification spring settles around ~180 ms, while `TOOLTIP_DWELL` is
    // 300 ms and the fade-in takes another 150 ms. `dock.anim_pending()` must
    // stay `true` after every `dock.render(...)` call until the tooltip is
    // fully raised (`alpha > 0.99`).
    let mut input = Input::new(display, 1.0 / 60.0);
    let rest = dock.pointer_bounds(display);
    input.set_cursor(rest.x + rest.w - 30.0, rest.y + rest.h * 0.5);

    for step in 0..45 {
        dock.prepare_frame(&input, &[], &workspaces);
        ui.frame(&input, |f| {
            dock.render(f, &input, &[], &workspaces, &i18n, &mut out)
        });
        if dock.tooltip_alpha() <= 0.999 {
            assert!(
                dock.anim_pending(),
                "step {step}: dock.anim_pending() must remain true while hover dwell/fade is in progress (hover_elapsed={}, alpha={})",
                dock.hover_elapsed,
                dock.tooltip_alpha()
            );
        }
    }
    assert_eq!(dock.tooltip_tile.as_deref(), Some("app:foot.desktop"));
    assert!(
        dock.tooltip_alpha() > 0.999,
        "tooltip must reach full opacity without pointer movement: alpha={}",
        dock.tooltip_alpha()
    );
}

#[test]
fn the_damage_regions_contain_tooltip_and_shadow_at_full_magnification() {
    let display = (1920.0, 1080.0);
    for position in [
        DockPosition::Bottom,
        DockPosition::Left,
        DockPosition::Right,
    ] {
        let mut dock = dock_with(vec![app("foot.desktop")]);
        dock.set_position(position);
        dock.hovered_tile = Some("app:foot.desktop".to_string());
        dock.tooltip_tile = Some("app:foot.desktop".to_string());
        dock.motion.tooltip_alpha = 0.5;
        dock.anim_active = true;
        dock.motion.anim_active = true;
        // Simulate the first frame the tooltip appears (`hover_damage_rect` not
        // yet populated by `render`).
        dock.hover_damage_rect = None;

        let n = 2;
        let eased = vec![DOCK_TILE_MAX * SPRING_OVERSHOOT_MARGIN; n];
        let geometry = dock.resolve_strip_geometry(
            &eased,
            &[0, 1],
            2,
            if position.is_vertical() {
                display.1
            } else {
                display.0
            },
            display,
        );
        let owner = geometry.icon_rects[1];
        let mut ui = lens::Ui::headless().unwrap();
        let input = Input::new(display, 1.0 / 60.0);
        let mut pill = Rect::default();
        ui.frame(&input, |f| {
            pill = rendering::tooltip_rect(
                f,
                &dock.design,
                "Foot",
                owner,
                display,
                0.5,
                Dock::popup_side_for(position),
            );
        });
        let style = dock
            .design
            .surfaces
            .for_role(tessera_design::SurfaceRole::Tooltip);
        let fringe = 2.0 * style.shadow_blur.max(0.0) + style.shadow_offset_y.abs() + 1.0;
        let regions = dock
            .damage_regions(&[], display)
            .expect("animating dock must report damage regions");
        let encloses = regions.iter().any(|r| {
            r.x <= (pill.x - fringe).max(0.0)
                && r.y <= (pill.y - fringe).max(0.0)
                && r.x + r.w >= (pill.x + pill.w + fringe).min(display.0)
                && r.y + r.h >= (pill.y + pill.h + fringe).min(display.1)
        });
        assert!(
            encloses,
            "{position:?}: damage_regions {regions:?} did not enclose magnified tooltip pill {pill:?} (fringe={fringe})"
        );
    }
}
