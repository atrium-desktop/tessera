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
    let repaint = FrameDamage::Area(vec![tessera_primitives::Rect::new(8, 6, 10, 9)]);
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
        frame_damage_render_area(&repaint, (256, 256)),
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
    // Under Optics ADR-0144, surface damage passes widen to the full extent to
    // keep rotating swapchains identical by construction.
    assert_eq!(pixel(0, 0), [10, 80, 220, 255]);
    assert_eq!(pixel(9, 7), [10, 80, 220, 255]);
    assert_eq!(pixel(17, 14), [10, 80, 220, 255]);
    assert_eq!(pixel(18, 15), [10, 80, 220, 255]);
}

#[test]
fn frame_damage_render_area_uses_the_exact_union() {
    let damage = FrameDamage::Area(vec![
        tessera_primitives::Rect::new(11, 7, 5, 9),
        tessera_primitives::Rect::new(29, 3, 4, 8),
    ]);
    assert_eq!(
        frame_damage_render_area(&damage, (256, 256)),
        Some(flux::CanvasRenderArea {
            x: 11,
            y: 3,
            width: 22,
            height: 13,
        })
    );
    assert_eq!(
        frame_damage_render_area(&FrameDamage::Full, (256, 256)),
        None
    );
    assert_eq!(
        frame_damage_render_area(&FrameDamage::None, (256, 256)),
        None
    );
}

/// Both flux backends reject a `renderArea` that is out of bounds, has a
/// negative origin, or has exactly one zero dimension
/// (`canvas_pass_render_area`). Handing one out does not merely skip a
/// rectangle: the pass fails to begin and the frame loses its chrome, which
/// surfaced as `shell chrome render failed: Render(FLUX_ERROR_INVALID_ARGUMENT)`.

#[test]
fn test_inv_arch_94_render_area_is_clamped_to_the_framebuffer() {
    // Damage straddling the right and bottom edges clamps inward.
    let edge = FrameDamage::Area(vec![tessera_primitives::Rect::new(1800, 1000, 400, 400)]);
    assert_eq!(
        frame_damage_render_area(&edge, (1920, 1080)),
        Some(flux::CanvasRenderArea {
            x: 1800,
            y: 1000,
            width: 120,
            height: 80,
        })
    );
    // A negative origin clamps to zero without changing the far edge.
    let negative = FrameDamage::Area(vec![tessera_primitives::Rect::new(-30, -20, 100, 90)]);
    assert_eq!(
        frame_damage_render_area(&negative, (1920, 1080)),
        Some(flux::CanvasRenderArea {
            x: 0,
            y: 0,
            width: 70,
            height: 70,
        })
    );
    // Damage wholly off the framebuffer has no usable area: the caller must
    // fall back to a full-destination pass, never to a rejected rect.
    let offscreen = FrameDamage::Area(vec![tessera_primitives::Rect::new(5000, 5000, 100, 100)]);
    assert_eq!(frame_damage_render_area(&offscreen, (1920, 1080)), None);
    let negative_far = FrameDamage::Area(vec![tessera_primitives::Rect::new(-500, 0, 100, 100)]);
    assert_eq!(frame_damage_render_area(&negative_far, (1920, 1080)), None);
    // A one-pixel-wide sliver is still a legal area, not a degenerate one.
    let sliver = FrameDamage::Area(vec![tessera_primitives::Rect::new(10, 10, 1, 40)]);
    assert_eq!(
        frame_damage_render_area(&sliver, (1920, 1080)),
        Some(flux::CanvasRenderArea {
            x: 10,
            y: 10,
            width: 1,
            height: 40,
        })
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
