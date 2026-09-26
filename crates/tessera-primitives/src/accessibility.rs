//! Accessibility primitives, semantic node topology, and visual correction models.
//!
//! Defined in accordance with ADR-0166 (Decoupled Native Accessibility Architecture Across
//! Compositor and Optics). All structures here are transport-neutral and strictly decoupled
//! from D-Bus, IPC, or platform windowing APIs (`[INV-A11Y-02]`).

use crate::geometry::Rect;

/// Color blindness / deficiency compensation mode for the compositor GPU pipeline.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum DaltonismMode {
    /// Normal color vision; no correction applied.
    #[default]
    None = 0,
    /// Protanopia (red-blind / red-weakness).
    Protanopia = 1,
    /// Deuteranopia (green-blind / green-weakness).
    Deuteranopia = 2,
    /// Tritanopia (blue-blind / blue-weakness).
    Tritanopia = 3,
}

impl DaltonismMode {
    /// Return the standard 3x3 Daltonization color transformation matrix
    /// for GPU fragment shader evaluation.
    ///
    /// Row-major order: `output_rgb = matrix * input_rgb`.
    #[must_use]
    pub const fn daltonization_matrix(&self) -> [[f32; 3]; 3] {
        match self {
            Self::None => [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            // Protanopia compensation (reallocates red signal into green and blue)
            Self::Protanopia => [
                [0.0, 1.05118294, -0.05116099],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            // Deuteranopia compensation (reallocates green signal into red and blue)
            Self::Deuteranopia => [
                [1.0, 0.0, 0.0],
                [0.9513092, 0.0, 0.04866992],
                [0.0, 0.0, 1.0],
            ],
            // Tritanopia compensation (reallocates blue signal into red and green)
            Self::Tritanopia => [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [-0.86744736, 1.86727089, 0.0],
            ],
        }
    }
}

/// Visual accessibility configuration governing compositor post-processing shaders (`[INV-A11Y-05]`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualAccessibilityConfig {
    /// Screen magnification zoom factor. Minimum is 1.0 (unmagnified).
    pub zoom_factor: f32,
    /// Invert display luminance and color channels.
    pub invert_colors: bool,
    /// Enhance contrast across window borders and text surfaces.
    pub high_contrast: bool,
    /// Daltonization color correction mode.
    pub daltonism: DaltonismMode,
}

impl Default for VisualAccessibilityConfig {
    fn default() -> Self {
        Self {
            zoom_factor: 1.0,
            invert_colors: false,
            high_contrast: false,
            daltonism: DaltonismMode::None,
        }
    }
}

impl VisualAccessibilityConfig {
    /// Clamp the zoom factor to a safe, usable range (1.0x to 10.0x).
    #[must_use]
    pub fn sanitized_zoom_factor(&self) -> f32 {
        self.zoom_factor.clamp(1.0, 10.0)
    }

    /// Whether any visual post-processing shader is currently active.
    #[must_use]
    pub fn is_shader_active(&self) -> bool {
        self.zoom_factor > 1.001
            || self.invert_colors
            || self.high_contrast
            || self.daltonism != DaltonismMode::None
    }
}

/// Physical keyboard accessibility configuration for motor-impaired typing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyboardAccessibilityConfig {
    /// Sticky keys: latch modifier keys (Shift, Ctrl, Alt, Super) sequentially
    /// rather than requiring concurrent multi-key chords.
    pub sticky_keys: bool,
    /// Slow keys delay in milliseconds. Key presses shorter than this threshold
    /// are rejected as involuntary tremors. 0 disables slow keys.
    pub slow_keys_ms: u32,
    /// Bounce keys debounce interval in milliseconds. Successive presses of the
    /// same key within this window are rejected. 0 disables bounce keys.
    pub bounce_keys_ms: u32,
}

/// Screen reader and assistive bus activation configuration.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenReaderConfig {
    /// Whether the compositor shell exports its native UI hierarchy to the
    /// Linux accessibility bus (`org.a11y.Bus`).
    pub enabled: bool,
}

/// Top-level unified accessibility configuration.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AccessibilityConfig {
    pub visual: VisualAccessibilityConfig,
    pub keyboard: KeyboardAccessibilityConfig,
    pub screen_reader: ScreenReaderConfig,
}

