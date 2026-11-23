use super::*;

#[test]
fn window_feedback_follows_complete_mixed_backing_tree_before_modal() {
    use OrderedSurfaceSource::{Dmabuf, Shm};
    use tessera_desktop::window::WindowId;
    // Below-parent subsurface, parent, above-parent subsurface, popup,
    // then a modal dialog and a nested modal, each with its own tree.
    let sources = ordered_surface_sources(
        &[10, 11, 12, 13, 20, 21, 30],
        &[10, 12, 20, 30],
        &[11, 13, 21],
    );
    let surfaces = ordered_window_surfaces(
        sources,
        |source| {
            Some(WindowId(match source {
                Shm(0 | 1) | Dmabuf(0 | 1) => 1,
                Shm(2) | Dmabuf(2) => 2,
                Shm(3) => 3,
                _ => unreachable!(),
            }))
        },
        None,
        None,
    );
    assert_eq!(
        surfaces,
        vec![
            (Shm(0), Some(WindowId(1)), false),
            (Dmabuf(0), Some(WindowId(1)), false),
            (Shm(1), Some(WindowId(1)), false),
            (Dmabuf(1), Some(WindowId(1)), true),
            (Shm(2), Some(WindowId(2)), false),
            (Dmabuf(2), Some(WindowId(2)), true),
            (Shm(3), Some(WindowId(3)), true),
        ]
    );
}

#[test]
fn workspace_feedback_excludes_filtered_and_unordered_windows() {
    use OrderedSurfaceSource::Shm;
    use tessera_desktop::window::WindowId;
    let sources = ordered_surface_sources(&[10, 20, 21], &[10, 20, 21, 30], &[]);
    let filter = HashSet::from([WindowId(2), WindowId(3)]);
    let surfaces = ordered_window_surfaces(
        sources,
        |source| {
            Some(WindowId(match source {
                Shm(0) => 1,
                Shm(1 | 2) => 2,
                Shm(3) => 3,
                _ => unreachable!(),
            }))
        },
        Some(&filter),
        None,
    );
    assert_eq!(
        surfaces,
        vec![
            (Shm(1), Some(WindowId(2)), false),
            (Shm(2), Some(WindowId(2)), true),
        ]
    );
}

#[test]
fn excluded_window_tree_and_feedback_leave_the_primary_pass() {
    use OrderedSurfaceSource::Shm;
    use tessera_desktop::window::WindowId;
    let sources = ordered_surface_sources(&[10, 20, 21], &[10, 20, 21], &[]);
    let excluded = HashSet::from([WindowId(2)]);
    let surfaces = ordered_window_surfaces(
        sources,
        |source| {
            Some(WindowId(match source {
                Shm(0) => 1,
                Shm(1 | 2) => 2,
                _ => unreachable!(),
            }))
        },
        None,
        Some(&excluded),
    );
    assert_eq!(surfaces, vec![(Shm(0), Some(WindowId(1)), true)]);
}

#[test]
fn unowned_surfaces_do_not_receive_window_feedback() {
    use OrderedSurfaceSource::Shm;
    use tessera_desktop::window::WindowId;
    let surfaces = ordered_window_surfaces(
        vec![Shm(0), Shm(1)],
        |source| (source == Shm(0)).then_some(WindowId(1)),
        None,
        None,
    );
    assert_eq!(
        surfaces,
        vec![(Shm(0), Some(WindowId(1)), true), (Shm(1), None, false),]
    );
}

#[test]
fn modifier_preference_is_stable_and_keeps_linear_last() {
    let tiled_a = 0x100;
    let tiled_b = 0x200;
    let mut modifiers = vec![
        drm_fmt::DRM_FORMAT_MOD_LINEAR,
        tiled_a,
        drm_fmt::DRM_FORMAT_MOD_LINEAR,
        tiled_b,
    ];
    prefer_native_modifiers(&mut modifiers);
    assert_eq!(
        modifiers,
        vec![
            tiled_a,
            tiled_b,
            drm_fmt::DRM_FORMAT_MOD_LINEAR,
            drm_fmt::DRM_FORMAT_MOD_LINEAR,
        ]
    );
}

#[test]
fn reusable_dmabuf_wait_is_once_per_commit_generation() {
    assert!(reusable_acquire_wait_required(7, 8, 12));
    assert!(!reusable_acquire_wait_required(8, 8, 12));
    assert!(!reusable_acquire_wait_required(7, 8, -1));
}

