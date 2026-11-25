//! Versionable desktop-settings model shared by chrome, IPC, and clients.
//!
//! These types describe compositor-owned persistent settings. Instant system
//! controls such as volume and Wi-Fi are intentionally absent: their source
//! of truth is the corresponding system service, not the compositor config.

use crate::output::{ModeSpec, OutputInfo};
use tessera_primitives::Point;
use tessera_primitives::input::InputStatus;

/// Desktop-wide color-scheme preference, using the freedesktop Settings
/// portal vocabulary.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorScheme {
    /// No preference; applications choose their own default.
    #[default]
    System,
    /// Prefer a dark appearance.
    Dark,
    /// Prefer a light appearance.
    Light,
}

impl ColorScheme {
    /// Resolve `System` to the compositor's built-in fallback appearance.
    ///
    /// The shell ships one concrete appearance per explicit scheme; until
    /// platform-level preference detection exists, "no preference" means the
    /// dark appearance, so an unset `color_scheme` never reaches the render
    /// side as an undecided value.
    #[must_use]
    pub fn or_dark(self) -> Self {
        match self {
            Self::System => Self::Dark,
            explicit => explicit,
        }
    }
}

/// Desktop-wide contrast preference.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Contrast {
    #[default]
    Normal,
    High,
}

/// The material every chrome surface is drawn in (ADR-0252).
///
/// One material applies to the whole shell at once — the dock, the Control
/// Center, notifications, alerts and menus never mix materials, because a
/// desktop whose surfaces disagree about what they are made of reads as
/// broken rather than layered.
///
/// Variants are declared cheapest first and the derived ordering is the cost
/// ordering: a [`MaterialCeiling`] caps a preference by taking the minimum.
/// Every variant down the ordering must therefore be strictly cheaper to draw
/// and no harder to read than the one above it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Material {
    /// A fully opaque neutral body. One rounded-rect fill; nothing beneath
    /// the surface is read or blended.
    #[default]
    Opaque,
    /// A uniformly translucent body: one alpha-blended fill, which the
    /// fixed-function blend stage does for free. Content beneath shows
    /// through unblurred.
    Translucent,
    /// A blurred and tinted capture of the content beneath the surface.
    Frosted,
    /// A refracting lens body over a full-resolution capture.
    LiquidGlass,
}

impl Material {
    /// Every material, cheapest first.
    pub const ALL: [Material; 4] = [
        Material::Opaque,
        Material::Translucent,
        Material::Frosted,
        Material::LiquidGlass,
    ];

    /// Whether drawing this material requires the compositor to capture and
    /// filter the scene beneath the surface. The materials below this line
    /// are painted values; the ones above it cost a backdrop pass.
    #[must_use]
    pub fn samples_backdrop(self) -> bool {
        matches!(self, Material::Frosted | Material::LiquidGlass)
    }

    /// The configuration spelling, as written under `[appearance]`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Material::Opaque => "opaque",
            Material::Translucent => "translucent",
            Material::Frosted => "frosted",
            Material::LiquidGlass => "liquid-glass",
        }
    }
}

/// Why a [`MaterialCeiling`] applies. Carried so the effective material can
/// be explained (in the log and in Settings) instead of silently differing
/// from what the user chose.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialCeilingReason {
    /// The renderer cannot draw anything above this material.
    Renderer,
    /// High contrast is on: a surface must not let content show through it.
    HighContrast,
    /// Running on battery ([`MaterialPowerPolicy::on_battery`]).
    OnBattery,
    /// The battery is at or below [`MaterialPowerPolicy::low_battery_percent`]
    /// and not charging.
    LowBattery,
    /// The platform power profile asks for low power.
    PowerSaver,
}

/// The automatic power rules of the material policy (ADR-0252 power ceilings).
///
/// Each field is a ceiling: the most expensive material drawn while that state
/// holds. They can only lower the user's preference, never raise it, so a
/// rule set to `liquid-glass` (the most expensive material) is simply "no
/// limit".
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialPowerPolicy {
    /// While on battery. Default: no limit.
    pub on_battery: Material,
    /// While the battery is at or below `low_battery_percent` and not
    /// charging. Default: translucent, so a draining laptop stops paying for
    /// a backdrop capture.
    pub low_battery: Material,
    /// The low-battery line, 1..=99. Default 20, the first default
    /// `[battery] warn_at` threshold.
    pub low_battery_percent: u8,
    /// While the platform power profile is power-saver. Default: translucent.
    pub power_saver: Material,
}

