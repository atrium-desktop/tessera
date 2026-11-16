use super::*;
use proptest::prelude::*;
use tessera_primitives::Transform;

fn placement_transform_strategy() -> impl Strategy<Value = Transform> {
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

fn placement_rect_within(width: i32, height: i32) -> impl Strategy<Value = Rect> {
    let width = width.max(1);
    let height = height.max(1);
    (0..width, 0..height).prop_flat_map(move |(x, y)| {
        (Just(x), Just(y), 1..=(width - x), 1..=(height - y))
            .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
    })
}

fn placement_geometry() -> impl Strategy<Value = (SurfaceGeometry, i32, i32)> {
    (
        1i32..=64,
        1i32..=64,
        placement_transform_strategy(),
        1i32..=4,
    )
        .prop_flat_map(|(local_width, local_height, transform, buffer_scale)| {
            let (buffer_width, buffer_height) = if transform.swap_axes() {
                (local_height * buffer_scale, local_width * buffer_scale)
            } else {
                (local_width * buffer_scale, local_height * buffer_scale)
            };
            let viewport_src = prop::option::of(placement_rect_within(local_width, local_height));
            let viewport_dst = prop::option::of(
                (1i32..=256, 1i32..=256).prop_map(|(w, h)| tessera_primitives::Size { w, h }),
            );
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
        })
}

fn placement_case() -> impl Strategy<Value = (SurfaceGeometry, i32, i32, Rect)> {
    placement_geometry().prop_flat_map(|(geometry, buffer_width, buffer_height)| {
        let (width, height) = geometry.surface_extent(buffer_width, buffer_height);
        placement_rect_within(width, height)
            .prop_map(move |rect| (geometry, buffer_width, buffer_height, rect))
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn placement_maps_full_surface_damage_to_exactly_the_drawn_rect(
        (geometry, buffer_width, buffer_height) in placement_geometry()
    ) {
        let placement = SurfacePlacement::resolve(&geometry, buffer_width, buffer_height);
        let (surface_width, surface_height) =
            geometry.surface_extent(buffer_width, buffer_height);
        prop_assert_eq!(placement.place_damage(&[], true), vec![placement.rect()]);
        prop_assert_eq!(
            placement.place(Rect::new(0, 0, surface_width, surface_height)),
            Some(placement.rect()),
            "geometry {:?}, buffer {}x{}",
            geometry, buffer_width, buffer_height
        );
    }

    #[test]
    fn placement_lands_every_surface_damage_inside_the_drawn_rect(
        (geometry, buffer_width, buffer_height, damage) in placement_case()
    ) {
        let placement = SurfacePlacement::resolve(&geometry, buffer_width, buffer_height);
        let landed = placement
            .place(damage)
            .expect("surface damage within the extent must land on the output");
        let drawn = placement.rect();
        prop_assert!(
            landed.fully_covered_by(&[drawn]),
            "damage {:?} landed at {:?}, outside the drawn rect {:?} (geometry {:?})",
            damage, landed, drawn, geometry
        );
    }
}

/// A placement for a surface whose buffer maps 1:1 onto logical pixels.
fn placement_at(origin: (i32, i32), width: i32, height: i32) -> SurfacePlacement {
    SurfacePlacement {
        origin: tessera_primitives::Point {
            x: origin.0,
            y: origin.1,
        },
        source: Rect::new(0, 0, width, height),
        surface_extent: (width, height),
        dest_extent: (width, height),
    }
}

#[test]
fn test_inv_arch_94_placement_maps_and_clamps_damage_to_the_surface() {
    let placement = placement_at((100, 50), 800, 600);
    // Tight damage translates by the compositor position.
    assert_eq!(
        placement.place_damage(&[Rect::new(10, 20, 30, 40)], false),
        vec![Rect::new(110, 70, 30, 40)]
    );
    // Out-of-bounds damage clamps to the surface extent.
    assert_eq!(
        placement.place_damage(&[Rect::new(-20, 590, 900, 50)], false),
        vec![Rect::new(100, 640, 800, 10)]
    );
    // Partial negative damage clips by intersection rather than moving the
    // origin to zero while preserving the original width.
    assert_eq!(
        placement_at((10, 20), 800, 600).place_damage(&[Rect::new(-20, 10, 30, 20)], false),
        vec![Rect::new(10, 30, 10, 20)]
    );
    // A report wholly outside the surface contributes no invented damage.
    assert!(
        placement
            .place_damage(&[Rect::new(-50, 10, 20, 20)], false)
            .is_empty()
    );
    // No damage information damages the whole surface.
    assert_eq!(
        placement.place_damage(&[], true),
        vec![Rect::new(100, 50, 800, 600)]
    );
    // Fully degenerate damage reports nothing.
    assert!(
        placement
            .place_damage(&[Rect::new(10, 10, 0, 0)], false)
            .is_empty()
    );
    // Disjoint damage rects are preserved individually, not unioned.
    assert_eq!(
        placement_at((0, 0), 800, 600)
            .place_damage(&[Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)], false),
        vec![Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)]
    );
}

#[test]
fn test_inv_arch_93_viewport_destination_is_the_damage_space_not_a_second_scale() {
    // A HiDPI client committing a 2048x1536 buffer at scale 2 with a
    // 512x384 wp_viewport destination: wl_surface.damage is already in the
    // 512x384 surface space. Mapping it through dest/source again would
    // repaint only a quarter of the changed pixels.
    let placement = SurfacePlacement::resolve(
        &SurfaceGeometry {
            position: tessera_primitives::Point { x: 100, y: 50 },
            buffer_scale: 2,
            viewport_dst: Some(tessera_primitives::Size { w: 512, h: 384 }),
            ..Default::default()
        },
        2048,
        1536,
    );
    assert_eq!(placement.source, Rect::new(0, 0, 1024, 768));
    assert_eq!(placement.surface_extent, (512, 384));
    assert_eq!(placement.dest_extent, (512, 384));
    assert_eq!(placement.rect(), Rect::new(100, 50, 512, 384));
    // A full-surface damage covers the whole destination.
    assert_eq!(
        placement.place_damage(&[], true),
        vec![Rect::new(100, 50, 512, 384)]
    );
    // Half the surface maps to half the destination, not a quarter.
    assert_eq!(
        placement.place_damage(&[Rect::new(0, 0, 256, 384)], false),
        vec![Rect::new(100, 50, 256, 384)]
    );
}

/// `wp_viewport.set_source` changes which buffer pixels are sampled, not
/// where the surface is drawn. Damage is in surface coordinates, so it maps
/// to output coordinates directly; the crop origin belongs to the
/// surface→buffer conversion, not to `place`.
#[test]
fn test_inv_arch_92_viewport_source_crop_does_not_shift_output_damage() {
    let placement = SurfacePlacement::resolve(
        &SurfaceGeometry {
            position: tessera_primitives::Point { x: 0, y: 0 },
            viewport_src: Some(Rect::new(100, 100, 400, 300)),
            ..Default::default()
        },
        800,
        600,
    );
    assert_eq!(placement.source, Rect::new(100, 100, 400, 300));
    assert_eq!(placement.surface_extent, (400, 300));
    assert_eq!(placement.dest_extent, (400, 300));
    assert_eq!(placement.rect(), Rect::new(0, 0, 400, 300));

    // Damage is already in the 400x300 surface space.
    assert_eq!(
        placement.place(Rect::new(0, 0, 200, 150)),
        Some(Rect::new(0, 0, 200, 150))
    );
    assert_eq!(
        placement.place(Rect::new(200, 150, 200, 150)),
        Some(Rect::new(200, 150, 200, 150))
    );
    // Damage outside the surface reaches no output pixel.
    assert_eq!(placement.place(Rect::new(400, 0, 10, 10)), None);
    assert_eq!(placement.place(Rect::new(-200, 0, 100, 100)), None);
    // A report straddling the boundary clips to the visible part.
    assert_eq!(
        placement.place(Rect::new(-50, 0, 100, 100)),
        Some(Rect::new(0, 0, 50, 100))
    );
    // The explicit full verdict covers the whole surface content.
    assert_eq!(
        placement.place_damage(&[], true),
        vec![Rect::new(0, 0, 400, 300)]
    );
}

/// A source crop combined with a destination that scales it: the surface is
/// already 200x150, so damage maps 1:1 into that drawn rect.
#[test]
fn test_inv_arch_92_viewport_source_and_destination_compose() {
    let placement = SurfacePlacement::resolve(
        &SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            viewport_src: Some(Rect::new(200, 100, 400, 300)),
            viewport_dst: Some(tessera_primitives::Size { w: 200, h: 150 }),
            ..Default::default()
        },
        800,
        600,
    );
    assert_eq!(placement.surface_extent, (200, 150));
    assert_eq!(
        placement.place(Rect::new(0, 0, 200, 150)),
        Some(Rect::new(10, 20, 200, 150))
    );
    assert_eq!(
        placement.place(Rect::new(0, 0, 100, 75)),
        Some(Rect::new(10, 20, 100, 75))
    );
}

