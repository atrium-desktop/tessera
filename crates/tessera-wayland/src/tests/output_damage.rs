use super::*;

#[test]
fn persisted_workspace_uses_output_position_not_stable_id() {
    let mut state = State::new(std::ptr::null_mut());
    let first = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    let first_window = tessera_desktop::window::WindowId(101);
    state.workspaces.place_toplevel(first, first_window);

    let second = state
        .workspaces
        .output(state.output)
        .and_then(|output| output.workspaces.get(1))
        .copied()
        .expect("placing on the first workspace creates a trailing workspace");
    let second_window = tessera_desktop::window::WindowId(102);
    state.workspaces.place_toplevel(second, second_window);

    assert_eq!(state.workspace_number_for_window(first_window), Some(1));
    assert_eq!(state.workspace_number_for_window(second_window), Some(2));
}

#[test]
fn backend_outputs_reconcile_workspace_connectors_and_geometry() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let geometry = |x, width| tessera_desktop::output::OutputGeometry {
        mode: tessera_desktop::output::OutputMode {
            width,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: tessera_desktop::output::Scale::IDENTITY,
        transform: tessera_primitives::Transform::Normal,
        logical_origin: tessera_primitives::Point { x, y: 0 },
    };
    server.set_outputs(vec![
        tessera_desktop::output::OutputInfo {
            connector: "DP-1".into(),
            geometry: geometry(0, 1920),
            kind: tessera_desktop::output::OutputKind::External,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        },
        tessera_desktop::output::OutputInfo {
            connector: "HDMI-A-1".into(),
            geometry: geometry(1920, 2560),
            kind: tessera_desktop::output::OutputKind::External,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        },
    ]);

    assert_eq!(
        server
            .output_infos()
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<Vec<_>>(),
        vec!["DP-1", "HDMI-A-1"]
    );
    assert_eq!(server.output_logical_rect().size.w, 1920);
    assert_eq!(
        server
            .workspace_snapshot()
            .outputs
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(["DP-1", "HDMI-A-1"])
    );
}

/// `[[output]]` policy (ADR-0028) overrides the backend-reported
/// geometry: scale and position apply per connector, and a `primary`
/// entry moves its output to the front of the list (index 0 is the
/// focused output whose geometry `output_logical_rect` reports).
#[test]
fn output_policies_apply_scale_position_and_primary() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let geometry = |x, width| tessera_desktop::output::OutputGeometry {
        mode: tessera_desktop::output::OutputMode {
            width,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: tessera_desktop::output::Scale::IDENTITY,
        transform: tessera_primitives::Transform::Normal,
        logical_origin: tessera_primitives::Point { x, y: 0 },
    };
    server.set_outputs(vec![
        tessera_desktop::output::OutputInfo {
            connector: "DP-1".into(),
            geometry: geometry(0, 1920),
            kind: tessera_desktop::output::OutputKind::External,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        },
        tessera_desktop::output::OutputInfo {
            connector: "HDMI-A-1".into(),
            geometry: geometry(1920, 2560),
            kind: tessera_desktop::output::OutputKind::External,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        },
    ]);

    server.set_output_policies(std::collections::HashMap::from([(
        "HDMI-A-1".to_owned(),
        tessera_desktop::output::OutputPolicy {
            scale: Some(2.0),
            position: Some(tessera_primitives::Point { x: 1920, y: 0 }),
            primary: true,
            ..Default::default()
        },
    )]));

    let infos = server.output_infos();
    assert_eq!(
        infos
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<Vec<_>>(),
        vec!["HDMI-A-1", "DP-1"],
        "the primary output leads the list"
    );
    assert_eq!(infos[0].geometry.scale.as_f32(), 2.0);
    assert_eq!(
        infos[0].geometry.logical_origin,
        tessera_primitives::Point { x: 1920, y: 0 }
    );
    assert_eq!(
        server.output_logical_rect().origin,
        tessera_primitives::Point { x: 1920, y: 0 },
        "the focused output geometry follows the primary policy"
    );
    // The other output keeps its backend-reported geometry.
    assert_eq!(infos[1].geometry.scale.as_f32(), 1.0);
    assert_eq!(
        infos[1].geometry.logical_origin,
        tessera_primitives::Point { x: 0, y: 0 }
    );
}

