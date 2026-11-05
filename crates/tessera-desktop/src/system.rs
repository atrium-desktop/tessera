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

/// The wireless-display protocol a sink speaks (ADR-0188, ADR-0192).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CastProtocol {
    /// Miracast / Wi-Fi Display (WFD).
    #[default]
    Wfd,
}

/// What a sink can do, as verified by its backend.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastCapability {
    /// The sink can receive a mirrored screen (the only mode this project ships).
    ScreenMirroring,
    Audio,
    InputBackchannel,
    ContentProtection,
}

/// One discovered sink, with the reason it cannot be used when it cannot.
///
/// There is deliberately no `connected` boolean: a discovered sink is not a
/// session, and the old field let the control center claim a cast that did not
/// exist. A live stream is [`CastSession`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CastSink {
    /// Stable within one discovery run: the backend's peer identity.
    pub id: String,
    /// Human-readable name (e.g. "Living Room TV").
    #[cfg_attr(feature = "serde", serde(default))]
    pub name: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub protocol: CastProtocol,
    #[cfg_attr(feature = "serde", serde(default))]
    pub capabilities: Vec<CastCapability>,
    /// `None` means the sink is castable. A blocker names its cause and the
    /// operator action; it is never a generic "not found".
    #[cfg_attr(feature = "serde", serde(default))]
    pub blocker: Option<String>,
}

/// Whether the out-of-process cast service is reachable.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CastServiceState {
    /// No service answered; the reason is in `CastStatus::last_error`.
    #[default]
    Unavailable,
    Available,
}

/// What the cast service's discovery pass is doing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CastDiscovery {
    #[default]
    Idle,
    Scanning,
    Failed,
}

/// The lifecycle of one casting session.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastSessionState {
    /// The P2P group or the RTSP listener is being established.
    Connecting,
    /// M1–M7 is in flight.
    Negotiating,
    /// Media is flowing.
    Streaming,
    /// The session is tearing down.
    Stopping,
    /// The session ended in a named failure (`CastSession::failure`).
    Failed,
}

/// One live (or failed) casting session. The service owns it; chrome renders it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastSession {
    /// Service-assigned, stable for the session's lifetime.
    pub id: u64,
    pub sink_id: String,
    pub sink_name: String,
    pub state: CastSessionState,
    #[cfg_attr(feature = "serde", serde(default))]
    pub frames_sent: u64,
    /// The negotiated audio codec when audio is active, e.g. `AAC 00000001 00`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub audio: Option<String>,
    /// Present exactly when `state == Failed`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub failure: Option<String>,
}

/// The compositor-facing snapshot of the cast service (ADR-0188, ADR-0192).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CastStatus {
    #[cfg_attr(feature = "serde", serde(default))]
    pub service: CastServiceState,
    #[cfg_attr(feature = "serde", serde(default))]
    pub discovery: CastDiscovery,
    #[cfg_attr(feature = "serde", serde(default))]
    pub sinks: Vec<CastSink>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub sessions: Vec<CastSession>,
    /// Why the service or the last command failed, when it did.
    #[cfg_attr(feature = "serde", serde(default))]
    pub last_error: Option<String>,
}

impl CastStatus {
    /// Whether any session is currently rendering.
    pub fn is_casting(&self) -> bool {
        self.sessions
            .iter()
            .any(|session| session.state == CastSessionState::Streaming)
    }

    /// The first live session, if any.
    pub fn active_session(&self) -> Option<&CastSession> {
        self.sessions
            .iter()
            .find(|session| session.state != CastSessionState::Failed)
    }
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

/// Which physical or logical transport carries an audio output (ADR-0215).
///
/// Mapped from the sound server's node vocabulary into a daemon-neutral enum,
/// so presentation never sees PipeWire's `device.api` strings or node-name
/// prefixes (`[INV-NET-DAEMON-NEUTRAL]`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioOutputKind {
    #[default]
    Other,
    /// The machine's built-in speakers.
    Speaker,
    /// A wired analog output: headphones, a line-out, a headset jack.
    Wired,
    /// An HDMI/DisplayPort output on a GPU.
    Hdmi,
    /// A Bluetooth audio sink (A2DP, or a headset's HFP output).
    Bluetooth,
    /// A USB audio interface or DAC.
    Usb,
}

impl AudioOutputKind {
    /// The order presentation groups outputs in: what a user reaches for
    /// first (the machine itself, then wired, then radio) before the
    /// display-carried and external transports. Stable ordering keeps the
    /// list from reshuffling when the server reorders nodes.
    pub fn sort_rank(self) -> u8 {
        match self {
            Self::Speaker => 0,
            Self::Wired => 1,
            Self::Bluetooth => 2,
            Self::Usb => 3,
            Self::Hdmi => 4,
            Self::Other => 5,
        }
    }
}

