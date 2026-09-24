//! Host probes for the backend-neutral live system model.
//!
//! Probing remains outside `tessera-desktop` because it uses Linux files and host
//! commands. Mutations remain owned by the executable.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub use tessera_desktop::settings::DisplayStatus;
pub use tessera_desktop::system::BatteryStatus;
pub use tessera_desktop::system::NetworkState;
pub use tessera_desktop::system::SystemStatus;

/// Probe live host services through standard Linux interfaces.
///
/// Every source is optional so VMs, nested sessions, and desktops without a
/// given service still receive a coherent snapshot.
pub fn detect_system_status() -> SystemStatus {
    let (volume, muted, wifi_enabled, wifi_ssid) = detect_forked_status();
    let (network, network_interface) = detect_network();
    let wifi_state = if wifi_ssid.is_some() {
        tessera_desktop::system::WifiLinkState::Connected
    } else if wifi_enabled == Some(false) {
        tessera_desktop::system::WifiLinkState::Disabled
    } else {
        tessera_desktop::system::WifiLinkState::Disconnected
    };
    SystemStatus {
        volume,
        muted,
        network,
        network_interface,
        wifi_ssid,
        battery: detect_battery(),
        wifi_enabled,
        wifi_state,
        wifi_networks: Vec::new(),
        bluetooth_enabled: detect_bluetooth_radio(),
        brightness: detect_brightness(),
        do_not_disturb: false,
        input: tessera_types::input::InputStatus::default(),
        display: DisplayStatus::default(),
        idle_inhibited: false,
        power_mode: tessera_desktop::power::PowerMode::default(),
        // Compositor-owned; the runtime patches its live value onto every
        // host sample before publishing (ADR-0128).
        capture_streams: 0,
    }
}

/// Probe every status field whose source is a cheap `/sys` read, deferring the
/// audio fork+exec probe (`wpctl get-volume`) to a separate full pass.
///
/// The status poller wakes on a short cadence to keep the HUD (battery,
/// brightness, charging, network link) fresh, but none of the external
/// commands changes on that timescale — volume moves only on user action
/// (which already triggers an out-of-cycle full probe via the refresh signal)
/// and wireless link changes stream asynchronously from `crate::wireless` (ADR-0162).
/// This light variant keeps the frequent poll off the fork
/// path entirely; `volume`, `muted`, `wifi_enabled`, and `wifi_ssid` are
/// filled in from the caller's last known values, so a snapshot built from
/// it only diverges where the sysfs-backed fields actually moved.
pub fn detect_system_status_lightweight(
    last_volume: Option<u8>,
    last_muted: bool,
    last_wifi_enabled: Option<bool>,
    last_wifi_ssid: Option<String>,
) -> SystemStatus {
    let (network, network_interface) = detect_network();
    let wifi_state = if last_wifi_ssid.is_some() {
        tessera_desktop::system::WifiLinkState::Connected
    } else if last_wifi_enabled == Some(false) {
        tessera_desktop::system::WifiLinkState::Disabled
    } else {
        tessera_desktop::system::WifiLinkState::Disconnected
    };
    SystemStatus {
        volume: last_volume,
        muted: last_muted,
        network,
        network_interface,
        wifi_ssid: last_wifi_ssid,
        battery: detect_battery(),
        wifi_enabled: last_wifi_enabled,
        wifi_state,
        wifi_networks: Vec::new(),
        bluetooth_enabled: detect_bluetooth_radio(),
        brightness: detect_brightness(),
        do_not_disturb: false,
        input: tessera_types::input::InputStatus::default(),
        display: DisplayStatus::default(),
        idle_inhibited: false,
        power_mode: tessera_desktop::power::PowerMode::default(),
        // Compositor-owned; the runtime patches its live value onto every
        // host sample before publishing (ADR-0128).
        capture_streams: 0,
    }
}

