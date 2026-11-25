//! Component-mounted eight-zone Hot Zone model (ADR-0254).
//!
//! Pure domain types shared between configuration (`tessera-config`), shell
//! chrome (`tessera-shell`), and the compositor runtime (`tessera`).
//!
//! Eight mutually exclusive perimeter slots (four edges, four corners) act as
//! pointer triggers that stay completely invisible until the pointer enters a
//! configurable proximity band. Each chrome component declares which slots it
//! can validly mount onto and its preferred trigger mode (`Hover` or `Click`),
//! eliminating geometrically invalid configurations by construction.

use crate::dock::DockPosition;

/// Default pointer proximity radius (in logical pixels) within which a bound
/// Hot Zone reveals its bezel-flush visual affordance.
pub const DEFAULT_HOT_ZONE_PROXIMITY_PX: f32 = 48.0;

/// Minimum allowed proximity radius (logical pixels).
pub const MIN_HOT_ZONE_PROXIMITY_PX: f32 = 12.0;

/// Maximum allowed proximity radius (logical pixels).
pub const MAX_HOT_ZONE_PROXIMITY_PX: f32 = 240.0;

/// Default intent dwell duration (in milliseconds) for `Hover`-triggered slots.
pub const DEFAULT_HOT_ZONE_HOVER_DWELL_MS: u32 = 140;

/// One of the eight perimeter screen slots available for mounting chrome triggers.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotZoneSlot {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl HotZoneSlot {
    /// All eight perimeter slots in deterministic order (edges then corners).
    pub const ALL: [HotZoneSlot; 8] = [
        HotZoneSlot::Top,
        HotZoneSlot::Bottom,
        HotZoneSlot::Left,
        HotZoneSlot::Right,
        HotZoneSlot::TopLeft,
        HotZoneSlot::TopRight,
        HotZoneSlot::BottomLeft,
        HotZoneSlot::BottomRight,
    ];

    /// Whether this slot lies along one of the four screen edges.
    #[inline]
    #[must_use]
    pub const fn is_edge(self) -> bool {
        matches!(
            self,
            HotZoneSlot::Top | HotZoneSlot::Bottom | HotZoneSlot::Left | HotZoneSlot::Right
        )
    }

    /// Whether this slot lies at one of the four screen corners.
    #[inline]
    #[must_use]
    pub const fn is_corner(self) -> bool {
        !self.is_edge()
    }

    /// Convert an edge slot into the matching [`DockPosition`]; returns `None`
    /// for corner slots.
    #[inline]
    #[must_use]
    pub const fn as_dock_position(self) -> Option<DockPosition> {
        match self {
            HotZoneSlot::Top => Some(DockPosition::Top),
            HotZoneSlot::Bottom => Some(DockPosition::Bottom),
            HotZoneSlot::Left => Some(DockPosition::Left),
            HotZoneSlot::Right => Some(DockPosition::Right),
            HotZoneSlot::TopLeft
            | HotZoneSlot::TopRight
            | HotZoneSlot::BottomLeft
            | HotZoneSlot::BottomRight => None,
        }
    }

    /// Convert a [`DockPosition`] into its corresponding edge slot.
    #[inline]
    #[must_use]
    pub const fn from_dock_position(position: DockPosition) -> HotZoneSlot {
        match position {
            DockPosition::Top => HotZoneSlot::Top,
            DockPosition::Bottom => HotZoneSlot::Bottom,
            DockPosition::Left => HotZoneSlot::Left,
            DockPosition::Right => HotZoneSlot::Right,
        }
    }

    /// Single-bit mask identifying this slot.
    #[inline]
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            HotZoneSlot::Top => 1 << 0,
            HotZoneSlot::Bottom => 1 << 1,
            HotZoneSlot::Left => 1 << 2,
            HotZoneSlot::Right => 1 << 3,
            HotZoneSlot::TopLeft => 1 << 4,
            HotZoneSlot::TopRight => 1 << 5,
            HotZoneSlot::BottomLeft => 1 << 6,
            HotZoneSlot::BottomRight => 1 << 7,
        }
    }

    /// Canonical kebab-case name used in configuration and diagnostics.
    #[inline]
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            HotZoneSlot::Top => "top",
            HotZoneSlot::Bottom => "bottom",
            HotZoneSlot::Left => "left",
            HotZoneSlot::Right => "right",
            HotZoneSlot::TopLeft => "top-left",
            HotZoneSlot::TopRight => "top-right",
            HotZoneSlot::BottomLeft => "bottom-left",
            HotZoneSlot::BottomRight => "bottom-right",
        }
    }
}

/// Compact bitmask over the eight [`HotZoneSlot`]s used by chrome components
/// to declare valid mounting positions (`[INV-HOTZONE-03]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HotZoneSlotMask(u8);

impl HotZoneSlotMask {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(HotZoneSlot::Top.bit());
    pub const BOTTOM: Self = Self(HotZoneSlot::Bottom.bit());
    pub const LEFT: Self = Self(HotZoneSlot::Left.bit());
    pub const RIGHT: Self = Self(HotZoneSlot::Right.bit());
    pub const TOP_LEFT: Self = Self(HotZoneSlot::TopLeft.bit());
    pub const TOP_RIGHT: Self = Self(HotZoneSlot::TopRight.bit());
    pub const BOTTOM_LEFT: Self = Self(HotZoneSlot::BottomLeft.bit());
    pub const BOTTOM_RIGHT: Self = Self(HotZoneSlot::BottomRight.bit());

