use super::*;

fn region(x: f32, y: f32, w: f32, h: f32) -> tessera_chrome::BackdropRegion {
    tessera_chrome::BackdropRegion {
        x,
        y,
        w,
        h,
        wash: None,
        opacity: 1.0,
    }
}

#[test]
fn backdrop_cache_key_tracks_geometry_and_material_exactly() {
    let capture = [BackdropCaptureRegion {
        origin: (0, 1000),
        extent: (1920, 80),
    }];
    let frost = [region(0.0, 1040.0, 1920.0, 40.0)];
    let base = BackdropCacheKey::new(
        (0, 1000),
        (1920, 80),
        (1920, 1080),
        12.0,
        1.0,
        false,
        &capture,
        &[],
        &[],
        None,
    );
    let sigma_changed = BackdropCacheKey::new(
        (0, 1000),
        (1920, 80),
        (1920, 1080),
        12.5,
        1.0,
        false,
        &capture,
        &[],
        &[],
        None,
    );
    assert_ne!(base, sigma_changed);
    assert_eq!(base, base.clone());

    let glass = tessera_chrome::LiquidGlassRegion {
        bounds: region(400.0, 1000.0, 320.0, 64.0),
        focus: Some(tessera_chrome::LiquidGlassFocus {
            bounds: region(420.0, 1008.0, 96.0, 48.0),
            corner_radius: 12.0,
            strength: 1.0,
        }),
        ..Default::default()
    };
    // Glass parameters live in the material key: a focus-geometry change
    // re-runs the effect composite but leaves the scene capture untouched.
    let with_focus = BackdropMaterialKey::new(&frost, &[glass], [255, 255, 255]);
    let moved_focus = BackdropMaterialKey::new(
        &frost,
        &[tessera_chrome::LiquidGlassRegion {
            focus: glass.focus.map(|focus| tessera_chrome::LiquidGlassFocus {
                bounds: region(
                    focus.bounds.x + 1.0,
                    focus.bounds.y,
                    focus.bounds.w,
                    focus.bounds.h,
                ),
                ..focus
            }),
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, moved_focus);
    // Every material leg participates: material strengths, polarity, the
    // adaptation writeback, and the scheme tint.
    let strengthened = BackdropMaterialKey::new(
        &frost,
        &[tessera_chrome::LiquidGlassRegion {
            frost_strength: 5.0,
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, strengthened);
    let polarized = BackdropMaterialKey::new(
        &frost,
        &[tessera_chrome::LiquidGlassRegion {
            plate_polarity: 0.0,
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, polarized);
    let adapted = BackdropMaterialKey::new(
        &frost,
        &[tessera_chrome::LiquidGlassRegion {
            adaptation: Some(tessera_chrome::LiquidGlassAdaptation {
                plate_luminance: 0.5,
                backdrop_energy: 0.25,
            }),
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, adapted);
    let tinted = BackdropMaterialKey::new(&frost, &[glass], [243, 245, 249]);
    assert_ne!(with_focus, tinted);
    // …while the capture key no longer sees any of it.
    let capture_with_glass = BackdropCacheKey::new(
        (0, 1000),
        (1920, 80),
        (1920, 1080),
        12.0,
        1.0,
        false,
        &capture,
        &[],
        &[],
        None,
    );
    assert_eq!(base, capture_with_glass);
}

#[test]
fn focused_preview_content_keeps_one_full_brightness_target() {
    let focused = tessera_model::window::WindowId(7);
    let sibling = tessera_model::window::WindowId(8);
    assert_eq!(
        tessera_chrome::preview::content_brightness(Some(focused), focused, 0.74),
        1.0
    );
    assert_eq!(
        tessera_chrome::preview::content_brightness(Some(focused), sibling, 0.74),
        0.74
    );
    assert_eq!(
        tessera_chrome::preview::content_brightness(None, sibling, 0.74),
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
fn capture_bounds_cover_top_bar_with_blur_margin() {
    // 32px status bar at the top of a 1920x1080 output, sigma 12: the
    // capture spans the full width but only the bar plus the 3σ margin.
    let (origin, size) = blur_capture_bounds(
        &[region(0.0, 0.0, 1920.0, 32.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 68));
}

#[test]
fn capture_bounds_union_disjoint_regions() {
    // Top bar + bottom dock: the union covers both, including margins.
    let (origin, size) = blur_capture_bounds(
        &[
            region(0.0, 0.0, 1920.0, 32.0),
            region(400.0, 1040.0, 1120.0, 40.0),
        ],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 1080));
}

#[test]
fn capture_regions_keep_top_bar_and_bottom_dock_disjoint() {
    let regions = blur_capture_regions(
        &[
            region(0.0, 0.0, 1920.0, 32.0),
            region(400.0, 1040.0, 1120.0, 40.0),
        ],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(
        regions,
        vec![
            BackdropCaptureRegion {
                origin: (0, 0),
                extent: (1920, 68),
            },
            BackdropCaptureRegion {
                origin: (364, 1004),
                extent: (1192, 76),
            },
        ]
    );
}

#[test]
fn capture_regions_merge_overlapping_blur_footprints_transitively() {
    let regions = blur_capture_regions(
        &[
            region(100.0, 100.0, 40.0, 40.0),
            region(170.0, 100.0, 40.0, 40.0),
            region(240.0, 100.0, 40.0, 40.0),
        ],
        (400, 300),
        (400, 300),
        1.0,
        12.0,
    );
    assert_eq!(
        regions,
        vec![BackdropCaptureRegion {
            origin: (64, 64),
            extent: (252, 112),
        }]
    );
}

#[test]
fn backdrop_graph_accumulates_sampling_radius_across_three_layers() {
    use tessera_chrome::{BackdropLayer, BackdropLayerId, BackdropLayerSource};

    let cover = BackdropLayer::new(BackdropLayerId(1), BackdropLayerSource::Scene, 4.0)
        .with_frost(vec![region(50.0, 50.0, 20.0, 20.0)]);
    let glass = BackdropLayer::new(
        BackdropLayerId(2),
        BackdropLayerSource::Layer(BackdropLayerId(1)),
        3.0,
    )
    .with_frost(vec![region(50.0, 50.0, 20.0, 20.0)]);
    let glass_again = BackdropLayer::new(
        BackdropLayerId(3),
        BackdropLayerSource::Layer(BackdropLayerId(2)),
        2.0,
    )
    .with_frost(vec![region(50.0, 50.0, 20.0, 20.0)]);

    let plan =
        plan_backdrop_graph(&[glass_again, cover, glass], (200, 200), (200, 200), 1.0).unwrap();
    assert_eq!(plan.order, vec![1, 2, 0]);
    assert_eq!(plan.sources, vec![None, Some(0), Some(1)]);
    assert_eq!(
        plan.layer_regions,
        vec![
            vec![BackdropCaptureRegion {
                origin: (38, 38),
                extent: (44, 44),
            }],
            vec![BackdropCaptureRegion {
                origin: (41, 41),
                extent: (38, 38),
            }],
            vec![BackdropCaptureRegion {
                origin: (44, 44),
                extent: (32, 32),
            }],
        ]
    );
    assert_eq!(
        plan.resolve_regions,
        vec![
            vec![BackdropCaptureRegion {
                origin: (35, 35),
                extent: (50, 50),
            }],
            vec![BackdropCaptureRegion {
                origin: (44, 44),
                extent: (32, 32),
            }],
            vec![BackdropCaptureRegion {
                origin: (50, 50),
                extent: (20, 20),
            }],
        ]
    );
    assert_eq!(
        plan.capture_regions,
        vec![BackdropCaptureRegion {
            // 3σ × (4 + 3 + 2) accumulated along the dependency path.
            origin: (23, 23),
            extent: (74, 74),
        }]
    );
}

#[test]
fn backdrop_graph_rejects_missing_sources_and_cycles() {
    use tessera_chrome::{BackdropLayer, BackdropLayerId, BackdropLayerSource};

    let missing = BackdropLayer::new(
        BackdropLayerId(1),
        BackdropLayerSource::Layer(BackdropLayerId(9)),
        4.0,
    )
    .with_frost(vec![region(10.0, 10.0, 20.0, 20.0)]);
    assert!(plan_backdrop_graph(&[missing], (100, 100), (100, 100), 1.0).is_err());

    let a = BackdropLayer::new(
        BackdropLayerId(1),
        BackdropLayerSource::Layer(BackdropLayerId(2)),
        4.0,
    )
    .with_frost(vec![region(10.0, 10.0, 20.0, 20.0)]);
    let b = BackdropLayer::new(
        BackdropLayerId(2),
        BackdropLayerSource::Layer(BackdropLayerId(1)),
        4.0,
    )
    .with_frost(vec![region(10.0, 10.0, 20.0, 20.0)]);
    assert!(plan_backdrop_graph(&[a, b], (100, 100), (100, 100), 1.0).is_err());
}

#[test]
#[allow(clippy::modulo_one)]
fn capture_bounds_align_to_downsample() {
    // A floating region: origin/size land on BACKDROP_DOWNSAMPLE
    // multiples so the capture grid stays exact. With a full-resolution
    // capture the bounds are the exact padded region union.
    let (origin, size) = blur_capture_bounds(
        &[region(100.0, 100.0, 200.0, 50.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (64, 64));
    assert_eq!(size, (272, 122));
    assert_eq!(origin.0 % BACKDROP_DOWNSAMPLE, 0);
    assert_eq!(origin.1 % BACKDROP_DOWNSAMPLE, 0);
    assert_eq!(size.0 % BACKDROP_DOWNSAMPLE, 0);
    assert_eq!(size.1 % BACKDROP_DOWNSAMPLE, 0);
}

#[test]
fn capture_bounds_respect_output_scale() {
    // scale=2: regions are logical, bounds physical (16 logical px bar
    // = 32 physical px; margin is 3σ in physical pixels).
    let (origin, size) = blur_capture_bounds(
        &[region(0.0, 0.0, 960.0, 16.0)],
        (960, 540),
        (1920, 1080),
        2.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 104));
}

#[test]
fn liquid_glass_geometry_maps_to_the_downsampled_capture_once() {
    let source = tessera_chrome::LiquidGlassRegion {
        bounds: region(400.0, 100.0, 320.0, 74.0),
        corner_radius: 18.0,
        opacity: 0.4,
        focus: Some(tessera_chrome::LiquidGlassFocus {
            bounds: region(480.0, 112.0, 120.0, 50.0),
            corner_radius: 12.0,
            strength: 0.8,
        }),
        ..Default::default()
    };
    // Output scale 2, capture downsample 1/2, physical capture origin
    // (600, 120): capture coordinates are logical*1 - origin*0.5.
    let groups = liquid_glass_groups(
        &[source],
        (600, 120),
        2.0,
        0.5,
        tessera_model::settings::ColorScheme::Dark,
    );
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].tint_color, [255, 255, 255]);
    assert_eq!(groups[0].primary.x, 100.0);
    assert_eq!(groups[0].primary.y, 40.0);
    assert_eq!(groups[0].primary.width, 320.0);
    assert_eq!(groups[0].primary.height, 74.0);
    assert_eq!(groups[0].primary.corner_radius, 18.0);
    assert_eq!(groups[0].opacity, 0.4);
    assert!(groups[0].merged.is_none());
    let focus = groups[0]
        .focus
        .expect("focus should share the capture mapping");
    assert_eq!(focus.shape.x, 180.0);
    assert_eq!(focus.shape.y, 52.0);
    assert_eq!(focus.shape.width, 120.0);
    assert_eq!(focus.shape.height, 50.0);
    assert_eq!(focus.shape.corner_radius, 12.0);
    assert_eq!(focus.strength, 0.8);
}

#[test]
fn capture_bounds_clamp_to_physical_extent() {
    // Bottom-edge dock: the margin past the screen edge is clamped away.
    let (origin, size) = blur_capture_bounds(
        &[region(400.0, 1048.0, 1120.0, 32.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (364, 1012));
    assert_eq!(size, (1192, 68));
}

#[test]
fn capture_bounds_fall_back_to_full_frame_without_regions() {
    let (origin, size) = blur_capture_bounds(
        &[region(10.0, 10.0, 0.0, 0.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 1080));
}

#[test]
fn opaque_frame_fill_replaces_undefined_and_previous_contents() {
    let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
        return;
    };
    let size = (32, 24);
    let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
    let canvas = flux::Canvas::new(&surface).unwrap();

    for expected in [[13, 77, 191, 255], [211, 43, 29, 255]] {
        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame(
            &canvas,
            &frame,
            flux::rgba(expected[0], expected[1], expected[2], expected[3]),
        )
        .unwrap();
        canvas.end_frame_checked().unwrap();
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
    let canvas = flux::Canvas::new(&surface).unwrap();

    let frame = surface.begin_frame().unwrap();
    begin_opaque_frame(&canvas, &frame, flux::rgba(200, 30, 20, 255)).unwrap();
    canvas.end_frame_checked().unwrap();
    frame.submit().unwrap().present().unwrap();

    let frame = surface.begin_frame().unwrap();
    let repaint = FrameDamage::Area(vec![tessera_model::Rect::new(8, 6, 10, 9)]);
    begin_opaque_frame_repaint(
        &canvas,
        &frame,
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
    canvas.end_frame_checked().unwrap();
    begin_stencil_frame_overlay(&canvas, &frame, frame_damage_render_area(&repaint)).unwrap();
    let arena = flux::Arena::with_capacity(4096).unwrap();
    let path = flux::Path::new(&arena).unwrap();
    path.move_to(11.0, 8.0)
        .line_to(15.0, 12.0)
        .line_to(11.0, 12.0)
        .line_to(15.0, 8.0)
        .close();
    canvas.fill_path(&path, &flux::Paint::solid(flux::rgba(20, 220, 70, 255)));
    canvas.end_frame_checked().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
    surface.read_pixels(&mut pixels).unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * size.0 as usize + x) * 4..][..4];
    assert_eq!(pixel(0, 0), [200, 30, 20, 255]);
    assert_eq!(pixel(9, 7), [10, 80, 220, 255]);
    assert_eq!(pixel(17, 14), [10, 80, 220, 255]);
    assert_eq!(pixel(18, 15), [200, 30, 20, 255]);
}

#[test]
fn backdrop_frost_mapping_carries_opacity_and_wash_into_capture_pixels() {
    // Logical 2x-scale output, capture downsample 1/2, physical capture
    // origin (100, 50): capture coords are logical*2 - origin, then halved.
    let washed = tessera_chrome::BackdropRegion {
        x: 10.0,
        y: 20.0,
        w: 40.0,
        h: 30.0,
        wash: Some(tessera_chrome::backdrop_wash(lens::Color::rgba(
            8, 10, 18, 126,
        ))),
        opacity: 0.25,
    };
    let plain = region(0.0, 0.0, 8.0, 8.0);
    let frost = backdrop_frost_in_capture(&[washed, plain], (100, 50), (200, 100), (100, 50), 2.0);
    assert_eq!(frost.len(), 2);
    // (10*2 - 100) / 2 = -40, (20*2 - 50) / 2 = -5: the rect starts off-capture
    // and is mapped verbatim (the dispatch clips to the region).
    assert_eq!((frost[0].x, frost[0].y), (-40.0, -5.0));
    assert_eq!((frost[0].width, frost[0].height), (40.0, 30.0));
    assert_eq!(frost[0].opacity, 0.25, "the fade channel reaches prism");
    assert_eq!(frost[0].tint_color, [8, 10, 18], "wash tint rides along");
    assert!((frost[0].tint_strength - 126.0 / 255.0).abs() < 0.01);
    assert_eq!(frost[1].opacity, 1.0, "an unfaded region stays opaque");
    assert_eq!(frost[1].tint_strength, 0.0, "no wash means plain frost");
}

/// The frost body's `opacity` is the channel chrome exit fades ride. This runs
/// the real layered-backdrop material (not just the descriptor mapping): at
/// `0.0` the cover must leave the sharp backdrop untouched, at `1.0` it must
/// show the blurred body, and mid-fade it must land between the two rather
/// than snapping to either end.
#[test]
fn frost_opacity_drains_the_cover_between_sharp_and_blurred() {
    let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
        return;
    };
    const W: u32 = 64;
    const H: u32 = 64;
    let format = flux::Format::Rgba8Unorm;

    // A hard checkerboard: blur is unmistakable, and the sharp input is
    // exactly reproducible for comparison.
    let mut sharp = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            let value = if on { 255 } else { 0 };
            let index = ((y * W + x) * 4) as usize;
            sharp[index..index + 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
    let input = flux::Image::from_bytes(&device, W, H, format, &sharp).unwrap();
    let surface = flux::Surface::offscreen(&device, W, H).unwrap();
    let canvas = flux::Canvas::new(&surface).unwrap();
    let mut blur = flux::BlurFilter::new(&device).unwrap();
    let mut filter = prism::BackdropLayerFilter::new(&device).unwrap();
    let regions = [flux::BlurRegion {
        x: 0,
        y: 0,
        width: W,
        height: H,
    }];

    let mut render = |opacity: f32| -> Vec<u8> {
        let frame = surface.begin_frame().unwrap();
        let blurred = blur
            .apply_regions(&frame, &input, 6.0, &regions)
            .expect("blur applies");
        let frost = [prism::BackdropFrost {
            x: 0.0,
            y: 0.0,
            width: W as f32,
            height: H as f32,
            corner_radius: 0.0,
            opacity,
            tint_color: [255, 255, 255],
            tint_strength: 0.0,
        }];
        let material = filter
            .apply(
                &frame,
                &input,
                &blurred,
                &frost,
                &[],
                prism::LiquidGlassParams::default(),
            )
            .expect("layered backdrop applies");
        canvas
            .begin_frame(Some(&frame), Some(flux::rgba(0, 0, 0, 255)))
            .unwrap();
        material.draw(&canvas, 0.0, 0.0, W as f32, H as f32);
        canvas.end_frame_checked().unwrap();
        frame.submit().unwrap().present().unwrap();
        let mut pixels = vec![0; (W * H * 4) as usize];
        surface.read_pixels(&mut pixels).unwrap();
        pixels
    };

    let drained = render(0.0);
    let frosted = render(1.0);
    let mid = render(0.5);

    let channel =
        |pixels: &[u8], x: u32, y: u32, c: usize| pixels[((y * W + x) * 4) as usize + c] as i32;
    // Opacity 0 restores the sharp backdrop exactly (the output transform
    // dithers ±1 LSB, hence the tolerance).
    for y in 0..H {
        for x in 0..W {
            for c in 0..3 {
                assert!(
                    (channel(&drained, x, y, c) - channel(&sharp, x, y, c)).abs() <= 1,
                    "a fully drained cover must be the sharp backdrop at ({x},{y})"
                );
            }
        }
    }
    // Opacity 1 shows the blurred body: somewhere the frost differs strongly
    // from the sharp input.
    let strongest = (1..H - 1)
        .flat_map(|y| (1..W - 1).map(move |x| (x, y)))
        .max_by_key(|(x, y)| {
            (0..3)
                .map(|c| (channel(&frosted, *x, *y, c) - channel(&sharp, *x, *y, c)).abs())
                .max()
                .unwrap_or(0)
        })
        .expect("the image has interior pixels");
    let (sx, sy) = strongest;
    let full_delta = (channel(&frosted, sx, sy, 0) - channel(&sharp, sx, sy, 0)).abs();
    assert!(
        full_delta > 32,
        "an opaque frost must visibly blur: delta {full_delta}"
    );
    // Mid-fade lands between the two endpoints, near the arithmetic mean —
    // the cover drains with the animation instead of popping.
    let sharp_value = channel(&sharp, sx, sy, 0);
    let full_value = channel(&frosted, sx, sy, 0);
    let mid_value = channel(&mid, sx, sy, 0);
    let (low, high) = (sharp_value.min(full_value), sharp_value.max(full_value));
    assert!(
        mid_value > low + 1 && mid_value < high - 1,
        "a half-faded cover must sit between sharp ({sharp_value}) and frosted \
         ({full_value}), got {mid_value}"
    );
    assert!(
        (mid_value - (low + high) / 2).abs() <= 16,
        "a half-faded cover tracks the blend, got {mid_value} for [{low}, {high}]"
    );
}

#[test]
fn frame_damage_render_area_uses_the_exact_union() {
    let damage = FrameDamage::Area(vec![
        tessera_model::Rect::new(11, 7, 5, 9),
        tessera_model::Rect::new(29, 3, 4, 8),
    ]);
    assert_eq!(
        frame_damage_render_area(&damage),
        Some(flux::CanvasRenderArea {
            x: 11,
            y: 3,
            width: 22,
            height: 13,
        })
    );
    assert_eq!(frame_damage_render_area(&FrameDamage::Full), None);
    assert_eq!(frame_damage_render_area(&FrameDamage::None), None);
}