impl Default for MaterialPowerPolicy {
    fn default() -> Self {
        Self {
            on_battery: Material::LiquidGlass,
            low_battery: Material::Translucent,
            low_battery_percent: 20,
            power_saver: Material::Translucent,
        }
    }
}

impl MaterialPowerPolicy {
    /// The ceilings this policy puts in force for `power` and `battery`.
    #[must_use]
    pub fn ceilings(
        &self,
        power: crate::system::PowerState,
        battery: Option<crate::system::BatteryStatus>,
    ) -> Vec<MaterialCeiling> {
        let mut ceilings = Vec::new();
        if power.on_battery {
            ceilings.push(MaterialCeiling {
                max: self.on_battery,
                reason: MaterialCeilingReason::OnBattery,
            });
        }
        if let Some(battery) = battery
            && !battery.charging
            && power.on_battery
            && battery.percent <= self.low_battery_percent
        {
            ceilings.push(MaterialCeiling {
                max: self.low_battery,
                reason: MaterialCeilingReason::LowBattery,
            });
        }
        if power.power_saver {
            ceilings.push(MaterialCeiling {
                max: self.power_saver,
                reason: MaterialCeilingReason::PowerSaver,
            });
        }
        ceilings
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1..=99).contains(&self.low_battery_percent) {
            return Err("material low-battery percentage is outside 1..=99");
        }
        Ok(())
    }
}

/// An upper bound on the material the shell may draw right now.
///
/// The switching policy is "the user's preference, capped by every ceiling
/// in force": [`resolve_material`]. Manual choice is the preference; every
/// automatic rule — the renderer's capability, accessibility, and later the
/// power source or a low-battery state — is expressed as a ceiling, so an
/// automatic rule can only ever make the desktop cheaper and plainer than the
/// user asked for, never more expensive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialCeiling {
    pub max: Material,
    pub reason: MaterialCeilingReason,
}

/// The outcome of [`resolve_material`]: what will be drawn and, when that
/// differs from the preference, which ceiling decided it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMaterial {
    pub preferred: Material,
    pub effective: Material,
    /// The ceiling that lowered the preference, `None` when the preference
    /// is drawn as chosen. When several ceilings bind, the lowest one is
    /// reported, since it is the one that decided the outcome.
    pub limited_by: Option<MaterialCeilingReason>,
}

/// Resolve a material preference against the ceilings currently in force.
#[must_use]
pub fn resolve_material(
    preferred: Material,
    ceilings: impl IntoIterator<Item = MaterialCeiling>,
) -> ResolvedMaterial {
    let mut resolved = ResolvedMaterial {
        preferred,
        effective: preferred,
        limited_by: None,
    };
    for ceiling in ceilings {
        if ceiling.max < resolved.effective {
            resolved.effective = ceiling.max;
            resolved.limited_by = Some(ceiling.reason);
        }
    }
    resolved
}

/// An sRGB accent color stored without an alpha channel.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl AccentColor {
    /// Parse the canonical configuration representation (`#RRGGBB`).
    pub fn parse_hex(value: &str) -> Result<Self, &'static str> {
        let bytes = value.as_bytes();
        if bytes.len() != 7 || bytes[0] != b'#' {
            return Err("accent color must use #RRGGBB");
        }
        let component = |start| {
            u8::from_str_radix(&value[start..start + 2], 16)
                .map_err(|_| "accent color must use #RRGGBB")
        };
        Ok(Self {
            red: component(1)?,
            green: component(3)?,
            blue: component(5)?,
        })
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }

    /// Components normalized to the Settings portal `(ddd)` representation.
    pub fn normalized(self) -> (f64, f64, f64) {
        (
            f64::from(self.red) / 255.0,
            f64::from(self.green) / 255.0,
            f64::from(self.blue) / 255.0,
        )
    }
}

