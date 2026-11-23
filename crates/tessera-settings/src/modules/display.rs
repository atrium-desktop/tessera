use optics_ui::{Align, Frame, LayoutOpts, TextBuf};
use tessera_design::Design;
use tessera_desktop::output::ModeSpec;
use tessera_desktop::output::OutputInfo;
use tessera_desktop::output::OutputMode;
use tessera_desktop::output::OutputRole;
use tessera_desktop::settings::DisplaySettings;
use tessera_desktop::settings::DisplayStatus;
use tessera_desktop::settings::SettingsAction;
use tessera_desktop::settings::SettingsSnapshot;
use tessera_i18n::{Localizer, Message};
use tessera_primitives::Point;

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::widgets::{section_heading_layout, settings_card_layout, unavailable_row};

pub(crate) const DISPLAY_MODULE_ID: ModuleId = ModuleId::new("display");

const LAYOUT_RIGHT: i32 = 0;
const LAYOUT_LEFT: i32 = 1;
const LAYOUT_ABOVE: i32 = 2;
const LAYOUT_BELOW: i32 = 3;
const LAYOUT_CUSTOM: i32 = 4;

const ROLE_AUTO: i32 = 0;
const ROLE_EXTEND: i32 = 1;
const ROLE_MIRROR: i32 = 2;

/// Display editor with an explicit apply boundary.
pub(crate) struct DisplayModule {
    status: DisplayStatus,
    output_index: i32,
    mode_index: i32,
    scale: f32,
    primary: bool,
    role: i32,
    mirror_source_index: i32,
    topology_index: i32,
    layout: i32,
    x: TextBuf,
    y: TextBuf,
    editor_connector: Option<String>,
    dirty: bool,
}

impl DisplayModule {
    pub(crate) fn new() -> Self {
        Self {
            status: DisplayStatus::default(),
            output_index: 0,
            mode_index: 0,
            scale: 1.0,
            primary: true,
            role: ROLE_AUTO,
            mirror_source_index: 0,
            topology_index: 0,
            layout: LAYOUT_RIGHT,
            x: TextBuf::new(16, "0"),
            y: TextBuf::new(16, "0"),
            editor_connector: None,
            dirty: false,
        }
    }

    fn sync_editor(&mut self, connector: Option<&str>) {
        let outputs = &self.status.outputs;
        if outputs.is_empty() {
            self.output_index = 0;
            self.mode_index = 0;
            self.role = ROLE_AUTO;
            self.mirror_source_index = 0;
            self.editor_connector = None;
            self.dirty = false;
            return;
        }
        let preferred = connector
            .or(self.editor_connector.as_deref())
            .and_then(|connector| {
                outputs
                    .iter()
                    .position(|output| output.connector == connector)
            })
            .unwrap_or(0);
        let output = outputs[preferred].clone();
        self.output_index = preferred as i32;
        self.editor_connector = Some(output.connector.clone());
        self.mode_index = output
            .available_modes
            .iter()
            .position(|mode| *mode == output.geometry.mode)
            .unwrap_or(0) as i32;
        self.scale = output.geometry.scale.as_f32().clamp(0.25, 4.0);
        self.primary = preferred == 0;
        self.role = if output.role_source.is_auto() {
            ROLE_AUTO
        } else if output.role.is_mirror() {
            ROLE_MIRROR
        } else {
            ROLE_EXTEND
        };
        // Keep the source index ready even when the effective role is an
        // automatic mirror: switching the dropdown to Mirror should start at
        // the source the user is already looking at.
        self.mirror_source_index = match &output.role {
            OutputRole::Mirror { source } => outputs
                .iter()
                .filter(|candidate| {
                    candidate.connector != output.connector
                        && candidate.kind.is_physical()
                        && !candidate.role.is_mirror()
                })
                .position(|candidate| &candidate.connector == source)
                .unwrap_or(0) as i32,
            OutputRole::Extend => 0,
        };
        self.x.set(&output.geometry.logical_origin.x.to_string());
        self.y.set(&output.geometry.logical_origin.y.to_string());
        self.layout = outputs
            .first()
            .map(|primary| infer_layout(&output, primary))
            .unwrap_or(LAYOUT_CUSTOM);
        self.dirty = false;
    }

