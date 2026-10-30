//! Control-center presentation, split by surface.
//!
//! Each page painter lives in its own submodule; this module keeps the
//! shared canvas, tooltip, cluster chrome, and the re-exports the rest of
//! the component resolves through.

pub(crate) use super::*;

pub(crate) use lens::{Band, Icon};
pub(crate) use tessera_design::materials::{chrome_place, sized, transparent};

mod bluetooth;
mod canvas;
mod cast;
pub(super) mod clock;
mod media;
mod notifications;
mod overview;
pub(super) mod power;
mod profile;
mod quick;
mod tray_panel;
mod wifi;
mod work_mode;

pub(crate) use canvas::canvas_alpha;

// ---- rendering -----------------------------------------------------------

impl ControlCenter {
    /// The Bluetooth tile's subtitle: the connected peripheral's name when a
    /// link is up, otherwise the radio state (ADR-0175).
    /// The Cast tile's subtitle: the live casting/extended-display state read
    /// from the compositor-published status fields (`[INV-ARCH-80]`,
    /// ADR-0188). Casting streams win over a merely-present extended display;
    /// an idle desktop reads as not casting.
    pub(super) fn cast_tile_subtitle(status: &SystemStatus, i18n: &Localizer) -> String {
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

/// Render a small frameless tooltip pill with `reveal` opacity over the
/// anchor's top edge. Purely presentational; hover state is caller-owned.
fn render_tooltip(
    f: &mut Frame,
    id: &str,
    anchor: Rect,
    text: &str,
    hud: ControlCenterColors,
    type_scale: TypeScale,
    reveal: f32,
) {
    let original_opacity = f.opacity();
    f.set_opacity(reveal * original_opacity);
    f.place(
        id,
        &chrome_place(
            anchor,
            LayoutOpts {
                radius: 8.0,
                bg: hud.surface_recessed,
                border: hud.border,
                border_width: 1.0,
                ..Default::default()
            },
        ),
        |f| {
            f.row_ex(
                &LayoutOpts {
                    width: anchor.w,
                    height: anchor.h,
                    cross: Align::Center,
                    pad: 6.0,
                    ..Default::default()
                },
                |f| {
                    display_label(f, text, type_scale.footnote);
                },
            );
        },
    );
    f.set_opacity(original_opacity);
}
