//! Backend-neutral live system status and immediate control intents.
//!
//! These types describe state whose authority remains with a live service or
//! the current compositor session. They are deliberately separate from
//! [`crate::settings`], whose transactions persist configuration.

use crate::power::PowerMode;
use crate::settings::DisplayStatus;
use tessera_primitives::input::InputStatus;

/// Link and radio state of the Bluetooth adapter (ADR-0175).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BluetoothLinkState {
    #[default]
    Disabled,
    Unavailable,
    Idle,
    Scanning,
    Connected,
}

/// Coarse connectivity state shown by desktop status surfaces.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkState {
    #[default]
    Offline,
    Wifi,
    Wired,
}

/// Security requirements of a wireless network (ADR-0162).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WifiSecurity {
    #[default]
    Open,
    WpaPsk,
    Enterprise,
}

/// Link and radio state of the wireless interface (ADR-0162).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WifiLinkState {
    #[default]
    Disabled,
    Disconnected,
    Scanning,
    Connecting,
    Connected,
}

/// One observed wireless network / access point (ADR-0162, ADR-0167).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    /// Normalized signal strength (0..=4).
    pub signal_bars: u8,
    pub security: WifiSecurity,
    pub is_connected: bool,
    pub is_saved: bool,
    /// Whether automatic reconnection is enabled for this known profile (ADR-0167).
    #[cfg_attr(feature = "serde", serde(default))]
    pub auto_connect: bool,
    /// Center frequency in MHz (e.g. 2412 for 2.4 GHz, 5180 for 5 GHz), if known (ADR-0167).
    #[cfg_attr(feature = "serde", serde(default))]
    pub frequency_mhz: Option<u32>,
}

/// Device class of a Bluetooth peripheral (ADR-0175).
///
/// Mapped from the host's device class/icon vocabulary into a daemon-neutral
/// enum, so presentation never sees BlueZ's `Icon` strings or class bitfield
/// (`[INV-NET-DAEMON-NEUTRAL]`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BluetoothKind {
    #[default]
    Other,
    Audio,
    Input,
    Phone,
    Computer,
    Imaging,
    Wearable,
}

/// One observed Bluetooth peripheral, paired or merely in range (ADR-0175).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BluetoothDevice {
    /// Stable transport address (e.g. `AC:12:34:56:78:9A`). The identity a
    /// pair/connect/disconnect/forget intent names, because an alias is
    /// user-editable and need not be unique.
    pub address: String,
    /// Human-readable alias. Empty until the host resolves one (a freshly
    /// discovered peripheral may advertise none).
    #[cfg_attr(feature = "serde", serde(default))]
    pub name: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: BluetoothKind,
    /// True while an ACL link to this peripheral is up.
    pub connected: bool,
    /// True when credentials are stored host-side: the entry survives the
    /// peripheral leaving range and may reconnect without re-pairing.
    pub paired: bool,
    /// True when the host is allowed to connect without confirmation.
    #[cfg_attr(feature = "serde", serde(default))]
    pub trusted: bool,
}

/// Battery state read from the host power service.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryStatus {
    pub percent: u8,
    pub charging: bool,
}

/// Per-threshold "already warned" latches for the low-battery alert.
///
/// The compositor runtime feeds every applied battery sample through
/// [`BatteryWarningLatches::poll`]. A threshold fires once per discharge
/// cycle and rearms only after the level recovers above it; charging clears
/// every latch, so the next discharge warns again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatteryWarningLatches {
    fired: Vec<u8>,
}

impl BatteryWarningLatches {
    /// Evaluate one battery sample against the configured warning thresholds
    /// (1..=99, strictly descending — see
    /// [`crate::settings::BatterySettings`]; an empty slice disables the
    /// feature). Returns the threshold to alert on, if any.
    pub fn poll(&mut self, percent: u8, charging: bool, thresholds: &[u8]) -> Option<u8> {
        if charging {
            self.fired.clear();
            return None;
        }
        // A threshold whose level recovered above it can fire again.
        self.fired.retain(|latched| percent <= *latched);
        let crossed: Vec<u8> = thresholds
            .iter()
            .copied()
            .filter(|threshold| percent <= *threshold && !self.fired.contains(threshold))
            .collect();
        if crossed.is_empty() {
            return None;
        }
        // One alert even when a fast drop crosses several thresholds at
        // once; the lowest is the most severe.
        let lowest = crossed.iter().min().copied();
        self.fired.extend(crossed);
        lowest
    }
}

