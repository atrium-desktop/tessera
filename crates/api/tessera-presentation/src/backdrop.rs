use super::damage::FrameDamage;

/// Backdrop effects are evaluated at full physical resolution. Liquid-glass
/// lensing samples the sharp capture directly, so a downsampled capture would
/// read as a low-resolution smear behind every glass body. The capture is
/// still clamped to the union of the declared regions (plus blur footprint),
/// and the blur itself stays cheap through the fixed-cost dual-Kawase
/// pyramid, so the full-resolution target only costs the region render.
pub const BACKDROP_DOWNSAMPLE: u32 = 1;

/// One connected backdrop capture/compute region in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdropCaptureRegion {
    pub origin: (u32, u32),
    pub extent: (u32, u32),
}

/// Map physical-output capture regions into the reusable capture image.
/// Origins round down and far edges round up so downsampling never drops a
/// source pixel required by the blur footprint.
pub fn blur_regions_in_capture(
    regions: &[BackdropCaptureRegion],
    capture_origin: (u32, u32),
    capture_extent: (u32, u32),
    capture_size: (u32, u32),
) -> Vec<flux::BlurRegion> {
    let scale_x = capture_size.0 as f32 / capture_extent.0.max(1) as f32;
    let scale_y = capture_size.1 as f32 / capture_extent.1.max(1) as f32;
    regions
        .iter()
        .filter_map(|region| {
            let rel_x0 = region.origin.0.saturating_sub(capture_origin.0);
            let rel_y0 = region.origin.1.saturating_sub(capture_origin.1);
            let rel_x1 = region
                .origin
                .0
                .saturating_add(region.extent.0)
                .saturating_sub(capture_origin.0);
            let rel_y1 = region
                .origin
                .1
                .saturating_add(region.extent.1)
                .saturating_sub(capture_origin.1);
            let x0 = ((rel_x0 as f32 * scale_x).floor() as u32).min(capture_size.0);
            let y0 = ((rel_y0 as f32 * scale_y).floor() as u32).min(capture_size.1);
            let x1 = ((rel_x1 as f32 * scale_x).ceil() as u32).min(capture_size.0);
            let y1 = ((rel_y1 as f32 * scale_y).ceil() as u32).min(capture_size.1);
            (x1 > x0 && y1 > y0).then_some(flux::BlurRegion {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            })
        })
        .collect()
}

pub fn intersect_blur_regions(
    left: &[flux::BlurRegion],
    right: &[flux::BlurRegion],
) -> Vec<flux::BlurRegion> {
    let mut intersections = Vec::new();
    for left in left {
        for right in right {
            let x0 = left.x.max(right.x);
            let y0 = left.y.max(right.y);
            let x1 = left
                .x
                .saturating_add(left.width)
                .min(right.x.saturating_add(right.width));
            let y1 = left
                .y
                .saturating_add(left.height)
                .min(right.y.saturating_add(right.height));
            if x1 > x0 && y1 > y0 {
                intersections.push(flux::BlurRegion {
                    x: x0,
                    y: y0,
                    width: x1 - x0,
                    height: y1 - y0,
                });
            }
        }
    }
    intersections
}

pub fn backdrop_refresh_regions(
    valid: bool,
    model_active: bool,
    source_damage: &FrameDamage,
    input_regions: &[BackdropCaptureRegion],
) -> Vec<BackdropCaptureRegion> {
    if model_active || !valid || matches!(source_damage, FrameDamage::Full) {
        return input_regions.to_vec();
    }
    let FrameDamage::Area(damage) = source_damage else {
        return Vec::new();
    };
    input_regions
        .iter()
        .copied()
        .filter(|region| {
            let input = tessera_model::Rect::new(
                region.origin.0 as i32,
                region.origin.1 as i32,
                region.extent.0 as i32,
                region.extent.1 as i32,
            );
            damage.iter().any(|dirty| dirty.intersect(input).is_some())
        })
        .collect()
}

/// Record `material` as the fingerprint this slot's composite will be built
/// with, returning whether the slot was previously serving a different one.
///
/// The fingerprint lifetime matches the composite image it describes — one
/// per frame slot — so a material change seen by one slot stays pending for
/// the other in-flight slots until each rebuilds its own composite. A single
/// shared fingerprint would mark the change consumed after the first slot
/// rebuilt, leaving the rest presenting a stale composite on their next
/// `Cached` frame — visible as the effect (notably the glass drop shadow)
/// flickering between two versions while the slots rotate.
pub fn slot_material_changed<T: Clone + PartialEq>(
    slots: &mut Vec<Option<T>>,
    slot: usize,
    material: &T,
) -> bool {
    if slots.len() <= slot {
        slots.resize_with(slot + 1, || None);
    }
    let changed = slots[slot].as_ref() != Some(material);
    slots[slot] = Some(material.clone());
    changed
}