/// One concrete snapshot of all compositor-owned desktop preferences.
///
/// Configuration may omit values and explicit startup environment overrides
/// may replace them, but consumers only see this resolved representation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopPreferences {
    pub color_scheme: ColorScheme,
    pub accent_color: Option<AccentColor>,
    pub contrast: Contrast,
    /// The chrome material the user prefers. What is drawn is this value
    /// after [`resolve_material`]; see [`Material`].
    #[cfg_attr(feature = "serde", serde(default))]
    pub material: Material,
    /// The automatic power ceilings on `material` (ADR-0252 power ceilings).
    #[cfg_attr(feature = "serde", serde(default))]
    pub material_power: MaterialPowerPolicy,
    pub reduced_motion: bool,
    pub font_name: String,
    pub monospace_font_name: String,
    pub text_scale: f64,
    pub icon_theme: String,
    pub cursor_theme: String,
    pub cursor_size: u32,
    #[cfg_attr(feature = "serde", serde(default))]
    pub accessibility: tessera_primitives::accessibility::AccessibilityConfig,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::System,
            accent_color: None,
            contrast: Contrast::Normal,
            material: Material::Opaque,
            material_power: MaterialPowerPolicy::default(),
            reduced_motion: false,
            font_name: "Sans 10".into(),
            monospace_font_name: "Monospace 10".into(),
            text_scale: 1.0,
            icon_theme: "hicolor".into(),
            cursor_theme: "default".into(),
            cursor_size: 24,
            accessibility: tessera_primitives::accessibility::AccessibilityConfig::default(),
        }
    }
}

impl DesktopPreferences {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.text_scale.is_finite() || !(0.5..=3.0).contains(&self.text_scale) {
            return Err("text scale is outside 0.5..=3.0");
        }
        if !(8..=128).contains(&self.cursor_size) {
            return Err("cursor size is outside 8..=128");
        }
        self.material_power.validate()?;
        for value in [
            &self.font_name,
            &self.monospace_font_name,
            &self.icon_theme,
            &self.cursor_theme,
        ] {
            if value.trim().is_empty() || value.len() > 256 {
                return Err("desktop preference string is empty or too long");
            }
        }
        let zoom = self.accessibility.visual.zoom_factor;
        if !zoom.is_finite() || !(1.0..=10.0).contains(&zoom) {
            return Err("accessibility zoom factor is outside 1.0..=10.0");
        }
        Ok(())
    }
}

/// Staged inactivity policy owned by the Tessera session.
///
/// A timeout of zero disables that individual stage. Keeping inactive values
/// in the snapshot lets System Settings disable automatic idle handling and
/// later restore it without discarding the user's preferred timings.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleSettings {
    /// Whether inactivity notifications may trigger the configured stages.
    /// Explicit locking and lock-before-sleep remain available when false.
    pub enabled: bool,
    pub dim_after_seconds: u32,
    pub lock_after_seconds: u32,
    pub display_off_after_seconds: u32,
    pub suspend_after_seconds: u32,
    /// Backlight level used by the dim stage, as a percentage.
    pub dim_percent: u8,
}

impl Default for IdleSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            dim_after_seconds: 5 * 60,
            lock_after_seconds: 10 * 60,
            display_off_after_seconds: 11 * 60,
            suspend_after_seconds: 30 * 60,
            dim_percent: 30,
        }
    }
}

impl IdleSettings {
    /// A generous upper bound that still catches a seconds/milliseconds typo
    /// before it reaches the Wayland protocol.
    pub const MAX_TIMEOUT_SECONDS: u32 = 7 * 24 * 60 * 60;

    pub fn validate(self) -> Result<(), &'static str> {
        if !(1..=100).contains(&self.dim_percent) {
            return Err("idle dim percentage is outside 1..=100");
        }
        let stages = [
            self.dim_after_seconds,
            self.lock_after_seconds,
            self.display_off_after_seconds,
            self.suspend_after_seconds,
        ];
        if stages
            .into_iter()
            .any(|seconds| seconds > Self::MAX_TIMEOUT_SECONDS)
        {
            return Err("idle timeout is longer than seven days");
        }
        if (self.display_off_after_seconds != 0 || self.suspend_after_seconds != 0)
            && self.lock_after_seconds == 0
        {
            return Err("locking must be enabled before display power-off or suspend");
        }
        let mut previous = 0;
        for current in stages.into_iter().filter(|seconds| *seconds != 0) {
            if current <= previous {
                return Err("enabled idle stages must be strictly increasing");
            }
            previous = current;
        }
        Ok(())
    }
}

