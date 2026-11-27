//! Tessera's chrome glyph set (ADR-0262).
//!
//! Since Optics v0.0.88 (Optics ADR-0174) optics-ui bundles no icon pack:
//! only the four chevrons stay built in, and every other glyph is vector
//! geometry the host registers on each `optics_ui` context. Tessera owns the
//! glyphs its chrome draws here, as precompiled 24×24 stroke commands
//! ([`glyphs`]), so nothing parses SVG at runtime.
//!
//! Registration is per context and must happen before anything else
//! registers icons on it: [`install`] registers every [`Glyph`] in
//! declaration order right after the context is created. A fresh registry
//! hands out slots sequentially, so every context Tessera installs into
//! assigns the same ids, and [`Glyph::id`] can resolve a glyph without a
//! handle to the context. [`install`] verifies that and reports a context
//! whose ids diverge rather than letting it draw the wrong glyphs.

mod glyphs;

use std::sync::OnceLock;

use optics_ui::Ui;
use optics_ui::sys::{self, optics_ui_icon_id};

/// A Tessera chrome glyph. Declaration order is registration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Glyph {
    AlertTriangle,
    BatteryCharging,
    Bell,
    BellOff,
    Bluetooth,
    Cast,
    Check,
    Crop,
    Edit,
    Eye,
    FileText,
    Grid,
    Headphones,
    Image,
    Info,
    Key,
    Layout,
    Lock,
    Mic,
    MicOff,
    Monitor,
    Moon,
    MousePointer,
    Pause,
    Play,
    Power,
    RefreshCw,
    RotateCw,
    Search,
    Settings,
    Shield,
    SkipBack,
    SkipForward,
    Sliders,
    Speaker,
    Sun,
    Users,
    Volume1,
    Volume2,
    VolumeX,
    Wifi,
    X,
    Zap,
    /// The Bluetooth rune struck through: the radio-off mark (ADR-0175).
    BluetoothOff,
    /// A keyboard with three illumination rays: the keyboard backlight.
    KeyboardBacklight,
}

impl Glyph {
    /// Every glyph, in registration order.
    pub const ALL: [Glyph; 45] = [
        Glyph::AlertTriangle,
        Glyph::BatteryCharging,
        Glyph::Bell,
        Glyph::BellOff,
        Glyph::Bluetooth,
        Glyph::Cast,
        Glyph::Check,
        Glyph::Crop,
        Glyph::Edit,
        Glyph::Eye,
        Glyph::FileText,
        Glyph::Grid,
        Glyph::Headphones,
        Glyph::Image,
        Glyph::Info,
        Glyph::Key,
        Glyph::Layout,
        Glyph::Lock,
        Glyph::Mic,
        Glyph::MicOff,
        Glyph::Monitor,
        Glyph::Moon,
        Glyph::MousePointer,
        Glyph::Pause,
        Glyph::Play,
        Glyph::Power,
        Glyph::RefreshCw,
        Glyph::RotateCw,
        Glyph::Search,
        Glyph::Settings,
        Glyph::Shield,
        Glyph::SkipBack,
        Glyph::SkipForward,
        Glyph::Sliders,
        Glyph::Speaker,
        Glyph::Sun,
        Glyph::Users,
        Glyph::Volume1,
        Glyph::Volume2,
        Glyph::VolumeX,
        Glyph::Wifi,
        Glyph::X,
        Glyph::Zap,
        Glyph::BluetoothOff,
        Glyph::KeyboardBacklight,
    ];

    /// The Bluetooth glyph for a radio that is on (`true`) or off.
    #[must_use]
    pub fn bluetooth(enabled: bool) -> Glyph {
        if enabled {
            Glyph::Bluetooth
        } else {
            Glyph::BluetoothOff
        }
    }

    /// The optics-ui icon id this glyph was registered under, or
    /// `OPTICS_UI_ICON_NONE` (draws nothing) before [`install`] ran.
    #[must_use]
    pub fn id(self) -> optics_ui_icon_id {
        IDS.get()
            .map_or(optics_ui_icon_id::OPTICS_UI_ICON_NONE, |ids| {
                ids[self as usize]
            })
    }