/// Run only the forked probes and return
/// `(volume, muted, wifi_enabled, wifi_ssid)`.
///
/// Wi-Fi state and SSIDs are owned by the dedicated wireless subsystem
/// (`crate::wireless`, ADR-0162); this helper only samples volume via PipeWire
/// and the kernel rfkill radio status.
pub fn detect_forked_status() -> (Option<u8>, bool, Option<bool>, Option<String>) {
    let (volume, muted) = detect_volume();
    (volume, muted, detect_wifi_radio(), None)
}

fn detect_volume() -> (Option<u8>, bool) {
    let volume_output = command_output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]);
    volume_output
        .as_deref()
        .map(|output| {
            let value = output
                .split_whitespace()
                .find_map(|part| part.parse::<f32>().ok())
                .map(|value| (value * 100.0).round().clamp(0.0, 100.0) as u8);
            (value, output.contains("MUTED"))
        })
        .unwrap_or((None, false))
}

/// The default-route interface first (the one the panel should name),
/// falling back to any live link. Returns the state plus the sysfs name.
fn detect_network() -> (NetworkState, String) {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return (NetworkState::Offline, String::new());
    };
    let mut wired: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == "lo" {
            continue;
        }
        let path = entry.path();
        let up = fs::read_to_string(path.join("operstate"))
            .map(|state| matches!(state.trim(), "up" | "unknown"))
            .unwrap_or(false);
        if !up {
            continue;
        }
        let name = name.to_string_lossy().into_owned();
        if path.join("wireless").is_dir() || name.starts_with("wl") {
            return (NetworkState::Wifi, name);
        }
        wired = wired.or(Some(name));
    }
    match wired {
        Some(name) => (NetworkState::Wired, name),
        None => (NetworkState::Offline, String::new()),
    }
}

fn detect_battery() -> Option<BatteryStatus> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("BAT")
        {
            continue;
        }
        let Some(percent) = read_u64(&entry.path().join("capacity")) else {
            continue;
        };
        let percent = percent.min(100) as u8;
        let charging = fs::read_to_string(entry.path().join("status"))
            .map(|status| status.trim().eq_ignore_ascii_case("charging"))
            .unwrap_or(false);
        return Some(BatteryStatus { percent, charging });
    }
    None
}

fn detect_wifi_radio() -> Option<bool> {
    let entries = fs::read_dir("/sys/class/rfkill").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = fs::read_to_string(path.join("type")) else {
            continue;
        };
        if kind.trim() != "wlan" {
            continue;
        }
        if let Some(state) = read_u64(&path.join("state")) {
            return Some(state != 0);
        }
    }
    None
}

fn detect_bluetooth_radio() -> Option<bool> {
    let entries = fs::read_dir("/sys/class/rfkill").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = fs::read_to_string(path.join("type")) else {
            continue;
        };
        if kind.trim() != "bluetooth" {
            continue;
        }
        if let Some(state) = read_u64(&path.join("state")) {
            return Some(state != 0);
        }
    }
    None
}

fn detect_brightness() -> Option<u8> {
    let entries = fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(current) = read_u64(&path.join("brightness")) else {
            continue;
        };
        let Some(maximum) = read_u64(&path.join("max_brightness")) else {
            continue;
        };
        if maximum == 0 {
            continue;
        }
        return Some(((current.saturating_mul(100) + maximum / 2) / maximum).min(100) as u8);
    }
    None
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_status_marks_optional_devices_unavailable() {
        let status = SystemStatus::default();
        assert_eq!(status.network, NetworkState::Offline);
        assert_eq!(status.volume, None);
        assert_eq!(status.brightness, None);
        assert_eq!(status.wifi_enabled, None);
        assert_eq!(status.bluetooth_enabled, None);
        assert_eq!(
            status.input.touchpad.config,
            tessera_types::input::TouchpadConfig::default()
        );
        assert_eq!(
            status.input.keyboard,
            tessera_types::input::KeyboardConfig::default()
        );
    }

    #[test]
    fn detect_wifi_radio_safely_evaluates() {
        // Runs against host environment or sandbox without panicking or spawning
        let _ = detect_wifi_radio();
    }
}
