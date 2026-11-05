//! Host system abstraction for live hardware mutations and telemetry probing.
//!
//! Separates real Linux host commands and sysfs probes from the compositor runtime,
//! enabling deterministic, zero-side-effect simulation through [`MockHostSystem`] in
//! preview and testing environments.
//!
//! Audio does **not** live here (ADR-0216). Its observation and intent contract
//! is [`crate::audio`], which this module composes: the sound server's graph is
//! a subsystem with its own parse seam, its own captured-document tests, and its
//! own mock, and folding it into the same file as `brightnessctl` and battery
//! sysfs would have made the one seam every test needs the least reviewable part
//! of the crate. What stays here is the composition: one `ForkedStatus` per full
//! probe, so a caller cannot accidentally observe two different moments of the
//! audio graph.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::audio::{AudioBackend, AudioSnapshot, PipeWireBackend};
use crate::host_command::spawn_host_command;

pub use tessera_desktop::system::BatteryStatus;
pub use tessera_desktop::system::NetworkState;
pub use tessera_desktop::system::SystemStatus;

/// Everything the status poller learns from probes that fork a subprocess, as
/// one coherent record so a new field cannot be silently dropped by the cheap
/// poll between full probes.
///
/// The cheap `/sys` pass reuses these values; it must carry all of them or the
/// snapshot would forget, say, which outputs exist every second poll — the
/// panel would then flicker its output list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkedStatus {
    /// The whole audio observation — inventories, defaults, levels, and the
    /// live capture flag — as one value (ADR-0215, ADR-0216).
    pub audio: AudioSnapshot,
    pub wifi_enabled: Option<bool>,
    pub wifi_ssid: Option<String>,
}

/// Backend-neutral contract for host system controls and status observation.
///
/// Audio is deliberately expressed here in the same verbs the compositor needs
/// and delegated to [`crate::audio`] by both implementations, so the runtime has
/// one dependency to inject and the audio contract has one owner.
pub trait HostSystem: Send + Sync {
    /// Mutate audio output volume level (0..=100).
    fn set_volume(&self, level: u8) -> Result<(), String>;

    /// Adjust audio output volume by delta (-100..=100).
    fn step_volume(&self, delta: i8) -> Result<(), String>;

    /// Toggle audio output mute state.
    fn toggle_mute(&self) -> Result<(), String>;

    /// Route playback to the named audio output (ADR-0215), by the sound
    /// server's own node name. The server owns the routing; this is a request
    /// to it, and success means the request was accepted, not that every
    /// already-running stream moved (a stream may pin its target).
    fn set_audio_output(&self, name: &str) -> Result<(), String>;

    /// Route new capture streams to the named audio input (ADR-0216), by the
    /// sound server's own node name.
    fn set_audio_input(&self, name: &str) -> Result<(), String>;

    /// Set the default input's capture volume percentage (ADR-0216).
    fn set_microphone_volume(&self, level: u8) -> Result<(), String>;

    /// Toggle the default input's mute (ADR-0216). Separate from
    /// [`HostSystem::toggle_mute`] on purpose: silencing the microphone is a
    /// privacy action and must not be reachable from the playback fader.
    fn toggle_microphone_mute(&self) -> Result<(), String>;

    /// Set screen backlight brightness (1..=100).
    fn set_brightness(&self, level: u8) -> Result<(), String>;

    /// Set keyboard backlight brightness in percent (0..=100) (ADR-0168).
    fn set_keyboard_brightness(&self, level: u8) -> Result<(), String>;

    /// Step keyboard backlight brightness through supported hardware tiers (ADR-0168).
    fn step_keyboard_brightness(&self) -> Result<(), String>;

    /// Dispatch session suspend.
    fn suspend(&self) -> Result<(), String>;

    /// Dispatch system reboot.
    fn reboot(&self) -> Result<(), String>;

    /// Dispatch system power off.
    fn power_off(&self) -> Result<(), String>;

