//! Sound page: which device playback goes to, and which microphone captures
//! (ADR-0215, ADR-0216).
//!
//! The sound server already picks sensible defaults — it moves to a headset when
//! one connects — but "sensible for me" is not "sensible right now". A user
//! demonstrating something to the room wants the built-in speakers while a
//! Bluetooth headset is still connected; a user on a call wants the headset
//! microphone even though the webcam's array has a higher priority. In both
//! cases the only way to say so is to name the device.
//!
//! This page renders the server's own inventories verbatim
//! ([`SystemStatus::audio_outputs`], [`SystemStatus::audio_inputs`]): it never
//! enumerates hardware, guesses a transport, or remembers a preference. Every
//! row is one [`SystemAction::SetAudioOutput`] or
//! [`SystemAction::SetAudioInput`] naming the server's node name; the default
//! marker then moves where the *server* says it moved. A picked device that the
//! server refuses leaves the marker where it was, and the next snapshot shows
//! that — chrome does not optimistically claim the switch.
//!
//! The input half additionally carries a **capture indicator**: when any client
//! holds a live link from a microphone, that is reported here, in the page and
//! in the microphone fader's own label. It is drawn from
//! [`SystemStatus::microphone_active`], an observation of the server's link
//! graph, so it is true for a browser tab or an agent session alike — chrome
//! never infers it from its own bookkeeping.

use super::*;
use tessera_desktop::system::{AudioInputKind, AudioOutputKind};

impl ControlCenter {
    /// The Sound page: live capture state, the input fader, then both device
    /// lists.
    pub(crate) fn render_audio_detail_view(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();

        f.set_theme(themes::hud(&hud));
        f.place(
            "tessera-hud-audio-detail",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        gap: 12.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        self.render_audio_detail_header(f, area, i18n, &hud, type_scale);
                        // The capture indicator sits directly under the header,
                        // above every control, because it is the one reading on
                        // this page the user did not ask for and must not have
                        // to hunt for (ADR-0216).
                        if status.microphone_active {
                            render_capture_banner(
                                f,
                                area.w,
                                i18n,
                                &hud,
                                type_scale,
                                self.design.colors.critical,
                            );
                        }
                        // The scroll body keeps the remaining height: the header
                        // (36px) and its 12px gap, plus the banner's 30px and its
                        // gap when one is drawn. Both are flow elements, so the
                        // body cannot overlap them.
                        let header_h = 36.0;
                        let gap = 12.0;
                        let reserved = header_h
                            + gap
                            + if status.microphone_active {
                                CAPTURE_BANNER_H + gap
                            } else {
                                0.0
                            };
                        let body_h = (area.h - reserved).max(1.0);
                        f.scroll("tessera-audio-scroll", |f| {
                            f.column_ex(
                                &LayoutOpts {
                                    width: area.w,
                                    height: body_h,
                                    gap: 10.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |f| {
                                    self.render_microphone_fader(
                                        f, area, i18n, &hud, type_scale, &status, out,
                                    );
                                    render_section(
                                        f,
                                        area.w,
                                        i18n.text(Message::Output),
                                        type_scale,
                                        &hud,
                                    );
                                    render_output_rows(
                                        f, area, i18n, &hud, type_scale, &status, out,
                                    );
                                    // The input list is only worth a heading of
                                    // its own when there is a choice to make;
                                    // with one microphone the fader above
                                    // already names it.
                                    if status.has_audio_input_choice() {
                                        render_section(
                                            f,
                                            area.w,
                                            i18n.text(Message::Inputs),
                                            type_scale,
                                            &hud,
                                        );
                                        render_input_rows(
                                            f, area, i18n, &hud, type_scale, &status, out,
                                        );
                                    }
                                },
                            );
                        });
                    },
                );
            },
        );
        f.set_theme(original);
    }

    fn render_audio_detail_header(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        hud: &ControlCenterColors,
        type_scale: TypeScale,
    ) {
        let hud = *hud;
        // Header row: [< Back] Title
        f.row_ex(
            &LayoutOpts {
                width: area.w,
                height: 36.0,
                cross: Align::Center,
                gap: 8.0,
                ..Default::default()
            },
            |f| {
                let (back_resp, _) = f.pressable_row(
                    "tessera-audio-back-btn",
                    i18n.text(Message::Back),
                    &LayoutOpts {
                        width: 80.0,
                        height: 36.0,
                        radius: 10.0,
                        pad: 6.0,
                        gap: 4.0,
                        bg: hud.surface_recessed,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f, _| {
                        f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_CHEVRON_LEFT, 16.0);
                        display_label(f, i18n.text(Message::Back), type_scale.body);
                    },
                );
                if back_resp.clicked {
                    self.audio_expanded = false;
                }
                f.spacer(4.0);
                display_label(f, i18n.text(Message::Sound), type_scale.headline);
            },
        );
    }

    /// The capture fader: level, mute, and the microphone currently in use.
    ///
    /// Muting is its own pressable icon rather than a shared control with the
    /// playback fader, because silencing a microphone is a privacy action and
    /// must never be one misclick away from silencing playback (ADR-0216).
    fn render_microphone_fader(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        hud: &ControlCenterColors,
        type_scale: TypeScale,
        status: &SystemStatus,
        out: &mut ChromeEvents,
    ) {
        if status.audio_inputs.is_empty() && status.microphone_volume.is_none() {
            // No server answered. Saying nothing is honest: the output section
            // below reports the same absence, and a microphone fader pinned to
            // zero would claim the machine has no microphone.
            return;
        }
        let default_name = status
            .default_audio_input()
            .map(|input| input.description.clone())
            .unwrap_or_else(|| i18n.text(Message::Microphone).to_owned());
        let microphone = render_horizontal_fader(
            f,
            "tessera-hud-audio-mic",
            &default_name,
            super::super::rendering::microphone_icon_raw(status),
            true,
            status.microphone_volume,
            (0, 100),
            (area.w, 72.0),
            if status.microphone_muted {
                hud.text_muted
            } else {
                hud.accent
            },
            *hud,
            type_scale,
            None,
        );
        if microphone.icon_clicked {
            out.system_actions.push(SystemAction::ToggleMicrophoneMute);
        }
        if let Some(level) = microphone.level {
            out.system_actions
                .push(SystemAction::SetMicrophoneVolume { level });
        }
    }
}