/// Low-battery warning thresholds, as discharge percentages.
///
/// Each threshold raises the compositor's modal low-battery alert once per
/// discharge cycle; an empty list disables the feature.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatterySettings {
    /// Percentages that raise the alert, highest first.
    pub warn_at: Vec<u8>,
}

impl Default for BatterySettings {
    fn default() -> Self {
        Self {
            warn_at: vec![20, 5],
        }
    }
}

impl BatterySettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        let mut previous = 100;
        for threshold in &self.warn_at {
            if !(1..=99).contains(threshold) {
                return Err("battery warning percentage is outside 1..=99");
            }
            if *threshold >= previous {
                return Err("battery warning percentages must be strictly descending");
            }
            previous = *threshold;
        }
        Ok(())
    }
}

/// Live display capabilities and current effective configuration.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayStatus {
    /// Whether this session owns physical display configuration.
    pub configurable: bool,
    /// Session default for connectors without an explicit role.
    pub topology: crate::output::DisplayTopology,
    pub outputs: Vec<OutputInfo>,
    /// Last persistence/application failure, cleared by a successful edit.
    pub error: Option<String>,
    /// Seconds left before a tentative display edit is automatically reverted.
    pub pending_revert_seconds: Option<u32>,
}

/// Compositor-owned dock preferences exposed to System Settings.
///
/// Pinning and dock position stay dock-context-menu edits; this struct
/// carries the settings-panel surface of the `[dock]` configuration table.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, deny_unknown_fields))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DockSettings {
    /// The animation played when a window minimizes into the dock.
    pub minimize_animation: crate::dock::MinimizeAnimationStyle,
}

/// One complete, validated output edit (ADR-0243).
///
/// A mirrored output has no independent scale or position: both are `None`
/// and follow the source. Its optional `mode` selects the mirror connector's
/// own native scanout mode. An extended output sends its explicit geometry.
/// `role: None` clears an explicit role and returns the connector to the
/// `[display] topology` policy; `Some` pins the role.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct DisplaySettings {
    pub connector: String,
    /// `None` restores the automatic topology policy; `Some` pins the role.
    pub role: Option<crate::output::OutputRole>,
    pub mode: Option<ModeSpec>,
    pub scale: Option<f64>,
    pub position: Option<Point>,
    pub primary: bool,
    /// Apply as a preview: the compositor starts a revert timer and only a
    /// matching [`SettingsAction::ConfirmDisplay`] keeps the change.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tentative: bool,
}

impl DisplaySettings {
    /// Validate transport-level shape. Hardware membership is checked by the
    /// compositor main loop against the live connector list.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.connector.trim().is_empty() || self.connector.len() > 128 {
            return Err("display connector is empty or too long");
        }
        match &self.role {
            None | Some(crate::output::OutputRole::Extend) => {}
            Some(crate::output::OutputRole::Mirror { source }) => {
                if source.trim().is_empty() || source.len() > 128 {
                    return Err("mirror source is empty or too long");
                }
                if source == &self.connector {
                    return Err("an output cannot mirror itself");
                }
                if self.primary {
                    return Err("a mirrored output cannot be primary");
                }
                if self.scale.is_some() || self.position.is_some() {
                    return Err("a mirrored output follows its source's scale and position");
                }
            }
        }
        if let Some(scale) = self.scale
            && (!scale.is_finite() || !(0.25..=4.0).contains(&scale))
        {
            return Err("display scale is outside 0.25..=4.0");
        }
        if let Some(mode) = self.mode
            && (mode.width <= 0
                || mode.height <= 0
                || mode.width > 32_768
                || mode.height > 32_768
                || mode.refresh_hz.is_some_and(|hz| !(1..=1_000).contains(&hz)))
        {
            return Err("display mode is outside the supported range");
        }
        Ok(())
    }
}

/// User-facing desktop wallpaper configuration.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperSettings {
    pub mode: String,
    pub source: Option<String>,
}

impl Default for WallpaperSettings {
    fn default() -> Self {
        Self {
            mode: "image".into(),
            source: None,
        }
    }
}

