//! KMS plane descriptors and allocation for output layers.
//!
//! An **output layer** is an independently scanned-out source: primary,
//! cursor, or overlay. A **scene layer** is z-order/damage in the compositor
//! and never enters this module. In particular, wallpaper/background and
//! always-on-top are scene semantics; neither is a plane role.
//!
//! The backend always allocates exactly one primary plane per CRTC and at
//! most one cursor plane per CRTC. Overlay planes are inventoried per CRTC and
//! may be allocated only through `OverlayPlanePolicy::Enabled` after the
//! complete plane set passes one atomic `TEST_ONLY` request (ADR-0234,
//! ADR-0235). Scene concepts never become plane roles.

use super::output::{plane_modifiers, property_map, required_prop};
use super::*;

/// The three KMS plane roles. There is deliberately no `Background` or
/// `AlwaysOnTop` variant: those are scene concepts, not scanout layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlaneRole {
    Primary,
    Cursor,
    Overlay,
}

/// One discovered plane and the capabilities the allocator may use.
#[derive(Debug, Clone)]
pub(super) struct PlaneCapabilities {
    pub(super) handle: plane::Handle,
    pub(super) possible_crtcs: Vec<crtc::Handle>,
    pub(super) formats: Vec<u32>,
}

/// One format/modifier pair an overlay plane accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverlayFormatCapabilities {
    pub(super) fourcc: u32,
    pub(super) modifiers: Vec<u64>,
}

/// Atomic properties used to program one overlay plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverlayPlaneProperties {
    pub(super) fb_id: property::Handle,
    pub(super) crtc_id: property::Handle,
    pub(super) src_x: property::Handle,
    pub(super) src_y: property::Handle,
    pub(super) src_w: property::Handle,
    pub(super) src_h: property::Handle,
    pub(super) crtc_x: property::Handle,
    pub(super) crtc_y: property::Handle,
    pub(super) crtc_w: property::Handle,
    pub(super) crtc_h: property::Handle,
    pub(super) alpha: Option<property::Handle>,
    pub(super) zpos: Option<property::Handle>,
    pub(super) in_fence_fd: Option<property::Handle>,
    pub(super) fb_damage_clips: Option<property::Handle>,
}

/// Capabilities the overlay planner needs from one KMS overlay plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverlayPlaneCapabilities {
    pub(super) handle: plane::Handle,
    pub(super) possible_crtcs: Vec<crtc::Handle>,
    pub(super) formats: Vec<OverlayFormatCapabilities>,
    /// Driver-declared `zpos` range. `None` means the plane keeps its default
    /// order relative to Primary and Cursor.
    pub(super) zpos: Option<(u64, u64)>,
    pub(super) alpha: bool,
    pub(super) in_fence_fd: bool,
    pub(super) fb_damage_clips: bool,
    pub(super) props: OverlayPlaneProperties,
}

impl OverlayPlaneCapabilities {
    pub(super) fn supports(&self, fourcc: u32, modifier: u64) -> bool {
        self.formats
            .iter()
            .any(|format| format.fourcc == fourcc && format.modifiers.contains(&modifier))
    }

    pub(super) fn supports_zpos(&self, zpos: Option<u32>) -> bool {
        match (zpos, self.zpos) {
            (None, _) => true,
            (Some(zpos), Some((min, max))) => u64::from(zpos) >= min && u64::from(zpos) <= max,
            (Some(_), None) => false,
        }
    }
}

/// Discovered planes grouped by their KMS role.
#[derive(Debug, Default)]
pub(super) struct PlaneInventory {
    pub(super) primary: Vec<PlaneCapabilities>,
    pub(super) cursor: Vec<PlaneCapabilities>,
    pub(super) overlay: Vec<OverlayPlaneCapabilities>,
}

/// Overlay allocation policy.
///
/// `Disabled` keeps every discovered overlay plane out of commits. `Enabled`
/// still requires the complete plane set to pass one atomic `TEST_ONLY`
/// request before the real commit. There is no best-effort variant: a rejected
/// plane falls back to composition rather than producing a partial scanout
/// (ADR-0235).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayPlanePolicy {
    Disabled,
    /// Explicit hardware-validation path (`TESSERA_OVERLAY_OFFLOAD`). The
    /// backend still runs the complete-set `TEST_ONLY` before committing.
    Enabled,
}