/// GC drops only the textures for ids not present in the live set. The
/// cache itself is empty here (no gfx device in unit tests), but the
/// bookkeeping on `failed_imports` is exercised by the same path.
#[test]
fn gc_keeps_live_drops_dead() {
    let mut r = Renderer::new();
    // Simulate a prior failed import so we can verify GC drops its record.
    r.failed_imports.insert(7, ());
    r.failed_imports.insert(9, ());
    // Live set: 7 stays, 9 goes.
    r.gc([7usize].into_iter());
    assert!(
        r.failed_imports.contains_key(&7),
        "live id should be retained"
    );
    assert!(
        !r.failed_imports.contains_key(&9),
        "dead id should be collected"
    );
}

/// Subsurfaces re-use the same cache as toplevels (keyed by surface id).
/// Verify that calling `gc` with both id sets keeps both alive.
#[test]
fn gc_keeps_both_toplevel_and_subsurface_ids() {
    let mut r = Renderer::new();
    r.failed_imports.insert(1, ());
    r.failed_imports.insert(2, ());
    r.gc([1usize, 2].into_iter());
    assert!(r.failed_imports.contains_key(&1));
    assert!(r.failed_imports.contains_key(&2));
}

/// `Transform::Normal` returns a borrowed Cow — no allocation, no copy.
#[test]
fn transform_normal_is_borrowed() {
    let pixels = vec![0u8; 8 * 4]; // 8 pixels, BGRA8
    let out = transform_pixels(&pixels, 8, 1, Transform::Normal);
    assert!(matches!(out, Cow::Borrowed(_)));
}

/// Rotate90 of a 2×2 grid swaps axes. Construct a grid with distinct
/// pixels per cell and verify the rotation places each in the right
/// destination cell.
#[test]
fn transform_rotate90_swaps_axes_and_places_pixels() {
    // 2×2 grid, each pixel is BGRA8 with a unique value in the R channel.
    // Layout:  A=0x10 at (0,0)   B=0x20 at (1,0)
    //          C=0x30 at (0,1)   D=0x40 at (1,1)
    let pixels = [
        0x10, 0, 0, 0xff, 0x20, 0, 0, 0xff, // row 0: A B
        0x30, 0, 0, 0xff, 0x40, 0, 0, 0xff, // row 1: C D
    ];
    let out = transform_pixels(&pixels, 2, 2, Transform::Rotate90);
    let owned = match out {
        Cow::Owned(v) => v,
        _ => panic!("rotate90 should own its output"),
    };
    // Rotate90 means buffer is 90° CCW from upright; display applies
    // 90° CW. Result of rotating  A B  by 90° CW:
    //                        C D
    //   C A
    //   D B
    let r = |x: usize, y: usize| owned[(y * 2 + x) * 4];
    assert_eq!(r(0, 0), 0x30, "top-left should be C");
    assert_eq!(r(1, 0), 0x10, "top-right should be A");
    assert_eq!(r(0, 1), 0x40, "bottom-left should be D");
    assert_eq!(r(1, 1), 0x20, "bottom-right should be B");
}

/// A 3×1 buffer rotated 90° becomes a 1×3 buffer (axes swap).
#[test]
fn transform_rotate90_swaps_axes_for_non_square() {
    let pixels = [
        0x10, 0, 0, 0xff, // (0,0)=A
        0x20, 0, 0, 0xff, // (1,0)=B
        0x30, 0, 0, 0xff, // (2,0)=C
    ];
    let (tw, th) = transformed_dims(3, 1, Transform::Rotate90);
    assert_eq!((tw, th), (1, 3));
    let out = transform_pixels(&pixels, 3, 1, Transform::Rotate90);
    let owned = match out {
        Cow::Owned(v) => v,
        _ => panic!("should own"),
    };
    // 1×3 layout.
    assert_eq!(owned.len(), 3 * 4);
}

/// Rotate180 reverses both axes.
#[test]
fn transform_rotate180_reverses_both_axes() {
    let pixels = [
        0x10, 0, 0, 0xff, 0x20, 0, 0, 0xff, // A B
        0x30, 0, 0, 0xff, 0x40, 0, 0, 0xff, // C D
    ];
    let out = transform_pixels(&pixels, 2, 2, Transform::Rotate180);
    let owned = match out {
        Cow::Owned(v) => v,
        _ => panic!("should own"),
    };
    // 180° of  A B  ->  D C
    //         C D      B A
    let r = |x: usize, y: usize| owned[(y * 2 + x) * 4];
    assert_eq!(r(0, 0), 0x40);
    assert_eq!(r(1, 0), 0x30);
    assert_eq!(r(0, 1), 0x20);
    assert_eq!(r(1, 1), 0x10);
}