/// One audio output the sound server can route playback to (ADR-0215).
///
/// Live service state, not configuration: the server owns the list, the
/// default, and every stream's link. The compositor only observes it and
/// forwards the user's intent back as a [`SystemAction::SetAudioOutput`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioOutput {
    /// The server's stable node identity (PipeWire `node.name`), e.g.
    /// `alsa_output.pci-0000_00_1f.3.analog-stereo`. This is the handle an
    /// intent names, because the numeric id is reassigned across restarts.
    pub name: String,
    /// Human-readable description for presentation, e.g. `Built-in Audio
    /// Analog Stereo` or a Bluetooth headset's alias.
    #[cfg_attr(feature = "serde", serde(default))]
    pub description: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: AudioOutputKind,
    /// True for the node the server routes new playback to.
    #[cfg_attr(feature = "serde", serde(default))]
    pub is_default: bool,
}

/// Which capture device an audio input is (ADR-0216).
///
/// The input-side counterpart of [`AudioOutputKind`]: mapped from the sound
/// server's node vocabulary inside the host probe, so presentation never sees
/// `device.api` strings or node-name prefixes (`[INV-NET-DAEMON-NEUTRAL]`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioInputKind {
    #[default]
    Other,
    /// The machine's own microphone array.
    BuiltIn,
    /// A wired headset or a microphone on a headset jack.
    Headset,
    /// A Bluetooth headset's microphone (HFP/HSP).
    Bluetooth,
    /// A USB microphone, interface, or webcam microphone.
    Usb,
}

impl AudioInputKind {
    /// The order presentation groups inputs in: the machine's own microphone
    /// first, then wired, then radio, then external transports — the same
    /// transport-first rhythm [`AudioOutputKind`] uses, so both pickers read
    /// alike.
    pub fn sort_rank(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::Headset => 1,
            Self::Bluetooth => 2,
            Self::Usb => 3,
            Self::Other => 4,
        }
    }
}

/// One audio input (capture device) the sound server exposes (ADR-0216).
///
/// Live service state, exactly like [`AudioOutput`]: the server owns the list
/// and the default. `muted` reports the *server's* per-node mute, which is what
/// a user meets as "this microphone is muted" in another mixer.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioInput {
    /// The server's stable node identity (PipeWire `node.name`), never a node
    /// number, which is reassigned on every server restart.
    pub name: String,
    /// Human-readable description for presentation.
    #[cfg_attr(feature = "serde", serde(default))]
    pub description: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: AudioInputKind,
    /// True for the node new capture streams are attached to.
    #[cfg_attr(feature = "serde", serde(default))]
    pub is_default: bool,
}

/// Anything the sound server addresses by a stable node name (ADR-0215,
/// ADR-0216).
///
/// The trait exists so the *handle discipline* — one name, no node numbers, no
/// ordinals — is expressed once and enforced identically for sinks and sources,
/// instead of being re-implemented (and eventually re-implemented differently)
/// per inventory.
pub trait AudioNode {
    /// The server's stable `node.name`.
    fn node_name(&self) -> &str;
}

impl AudioNode for AudioOutput {
    fn node_name(&self) -> &str {
        &self.name
    }
}

impl AudioNode for AudioInput {
    fn node_name(&self) -> &str {
        &self.name
    }
}

/// One coherent observation of live host and compositor-owned session state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemStatus {
    pub volume: Option<u8>,
    pub muted: bool,
    /// Every audio output the sound server exposes, ordered by
    /// [`AudioOutputKind::sort_rank`] and then by description, with the
    /// default first within its kind (ADR-0215). Empty when no sound server
    /// answered, which is how presentation tells "no outputs" from "outputs
    /// unknown".
    #[cfg_attr(feature = "serde", serde(default))]
    pub audio_outputs: Vec<AudioOutput>,
    /// Every audio input (capture device) the sound server exposes, ordered by
    /// [`AudioInputKind::sort_rank`] then default then name (ADR-0216). Empty
    /// when no sound server answered — the same honest reading as
    /// `audio_outputs`, not "no hardware".
    #[cfg_attr(feature = "serde", serde(default))]
    pub audio_inputs: Vec<AudioInput>,
    /// The default input's capture volume, or `None` without a sound server.
    /// Deliberately `None` rather than `Some(0)`: absent is not silent.
    #[cfg_attr(feature = "serde", serde(default))]
    pub microphone_volume: Option<u8>,
    /// The default input's mute state as the server reports it (ADR-0216).
    #[cfg_attr(feature = "serde", serde(default))]
    pub microphone_muted: bool,
    /// Whether any client is currently capturing audio (ADR-0216).
    ///
    /// Observed, never inferred from chrome's own state: a browser tab, a
    /// recorder, or an agent session all show up the same way. This is what
    /// makes the microphone indicator honest — it reports the sound server's
    /// link graph, not a request the desktop hoped for.
    #[cfg_attr(feature = "serde", serde(default))]
    pub microphone_active: bool,
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
    /// Live virtual extend outputs (`OutputKind::Virtual`, ADR-0186). Chrome
    /// shows how many extended displays exist (ADR-0188). Compositor-owned
    /// like `capture_streams`: the host status poller must not clear it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub virtual_displays: u32,
    /// Live streams rendering a virtual extend output (the extended-display
    /// "casting" streams, as distinct from mirror or window capture,
    /// ADR-0188). Compositor-owned like `capture_streams`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub cast_streams: u32,
    /// Live casting domain state (ADR-0188, ADR-0192). The out-of-process cast
    /// service owns discovery and sessions; the compositor publishes its
    /// snapshot and never infers a session from network or window state.
    #[cfg_attr(feature = "serde", serde(default))]
    pub cast: CastStatus,
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