    fn position(&self, output: &OutputInfo, mode: OutputMode) -> Option<Point> {
        let custom = || {
            Some(Point {
                x: self.x.as_str().trim().parse().ok()?,
                y: self.y.as_str().trim().parse().ok()?,
            })
        };
        if self.layout == LAYOUT_CUSTOM {
            return custom();
        }
        let primary = self.status.outputs.first()?;
        if primary.connector == output.connector {
            return custom();
        }
        let primary_rect = primary.geometry.logical_rect();
        let scale = self.scale.clamp(0.25, 4.0);
        let selected_w = ((mode.width as f32) / scale).round().max(1.0) as i32;
        let selected_h = ((mode.height as f32) / scale).round().max(1.0) as i32;
        Some(match self.layout {
            LAYOUT_LEFT => Point {
                x: primary_rect.origin.x - selected_w,
                y: primary_rect.origin.y,
            },
            LAYOUT_ABOVE => Point {
                x: primary_rect.origin.x,
                y: primary_rect.origin.y - selected_h,
            },
            LAYOUT_BELOW => Point {
                x: primary_rect.origin.x,
                y: primary_rect.origin.y + primary_rect.size.h,
            },
            _ => Point {
                x: primary_rect.origin.x + primary_rect.size.w,
                y: primary_rect.origin.y,
            },
        })
    }
}

