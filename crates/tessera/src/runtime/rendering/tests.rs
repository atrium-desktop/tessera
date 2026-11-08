use super::*;

#[test]
fn focused_preview_content_keeps_one_full_brightness_target() {
    let focused = tessera_desktop::window::WindowId(7);
    let sibling = tessera_desktop::window::WindowId(8);
    assert_eq!(
        tessera_shell::component::preview::content_brightness(Some(focused), focused, 0.74),
        1.0
    );
    assert_eq!(
        tessera_shell::component::preview::content_brightness(Some(focused), sibling, 0.74),
        0.74
    );
    assert_eq!(
        tessera_shell::component::preview::content_brightness(None, sibling, 0.74),
        1.0
    );
}

#[test]
fn screenshot_freeze_keeps_the_trigger_cursor_snapshot() {
    let trigger = CaptureCursorState {
        position: (42.25, 73.5),
        shape: 7,
        hidden: true,
        client_surface: true,
    };
    let later = CaptureCursorState {
        position: (900.0, 500.0),
        shape: 1,
        hidden: true,
        client_surface: false,
    };
    let mut freeze = ScreenshotFreeze::new();

    freeze.request_open(Some(trigger));
    freeze.request_open(Some(later));
    assert_eq!(freeze.trigger_cursor(), Some(trigger));

    freeze.disarm();
    assert_eq!(freeze.trigger_cursor(), None);
}

#[test]
fn screenshot_freeze_lifecycle_tracks_capture_and_opened_phases() {
    let mut freeze = ScreenshotFreeze::new();
    assert!(!freeze.needs_capture());
    assert!(!freeze.active());
    assert!(!freeze.pending_open);
    assert!(!freeze.opened);

    freeze.request_open(None);
    assert!(freeze.needs_capture());
    assert!(!freeze.active());
    assert!(freeze.pending_open);
    assert!(!freeze.opened);

    // Frame capture completes with effects and chrome drawn into the snapshot.
    freeze.mark_captured(0);
    assert!(!freeze.needs_capture());
    assert!(freeze.active());
    assert!(freeze.pending_open);
    assert!(!freeze.opened);

    // Selector chrome opens over the frozen desktop snapshot.
    freeze.mark_opened();
    assert!(!freeze.needs_capture());
    assert!(freeze.active());
    assert!(!freeze.pending_open);
    assert!(freeze.opened);

    // Confirmation / dismissal disarms the freeze session.
    assert!(freeze.should_disarm(false));
    assert!(!freeze.should_disarm(true));
    freeze.disarm();
    assert!(!freeze.active());
    assert!(!freeze.needs_capture());
}

#[test]
fn opaque_frame_fill_replaces_undefined_and_previous_contents() {
    let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
        return;
    };
    let size = (32, 24);
    let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
    let mut canvas = flux::Canvas::new(&surface).unwrap();

    for expected in [[13, 77, 191, 255], [211, 43, 29, 255]] {
        let mut frame = surface.begin_frame().unwrap();
        let session = begin_opaque_frame(
            &mut canvas,
            &mut frame,
            flux::rgba(expected[0], expected[1], expected[2], expected[3]),
        )
        .unwrap();
        session.end().unwrap();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
        surface.read_pixels(&mut pixels).unwrap();
        // ADR-0069 (optics): the output transform dithers ±1 LSB when
        // quantizing to 8 bit, so an opaque fill is exact up to one
        // quantum per channel, not bit-identical.
        assert!(
            pixels.chunks_exact(4).all(|pixel| {
                pixel
                    .iter()
                    .zip(expected.iter())
                    .all(|(got, want)| got.abs_diff(*want) <= 1)
            }),
            "opaque fill did not replace every output pixel"
        );
    }
}