    /// Detect complete system status snapshot.
    fn detect_status(&self) -> SystemStatus;

    /// Detect lightweight system status (cheap /sys read without forking),
    /// reusing the forked readings it cannot observe.
    fn detect_status_lightweight(&self, last: &ForkedStatus) -> SystemStatus;

    /// Probe the forked half of the status snapshot.
    fn detect_forked_status(&self) -> ForkedStatus;
}

/// Production Linux host system backend interacting with live OS services
/// (`brightnessctl`, `rfkill`, `systemctl`) and, for audio, the sound server's
/// own CLI bridge (ADR-0216).
#[derive(Debug, Clone)]
pub struct LiveHostSystem {
    audio: Arc<dyn AudioBackend>,
}

impl Default for LiveHostSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveHostSystem {
    pub fn new() -> Self {
        Self {
            audio: Arc::new(PipeWireBackend::new()),
        }
    }
}

impl HostSystem for LiveHostSystem {
    fn set_volume(&self, level: u8) -> Result<(), String> {
        self.audio.set_output_volume(level)
    }

    fn step_volume(&self, delta: i8) -> Result<(), String> {
        self.audio.step_output_volume(delta)
    }

    fn toggle_mute(&self) -> Result<(), String> {
        self.audio.toggle_output_mute()
    }

    fn set_audio_output(&self, name: &str) -> Result<(), String> {
        self.audio.set_output(name)
    }

    fn set_audio_input(&self, name: &str) -> Result<(), String> {
        self.audio.set_input(name)
    }

    fn set_microphone_volume(&self, level: u8) -> Result<(), String> {
        self.audio.set_input_volume(level)
    }

    fn toggle_microphone_mute(&self) -> Result<(), String> {
        self.audio.toggle_input_mute()
    }

    fn set_brightness(&self, level: u8) -> Result<(), String> {
        let amount = format!("{level}%");
        spawn_host_command("brightnessctl", &["--class=backlight", "set", &amount])
    }

    fn set_keyboard_brightness(&self, level: u8) -> Result<(), String> {
        let amount = format!("{level}%");
        spawn_host_command(
            "brightnessctl",
            &["--device=*::kbd_backlight", "set", &amount],
        )
    }

    fn step_keyboard_brightness(&self) -> Result<(), String> {
        let (level, steps) = detect_keyboard_backlight();
        let cur = level.unwrap_or(0);
        // Cycle the hardware's own ladder when the backlight is stepped, so a
        // three-rung ThinkPad advances 0 → 50 → 100, not through a fixed
        // four-rung sequence the device cannot represent.
        let next = match steps.filter(|n| *n >= 2) {
            Some(levels) => {
                let last = (levels - 1) as u32;
                let rung = (0..levels as u32)
                    .min_by_key(|step| {
                        let percent = (step * 100 + last / 2) / last;
                        (percent as i32 - cur as i32).unsigned_abs()
                    })
                    .unwrap_or(0);
                let next_rung = (rung + 1) % levels as u32;
                ((next_rung * 100 + last / 2) / last) as u8
            }
            None => {
                if cur >= 90 {
                    0
                } else if cur >= 60 {
                    100
                } else if cur >= 30 {
                    66
                } else {
                    33
                }
            }
        };
        self.set_keyboard_brightness(next)
    }

    fn suspend(&self) -> Result<(), String> {
        spawn_host_command("systemctl", &["suspend"])
    }

    fn reboot(&self) -> Result<(), String> {
        spawn_host_command("systemctl", &["reboot"])
    }

    fn power_off(&self) -> Result<(), String> {
        spawn_host_command("systemctl", &["poweroff"])
    }

    fn detect_status(&self) -> SystemStatus {
        detect_system_status()
    }

    fn detect_status_lightweight(&self, last: &ForkedStatus) -> SystemStatus {
        detect_system_status_lightweight(last)
    }

    fn detect_forked_status(&self) -> ForkedStatus {
        detect_forked_status()
    }
}

