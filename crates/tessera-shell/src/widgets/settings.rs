//! Settings, control panels, and form scaffolding.

use lens::{Align, Frame, LayoutOpts};
use tessera_design::{Design, materials};

/// Returns standard layout options for a settings card surface.
pub fn settings_card_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        ..materials::card(design)
    }
}

/// Returns standard layout options for a section heading row.
pub fn section_heading_layout() -> LayoutOpts {
    LayoutOpts {
        height: 24.0,
        gap: 8.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// Render a standard unavailable/stub setting row.
pub fn render_unavailable_row(
    frame: &mut Frame,
    label: &str,
    unavailable_text: &str,
    design: &Design,
) {
    frame.row_ex(
        &LayoutOpts {
            height: 26.0,
            gap: 8.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.label_sized(label, design.typography.label);
            frame.flex(1.0);
            frame.spacer(0.0);
            frame.label_sized(unavailable_text, design.typography.footnote);
        },
    );
}
