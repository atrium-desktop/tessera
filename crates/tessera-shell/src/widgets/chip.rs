//! Status chips, pills, and badges for HUD and chrome surfaces.

use lens::{Color, LayoutOpts, Style};
use tessera_design::{Design, materials};

/// The floating HUD chip layout options. The physical body and rim lighting are
/// provided exclusively by the compositor's liquid glass pass, so the painted
/// background and border remain clean and transparent.
pub fn chip_opts(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: design.radii.chip,
        pad: 0.0,
        ..materials::surface_layout()
    }
}

/// Text contour outline parameters: `(color, width)`.
pub fn hud_text_outline_params(design: &Design) -> (Color, f32) {
    let hud = design.hud_foreground;
    (hud.contour, hud.text_contour_width)
}

/// Style with text contour outline for HUD typography readability over dynamic backdrops.
pub fn hud_text_outline(design: &Design) -> Style {
    let (color, width) = hud_text_outline_params(design);
    Style::new()
        .with_outline_color(color)
        .with_outline_width(width)
}

/// Glyph/icon contour outline parameters: `(color, width)`.
pub fn hud_glyph_outline_params(design: &Design) -> (Color, f32) {
    let hud = design.hud_foreground;
    (hud.contour, hud.glyph_contour_width)
}

/// Style with glyph contour outline for HUD icons.
pub fn hud_glyph_outline(design: &Design) -> Style {
    let (color, width) = hud_glyph_outline_params(design);
    Style::new()
        .with_outline_color(color)
        .with_outline_width(width)
}

/// Calculate dynamic color for a workspace dot based on pagination intensity `[0.0, 1.0]`.
pub fn workspace_dot_color(design: &Design, intensity: f32) -> Color {
    let primary = design.hud_foreground.primary;
    let intensity = intensity.clamp(0.0, 1.0);
    let alpha = (78.0 + (248.0 - 78.0) * intensity).round() as u8;
    primary.with_alpha(alpha)
}

/// Calculate workspace dot highlight intensity for dot `index` given continuous pagination `position`.
pub fn workspace_dot_intensity(index: usize, position: f32) -> f32 {
    (1.0 - (index as f32 - position).abs()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_dot_intensity() {
        assert_eq!(workspace_dot_intensity(0, 0.0), 1.0);
        assert_eq!(workspace_dot_intensity(1, 0.0), 0.0);
        assert_eq!(workspace_dot_intensity(0, 0.5), 0.5);
        assert_eq!(workspace_dot_intensity(1, 0.5), 0.5);
    }
}
