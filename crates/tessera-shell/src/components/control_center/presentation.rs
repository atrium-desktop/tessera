//! Control Center presentation, split by surface.
//!
//! Each surface lives in its own submodule; this module keeps the helpers they
//! share and the tooltip every icon-only control uses.

pub(crate) use super::*;

pub(crate) use optics_ui::Icon;
pub(crate) use tessera_design::materials::{chrome_place, sized, transparent};

mod audio;
mod bluetooth;
mod cast;
pub(super) mod controls;
mod header;
mod media;
pub(super) mod notifications;
mod tray_panel;
mod wifi;
mod work_mode;

// ---- rendering -----------------------------------------------------------

impl ControlCenter {
    /// The Cast tile's subtitle: the live casting/extended-display state read
    /// from the compositor-published status fields (`[INV-ARCH-80]`,
    /// ADR-0188). Casting streams win over a merely-present extended display;
    /// an idle desktop reads as not casting.
    pub(super) fn cast_tile_subtitle(status: &SystemStatus, i18n: &Localizer) -> String {
        use tessera_desktop::system::CastSessionState;
        if let Some(session) = status.cast.active_session() {
            let state = match session.state {
                CastSessionState::Connecting => i18n.text(Message::Casting),
                CastSessionState::Negotiating => i18n.text(Message::Casting),
                CastSessionState::Streaming => i18n.text(Message::Casting),
                CastSessionState::Stopping => i18n.text(Message::NotCasting),
                CastSessionState::Failed => i18n.text(Message::NotCasting),
            };
            return format!("{state} · {}", session.sink_name);
        }
        if status.cast_streams > 0 {
            format!("{} · {}", i18n.text(Message::Casting), status.cast_streams)
        } else if status.virtual_displays > 0 {
            format!(
                "{} · {}",
                i18n.text(Message::Displays),
                status.virtual_displays
            )
        } else {
            i18n.text(Message::NotCasting).to_owned()
        }
    }

    /// Whether the sound fader offers the output picker (ADR-0215).
    ///
    /// The disclosure appears only when there is a choice to make. With no
    /// outputs there is nothing to list (no sound server answered), and with
    /// one the page would show the device already playing — a control that
    /// cannot change anything reads as broken rather than as unnecessary.
    ///
    /// A second reason to open the page is a microphone that can be muted or an
    /// input that can be chosen (ADR-0216): the disclosure is the only way into
    /// the capture controls, so it must appear when they are actionable even
    /// with a single output.
    pub(super) fn audio_output_disclosure_visible(status: &SystemStatus) -> bool {
        status.audio_outputs.len() > 1
            || status.has_audio_input_choice()
            || status.microphone_volume.is_some()
    }

    /// The intent a click on one output row produces, or `None` when the click
    /// cannot change anything (ADR-0215).
    ///
    /// Selecting the row that is already default is not a no-op the server
    /// should be asked to perform: it would rewrite the same key and raise a
    /// reading for a state that did not change.
    pub(super) fn audio_output_intent(
        output: &tessera_desktop::system::AudioOutput,
    ) -> Option<SystemAction> {
        if output.is_default {
            return None;
        }
        Some(SystemAction::SetAudioOutput {
            name: output.name.clone(),
        })
    }

    /// The intent a click on one input row produces (ADR-0216).
    ///
    /// The same rule as [`Self::audio_output_intent`], for the same reason: the
    /// server owns the default, so re-selecting the current one would be a write
    /// with no state change behind it.
    pub(super) fn audio_input_intent(
        input: &tessera_desktop::system::AudioInput,
    ) -> Option<SystemAction> {
        if input.is_default {
            return None;
        }
        Some(SystemAction::SetAudioInput {
            name: input.name.clone(),
        })
    }