/// Coherent persistent-settings snapshot. `revision` changes after every
/// accepted mutation and lets clients reject stale drafts.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsSnapshot {
    pub revision: u64,
    pub input: InputStatus,
    pub display: DisplayStatus,
    pub preferences: DesktopPreferences,
    pub idle: IdleSettings,
    pub dock: DockSettings,
    pub wallpaper: WallpaperSettings,
    /// What the shell draws right now: `preferences.material` after every
    /// ceiling in force, and the one that decided it (ADR-0252). Live state,
    /// not a setting: it moves with the power source without a revision.
    #[cfg_attr(feature = "serde", serde(default))]
    pub material: Option<ResolvedMaterial>,
}

/// One typed persistent-settings transaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum SettingsAction {
    SetInput {
        config: tessera_primitives::input::InputConfig,
    },
    SetDisplay {
        settings: DisplaySettings,
    },
    SetDisplayTopology {
        topology: crate::output::DisplayTopology,
    },
    /// Keep the most recent tentative display edit.
    ConfirmDisplay,
    SetDesktopPreferences {
        preferences: DesktopPreferences,
    },
    SetIdle {
        settings: IdleSettings,
    },
    SetDock {
        settings: DockSettings,
    },
    SetWallpaper {
        settings: WallpaperSettings,
    },
}