/// The capture banner's height, stated once so the scroll body's reservation and
/// the painted band cannot disagree.
const CAPTURE_BANNER_H: f32 = 30.0;

/// The one-line capture banner: a marker that the machine is listening.
///
/// Non-interactive by construction. An indicator a click can dismiss is a
/// promise the desktop cannot keep, and this one reports the server's link
/// graph, not a preference (ADR-0216). It is a flow row rather than a placed
/// surface, so the list beneath it cannot be painted over.
fn render_capture_banner(
    f: &mut Frame,
    width: f32,
    i18n: &Localizer,
    hud: &ControlCenterColors,
    type_scale: TypeScale,
    emphasis: Color,
) {
    f.set_theme(themes::hud(hud).with_fg(emphasis));
    f.row_ex(
        &LayoutOpts {
            width,
            height: CAPTURE_BANNER_H,
            radius: 10.0,
            bg: hud.surface_recessed,
            border: hud.border,
            border_width: 1.0,
            pad: 8.0,
            cross: Align::Center,
            gap: 6.0,
            ..Default::default()
        },
        |f| {
            f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_MIC, 16.0);
            display_label(f, i18n.text(Message::MicrophoneInUse), type_scale.footnote);
        },
    );
}

/// One section heading inside the Sound page.
fn render_section(
    f: &mut Frame,
    width: f32,
    title: &str,
    type_scale: TypeScale,
    hud: &ControlCenterColors,
) {
    f.set_theme(themes::hud(hud).with_fg(hud.text_muted));
    f.row_ex(
        &LayoutOpts {
            width,
            height: 20.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            display_label(f, title, type_scale.footnote);
        },
    );
}

/// The playback-device rows (ADR-0215).
fn render_output_rows(
    f: &mut Frame,
    area: Rect,
    i18n: &Localizer,
    hud: &ControlCenterColors,
    type_scale: TypeScale,
    status: &SystemStatus,
    out: &mut ChromeEvents,
) {
    if status.audio_outputs.is_empty() {
        // No inventory is not "no hardware": it means no sound server answered.
        // Saying so keeps the empty page honest instead of implying the machine
        // has no speakers.
        render_empty_note(f, area, i18n, hud, type_scale);
        return;
    }
    f.column_ex(
        &LayoutOpts {
            width: area.w,
            gap: 8.0,
            cross: Align::Stretch,
            ..Default::default()
        },
        |f| {
            for (idx, output) in status.audio_outputs.iter().enumerate() {
                let is_default = output.is_default;
                let (row, _) = f.pressable_row(
                    &format!("tessera-audio-output-{idx}"),
                    &output.description,
                    &LayoutOpts {
                        width: area.w,
                        height: 44.0,
                        pad: 8.0,
                        radius: 12.0,
                        bg: hud.surface_recessed,
                        cross: Align::Center,
                        gap: 8.0,
                        ..Default::default()
                    },
                    |f, _| {
                        f.set_theme(themes::hud(hud).with_fg(if is_default {
                            hud.accent
                        } else {
                            hud.text
                        }));
                        f.icon_raw(audio_output_icon(output.kind), 18.0);
                        f.set_theme(themes::hud(hud).with_fg(hud.text));
                        display_label(f, &output.description, type_scale.body);
                        f.flex(1.0);
                        f.spacer(0.0);
                        if is_default {
                            // The marker is the server's statement, not a
                            // cached click.
                            f.set_theme(themes::hud(hud).with_fg(hud.accent));
                            f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_CHECK, 16.0);
                        }
                    },
                );
                if row.clicked
                    && let Some(action) = ControlCenter::audio_output_intent(output)
                {
                    out.system_actions.push(action);
                }
            }
        },
    );
}