#[test]
fn damaged_base_and_stencil_overlay_preserve_pixels_outside_the_scissor() {
    let Ok(device) = flux::Device::new(true, &[], &[], 1) else {
        return;
    };
    let size = (32, 24);
    let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
    let mut canvas = flux::Canvas::new(&surface).unwrap();

    let mut frame = surface.begin_frame().unwrap();
    let session =
        begin_opaque_frame(&mut canvas, &mut frame, flux::rgba(200, 30, 20, 255)).unwrap();
    session.end().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut frame = surface.begin_frame().unwrap();
    /* Whole-output repaint on purpose. This test exists to prove the *pass
     * boundary* between the optimized base pass and the stencil-capable chrome
     * overlay, not to pin damage localization — the latter is covered by
     * `frame_damage_scope_*` above. Using a narrow rect here would make the
     * assertions depend on flux's swapchain epoch history, which is a different
     * subsystem's contract and would silently couple the two. */
    let repaint = FrameDamage::Full;
    let session = begin_opaque_frame_repaint(
        &mut canvas,
        &mut frame,
        size,
        flux::rgba(10, 80, 220, 255),
        &repaint,
    )
    .unwrap();
    // End the optimized image/base pass, then exercise the exact pass
    // boundary used before Lens. A self-intersecting fill is intentional:
    // Flux rejects it from a no-stencil pass, so successful checked end
    // proves the overlay really has stencil rather than merely being a
    // second no-stencil LOAD pass.
    session.end().unwrap();
    let session = begin_stencil_frame_overlay(
        &mut canvas,
        &mut frame,
        frame_damage_scope(&repaint, (256, 256)),
    )
    .unwrap();
    let arena = flux::Arena::with_capacity(4096).unwrap();
    let path = flux::Path::new(&arena).unwrap();
    path.move_to(11.0, 8.0)
        .line_to(15.0, 12.0)
        .line_to(11.0, 12.0)
        .line_to(15.0, 8.0)
        .close();
    session.fill_path(
        &path,
        flux::FillRule::NonZero,
        &flux::Brush::solid(flux::rgba(20, 220, 70, 255)),
    );
    session.end().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
    surface.read_pixels(&mut pixels).unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * size.0 as usize + x) * 4..][..4];
    /* A full repaint replaces every pixel, up to the output transform's ±1 LSB
     * dither (Optics ADR-0069). */
    let repainted = [10, 80, 220, 255];
    for (x, y) in [(0, 0), (9, 7), (17, 14), (18, 15)] {
        let got = pixel(x, y);
        assert!(
            got.iter()
                .zip(repainted.iter())
                .all(|(g, w)| g.abs_diff(*w) <= 1),
            "base pass did not repaint ({x},{y}): {got:?}"
        );
    }

    /* The overlay's self-intersecting "bowtie" must have painted green over the
     * base blue somewhere in its bounds. Exact pixels are not asserted: the two
     * lobes meet at (13,10), where coverage is partial and the result is a
     * blend. What matters is that the second pass drew at all — and since flux
     * rejects a self-intersecting fill from a *no-stencil* pass, a successful
     * checked end plus green pixels proves the overlay really has stencil
     * rather than merely being a second no-stencil LOAD pass. */
    let mut painted = 0;
    for y in 8..=12 {
        for x in 11..=15 {
            let p = pixel(x, y);
            let base = p
                .iter()
                .zip(repainted.iter())
                .all(|(g, w)| g.abs_diff(*w) <= 1);
            if !base && p[1] > p[2] {
                painted += 1; /* greener than the blue base */
            }
        }
    }
    assert!(
        painted > 0,
        "stencil overlay drew nothing: no green pixel inside its bounds"
    );
}

/// The output pass scope is the damage *set*, not its bounding box.
///
/// A compositor frame's damage is routinely two distant bands — a top HUD and a
/// bottom dock — whose union is the whole output. Collapsing the set to that box
/// before flux sees it would make every chrome repaint an output-wide composite
/// and destroy the localisation the damage pipeline exists to carry
/// (ADR-0203, Optics ADR-0149). The list is therefore the contract here.
#[test]
fn frame_damage_scope_preserves_the_set_instead_of_its_bounding_box() {
    let damage = FrameDamage::Area(vec![
        tessera_primitives::Rect::new(11, 7, 5, 9),
        tessera_primitives::Rect::new(29, 3, 4, 8),
    ]);
    let scope = frame_damage_scope(&damage, (256, 256));
    assert_eq!(
        scope,
        vec![
            flux::CanvasRenderArea {
                x: 11.0,
                y: 7.0,
                width: 5.0,
                height: 9.0,
            },
            flux::CanvasRenderArea {
                x: 29.0,
                y: 3.0,
                width: 4.0,
                height: 8.0,
            },
        ],
        "each damaged region must reach flux as its own rectangle"
    );

    /* `Full` and `None` both mean the whole destination, which flux expresses
     * as an empty scope. A non-empty scope would be a *narrower* claim. */
    assert!(frame_damage_scope(&FrameDamage::Full, (256, 256)).is_empty());
    assert!(frame_damage_scope(&FrameDamage::None, (256, 256)).is_empty());
}

