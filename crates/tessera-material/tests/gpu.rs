//! GPU smoke tests: both filters dispatch on a real device and composite a
//! body that differs from the sharp capture beneath it.
//!
//! The capture is a hard red/blue split. Inside a body centred on the seam,
//! the blurred backdrop must mix the two colours; outside every body the
//! slab is transparent, so the sharp capture shows through unchanged.

use tessera_material::{
    FrostedFilter, FrostedGroup, FrostedParams, FrostedShape, LiquidGlassFilter, LiquidGlassGroup,
    LiquidGlassParams, LiquidGlassShape,
};

const W: u32 = 128;
const H: u32 = 96;
const BODY: (f32, f32, f32, f32) = (32.0, 24.0, 64.0, 48.0);

enum Kind {
    Frosted,
    Glass,
}

/// Render the split capture, run `kind` over it, draw capture + slab to an
/// offscreen surface, and return its RGBA pixels. `None` without Vulkan.
fn render(kind: Kind) -> Option<Vec<u8>> {
    let device = optics_gfx::Device::new(true, &[], &[], 0).ok()?;
    let surface = optics_gfx::Surface::offscreen(&device, W, H).unwrap();
    let mut canvas = optics_gfx::Canvas::new(&surface).unwrap();
    let mut capture = optics_gfx::Image::render_target(&device, W, H, surface.format()).unwrap();
    let mut blur = optics_gfx::BlurFilter::new(&device).unwrap();
    let mut frosted = FrostedFilter::new(&device).unwrap();
    let mut glass = LiquidGlassFilter::new(&device).unwrap();

    let mut frame = surface.begin_frame().unwrap();
    let session = canvas
        .begin_session(
            optics_gfx::ImageTarget {
                frame: &mut frame,
                image: &mut capture,
            },
            optics_gfx::CanvasPassOptions {
                clear: Some(optics_gfx::rgba(220, 20, 20, 255)),
                ..Default::default()
            },
        )
        .unwrap();
    session.fill_rect(
        (W / 2) as f32,
        0.0,
        (W / 2) as f32,
        H as f32,
        optics_gfx::rgba(20, 20, 220, 255),
    );
    session.end().unwrap();

    let blurred = blur.apply(&frame, &capture, 12.0).unwrap();
    let (x, y, w, h) = BODY;
    enum Slab<'a> {
        Frosted(tessera_material::FrostedImage<'a>),
        Glass(tessera_material::LiquidGlassImage<'a>),
    }
    let slab = match kind {
        Kind::Frosted => Slab::Frosted(
            frosted
                .apply(
                    &frame,
                    &capture,
                    &blurred,
                    &[FrostedGroup {
                        shape: FrostedShape {
                            x,
                            y,
                            width: w,
                            height: h,
                            corner_radius: 12.0,
                        },
                        opacity: 1.0,
                        tint_color: [0, 0, 0],
                        tint_strength: Some(0.0),
                        saturation: None,
                        shadow_alpha: 0.0,
                        shadow_blur: 0.0,
                        shadow_offset_y: 0.0,
                        noise_intensity: Some(0.0),
                    }],
                    FrostedParams::default(),
                )
                .expect("frosted dispatch"),
        ),
        Kind::Glass => Slab::Glass(
            glass
                .apply(
                    &frame,
                    &capture,
                    &blurred,
                    &[LiquidGlassGroup {
                        primary: LiquidGlassShape {
                            x,
                            y,
                            width: w,
                            height: h,
                            corner_radius: 16.0,
                        },
                        merged: None,
                        blend_radius: 0.0,
                        opacity: 1.0,
                        shadow_alpha: 0.0,
                        shadow_blur: 0.0,
                        shadow_offset_y: 0.0,
                        tint_color: [0, 0, 0],
                        focus: None,
                        frost_strength: Some(4.0),
                        tint_strength: Some(0.0),
                        saturation: None,
                        curvature: None,
                        contact_ao: None,
                        ambient_fresnel: None,
                    }],
                    LiquidGlassParams::default(),
                )
                .expect("liquid-glass dispatch"),
        ),
    };

    let session = canvas
        .begin_session(
            &mut frame,
            optics_gfx::CanvasPassOptions {
                clear: Some(optics_gfx::rgba(0, 0, 0, 255)),
                ..Default::default()
            },
        )
        .unwrap();
    session.draw_image(&capture, 0.0, 0.0, W as f32, H as f32);
    match &slab {
        Slab::Frosted(image) => image.draw(&session),
        Slab::Glass(image) => image.draw(&session),
    }
    session.end().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut pixels = vec![0u8; (W * H * 4) as usize];
    surface.read_pixels(&mut pixels).unwrap();
    Some(pixels)
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Red/blue channel pair regardless of the surface's RGBA/BGRA order.
fn red_blue(p: [u8; 4]) -> (u8, u8) {
    let (a, b) = (p[0], p[2]);
    (a.max(b), a.min(b))
}

fn assert_body_mixes_the_split(pixels: &[u8], what: &str) {
    // On the seam inside the body: both channels substantially present.
    let seam = pixel(pixels, W / 2, H / 2);
    let (hi, lo) = red_blue(seam);
    assert!(
        lo > 50 && hi > 50,
        "{what}: seam pixel {seam:?} does not mix the blurred red/blue backdrop"
    );
    // Far outside the body: the sharp capture, one channel dominant.
    let outside = pixel(pixels, 4, 4);
    let (hi, lo) = red_blue(outside);
    assert!(
        hi > 180 && lo < 60,
        "{what}: pixel outside the body {outside:?} is not the sharp capture"
    );
}

#[test]
fn frosted_filter_blurs_the_backdrop_inside_its_body_only() {
    let Some(pixels) = render(Kind::Frosted) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    assert_body_mixes_the_split(&pixels, "frosted");
}

#[test]
fn liquid_glass_filter_composites_a_lens_inside_its_body_only() {
    let Some(pixels) = render(Kind::Glass) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    assert_body_mixes_the_split(&pixels, "liquid glass");
}