/// Simulated in-memory host system state for deterministic testing and preview.
#[derive(Debug, Clone)]
pub struct MockHostState {
    pub brightness: Option<u8>,
    pub kbd_brightness: Option<u8>,
    /// Distinct illumination steps the simulated keyboard backlight exposes;
    /// `None` models fine-grained/unknown hardware (ADR-0168 amendment).
    pub kbd_brightness_levels: Option<u8>,
    pub bluetooth_enabled: Option<bool>,
    pub battery: Option<BatteryStatus>,
    pub network: NetworkState,
    pub network_interface: String,
    pub wifi_ssid: Option<String>,
    pub wifi_enabled: Option<bool>,
    pub last_power_action: Option<String>,
}

impl Default for MockHostState {
    fn default() -> Self {
        Self {
            brightness: Some(80),
            kbd_brightness: Some(66),
            // The mock models a stepped backlight, matching the four-rung
            // ladder preview shows.
            kbd_brightness_levels: Some(4),
            bluetooth_enabled: Some(true),
            battery: Some(BatteryStatus {
                percent: 88,
                charging: true,
            }),
            network: NetworkState::Wifi,
            network_interface: "wlan0".into(),
            wifi_ssid: Some("Home-WiFi-5G".into()),
            wifi_enabled: Some(true),
            last_power_action: None,
        }
    }
}

/// Headless controllable mock host system backend for testing and preview.
#[derive(Debug, Clone)]
pub struct MockHostSystem {
    state: Arc<Mutex<MockHostState>>,
    audio: Arc<crate::audio::MockAudioBackend>,
}

impl Default for MockHostSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl MockHostSystem {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockHostState::default())),
            audio: Arc::new(crate::audio::MockAudioBackend::new()),
        }
    }

    #[allow(dead_code)]
    pub fn with_state(state: MockHostState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
            audio: Arc::new(crate::audio::MockAudioBackend::new()),
        }
    }

    /// The mock sound server this host delegates audio to, so a test can script
    /// capture flags, inventories, and failures directly (ADR-0216).
    #[allow(dead_code)]
    pub fn audio(&self) -> &crate::audio::MockAudioBackend {
        &self.audio
    }

    #[allow(dead_code)]
    pub fn state(&self) -> Arc<Mutex<MockHostState>> {
        self.state.clone()
    }
}

impl HostSystem for MockHostSystem {
    fn set_volume(&self, level: u8) -> Result<(), String> {
        self.audio.set_output_volume(level)
    }

    fn step_volume(&self, delta: i8) -> Result<(), String> {
        self.audio.step_output_volume(delta)
    }

    fn toggle_mute(&self) -> Result<(), String> {
        self.audio.toggle_output_mute()
    }

    fn set_audio_output(&self, name: &str) -> Result<(), String> {
        self.audio.set_output(name)
    }

    fn set_audio_input(&self, name: &str) -> Result<(), String> {
        self.audio.set_input(name)
    }

    fn set_microphone_volume(&self, level: u8) -> Result<(), String> {
        self.audio.set_input_volume(level)
    }

    fn toggle_microphone_mute(&self) -> Result<(), String> {
        self.audio.toggle_input_mute()
    }