/// Both flux backends reject a scope naming no pixel of the target, or a rect
/// with a degenerate dimension. Handing one out does not merely skip a
/// rectangle: the pass fails to begin and the frame loses its chrome, which
/// surfaced as `shell chrome render failed: Render(FLUX_ERROR_INVALID_ARGUMENT)`
/// and then, on the shell side, as a per-frame rebuild loop.
#[test]
fn test_inv_arch_94_every_scope_rect_is_clamped_to_the_framebuffer() {
    // Damage straddling the right and bottom edges clamps inward, one rect at
    // a time.
    let edge = FrameDamage::Area(vec![
        tessera_primitives::Rect::new(1800, 1000, 400, 400),
        tessera_primitives::Rect::new(10, 10, 20, 20),
    ]);
    assert_eq!(
        frame_damage_scope(&edge, (1920, 1080)),
        vec![
            flux::CanvasRenderArea {
                x: 1800.0,
                y: 1000.0,
                width: 120.0,
                height: 80.0,
            },
            flux::CanvasRenderArea {
                x: 10.0,
                y: 10.0,
                width: 20.0,
                height: 20.0,
            },
        ]
    );

    // A negative origin clamps to zero without moving the far edge.
    let negative = FrameDamage::Area(vec![tessera_primitives::Rect::new(-30, -20, 100, 90)]);
    assert_eq!(
        frame_damage_scope(&negative, (1920, 1080)),
        vec![flux::CanvasRenderArea {
            x: 0.0,
            y: 0.0,
            width: 70.0,
            height: 70.0,
        }]
    );

    // A rect wholly off the framebuffer contributes nothing. When *no* rect
    // survives, the pass must still cover something: a full destination, never
    // a rejected rectangle and never an empty scope that would read as `Full`
    // arrived by accident.
    let offscreen = FrameDamage::Area(vec![tessera_primitives::Rect::new(5000, 5000, 100, 100)]);
    assert_eq!(
        frame_damage_scope(&offscreen, (1920, 1080)),
        vec![flux::CanvasRenderArea {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }]
    );
    let negative_far = FrameDamage::Area(vec![tessera_primitives::Rect::new(-500, 0, 100, 100)]);
    assert_eq!(
        frame_damage_scope(&negative_far, (1920, 1080)),
        vec![flux::CanvasRenderArea {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }]
    );

    // A one-pixel sliver is still legal, and an off-screen sibling is dropped
    // without discarding the sliver.
    let mixed = FrameDamage::Area(vec![
        tessera_primitives::Rect::new(10, 10, 1, 40),
        tessera_primitives::Rect::new(9000, 9000, 4, 4),
    ]);
    assert_eq!(
        frame_damage_scope(&mixed, (1920, 1080)),
        vec![flux::CanvasRenderArea {
            x: 10.0,
            y: 10.0,
            width: 1.0,
            height: 40.0,
        }]
    );
}