#[test]
fn test_inv_arch_93_transition_extent_overrides_the_viewport_destination() {
    // ADR-0029: an in-flight transition draws the texture at the
    // interpolated size, so damage must follow that rect, not the
    // viewport destination the client declared.
    let placement = SurfacePlacement::resolve(
        &SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 10 },
            viewport_dst: Some(tessera_primitives::Size { w: 400, h: 300 }),
            transition_size: Some(tessera_primitives::Size { w: 200, h: 150 }),
            ..Default::default()
        },
        400,
        300,
    );
    assert_eq!(placement.dest_extent, (200, 150));
    assert_eq!(
        placement.place_damage(&[Rect::new(0, 0, 400, 300)], false),
        vec![Rect::new(10, 10, 200, 150)]
    );
}

#[test]
fn union_bbox_grows_to_cover() {
    let mut bbox = None;
    union_bbox(&mut bbox, Rect::new(10, 10, 20, 20));
    union_bbox(&mut bbox, Rect::new(0, 40, 15, 10));
    assert_eq!(bbox, Some(Rect::new(0, 10, 30, 40)));
}

#[test]
fn frame_damage_union_preserves_full_and_joins_areas() {
    let a = FrameDamage::Area(vec![Rect::new(10, 20, 30, 40)]);
    let b = FrameDamage::Area(vec![Rect::new(0, 50, 20, 20)]);
    // Lists concatenate; the union is recoverable via area_union.
    let joined = union_frame_damage(a, b);
    assert_eq!(joined.area_union(), Some(Rect::new(0, 20, 40, 50)));
    assert_eq!(
        union_frame_damage(FrameDamage::None, joined.clone()),
        joined
    );
    assert_eq!(
        union_frame_damage(FrameDamage::Full, joined),
        FrameDamage::Full
    );
}

