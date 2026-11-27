use optics_ui::Frame;
use tessera_design::Design;
use tessera_design::icons::Glyph;
use tessera_desktop::settings::{DesktopPreferences, SettingsAction, SettingsSnapshot};
use tessera_i18n::{Localizer, Message};

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::widgets::{section_heading_layout, settings_card_layout};

pub(crate) const ACCESSIBILITY_MODULE_ID: ModuleId = ModuleId::new("accessibility");

/// Accessibility settings module governing visual, hearing, and typing aids.
pub(crate) struct AccessibilityModule {
    preferences: DesktopPreferences,
}

impl AccessibilityModule {
    pub(crate) fn new() -> Self {
        Self {
            preferences: DesktopPreferences::default(),
        }
    }
}

impl Default for AccessibilityModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for AccessibilityModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: ACCESSIBILITY_MODULE_ID,
            title: Message::Accessibility,
            icon: Glyph::Eye.id(),
            category: ModuleCategory::Personalization,
            keywords: &[
                "accessibility",
                "zoom",
                "magnifier",
                "screen reader",
                "orca",
                "contrast",
                "sticky keys",
                "slow keys",
                "bounce keys",
                "daltonism",
            ],
            apply_policy: ApplyPolicy::Instant,
            availability: ModuleAvailability::Available,
        }
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        out: &mut ModuleEvents,
    ) {
        let mut preferences = self.preferences.clone();
        let mut changed = false;

        frame
            .column()
            .with_opts(&settings_card_layout(design))
            .show(|frame| {
                frame
                    .row()
                    .with_opts(&section_heading_layout())
                    .show(|frame| {
                        frame.label_with(
                            i18n.text(Message::Accessibility),
                            &optics_ui::LabelOpts {
                                size: design.typography.title,
                                ..Default::default()
                            },
                        );
                    });

                // 1. Visual Accessibility
                frame
                    .row()
                    .with_opts(&section_heading_layout())
                    .show(|frame| {
                        frame.label_with(
                            "Visual Assistance",
                            &optics_ui::LabelOpts {
                                size: design.typography.label,
                                ..Default::default()
                            },
                        );
                        frame.flex(1.0);
                    });

                if frame.checkbox(
                    "Invert colors",
                    &mut preferences.accessibility.visual.invert_colors,
                ) {
                    changed = true;
                }

                if frame.checkbox(
                    "High contrast mode",
                    &mut preferences.accessibility.visual.high_contrast,
                ) {
                    changed = true;
                }

                // 2. Typing & Physical Assistance
                frame
                    .row()
                    .with_opts(&section_heading_layout())
                    .show(|frame| {
                        frame.label_with(
                            "Typing & Keyboard",
                            &optics_ui::LabelOpts {
                                size: design.typography.label,
                                ..Default::default()
                            },
                        );
                        frame.flex(1.0);
                    });

                if frame.checkbox(
                    "Sticky keys (latch modifiers sequentially)",
                    &mut preferences.accessibility.keyboard.sticky_keys,
                ) {
                    changed = true;
                }

                // 3. Screen Reader
                frame
                    .row()
                    .with_opts(&section_heading_layout())
                    .show(|frame| {
                        frame.label_with(
                            "Screen Reader (AT-SPI)",
                            &optics_ui::LabelOpts {
                                size: design.typography.label,
                                ..Default::default()
                            },
                        );
                        frame.flex(1.0);
                    });

                if frame.checkbox(
                    "Enable desktop accessibility bus for Orca",
                    &mut preferences.accessibility.screen_reader.enabled,
                ) {
                    changed = true;
                }
            });

        if changed {
            self.preferences = preferences.clone();
            out.actions
                .push(SettingsAction::SetDesktopPreferences { preferences });
        }
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        self.preferences = snapshot.preferences.clone();
    }
}