/// Window shadows are deferred effect outputs (Optics ADR-0147): `apply`
/// registers a pass, and the pixels exist only once something requires them.
/// `WindowShadowRenderer::prepare` therefore materializes each output at the
/// boundary it applied from, before the pointer escapes to
/// `tessera-composite`. Without that, the composite pass samples a pending
/// image and the draw is rejected — the shadow silently disappears instead of
/// the window losing its depth.
#[test]
fn window_shadows_are_materialized_before_they_are_composited() {
    use tessera_desktop::window::{Window, WindowShadowStyle};

    let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
        return;
    };
    let surface = flux::Surface::offscreen(&device, 320, 240).unwrap();
    let mut canvas = flux::Canvas::new(&surface).unwrap();
    let mut renderer = WindowShadowRenderer::new(&device).unwrap();
    let mut window = Window::new(tessera_desktop::window::WindowId(1));
    window.size = tessera_primitives::Size { w: 120, h: 80 };
    window.position = tessera_primitives::Point { x: 16, y: 24 };

    let mut frame = surface.begin_frame().unwrap();
    let shadows = renderer.prepare(
        &device,
        &mut canvas,
        &mut frame,
        std::slice::from_ref(&window),
        1.0,
        WindowShadowStyle::Soft,
    );
    assert_eq!(shadows.len(), 1, "a soft shadow is rendered per window");

    // Every registered pass ran: prepare required each output it returns, so
    // the plan reports no pruned pass.
    let counters = tessera_composite::effects::plan_counters(&frame)
        .expect("a recording frame reports its plan");
    assert_eq!(counters.executed, counters.registered);
    assert_eq!(counters.pruned, 0);

    // The pointer is now sampleable: drawing it inside a pass is accepted, and
    // that is the real assertion — a pending output would be rejected here.
    // The raw handle is read before the session borrows the canvas mutably:
    // the FFI draw needs it for the duration of the pass, and `begin_session`
    // holds an exclusive borrow.
    let canvas_raw = canvas.as_raw();
    let session = canvas
        .begin_session(
            &mut frame,
            flux::CanvasPassOptions {
                // A white clear, not a black one: the shadow's tint is black
                // (`ShadowParams::tint_*` are 0), so a black backdrop would
                // make "the shadow arrived" and "nothing was drawn"
                // indistinguishable pixel-for-pixel.
                clear: Some(flux::rgba(255, 255, 255, 255)),
                ..Default::default()
            },
        )
        .unwrap();
    let shadow = &shadows[0];
    let geometry = flux::sys::flux_geometry {
        kind: flux::sys::flux_geom_kind::FLUX_GEOM_RECT,
        _pad: [0; 3],
        stroke: flux::sys::flux_stroke_style::default(),
        __bindgen_anon_1: flux::sys::flux_geometry__bindgen_ty_1 {
            rect: flux::sys::flux_geom_rect_data {
                rect: flux::sys::flux_rect {
                    x: shadow.x,
                    y: shadow.y,
                    w: shadow.w,
                    h: shadow.h,
                },
            },
        },
    };
    let brush = flux::sys::flux_brush {
        kind: flux::sys::flux_brush_kind::FLUX_BRUSH_IMAGE_PATTERN,
        blend: flux::sys::flux_blend_mode::FLUX_BLEND_SRC_OVER,
        opacity: 1.0,
        __bindgen_anon_1: flux::sys::flux_brush__bindgen_ty_1 {
            image: flux::sys::flux_brush_image_data {
                image: shadow.raw,
                sampler: std::ptr::null_mut(),
                src_rect: flux::sys::flux_rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
                opaque_only: false,
                tint: 0xFFFFFFFF,
                clip_rect: flux::sys::flux_rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
                clip_radius: 0.0,
            },
        },
    };
    unsafe { flux::sys::flux_canvas_draw_geometry(canvas_raw, &geometry, &brush) };
    session.end().unwrap();
    assert!(
        flux::ErrorInfo::last().is_none_or(|info| {
            info.code == flux::sys::flux_result::FLUX_OK || info.function.is_none()
        }),
        "the shadow draw must not be rejected: {:?}",
        flux::ErrorInfo::last()
    );

    frame.submit().unwrap().present().unwrap();
    let mut pixels = vec![0u8; 320 * 240 * 4];
    surface.read_pixels(&mut pixels).unwrap();
    assert!(
        pixels
            .chunks_exact(4)
            .any(|pixel| pixel[3] == 255 && pixel[0] < 250),
        "the composited shadow must darken the white backdrop"
    );
}
