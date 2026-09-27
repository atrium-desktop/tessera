//! Host system abstraction for live hardware mutations and telemetry probing.
//!
//! Separates real Linux host commands and sysfs probes from the compositor runtime,
//! enabling deterministic, zero-side-effect simulation through [`MockHostSystem`] in
//! preview and testing environments.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

pub use tessera_desktop::settings::DisplayStatus;
pub use tessera_desktop::system::BatteryStatus;
pub use tessera_desktop::system::NetworkState;
pub use tessera_desktop::system::SystemStatus;

/// Backend-neutral contract for host system controls and status observation.
pub trait HostSystem: Send + Sync {
    /// Mutate audio output volume level (0..=100).
    fn set_volume(&self, level: u8) -> Result<(), String>;

    /// Adjust audio output volume by delta (-100..=100).
    fn step_volume(&self, delta: i8) -> Result<(), String>;

    /// Toggle audio output mute state.
    fn toggle_mute(&self) -> Result<(), String>;

    /// Set screen backlight brightness (1..=100).
    fn set_brightness(&self, level: u8) -> Result<(), String>;

    /// Set keyboard backlight brightness in percent (0..=100) (ADR-0168).
    fn set_keyboard_brightness(&self, level: u8) -> Result<(), String>;

    /// Step keyboard backlight brightness through supported hardware tiers (ADR-0168).
    fn step_keyboard_brightness(&self) -> Result<(), String>;

    /// Enable or disable bluetooth radio.
    fn set_bluetooth_enabled(&self, enabled: bool) -> Result<(), String>;

    /// Dispatch session suspend.
    fn suspend(&self) -> Result<(), String>;

    /// Dispatch system reboot.
    fn reboot(&self) -> Result<(), String>;

    /// Dispatch system power off.
    fn power_off(&self) -> Result<(), String>;

    /// Detect complete system status snapshot.
    fn detect_status(&self) -> SystemStatus;

    /// Detect lightweight system status (cheap /sys read without forking).
    fn detect_status_lightweight(
        &self,
        last_volume: Option<u8>,
        last_muted: bool,
        last_wifi_enabled: Option<bool>,
        last_wifi_ssid: Option<String>,
    ) -> SystemStatus;

    /// Probe forked status (`(volume, muted, wifi_enabled, wifi_ssid)`).
    fn detect_forked_status(&self) -> (Option<u8>, bool, Option<bool>, Option<String>);
}

/// Production Linux host system backend interacting with live OS services (`wpctl`, `brightnessctl`, `rfkill`, `systemctl`).
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveHostSystem;

impl LiveHostSystem {
    pub fn new() -> Self {
        Self
    }
}

impl HostSystem for LiveHostSystem {
    fn set_volume(&self, level: u8) -> Result<(), String> {
        let amount = format!("{level}%");
        spawn_host_command(
            "wpctl",
            &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
        )
    }

    fn step_volume(&self, delta: i8) -> Result<(), String> {
        let amount = format!(
            "{}%{}",
            delta.unsigned_abs(),
            if delta >= 0 { "+" } else { "-" }
        );
        spawn_host_command(
            "wpctl",
            &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
        )
    }

    fn toggle_mute(&self) -> Result<(), String> {
        spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
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
        let cur = detect_keyboard_brightness().unwrap_or(0);
        let next = if cur >= 90 {
            0
        } else if cur >= 60 {
            100
        } else if cur >= 30 {
            66
        } else {
            33
        };
        self.set_keyboard_brightness(next)
    }

