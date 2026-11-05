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

/// The description of the output that became default between two snapshots, when
/// one did (ADR-0215).
///
/// Only a *transition* counts. The first snapshot after a device appears, or a
/// snapshot that merely re-lists the same default, is not a switch and raises no
/// reading. This is also why the reading lives on the observation path rather
/// than on dispatch: it can only be computed once the machine has answered.
pub(super) fn audio_output_default_change(
    before: &tessera_desktop::system::SystemStatus,
    after: &tessera_desktop::system::SystemStatus,
) -> Option<String> {
    let current = after
        .audio_outputs
        .iter()
        .find(|output| output.is_default)?;
    let previous = before.audio_outputs.iter().find(|output| output.is_default);
    // A first observation (no previous default at all) is not a user-visible
    // switch: nothing moved, the list simply arrived.
    let previous = previous?;
    (previous.name != current.name).then(|| current.description.clone())
}

/// Whether `name` may be forwarded to the sound server, given the last observed
/// inventory of either direction (ADR-0215, ADR-0216).
///
/// Split from the action handler so the rule is asserted against the code the
/// runtime actually runs. The rule exists because the sound server does *not*
/// reject an unresolvable name: `pw-metadata` exits 0, and WirePlumber persists
/// the garbage as the stored default, evicting the user's real preference. Since
/// the write cannot be validated after the fact, it is validated before the
/// spawn.
///
/// Generic over the direction so sinks and sources obey one rule: an empty
/// inventory means no server answered, and a name outside the inventory is
/// refused whether it names an output or an input.
pub(super) fn audio_node_observed<T: tessera_desktop::system::AudioNode>(
    inventory: &[T],
    name: &str,
) -> bool {
    !inventory.is_empty() && inventory.iter().any(|node| node.node_name() == name)
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
    casting: &crate::casting::CastHandle,
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
                // A level write ends the mute — on the live backend it is the
                // second half of the same intent (`set_default_volume`), because
                // the server does not clear it on its own (measured). The mock
                // mirrors the whole contract, so this assignment is not an
                // assumption about the machine: it restates what the backend
                // just did, and the next probe would correct it if the backend
                // had not.
                if status.muted {
                    status.muted = false;
                }
            }
        }
        SystemAction::SetAudioOutput { name } => {
            // Forward the intent; the server owns routing. The default marker in
            // `status` moves on the next probe, never here — a stream may pin
            // its own target, so "the default moved" is the server's statement
            // to make (ADR-0215).
            //
            // The name must be one the last probe observed. Not because a
            // just-connected device is illegitimate, but because an
            // unresolvable name is not rejected by the sound server: the write
            // succeeds, and WirePlumber *persists* it as the stored default,
            // evicting the user's real preference (`default-nodes` state,
            // `[INV-AUDIO-2]`). A retryable refusal after the next probe is the
            // honest failure; silently discarding the saved default is not.
            if status.audio_outputs.is_empty() {
                return Err("no audio outputs: no sound server answered".into());
            }
            if !audio_node_observed(&status.audio_outputs, &name) {
                return Err(format!(
                    "no audio output named {name}; it may have just appeared, retry after the next status probe"
                ));
            }
            host_system.set_audio_output(&name)?;
        }
        SystemAction::SetAudioInput { name } => {
            // The input-side counterpart of `SetAudioOutput`, and refused for
            // exactly the same reason: the write is name-addressed and the
            // server accepts a name it cannot resolve, which WirePlumber then
            // persists as the stored capture default (ADR-0216).
            if status.audio_inputs.is_empty() {
                return Err("no audio inputs: no sound server answered".into());
            }
            if !audio_node_observed(&status.audio_inputs, &name) {
                return Err(format!(
                    "no audio input named {name}; it may have just appeared, retry after the next status probe"
                ));
            }
            host_system.set_audio_input(&name)?;
        }
        SystemAction::SetMicrophoneVolume { level } => {
            if status.microphone_volume != Some(level) {
                host_system.set_microphone_volume(level)?;
                status.microphone_volume = Some(level);
                // The same contract as `SetVolume`: the backend clears the mute
                // as part of the write, because the server does not (ADR-0216).
                status.microphone_muted = false;
            }
        }
        SystemAction::ToggleMicrophoneMute => {
            if status.audio_inputs.is_empty() && status.microphone_volume.is_none() {
                return Err("no microphone: no sound server answered".into());
            }
            host_system.toggle_microphone_mute()?;
            status.microphone_muted = !status.microphone_muted;
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
        SystemAction::ScanCast => {
            // The service owns discovery; chrome renders the snapshot it pushes
            // (merged in `iteration.rs`). No iwd read and no state mutation here.
            casting.start_discovery();
        }
        SystemAction::StopCastDiscovery => {
            casting.stop_discovery();
        }
        SystemAction::ConnectCast { sink } => {
            casting.connect(sink);
        }
        SystemAction::DisconnectCast { session } => {
            casting.disconnect(session);
        }
        SystemAction::SetDoNotDisturb { enabled } => {
            notifications
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_do_not_disturb(enabled);
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

/// The reading an applied action is worth, if any.
///
/// Returns `None` for actions whose effect the user already sees (a window
/// command, a session action) and for actions that report their own outcome
/// (a refused request). Level changes and link changes have no other surface, so
/// they report here.
///
/// The current level is read from `status` *after* the action applied, so the
/// reading states the value the machine reached rather than the one it was asked
/// for.
pub(super) fn osd_for_action(
    action: &tessera_protocol::SystemAction,
    status: &tessera_desktop::system::SystemStatus,
    localizer: tessera_shell::Localizer,
) -> Option<tessera_shell::component::OsdParams> {
    use tessera_protocol::SystemAction;
    use tessera_shell::component::{OsdKind, OsdParams};

    // A mute is a change the user cannot see, so it reports too — as a zero
    // level rather than a separate kind.
    let volume_reading = |status: &tessera_desktop::system::SystemStatus| OsdParams {
        kind: OsdKind::Volume,
        title: if status.muted {
            localizer.muted_volume(status.volume.unwrap_or(0))
        } else {
            localizer.volume_level(status.volume.unwrap_or(0))
        },
        detail: None,
        level: Some(if status.muted {
            0
        } else {
            status.volume.unwrap_or(0)
        }),
        critical: false,
    };

    match action {
        SystemAction::SetVolume { .. }
        | SystemAction::StepVolume { .. }
        | SystemAction::ToggleMute => Some(volume_reading(status)),
        SystemAction::SetBrightness { .. } => status.brightness.map(|level| OsdParams {
            kind: OsdKind::Brightness,
            title: localizer.brightness_level(level),
            detail: None,
            level: Some(level),
            critical: false,
        }),
        SystemAction::SetWifi { enabled } => Some(OsdParams {
            kind: OsdKind::Wireless,
            title: if *enabled {
                localizer.wireless_enabled()
            } else {
                localizer.wireless_disabled()
            },
            detail: None,
            level: None,
            critical: false,
        }),
        SystemAction::SetBluetooth { enabled } => Some(OsdParams {
            kind: OsdKind::Bluetooth,
            title: if *enabled {
                localizer.bluetooth_enabled()
            } else {
                localizer.bluetooth_disabled()
            },
            detail: None,
            level: None,
            critical: false,
        }),
        // The output picker sits inside the panel, and the panel's own marker is
        // the acknowledgement: it moves only when the sound server reports the
        // new default. A reading raised here would fire *before* that — the
        // write is a spawn the compositor does not wait on, and the server may
        // still refuse to move (a pinned stream, a device that left range). The
        // reading is raised by the probe-driven transition instead
        // (`CompositorRuntime::apply_detected_status`), which speaks only after
        // observing the machine (ADR-0215).
        SystemAction::SetAudioOutput { .. } | SystemAction::SetAudioInput { .. } => None,
        // The microphone fader and its mute sit inside the panel too, and their
        // acknowledgement is the fader itself: the level the user dragged, and
        // the glyph they pressed. A transient corner reading for a control still
        // under the pointer would be noise, and it would also announce a mute
        // the server had not applied yet (ADR-0216).
        SystemAction::SetMicrophoneVolume { .. } | SystemAction::ToggleMicrophoneMute => None,
        _ => None,
    }
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

    #[test]
    fn an_output_name_the_last_probe_did_not_see_is_refused() {
        // The sound server does not reject an unresolvable name: `pw-metadata`
        // exits 0 and WirePlumber persists it as the stored default, evicting
        // the user's real preference. The write cannot be validated afterwards,
        // so it is validated before the spawn.
        let outputs = vec![tessera_desktop::system::AudioOutput {
            name: "alsa_output.pci-speaker".into(),
            description: "Built-in Speaker".into(),
            kind: tessera_desktop::system::AudioOutputKind::Speaker,
            is_default: true,
        }];
        assert!(audio_node_observed(&outputs, "alsa_output.pci-speaker"));
        assert!(
            !audio_node_observed(&outputs, "bogus_node_xyz"),
            "an unlisted name would overwrite the persisted default with garbage"
        );
        // No inventory means no sound server answered, so no name can resolve.
        assert!(
            !audio_node_observed::<tessera_desktop::system::AudioOutput>(
                &[],
                "alsa_output.pci-speaker"
            )
        );
        // A node name is matched exactly: a prefix or a case variant is a
        // different node, and a near-miss is how a silent wrong-device switch
        // would happen.
        assert!(!audio_node_observed(&outputs, "alsa_output.pci-speake"));
        assert!(!audio_node_observed(&outputs, "ALSA_OUTPUT.PCI-SPEAKER"));
    }

    #[test]
    fn an_input_name_the_last_probe_did_not_see_is_refused() {
        // The input half of the same defense (ADR-0216). The sound server
        // accepts an unresolvable capture default just as it accepts an
        // unresolvable playback one, and WirePlumber persists it, so the name is
        // validated against the last observation before the write.
        let inputs = vec![tessera_desktop::system::AudioInput {
            name: "alsa_input.pci-builtin".into(),
            description: "Built-in Microphone".into(),
            kind: tessera_desktop::system::AudioInputKind::BuiltIn,
            is_default: true,
        }];
        assert!(audio_node_observed(&inputs, "alsa_input.pci-builtin"));
        assert!(!audio_node_observed(&inputs, "alsa_input.pci-other"));
        assert!(!audio_node_observed::<tessera_desktop::system::AudioInput>(
            &[],
            "alsa_input.pci-builtin"
        ));
    }

    #[test]
    fn dispatching_a_microphone_change_reports_nothing() {
        // The fader and its glyph are the acknowledgement, and both are still
        // under the pointer; a corner reading would be noise and would announce
        // a state the server had not applied yet (ADR-0216).
        let status = tessera_desktop::system::SystemStatus {
            microphone_volume: Some(60),
            ..tessera_desktop::system::SystemStatus::default()
        };
        for action in [
            SystemAction::ToggleMicrophoneMute,
            SystemAction::SetMicrophoneVolume { level: 30 },
            SystemAction::SetAudioInput {
                name: "alsa_input.pci-builtin".into(),
            },
        ] {
            assert!(
                osd_for_action(&action, &status, tessera_shell::Localizer::default()).is_none(),
                "{action:?} must not raise a reading"
            );
        }
    }

    #[test]
    fn dispatching_an_output_switch_reports_nothing() {
        // The write is a spawn the compositor does not wait on, and the sound
        // server refuses some moves outright. A reading raised at dispatch would
        // therefore announce a move that may not happen (ADR-0215).
        let status = tessera_desktop::system::SystemStatus {
            audio_outputs: vec![tessera_desktop::system::AudioOutput {
                name: "alsa_output.pci-hdmi2".into(),
                description: "Living Room TV".into(),
                kind: tessera_desktop::system::AudioOutputKind::Hdmi,
                is_default: false,
            }],
            ..tessera_desktop::system::SystemStatus::default()
        };
        assert!(
            osd_for_action(
                &SystemAction::SetAudioOutput {
                    name: "alsa_output.pci-hdmi2".into(),
                },
                &status,
                tessera_shell::Localizer::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn a_switch_is_reported_only_once_the_marker_has_moved() {
        // The reading is computed from two observations, so it speaks only after
        // the machine has answered. It names the output in the server's own
        // words: a node name such as `alsa_output.pci-…HiFi__HDMI2__sink` is an
        // implementation identity, not something to show a person.
        let speaker = |is_default| tessera_desktop::system::AudioOutput {
            name: "alsa_output.pci-speaker".into(),
            description: "Built-in Speaker".into(),
            kind: tessera_desktop::system::AudioOutputKind::Speaker,
            is_default,
        };
        let tv = |is_default| tessera_desktop::system::AudioOutput {
            name: "alsa_output.pci-hdmi2".into(),
            description: "Living Room TV".into(),
            kind: tessera_desktop::system::AudioOutputKind::Hdmi,
            is_default,
        };
        let snapshot = |outputs: Vec<tessera_desktop::system::AudioOutput>| {
            tessera_desktop::system::SystemStatus {
                audio_outputs: outputs,
                ..tessera_desktop::system::SystemStatus::default()
            }
        };

        let before = snapshot(vec![speaker(true), tv(false)]);
        assert_eq!(
            audio_output_default_change(&before, &snapshot(vec![speaker(false), tv(true)])),
            Some("Living Room TV".into())
        );
        // Re-listing the same default is not a switch.
        assert_eq!(
            audio_output_default_change(&before, &snapshot(vec![speaker(true), tv(false)])),
            None
        );
        // The first observation of a list is not a switch either: nothing moved,
        // the list simply arrived.
        let empty = snapshot(Vec::new());
        assert_eq!(
            audio_output_default_change(&empty, &snapshot(vec![speaker(false), tv(true)])),
            None
        );
        // And a list with no default marker at all reports nothing.
        assert_eq!(
            audio_output_default_change(&before, &snapshot(vec![speaker(false), tv(false)])),
            None
        );
    }
}