#[test]
fn damage_normalization_overflow_covers_every_input_rect() {
    // Force the disjoint decomposition past its work budget. The fallback
    // must be the bounding box of the whole input, not of the prefix that
    // happened to fit before the budget was exhausted.
    let rects: Vec<Rect> = (0..512)
        .map(|index| {
            let offset = index * 3;
            Rect::new(offset, offset / 2, 600, 400)
        })
        .collect();
    let expected = {
        let mut bbox = None;
        for rect in &rects {
            union_bbox(&mut bbox, *rect);
        }
        bbox.expect("non-empty input")
    };
    let normalized = FrameDamage::from_rects(rects);
    assert_eq!(normalized.area_union(), Some(expected));
    assert_eq!(
        normalized.area_rects().map(<[Rect]>::len),
        Some(1),
        "overflow must degrade to one full-coverage bounding box"
    );
}

#[test]
fn frame_damage_history_is_deduplicated_and_bounded() {
    let repeated = vec![Rect::new(1, 2, 3, 4); 100];
    assert_eq!(
        FrameDamage::from_rects(repeated),
        FrameDamage::Area(vec![Rect::new(1, 2, 3, 4)])
    );

    let disjoint: Vec<_> = (0..=MAX_FRAME_DAMAGE_RECTS)
        .map(|index| Rect::new(index as i32 * 2, 0, 1, 1))
        .collect();
    assert_eq!(
        FrameDamage::from_rects(disjoint),
        FrameDamage::Area(vec![Rect::new(
            0,
            0,
            (MAX_FRAME_DAMAGE_RECTS as i32) * 2 + 1,
            1,
        )])
    );
}