    pub const EDGES: Self = Self(
        HotZoneSlot::Top.bit()
            | HotZoneSlot::Bottom.bit()
            | HotZoneSlot::Left.bit()
            | HotZoneSlot::Right.bit(),
    );
    pub const CORNERS: Self = Self(
        HotZoneSlot::TopLeft.bit()
            | HotZoneSlot::TopRight.bit()
            | HotZoneSlot::BottomLeft.bit()
            | HotZoneSlot::BottomRight.bit(),
    );
    pub const ALL: Self = Self(0xFF);

    /// Combine two slot masks.
    #[inline]
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether `slot` is permitted by this mask.
    #[inline]
    #[must_use]
    pub const fn contains(self, slot: HotZoneSlot) -> bool {
        (self.0 & slot.bit()) != 0
    }
}

/// How a pointer activates a mounted Hot Zone once inside its trigger hitbox.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotZoneTrigger {
    /// Activate after brief continuous dwell inside the edge/corner trigger hitbox.
    Hover,
    /// Activate on a clean press-and-release inside the trigger hitbox.
    Click,
}

/// Chrome component that can be mounted onto a Hot Zone slot.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotZoneTarget {
    /// Application Dock (edges only; adapts to horizontal or vertical layout).
    Dock,
    /// Read-only transient Glance HUD (clock, workspace, ambient system readings).
    GlanceHud,
    /// Window and workspace Overview.
    Overview,
    /// Right/side-anchored Control Center sheet.
    ControlCenter,
    /// Pivot application launcher / intent surface.
    Pivot,
}

/// Declarative Hot Zone mounting contract advertised by a chrome component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotZoneCapability {
    /// Perimeter slots this component supports mounting onto.
    pub allowed_slots: HotZoneSlotMask,
    /// Default activation semantics when the user does not specify one.
    pub default_trigger: HotZoneTrigger,
}

impl HotZoneTarget {
    /// Canonical kebab-case name used in configuration and diagnostics.
    #[inline]
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            HotZoneTarget::Dock => "dock",
            HotZoneTarget::GlanceHud => "glance-hud",
            HotZoneTarget::Overview => "overview",
            HotZoneTarget::ControlCenter => "control-center",
            HotZoneTarget::Pivot => "pivot",
        }
    }

    /// Component-declared mounting capability and default trigger mode.
    #[inline]
    #[must_use]
    pub const fn capability(self) -> HotZoneCapability {
        match self {
            // A linear icon strip only makes geometric sense along a screen edge.
            HotZoneTarget::Dock => HotZoneCapability {
                allowed_slots: HotZoneSlotMask::EDGES,
                default_trigger: HotZoneTrigger::Hover,
            },
            HotZoneTarget::GlanceHud => HotZoneCapability {
                allowed_slots: HotZoneSlotMask::ALL,
                default_trigger: HotZoneTrigger::Hover,
            },
            HotZoneTarget::Overview => HotZoneCapability {
                allowed_slots: HotZoneSlotMask::ALL,
                default_trigger: HotZoneTrigger::Click,
            },
            HotZoneTarget::ControlCenter => HotZoneCapability {
                allowed_slots: HotZoneSlotMask::LEFT
                    .union(HotZoneSlotMask::RIGHT)
                    .union(HotZoneSlotMask::CORNERS),
                default_trigger: HotZoneTrigger::Click,
            },
            HotZoneTarget::Pivot => HotZoneCapability {
                allowed_slots: HotZoneSlotMask::ALL,
                default_trigger: HotZoneTrigger::Click,
            },
        }
    }
}

/// One resolved Hot Zone binding pairing a screen slot, target component, and
/// activation trigger mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotZoneBinding {
    pub slot: HotZoneSlot,
    pub target: HotZoneTarget,
    pub trigger: HotZoneTrigger,
}

/// Resolved Hot Zone policy consumed by the shell host and Hot Zone chrome.
#[derive(Debug, Clone, PartialEq)]
pub struct HotZonePolicy {
    /// Pointer proximity radius (logical pixels) that reveals bound hot zones.
    pub proximity_px: f32,
    /// Continuous dwell duration (milliseconds) required for `Hover` triggers.
    pub hover_dwell_ms: u32,
    /// Active slot-to-component bindings (at most one binding per slot).
    pub bindings: Vec<HotZoneBinding>,
}

/// Validation failure produced when checking a [`HotZonePolicy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotZoneValidationError {
    DuplicateSlot {
        slot: HotZoneSlot,
    },
    UnsupportedSlot {
        target: HotZoneTarget,
        slot: HotZoneSlot,
    },
    DuplicateDockBinding,
}

impl Default for HotZonePolicy {
    fn default() -> Self {
        Self::with_dock_position(DockPosition::Bottom)
    }
}