/// FlipHorizontal mirrors within each row.
#[test]
fn transform_flip_horizontal_mirrors_row() {
    let pixels = [
        0x10, 0, 0, 0xff, 0x20, 0, 0, 0xff, // A B
        0x30, 0, 0, 0xff, 0x40, 0, 0, 0xff, // C D
    ];
    let out = transform_pixels(&pixels, 2, 2, Transform::FlipHorizontal);
    let owned = match out {
        Cow::Owned(v) => v,
        _ => panic!("should own"),
    };
    // Flip X:  A B  ->  B A
    //          C D      D C
    let r = |x: usize, y: usize| owned[(y * 2 + x) * 4];
    assert_eq!(r(0, 0), 0x20);
    assert_eq!(r(1, 0), 0x10);
    assert_eq!(r(0, 1), 0x40);
    assert_eq!(r(1, 1), 0x30);
}

/// Genie strips leave the destination untouched at progress zero.
#[test]
fn genie_strips_are_undeformed_at_progress_zero() {
    let target = tessera_primitives::Point { x: 500, y: 1000 };
    let strips = genie_strips(100.0, 50.0, 400.0, 240.0, target, 0.0);
    assert_eq!(strips.len(), GENIE_STRIPS);
    let n = GENIE_STRIPS as f32;
    for (i, strip) in strips.iter().enumerate() {
        assert_eq!(strip.dst_x, 100.0, "strip {i} x");
        assert_eq!(strip.dst_w, 400.0, "strip {i} w");
        assert!((strip.dst_y - (50.0 + 240.0 * i as f32 / n)).abs() < 1e-4);
        assert!((strip.src_v - i as f32 / n).abs() < 1e-6);
        assert!((strip.src_dv - 1.0 / n).abs() < 1e-6);
    }
}

/// The pinch strengthens toward the bottom edge and funnels the lowest
/// strip onto the icon's centre as progress completes.
#[test]
fn genie_strips_funnel_the_bottom_edge_into_the_icon() {
    let target = tessera_primitives::Point { x: 700, y: 1000 };
    let strips = genie_strips(100.0, 50.0, 400.0, 240.0, target, 1.0);
    let widths: Vec<f32> = strips.iter().map(|strip| strip.dst_w).collect();
    assert!(
        widths.windows(2).all(|pair| pair[0] >= pair[1]),
        "pinch widens downward: {widths:?}"
    );
    let top = strips.first().unwrap();
    assert!(top.dst_w > 300.0, "top edge barely pinches: {top:?}");
    let bottom = strips.last().unwrap();
    let bottom_centre = bottom.dst_x + bottom.dst_w * 0.5;
    assert!(
        (bottom_centre - 700.0).abs() < 30.0,
        "bottom edge converges on the icon: {bottom:?}"
    );
    assert!(
        bottom.dst_w < 200.0,
        "bottom edge nearly closes: {bottom:?}"
    );
    // Vertical coverage stays intact: the funnel is horizontal only.
    let covered = strips.iter().map(|strip| strip.dst_h).sum::<f32>();
    assert!((covered - 240.0).abs() < 1e-3);
}

fn shm_frame<'a>(
    width: i32,
    height: i32,
    geometry: tessera_primitives::SurfaceGeometry,
    damage: &'a [tessera_primitives::Rect],
    damage_full: bool,
) -> SurfacePixels<'a> {
    SurfacePixels {
        id: 1,
        window: None,
        width,
        height,
        generation: 1,
        pixels: &[],
        geometry,
        damage,
        damage_full,
        opaque_region: None,
        color: None,
    }
}

#[test]
fn shm_upload_plan_distinguishes_full_empty_and_region() {
    use tessera_primitives::{Rect, Size, SurfaceGeometry};

    let geometry = SurfaceGeometry {
        viewport_dst: Some(Size { w: 4, h: 2 }),
        ..Default::default()
    };
    assert_eq!(
        plan_shm_upload(&shm_frame(8, 4, geometry, &[], true)),
        ShmUploadPlan::Full
    );
    assert_eq!(
        plan_shm_upload(&shm_frame(8, 4, geometry, &[], false)),
        ShmUploadPlan::None,
        "an empty non-full list is a visible no-op, not a full upload"
    );
    assert_eq!(
        plan_shm_upload(&shm_frame(8, 4, geometry, &[Rect::new(4, 0, 1, 1)], false,)),
        ShmUploadPlan::None,
        "damage outside the surface must not fall back to a full upload"
    );
    // A 4x2 viewport destination over an 8x4 buffer maps 2:1.
    assert_eq!(
        plan_shm_upload(&shm_frame(8, 4, geometry, &[Rect::new(1, 1, 2, 1)], false,)),
        ShmUploadPlan::Region(Rect::new(2, 2, 4, 2))
    );
}

