//! Compile-and-link coverage for the liquid-glass wrapper: the safe types
//! keep their documented shape, defaults match `OPTICS_MATERIAL_LIQUID_GLASS_DESC_INIT`,
//! and the material + gfx shared libraries link and run. The filter itself
//! needs a recording Vulkan frame, so GPU coverage stays with the C tests
//! (`meson test`) — mirroring the gfx crate, whose effect wrappers are also
//! covered at compile level only.

use tessera_material::{LiquidGlassFocus, LiquidGlassGroup, LiquidGlassParams, LiquidGlassShape};

#[test]
fn params_default_matches_c_init_macro() {
    let p = LiquidGlassParams::default();
    assert_eq!(p.refraction, 8.0);
    assert_eq!(p.chromatic_aberration, 1.25);
    assert_eq!(p.saturation, 1.08);
    assert_eq!(p.brightness, 1.02);
    assert_eq!(p.edge_width, 18.0);
    assert_eq!(p.rim_light, 0.55);
    assert_eq!(p.light_direction, (-0.45, -0.89));
    assert_eq!(p.opacity, 1.0);
    assert_eq!(p.size_reference, 72.0);
    assert_eq!(p.size_scale_min, 0.15);
    assert_eq!(p.tint_strength, 1.0);
    assert_eq!(p.frost_strength, 1.0);
}

#[test]
fn shape_focus_group_are_plain_copy_data() {
    let primary = LiquidGlassShape {
        x: 10.0,
        y: 20.0,
        width: 300.0,
        height: 200.0,
        corner_radius: 16.0,
    };
    let focus = LiquidGlassFocus {
        shape: primary,
        strength: 0.5,
    };
    let group = LiquidGlassGroup {
        primary,
        merged: None,
        blend_radius: 12.0,
        opacity: 1.0,
        shadow_alpha: 0.0,
        shadow_blur: 0.0,
        shadow_offset_y: 0.0,
        tint_color: [255, 255, 255],
        focus: Some(focus),
        frost_strength: None,
        tint_strength: None,
        saturation: None,
        curvature: None,
        contact_ao: None,
        ambient_fresnel: None,
    };
    let copied = group; // Copy
    assert_eq!(group, copied);
    assert_eq!(copied.focus.expect("focus").strength, 0.5);
    assert_eq!(copied.tint_color, [255, 255, 255]);
}

#[test]
fn group_overrides_map_none_to_inherit_sentinel() {
    let primary = LiquidGlassShape {
        x: 0.0,
        y: 0.0,
        width: 64.0,
        height: 32.0,
        corner_radius: 8.0,
    };
    let group = LiquidGlassGroup {
        primary,
        merged: None,
        blend_radius: 0.0,
        opacity: 1.0,
        shadow_alpha: 0.0,
        shadow_blur: 0.0,
        shadow_offset_y: 0.0,
        tint_color: [200, 224, 255],
        focus: None,
        frost_strength: Some(0.75),
        tint_strength: None,
        saturation: Some(1.2),
        curvature: None,
        contact_ao: Some(0.35),
        ambient_fresnel: None,
    };
    let raw = group.as_raw();
    // None → the C header's <0 inherit/disabled sentinel; Some → verbatim.
    assert_eq!(raw.frost_strength, 0.75);
    assert_eq!(raw.tint_strength, -1.0);
    assert_eq!(raw.saturation, 1.2);
    assert_eq!(raw.contact_ao, 0.35);
    assert_eq!(raw.ambient_fresnel, -1.0);
    assert_eq!(raw.tint_color, 0xC8E0FF);
    assert_eq!(raw.shape_count, 1);
}

#[test]
fn stats_method_signature_and_stat_layout() {
    // Compile-and-type level only: stats needs a recording Vulkan frame,
    // whose GPU coverage stays with the C tests (see the module note).
    fn accepts(
        filter: &mut tessera_material::LiquidGlassFilter,
        frame: &optics_gfx::Frame<'_>,
        out: &mut [tessera_material::BackdropStats],
    ) -> Result<usize, tessera_material::Error> {
        filter.stats(frame, out)
    }
    let _ = accepts;
    let stat = tessera_material::BackdropStats::default();
    assert_eq!(
        stat,
        tessera_material::BackdropStats {
            mean_luminance: 0.0,
            high_freq_energy: 0.0
        }
    );
}

#[test]
fn error_is_gfx_error_and_displays() {
    // tessera_material::Error re-exports optics_gfx::Error; Display resolves the code through
    // liboptics_gfx's optics_gfx_result_string — so this also proves the link.
    let err = tessera_material::Error(
        tessera_material::sys::optics_gfx_result::OPTICS_GFX_ERROR_INVALID_ARGUMENT,
    );
    let text = err.to_string();
    assert!(text.starts_with("gfx error:"), "unexpected: {text}");
}
