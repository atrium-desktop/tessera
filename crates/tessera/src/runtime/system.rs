use super::*;
use crate::host_system::HostSystem;

/// Publish one normalized live-system snapshot to in-process chrome and IPC.
pub(super) fn publish_system_status_parts(
    status: &tessera_desktop::system::SystemStatus,
    shell: &mut tessera_shell::Shell,
    live: &std::sync::Arc<LiveState>,
    ipc: &Option<tessera_ipc::Server>,
) {
    shell.set_system_status(status.clone());
    live.set_system_status(status.clone());
    if let Some(ipc) = ipc {
        ipc.broadcast(tessera_protocol::Event::SystemStatusChanged);
    }
}

/// Apply one immediate system control through the authoritative runtime path.
///
/// Host operations are delegated to [`HostSystem`], keeping the compositor main
/// loop decoupled from platform specifics or simulation environments.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_system_action(
    server: &mut tessera_wayland::Server,
    host: &mut tessera_platform::host::Host,
    notifications: &std::sync::Arc<std::sync::Mutex<tessera_desktop::notify::NotificationQueue>>,
    status: &mut tessera_desktop::system::SystemStatus,
    idle_inhibits: &mut super::idle::IdleInhibits,
    idle_process: &mut super::session::IdleProcess,
    wireless: &crate::wireless::WirelessHandle,
    bluetooth: &crate::bluetooth::BluetoothHandle,
    host_system: &dyn HostSystem,
    action: tessera_desktop::system::SystemAction,
) -> Result<(), String> {
    use tessera_desktop::system::SystemAction;

    action.validate().map_err(str::to_owned)?;
    validate_session_boundary(server.session_lock_confirmed(), &action)?;
    match action {
        SystemAction::ToggleMute => {
            host_system.toggle_mute()?;
            status.muted = !status.muted;
        }
        SystemAction::StepVolume { delta } => {
            host_system.step_volume(delta)?;
            let current = status.volume.unwrap_or(0) as i16;
            status.volume = Some((current + i16::from(delta)).clamp(0, 100) as u8);
        }
        SystemAction::SetVolume { level } => {
            if status.volume != Some(level) {
                host_system.set_volume(level)?;
                status.volume = Some(level);
                if status.muted {
                    status.muted = false;
                }
            }
        }
        SystemAction::SetBrightness { level } => {
            if status.brightness != Some(level) {
                host_system.set_brightness(level)?;
                status.brightness = Some(level);
            }
        }
        SystemAction::SetKeyboardBrightness { level } => {
            if status.kbd_brightness != Some(level) {
                host_system.set_keyboard_brightness(level)?;
                status.kbd_brightness = Some(level);
            }
        }
        SystemAction::StepKeyboardBrightness => {
            host_system.step_keyboard_brightness()?;
            // Advance to the next rung of the hardware's own ladder when it is
            // stepped; otherwise fall back to the documented default ladder.
            let ladder = status
                .kbd_brightness_tiers()
                .unwrap_or_else(|| tessera_desktop::system::KBD_BRIGHTNESS_FALLBACK_TIERS.to_vec());
            let cur = status.kbd_brightness.unwrap_or(0);
            let index = ladder
                .iter()
                .enumerate()
                .min_by_key(|(_, rung)| rung.abs_diff(cur))
                .map(|(index, _)| index)
                .unwrap_or(0);
            let next = ladder[(index + 1) % ladder.len()];
            status.kbd_brightness = Some(next);
        }
        SystemAction::SetWifi { enabled } => {
            wireless.set_enabled(enabled);
            status.wifi_enabled = Some(enabled);
            if !enabled {
                status.wifi_state = tessera_desktop::system::WifiLinkState::Disabled;
            }
        }
        SystemAction::ScanWifi => {
            wireless.request_scan();
            status.wifi_state = tessera_desktop::system::WifiLinkState::Scanning;
        }
        SystemAction::ConnectWifi { ssid, passphrase } => {
            wireless.connect(ssid, passphrase);
            status.wifi_state = tessera_desktop::system::WifiLinkState::Connecting;
        }
        SystemAction::DisconnectWifi => {
            wireless.disconnect();
            status.wifi_state = tessera_desktop::system::WifiLinkState::Disconnected;
            status.wifi_ssid = None;
        }
        SystemAction::ForgetWifi { ssid } => {
            wireless.forget(ssid.clone());
            if status.wifi_ssid.as_deref() == Some(&ssid) {
                status.wifi_state = tessera_desktop::system::WifiLinkState::Disconnected;
                status.wifi_ssid = None;
            }
            for net in &mut status.wifi_networks {
                if net.ssid == ssid {
                    net.is_saved = false;
                    net.auto_connect = false;
                    if net.is_connected {
                        net.is_connected = false;
                    }
                }
            }
        }
        SystemAction::SetWifiAutoConnect { ssid, auto_connect } => {
            wireless.set_auto_connect(ssid.clone(), auto_connect);
            for net in &mut status.wifi_networks {
                if net.ssid == ssid {
                    net.auto_connect = auto_connect;
                }
            }
        }
        SystemAction::SetBluetooth { enabled } => {
            // Parity with `SetWifi`: the write goes only through the subsystem
            // (BlueZ owns the adapter's rfkill soft-block itself), never a
            // forked host command.
            bluetooth.set_enabled(enabled);
            status.bluetooth_enabled = Some(enabled);
            if !enabled {
                status.bluetooth_state = tessera_desktop::system::BluetoothLinkState::Disabled;
                for device in &mut status.bluetooth_devices {
                    device.connected = false;
                }
            } else if status.bluetooth_state
                == tessera_desktop::system::BluetoothLinkState::Disabled
            {
                status.bluetooth_state = tessera_desktop::system::BluetoothLinkState::Idle;
            }
        }
        SystemAction::ScanBluetooth => {
            bluetooth.request_scan();
            status.bluetooth_state = tessera_desktop::system::BluetoothLinkState::Scanning;
        }
        SystemAction::PairBluetooth { address } => {
            bluetooth.pair(address);
        }
        SystemAction::ConnectBluetooth { address } => {
            bluetooth.connect(address);
        }
        SystemAction::DisconnectBluetooth { address } => {
            bluetooth.disconnect(address.clone());
            for device in &mut status.bluetooth_devices {
                if device.address == address {
                    device.connected = false;
                }
            }
            status.bluetooth_state = tessera_desktop::system::BluetoothLinkState::Idle;
        }
        SystemAction::ForgetBluetooth { address } => {
            bluetooth.forget(address.clone());
            status
                .bluetooth_devices
                .retain(|device| device.address != address);
        }
        SystemAction::SetDoNotDisturb { enabled } => {
            notifications.lock().unwrap().set_do_not_disturb(enabled);
            status.do_not_disturb = enabled;
        }
        SystemAction::SetOutputPower { powered } => {
            host.set_outputs_powered(powered)?;
        }
        SystemAction::SetIdleInhibit { inhibit } => {
            // Legacy single-bit shape (ADR-0140 maps it onto the mode):
            // "inhibit" meant "keep every stage resumed", which is exactly
            // the Awake mode; releasing returns to the balanced default.
            let mode = if inhibit {
                tessera_desktop::power::PowerMode::Awake
            } else {
                tessera_desktop::power::PowerMode::Balanced
            };
            apply_power_mode(server, status, idle_inhibits, idle_process, mode);
        }
        SystemAction::SetPowerMode { mode } => {
            apply_power_mode(server, status, idle_inhibits, idle_process, mode);
        }
        SystemAction::Suspend => {
            host_system.suspend()?;
        }
        SystemAction::Reboot => {
            host_system.reboot()?;
        }
        SystemAction::PowerOff => {
            host_system.power_off()?;
        }
    }
    Ok(())
}