/// `destination_size` covers the four viewport/scale combinations.
#[test]
fn destination_size_handles_viewport_and_scale() {
    use tessera_primitives::Rect;
    use tessera_primitives::Size;

    // No viewport, scale 1: post-transform buffer dims as-is.
    let (w, h) = destination_size((100, 50), None, None, 1);
    assert_eq!((w, h), (100.0, 50.0));

    // No viewport, scale 2: client committed at 2× the logical size.
    let (w, h) = destination_size((100, 50), None, None, 2);
    assert_eq!((w, h), (50.0, 25.0));

    // Source-only: coordinates are already after buffer scale.
    let src = Rect::new(10, 10, 80, 40);
    let (w, h) = destination_size((100, 50), Some(src), None, 1);
    assert_eq!((w, h), (80.0, 40.0));
    let (w, h) = destination_size((100, 50), Some(src), None, 2);
    assert_eq!((w, h), (80.0, 40.0));

    // Destination wins regardless of scale (already logical px).
    let dst = Size { w: 30, h: 20 };
    let (w, h) = destination_size((100, 50), Some(src), Some(dst), 1);
    assert_eq!((w, h), (30.0, 20.0));
    let (w, h) = destination_size((100, 50), Some(src), Some(dst), 2);
    assert_eq!((w, h), (30.0, 20.0));

    // Destination-only with scale: dst taken as-is, source falls back
    // to whole buffer (UV math), scale ignored on dst.
    let (w, h) = destination_size((100, 50), None, Some(dst), 2);
    assert_eq!((w, h), (30.0, 20.0));
}

/// A malformed `buffer_scale <= 0` is clamped to 1 rather than producing
/// a divide-by-zero or negative destination. The server and
/// `SurfaceGeometry::default` both ensure `>= 1`, so this is defence in
/// depth.
#[test]
fn destination_size_clamps_nonpositive_scale() {
    let (w, h) = destination_size((100, 50), None, None, 0);
    assert_eq!((w, h), (100.0, 50.0));
}

/// A scale-2 buffer is 3072x1920 pixels but its opaque region is still
/// expressed in the 1536x960 surface coordinate space. A full-surface
/// region must therefore produce exactly one full opaque blit, not a
/// half-sized opaque piece plus a second enlarged copy of the texture.
#[test]
fn opaque_split_uses_surface_coordinates_at_hidpi_scale() {
    let region = [tessera_primitives::Rect::new(0, 0, 1536, 960)];
    let (opaque, blended) = split_opaque_blit(
        (0.0, 0.0, 1536.0, 960.0),
        (1536.0, 960.0),
        Some(&region),
        true,
    );

    assert_eq!(
        opaque,
        vec![BlitRect {
            dst_x: 0.0,
            dst_y: 0.0,
            dst_w: 1536.0,
            dst_h: 960.0,
            src_u: 0.0,
            src_v: 0.0,
            src_du: 1.0,
            src_dv: 1.0,
        }]
    );
    assert!(blended.is_empty());
}

/// Client-owned region coordinates may approach the i32 limits. Clipping
/// must reject an entirely out-of-bounds rectangle without overflowing its
/// far-edge addition or disturbing the full blended fallback.
#[test]
fn opaque_split_saturates_extreme_client_rect_edges() {
    let region = [tessera_primitives::Rect::new(
        i32::MAX - 4,
        i32::MAX - 4,
        100,
        100,
    )];
    let (opaque, blended) =
        split_opaque_blit((0.0, 0.0, 128.0, 64.0), (128.0, 64.0), Some(&region), true);

    assert!(opaque.is_empty());
    assert_eq!(
        blended,
        vec![BlitRect {
            dst_x: 0.0,
            dst_y: 0.0,
            dst_w: 128.0,
            dst_h: 64.0,
            src_u: 0.0,
            src_v: 0.0,
            src_du: 1.0,
            src_dv: 1.0,
        }]
    );
}