/// Semantic widget roles in transport-neutral format.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u16)]
pub enum AccessibleRole {
    #[default]
    Unknown = 0,
    PushButton = 1,
    TextInput = 2,
    /// Sensitive input field (password, PIN, encryption passphrase).
    /// Enforces `[INV-A11Y-03] Zero Data Leak on Sensitive Fields`.
    PasswordText = 3,
    CheckBox = 4,
    RadioButton = 5,
    Slider = 6,
    Label = 7,
    List = 8,
    ListItem = 9,
    Window = 10,
    Dialog = 11,
    MenuBar = 12,
    MenuItem = 13,
    Notification = 14,
}

impl AccessibleRole {
    /// Returns true if this role contains sensitive credentials that MUST NOT
    /// be exposed over IPC or broadcast signals (`[INV-A11Y-03]`).
    #[must_use]
    pub const fn is_sensitive(&self) -> bool {
        matches!(self, Self::PasswordText)
    }
}

/// Boolean state flags for an accessible UI element.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccessibleState {
    pub focused: bool,
    pub disabled: bool,
    pub selected: bool,
    pub checked: bool,
    pub expanded: bool,
    pub modal: bool,
}

/// Standard accessible actions supported by UI widgets.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibleAction {
    /// Perform the default action (e.g. click a button, toggle a checkbox).
    DefaultAction,
    /// Request focus for this element.
    Focus,
    /// Select this element in a list or collection.
    Select,
    /// Set the string value of this input or control.
    SetValue(String),
}

/// Transport-neutral semantic UI node representation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessibleNode {
    /// Unique identifier within the local semantic hierarchy.
    pub id: u64,
    /// Widget semantic role.
    pub role: AccessibleRole,
    /// User-facing label or name (e.g. "Launchpad", "Close").
    pub name: Option<String>,
    /// Accessible descriptive text or tooltip.
    pub description: Option<String>,
    /// Physical bounding box in screen or parent surface coordinates.
    pub bounds: Rect,
    /// Interactive state flags.
    pub state: AccessibleState,
    /// Ordered identifiers of direct children nodes.
    pub children: Vec<u64>,
}

impl AccessibleNode {
    /// Create a new accessible node with default empty state.
    #[must_use]
    pub fn new(id: u64, role: AccessibleRole, bounds: Rect) -> Self {
        Self {
            id,
            role,
            name: None,
            description: None,
            bounds,
            state: AccessibleState::default(),
            children: Vec::new(),
        }
    }

    /// Return safe name respecting `[INV-A11Y-03]` (redacts password fields).
    #[must_use]
    pub fn safe_name(&self) -> Option<&str> {
        if self.role.is_sensitive() {
            None
        } else {
            self.name.as_deref()
        }
    }
}

/// Incremental semantic tree update emitted by UI components.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SemanticTreeUpdate {
    /// Root node identifier of the updated tree.
    pub root_id: u64,
    /// Nodes added or updated in this frame.
    pub nodes: Vec<AccessibleNode>,
    /// Node identifiers removed in this frame.
    pub removed_ids: Vec<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daltonism_matrices_preserve_identity_for_none() {
        let none = DaltonismMode::None;
        let mat = none.daltonization_matrix();
        assert_eq!(mat[0], [1.0, 0.0, 0.0]);
        assert_eq!(mat[1], [0.0, 1.0, 0.0]);
        assert_eq!(mat[2], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn visual_config_sanitizes_zoom() {
        let mut cfg = VisualAccessibilityConfig::default();
        cfg.zoom_factor = 0.2;
        assert_eq!(cfg.sanitized_zoom_factor(), 1.0);
        cfg.zoom_factor = 25.0;
        assert_eq!(cfg.sanitized_zoom_factor(), 10.0);
        cfg.zoom_factor = 2.5;
        assert_eq!(cfg.sanitized_zoom_factor(), 2.5);
    }

    #[test]
    fn sensitive_node_redacts_credentials() {
        let mut node = AccessibleNode::new(1, AccessibleRole::PasswordText, Rect::default());
        node.name = Some("super_secret_password".into());
        assert!(node.role.is_sensitive());
        assert_eq!(node.safe_name(), None);

        let mut regular = AccessibleNode::new(2, AccessibleRole::PushButton, Rect::default());
        regular.name = Some("Submit".into());
        assert!(!regular.role.is_sensitive());
        assert_eq!(regular.safe_name(), Some("Submit"));
    }
}