/// A material change must rewrite the *entire* effect composite, not only the
/// source-damaged subset: undamaged regions would otherwise keep the previous
/// shadow/glass material indefinitely. The empty case passes through so the
/// planner can still emit `Recompute` (which already covers every capture
/// region).
pub fn refresh_regions_covering_material_change(
    material_changed: bool,
    refresh_regions: Vec<BackdropCaptureRegion>,
    capture_regions: &[BackdropCaptureRegion],
) -> Vec<BackdropCaptureRegion> {
    if material_changed && !refresh_regions.is_empty() && refresh_regions != capture_regions {
        capture_regions.to_vec()
    } else {
        refresh_regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_refresh_is_driven_by_source_footprint() {
        let input = [
            BackdropCaptureRegion {
                origin: (0, 0),
                extent: (1920, 80),
            },
            BackdropCaptureRegion {
                origin: (0, 1000),
                extent: (1920, 80),
            },
        ];
        let video_above_dock =
            FrameDamage::Area(vec![tessera_model::Rect::new(100, 100, 800, 450)]);
        assert!(backdrop_refresh_regions(true, false, &video_above_dock, &input).is_empty());
        let video_under_dock =
            FrameDamage::Area(vec![tessera_model::Rect::new(100, 1020, 800, 60)]);
        assert_eq!(
            backdrop_refresh_regions(true, false, &video_under_dock, &input),
            vec![input[1]]
        );
        assert_eq!(
            backdrop_refresh_regions(false, false, &FrameDamage::None, &input),
            input
        );
        assert_eq!(
            backdrop_refresh_regions(true, true, &FrameDamage::None, &input),
            input
        );
    }

    #[test]
    fn capture_regions_map_outward_into_downsampled_target() {
        let mapped = blur_regions_in_capture(
            &[BackdropCaptureRegion {
                origin: (101, 121),
                extent: (81, 41),
            }],
            (50, 100),
            (400, 200),
            (200, 100),
        );
        assert_eq!(
            mapped,
            vec![flux::BlurRegion {
                x: 25,
                y: 10,
                width: 41,
                height: 21,
            }]
        );
    }

    #[test]
    fn material_fingerprints_track_each_frame_slot_independently() {
        // Any fingerprint value exercises the per-slot pending semantics; the
        // composition root feeds structured material keys.
        #[derive(Clone, PartialEq)]
        struct Key(u64);
        let base = Key(7);
        let restyled = Key(11);

        // Ring warm-up: three slots each accept the base material once.
        let mut slots = Vec::new();
        for slot in 0..3 {
            assert!(slot_material_changed(&mut slots, slot, &base));
        }
        for slot in 0..3 {
            assert!(!slot_material_changed(&mut slots, slot, &base));
        }

        // A material change lands on slot 0 only. Slots 1 and 2 still serve the
        // old composite: their fingerprints must stay pending so each observes
        // the change on its own next frame instead of being told the change was
        // already consumed (which would present the stale shadow/glass composite
        // every time the ring rotates).
        assert!(slot_material_changed(&mut slots, 0, &restyled));
        assert!(!slot_material_changed(&mut slots, 0, &restyled));
        assert!(slot_material_changed(&mut slots, 1, &restyled));
        assert!(slot_material_changed(&mut slots, 2, &restyled));
        for slot in 0..3 {
            assert!(!slot_material_changed(&mut slots, slot, &restyled));
        }
    }

    #[test]
    fn a_material_change_widens_a_partial_refresh_to_every_capture_region() {
        let full = vec![
            BackdropCaptureRegion {
                origin: (0, 0),
                extent: (1920, 80),
            },
            BackdropCaptureRegion {
                origin: (0, 1000),
                extent: (1920, 80),
            },
        ];
        let partial = vec![full[1]];

        // Without a material change the source-damage plan passes through.
        assert_eq!(
            refresh_regions_covering_material_change(false, partial.clone(), &full),
            partial
        );
        // With one, the refresh must cover every capture region so no composite
        // region keeps the previous material.
        assert_eq!(
            refresh_regions_covering_material_change(true, partial, &full),
            full
        );
        // An already-complete refresh and an empty (Recompute) plan pass through.
        assert_eq!(
            refresh_regions_covering_material_change(true, full.clone(), &full),
            full
        );
        assert!(refresh_regions_covering_material_change(true, Vec::new(), &full).is_empty());
    }
}