    fn set_bluetooth_enabled(&self, enabled: bool) -> Result<(), String> {
        spawn_host_command(
            "rfkill",
            &[if enabled { "unblock" } else { "block" }, "bluetooth"],
        )
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

    fn detect_status_lightweight(
        &self,
        last_volume: Option<u8>,
        last_muted: bool,
        last_wifi_enabled: Option<bool>,
        last_wifi_ssid: Option<String>,
    ) -> SystemStatus {
        detect_system_status_lightweight(
            last_volume,
            last_muted,
            last_wifi_enabled,
            last_wifi_ssid,
        )
    }

    fn detect_forked_status(&self) -> (Option<u8>, bool, Option<bool>, Option<String>) {
        detect_forked_status()
    }
}

/// Simulated in-memory host system state for deterministic testing and preview.
#[derive(Debug, Clone)]
pub struct MockHostState {
    pub volume: Option<u8>,
    pub muted: bool,
    pub brightness: Option<u8>,
    pub kbd_brightness: Option<u8>,
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
            volume: Some(65),
            muted: false,
            brightness: Some(80),
            kbd_brightness: Some(66),
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
#[derive(Debug, Clone, Default)]
pub struct MockHostSystem {
    state: Arc<Mutex<MockHostState>>,
}

impl MockHostSystem {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockHostState::default())),
        }
    }

    #[allow(dead_code)]
    pub fn with_state(state: MockHostState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    #[allow(dead_code)]
    pub fn state(&self) -> Arc<Mutex<MockHostState>> {
        self.state.clone()
    }
}

impl HostSystem for MockHostSystem {
    fn set_volume(&self, level: u8) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.volume = Some(level);
        if st.muted {
            st.muted = false;
        }
        Ok(())
    }

    fn step_volume(&self, delta: i8) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let cur = st.volume.unwrap_or(0) as i16;
        st.volume = Some((cur + i16::from(delta)).clamp(0, 100) as u8);
        Ok(())
    }

    fn toggle_mute(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.muted = !st.muted;
        Ok(())
    }

    fn set_brightness(&self, level: u8) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.brightness = Some(level);
        Ok(())
    }

    fn set_keyboard_brightness(&self, level: u8) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.kbd_brightness = Some(level);
        Ok(())
    }

    fn step_keyboard_brightness(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        let cur = st.kbd_brightness.unwrap_or(0);
        let next = if cur >= 90 {
            0
        } else if cur >= 60 {
            100
        } else if cur >= 30 {
            66
        } else {
            33
        };
        st.kbd_brightness = Some(next);
        Ok(())
    }

    fn set_bluetooth_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.bluetooth_enabled = Some(enabled);
        Ok(())
    }

    fn suspend(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.last_power_action = Some("suspend".into());
        Ok(())
    }

    fn reboot(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.last_power_action = Some("reboot".into());
        Ok(())
    }

    fn power_off(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        st.last_power_action = Some("power_off".into());
        Ok(())
    }

    fn detect_status(&self) -> SystemStatus {
        let st = self.state.lock().unwrap();
        SystemStatus {
            volume: st.volume,
            muted: st.muted,
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
            brightness: st.brightness,
            kbd_brightness: st.kbd_brightness,
            do_not_disturb: false,
            input: tessera_primitives::input::InputStatus::default(),
            display: DisplayStatus::default(),
            idle_inhibited: false,
            power_mode: tessera_desktop::power::PowerMode::default(),
            capture_streams: 0,
        }
    }

    fn detect_status_lightweight(
        &self,
        _last_volume: Option<u8>,
        _last_muted: bool,
        _last_wifi_enabled: Option<bool>,
        _last_wifi_ssid: Option<String>,
    ) -> SystemStatus {
        self.detect_status()
    }

    fn detect_forked_status(&self) -> (Option<u8>, bool, Option<bool>, Option<String>) {
        let st = self.state.lock().unwrap();
        (st.volume, st.muted, st.wifi_enabled, st.wifi_ssid.clone())
    }
}

/// Probe live host services through standard Linux interfaces.
fn detect_system_status() -> SystemStatus {
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
        kbd_brightness: detect_keyboard_brightness(),
        do_not_disturb: false,
        input: tessera_primitives::input::InputStatus::default(),
        display: DisplayStatus::default(),
        idle_inhibited: false,
        power_mode: tessera_desktop::power::PowerMode::default(),
        capture_streams: 0,
    }
}