/// One coherent observation of live host and compositor-owned session state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemStatus {
    pub volume: Option<u8>,
    pub muted: bool,
    pub network: NetworkState,
    /// The sysfs name of the interface behind `network` (e.g. `wlan0`,
    /// `enp3s0`), empty when offline. Presentation shows it beside the
    /// link state so the panel reads as "which NIC".
    #[cfg_attr(feature = "serde", serde(default))]
    pub network_interface: String,
    /// The associated Wi-Fi network name when `network` is a live wireless
    /// link, `None` otherwise (wired, offline, or disconnected).
    /// Synchronized from the wireless subsystem (ADR-0162).
    #[cfg_attr(feature = "serde", serde(default))]
    pub wifi_ssid: Option<String>,
    pub battery: Option<BatteryStatus>,
    /// `None` means the Wi-Fi radio service is unavailable.
    pub wifi_enabled: Option<bool>,
    /// Fine-grained wireless link state (ADR-0162).
    #[cfg_attr(feature = "serde", serde(default))]
    pub wifi_state: WifiLinkState,
    /// Discovered wireless networks, sorted by signal strength (ADR-0162).
    #[cfg_attr(feature = "serde", serde(default))]
    pub wifi_networks: Vec<WifiNetwork>,
    /// `None` means no Bluetooth radio service is available.
    pub bluetooth_enabled: Option<bool>,
    /// Fine-grained Bluetooth link state (ADR-0175).
    #[cfg_attr(feature = "serde", serde(default))]
    pub bluetooth_state: BluetoothLinkState,
    /// Known and discovered Bluetooth peripherals (ADR-0175). Presentation
    /// shows paired peripherals first, then in-range ones by recency.
    #[cfg_attr(feature = "serde", serde(default))]
    pub bluetooth_devices: Vec<BluetoothDevice>,
    /// Backlight level in percent, or `None` without a controllable backlight.
    pub brightness: Option<u8>,
    /// Keyboard backlight level in percent, or `None` without a controllable keyboard backlight (ADR-0168).
    #[cfg_attr(feature = "serde", serde(default))]
    pub kbd_brightness: Option<u8>,
    /// The keyboard backlight's number of distinct illumination steps,
    /// including "off" — the honest granularity of the hardware, probed from
    /// `/sys/class/leds/*::kbd_backlight/max_brightness` as `max + 1`
    /// (ADR-0168 amendment). `Some(n)` means the device exposes `n` discrete
    /// levels and chrome should present a stepped selector; `None` means the
    /// granularity is unknown or effectively continuous, so chrome presents a
    /// continuous control. Meaningless (and always `None`) when
    /// `kbd_brightness` is `None`; the probe is the single authority that
    /// upholds that pairing.
    ///
    /// Additive field (ADR-0140 precedent): a peer that predates it
    /// deserializes the same status without the key, so no protocol bump is
    /// required.
    #[cfg_attr(feature = "serde", serde(default))]
    pub kbd_brightness_levels: Option<u8>,
    pub do_not_disturb: bool,
    /// Included so one host probe can feed both status and settings surfaces.
    pub input: InputStatus,
    /// Included so one host snapshot can feed both status and settings surfaces.
    pub display: DisplayStatus,
    /// Session-owned "always on" idle inhibition — the derived legacy view
    /// of the session power mode (ADR-0140): true exactly when the mode
    /// disarms the automatic lock stage. Unlike the connection-scoped IPC
    /// inhibitors it survives client disconnects; the runtime owns and
    /// reconciles it.
    pub idle_inhibited: bool,
    /// Session power mode (ADR-0140): which idle stages stay armed. Like
    /// `idle_inhibited` this is compositor-owned runtime state; the host
    /// status poller must not clear it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub power_mode: PowerMode,
    /// Live compositor-owned capture streams (`StreamOutputStart`,
    /// ADR-0052). Chrome shows a persistent, non-interactive recording
    /// indicator while this is non-zero (ADR-0128). Compositor-owned like
    /// `idle_inhibited`: the host status poller must not clear it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub capture_streams: u32,
}