    fn commands(self) -> &'static [glyphs::Cmd] {
        match self {
            Glyph::AlertTriangle => glyphs::ALERT_TRIANGLE,
            Glyph::BatteryCharging => glyphs::BATTERY_CHARGING,
            Glyph::Bell => glyphs::BELL,
            Glyph::BellOff => glyphs::BELL_OFF,
            Glyph::Bluetooth => glyphs::BLUETOOTH,
            Glyph::Cast => glyphs::CAST,
            Glyph::Check => glyphs::CHECK,
            Glyph::Crop => glyphs::CROP,
            Glyph::Edit => glyphs::EDIT,
            Glyph::Eye => glyphs::EYE,
            Glyph::FileText => glyphs::FILE_TEXT,
            Glyph::Grid => glyphs::GRID,
            Glyph::Headphones => glyphs::HEADPHONES,
            Glyph::Image => glyphs::IMAGE,
            Glyph::Info => glyphs::INFO,
            Glyph::Key => glyphs::KEY,
            Glyph::Layout => glyphs::LAYOUT,
            Glyph::Lock => glyphs::LOCK,
            Glyph::Mic => glyphs::MIC,
            Glyph::MicOff => glyphs::MIC_OFF,
            Glyph::Monitor => glyphs::MONITOR,
            Glyph::Moon => glyphs::MOON,
            Glyph::MousePointer => glyphs::MOUSE_POINTER,
            Glyph::Pause => glyphs::PAUSE,
            Glyph::Play => glyphs::PLAY,
            Glyph::Power => glyphs::POWER,
            Glyph::RefreshCw => glyphs::REFRESH_CW,
            Glyph::RotateCw => glyphs::ROTATE_CW,
            Glyph::Search => glyphs::SEARCH,
            Glyph::Settings => glyphs::SETTINGS,
            Glyph::Shield => glyphs::SHIELD,
            Glyph::SkipBack => glyphs::SKIP_BACK,
            Glyph::SkipForward => glyphs::SKIP_FORWARD,
            Glyph::Sliders => glyphs::SLIDERS,
            Glyph::Speaker => glyphs::SPEAKER,
            Glyph::Sun => glyphs::SUN,
            Glyph::Users => glyphs::USERS,
            Glyph::Volume1 => glyphs::VOLUME_1,
            Glyph::Volume2 => glyphs::VOLUME_2,
            Glyph::VolumeX => glyphs::VOLUME_X,
            Glyph::Wifi => glyphs::WIFI,
            Glyph::X => glyphs::X,
            Glyph::Zap => glyphs::ZAP,
            Glyph::BluetoothOff => glyphs::BLUETOOTH_OFF,
            Glyph::KeyboardBacklight => glyphs::KEYBOARD_BACKLIGHT,
        }
    }
}

impl From<Glyph> for optics_ui_icon_id {
    fn from(glyph: Glyph) -> Self {
        glyph.id()
    }
}

/// Ids assigned by the first context [`install`] registered into.
static IDS: OnceLock<[optics_ui_icon_id; Glyph::ALL.len()]> = OnceLock::new();

/// Why [`install`] could not make [`Glyph::id`] valid for a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// optics-ui rejected a glyph's geometry or ran out of memory.
    Rejected(Glyph),
    /// The context assigned a different id than earlier contexts did: it
    /// already had icons registered before [`install`] ran.
    Diverged {
        glyph: Glyph,
        expected: u32,
        actual: u32,
    },
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(glyph) => write!(f, "optics-ui rejected the {glyph:?} glyph"),
            Self::Diverged {
                glyph,
                expected,
                actual,
            } => write!(
                f,
                "{glyph:?} registered as icon {actual:#x}, expected {expected:#x}: \
                 install glyphs before registering any other icon"
            ),
        }
    }
}

impl std::error::Error for InstallError {}

fn command(&(kind, params): &glyphs::Cmd) -> sys::optics_ui_icon_cmd {
    use sys::optics_ui_icon_cmd_kind as k;
    let kind = match kind {
        0 => k::OPTICS_UI_ICON_CMD_MOVE,
        1 => k::OPTICS_UI_ICON_CMD_LINE,
        2 => k::OPTICS_UI_ICON_CMD_CUBIC,
        3 => k::OPTICS_UI_ICON_CMD_QUAD,
        4 => k::OPTICS_UI_ICON_CMD_CLOSE,
        5 => k::OPTICS_UI_ICON_CMD_CIRCLE,
        _ => k::OPTICS_UI_ICON_CMD_RECT,
    };
    sys::optics_ui_icon_cmd {
        kind,
        _pad: [0; 3],
        params,
    }
}

/// Register every [`Glyph`] on a freshly created `ui` context.
///
/// Call it once per context, before anything else registers an icon there.
pub fn install(ui: &mut Ui) -> Result<(), InstallError> {
    let mut ids = [optics_ui_icon_id::OPTICS_UI_ICON_NONE; Glyph::ALL.len()];
    for glyph in Glyph::ALL {
        let cmds: Vec<sys::optics_ui_icon_cmd> = glyph.commands().iter().map(command).collect();
        ids[glyph as usize] = ui
            .register_icon(
                &cmds,
                &[],
                sys::optics_ui_icon_paint::OPTICS_UI_ICON_PAINT_STROKE,
            )
            .ok_or(InstallError::Rejected(glyph))?;
    }
    let expected = IDS.get_or_init(|| ids);
    for glyph in Glyph::ALL {
        let (want, got) = (expected[glyph as usize], ids[glyph as usize]);
        if want != got {
            return Err(InstallError::Diverged {
                glyph,
                expected: want.0,
                actual: got.0,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_glyph_in_declaration_order() {
        for (index, glyph) in Glyph::ALL.iter().enumerate() {
            assert_eq!(
                *glyph as usize, index,
                "{glyph:?} is out of order in Glyph::ALL"
            );
            assert!(!glyph.commands().is_empty(), "{glyph:?} has no geometry");
        }
    }

    #[test]
    fn glyph_commands_stay_inside_the_icon_box() {
        for glyph in Glyph::ALL {
            for &(kind, params) in glyph.commands() {
                assert!(kind <= 6, "{glyph:?} has unknown command kind {kind}");
                for value in params {
                    assert!(
                        value.is_finite() && (-1.0..=25.0).contains(&value),
                        "{glyph:?} has a coordinate outside the 24x24 box: {value}"
                    );
                }
            }
        }
    }
}