    pub(super) fn bluetooth_tile_subtitle(status: &SystemStatus, i18n: &Localizer) -> String {
        // A live link names the device; an off radio never does, even if a
        // stale `connected` flag lingers host-side mid-disconnect.
        if let Some(device) = status.bluetooth_devices.iter().find(|d| d.connected)
            && status.bluetooth_enabled == Some(true)
        {
            return device.name.clone();
        }
        match status.bluetooth_enabled {
            None => i18n.text(Message::Unavailable).to_owned(),
            Some(false) => i18n.text(Message::Off).to_owned(),
            Some(true) => match status.bluetooth_state {
                tessera_desktop::system::BluetoothLinkState::Scanning => "Scanning…".to_owned(),
                _ => i18n.text(Message::On).to_owned(),
            },
        }
    }

    /// Route a destructive session action through the system-level
    /// confirmation. The panel never performs power transitions itself:
    /// like the power notification paths, the request leaves through
    /// `ChromeEvents` and the compositor runtime opens the consent chrome
    /// (`StartConfirmPick`) before executing anything.
    pub(super) fn request_system_confirm(&mut self, action: SystemAction, out: &mut ChromeEvents) {
        if self.power_pending_confirm.is_some() {
            return;
        }
        self.power_pending_confirm = Some(action.clone());
        out.system_actions.push(action);
    }
}

impl ControlCenter {
    /// Record the icon control under the pointer this frame; the tooltip
    /// appears after a short dwell.
    pub(super) fn hover_tip(&mut self, tip: Tip, anchor: Rect) {
        if self.tip.map(|(current, _)| current) != Some(tip) {
            self.tip_dwell = 0.0;
        }
        self.tip = Some((tip, anchor));
    }

    /// Paint the hovered control's tooltip just below it, kept inside the
    /// controls card so the sheet's footprint covers it.
    pub(super) fn render_tip(&mut self, f: &mut Frame, i18n: &Localizer) {
        let Some((tip, anchor)) = self.tip else {
            return;
        };
        if self.tip_reveal <= 0.02 {
            return;
        }
        let text = match tip {
            Tip::Settings => i18n.text(Message::Settings),
            Tip::Lock => i18n.text(Message::LockNow),
            Tip::Power => i18n.text(Message::PowerAndSession),
            Tip::DoNotDisturb => i18n.text(Message::DoNotDisturb),
            Tip::DarkMode => i18n.text(Message::DarkMode),
            Tip::WorkMode(index) => PowerMode::ALL
                .get(index)
                .copied()
                .map_or("", |mode| i18n.text(power_mode_hint(mode))),
        };
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let width = (measure(f, text, type_scale.footnote) + 20.0).max(48.0);
        let card = self.sheet.controls;
        let x = (anchor.x + anchor.w * 0.5 - width * 0.5).clamp(
            card.x + 4.0,
            (card.x + card.w - width - 4.0).max(card.x + 4.0),
        );
        let rect = Rect {
            x,
            y: anchor.y + anchor.h + 6.0,
            w: width,
            h: 26.0,
        };
        let original_opacity = f.opacity();
        f.set_opacity(self.tip_reveal * original_opacity);
        let original = f.theme();
        f.set_theme(themes::hud(&hud));
        f.place(
            "control-center-tooltip",
            &chrome_place(
                rect,
                LayoutOpts {
                    corner_radii: optics_ui::CornerRadii::uniform(8.0),
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.centered(rect.w, rect.h, |f| {
                    display_label(f, text, type_scale.footnote);
                });
            },
        );
        f.set_theme(original);
        f.set_opacity(original_opacity);
    }
}

/// The one-line idle behavior of each work mode, shown as its tooltip.
pub(super) fn power_mode_hint(mode: PowerMode) -> Message {
    match mode {
        PowerMode::Balanced => Message::PowerModeBalancedHint,
        PowerMode::Awake => Message::PowerModeAwakeHint,
        PowerMode::Secure => Message::PowerModeSecureHint,
    }
}

fn measure(f: &mut Frame, text: &str, size: f32) -> f32 {
    f.measure_text(text, size).width
}