impl Default for DisplayModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for DisplayModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: DISPLAY_MODULE_ID,
            title: Message::Display,
            icon: optics_ui::sys::optics_ui_icon_id::OPTICS_UI_ICON_MONITOR,
            category: ModuleCategory::Hardware,
            keywords: &["display", "monitor", "resolution", "refresh", "scale"],
            apply_policy: ApplyPolicy::Explicit,
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
        frame.heading(i18n.text(Message::Display), 2);
        frame.label_wrapped_sized(
            i18n.text(Message::DisplayDescription),
            design.typography.label,
            560.0,
        );

        if let Some(seconds) = self.status.pending_revert_seconds {
            frame.label_wrapped_sized(
                &format!("{} {seconds}s", i18n.text(Message::DisplayRevertPending)),
                design.typography.footnote,
                560.0,
            );
            frame.size_next(160.0, 30.0);
            if frame.button(i18n.text(Message::ConfirmDisplay)) {
                out.actions.push(SettingsAction::ConfirmDisplay);
            }
        }
        if self.status.configurable {
            let topology_labels = [
                i18n.text(Message::Automatic),
                i18n.text(Message::ExtendDesktop),
                i18n.text(Message::MirrorDisplay),
            ];
            frame.label_sized(i18n.text(Message::DefaultTopology), design.typography.label);
            let before = self.topology_index;
            frame.dropdown(
                "##settings-display-topology",
                &mut self.topology_index,
                &topology_labels,
            );
            if self.topology_index != before {
                out.actions.push(SettingsAction::SetDisplayTopology {
                    topology: topology_from_index(self.topology_index),
                });
            }
        }

        let outputs = self.status.outputs.clone();
        frame.column_ex(&settings_card_layout(design), |frame| {
            if outputs.is_empty() {
                unavailable_row(frame, i18n.text(Message::NoDisplays), i18n, design);
                return;
            }
            let output_labels = outputs
                .iter()
                .map(|output| {
                    format!(
                        "{} · {} × {}",
                        output.connector, output.geometry.mode.width, output.geometry.mode.height
                    )
                })
                .collect::<Vec<_>>();
            let output_items = output_labels.iter().map(String::as_str).collect::<Vec<_>>();
            let before_output = self.output_index;
            frame.label_sized(i18n.text(Message::Displays), design.typography.label);
            frame.dropdown(
                "##settings-display-output",
                &mut self.output_index,
                &output_items,
            );
            self.output_index = self
                .output_index
                .clamp(0, outputs.len().saturating_sub(1) as i32);
            if self.output_index != before_output
                || self.editor_connector.as_deref()
                    != Some(outputs[self.output_index as usize].connector.as_str())
            {
                let connector = outputs[self.output_index as usize].connector.clone();
                self.sync_editor(Some(&connector));
            }
            let output = outputs[self.output_index as usize].clone();

            if !self.status.configurable {
                frame.label_wrapped_sized(
                    i18n.text(Message::DisplayHostManaged),
                    design.typography.footnote,
                    560.0,
                );
                display_summary(frame, &output, design);
                return;
            }

            let can_mirror = output.kind.is_physical();
            if can_mirror {
                let role_labels = [
                    i18n.text(Message::Automatic),
                    i18n.text(Message::ExtendDesktop),
                    i18n.text(Message::MirrorDisplay),
                ];
                frame.label_sized(i18n.text(Message::OutputRole), design.typography.label);
                if frame.dropdown("##settings-display-role", &mut self.role, &role_labels) {
                    self.dirty = true;
                }
            } else {
                self.role = ROLE_AUTO;
            }
            let mirroring = can_mirror && self.role == ROLE_MIRROR;
            let automatic = self.role == ROLE_AUTO;
            let mut mirror_source: Option<String> = None;
            if mirroring {
                let candidates = outputs
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| {
                        candidate.connector != output.connector
                            && candidate.kind.is_physical()
                            && !candidate.role.is_mirror()
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                if candidates.is_empty() {
                    frame.label_wrapped_sized(
                        i18n.text(Message::NoMirrorSource),
                        design.typography.footnote,
                        560.0,
                    );
                } else {
                    let source_labels = candidates
                        .iter()
                        .map(|index| {
                            let candidate = &outputs[*index];
                            format!(
                                "{} · {} × {}",
                                candidate.connector,
                                candidate.geometry.mode.width,
                                candidate.geometry.mode.height
                            )
                        })
                        .collect::<Vec<_>>();
                    let source_items = source_labels.iter().map(String::as_str).collect::<Vec<_>>();
                    self.mirror_source_index = self
                        .mirror_source_index
                        .clamp(0, candidates.len().saturating_sub(1) as i32);
                    frame.label_sized(i18n.text(Message::MirrorSource), design.typography.label);
                    if frame.dropdown(
                        "##settings-display-mirror-source",
                        &mut self.mirror_source_index,
                        &source_items,
                    ) {
                        self.dirty = true;
                    }
                    mirror_source = Some(
                        outputs[candidates[self.mirror_source_index as usize]]
                            .connector
                            .clone(),
                    );
                    frame.label_wrapped_sized(
                        i18n.text(Message::MirrorFollowsSource),
                        design.typography.footnote,
                        560.0,
                    );
                }
            }

            if automatic {
                frame.label_wrapped_sized(
                    i18n.text(Message::FollowsAutomaticTopology),
                    design.typography.footnote,
                    560.0,
                );
                frame.label_wrapped_sized(&output.role.label(), design.typography.footnote, 560.0);
            }

            let modes = if output.available_modes.is_empty() {
                vec![output.geometry.mode]
            } else {
                output.available_modes.clone()
            };
            let mode_labels = modes.iter().map(format_output_mode).collect::<Vec<_>>();
            let mode_items = mode_labels.iter().map(String::as_str).collect::<Vec<_>>();
            self.mode_index = self
                .mode_index
                .clamp(0, modes.len().saturating_sub(1) as i32);
            frame.label_sized(
                i18n.text(Message::ResolutionAndRefresh),
                design.typography.label,
            );
            if frame.dropdown("##settings-display-mode", &mut self.mode_index, &mode_items) {
                self.dirty = true;
            }
            let mut selected_mode: Option<OutputMode> = Some(modes[self.mode_index as usize]);
            let mut selected_position: Option<Point> = None;
            if !mirroring {
                frame.row_ex(&section_heading_layout(), |frame| {
                    frame.label_sized(i18n.text(Message::Scale), design.typography.label);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    frame.label_sized(
                        &format!("{:.0}%", self.scale * 100.0),
                        design.typography.label,
                    );
                });
                if frame.slider("##settings-display-scale", &mut self.scale, 0.25, 4.0) {
                    self.scale = (self.scale * 4.0).round() / 4.0;
                    self.dirty = true;
                }

                if self.primary {
                    frame.label_sized(i18n.text(Message::PrimaryDisplay), design.typography.label);
                } else {
                    frame.size_next(160.0, 30.0);
                    if frame.button(i18n.text(Message::MakePrimary)) {
                        self.primary = true;
                        self.dirty = true;
                    }
                }

                let arrangement_labels = [
                    i18n.text(Message::RightOfPrimary),
                    i18n.text(Message::LeftOfPrimary),
                    i18n.text(Message::AbovePrimary),
                    i18n.text(Message::BelowPrimary),
                    i18n.text(Message::CustomPosition),
                ];
                frame.label_sized(i18n.text(Message::Arrangement), design.typography.label);
                let current_primary = outputs
                    .first()
                    .is_some_and(|primary| primary.connector == output.connector);
                if current_primary {
                    self.layout = LAYOUT_CUSTOM;
                    frame.label_sized(
                        i18n.text(Message::CustomPosition),
                        design.typography.footnote,
                    );
                } else if frame.dropdown(
                    "##settings-display-arrangement",
                    &mut self.layout,
                    &arrangement_labels,
                ) {
                    self.dirty = true;
                }
                if self.layout == LAYOUT_CUSTOM {
                    frame.row_ex(
                        &LayoutOpts {
                            gap: 10.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.flex(1.0);
                            frame.column_ex(
                                &LayoutOpts {
                                    gap: 4.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |frame| {
                                    frame.label_sized(
                                        i18n.text(Message::HorizontalPosition),
                                        design.typography.footnote,
                                    );
                                    let resp_x = frame.textfield_ex(
                                        "##settings-display-position-x",
                                        &mut self.x,
                                        true,
                                    );
                                    if resp_x.changed || resp_x.submitted {
                                        self.dirty = true;
                                    }
                                },
                            );
                            frame.flex(1.0);
                            frame.column_ex(
                                &LayoutOpts {
                                    gap: 4.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |frame| {
                                    frame.label_sized(
                                        i18n.text(Message::VerticalPosition),
                                        design.typography.footnote,
                                    );
                                    let resp_y = frame.textfield_ex(
                                        "##settings-display-position-y",
                                        &mut self.y,
                                        true,
                                    );
                                    if resp_y.changed || resp_y.submitted {
                                        self.dirty = true;
                                    }
                                },
                            );
                        },
                    );
                }

                let mode = modes[self.mode_index as usize];
                let position = self.position(&output, mode);
                if position.is_none() {
                    frame.label_wrapped_sized(
                        i18n.text(Message::InvalidPosition),
                        design.typography.footnote,
                        560.0,
                    );
                }
                selected_mode = Some(mode);
                selected_position = position;
            }
            if let Some(error) = self.status.error.as_deref() {
                frame.label_wrapped_sized(error, design.typography.footnote, 560.0);
            }
            frame.label_wrapped_sized(
                i18n.text(Message::DisplayApplyHint),
                design.typography.footnote,
                560.0,
            );
            frame.row_ex(
                &LayoutOpts {
                    height: 32.0,
                    gap: 8.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.size_next(190.0, 30.0);
                    let apply = frame.button(i18n.text(Message::ApplyDisplaySettings));
                    if apply && self.dirty {
                        let settings = if mirroring {
                            mirror_source.clone().map(|source| DisplaySettings {
                                connector: output.connector.clone(),
                                role: Some(OutputRole::Mirror { source }),
                                mode: selected_mode.map(mode_spec),
                                scale: None,
                                position: None,
                                primary: false,
                                tentative: true,
                            })
                        } else {
                            selected_mode
                                .zip(selected_position)
                                .map(|(mode, position)| DisplaySettings {
                                    connector: output.connector.clone(),
                                    role: if automatic {
                                        None
                                    } else {
                                        Some(OutputRole::Extend)
                                    },
                                    mode: Some(mode_spec(mode)),
                                    scale: Some(f64::from(self.scale.clamp(0.25, 4.0))),
                                    position: Some(position),
                                    primary: self.primary,
                                    tentative: true,
                                })
                        };
                        if let Some(settings) = settings {
                            out.actions.push(SettingsAction::SetDisplay { settings });
                            self.dirty = false;
                        }
                    }
                    frame.size_next(92.0, 30.0);
                    if frame.button(i18n.text(Message::ResetDisplaySettings)) {
                        let connector = output.connector.clone();
                        self.sync_editor(Some(&connector));
                    }
                },
            );
        });
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        let selected = self.editor_connector.clone();
        let selected_still_present = selected.as_deref().is_some_and(|connector| {
            snapshot
                .display
                .outputs
                .iter()
                .any(|output| output.connector == connector)
        });
        self.status = snapshot.display.clone();
        self.topology_index = topology_index(self.status.topology);
        if !self.dirty || !selected_still_present {
            self.sync_editor(selected.as_deref());
        }
    }
}

fn topology_index(topology: tessera_desktop::output::DisplayTopology) -> i32 {
    match topology {
        tessera_desktop::output::DisplayTopology::Auto => 0,
        tessera_desktop::output::DisplayTopology::Extend => 1,
        tessera_desktop::output::DisplayTopology::Mirror => 2,
    }
}

fn topology_from_index(index: i32) -> tessera_desktop::output::DisplayTopology {
    match index {
        1 => tessera_desktop::output::DisplayTopology::Extend,
        2 => tessera_desktop::output::DisplayTopology::Mirror,
        _ => tessera_desktop::output::DisplayTopology::Auto,
    }
}

fn infer_layout(output: &OutputInfo, primary: &OutputInfo) -> i32 {
    if output.connector == primary.connector {
        return LAYOUT_CUSTOM;
    }
    let output_rect = output.geometry.logical_rect();
    let primary_rect = primary.geometry.logical_rect();
    if output_rect.origin.x == primary_rect.origin.x + primary_rect.size.w
        && output_rect.origin.y == primary_rect.origin.y
    {
        LAYOUT_RIGHT
    } else if output_rect.origin.x + output_rect.size.w == primary_rect.origin.x
        && output_rect.origin.y == primary_rect.origin.y
    {
        LAYOUT_LEFT
    } else if output_rect.origin.y + output_rect.size.h == primary_rect.origin.y
        && output_rect.origin.x == primary_rect.origin.x
    {
        LAYOUT_ABOVE
    } else if output_rect.origin.y == primary_rect.origin.y + primary_rect.size.h
        && output_rect.origin.x == primary_rect.origin.x
    {
        LAYOUT_BELOW
    } else {
        LAYOUT_CUSTOM
    }
}

fn mode_spec(mode: OutputMode) -> ModeSpec {
    ModeSpec {
        width: mode.width,
        height: mode.height,
        refresh_hz: Some(mode.refresh_mhz.saturating_add(500) / 1_000),
    }
}

fn format_output_mode(mode: &OutputMode) -> String {
    let refresh = if mode.refresh_mhz.is_multiple_of(1_000) {
        format!("{} Hz", mode.refresh_mhz / 1_000)
    } else {
        format!("{:.2} Hz", mode.refresh_mhz as f64 / 1_000.0)
    };
    format!("{} × {} · {refresh}", mode.width, mode.height)
}

fn display_summary(frame: &mut Frame, output: &OutputInfo, design: &Design) {
    frame.label_sized(
        &format_output_mode(&output.geometry.mode),
        design.typography.label,
    );
    frame.label_sized(
        &format!(
            "{:.0}% · ({}, {})",
            output.geometry.scale.as_f32() * 100.0,
            output.geometry.logical_origin.x,
            output.geometry.logical_origin.y
        ),
        design.typography.footnote,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(connector: &str, x: i32, y: i32, width: i32, height: i32) -> OutputInfo {
        let mode = OutputMode {
            width,
            height,
            refresh_mhz: 60_000,
        };
        OutputInfo {
            connector: connector.into(),
            geometry: tessera_desktop::output::OutputGeometry {
                mode,
                scale: tessera_desktop::output::Scale::IDENTITY,
                transform: tessera_primitives::Transform::Normal,
                logical_origin: Point { x, y },
                logical_size_override: None,
            },
            kind: tessera_desktop::output::OutputKind::External,
            available_modes: vec![mode],
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
            role: tessera_desktop::output::OutputRole::Extend,
            role_source: tessera_desktop::output::OutputRoleSource::Auto,
        }
    }

    #[test]
    fn editor_tracks_connector_mode_and_extended_layout() {
        let mut module = DisplayModule::new();
        module.update_settings(&SettingsSnapshot {
            display: DisplayStatus {
                configurable: true,
                topology: tessera_desktop::output::DisplayTopology::Auto,
                outputs: vec![
                    output("eDP-1", 0, 0, 1920, 1080),
                    output("DP-1", 1920, 0, 2560, 1440),
                ],
                error: None,
                pending_revert_seconds: None,
            },
            ..SettingsSnapshot::default()
        });
        module.sync_editor(Some("DP-1"));
        assert_eq!(module.output_index, 1);
        assert_eq!(module.mode_index, 0);
        assert_eq!(module.layout, LAYOUT_RIGHT);
        assert!(!module.primary);
        assert_eq!(module.x.as_str(), "1920");
        assert_eq!(module.y.as_str(), "0");
    }

    #[test]
    fn editor_shows_automatic_for_an_auto_resolved_mirror() {
        let mut module = DisplayModule::new();
        let mut mirror = output("HDMI-A-1", 0, 0, 1280, 720);
        mirror.role = OutputRole::Mirror {
            source: "eDP-1".into(),
        };
        mirror.role_source = tessera_desktop::output::OutputRoleSource::Auto;
        mirror.geometry.logical_size_override = Some(tessera_primitives::Size { w: 1920, h: 1080 });
        module.update_settings(&SettingsSnapshot {
            display: DisplayStatus {
                configurable: true,
                topology: tessera_desktop::output::DisplayTopology::Auto,
                outputs: vec![output("eDP-1", 0, 0, 1920, 1080), mirror],
                error: None,
                pending_revert_seconds: None,
            },
            ..SettingsSnapshot::default()
        });
        module.sync_editor(Some("HDMI-A-1"));
        assert_eq!(module.role, ROLE_AUTO);
        assert_eq!(module.mirror_source_index, 0);
    }

    #[test]
    fn editor_syncs_a_mirror_role_and_its_source() {
        let mut module = DisplayModule::new();
        let mut mirror = output("HDMI-A-1", 0, 0, 1280, 720);
        mirror.role = OutputRole::Mirror {
            source: "DP-1".into(),
        };
        mirror.role_source = tessera_desktop::output::OutputRoleSource::Explicit;
        mirror.geometry.logical_size_override = Some(tessera_primitives::Size { w: 1920, h: 1080 });
        module.update_settings(&SettingsSnapshot {
            display: DisplayStatus {
                configurable: true,
                topology: tessera_desktop::output::DisplayTopology::Auto,
                outputs: vec![
                    output("eDP-1", 0, 0, 1920, 1080),
                    output("DP-1", 1920, 0, 1920, 1080),
                    mirror,
                ],
                error: None,
                pending_revert_seconds: None,
            },
            ..SettingsSnapshot::default()
        });
        module.sync_editor(Some("HDMI-A-1"));
        assert_eq!(module.role, ROLE_MIRROR);
        assert_eq!(
            module.mirror_source_index, 1,
            "the index is within the filtered source list"
        );
        assert!(!module.primary);
        assert_eq!(module.status.outputs[2].geometry.logical_size().w, 1920);
    }

    #[test]
    fn relative_layout_uses_edited_mode_and_scale() {
        let mut module = DisplayModule::new();
        let primary = output("eDP-1", 0, 0, 1920, 1080);
        let secondary = output("DP-1", 1920, 0, 2560, 1440);
        module.status.outputs = vec![primary, secondary.clone()];
        module.layout = LAYOUT_LEFT;
        module.scale = 2.0;
        assert_eq!(
            module.position(
                &secondary,
                OutputMode {
                    width: 2560,
                    height: 1440,
                    refresh_mhz: 144_000,
                },
            ),
            Some(Point { x: -1280, y: 0 })
        );
    }
}