#[test]
fn surface_damage_accumulates_until_present_and_full_is_sticky() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    accumulate_committed_damage(
        &mut surface,
        vec![tessera_primitives::Rect::new(10, 20, 30, 40)],
        false,
    );
    accumulate_committed_damage(
        &mut surface,
        vec![tessera_primitives::Rect::new(100, 5, 10, 10)],
        false,
    );
    assert_eq!(
        surface.committed_damage,
        vec![
            tessera_primitives::Rect::new(10, 20, 30, 40),
            tessera_primitives::Rect::new(100, 5, 10, 10),
        ]
    );
    assert!(!surface.committed_damage_full);

    accumulate_committed_damage(&mut surface, Vec::new(), true);
    assert!(surface.committed_damage.is_empty());
    assert!(surface.committed_damage_full);

    // `unknown_full` is absorbing *within* a frame, but it no longer
    // discards precise damage from later commits in the same render
    // interval: those rects accumulate so the acknowledging present clears
    // both states together and the next frame resumes incremental updates
    // instead of inheriting a stale full latch (the historical behaviour
    // made one damage-less commit permanently poison every following
    // precise commit until present, forcing whole-buffer copies).
    accumulate_committed_damage(
        &mut surface,
        vec![tessera_primitives::Rect::new(1, 1, 2, 2)],
        false,
    );
    assert_eq!(
        surface.committed_damage,
        vec![tessera_primitives::Rect::new(1, 1, 2, 2)]
    );
    assert!(surface.committed_damage_full);
}

#[test]
fn surface_damage_region_subtracts_overlap_without_inventing_bbox_pixels() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    accumulate_committed_damage(
        &mut surface,
        vec![tessera_primitives::Rect::new(0, 0, 10, 10)],
        false,
    );
    accumulate_committed_damage(
        &mut surface,
        vec![tessera_primitives::Rect::new(5, 5, 10, 10)],
        false,
    );

    assert!(
        tessera_primitives::Rect::new(0, 0, 10, 10).fully_covered_by(&surface.committed_damage)
    );
    assert!(
        tessera_primitives::Rect::new(5, 5, 10, 10).fully_covered_by(&surface.committed_damage)
    );
    assert!(
        !surface
            .committed_damage
            .iter()
            .any(|rect| rect.contains(tessera_primitives::Point { x: 12, y: 2 })),
        "the overlap union must not dirty the bounding-box-only corner"
    );
}

#[test]
fn surface_damage_region_caps_pathological_client_lists() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    let rects = (0..=MAX_COMMITTED_DAMAGE_RECTS)
        .map(|index| tessera_primitives::Rect::new((index * 2) as i32, 0, 1, 1))
        .collect();
    accumulate_committed_damage(&mut surface, rects, false);

    assert_eq!(surface.committed_damage.len(), 1);
    assert_eq!(
        surface.committed_damage[0],
        tessera_primitives::Rect::new(0, 0, (MAX_COMMITTED_DAMAGE_RECTS * 2 + 1) as i32, 1)
    );
}

#[test]
fn surface_damage_region_promotes_unrepresentable_span_to_full() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    let mut rects = (0..MAX_COMMITTED_DAMAGE_RECTS)
        .map(|index| tessera_primitives::Rect::new(i32::MIN + (index as i32 * 2), 0, 1, 1))
        .collect::<Vec<_>>();
    rects.push(tessera_primitives::Rect::new(i32::MAX - 1, 0, 1, 1));
    accumulate_committed_damage(&mut surface, rects, false);

    assert!(surface.committed_damage_full);
    assert!(surface.committed_damage.is_empty());
}

#[test]
fn buffer_damage_at_hidpi_scale_rounds_outward_to_surface_coordinates() {
    let mapped = buffer_damage_to_surface(tessera_primitives::Rect::new(3, 5, 8, 10), 2);
    assert_eq!(mapped, tessera_primitives::Rect::new(1, 2, 5, 6));
}