#[test]
fn logical_to_physical_rounds_outward_and_clamps() {
    // scale 2: outward rounding keeps every touched physical pixel.
    let mapped = logical_to_physical(Rect::new(5, 5, 11, 11), 2.0, (1920, 1080));
    assert_eq!(mapped, Some(Rect::new(10, 10, 22, 22)));
    // Extending past the framebuffer clamps to the physical extent.
    let clamped = logical_to_physical(Rect::new(900, 500, 100, 100), 2.0, (1920, 1080));
    assert_eq!(clamped, Some(Rect::new(1800, 1000, 120, 80)));
    // Fully off-screen and unusable scales report no mapping.
    assert_eq!(
        logical_to_physical(Rect::new(-500, 0, 100, 100), 1.0, (1920, 1080)),
        None
    );
    assert_eq!(
        logical_to_physical(Rect::new(0, 0, 10, 10), 0.0, (1920, 1080)),
        None
    );
}

#[test]
fn frame_damage_from_rects_decomposes_overlaps_into_pairwise_disjoint_rects() {
    let a = Rect::new(10, 10, 20, 20);
    let b = Rect::new(20, 20, 20, 20);
    let damage = FrameDamage::from_rects(vec![a, b]);
    let rects = damage.area_rects().expect("must remain Area");
    assert_eq!(rects.len(), 3);
    let mut total_area = 0;
    for (i, r1) in rects.iter().enumerate() {
        total_area += r1.size.w * r1.size.h;
        for r2 in &rects[i + 1..] {
            assert!(
                r1.intersect(*r2).is_none(),
                "overlapping rects in normalized damage: {r1:?} vs {r2:?}"
            );
        }
    }
    assert_eq!(total_area, 400 + 400 - 100);
}

#[test]
fn client_damage_maps_degrading_to_full() {
    // Disjoint logical rects stay disjoint through the mapping.
    let damage = ClientDamage::Area(vec![Rect::new(0, 0, 5, 5), Rect::new(10, 10, 5, 5)]);
    assert_eq!(
        damage.clone().into_frame_damage(1.0, (100, 100)),
        FrameDamage::Area(vec![Rect::new(0, 0, 5, 5), Rect::new(10, 10, 5, 5)])
    );
    // Offscreen rectangles contribute zero physical damage without degrading to Full (INV-ARCH-99).
    let with_offscreen =
        ClientDamage::Area(vec![Rect::new(0, 0, 5, 5), Rect::new(-50, -50, 20, 20)]);
    assert_eq!(
        with_offscreen.into_frame_damage(1.0, (100, 100)),
        FrameDamage::Area(vec![Rect::new(0, 0, 5, 5)])
    );
    let all_offscreen = ClientDamage::Area(vec![Rect::new(-50, -50, 20, 20)]);
    assert_eq!(
        all_offscreen.into_frame_damage(1.0, (100, 100)),
        FrameDamage::None
    );
    // An invalid scale degrades the whole frame to Full.
    assert_eq!(damage.into_frame_damage(0.0, (100, 100)), FrameDamage::Full);
    assert_eq!(
        ClientDamage::None.into_frame_damage(1.0, (100, 100)),
        FrameDamage::None
    );
}