fn validate_session_boundary(
    lock_confirmed: bool,
    action: &tessera_desktop::system::SystemAction,
) -> Result<(), String> {
    if matches!(
        action,
        tessera_desktop::system::SystemAction::SetOutputPower { powered: false }
    ) && !lock_confirmed
    {
        Err("output power-off requires a confirmed session lock".into())
    } else {
        Ok(())
    }
}

/// Apply a session power mode (ADR-0140).
fn apply_power_mode(
    server: &mut tessera_wayland::Server,
    status: &mut tessera_desktop::system::SystemStatus,
    idle_inhibits: &mut super::idle::IdleInhibits,
    idle_process: &mut super::session::IdleProcess,
    mode: tessera_desktop::power::PowerMode,
) {
    let session_holds_inhibitor = !mode.locks_automatically();
    let effective = idle_inhibits.set(
        super::idle::SESSION_IDLE_INHIBIT_ID,
        session_holds_inhibitor,
    );
    server.set_ipc_idle_inhibit(effective);
    idle_process.set_mode(mode);
    status.power_mode = mode;
    status.idle_inhibited = session_holds_inhibitor;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::system::SystemAction;

    #[test]
    fn output_power_requires_confirmed_secure_presentation() {
        let action = SystemAction::SetOutputPower { powered: false };
        assert!(validate_session_boundary(false, &action).is_err());
        assert!(validate_session_boundary(true, &action).is_ok());
        assert!(
            validate_session_boundary(false, &SystemAction::SetOutputPower { powered: true })
                .is_ok()
        );
        assert!(validate_session_boundary(false, &SystemAction::ToggleMute).is_ok());
    }
}