/// Machine form factor, for chrome that adapts its presentation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChassisKind {
    #[default]
    Desktop,
    Laptop,
}

/// One sample of host resource utilisation (polled separately from
/// [`SystemStatus`]).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceStats {
    /// Aggregate CPU usage in percent, 0..=100.
    pub cpu_percent: f32,
    /// Best-effort DRM busy percent; `None` when the driver exposes none.
    pub gpu_percent: Option<f32>,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub net_rx_bytes_per_sec: f64,
    pub net_tx_bytes_per_sec: f64,
    /// Usage of the filesystem mounted at `/`.
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    /// Static in practice; carried so one channel feeds the panel.
    pub chassis: ChassisKind,
}

impl Default for ResourceStats {
    fn default() -> Self {
        ResourceStats {
            cpu_percent: 0.0,
            gpu_percent: None,
            mem_used_bytes: 0,
            mem_total_bytes: 0,
            net_rx_bytes_per_sec: 0.0,
            net_tx_bytes_per_sec: 0.0,
            disk_used_bytes: 0,
            disk_total_bytes: 0,
            chassis: ChassisKind::default(),
        }
    }
}

/// An immediate live-system mutation.
///
/// Unlike [`crate::settings::SettingsAction`], applying one of these actions
/// does not imply that configuration was persisted.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemAction {
    ToggleMute,
    StepVolume {
        delta: i8,
    },
    SetVolume {
        level: u8,
    },
    SetBrightness {
        level: u8,
    },
    /// Set keyboard backlight level in percent (0..=100) (ADR-0168).
    SetKeyboardBrightness {
        level: u8,
    },
    /// Step keyboard backlight brightness through supported hardware tiers (ADR-0168).
    StepKeyboardBrightness,
    SetWifi {
        enabled: bool,
    },
    /// Request an active wireless network scan (ADR-0162).
    ScanWifi,
    /// Initiate a connection to a specified Wi-Fi network (ADR-0162).
    ConnectWifi {
        ssid: String,
        passphrase: Option<String>,
    },
    /// Disconnect from the currently associated Wi-Fi network (ADR-0162).
    DisconnectWifi,
    /// Discard saved credentials and configuration for a specified network (ADR-0167).
    ForgetWifi {
        ssid: String,
    },
    /// Set auto-connect behavior for a saved network (ADR-0167).
    SetWifiAutoConnect {
        ssid: String,
        auto_connect: bool,
    },
    SetBluetooth {
        enabled: bool,
    },
    /// Request an active Bluetooth discovery scan (ADR-0175).
    ScanBluetooth,
    /// Initiate or accept a connection to a peripheral by address (ADR-0175).
    ConnectBluetooth {
        address: String,
    },
    /// Tear down the ACL link to a peripheral by address (ADR-0175).
    DisconnectBluetooth {
        address: String,
    },
    /// Pair with a discovered peripheral by address (ADR-0175).
    PairBluetooth {
        address: String,
    },
    /// Remove host-side pairing keys for a peripheral by address (ADR-0175).
    ForgetBluetooth {
        address: String,
    },
    SetDoNotDisturb {
        enabled: bool,
    },
    /// Enable or disable physical scanout without changing the output
    /// topology. Used by the trusted idle policy after the session lock has
    /// been compositor-confirmed.
    SetOutputPower {
        powered: bool,
    },
    /// Hold or release the session-owned "always on" idle inhibitor (the
    /// command panel toggle). While held, idle notifications stay resumed:
    /// no automatic dimming, locking, or display power-off.
    ///
    /// Superseded by [`SystemAction::SetPowerMode`] (ADR-0140), which covers
    /// this shape as `Awake`/`Balanced`. Kept on the wire for older clients;
    /// the runtime maps it onto the mode.
    SetIdleInhibit {
        inhibit: bool,
    },
    /// Select the session power mode (ADR-0140): which idle stages stay
    /// armed. Session-scoped runtime state, not persisted; manual locking
    /// and lock-before-sleep are unaffected by every mode.
    SetPowerMode {
        mode: PowerMode,
    },
    /// Suspend the host system.
    Suspend,
    /// Reboot the host system.
    Reboot,
    /// Power off / shut down the host system.
    PowerOff,
}