/// Transition scaling changes only the destination mapping; the source UV
/// remains normalised against the original surface logical extent.
#[test]
fn opaque_split_scales_destination_without_rescaling_source_uv() {
    let region = [tessera_primitives::Rect::new(0, 0, 1536, 120)];
    let (opaque, blended) = split_opaque_blit(
        (10.0, 20.0, 3072.0, 1920.0),
        (1536.0, 960.0),
        Some(&region),
        true,
    );

    assert_eq!(opaque.len(), 1);
    assert_eq!(opaque[0].dst_x, 10.0);
    assert_eq!(opaque[0].dst_y, 20.0);
    assert_eq!(opaque[0].dst_w, 3072.0);
    assert_eq!(opaque[0].dst_h, 240.0);
    assert_eq!(opaque[0].src_u, 0.0);
    assert_eq!(opaque[0].src_v, 0.0);
    assert_eq!(opaque[0].src_du, 1.0);
    assert_eq!(opaque[0].src_dv, 0.125);

    assert_eq!(blended.len(), 1);
    assert_eq!(blended[0].dst_y, 260.0);
    assert_eq!(blended[0].dst_h, 1680.0);
    assert_eq!(blended[0].src_v, 0.125);
    assert_eq!(blended[0].src_dv, 0.875);
}

#[test]
fn viewport_source_converts_post_scale_coordinates_to_buffer_uvs() {
    let src = tessera_primitives::Rect::new(10, 5, 30, 20);
    assert_eq!(viewport_uv(src, (200, 100), 2), (0.1, 0.1, 0.3, 0.4));
}

#[test]
fn mixed_backing_sources_follow_compositor_order() {
    let sources = ordered_surface_sources(&[10, 20, 30], &[10, 30], &[20]);
    assert_eq!(
        sources,
        vec![
            OrderedSurfaceSource::Shm(0),
            OrderedSurfaceSource::Dmabuf(0),
            OrderedSurfaceSource::Shm(1),
        ]
    );
}

#[test]
fn mixed_backing_window_chrome_does_not_escape_its_tree_order() {
    // Window A's dma-buf chrome must be drawn before the shm root of
    // foreground window B. A global dma-buf pass would reverse them.
    let sources = ordered_surface_sources(&[10, 11, 20, 21], &[10, 20], &[11, 21]);
    assert_eq!(
        sources,
        vec![
            OrderedSurfaceSource::Shm(0),
            OrderedSurfaceSource::Dmabuf(0),
            OrderedSurfaceSource::Shm(1),
            OrderedSurfaceSource::Dmabuf(1),
        ]
    );
}

#[test]
fn a_frame_missing_from_the_authoritative_order_stays_hidden() {
    let sources = ordered_surface_sources(&[10], &[10, 20], &[]);
    assert_eq!(sources, vec![OrderedSurfaceSource::Shm(0)]);
}

#[test]
fn workspace_filter_is_closed_to_foreign_and_unowned_surfaces() {
    use tessera_desktop::window::WindowId;

    let filter = HashSet::from([WindowId(2)]);
    assert!(surface_passes_window_filter(
        Some(WindowId(2)),
        Some(&filter)
    ));
    assert!(!surface_passes_window_filter(
        Some(WindowId(1)),
        Some(&filter)
    ));
    assert!(!surface_passes_window_filter(None, Some(&filter)));
    assert!(surface_passes_window_filter(None, None));
}

#[test]
fn mapped_surface_modulation_separates_brightness_from_opacity() {
    let style = MappedSurfaceStyle {
        opacity: 0.5,
        brightness: 0.75,
        rounded_clip: tessera_primitives::Rect::new(0, 0, 100, 80),
        corner_radius: 12.0,
    };
    assert_eq!(mapped_surface_modulation(style), (191, 128));
    assert_eq!(
        mapped_surface_modulation(MappedSurfaceStyle {
            opacity: f32::NAN,
            brightness: f32::NAN,
            ..style
        }),
        (255, 255)
    );
}

#[test]
fn resize_shadow_follows_direct_resize_eligibility() {
    let mut window = tessera_desktop::window::Window::new(tessera_desktop::window::WindowId(1));
    window.size = tessera_primitives::Size { w: 640, h: 480 };
    window.layout_role = tessera_desktop::layout::LayoutRole::Floating;
    assert!(window_casts_resize_shadow(&window));

    window.state.maximized = true;
    assert!(!window_casts_resize_shadow(&window));
    window.state.maximized = false;
    window.read_only = true;
    assert!(!window_casts_resize_shadow(&window));
    window.read_only = false;
    window.layout_role = tessera_desktop::layout::LayoutRole::Tiled;
    assert!(!window_casts_resize_shadow(&window));
}

#[test]
fn evict_surface_purges_cached_textures_immediately() {
    let mut renderer = Renderer::new();
    // Insert dummy failed import and verify evict clears it
    renderer.failed_imports.insert(42, ());
    assert!(renderer.failed_imports.contains_key(&42));
    renderer.evict_surface(42);
    assert!(!renderer.failed_imports.contains_key(&42));
}