#[test]
fn committed_opaque_region_culls_a_fully_covered_window_tree() {
    let mut state = State::new(std::ptr::null_mut());
    state.output_geometry = tessera_desktop::output::OutputGeometry {
        mode: tessera_desktop::output::OutputMode {
            width: 100,
            height: 80,
            refresh_mhz: 60_000,
        },
        ..Default::default()
    };
    let background_id = tessera_desktop::window::WindowId(301);
    let foreground_id = tessera_desktop::window::WindowId(302);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(
            client,
            &[background_id, foreground_id],
            HUMAN_INTERACTION_DOMAIN,
        )
        .expect("register physical window authority");
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, background_id);
    state.workspaces.place_toplevel(workspace, foreground_id);
    let output = state.output_geometry.logical_rect();

    let mut background = Box::new(SurfaceRec::new(0x301usize as *mut ffi::wl_resource));
    background.mapped = true;
    background.xdg_toplevel = background.resource;
    background.window.id = background_id;
    background.window.size = output.size;
    background.position = output.origin;
    background.width = output.size.w;
    background.height = output.size.h;
    background.pixels = vec![0x20; 4];

    let mut foreground = Box::new(SurfaceRec::new(0x302usize as *mut ffi::wl_resource));
    foreground.mapped = true;
    foreground.xdg_toplevel = foreground.resource;
    foreground.window.id = foreground_id;
    foreground.window.size = output.size;
    foreground.position = output.origin;
    foreground.width = output.size.w;
    foreground.height = output.size.h;
    foreground.pixels = vec![0xE0; 4];
    foreground.opaque_region = Some(vec![tessera_primitives::Rect::new(
        0,
        0,
        output.size.w - 1,
        output.size.h,
    )]);

    state.surfaces = vec![background.as_mut(), foreground.as_mut()];
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        runtime_dir: None,
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    assert!(!server.occluded_window_ids().contains(&background_id));
    foreground.opaque_region = Some(vec![tessera_primitives::Rect::new(
        0,
        0,
        output.size.w,
        output.size.h,
    )]);
    // An actively moving foreground cannot safely occlude the pixels it is
    // moving away from/to.
    foreground.window.transition = Some(tessera_desktop::transition::WindowTransition {
        from: tessera_primitives::Rect::new(0, 0, output.size.w / 2, output.size.h / 2),
        started_ms: server.now_ms(),
        duration_ms: 60_000,
        easing: tessera_desktop::transition::Easing::EaseOutCubic,
        effect: None,
    });
    assert!(!server.occluded_window_ids().contains(&background_id));

    // Once the transition settles it is normalized out of model state. The
    // same opaque foreground must immediately resume contributing coverage,
    // otherwise a covered video window continues rendering indefinitely.
    foreground.window.transition = Some(tessera_desktop::transition::WindowTransition {
        from: tessera_primitives::Rect::new(0, 0, output.size.w / 2, output.size.h / 2),
        started_ms: 0,
        duration_ms: 0,
        easing: tessera_desktop::transition::Easing::EaseOutCubic,
        effect: None,
    });
    assert_eq!(server.settle_finished_transitions(), 1);
    assert!(foreground.window.transition.is_none());
    let occluded = server.occluded_window_ids();
    assert!(occluded.contains(&background_id));
    assert!(!occluded.contains(&foreground_id));

    let physical_windows = server
        .client_surface_frames()
        .into_iter()
        .filter_map(|frame| frame.window)
        .collect::<Vec<_>>();
    assert_eq!(physical_windows, vec![foreground_id]);

    let preview_windows = server
        .client_preview_surface_frames()
        .into_iter()
        .filter_map(|frame| frame.window)
        .collect::<Vec<_>>();
    assert_eq!(preview_windows, vec![background_id, foreground_id]);
    assert_eq!(
        server.client_preview_surface_frame_order(),
        vec![background.resource as usize, foreground.resource as usize]
    );
}

#[test]
fn xdg_unmap_requires_a_fresh_initial_configure() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    surface.xdg_configured = true;
    surface.xdg_configure_acked = true;
    surface.pending_xdg_configures = vec![41, 42];

    reset_xdg_configure_state_after_unmap(&mut surface);

    assert!(!surface.xdg_configured);
    assert!(!surface.xdg_configure_acked);
    assert!(surface.pending_xdg_configures.is_empty());
}

/// [INV-ARCH-70]: only a buffer already owned by a presented frame may be
/// deferred to the next presentation epoch. An unpresented buffer superseded
/// mid-vblank by a newer commit must be released immediately, or an uncapped
/// client's fixed swapchain deadlocks in `get_back_bo`.
#[test]
fn test_inv_arch_70_unpresented_buffers_release_immediately() {
    assert_eq!(
        buffer_retirement(false),
        BufferRetirement::ReleaseNow,
        "an unpresented buffer must not wait on vblank"
    );
    assert_eq!(buffer_retirement(true), BufferRetirement::Defer);
}