/// The capture-device rows (ADR-0216).
fn render_input_rows(
    f: &mut Frame,
    area: Rect,
    i18n: &Localizer,
    hud: &ControlCenterColors,
    type_scale: TypeScale,
    status: &SystemStatus,
    out: &mut ChromeEvents,
) {
    if status.audio_inputs.is_empty() {
        render_empty_note(f, area, i18n, hud, type_scale);
        return;
    }
    f.column_ex(
        &LayoutOpts {
            width: area.w,
            gap: 8.0,
            cross: Align::Stretch,
            ..Default::default()
        },
        |f| {
            for (idx, input) in status.audio_inputs.iter().enumerate() {
                let is_default = input.is_default;
                let (row, _) = f.pressable_row(
                    &format!("tessera-audio-input-{idx}"),
                    &input.description,
                    &LayoutOpts {
                        width: area.w,
                        height: 44.0,
                        pad: 8.0,
                        radius: 12.0,
                        bg: hud.surface_recessed,
                        cross: Align::Center,
                        gap: 8.0,
                        ..Default::default()
                    },
                    |f, _| {
                        f.set_theme(themes::hud(hud).with_fg(if is_default {
                            hud.accent
                        } else {
                            hud.text
                        }));
                        f.icon_raw(audio_input_icon(input.kind), 18.0);
                        f.set_theme(themes::hud(hud).with_fg(hud.text));
                        display_label(f, &input.description, type_scale.body);
                        f.flex(1.0);
                        f.spacer(0.0);
                        if is_default {
                            f.set_theme(themes::hud(hud).with_fg(hud.accent));
                            f.icon_raw(lens::sys::lens_icon_id::LENS_ICON_CHECK, 16.0);
                        }
                    },
                );
                if row.clicked
                    && let Some(action) = ControlCenter::audio_input_intent(input)
                {
                    out.system_actions.push(action);
                }
            }
        },
    );
}

/// The single "no sound server answered" note, shared by both lists so they
/// cannot word the same absence two different ways.
fn render_empty_note(
    f: &mut Frame,
    area: Rect,
    i18n: &Localizer,
    hud: &ControlCenterColors,
    type_scale: TypeScale,
) {
    f.set_theme(themes::hud(hud).with_fg(hud.text_muted));
    f.column_ex(
        &LayoutOpts {
            width: area.w,
            height: 44.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            f.flex(1.0);
            f.spacer(0.0);
            display_label(f, i18n.text(Message::Unavailable), type_scale.body);
            f.flex(1.0);
            f.spacer(0.0);
        },
    );
}

/// The glyph for one output transport. Purely presentational: the transport is
/// already decided in `tessera-desktop`, and an unrecognized one still gets a
/// speaker rather than no icon at all.
fn audio_output_icon(kind: AudioOutputKind) -> lens::sys::lens_icon_id {
    use lens::sys::lens_icon_id;
    match kind {
        AudioOutputKind::Bluetooth => lens_icon_id::LENS_ICON_BLUETOOTH,
        AudioOutputKind::Wired => lens_icon_id::LENS_ICON_HEADPHONES,
        AudioOutputKind::Hdmi => lens_icon_id::LENS_ICON_MONITOR,
        AudioOutputKind::Usb | AudioOutputKind::Speaker | AudioOutputKind::Other => {
            lens_icon_id::LENS_ICON_SPEAKER
        }
    }
}

/// The glyph for one input transport, by the same discipline as
/// [`audio_output_icon`]: the transport is already decided, and an
/// unrecognized one still reads as a microphone.
fn audio_input_icon(kind: AudioInputKind) -> lens::sys::lens_icon_id {
    use lens::sys::lens_icon_id;
    match kind {
        AudioInputKind::Bluetooth => lens_icon_id::LENS_ICON_BLUETOOTH,
        AudioInputKind::Headset => lens_icon_id::LENS_ICON_HEADPHONES,
        AudioInputKind::BuiltIn | AudioInputKind::Usb | AudioInputKind::Other => {
            lens_icon_id::LENS_ICON_MIC
        }
    }
}
