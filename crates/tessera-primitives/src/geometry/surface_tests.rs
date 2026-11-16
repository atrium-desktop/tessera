use super::*;
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn transform_strategy() -> impl Strategy<Value = Transform> {
        prop_oneof![
            Just(Transform::Normal),
            Just(Transform::Rotate90),
            Just(Transform::Rotate180),
            Just(Transform::Rotate270),
            Just(Transform::FlipHorizontal),
            Just(Transform::FlipRotate90),
            Just(Transform::FlipRotate180),
            Just(Transform::FlipRotate270),
        ]
    }

    fn rect_within(width: i32, height: i32) -> impl Strategy<Value = Rect> {
        let width = width.max(1);
        let height = height.max(1);
        (0..width, 0..height).prop_flat_map(move |(x, y)| {
            (Just(x), Just(y), 1..=(width - x), 1..=(height - y))
                .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
        })
    }

    fn geometry_case() -> impl Strategy<Value = (SurfaceGeometry, i32, i32)> {
        (1i32..=128, 1i32..=128, transform_strategy(), 1i32..=4).prop_flat_map(
            |(local_width, local_height, transform, buffer_scale)| {
                // Valid Wayland state: the raw buffer is exactly the
                // surface-local extent times buffer_scale, with the
                // transform applied to the axes.
                let (buffer_width, buffer_height) = if transform.swap_axes() {
                    (local_height * buffer_scale, local_width * buffer_scale)
                } else {
                    (local_width * buffer_scale, local_height * buffer_scale)
                };
                let viewport_src = prop::option::of(rect_within(local_width, local_height));
                let viewport_dst =
                    prop::option::of((1i32..=1024, 1i32..=1024).prop_map(|(w, h)| Size { w, h }));
                (
                    Just(buffer_width),
                    Just(buffer_height),
                    Just(transform),
                    Just(buffer_scale),
                    viewport_src,
                    viewport_dst,
                )
                    .prop_map(
                        |(
                            buffer_width,
                            buffer_height,
                            transform,
                            buffer_scale,
                            viewport_src,
                            viewport_dst,
                        )| {
                            (
                                SurfaceGeometry {
                                    transform,
                                    buffer_scale,
                                    viewport_src,
                                    viewport_dst,
                                    ..Default::default()
                                },
                                buffer_width,
                                buffer_height,
                            )
                        },
                    )
            },
        )
    }

    fn geometry_with_surface_rect() -> impl Strategy<Value = (SurfaceGeometry, i32, i32, Rect)> {
        geometry_case().prop_flat_map(|(geometry, buffer_width, buffer_height)| {
            let (width, height) = geometry.surface_extent(buffer_width, buffer_height);
            rect_within(width, height)
                .prop_map(move |rect| (geometry, buffer_width, buffer_height, rect))
        })
    }

    fn geometry_with_source_rect() -> impl Strategy<Value = (SurfaceGeometry, i32, i32, Rect)> {
        geometry_case().prop_flat_map(|(geometry, buffer_width, buffer_height)| {
            let (width, height) = geometry.surface_extent(buffer_width, buffer_height);
            let visible = geometry
                .surface_rect_to_buffer(Rect::new(0, 0, width, height), buffer_width, buffer_height)
                .expect("the visible surface must map to a buffer rect");
            rect_within(visible.size.w, visible.size.h).prop_map(move |rect| {
                (
                    geometry,
                    buffer_width,
                    buffer_height,
                    Rect::new(
                        visible.origin.x + rect.origin.x,
                        visible.origin.y + rect.origin.y,
                        rect.size.w,
                        rect.size.h,
                    ),
                )
            })
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        #[test]
        fn every_surface_damage_rect_round_trips_through_the_buffer(
            (geometry, buffer_width, buffer_height, rect) in geometry_with_surface_rect()
        ) {
            let raw = geometry
                .surface_rect_to_buffer(rect, buffer_width, buffer_height)
                .expect("visible surface damage must map into the buffer");
            let back = geometry.buffer_rect_to_logical(raw, buffer_width, buffer_height);
            prop_assert!(
                rect.fully_covered_by(&[back]),
                "surface {:?} -> raw {:?} -> surface {:?} did not cover the input (geometry {:?}, buffer {}x{})",
                rect, raw, back, geometry, buffer_width, buffer_height
            );
        }

        #[test]
        fn every_visible_buffer_damage_rect_round_trips_through_the_surface(
            (geometry, buffer_width, buffer_height, rect) in geometry_with_source_rect()
        ) {
            let surface = geometry.buffer_rect_to_logical(rect, buffer_width, buffer_height);
            prop_assert!(
                !surface.is_empty(),
                "visible buffer damage {:?} mapped to an empty surface rect (geometry {:?}, buffer {}x{})",
                rect, geometry, buffer_width, buffer_height
            );
            let back = geometry
                .surface_rect_to_buffer(surface, buffer_width, buffer_height)
                .expect("surface damage from a visible buffer rect must map back");
            prop_assert!(
                rect.fully_covered_by(&[back]),
                "buffer {:?} -> surface {:?} -> buffer {:?} did not cover the input (geometry {:?}, buffer {}x{})",
                rect, surface, back, geometry, buffer_width, buffer_height
            );
        }
    }

    #[test]
    fn buffer_rect_to_logical_maps_fractional_scale_viewport() {
        // Chrome on a 2× HiDPI output: buffer 2400×1600 behind a
        // wp_viewport destination of 1200×800 logical px, buffer_scale 1.
        // A 16×16 cursor-adjacent damage rect in buffer px must land in
        // logical px (outward-rounded), not force full damage.
        let geometry = SurfaceGeometry {
            viewport_dst: Some(Size { w: 1200, h: 800 }),
            ..Default::default()
        };
        let mapped = geometry.buffer_rect_to_logical(Rect::new(100, 60, 16, 16), 2400, 1600);
        // 100/2400*1200 = 50 exactly; 60/1600*800 = 30 exactly; the 16px
        // span rounds outward to 8+1 logical px per axis.
        assert_eq!(mapped.origin.x, 50);
        assert_eq!(mapped.origin.y, 30);
        assert!(mapped.size.w >= 8 && mapped.size.w <= 9);
        assert!(mapped.size.h >= 8 && mapped.size.h <= 9);
    }

    #[test]
    fn buffer_rect_to_logical_viewport_crop_offsets() {
        // viewport src crops the buffer: damage inside the crop maps with
        // the crop offset removed and stretched to the destination.
        let geometry = SurfaceGeometry {
            viewport_src: Some(Rect::new(100, 50, 400, 400)),
            viewport_dst: Some(Size { w: 200, h: 200 }),
            ..Default::default()
        };
        let mapped = geometry.buffer_rect_to_logical(Rect::new(100, 50, 400, 400), 800, 600);
        assert_eq!(mapped.origin.x, 0);
        assert_eq!(mapped.origin.y, 0);
        assert_eq!(mapped.size.w, 200);
        assert_eq!(mapped.size.h, 200);
    }

    #[test]
    fn buffer_rect_to_logical_round_trips_buffer_scale_with_outward_rounding() {
        // No viewport: the mapping degenerates to the buffer-scale division
        // with outward rounding.
        let geometry = SurfaceGeometry {
            buffer_scale: 2,
            ..Default::default()
        };
        let mapped = geometry.buffer_rect_to_logical(Rect::new(3, 5, 8, 10), 200, 200);
        // x: floor(3/2)=1..ceil(11/2)=6 → w=5;
        // y: floor(5/2)=2..ceil(15/2)=8 → h=6.
        assert_eq!(mapped, Rect::new(1, 2, 5, 6));
    }

    #[test]
    fn surface_rect_to_buffer_maps_fractional_scale_viewport() {
        // Chrome on a 2x output: surface damage is in the 1200x800
        // destination space; the raw buffer is 2400x1600.
        let geometry = SurfaceGeometry {
            viewport_dst: Some(Size { w: 1200, h: 800 }),
            ..Default::default()
        };
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(50, 30, 8, 8), 2400, 1600),
            Some(Rect::new(100, 60, 16, 16))
        );
    }

    #[test]
    fn surface_rect_to_buffer_honours_viewport_source_origin() {
        // A crop at (100, 50) draws the source origin at surface (0, 0).
        // Surface damage must therefore be offset back into the buffer.
        let geometry = SurfaceGeometry {
            viewport_src: Some(Rect::new(100, 50, 400, 400)),
            viewport_dst: Some(Size { w: 200, h: 200 }),
            ..Default::default()
        };
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(0, 0, 200, 200), 800, 600),
            Some(Rect::new(100, 50, 400, 400))
        );
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(0, 0, 100, 100), 800, 600),
            Some(Rect::new(100, 50, 200, 200))
        );
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(200, 0, 10, 10), 800, 600),
            None,
            "damage outside the destination must not invent buffer pixels"
        );
    }

    #[test]
    fn surface_rect_to_buffer_crops_without_a_destination() {
        let geometry = SurfaceGeometry {
            viewport_src: Some(Rect::new(100, 50, 400, 300)),
            ..Default::default()
        };
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(0, 0, 400, 300), 800, 600),
            Some(Rect::new(100, 50, 400, 300))
        );
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(25, 30, 50, 40), 800, 600),
            Some(Rect::new(125, 80, 50, 40))
        );
    }

    #[test]
    fn surface_rect_to_buffer_composes_buffer_scale_and_crop() {
        // buffer_scale 2 turns the 800x600 raw buffer into a 400x300
        // local space; the viewport crop then starts at local (50, 50).
        let geometry = SurfaceGeometry {
            buffer_scale: 2,
            viewport_src: Some(Rect::new(50, 50, 200, 150)),
            viewport_dst: Some(Size { w: 100, h: 75 }),
            ..Default::default()
        };
        assert_eq!(
            geometry.surface_rect_to_buffer(Rect::new(0, 0, 100, 75), 800, 600),
            Some(Rect::new(100, 100, 400, 300))
        );
    }

    #[test]
    fn surface_rect_to_buffer_inverts_the_wayland_transforms() {
        let dims = (100, 60);
        let raw = Rect::new(10, 15, 20, 10);
        for transform in [
            Transform::Normal,
            Transform::Rotate90,
            Transform::Rotate180,
            Transform::Rotate270,
            Transform::FlipHorizontal,
            Transform::FlipRotate90,
            Transform::FlipRotate180,
            Transform::FlipRotate270,
        ] {
            let geometry = SurfaceGeometry {
                transform,
                ..Default::default()
            };
            let surface = transform.map_buffer_rect_to_surface(raw, dims);
            assert_eq!(
                geometry.surface_rect_to_buffer(surface, dims.0, dims.1),
                Some(raw),
                "{transform:?}"
            );
        }
    }

    #[test]
    fn buffer_and_surface_damage_conversions_round_trip_under_viewport() {
        let dims = (800, 600);
        let geometry = SurfaceGeometry {
            buffer_scale: 2,
            viewport_src: Some(Rect::new(50, 50, 200, 150)),
            viewport_dst: Some(Size { w: 100, h: 75 }),
            ..Default::default()
        };
        let raw = Rect::new(100, 100, 400, 300);
        // raw -> surface clips to the destination and removes the crop
        // offset; the inverse then puts it back into raw buffer space.
        let surface = geometry.buffer_rect_to_logical(raw, dims.0, dims.1);
        assert_eq!(surface, Rect::new(0, 0, 100, 75));
        assert_eq!(
            geometry.surface_rect_to_buffer(surface, dims.0, dims.1),
            Some(raw)
        );
    }

    #[test]
    fn buffer_rect_to_logical_outward_rounds_partial_spans() {
        let geometry = SurfaceGeometry {
            viewport_dst: Some(Size { w: 1199, h: 799 }),
            ..Default::default()
        };
        // 1 buffer px at 2400→1199 does not land on an integer logical px;
        // outward rounding must cover the partial span on both axes.
        let mapped = geometry.buffer_rect_to_logical(Rect::new(0, 0, 1, 1), 2400, 1600);
        assert_eq!(mapped.origin, Point { x: 0, y: 0 });
        assert!(mapped.size.w >= 1 && mapped.size.h >= 1);
    }

    #[test]
    fn rect_contains_inclusive_origin_exclusive_far_corner() {
        let r = Rect::new(10, 20, 100, 50);
        assert!(r.contains(Point { x: 10, y: 20 })); // top-left inclusive
        assert!(r.contains(Point { x: 109, y: 69 })); // bottom-right - 1
        assert!(!r.contains(Point { x: 110, y: 20 })); // right edge exclusive
        assert!(!r.contains(Point { x: 10, y: 70 })); // bottom edge exclusive
        assert!(!r.contains(Point { x: 9, y: 20 })); // left of origin
        assert!(!r.contains(Point { x: 10, y: 19 })); // above origin
    }

    #[test]
    fn transform_swap_axes_matches_90_and_270_only() {
        let should_swap = [
            Transform::Rotate90,
            Transform::Rotate270,
            Transform::FlipRotate90,
            Transform::FlipRotate270,
        ];
        let should_not_swap = [
            Transform::Normal,
            Transform::Rotate180,
            Transform::FlipHorizontal,
            Transform::FlipRotate180,
        ];
        for t in should_swap {
            assert!(t.swap_axes(), "{t:?} should swap axes");
        }
        for t in should_not_swap {
            assert!(!t.swap_axes(), "{t:?} should not swap axes");
        }
    }

    #[test]
    fn transform_maps_buffer_damage_to_the_renderer_surface_orientation() {
        let rect = Rect::new(10, 15, 20, 10);
        let dims = (100, 60);
        let expected = [
            (Transform::Normal, Rect::new(10, 15, 20, 10)),
            (Transform::Rotate90, Rect::new(35, 10, 10, 20)),
            (Transform::Rotate180, Rect::new(70, 35, 20, 10)),
            (Transform::Rotate270, Rect::new(15, 70, 10, 20)),
            (Transform::FlipHorizontal, Rect::new(70, 15, 20, 10)),
            (Transform::FlipRotate90, Rect::new(15, 10, 10, 20)),
            (Transform::FlipRotate180, Rect::new(10, 35, 20, 10)),
            (Transform::FlipRotate270, Rect::new(35, 70, 10, 20)),
        ];

        for (transform, mapped) in expected {
            assert_eq!(
                transform.map_buffer_rect_to_surface(rect, dims),
                mapped,
                "{transform:?}"
            );
        }
    }

    #[test]
    fn transform_maps_a_full_buffer_to_the_full_transformed_extent() {
        let full = Rect::new(0, 0, 100, 60);
        for transform in [
            Transform::Normal,
            Transform::Rotate180,
            Transform::FlipHorizontal,
            Transform::FlipRotate180,
        ] {
            assert_eq!(
                transform.map_buffer_rect_to_surface(full, (100, 60)),
                Rect::new(0, 0, 100, 60),
                "{transform:?}"
            );
        }
        for transform in [
            Transform::Rotate90,
            Transform::Rotate270,
            Transform::FlipRotate90,
            Transform::FlipRotate270,
        ] {
            assert_eq!(
                transform.map_buffer_rect_to_surface(full, (100, 60)),
                Rect::new(0, 0, 60, 100),
                "{transform:?}"
            );
        }
    }

    #[test]
    fn transform_name_round_trips_through_from_name() {
        let all = [
            Transform::Normal,
            Transform::Rotate90,
            Transform::Rotate180,
            Transform::Rotate270,
            Transform::FlipHorizontal,
            Transform::FlipRotate90,
            Transform::FlipRotate180,
            Transform::FlipRotate270,
        ];
        for t in all {
            assert_eq!(Transform::from_name(t.name()), Some(t), "{t:?}");
        }
    }

    #[test]
    fn transform_from_name_accepts_underscore_aliases_only() {
        assert_eq!(
            Transform::from_name("flipped_90"),
            Some(Transform::FlipRotate90)
        );
        assert_eq!(
            Transform::from_name("flipped_270"),
            Some(Transform::FlipRotate270)
        );
        // Exact lowercase matching: no case folding, no invented synonyms.
        assert_eq!(Transform::from_name("Normal"), None);
        assert_eq!(Transform::from_name("rotate90"), None);
        assert_eq!(Transform::from_name("upside-down"), None);
        assert_eq!(Transform::from_name(""), None);
    }
}