#[test]
fn test_persist_app_geometry_saves_state() {
    let temp_dir =
        std::env::temp_dir().join(format!("tessera_test_persist_{}", std::process::id()));
    let state_file = temp_dir.join("window_state.json");
    let mut state = State::new(std::ptr::null_mut());
    state.window_state_path = state_file.clone();

    let app_id = "org.test.app";
    let rect = tessera_primitives::Rect {
        origin: tessera_primitives::Point { x: 300, y: 400 },
        size: tessera_primitives::Size { w: 900, h: 600 },
    };

    state.persist_app_geometry(
        app_id,
        rect,
        Some(2),
        Some(tessera_desktop::layout::LayoutRole::Floating),
    );

    assert_eq!(state.last_app_geometries.get(app_id), Some(&rect));
    let store_entry = state.window_state_store.get(app_id).unwrap();
    assert_eq!(store_entry.position, Some(rect.origin));
    assert_eq!(store_entry.size, Some(rect.size));
    assert_eq!(store_entry.workspace, Some(2));
    assert_eq!(
        store_entry.layout_role,
        Some(tessera_desktop::layout::LayoutRole::Floating)
    );

    assert!(state_file.exists());
    let loaded = window_state::WindowStateStore::load_from_path(&state_file);
    assert_eq!(loaded.get(app_id).unwrap(), store_entry);

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn pending_damage_accumulator_is_bounded_and_conservative() {
    let mut list = Vec::new();
    for index in 0..MAX_PENDING_DAMAGE_RECTS {
        push_pending_damage(
            &mut list,
            tessera_primitives::Rect::new((index * 2) as i32, 0, 1, 1),
        );
    }
    assert_eq!(list.len(), MAX_PENDING_DAMAGE_RECTS);

    // One past the budget: the list collapses to a single conservative
    // bounding box that still covers every previously declared rect plus
    // the new one — never a silent drop.
    push_pending_damage(&mut list, tessera_primitives::Rect::new(0, 5, 4, 1));
    assert_eq!(list.len(), 1);
    let bbox = list[0];
    assert!(bbox.contains(tessera_primitives::Point { x: 0, y: 0 }));
    assert!(bbox.contains(tessera_primitives::Point { x: 2, y: 5 }));
    // rects span x=0..((N-1)*2+1); the bbox must cover that whole strip.
    assert!(
        tessera_primitives::Rect::new(0, 0, (MAX_PENDING_DAMAGE_RECTS * 2 - 1) as i32, 1)
            .fully_covered_by(&list)
    );
}

#[test]
fn wl_region_accumulator_is_bounded_and_conservative() {
    let mut rects = Vec::new();
    for index in 0..MAX_REGION_RECTS {
        push_region_rect(
            &mut rects,
            tessera_primitives::Rect::new((index * 3) as i32, 0, 1, 1),
        );
    }
    assert_eq!(rects.len(), MAX_REGION_RECTS);

    push_region_rect(&mut rects, tessera_primitives::Rect::new(0, 7, 2, 1));
    assert_eq!(rects.len(), 1);
    // rects span x=0..((N-1)*3+1); the bbox must cover that whole strip.
    assert!(
        tessera_primitives::Rect::new(0, 0, (MAX_REGION_RECTS * 3 - 2) as i32, 1)
            .fully_covered_by(&rects)
    );
    assert!(rects[0].contains(tessera_primitives::Point { x: 1, y: 7 }));
}

#[test]
fn app_geometry_memory_is_bounded_like_the_persisted_store() {
    // Client-churned app ids must not grow the in-memory geometry map past
    // the store's own entry ceiling. The state path must be redirected to a
    // scratch file before any persist call: `persist_app_geometry` writes
    // the store through `State::window_state_path`, which otherwise points
    // at the developer's real session state.
    let temp_dir = std::env::temp_dir().join(format!(
        "tessera_test_geometry_bound_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let mut state = State::new(std::ptr::null_mut());
    state.window_state_path = temp_dir.join("window_state.json");
    for index in 0..(crate::state::MAX_APP_GEOMETRY_ENTRIES + 25) {
        state.persist_app_geometry(
            &format!("com.example.churn-{index}"),
            tessera_primitives::Rect::new(index as i32, 0, 10, 10),
            None,
            None,
        );
    }
    assert!(
        state.last_app_geometries.len() <= crate::state::MAX_APP_GEOMETRY_ENTRIES,
        "in-memory map grew to {}",
        state.last_app_geometries.len()
    );
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn test_inv_arch_74_physical_output_always_outranks_virtual_primary() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    server.set_outputs(vec![tessera_desktop::output::OutputInfo {
        connector: "eDP-1".into(),
        geometry: tessera_desktop::output::OutputGeometry {
            mode: tessera_desktop::output::OutputMode {
                width: 1920,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: tessera_desktop::output::Scale::IDENTITY,
            transform: tessera_primitives::Transform::Normal,
            logical_origin: tessera_primitives::Point { x: 0, y: 0 },
        },
        kind: tessera_desktop::output::OutputKind::Internal,
        available_modes: Vec::new(),
        color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
    }]);
    let virt = server
        .create_virtual_output(
            Some(42),
            tessera_desktop::output::VirtualOutputSpec {
                connector: Some("VIRTUAL-1".into()),
                mode: tessera_desktop::output::OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60_000,
                },
                scale: tessera_desktop::output::Scale::IDENTITY,
                logical_origin: None,
                persistent: false,
            },
        )
        .expect("create VIRTUAL-1");
    assert_eq!(virt.connector, "VIRTUAL-1");
    assert_eq!(virt.kind, tessera_desktop::output::OutputKind::Virtual);
    assert_eq!(
        virt.geometry.logical_origin,
        tessera_primitives::Point { x: 1920, y: 0 }
    );

    // Even if an output policy marks VIRTUAL-1 as `primary = true`, eDP-1 must
    // remain at index 0 (`[INV-ARCH-74]`).
    server.set_output_policies(std::collections::HashMap::from([(
        "VIRTUAL-1".to_owned(),
        tessera_desktop::output::OutputPolicy {
            primary: true,
            ..Default::default()
        },
    )]));
    let infos = server.output_infos();
    assert_eq!(infos[0].connector, "eDP-1");
    assert!(infos[0].kind.is_physical());
    assert_eq!(infos[1].connector, "VIRTUAL-1");
    assert!(infos[1].kind.is_virtual());
}

#[test]
fn test_inv_arch_76_virtual_output_does_not_evict_the_physical_scene() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    server.set_outputs(vec![tessera_desktop::output::OutputInfo {
        connector: "eDP-1".into(),
        geometry: tessera_desktop::output::OutputGeometry {
            mode: tessera_desktop::output::OutputMode {
                width: 1920,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: tessera_desktop::output::Scale::IDENTITY,
            transform: tessera_primitives::Transform::Normal,
            logical_origin: tessera_primitives::Point { x: 0, y: 0 },
        },
        kind: tessera_desktop::output::OutputKind::Internal,
        available_modes: Vec::new(),
        color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
    }]);
    server
        .create_virtual_output(
            Some(98),
            tessera_desktop::output::VirtualOutputSpec {
                connector: Some("VIRTUAL-1".into()),
                mode: tessera_desktop::output::OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60_000,
                },
                scale: tessera_desktop::output::Scale::IDENTITY,
                logical_origin: None,
                persistent: false,
            },
        )
        .expect("create VIRTUAL-1");

    // One window per output: a physical one and a virtual-output one.
    let physical_window = tessera_desktop::window::WindowId(31);
    let virtual_window = tessera_desktop::window::WindowId(32);
    let client = server.state.authority.register_client(None);
    server
        .state
        .authority
        .create_interaction_group(
            client,
            &[physical_window, virtual_window],
            HUMAN_INTERACTION_DOMAIN,
        )
        .expect("register human window authority");

    let mut physical_rec = Box::new(SurfaceRec::new(0x710usize as *mut ffi::wl_resource));
    physical_rec.state = server.state.as_mut();
    physical_rec.xdg_toplevel = 0x711usize as *mut ffi::wl_resource;
    physical_rec.mapped = true;
    physical_rec.window.id = physical_window;
    physical_rec.position = tessera_primitives::Point { x: 100, y: 100 };
    physical_rec.window.position = physical_rec.position;
    physical_rec.window.size = tessera_primitives::Size { w: 400, h: 300 };
    physical_rec.width = 400;
    physical_rec.height = 300;
    physical_rec.pixels = vec![0xff; 400 * 300 * 4];
    server.state.surfaces.push(physical_rec.as_mut());

    let mut virtual_rec = Box::new(SurfaceRec::new(0x720usize as *mut ffi::wl_resource));
    virtual_rec.state = server.state.as_mut();
    virtual_rec.xdg_toplevel = 0x721usize as *mut ffi::wl_resource;
    virtual_rec.mapped = true;
    virtual_rec.window.id = virtual_window;
    // Inside VIRTUAL-1's logical rectangle, far from the physical one.
    virtual_rec.position = tessera_primitives::Point { x: 2100, y: 100 };
    virtual_rec.window.position = virtual_rec.position;
    virtual_rec.window.size = tessera_primitives::Size { w: 400, h: 300 };
    virtual_rec.width = 400;
    virtual_rec.height = 300;
    virtual_rec.pixels = vec![0xff; 400 * 300 * 4];
    server.state.surfaces.push(virtual_rec.as_mut());

    let snapshot = server.workspace_snapshot();
    let physical_ws = snapshot
        .outputs
        .iter()
        .find(|output| output.connector == "eDP-1")
        .and_then(|output| output.workspaces.first())
        .map(|ws| ws.id)
        .expect("eDP-1 workspace");
    let virtual_ws = snapshot
        .outputs
        .iter()
        .find(|output| output.connector == "VIRTUAL-1")
        .and_then(|output| output.workspaces.first())
        .map(|ws| ws.id)
        .expect("VIRTUAL-1 workspace");
    server
        .state
        .workspaces
        .place_toplevel(physical_ws, physical_window);
    server
        .state
        .workspaces
        .place_toplevel(virtual_ws, virtual_window);

    // `[INV-ARCH-76]` is about *scoping*, not blanket suppression: the
    // physical scene keeps its own window while the virtual output's window is
    // excluded. An over-filtering `physical_render_visible` (e.g. dropping
    // every window whenever a virtual output exists) would fail here.
    let physical_scene = server.desktop_frame_sets();
    assert_eq!(
        physical_scene.shm.len(),
        1,
        "the physical window must survive a concurrent virtual output"
    );
    assert_eq!(physical_scene.shm[0].window, Some(physical_window));
    assert!(physical_scene.visible.contains(&physical_window));
    assert!(!physical_scene.visible.contains(&virtual_window));

    // The virtual output renders only its own window — never the physical one.
    let virtual_frames = server.output_capture_frames("VIRTUAL-1");
    assert_eq!(virtual_frames.len(), 1);
    assert_eq!(virtual_frames[0].window, Some(virtual_window));

    server.state.surfaces.clear();
}

#[test]
fn test_inv_arch_75_connection_disconnect_reclaims_virtual_outputs_and_relocates_windows() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    server.set_outputs(vec![tessera_desktop::output::OutputInfo {
        connector: "eDP-1".into(),
        geometry: tessera_desktop::output::OutputGeometry {
            mode: tessera_desktop::output::OutputMode {
                width: 1920,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: tessera_desktop::output::Scale::IDENTITY,
            transform: tessera_primitives::Transform::Normal,
            logical_origin: tessera_primitives::Point { x: 0, y: 0 },
        },
        kind: tessera_desktop::output::OutputKind::Internal,
        available_modes: Vec::new(),
        color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
    }]);
    server
        .create_virtual_output(
            Some(99),
            tessera_desktop::output::VirtualOutputSpec {
                connector: Some("VIRTUAL-1".into()),
                mode: tessera_desktop::output::OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60_000,
                },
                scale: tessera_desktop::output::Scale::IDENTITY,
                logical_origin: None,
                persistent: false,
            },
        )
        .expect("create VIRTUAL-1");

    // Place a mapped window on VIRTUAL-1's current workspace at (2100, 120).
    let window_id = tessera_desktop::window::WindowId(17);
    let client = server.state.authority.register_client(None);
    server
        .state
        .authority
        .create_interaction_group(client, &[window_id], HUMAN_INTERACTION_DOMAIN)
        .expect("register human window authority");
    let mut rec = Box::new(SurfaceRec::new(0x700usize as *mut ffi::wl_resource));
    rec.state = server.state.as_mut();
    rec.xdg_toplevel = 0x701usize as *mut ffi::wl_resource;
    rec.mapped = true;
    rec.window.id = window_id;
    rec.position = tessera_primitives::Point { x: 2100, y: 120 };
    rec.window.position = rec.position;
    rec.window.size = tessera_primitives::Size { w: 400, h: 300 };
    rec.width = 400;
    rec.height = 300;
    rec.pixels = vec![0xff; 400 * 300 * 4];
    server.state.surfaces.push(rec.as_mut());

    let virt_ws = server
        .workspace_snapshot()
        .outputs
        .iter()
        .find(|output| output.connector == "VIRTUAL-1")
        .and_then(|output| output.workspaces.first())
        .map(|ws| ws.id)
        .expect("VIRTUAL-1 workspace");
    server.state.workspaces.place_toplevel(virt_ws, window_id);

    // `[INV-ARCH-76]`: while on VIRTUAL-1, the window is excluded from the
    // physical desktop_frame_sets() but present in output_capture_frames("VIRTUAL-1").
    assert!(server.desktop_frame_sets().shm.is_empty());
    assert_eq!(server.output_capture_frames("VIRTUAL-1").len(), 1);

    // Disconnecting connection 99 reclaims VIRTUAL-1 and relocates the window
    // into eDP-1's bounds (`[INV-ARCH-75]`).
    let reclaimed = server.disconnect_virtual_outputs(99);
    assert_eq!(reclaimed, vec!["VIRTUAL-1".to_string()]);
    assert!(!server.is_virtual_output("VIRTUAL-1"));
    assert_eq!(rec.position, tessera_primitives::Point { x: 180, y: 120 });
    server.switch_workspace_to(virt_ws);
    assert_eq!(server.desktop_frame_sets().shm.len(), 1);
    server.state.surfaces.clear();
}

#[test]
fn test_inv_arch_78_output_scoped_input_rejects_out_of_bounds_and_locked_sessions() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    server.set_outputs(vec![tessera_desktop::output::OutputInfo {
        connector: "eDP-1".into(),
        geometry: tessera_desktop::output::OutputGeometry {
            mode: tessera_desktop::output::OutputMode {
                width: 1920,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: tessera_desktop::output::Scale::IDENTITY,
            transform: tessera_primitives::Transform::Normal,
            logical_origin: tessera_primitives::Point { x: 0, y: 0 },
        },
        kind: tessera_desktop::output::OutputKind::Internal,
        available_modes: Vec::new(),
        color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
    }]);
    server
        .create_virtual_output(
            Some(1),
            tessera_desktop::output::VirtualOutputSpec {
                connector: Some("VIRTUAL-1".into()),
                mode: tessera_desktop::output::OutputMode {
                    width: 1280,
                    height: 720,
                    refresh_mhz: 60_000,
                },
                scale: tessera_desktop::output::Scale::IDENTITY,
                logical_origin: None,
                persistent: false,
            },
        )
        .expect("create VIRTUAL-1");

    // In-bounds click at (100, 50) on VIRTUAL-1 translates to global (2020, 50).
    let events = server
        .prepare_output_synthetic_input(
            "VIRTUAL-1",
            &[tessera_primitives::input::SyntheticInputAction::Click {
                position: tessera_primitives::Point { x: 100, y: 50 },
                button: 0x110,
            }],
        )
        .expect("in-bounds output input must succeed");
    assert!(matches!(
        events.first(),
        Some(tessera_primitives::input::InputEvent::PointerMotion { x, y, .. })
            if (*x - 2020.0).abs() < f32::EPSILON && (*y - 50.0).abs() < f32::EPSILON
    ));

    // Out-of-bounds coordinates and unknown connectors must be rejected.
    assert!(
        server
            .prepare_output_synthetic_input(
                "VIRTUAL-1",
                &[
                    tessera_primitives::input::SyntheticInputAction::PointerMove {
                        position: tessera_primitives::Point { x: 1280, y: 50 },
                    }
                ],
            )
            .is_none()
    );
    assert!(
        server
            .prepare_output_synthetic_input(
                "VIRTUAL-99",
                &[
                    tessera_primitives::input::SyntheticInputAction::PointerMove {
                        position: tessera_primitives::Point { x: 10, y: 10 },
                    }
                ],
            )
            .is_none()
    );

    // Locked sessions must fail closed.
    server.state.session_lock_phase = crate::SessionLockPhase::Locked {
        frame_pending: false,
    };
    assert!(
        server
            .prepare_output_synthetic_input(
                "VIRTUAL-1",
                &[
                    tessera_primitives::input::SyntheticInputAction::PointerMove {
                        position: tessera_primitives::Point { x: 10, y: 10 },
                    }
                ],
            )
            .is_none()
    );
}