/// The overlay portion of one output's plane allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverlayPlaneAllocation {
    /// Overlay planes that can drive this output's CRTC, with the
    /// format/modifier/zpos facts a complete-set planner needs. The handles
    /// also let telemetry distinguish a plane shared by several CRTCs from
    /// separate planes.
    pub(super) discovered: Vec<OverlayPlaneCapabilities>,
    pub(super) policy: OverlayPlanePolicy,
}

impl OverlayPlaneAllocation {
    pub(super) fn disabled(discovered: Vec<OverlayPlaneCapabilities>) -> Self {
        Self {
            discovered,
            policy: OverlayPlanePolicy::Disabled,
        }
    }

    pub(super) fn allows_allocation(&self) -> bool {
        self.policy == OverlayPlanePolicy::Enabled
    }
}

pub(super) fn apply_overlay_policy(displays: &mut DisplaySet, enabled: bool) {
    let policy = if enabled {
        OverlayPlanePolicy::Enabled
    } else {
        OverlayPlanePolicy::Disabled
    };
    for output in &mut displays.outputs {
        output.planes.overlay.policy = policy;
    }
}

/// Plane allocation for one output (one CRTC).
///
/// Exactly one primary plane is mandatory. Cursor is optional. Overlay is an
/// allocation record, not a scene layer: `OverlayPlanePolicy::Disabled`
/// proves that no content bypasses composition.
#[derive(Debug)]
pub(super) struct OutputPlanes {
    pub(super) primary: PrimaryPlane,
    pub(super) cursor: Option<CursorPlane>,
    pub(super) overlay: OverlayPlaneAllocation,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PrimaryPlaneProperties {
    pub(super) fb_id: property::Handle,
    pub(super) crtc_id: property::Handle,
    pub(super) src_x: property::Handle,
    pub(super) src_y: property::Handle,
    pub(super) src_w: property::Handle,
    pub(super) src_h: property::Handle,
    pub(super) crtc_x: property::Handle,
    pub(super) crtc_y: property::Handle,
    pub(super) crtc_w: property::Handle,
    pub(super) crtc_h: property::Handle,
    pub(super) in_fence_fd: Option<property::Handle>,
    /// `FB_DAMAGE_CLIPS`: per-commit damage hint consumed by PSR-style
    /// drivers. Absent on planes/kernels without damage tracking; commits
    /// then carry no hint at all.
    pub(super) fb_damage_clips: Option<property::Handle>,
}

#[derive(Debug)]
pub(super) struct PrimaryPlane {
    pub(super) handle: plane::Handle,
    pub(super) props: PrimaryPlaneProperties,
    /// Pre-created `FB_DAMAGE_CLIPS` blob covering the output's whole
    /// framebuffer rectangle, reused by full/unknown-damage commits.
    pub(super) full_damage_blob: Option<(property::Value<'static>, u64)>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CursorPlaneProperties {
    pub(super) plane_fb_id: property::Handle,
    pub(super) plane_crtc_id: property::Handle,
    pub(super) plane_src_x: property::Handle,
    pub(super) plane_src_y: property::Handle,
    pub(super) plane_src_w: property::Handle,
    pub(super) plane_src_h: property::Handle,
    pub(super) plane_crtc_x: property::Handle,
    pub(super) plane_crtc_y: property::Handle,
    pub(super) plane_crtc_w: property::Handle,
    pub(super) plane_crtc_h: property::Handle,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CursorPlane {
    pub(super) handle: plane::Handle,
    pub(super) props: CursorPlaneProperties,
}

/// Discover every KMS plane and classify it by role.
pub(super) fn discover_plane_inventory(
    card: &Card,
    resources: &control::ResourceHandles,
) -> Result<PlaneInventory, DrmError> {
    let mut inventory = PlaneInventory::default();
    for handle in card.plane_handles()? {
        let Ok(info) = card.get_plane(handle) else {
            continue;
        };
        let possible_crtcs = resources.filter_crtcs(info.possible_crtcs());
        match plane_role(card, handle) {
            Some(PlaneRole::Primary) => inventory.primary.push(PlaneCapabilities {
                handle,
                possible_crtcs,
                formats: info.formats().to_vec(),
            }),
            Some(PlaneRole::Cursor) => inventory.cursor.push(PlaneCapabilities {
                handle,
                possible_crtcs,
                formats: info.formats().to_vec(),
            }),
            Some(PlaneRole::Overlay) => {
                if let Some(capabilities) =
                    overlay_capabilities(card, handle, possible_crtcs, info.formats())
                {
                    inventory.overlay.push(capabilities);
                }
            }
            None => log::debug!("drm: ignoring plane {handle:?} with no known role"),
        }
    }
    Ok(inventory)
}

fn plane_role(card: &Card, handle: plane::Handle) -> Option<PlaneRole> {
    let properties = card.get_properties(handle).ok()?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id).ok()?;
        if info.name() == c"type" {
            return match value as u32 {
                value if value == control::PlaneType::Primary as u32 => Some(PlaneRole::Primary),
                value if value == control::PlaneType::Cursor as u32 => Some(PlaneRole::Cursor),
                value if value == control::PlaneType::Overlay as u32 => Some(PlaneRole::Overlay),
                _ => None,
            };
        }
    }
    None
}

fn overlay_capabilities(
    card: &Card,
    handle: plane::Handle,
    possible_crtcs: Vec<crtc::Handle>,
    formats: &[u32],
) -> Option<OverlayPlaneCapabilities> {
    let formats = formats
        .iter()
        .filter_map(|&fourcc| {
            let format = DrmFourcc::try_from(fourcc).ok()?;
            let modifiers = plane_modifiers(card, handle, format).ok()?;
            (!modifiers.is_empty()).then_some(OverlayFormatCapabilities { fourcc, modifiers })
        })
        .collect::<Vec<_>>();
    if formats.is_empty() {
        return None;
    }
    let properties = property_map(card, handle).unwrap_or_else(|error| {
        log::warn!("drm: cannot read overlay plane {handle:?} properties: {error}");
        HashMap::new()
    });
    let zpos = properties.get("zpos").and_then(|info| {
        if let property::ValueType::UnsignedRange(min, max) = info.value_type() {
            Some((*min, *max))
        } else {
            None
        }
    });
    Some(OverlayPlaneCapabilities {
        handle,
        possible_crtcs,
        formats,
        zpos,
        alpha: properties.contains_key("alpha"),
        in_fence_fd: properties.contains_key("IN_FENCE_FD"),
        fb_damage_clips: properties.contains_key("FB_DAMAGE_CLIPS"),
        props: OverlayPlaneProperties {
            fb_id: required_prop(&properties, "FB_ID").ok()?,
            crtc_id: required_prop(&properties, "CRTC_ID").ok()?,
            src_x: required_prop(&properties, "SRC_X").ok()?,
            src_y: required_prop(&properties, "SRC_Y").ok()?,
            src_w: required_prop(&properties, "SRC_W").ok()?,
            src_h: required_prop(&properties, "SRC_H").ok()?,
            crtc_x: required_prop(&properties, "CRTC_X").ok()?,
            crtc_y: required_prop(&properties, "CRTC_Y").ok()?,
            crtc_w: required_prop(&properties, "CRTC_W").ok()?,
            crtc_h: required_prop(&properties, "CRTC_H").ok()?,
            alpha: optional_prop(&properties, "alpha"),
            zpos: optional_prop(&properties, "zpos"),
            in_fence_fd: optional_prop(&properties, "IN_FENCE_FD"),
            fb_damage_clips: optional_prop(&properties, "FB_DAMAGE_CLIPS"),
        },
    })
}

/// Assign at most one compatible cursor plane to every output.
///
/// Cursor allocation is a maximum bipartite matching rather than a greedy
/// assignment: a flexible plane must remain available for a later CRTC whose
/// compatibility set is narrower. Failure is non-fatal; the compositor paints
/// a software cursor and direct scanout stays disabled while it is visible.
pub(super) fn assign_cursor_planes(
    card: &Card,
    outputs: &mut [Output],
    inventory: &PlaneInventory,
) {
    let usable_cursor_planes = inventory
        .cursor
        .iter()
        .filter(|plane| plane.formats.contains(&(DrmFourcc::Argb8888 as u32)))
        .filter(|plane| {
            plane_modifiers(card, plane.handle, DrmFourcc::Argb8888)
                .is_ok_and(|modifiers| modifiers.contains(&u64::from(DrmModifier::Linear)))
        })
        .filter_map(|plane| match build_cursor_plane(card, plane.handle) {
            Ok(cursor) => Some((plane, cursor)),
            Err(error) => {
                log::warn!(
                    "drm: cursor plane {:?} is incomplete: {error}",
                    plane.handle
                );
                None
            }
        })
        .collect::<Vec<_>>();
    let cursor_choices = outputs
        .iter()
        .map(|output| {
            usable_cursor_planes
                .iter()
                .enumerate()
                .filter_map(|(index, (plane, _))| {
                    plane.possible_crtcs.contains(&output.crtc).then_some(index)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for (output, cursor_index) in outputs.iter_mut().zip(assign_optional_planes(
        &cursor_choices,
        usable_cursor_planes.len(),
    )) {
        output.planes.cursor = cursor_index.map(|index| usable_cursor_planes[index].1);
    }
}

fn build_cursor_plane(card: &Card, handle: plane::Handle) -> Result<CursorPlane, DrmError> {
    let props = property_map(card, handle)?;
    Ok(CursorPlane {
        handle,
        props: CursorPlaneProperties {
            plane_fb_id: required_prop(&props, "FB_ID")?,
            plane_crtc_id: required_prop(&props, "CRTC_ID")?,
            plane_src_x: required_prop(&props, "SRC_X")?,
            plane_src_y: required_prop(&props, "SRC_Y")?,
            plane_src_w: required_prop(&props, "SRC_W")?,
            plane_src_h: required_prop(&props, "SRC_H")?,
            plane_crtc_x: required_prop(&props, "CRTC_X")?,
            plane_crtc_y: required_prop(&props, "CRTC_Y")?,
            plane_crtc_w: required_prop(&props, "CRTC_W")?,
            plane_crtc_h: required_prop(&props, "CRTC_H")?,
        },
    })
}

/// Maximize one-to-one optional plane allocation across a set of consumers.
///
/// Each entry in `choices` contains indices into the discovered plane list.
/// The augmenting-path assignment keeps flexible planes available for
/// consumers whose compatibility set is narrower.
pub(super) fn assign_optional_planes(
    choices: &[Vec<usize>],
    plane_count: usize,
) -> Vec<Option<usize>> {
    fn augment(
        consumer: usize,
        choices: &[Vec<usize>],
        owners: &mut [Option<usize>],
        visited: &mut [bool],
    ) -> bool {
        for &plane in &choices[consumer] {
            if plane >= owners.len() || visited[plane] {
                continue;
            }
            visited[plane] = true;
            let previous = owners[plane];
            if previous.is_none_or(|owner| augment(owner, choices, owners, visited)) {
                owners[plane] = Some(consumer);
                return true;
            }
        }
        false
    }

    let mut owners = vec![None; plane_count];
    for consumer in 0..choices.len() {
        let mut visited = vec![false; plane_count];
        let _ = augment(consumer, choices, &mut owners, &mut visited);
    }
    let mut assignments = vec![None; choices.len()];
    for (plane, consumer) in owners.into_iter().enumerate() {
        if let Some(consumer) = consumer {
            assignments[consumer] = Some(plane);
        }
    }
    assignments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inv_arch_123_cursor_plane_matching_is_one_to_one() {
        // Consumer 0 accepts either plane; consumer 1 only accepts plane 1.
        // A greedy assignment would strand consumer 1, so the augmenting path
        // must move consumer 0 to plane 0.
        let assignments = assign_optional_planes(&[vec![0, 1], vec![1]], 2);
        assert_eq!(assignments, vec![Some(0), Some(1)]);
    }

    #[test]
    fn optional_plane_matching_leaves_unavailable_consumers_empty() {
        let assignments = assign_optional_planes(&[vec![0], vec![0]], 1);
        assert_eq!(assignments, vec![Some(0), None]);
    }

    #[test]
    fn overlay_plane_support_checks_format_and_zpos() {
        let handle =
            plane::Handle::from(std::num::NonZeroU32::new(3).expect("non-zero test handle"));
        let capabilities = OverlayPlaneCapabilities {
            handle,
            possible_crtcs: Vec::new(),
            formats: vec![OverlayFormatCapabilities {
                fourcc: DrmFourcc::Xrgb8888 as u32,
                modifiers: vec![0, 7],
            }],
            zpos: Some((2, 4)),
            alpha: true,
            in_fence_fd: true,
            fb_damage_clips: true,
            props: OverlayPlaneProperties {
                fb_id: property::Handle::from(std::num::NonZeroU32::new(4).unwrap()),
                crtc_id: property::Handle::from(std::num::NonZeroU32::new(5).unwrap()),
                src_x: property::Handle::from(std::num::NonZeroU32::new(6).unwrap()),
                src_y: property::Handle::from(std::num::NonZeroU32::new(7).unwrap()),
                src_w: property::Handle::from(std::num::NonZeroU32::new(8).unwrap()),
                src_h: property::Handle::from(std::num::NonZeroU32::new(9).unwrap()),
                crtc_x: property::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
                crtc_y: property::Handle::from(std::num::NonZeroU32::new(11).unwrap()),
                crtc_w: property::Handle::from(std::num::NonZeroU32::new(12).unwrap()),
                crtc_h: property::Handle::from(std::num::NonZeroU32::new(13).unwrap()),
                alpha: None,
                zpos: None,
                in_fence_fd: None,
                fb_damage_clips: None,
            },
        };
        assert!(capabilities.supports(DrmFourcc::Xrgb8888 as u32, 0));
        assert!(capabilities.supports(DrmFourcc::Xrgb8888 as u32, 7));
        assert!(!capabilities.supports(DrmFourcc::Xrgb8888 as u32, 9));
        assert!(!capabilities.supports(DrmFourcc::Argb8888 as u32, 0));
        assert!(capabilities.supports_zpos(None));
        assert!(capabilities.supports_zpos(Some(2)));
        assert!(capabilities.supports_zpos(Some(4)));
        assert!(!capabilities.supports_zpos(Some(1)));
        assert!(!capabilities.supports_zpos(Some(5)));
    }

    #[test]
    fn test_inv_arch_128_disabled_policy_never_allocates_overlay() {
        let handle =
            plane::Handle::from(std::num::NonZeroU32::new(3).expect("non-zero test handle"));
        let capabilities = OverlayPlaneCapabilities {
            handle,
            possible_crtcs: Vec::new(),
            formats: vec![OverlayFormatCapabilities {
                fourcc: DrmFourcc::Xrgb8888 as u32,
                modifiers: vec![0],
            }],
            zpos: Some((1, 3)),
            alpha: true,
            in_fence_fd: true,
            fb_damage_clips: true,
            props: OverlayPlaneProperties {
                fb_id: property::Handle::from(std::num::NonZeroU32::new(4).unwrap()),
                crtc_id: property::Handle::from(std::num::NonZeroU32::new(5).unwrap()),
                src_x: property::Handle::from(std::num::NonZeroU32::new(6).unwrap()),
                src_y: property::Handle::from(std::num::NonZeroU32::new(7).unwrap()),
                src_w: property::Handle::from(std::num::NonZeroU32::new(8).unwrap()),
                src_h: property::Handle::from(std::num::NonZeroU32::new(9).unwrap()),
                crtc_x: property::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
                crtc_y: property::Handle::from(std::num::NonZeroU32::new(11).unwrap()),
                crtc_w: property::Handle::from(std::num::NonZeroU32::new(12).unwrap()),
                crtc_h: property::Handle::from(std::num::NonZeroU32::new(13).unwrap()),
                alpha: None,
                zpos: None,
                in_fence_fd: None,
                fb_damage_clips: None,
            },
        };
        let allocation = OverlayPlaneAllocation::disabled(vec![capabilities.clone()]);
        assert_eq!(allocation.policy, OverlayPlanePolicy::Disabled);
        assert!(!allocation.allows_allocation());
        assert_eq!(allocation.discovered, vec![capabilities]);
    }
}