impl SystemAction {
    /// Validate bounds shared by IPC and in-process callers.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::StepVolume { delta } if !(-100..=100).contains(delta) => {
                Err("volume step is outside -100..=100")
            }
            Self::SetVolume { level } if *level > 100 => Err("volume is outside 0..=100"),
            Self::SetBrightness { level } if !(1..=100).contains(level) => {
                Err("brightness is outside 1..=100")
            }
            Self::SetKeyboardBrightness { level } if *level > 100 => {
                Err("keyboard brightness is outside 0..=100")
            }
            Self::ConnectWifi { ssid, .. } if ssid.trim().is_empty() => Err("ssid cannot be empty"),
            Self::ForgetWifi { ssid } if ssid.trim().is_empty() => Err("ssid cannot be empty"),
            Self::SetWifiAutoConnect { ssid, .. } if ssid.trim().is_empty() => {
                Err("ssid cannot be empty")
            }
            Self::ConnectBluetooth { address }
            | Self::DisconnectBluetooth { address }
            | Self::PairBluetooth { address }
            | Self::ForgetBluetooth { address }
                if address.trim().is_empty() =>
            {
                Err("bluetooth address cannot be empty")
            }
            _ => Ok(()),
        }
    }
}

impl SystemStatus {
    /// The discrete illumination tiers of the keyboard backlight, as
    /// percentages, when the hardware exposes a *stepped* backlight
    /// (ADR-0168 amendment).
    ///
    /// Returns `None` when there is no keyboard backlight, when its granularity
    /// is unknown, or when the granularity is effectively continuous — in every
    /// such case chrome should present a continuous control (or nothing). When
    /// `Some`, the returned ladder has exactly
    /// [`kbd_brightness_levels`](Self::kbd_brightness_levels) rungs, evenly
    /// spaced from `0` to `100` inclusive, and is what a stepped selector
    /// renders.
    ///
    /// The threshold is deliberately conservative: real laptop backlights
    /// expose a handful of steps (ThinkPad commonly three, many others four),
    /// while a `max_brightness` of 255 is a fine-grained dimmer that deserves a
    /// fader, not a four-segment selector. This keeps the "stepped" decision
    /// honest instead of forcing every device onto a fixed ladder.
    pub fn kbd_brightness_tiers(&self) -> Option<Vec<u8>> {
        let levels = self.kbd_brightness_levels?;
        self.kbd_brightness?;
        if !(2..=KBD_BRIGHTNESS_MAX_STEPS).contains(&levels) {
            return None;
        }
        let last = levels - 1;
        Some(
            (0..levels)
                .map(|step| {
                    // Even spacing with the endpoints pinned: step 0 is 0%,
                    // step `last` is 100%, intermediates round to nearest.
                    ((step as u32 * 100 + last as u32 / 2) / last as u32) as u8
                })
                .collect(),
        )
    }

    /// The tier index a reported keyboard-backlight level belongs to: the
    /// nearest rung of [`kbd_brightness_tiers`](Self::kbd_brightness_tiers).
    ///
    /// Returns `0` when the device has no stepped ladder, so callers that only
    /// need an index never have to branch.
    pub fn kbd_brightness_tier_index(&self) -> usize {
        let Some(tiers) = self.kbd_brightness_tiers() else {
            return 0;
        };
        let level = self.kbd_brightness.unwrap_or(0);
        tiers
            .iter()
            .enumerate()
            .min_by_key(|(_, rung)| rung.abs_diff(level))
            .map(|(index, _)| index)
            .unwrap_or(0)
    }
}