/// Longest audio-output node name a [`SystemAction::SetAudioOutput`] may
/// carry (ADR-0215). PipeWire node names are well under this; the bound keeps
/// a peer from pushing an unbounded string through the journal and chrome.
pub const MAX_AUDIO_OUTPUT_NAME: usize = 256;

/// Longest audio-input node name a [`SystemAction::SetAudioInput`] may carry
/// (ADR-0216). Same bound and same reason as [`MAX_AUDIO_OUTPUT_NAME`]: the
/// two pickers must not disagree about what a name may look like.
pub const MAX_AUDIO_INPUT_NAME: usize = MAX_AUDIO_OUTPUT_NAME;

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
    /// Route new playback to a named audio output, by the server's node name
    /// (ADR-0215). This is an immediate service mutation: it changes the
    /// session's live routing, not saved configuration.
    SetAudioOutput {
        name: String,
    },
    /// Route new capture streams to the named audio input, by the server's
    /// node name (ADR-0216). The input-side counterpart of
    /// [`SystemAction::SetAudioOutput`], with the same handle discipline: the
    /// server's stable node name, never a node number.
    SetAudioInput {
        name: String,
    },
    /// Set the default input's capture volume percentage (ADR-0216).
    SetMicrophoneVolume {
        level: u8,
    },
    /// Toggle the default input's mute state on the server (ADR-0216).
    ///
    /// Distinct from [`SystemAction::ToggleMute`], which is the *output*
    /// fader: muting a microphone is a privacy action and must never be
    /// reachable by the control that silences playback.
    ToggleMicrophoneMute,
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
    /// Request an active wireless display discovery scan (ADR-0188, ADR-0192).
    ScanCast,
    /// Stop the cast service's discovery pass. The service owns the radio; the
    /// surface that started discovery releases it when it closes.
    StopCastDiscovery,
    /// Initiate a session with a discovered sink, by its service-assigned id
    /// (ADR-0188, ADR-0192). The cast service owns the session.
    ConnectCast {
        sink: String,
    },
    /// End a live casting session by its service-assigned id (ADR-0188,
    /// ADR-0192). Never by process name: the service owns the session.
    DisconnectCast {
        session: u64,
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
            Self::SetAudioOutput { name } if name.trim().is_empty() => {
                Err("audio output name cannot be empty")
            }
            Self::SetAudioOutput { name } if name.len() > MAX_AUDIO_OUTPUT_NAME => {
                Err("audio output name is too long")
            }
            Self::SetAudioInput { name } if name.trim().is_empty() => {
                Err("audio input name cannot be empty")
            }
            Self::SetAudioInput { name } if name.len() > MAX_AUDIO_INPUT_NAME => {
                Err("audio input name is too long")
            }
            Self::SetMicrophoneVolume { level } if *level > 100 => {
                Err("microphone volume is outside 0..=100")
            }
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
            Self::ConnectCast { sink } if sink.trim().is_empty() => {
                Err("cast sink id cannot be empty")
            }
            _ => Ok(()),
        }
    }
}

impl SystemStatus {
    /// The default audio output, or `None` when no server answered (ADR-0215).
    pub fn default_audio_output(&self) -> Option<&AudioOutput> {
        self.audio_outputs.iter().find(|output| output.is_default)
    }

    /// The default audio input, or `None` when no server answered (ADR-0216).
    pub fn default_audio_input(&self) -> Option<&AudioInput> {
        self.audio_inputs.iter().find(|input| input.is_default)
    }

    /// Whether the microphone picker is worth offering: at least two inputs.
    ///
    /// The rule mirrors [`audio_outputs`](Self::audio_outputs) exactly, and for
    /// the same reason: an empty inventory means no sound server answered, and
    /// a single input leaves nothing to choose, so a disclosure would open a
    /// page that cannot change anything.
    pub fn has_audio_input_choice(&self) -> bool {
        self.audio_inputs.len() > 1
    }

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
