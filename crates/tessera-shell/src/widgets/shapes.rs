//! Concentric geometric shapes, discs, rings, and dots.

use lens::{Color, Frame, LayoutOpts, Rect};
use tessera_design::materials;

/// Place a filled circular disc centered at `(center.0, center.1)`.
pub fn render_disc(frame: &mut Frame, id: &str, center: (f32, f32), diameter: f32, color: Color) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: color,
                border: Color::TRANSPARENT,
                radius: diameter * 0.5,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
}

/// Place a hollow circular ring centered at `(center.0, center.1)`.
pub fn render_ring(
    frame: &mut Frame,
    id: &str,
    center: (f32, f32),
    diameter: f32,
    color: Color,
    border_width: f32,
) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: Color::TRANSPARENT,
                border: color,
                border_width,
                radius: diameter * 0.5,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
}
