use optics_ui::{Align, Frame, LayoutOpts};
use tessera_design::Design;
use tessera_design::icons::Glyph;
use tessera_desktop::settings::SettingsSnapshot;
use tessera_i18n::{Localizer, Message};

use crate::module::{ModuleEvents, ModuleMetadata, SettingsModule};
use crate::widgets::settings_card_layout;

/// A discoverable module whose ownership boundary is settled while its
/// authoritative backend is still absent. Keeping these pages honest makes
/// routes and navigation stable without presenting controls that cannot be
/// persisted or applied.
pub(crate) struct UnavailableModule {
    metadata: ModuleMetadata,
    description: Message,
}

impl UnavailableModule {
    pub(crate) const fn new(metadata: ModuleMetadata, description: Message) -> Self {
        Self {
            metadata,
            description,
        }
    }
}

impl SettingsModule for UnavailableModule {
    fn metadata(&self) -> ModuleMetadata {
        self.metadata
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        _out: &mut ModuleEvents,
    ) {
        frame.heading(i18n.text(self.metadata.title), 2);
        frame.label_with(
            i18n.text(self.description),
            &optics_ui::LabelOpts {
                size: design.typography.label,
                wrap: true,
                max_width: 560.0,
                ..Default::default()
            },
        );
        frame
            .column()
            .with_opts(&settings_card_layout(design))
            .show(|frame| {
                frame
                    .row()
                    .with_opts(&LayoutOpts {
                        gap: 10.0,
                        cross: Align::Center,
                        ..Default::default()
                    })
                    .show(|frame| {
                        frame.icon(Glyph::Shield.id(), 22.0);
                        frame.heading(i18n.text(Message::SettingsBackendUnavailable), 3);
                    });
                frame.label_with(
                    i18n.text(Message::SettingsBackendUnavailableDescription),
                    &optics_ui::LabelOpts {
                        size: design.typography.footnote,
                        wrap: true,
                        max_width: 560.0,
                        ..Default::default()
                    },
                );
            });
    }

    fn update_settings(&mut self, _snapshot: &SettingsSnapshot) {}
}