impl SettingsAction {
    /// Validate transport-level bounds. Hardware capability checks and mode
    /// membership remain authoritative main-loop work.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::SetInput { config } => {
                if !config.touchpad.pointer_speed.is_finite()
                    || !(-1.0..=1.0).contains(&config.touchpad.pointer_speed)
                {
                    return Err("touchpad pointer speed is outside -1.0..=1.0");
                }
                if !config.touchpad.scroll_speed.is_finite()
                    || !tessera_primitives::input::SCROLL_SPEED_RANGE
                        .contains(&config.touchpad.scroll_speed)
                {
                    return Err("touchpad scroll speed is outside 0.1..=10.0");
                }
                if !config.mouse.pointer_speed.is_finite()
                    || !(-1.0..=1.0).contains(&config.mouse.pointer_speed)
                {
                    return Err("mouse pointer speed is outside -1.0..=1.0");
                }
                if !config.mouse.scroll_speed.is_finite()
                    || !tessera_primitives::input::SCROLL_SPEED_RANGE
                        .contains(&config.mouse.scroll_speed)
                {
                    return Err("mouse scroll speed is outside 0.1..=10.0");
                }
                if config.keyboard.repeat_rate > tessera_primitives::input::MAX_REPEAT_RATE {
                    return Err("keyboard repeat rate is above 150 repeats per second");
                }
                if config.keyboard.repeat_delay_ms == 0
                    || config.keyboard.repeat_delay_ms
                        > tessera_primitives::input::MAX_REPEAT_DELAY_MS
                {
                    return Err("keyboard repeat delay is outside 1..=2000 ms");
                }
                Ok(())
            }
            Self::SetDisplay { settings } => settings.validate(),
            Self::SetDisplayTopology { .. } | Self::ConfirmDisplay => Ok(()),
            Self::SetDesktopPreferences { preferences } => preferences.validate(),
            Self::SetIdle { settings } => settings.validate(),
            Self::SetWallpaper { settings } => {
                if !["image", "video", "3d", "parallax"].contains(&settings.mode.as_str()) {
                    return Err("wallpaper mode must be 'image', 'video', '3d', or 'parallax'");
                }
                if let Some(source) = &settings.source
                    && (source.trim().is_empty() || source.len() > 1024)
                {
                    return Err("wallpaper source path is empty or too long");
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Confirmation returned after the main loop persisted and applied a
/// settings action.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsReceipt {
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materials_are_ordered_cheapest_first() {
        // Ceilings cap a preference by taking the minimum, so the declaration
        // order is load-bearing: it is the cost order.
        let mut sorted = Material::ALL;
        sorted.sort();
        assert_eq!(sorted, Material::ALL);
        assert!(Material::Opaque < Material::Translucent);
        assert!(Material::Translucent < Material::Frosted);
        assert!(Material::Frosted < Material::LiquidGlass);
        assert_eq!(Material::default(), Material::Opaque);
    }

    #[test]
    fn only_the_glass_materials_sample_the_backdrop() {
        assert!(!Material::Opaque.samples_backdrop());
        assert!(!Material::Translucent.samples_backdrop());
        assert!(Material::Frosted.samples_backdrop());
        assert!(Material::LiquidGlass.samples_backdrop());
    }

    #[test]
    fn an_unconstrained_preference_is_drawn_as_chosen() {
        for material in Material::ALL {
            let resolved = resolve_material(material, []);
            assert_eq!(resolved.effective, material);
            assert_eq!(resolved.limited_by, None);
        }
    }

    #[test]
    fn a_ceiling_only_ever_lowers_the_preference() {
        let renderer = MaterialCeiling {
            max: Material::Translucent,
            reason: MaterialCeilingReason::Renderer,
        };
        let lowered = resolve_material(Material::LiquidGlass, [renderer]);
        assert_eq!(lowered.preferred, Material::LiquidGlass);
        assert_eq!(lowered.effective, Material::Translucent);
        assert_eq!(lowered.limited_by, Some(MaterialCeilingReason::Renderer));

        // A ceiling above the preference never raises it.
        let unchanged = resolve_material(Material::Opaque, [renderer]);
        assert_eq!(unchanged.effective, Material::Opaque);
        assert_eq!(unchanged.limited_by, None);
    }

    #[test]
    fn power_rules_cap_by_power_source_charge_and_profile() {
        use crate::system::{BatteryStatus, PowerState};
        let policy = MaterialPowerPolicy::default();
        let mains = PowerState::default();
        let battery = PowerState {
            on_battery: true,
            power_saver: false,
        };
        let at = |percent, charging| Some(BatteryStatus { percent, charging });
        let effective =
            |power, level| resolve_material(Material::LiquidGlass, policy.ceilings(power, level));

        // Mains, or a battery above the low line: glass as chosen.
        assert_eq!(
            effective(mains, at(10, false)).effective,
            Material::LiquidGlass
        );
        assert_eq!(
            effective(battery, at(80, false)).effective,
            Material::LiquidGlass
        );
        // Draining at or below the line: translucent, and the reason says so.
        let low = effective(battery, at(20, false));
        assert_eq!(low.effective, Material::Translucent);
        assert_eq!(low.limited_by, Some(MaterialCeilingReason::LowBattery));
        // Power saver caps on any source.
        let saver = effective(
            PowerState {
                on_battery: false,
                power_saver: true,
            },
            None,
        );
        assert_eq!(saver.limited_by, Some(MaterialCeilingReason::PowerSaver));

        // A stricter on-battery rule binds as soon as the plug comes out.
        let strict = MaterialPowerPolicy {
            on_battery: Material::Opaque,
            ..policy
        };
        let resolved = resolve_material(Material::Frosted, strict.ceilings(battery, at(90, false)));
        assert_eq!(resolved.effective, Material::Opaque);
        assert_eq!(resolved.limited_by, Some(MaterialCeilingReason::OnBattery));
        // A ceiling never raises a cheaper preference.
        let cheap = resolve_material(Material::Opaque, policy.ceilings(battery, at(5, false)));
        assert_eq!(cheap.effective, Material::Opaque);
        assert_eq!(cheap.limited_by, None);
        // The low line is validated.
        assert!(
            MaterialPowerPolicy {
                low_battery_percent: 0,
                ..policy
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn the_lowest_binding_ceiling_is_the_one_reported() {
        let renderer = MaterialCeiling {
            max: Material::Translucent,
            reason: MaterialCeilingReason::Renderer,
        };
        let contrast = MaterialCeiling {
            max: Material::Opaque,
            reason: MaterialCeilingReason::HighContrast,
        };
        for order in [[renderer, contrast], [contrast, renderer]] {
            let resolved = resolve_material(Material::Frosted, order);
            assert_eq!(resolved.effective, Material::Opaque);
            assert_eq!(
                resolved.limited_by,
                Some(MaterialCeilingReason::HighContrast)
            );
        }
    }

    #[test]
    fn action_validation_rejects_unbounded_values() {
        let config = tessera_primitives::input::TouchpadConfig {
            pointer_speed: 1.5,
            ..Default::default()
        };
        assert!(
            SettingsAction::SetInput {
                config: tessera_primitives::input::InputConfig {
                    touchpad: config,
                    ..Default::default()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            SettingsAction::SetInput {
                config: tessera_primitives::input::InputConfig {
                    mouse: tessera_primitives::input::MouseConfig {
                        scroll_speed: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            SettingsAction::SetInput {
                config: tessera_primitives::input::InputConfig {
                    keyboard: tessera_primitives::input::KeyboardConfig {
                        repeat_rate: 500,
                        repeat_delay_ms: 250,
                    },
                    ..Default::default()
                },
            }
            .validate()
            .is_err()
        );
        assert!(
            SettingsAction::SetInput {
                config: tessera_primitives::input::InputConfig::default(),
            }
            .validate()
            .is_ok()
        );

        let display = DisplaySettings {
            connector: "DP-1".into(),
            role: Some(crate::output::OutputRole::Extend),
            mode: Some(ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(144),
            }),
            scale: Some(1.5),
            position: Some(Point { x: 0, y: 0 }),
            primary: true,
            tentative: false,
        };
        assert!(
            SettingsAction::SetDisplay { settings: display }
                .validate()
                .is_ok()
        );

        let preferences = DesktopPreferences {
            text_scale: f64::NAN,
            ..Default::default()
        };
        assert!(
            SettingsAction::SetDesktopPreferences { preferences }
                .validate()
                .is_err()
        );

        let idle = IdleSettings {
            lock_after_seconds: 0,
            display_off_after_seconds: 60,
            ..Default::default()
        };
        assert!(
            SettingsAction::SetIdle { settings: idle }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn system_color_scheme_resolves_to_the_dark_fallback() {
        assert_eq!(ColorScheme::System.or_dark(), ColorScheme::Dark);
        assert_eq!(ColorScheme::Dark.or_dark(), ColorScheme::Dark);
        assert_eq!(ColorScheme::Light.or_dark(), ColorScheme::Light);
        assert_eq!(
            DesktopPreferences::default().color_scheme.or_dark(),
            ColorScheme::Dark
        );
    }

    #[test]
    fn accent_color_round_trips_and_normalizes() {
        let accent = AccentColor::parse_hex("#1a80FF").unwrap();
        assert_eq!(accent.to_hex(), "#1A80FF");
        assert_eq!(accent.normalized(), (26.0 / 255.0, 128.0 / 255.0, 1.0));
        assert!(AccentColor::parse_hex("1A80FF").is_err());
        assert!(AccentColor::parse_hex("#xyzxyz").is_err());
    }

    #[test]
    fn battery_warning_thresholds_default_and_validate() {
        let defaults = BatterySettings::default();
        assert_eq!(defaults.warn_at, vec![20, 5]);
        assert!(defaults.validate().is_ok());
        // An empty list disables the feature and is valid.
        assert!(BatterySettings { warn_at: vec![] }.validate().is_ok());
        assert!(
            BatterySettings {
                warn_at: vec![30, 10, 5],
            }
            .validate()
            .is_ok()
        );
        assert!(BatterySettings { warn_at: vec![0] }.validate().is_err());
        assert!(BatterySettings { warn_at: vec![100] }.validate().is_err());
        // Ascending order and duplicates are both rejected.
        assert!(
            BatterySettings {
                warn_at: vec![5, 20],
            }
            .validate()
            .is_err()
        );
        assert!(
            BatterySettings {
                warn_at: vec![20, 20],
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn desktop_preferences_serde_defaults_accessibility_when_missing() {
        let json_without_a11y = r#"{
            "color_scheme": "light",
            "accent_color": null,
            "contrast": "normal",
            "reduced_motion": false,
            "font_name": "Sans 10",
            "monospace_font_name": "Monospace 10",
            "text_scale": 1.0,
            "icon_theme": "Papirus-Dark",
            "cursor_theme": "Bibata-Modern-Ice",
            "cursor_size": 24
        }"#;

        let prefs: DesktopPreferences = serde_json::from_str(json_without_a11y).unwrap();
        assert_eq!(
            prefs.accessibility,
            tessera_primitives::accessibility::AccessibilityConfig::default()
        );

        let serialized = serde_json::to_string(&prefs).unwrap();
        let round_tripped: DesktopPreferences = serde_json::from_str(&serialized).unwrap();
        assert_eq!(round_tripped, prefs);
    }
}