/// Probe every status field whose source is a cheap `/sys` read, deferring the
/// audio fork+exec probe (`wpctl get-volume`) to a separate full pass.
fn detect_system_status_lightweight(
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
        kbd_brightness: detect_keyboard_brightness(),
        do_not_disturb: false,
        input: tessera_primitives::input::InputStatus::default(),
        display: DisplayStatus::default(),
        idle_inhibited: false,
        power_mode: tessera_desktop::power::PowerMode::default(),
        capture_streams: 0,
    }
}

/// Run only the forked probes and return `(volume, muted, wifi_enabled, wifi_ssid)`.
fn detect_forked_status() -> (Option<u8>, bool, Option<bool>, Option<String>) {
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

fn detect_keyboard_brightness() -> Option<u8> {
    let entries = fs::read_dir("/sys/class/leds").ok()?;
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
            return Some(0);
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

/// Spawn a short-lived host control command without blocking the compositor
/// main loop, while still reaping the child to avoid zombie accumulation.
fn spawn_host_command(program: &str, args: &[&str]) -> Result<(), String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start {program}: {error}"))?;
    host_command_reaper().reap(child);
    Ok(())
}

struct HostCommandReaper {
    tx: mpsc::Sender<Child>,
}

impl HostCommandReaper {
    fn reap(&self, child: Child) {
        let _ = self.tx.send(child);
    }
}

fn host_command_reaper() -> &'static HostCommandReaper {
    static REAPER: OnceLock<HostCommandReaper> = OnceLock::new();
    REAPER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Child>();
        std::thread::Builder::new()
            .name("tessera-host-cmd-reaper".into())
            .spawn(move || {
                while let Ok(mut child) = rx.recv() {
                    let _ = child.wait();
                }
            })
            .expect("spawn host-command reaper");
        HostCommandReaper { tx }
    })
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
            tessera_primitives::input::TouchpadConfig::default()
        );
        assert_eq!(
            status.input.keyboard,
            tessera_primitives::input::KeyboardConfig::default()
        );
    }

    #[test]
    fn detect_wifi_radio_safely_evaluates() {
        let _ = detect_wifi_radio();
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

        // Volume adjustments
        assert!(mock.set_volume(42).is_ok());
        assert_eq!(mock.detect_status().volume, Some(42));
        assert!(mock.step_volume(5).is_ok());
        assert_eq!(mock.detect_status().volume, Some(47));
        assert!(mock.toggle_mute().is_ok());
        assert!(mock.detect_status().muted);

        // Brightness and Bluetooth
        assert!(mock.set_brightness(95).is_ok());
        assert_eq!(mock.detect_status().brightness, Some(95));
        assert!(mock.set_bluetooth_enabled(false).is_ok());
        assert_eq!(mock.detect_status().bluetooth_enabled, Some(false));

        // Keyboard backlight
        assert_eq!(mock.detect_status().kbd_brightness, Some(66));
        assert!(mock.set_keyboard_brightness(100).is_ok());
        assert_eq!(mock.detect_status().kbd_brightness, Some(100));
        assert!(mock.step_keyboard_brightness().is_ok());
        assert_eq!(mock.detect_status().kbd_brightness, Some(0));
        assert!(mock.step_keyboard_brightness().is_ok());
        assert_eq!(mock.detect_status().kbd_brightness, Some(33));

        // Power actions record in mock state without touching host
        assert!(mock.suspend().is_ok());
        assert_eq!(mock.state().lock().unwrap().last_power_action.as_deref(), Some("suspend"));
        assert!(mock.reboot().is_ok());
        assert_eq!(mock.state().lock().unwrap().last_power_action.as_deref(), Some("reboot"));
        assert!(mock.power_off().is_ok());
        assert_eq!(mock.state().lock().unwrap().last_power_action.as_deref(), Some("power_off"));
    }
}
