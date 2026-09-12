//! Privacy-safe synthetic PNG comparison; run with --profile release-fast.
//! CSV times exclude scene generation/input cloning, include RGB preparation,
//! and report median/min/max over five encodes after one warm-up per case.
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use std::{hint::black_box, time::Instant};

fn scene(width: u32, height: u32, kind: &str) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    let mut seed = 0x12345678u32;
    for y in 0..height {
        for x in 0..width {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let gradient = [
                (x * 255 / width) as u8,
                (y * 255 / height) as u8,
                ((x + y) * 127 / (width + height)) as u8,
            ];
            let rgb = match kind {
                // Generated pseudo-glyph strokes, panels, title bars, separators.
                "ui" => {
                    if y % 240 < 28 {
                        [48, 58, 78]
                    } else if x % 480 < 2 || y % 24 == 0 {
                        [110, 115, 125]
                    } else if x % 480 < 380
                        && y % 24 > 7
                        && y % 24 < 18
                        && ((x / 8 + y / 24) % 7 < 5)
                        && (x % 8 < 2 || y % 5 == 0)
                    {
                        [210, 215, 220]
                    } else {
                        [28, 31, 38]
                    }
                }
                "gradient" => gradient,
                // Smooth spatial colour plus deterministic fine texture. Not a photograph.
                "photo-like" => gradient.map(|c| c.saturating_add((seed & 31) as u8)),
                _ => unreachable!(),
            };
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    pixels
}

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("use --profile release-fast");
        std::process::exit(1);
    }
    println!("scene,width,height,color,profile,bytes,median_ms,min_ms,max_ms");
    for (w, h) in [(1920, 1080), (3840, 2160)] {
        for kind in ["ui", "gradient", "photo-like"] {
            let source = scene(w, h, kind);
            for rgb in [false, true] {
                for (name, compression, filter) in [
                    ("Fast+Up", CompressionType::Fast, FilterType::Up),
                    ("Fast+Adaptive", CompressionType::Fast, FilterType::Adaptive),
                    (
                        "Default+Adaptive",
                        CompressionType::Default,
                        FilterType::Adaptive,
                    ),
                    ("Fast+Paeth", CompressionType::Fast, FilterType::Paeth),
                ] {
                    let mut times = Vec::new();
                    let mut bytes = 0;
                    for iteration in 0..6 {
                        let mut input = source.clone();
                        let start = Instant::now();
                        let color = if rgb {
                            assert!(input.chunks_exact(4).all(|p| p[3] == 255));
                            for i in 0..input.len() / 4 {
                                input.copy_within(i * 4..i * 4 + 3, i * 3);
                            }
                            input.truncate(input.len() / 4 * 3);
                            ExtendedColorType::Rgb8
                        } else {
                            ExtendedColorType::Rgba8
                        };
                        let mut png = Vec::new();
                        PngEncoder::new_with_quality(&mut png, compression, filter)
                            .write_image(black_box(&input), w, h, color)
                            .unwrap();
                        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                        bytes = black_box(png.len());
                        if iteration > 0 {
                            times.push(elapsed);
                        }
                        // Verify every profile outside the timed region.
                        if iteration == 0 {
                            assert_eq!(
                                image::load_from_memory(&png).unwrap().into_rgba8().as_raw(),
                                &source
                            );
                        }
                    }
                    times.sort_by(f64::total_cmp);
                    println!(
                        "{kind},{w},{h},{},{name},{bytes},{:.3},{:.3},{:.3}",
                        if rgb { "RGB" } else { "RGBA" },
                        times[2],
                        times[0],
                        times[4]
                    );
                }
            }
        }
    }
}