    fn set_brightness(&self, level: u8) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.brightness = Some(level);
        Ok(())
    }

    fn set_keyboard_brightness(&self, level: u8) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.kbd_brightness = Some(level);
        Ok(())
    }

    fn step_keyboard_brightness(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let cur = st.kbd_brightness.unwrap_or(0);
        // Advance through the simulated hardware's own ladder when it is
        // stepped, mirroring the live backend.
        let next = match st.kbd_brightness_levels.filter(|n| *n >= 2) {
            Some(levels) => {
                let last = (levels - 1) as u32;
                let rung = (0..levels as u32)
                    .min_by_key(|step| {
                        let percent = (step * 100 + last / 2) / last;
                        (percent as i32 - cur as i32).unsigned_abs()
                    })
                    .unwrap_or(0);
                let next_rung = (rung + 1) % levels as u32;
                ((next_rung * 100 + last / 2) / last) as u8
            }
            None => {
                if cur >= 90 {
                    0
                } else if cur >= 60 {
                    100
                } else if cur >= 30 {
                    66
                } else {
                    33
                }
            }
        };
        st.kbd_brightness = Some(next);
        Ok(())
    }

    fn suspend(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.last_power_action = Some("suspend".into());
        Ok(())
    }

    fn reboot(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.last_power_action = Some("reboot".into());
        Ok(())
    }

    fn power_off(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.last_power_action = Some("power_off".into());
        Ok(())
    }

    fn detect_status(&self) -> SystemStatus {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut status = self.sysfs_status(&st);
        apply_audio(&mut status, &self.audio.snapshot());
        status
    }

    fn detect_status_lightweight(&self, _last: &ForkedStatus) -> SystemStatus {
        // The mock is free to observe; there is no process to fork.
        self.detect_status()
    }

    fn detect_forked_status(&self) -> ForkedStatus {
        // One lock, not one per field: `std::sync::Mutex` is not reentrant, and
        // two acquisitions in one expression self-deadlock.
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        ForkedStatus {
            audio: self.audio.snapshot(),
            wifi_enabled: state.wifi_enabled,
            wifi_ssid: state.wifi_ssid.clone(),
        }
    }
}

impl MockHostSystem {
    /// The `/sys` half of the mock snapshot: everything the mock models that
    /// does not come from the sound server.
    fn sysfs_status(&self, st: &MockHostState) -> SystemStatus {
        SystemStatus {
            network: st.network,
            network_interface: st.network_interface.clone(),
            wifi_ssid: st.wifi_ssid.clone(),
            battery: st.battery,
            wifi_enabled: st.wifi_enabled,
            wifi_state: if st.wifi_ssid.is_some() {
                tessera_desktop::system::WifiLinkState::Connected
            } else {
                tessera_desktop::system::WifiLinkState::Disconnected
            },
            wifi_networks: Vec::new(),
            bluetooth_enabled: st.bluetooth_enabled,
            // The Bluetooth subsystem (ADR-0175) overlays state/devices from
            // the mock backend each tick; the mock host only owns the boolean.
            bluetooth_state: tessera_desktop::system::BluetoothLinkState::Disabled,
            bluetooth_devices: Vec::new(),
            brightness: st.brightness,
            kbd_brightness: st.kbd_brightness,
            kbd_brightness_levels: st.kbd_brightness_levels,
            ..SystemStatus::default()
        }
    }
}

/// Overlay one audio observation onto a status snapshot.
///
/// The single place the audio fields are copied, so the full and the cheap pass
/// cannot drift: a new audio field added to [`AudioSnapshot`] is wired here once
/// and both passes gain it (`[INV-AUDIO-6]`).
fn apply_audio(status: &mut SystemStatus, audio: &AudioSnapshot) {
    status.volume = audio.output_volume;
    status.muted = audio.output_muted;
    status.audio_outputs = audio.outputs.clone();
    status.audio_inputs = audio.inputs.clone();
    status.microphone_volume = audio.input_volume;
    status.microphone_muted = audio.input_muted;
    status.microphone_active = audio.capturing;
}

/// Probe live host services through standard Linux interfaces.
fn detect_system_status() -> SystemStatus {
    detect_system_status_lightweight(&detect_forked_status())
}