/// Above this many distinct illumination steps, a keyboard backlight is treated
/// as a continuous dimmer rather than a stepped selector (ADR-0168 amendment).
/// Five covers the real stepped devices (2–4 rungs) with headroom, while
/// excluding fine-grained PWM dimmers (`max_brightness` in the tens to 255).
pub const KBD_BRIGHTNESS_MAX_STEPS: u8 = 5;

/// The documented default keyboard-backlight ladder (ADR-0168 §1): the standard
/// laptop illumination sequence, evenly spaced with the endpoints pinned. For
/// four rungs that is `0% → 33% → 67% → 100%` — the rung percentages use the
/// same rounding the host uses to normalize a raw `brightness`/`max_brightness`
/// reading (2/3 = 67%), so the fallback and a hardware-derived ladder agree.
///
/// This is the fallback the tier-stepping action uses when the host does not
/// report a stepped granularity. When [`SystemStatus::kbd_brightness_tiers`]
/// returns `Some`, that hardware-derived ladder supersedes this one.
pub const KBD_BRIGHTNESS_FALLBACK_TIERS: [u8; 4] = [0, 33, 67, 100];

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
        assert_eq!(status.wifi_state, WifiLinkState::Disabled);
        assert!(status.wifi_networks.is_empty());
        assert_eq!(status.bluetooth_enabled, None);
        assert_eq!(status.bluetooth_state, BluetoothLinkState::Disabled);
        assert!(status.bluetooth_devices.is_empty());
        assert_eq!(status.kbd_brightness_levels, None);
    }

    #[test]
    fn bluetooth_actions_validate_their_address() {
        for action in [
            SystemAction::ConnectBluetooth {
                address: "  ".to_string(),
            },
            SystemAction::DisconnectBluetooth {
                address: String::new(),
            },
            SystemAction::PairBluetooth {
                address: "\t".to_string(),
            },
            SystemAction::ForgetBluetooth {
                address: String::new(),
            },
        ] {
            assert!(
                action.validate().is_err(),
                "{action:?} must reject an empty address"
            );
        }
        assert!(SystemAction::ScanBluetooth.validate().is_ok());
        assert!(
            SystemAction::ConnectBluetooth {
                address: "AC:12:34:56:78:9A".to_string(),
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn keyboard_backlight_tiers_derive_from_the_reported_step_count() {
        // A three-rung backlight spaces evenly with pinned endpoints.
        let status = SystemStatus {
            kbd_brightness: Some(50),
            kbd_brightness_levels: Some(3),
            ..SystemStatus::default()
        };
        assert_eq!(status.kbd_brightness_tiers(), Some(vec![0, 50, 100]));
        assert_eq!(status.kbd_brightness_tier_index(), 1);

        // Four rungs reproduce the documented ADR-0168 ladder. The rung
        // percentages use the same rounding the host uses to normalize a raw
        // `brightness`/`max_brightness` reading, so a derived tier always equals
        // the percentage the hardware reports (2/3 = 67%, hence 67 not 66).
        let status = SystemStatus {
            kbd_brightness: Some(67),
            kbd_brightness_levels: Some(4),
            ..SystemStatus::default()
        };
        assert_eq!(status.kbd_brightness_tiers(), Some(vec![0, 33, 67, 100]));
        assert_eq!(status.kbd_brightness_tier_index(), 2);

        // A two-rung on/off backlight is the degenerate but valid case.
        let status = SystemStatus {
            kbd_brightness: Some(0),
            kbd_brightness_levels: Some(2),
            ..SystemStatus::default()
        };
        assert_eq!(status.kbd_brightness_tiers(), Some(vec![0, 100]));
        assert_eq!(status.kbd_brightness_tier_index(), 0);
    }

    #[test]
    fn keyboard_backlight_tiers_reject_fine_grained_and_absent_hardware() {
        // Fine-grained dimmers are continuous, not stepped.
        for levels in [KBD_BRIGHTNESS_MAX_STEPS + 1, 255] {
            let status = SystemStatus {
                kbd_brightness: Some(50),
                kbd_brightness_levels: Some(levels),
                ..SystemStatus::default()
            };
            assert_eq!(status.kbd_brightness_tiers(), None, "levels={levels}");
        }
        // A step count without a level is meaningless.
        let status = SystemStatus {
            kbd_brightness: None,
            kbd_brightness_levels: Some(4),
            ..SystemStatus::default()
        };
        assert_eq!(status.kbd_brightness_tiers(), None);
        // No hardware at all.
        assert_eq!(SystemStatus::default().kbd_brightness_tiers(), None);
    }

    #[test]
    fn wifi_action_validation_checks_empty_ssid() {
        assert!(
            SystemAction::ConnectWifi {
                ssid: "  ".to_string(),
                passphrase: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            SystemAction::ConnectWifi {
                ssid: "MyHome".to_string(),
                passphrase: Some("secret123".to_string()),
            }
            .validate()
            .is_ok()
        );
        assert!(
            SystemAction::ForgetWifi {
                ssid: "  ".to_string(),
            }
            .validate()
            .is_err()
        );
        assert!(
            SystemAction::ForgetWifi {
                ssid: "MyHome".to_string(),
            }
            .validate()
            .is_ok()
        );
        assert!(
            SystemAction::SetWifiAutoConnect {
                ssid: "  ".to_string(),
                auto_connect: true,
            }
            .validate()
            .is_err()
        );
        assert!(
            SystemAction::SetWifiAutoConnect {
                ssid: "MyHome".to_string(),
                auto_connect: false,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn action_validation_rejects_out_of_range_levels() {
        assert!(SystemAction::SetVolume { level: 101 }.validate().is_err());
        assert!(SystemAction::SetBrightness { level: 0 }.validate().is_err());
        assert!(
            SystemAction::SetBrightness { level: 100 }
                .validate()
                .is_ok()
        );
        assert!(
            SystemAction::SetKeyboardBrightness { level: 101 }
                .validate()
                .is_err()
        );
        assert!(
            SystemAction::SetKeyboardBrightness { level: 100 }
                .validate()
                .is_ok()
        );
        assert!(
            SystemAction::SetKeyboardBrightness { level: 0 }
                .validate()
                .is_ok()
        );
        assert!(SystemAction::StepKeyboardBrightness.validate().is_ok());
    }

    #[test]
    fn a_threshold_fires_once_per_discharge_cycle() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(21, false, &thresholds), None);
        assert_eq!(latches.poll(20, false, &thresholds), Some(20));
        assert_eq!(latches.poll(20, false, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), None);
    }

    #[test]
    fn one_alert_covers_a_fast_drop_across_several_thresholds() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(4, false, &thresholds), Some(5));
        // Both crossed thresholds latched: neither refires afterwards.
        assert_eq!(latches.poll(3, false, &thresholds), None);
        assert_eq!(latches.poll(1, false, &thresholds), None);
    }

    #[test]
    fn charging_clears_the_latches_for_the_next_cycle() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(19, false, &thresholds), Some(20));
        assert_eq!(latches.poll(30, true, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), Some(20));
    }

    #[test]
    fn recovery_rearms_only_the_recovered_thresholds() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(4, false, &thresholds), Some(5));
        // Recovering to 10% rearms only the 5% threshold: the 20% one stays
        // latched until the level climbs back above 20%.
        assert_eq!(latches.poll(10, false, &thresholds), None);
        assert_eq!(latches.poll(15, false, &thresholds), None);
        assert_eq!(latches.poll(4, false, &thresholds), Some(5));
        assert_eq!(latches.poll(25, false, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), Some(20));
    }

    #[test]
    fn no_alert_while_charging_even_below_every_threshold() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(3, true, &thresholds), None);
        // The charging sample cleared nothing and latched nothing: the next
        // discharge sample alerts normally.
        assert_eq!(latches.poll(3, false, &thresholds), Some(5));
    }

    #[test]
    fn empty_thresholds_disable_the_feature() {
        let mut latches = BatteryWarningLatches::default();
        assert_eq!(latches.poll(3, false, &[]), None);
        assert_eq!(latches.poll(1, false, &[]), None);
    }
}