#[test]
fn flux_damage_round_trip_preserves_the_set_and_rounds_outward() {
    let damage = FrameDamage::Area(vec![Rect::new(10, 20, 30, 40), Rect::new(100, 120, 5, 7)]);
    let round_trip = FrameDamage::from_flux_damage(damage.to_flux_damage());
    assert_eq!(round_trip, damage);
    assert_eq!(
        FrameDamage::from_flux_damage(flux::Damage::None),
        FrameDamage::None
    );
    assert_eq!(
        FrameDamage::from_flux_damage(flux::Damage::Area(Vec::new())),
        FrameDamage::None
    );
    assert_eq!(
        FrameDamage::from_flux_damage(flux::Damage::Area(vec![flux::CanvasRenderArea {
            x: 10.4,
            y: 20.6,
            width: 3.2,
            height: 4.1,
        }])),
        FrameDamage::Area(vec![Rect::new(10, 20, 4, 5)])
    );
}

#[test]
fn explicit_full_verdict_is_not_inferred_from_an_empty_list() {
    struct Sample {
        generation: u64,
        damage_full: bool,
        geometry: SurfaceGeometry,
    }
    impl SurfaceDamageFrame for Sample {
        fn id(&self) -> usize {
            11
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn damage(&self) -> &[Rect] {
            &[]
        }
        fn damage_full(&self) -> bool {
            self.damage_full
        }
        fn placement(&self) -> SurfacePlacement {
            SurfacePlacement::resolve(&self.geometry, 100, 80)
        }
    }
    let geometry = SurfaceGeometry {
        position: tessera_primitives::Point { x: 10, y: 20 },
        ..Default::default()
    };
    let mut tracker = ClientDamageTracker::default();
    // First sighting dirties the surface footprint.
    assert!(matches!(
        tracker.assess([&Sample {
            generation: 1,
            damage_full: false,
            geometry,
        } as &dyn SurfaceDamageFrame]),
        ClientDamage::Area(_)
    ));
    // A generation change with no damage and full=false is a visible no-op.
    assert_eq!(
        tracker.assess([&Sample {
            generation: 2,
            damage_full: false,
            geometry,
        } as &dyn SurfaceDamageFrame]),
        ClientDamage::None
    );
    // The explicit full verdict is the only thing that dirties the whole
    // surface when the rectangle list is empty.
    assert_eq!(
        tracker.assess([&Sample {
            generation: 3,
            damage_full: true,
            geometry,
        } as &dyn SurfaceDamageFrame]),
        ClientDamage::Area(vec![Rect::new(10, 20, 100, 80)])
    );
}

#[test]
fn tracker_maps_geometry_change_as_localized_area_damage() {
    struct Sample {
        generation: u64,
        damage: Vec<Rect>,
        geometry: SurfaceGeometry,
    }
    impl SurfaceDamageFrame for Sample {
        fn id(&self) -> usize {
            7
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn damage(&self) -> &[Rect] {
            &self.damage
        }
        fn damage_full(&self) -> bool {
            false
        }
        fn placement(&self) -> SurfacePlacement {
            SurfacePlacement::resolve(&self.geometry, 100, 80)
        }
    }
    let geometry = SurfaceGeometry {
        position: tessera_primitives::Point { x: 10, y: 20 },
        ..Default::default()
    };
    let moved = SurfaceGeometry {
        position: tessera_primitives::Point { x: 11, y: 20 },
        ..geometry
    };

    let mut tracker = ClientDamageTracker::default();
    let initial = tracker.assess([&Sample {
        generation: 3,
        damage: Vec::new(),
        geometry,
    } as &dyn SurfaceDamageFrame]);
    assert!(
        matches!(initial, ClientDamage::Area(_)),
        "first sighting emits surface area"
    );
    assert!(matches!(
        tracker.assess([&Sample {
            generation: 3,
            damage: Vec::new(),
            geometry,
        } as &dyn SurfaceDamageFrame]),
        ClientDamage::None
    ));
    let moved_damage = tracker.assess([&Sample {
        generation: 3,
        damage: Vec::new(),
        geometry: moved,
    } as &dyn SurfaceDamageFrame]);
    assert!(matches!(moved_damage, ClientDamage::Area(_)));
}

/// A placement change must report the *visual footprint* on both sides, not
/// the bare buffer rectangle. The compositor-drawn shadow reaches past the
/// window; using `placement.rect()` for the destination side clipped the
/// leading shadow edge of a moving floating window.
#[test]
fn a_moved_surface_repaints_the_shadow_skirt_on_both_sides() {
    struct Sample {
        geometry: SurfaceGeometry,
    }
    impl SurfaceDamageFrame for Sample {
        fn id(&self) -> usize {
            7
        }
        fn generation(&self) -> u64 {
            1
        }
        fn damage(&self) -> &[Rect] {
            &[]
        }
        fn damage_full(&self) -> bool {
            false
        }
        fn placement(&self) -> SurfacePlacement {
            SurfacePlacement::resolve(&self.geometry, 100, 80)
        }
    }

    let before = SurfaceGeometry {
        position: tessera_primitives::Point { x: 10, y: 20 },
        ..Default::default()
    };
    let after = SurfaceGeometry {
        position: tessera_primitives::Point { x: 40, y: 60 },
        ..before
    };
    let old_footprint = SurfacePlacement::resolve(&before, 100, 80).bounds();
    let new_footprint = SurfacePlacement::resolve(&after, 100, 80).bounds();

    let mut tracker = ClientDamageTracker::default();
    let _ = tracker.assess([&Sample { geometry: before } as &dyn SurfaceDamageFrame]);
    let damage = tracker.assess([&Sample { geometry: after } as &dyn SurfaceDamageFrame]);
    let ClientDamage::Area(rects) = damage else {
        panic!("a move must produce localized damage");
    };
    assert!(rects.contains(&old_footprint), "vacated shadow skirt");
    assert!(rects.contains(&new_footprint), "new shadow skirt");
}

#[test]
fn tracker_maps_new_content_damage_for_unchanged_geometry() {
    struct Sample {
        generation: u64,
        damage: Vec<Rect>,
        geometry: SurfaceGeometry,
    }
    impl SurfaceDamageFrame for Sample {
        fn id(&self) -> usize {
            7
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn damage(&self) -> &[Rect] {
            &self.damage
        }
        fn damage_full(&self) -> bool {
            false
        }
        fn placement(&self) -> SurfacePlacement {
            SurfacePlacement::resolve(&self.geometry, 100, 80)
        }
    }
    let geometry = SurfaceGeometry {
        position: tessera_primitives::Point { x: 10, y: 20 },
        ..Default::default()
    };

    let mut tracker = ClientDamageTracker::default();
    assert!(matches!(
        tracker.assess([&Sample {
            generation: 3,
            damage: Vec::new(),
            geometry,
        } as &dyn SurfaceDamageFrame]),
        ClientDamage::Area(_)
    ));
    let unchanged = tracker.assess([&Sample {
        generation: 3,
        damage: Vec::new(),
        geometry,
    } as &dyn SurfaceDamageFrame]);
    assert_eq!(unchanged, ClientDamage::None);
    let content = tracker.assess([&Sample {
        generation: 4,
        damage: vec![Rect::new(2, 3, 4, 5)],
        geometry,
    } as &dyn SurfaceDamageFrame]);
    assert_eq!(content, ClientDamage::Area(vec![Rect::new(12, 23, 4, 5)]));
}

#[test]
fn hidpi_surface_maps_logical_damage_without_forcing_full() {
    struct Sample {
        generation: u64,
        damage: Vec<Rect>,
        geometry: SurfaceGeometry,
    }
    impl SurfaceDamageFrame for Sample {
        fn id(&self) -> usize {
            7
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn damage(&self) -> &[Rect] {
            &self.damage
        }
        fn damage_full(&self) -> bool {
            false
        }
        fn placement(&self) -> SurfacePlacement {
            SurfacePlacement::resolve(&self.geometry, 200, 160)
        }
    }
    let geometry = SurfaceGeometry {
        position: tessera_primitives::Point { x: 10, y: 20 },
        buffer_scale: 2,
        ..Default::default()
    };

    let mut tracker = ClientDamageTracker::default();
    assert!(matches!(
        tracker.assess([&Sample {
            generation: 3,
            damage: Vec::new(),
            geometry,
        } as &dyn SurfaceDamageFrame]),
        ClientDamage::Area(_)
    ));
    let content = tracker.assess([&Sample {
        generation: 4,
        damage: vec![Rect::new(2, 3, 4, 5)],
        geometry,
    } as &dyn SurfaceDamageFrame]);
    assert_eq!(content, ClientDamage::Area(vec![Rect::new(12, 23, 4, 5)]));
}

#[test]
fn viewport_destination_damage_is_not_scaled_twice() {
    struct Sample {
        generation: u64,
        damage: Vec<Rect>,
        geometry: SurfaceGeometry,
    }
    impl SurfaceDamageFrame for Sample {
        fn id(&self) -> usize {
            9
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn damage(&self) -> &[Rect] {
            &self.damage
        }
        fn damage_full(&self) -> bool {
            false
        }
        fn placement(&self) -> SurfacePlacement {
            SurfacePlacement::resolve(&self.geometry, 200, 160)
        }
    }
    let geometry = SurfaceGeometry {
        position: tessera_primitives::Point { x: 10, y: 20 },
        viewport_dst: Some(tessera_primitives::Size { w: 100, h: 80 }),
        ..Default::default()
    };
    let mut tracker = ClientDamageTracker::default();
    let _ = tracker.assess([&Sample {
        generation: 1,
        damage: Vec::new(),
        geometry,
    } as &dyn SurfaceDamageFrame]);
    let content = tracker.assess([&Sample {
        generation: 2,
        damage: vec![Rect::new(10, 10, 20, 20)],
        geometry,
    } as &dyn SurfaceDamageFrame]);
    // The surface is already 100x80; the raw buffer being 200x160 must not
    // shrink the damage into the top-left quadrant.
    assert_eq!(content, ClientDamage::Area(vec![Rect::new(20, 30, 20, 20)]));
}

#[test]
fn offscreen_damage_translates_origin_then_maps_to_target() {
    // A surface at desktop-logical (2005, 45) inside a virtual output
    // whose origin is (2000, 0): the target-relative rect starts at
    // (5, 45) (`[INV-ARCH-79]`).
    let mapped = offscreen_damage_to_target(
        ClientDamage::Area(vec![Rect::new(2005, 45, 10, 10)]),
        tessera_primitives::Point { x: 2000, y: 0 },
        1.0,
        (64, 64),
    );
    assert_eq!(mapped.area_rects(), Some(&[Rect::new(5, 45, 10, 10)][..]));

    // A 2x target maps logical to physical pixels after the shift.
    let scaled = offscreen_damage_to_target(
        ClientDamage::Area(vec![Rect::new(2005, 45, 10, 10)]),
        tessera_primitives::Point { x: 2000, y: 0 },
        2.0,
        (128, 128),
    );
    assert_eq!(scaled.area_rects(), Some(&[Rect::new(10, 90, 20, 20)][..]));

    // Non-area verdicts pass through unchanged.
    assert!(matches!(
        offscreen_damage_to_target(
            ClientDamage::None,
            tessera_primitives::Point { x: 0, y: 0 },
            1.0,
            (64, 64)
        ),
        FrameDamage::None
    ));
}