/// Probe every status field whose source is a cheap `/sys` read, reusing the
/// fork+exec readings it cannot observe (the sound server's graph, the radio's
/// soft-block).
fn detect_system_status_lightweight(last: &ForkedStatus) -> SystemStatus {
    let (network, network_interface) = detect_network();
    let (kbd_level, kbd_levels) = detect_keyboard_backlight();
    let wifi_state = if last.wifi_ssid.is_some() {
        tessera_desktop::system::WifiLinkState::Connected
    } else if last.wifi_enabled == Some(false) {
        tessera_desktop::system::WifiLinkState::Disabled
    } else {
        tessera_desktop::system::WifiLinkState::Disconnected
    };
    let mut status = SystemStatus {
        network,
        network_interface,
        wifi_ssid: last.wifi_ssid.clone(),
        battery: detect_battery(),
        wifi_enabled: last.wifi_enabled,
        wifi_state,
        wifi_networks: Vec::new(),
        bluetooth_enabled: detect_bluetooth_radio(),
        // Overlaid from the Bluetooth subsystem each tick (ADR-0175).
        bluetooth_state: tessera_desktop::system::BluetoothLinkState::Disabled,
        bluetooth_devices: Vec::new(),
        brightness: detect_brightness(),
        kbd_brightness: kbd_level,
        kbd_brightness_levels: kbd_levels,
        ..SystemStatus::default()
    };
    apply_audio(&mut status, &last.audio);
    status
}

/// Run every forked probe: the sound server's whole audio observation and the
/// radio's soft-block.
///
/// The audio half is one call, so the inventories, the default markers, the
/// levels, and the live capture flag always describe the same moment of a graph
/// that is moving under the desktop (`[INV-AUDIO-6]`).
fn detect_forked_status() -> ForkedStatus {
    ForkedStatus {
        audio: PipeWireBackend::new().snapshot(),
        wifi_enabled: detect_wifi_radio(),
        wifi_ssid: None,
    }
}

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

