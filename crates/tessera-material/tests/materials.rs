//! Tests for the frosted material binding.

use tessera_material::{FrostedGroup, FrostedParams, FrostedShape};

#[test]
fn frosted_params_default() {
    let p = FrostedParams::default();
    assert_eq!(p.opacity, 1.0);
    assert_eq!(p.saturation, 1.25);
    assert_eq!(p.tint_strength, 0.15);
    assert_eq!(p.noise_intensity, 0.015);
}

#[test]
fn frosted_group_mapping() {
    let group = FrostedGroup {
        shape: FrostedShape {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            corner_radius: 8.0,
        },
        opacity: 0.9,
        tint_color: [255, 128, 64],
        tint_strength: Some(0.3),
        saturation: None,
        shadow_alpha: 0.25,
        shadow_blur: 10.0,
        shadow_offset_y: 4.0,
        noise_intensity: Some(0.02),
    };

    let raw = group.as_raw();
    assert_eq!(raw.shape.bounds.x, 10.0);
    assert_eq!(raw.shape.bounds.y, 20.0);
    assert_eq!(raw.shape.bounds.w, 100.0);
    assert_eq!(raw.shape.bounds.h, 50.0);
    assert_eq!(raw.shape.corner_radius, 8.0);
    assert_eq!(raw.opacity, 0.9);
    assert_eq!(raw.tint_color, 0xFF8040);
    assert_eq!(raw.tint_strength, 0.3);
    assert_eq!(raw.saturation, -1.0); // Sentinel
    assert_eq!(raw.noise_intensity, 0.02);
}