impl HotZonePolicy {
    /// Default eight-zone mapping with the Dock mounted on `dock_position`.
    #[must_use]
    pub fn with_dock_position(dock_position: DockPosition) -> Self {
        let dock_slot = HotZoneSlot::from_dock_position(dock_position);
        let mut bindings = vec![HotZoneBinding {
            slot: dock_slot,
            target: HotZoneTarget::Dock,
            trigger: HotZoneTarget::Dock.capability().default_trigger,
        }];
        let glance_slot = if dock_slot == HotZoneSlot::Top {
            HotZoneSlot::Bottom
        } else {
            HotZoneSlot::Top
        };
        bindings.push(HotZoneBinding {
            slot: glance_slot,
            target: HotZoneTarget::GlanceHud,
            trigger: HotZoneTarget::GlanceHud.capability().default_trigger,
        });
        bindings.push(HotZoneBinding {
            slot: HotZoneSlot::TopLeft,
            target: HotZoneTarget::Overview,
            trigger: HotZoneTarget::Overview.capability().default_trigger,
        });
        let cc_slot = if dock_slot == HotZoneSlot::Right {
            HotZoneSlot::TopRight
        } else {
            HotZoneSlot::Right
        };
        bindings.push(HotZoneBinding {
            slot: cc_slot,
            target: HotZoneTarget::ControlCenter,
            trigger: HotZoneTarget::ControlCenter.capability().default_trigger,
        });
        Self {
            proximity_px: DEFAULT_HOT_ZONE_PROXIMITY_PX,
            hover_dwell_ms: DEFAULT_HOT_ZONE_HOVER_DWELL_MS,
            bindings,
        }
    }

    /// Validate all invariants (`[INV-HOTZONE-02]`, `[INV-HOTZONE-03]`).
    pub fn validate(&self) -> Result<(), HotZoneValidationError> {
        let mut seen = HotZoneSlotMask::NONE;
        let mut dock_count = 0usize;
        for binding in &self.bindings {
            if seen.contains(binding.slot) {
                return Err(HotZoneValidationError::DuplicateSlot { slot: binding.slot });
            }
            seen = seen.union(HotZoneSlotMask(binding.slot.bit()));
            if !binding
                .target
                .capability()
                .allowed_slots
                .contains(binding.slot)
            {
                return Err(HotZoneValidationError::UnsupportedSlot {
                    target: binding.target,
                    slot: binding.slot,
                });
            }
            if binding.target == HotZoneTarget::Dock {
                dock_count += 1;
                if dock_count > 1 {
                    return Err(HotZoneValidationError::DuplicateDockBinding);
                }
            }
        }
        Ok(())
    }

    /// Return the edge slot currently bound to the Dock, if any.
    #[must_use]
    pub fn dock_slot(&self) -> Option<HotZoneSlot> {
        self.bindings
            .iter()
            .find(|binding| binding.target == HotZoneTarget::Dock)
            .map(|binding| binding.slot)
    }

    /// Return the binding for `slot`, if any.
    #[must_use]
    pub fn binding_for(&self, slot: HotZoneSlot) -> Option<HotZoneBinding> {
        self.bindings
            .iter()
            .copied()
            .find(|binding| binding.slot == slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_valid_for_every_dock_edge() {
        for pos in [
            DockPosition::Top,
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Right,
        ] {
            let policy = HotZonePolicy::with_dock_position(pos);
            assert_eq!(policy.validate(), Ok(()));
            assert_eq!(
                policy.dock_slot(),
                Some(HotZoneSlot::from_dock_position(pos))
            );
        }
    }

    #[test]
    fn dock_rejects_corner_slots() {
        let policy = HotZonePolicy {
            proximity_px: DEFAULT_HOT_ZONE_PROXIMITY_PX,
            hover_dwell_ms: DEFAULT_HOT_ZONE_HOVER_DWELL_MS,
            bindings: vec![HotZoneBinding {
                slot: HotZoneSlot::TopLeft,
                target: HotZoneTarget::Dock,
                trigger: HotZoneTrigger::Hover,
            }],
        };
        assert_eq!(
            policy.validate(),
            Err(HotZoneValidationError::UnsupportedSlot {
                target: HotZoneTarget::Dock,
                slot: HotZoneSlot::TopLeft,
            })
        );
    }

    #[test]
    fn duplicate_slot_is_rejected() {
        let policy = HotZonePolicy {
            proximity_px: DEFAULT_HOT_ZONE_PROXIMITY_PX,
            hover_dwell_ms: DEFAULT_HOT_ZONE_HOVER_DWELL_MS,
            bindings: vec![
                HotZoneBinding {
                    slot: HotZoneSlot::Top,
                    target: HotZoneTarget::GlanceHud,
                    trigger: HotZoneTrigger::Hover,
                },
                HotZoneBinding {
                    slot: HotZoneSlot::Top,
                    target: HotZoneTarget::Overview,
                    trigger: HotZoneTrigger::Click,
                },
            ],
        };
        assert_eq!(
            policy.validate(),
            Err(HotZoneValidationError::DuplicateSlot {
                slot: HotZoneSlot::Top
            })
        );
    }
}
