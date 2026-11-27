//! Form scaffolding shared by the settings pages: card and section-heading
//! layouts and the honest "unavailable" row.

use optics_ui::{Align, Frame, LayoutOpts};
use tessera_design::{Design, materials};
use tessera_i18n::{Localizer, Message};

/// Layout options for one settings card surface.
pub(crate) fn settings_card_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        ..materials::card(design)
    }
}

/// Layout options for one section heading row.
pub(crate) fn section_heading_layout() -> LayoutOpts {
    LayoutOpts {
        height: 24.0,
        gap: 8.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A setting row whose backend cannot serve it: the label stays visible so the
/// page's shape does not depend on the hardware, and the value reads as
/// unavailable instead of disappearing.
pub(crate) fn unavailable_row(frame: &mut Frame, label: &str, i18n: &Localizer, design: &Design) {
    frame
        .row()
        .with_opts(&LayoutOpts {
            height: 26.0,
            gap: 8.0,
            cross: Align::Center,
            ..Default::default()
        })
        .show(|frame| {
            frame.label_with(
                label,
                &optics_ui::LabelOpts {
                    size: design.typography.label,
                    ..Default::default()
                },
            );
            frame.flex(1.0);
            frame.spacer(0.0);
            frame.label_with(
                i18n.text(Message::Unavailable),
                &optics_ui::LabelOpts {
                    size: design.typography.footnote,
                    ..Default::default()
                },
            );
        });
}