/// The rfkill soft-block state of the Bluetooth radio, or `None` when the
/// host has no Bluetooth rfkill entry. The Bluetooth subsystem (ADR-0175)
/// owns the authoritative radio state; this probe is the fallback for a host
/// with no BlueZ service, where the soft-block is the only honest signal.
pub(crate) fn detect_bluetooth_radio() -> Option<bool> {
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

/// Probe the keyboard backlight: `(level_percent, distinct_steps)`.
///
/// The level is normalized to 0..=100 for presentation. The step count is
/// `max_brightness + 1` — the number of *distinct* illumination levels the LED
/// class exposes including "off" — which is the honest granularity chrome needs
/// to choose between a stepped selector and a continuous control (ADR-0168
/// amendment). A `max_brightness` of `0` is treated as unknown granularity
/// (`None`) rather than a single step, since a zero-max LED cannot be set.
///
/// Returns `(None, None)` when no standard `*::kbd_backlight` node exists.
fn detect_keyboard_backlight() -> (Option<u8>, Option<u8>) {
    let Some(entries) = fs::read_dir("/sys/class/leds").ok() else {
        return (None, None);
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with("::kbd_backlight") && !name_str.contains("kbd_backlight") {
            continue;
        }
        let path = entry.path();
        let Some(current) = read_u64(&path.join("brightness")) else {
            continue;
        };
        let Some(maximum) = read_u64(&path.join("max_brightness")) else {
            continue;
        };
        if maximum == 0 {
            // Present but unusable; report a level with unknown granularity so
            // chrome falls back to a continuous control.
            return (Some(0), None);
        }
        let level = ((current.saturating_mul(100) + maximum / 2) / maximum).min(100) as u8;
        let steps = maximum.saturating_add(1).min(u8::MAX as u64) as u8;
        return (Some(level), Some(steps));
    }
    (None, None)
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_status_marks_optional_devices_unavailable() {
        let status = SystemStatus::default();
        assert_eq!(status.network, NetworkState::Offline);
        // Absent audio is unknown, never silent: a fader pinned at zero would
        // claim the machine has no sound when the probe simply had not run.
        assert_eq!(status.volume, None);
        assert_eq!(status.microphone_volume, None);
        assert!(!status.microphone_muted);
        assert!(!status.microphone_active);
        assert!(status.audio_outputs.is_empty());
        assert!(status.audio_inputs.is_empty());
        assert_eq!(status.brightness, None);
        assert_eq!(status.wifi_enabled, None);
        assert_eq!(status.bluetooth_enabled, None);
        assert_eq!(
            status.input.touchpad.config,
            tessera_primitives::input::TouchpadConfig::default()
        );
        assert_eq!(
            status.input.keyboard,
            tessera_primitives::input::KeyboardConfig::default()
        );
    }

    #[test]
    fn test_inv_audio_6_one_observation_feeds_both_passes() {
        // `[INV-AUDIO-6]`: the whole audio reading comes from one observation,
        // and the cheap `/sys` pass reuses it through the single `apply_audio`
        // wiring rather than re-deriving any field. Both passes must therefore
        // report byte-identical audio, or the picker would flicker between them.
        let mock = MockHostSystem::new();
        let forked = mock.detect_forked_status();
        let full = mock.detect_status();
        let cheap = mock.detect_status_lightweight(&forked);
        assert_eq!(full.volume, cheap.volume);
        assert_eq!(full.muted, cheap.muted);
        assert_eq!(full.audio_outputs, cheap.audio_outputs);
        assert_eq!(full.audio_inputs, cheap.audio_inputs);
        assert_eq!(full.microphone_volume, cheap.microphone_volume);
        assert_eq!(full.microphone_muted, cheap.microphone_muted);
        assert_eq!(full.microphone_active, cheap.microphone_active);
        // And the live probe's cheap pass agrees with the full pass for the same
        // reason, on whatever machine the suite runs on.
        let live = detect_forked_status();
        let full = detect_system_status_lightweight(&live);
        let cheap = detect_system_status_lightweight(&live);
        assert_eq!(full.audio_outputs, cheap.audio_outputs);
        assert_eq!(full.audio_inputs, cheap.audio_inputs);
        assert_eq!(full.microphone_active, cheap.microphone_active);
    }

    #[test]
    fn detect_wifi_radio_safely_evaluates() {
        let _ = detect_wifi_radio();
    }

    #[test]
    fn the_live_audio_probe_reports_a_coherent_graph() {
        // Live probe, like `detect_wifi_radio_safely_evaluates`: the inventory
        // depends on the machine, so only the invariants are asserted. A host
        // with no sound server yields an empty graph, which is also valid.
        let audio = detect_forked_status().audio;
        assert!(
            audio
                .outputs
                .iter()
                .filter(|output| output.is_default)
                .count()
                <= 1,
            "at most one output can be the default: {:?}",
            audio.outputs
        );
        assert!(
            audio.inputs.iter().filter(|input| input.is_default).count() <= 1,
            "at most one input can be the default: {:?}",
            audio.inputs
        );
        let labelled = audio
            .outputs
            .iter()
            .map(|output| (&output.name, &output.description))
            .chain(
                audio
                    .inputs
                    .iter()
                    .map(|input| (&input.name, &input.description)),
            );
        for (name, description) in labelled {
            assert!(!name.is_empty());
            assert!(!description.is_empty(), "every row needs a label");
        }
        // The list is ordered by transport rank, so the pickers do not
        // reshuffle between probes.
        assert!(
            audio
                .outputs
                .windows(2)
                .all(|pair| pair[0].kind.sort_rank() <= pair[1].kind.sort_rank()),
            "outputs are grouped by transport: {:?}",
            audio.outputs
        );
        assert!(
            audio
                .inputs
                .windows(2)
                .all(|pair| pair[0].kind.sort_rank() <= pair[1].kind.sort_rank()),
            "inputs are grouped by transport: {:?}",
            audio.inputs
        );
        // The capture flag agrees with the graph it was read from: no server,
        // no capture.
        if audio.outputs.is_empty() && audio.inputs.is_empty() {
            assert!(!audio.capturing);
        }
    }

    #[test]
    fn mock_host_system_deterministic_mutations_and_telemetry() {
        let mock = MockHostSystem::new();
        let status = mock.detect_status();
        assert_eq!(status.volume, Some(65));
        assert_eq!(status.brightness, Some(80));
        assert!(!status.muted);
        assert_eq!(status.bluetooth_enabled, Some(true));
        assert_eq!(status.battery.unwrap().percent, 88);
        // The microphone half is reported alongside the playback half, from the
        // same observation.
        assert_eq!(status.microphone_volume, Some(80));
        assert!(!status.microphone_muted);
        assert!(!status.microphone_active);
        assert_eq!(status.audio_inputs.len(), 2);

        // Volume adjustments
        assert!(mock.set_volume(42).is_ok());
        assert_eq!(mock.detect_status().volume, Some(42));
        assert!(mock.step_volume(5).is_ok());
        assert_eq!(mock.detect_status().volume, Some(47));
        assert!(mock.toggle_mute().is_ok());
        assert!(mock.detect_status().muted);

        // Microphone adjustments travel the dedicated verbs, so silencing the
        // microphone can never be reached through the playback fader.
        assert!(mock.set_microphone_volume(30).is_ok());
        assert_eq!(mock.detect_status().microphone_volume, Some(30));
        assert!(mock.toggle_microphone_mute().is_ok());
        let status = mock.detect_status();
        assert!(status.microphone_muted);
        assert!(
            status.muted,
            "the output mute is untouched by the microphone toggle"
        );

        // Brightness and Bluetooth
        assert!(mock.set_brightness(95).is_ok());
        assert_eq!(mock.detect_status().brightness, Some(95));
        // Bluetooth power is owned by the subsystem (ADR-0175): the host
        // probe only reports the rfkill reading and no longer exposes a
        // `rfkill block/unblock` write path.

        // Keyboard backlight
        assert!(mock.set_keyboard_brightness(50).is_ok());
        assert_eq!(mock.detect_status().kbd_brightness, Some(50));
        assert!(mock.step_keyboard_brightness().is_ok());
        assert_eq!(mock.detect_status().kbd_brightness, Some(67));

        // Power actions
        assert!(mock.suspend().is_ok());
        assert_eq!(
            mock.state().lock().unwrap().last_power_action.as_deref(),
            Some("suspend")
        );
        assert!(mock.reboot().is_ok());
        assert_eq!(
            mock.state().lock().unwrap().last_power_action.as_deref(),
            Some("reboot")
        );
        assert!(mock.power_off().is_ok());
        assert_eq!(
            mock.state().lock().unwrap().last_power_action.as_deref(),
            Some("power_off")
        );
    }

    #[test]
    fn the_mock_reports_a_live_capture_only_when_the_sound_server_says_so() {
        // `microphone_active` is an observation of the server's link graph, so
        // preview and tests drive it explicitly rather than deriving it from
        // chrome's own bookkeeping (`[INV-AUDIO-7]`).
        let mock = MockHostSystem::new();
        assert!(!mock.detect_status().microphone_active);
        mock.audio()
            .state()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capturing = true;
        assert!(mock.detect_status().microphone_active);
        assert!(mock.detect_forked_status().audio.capturing);
    }

    #[test]
    fn mock_audio_output_switch_moves_the_only_default() {
        let mock = MockHostSystem::new();
        assert_eq!(
            mock.detect_forked_status()
                .audio
                .outputs
                .iter()
                .filter(|output| output.is_default)
                .count(),
            1,
            "the mock starts with exactly one default"
        );
        assert!(mock.set_audio_output("mock_output_headphones").is_ok());
        let outputs = mock.detect_status().audio_outputs;
        assert_eq!(
            outputs
                .iter()
                .filter(|output| output.is_default)
                .map(|output| output.name.as_str())
                .collect::<Vec<_>>(),
            ["mock_output_headphones"]
        );
        // A name the mock does not expose is refused rather than silently
        // ignored, matching the live backend's contract.
        assert!(mock.set_audio_output("mock_output_nonexistent").is_err());
    }
}
