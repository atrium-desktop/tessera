//! Pure lens option factories for tessera surface materials.

use lens::{Align, Band, Color, LayoutOpts, PlaceMode, PlaceOpts, Rect};

use crate::Design;
use crate::tokens::SurfaceRole;

/// Base layout for absolutely-placed tessera surfaces.
///
/// Preserves the defaults of the retired lens `OverlayOpts` (4px child gap,
/// 6px padding) that the materials below were tuned against; the
/// [`LayoutOpts`] default of zero gap and padding would silently compact
/// every migrated surface.
#[must_use]
pub fn surface_layout() -> LayoutOpts {
    LayoutOpts {
        gap: 4.0,
        pad: 6.0,
        ..Default::default()
    }
}

/// Persistent chrome-band placement for a surface that stays on screen every
/// frame — the ADR-0060 replacement for the retired `Frame::layer`. `rect` is
/// the top-left position plus a minimum extent; the surface grows to fit its
/// body. The body always builds: there is no open/close state and no
/// dismissal.
#[must_use]
pub fn chrome_place(rect: Rect, layout: LayoutOpts) -> PlaceOpts {
    PlaceOpts {
        band: Band::Chrome,
        mode: PlaceMode::Exact,
        rect,
        transient: false,
        layout,
        ..Default::default()
    }
}

/// The flat material used by compact menus and popovers.
#[must_use]
pub fn popover(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: design.colors.popover_surface,
        border: design.colors.popover_border,
        border_width: design.strokes.hairline,
        radius: design.radii.popover,
        pad: 0.0,
        ..surface_layout()
    }
}

/// The complete body of a floating chrome panel.
///
/// This is the whole surface — fill, hairline, corners — because the
/// compositor no longer draws a material beneath chrome. A panel's cost is one
/// rounded-rect fill; there is no capture to sample and no shader to run, which
/// is what lets chrome sit on screen at a steady frame time.
#[must_use]
pub fn panel(design: &Design) -> LayoutOpts {
    let style = design.surfaces.for_role(SurfaceRole::FloatingPanel);
    LayoutOpts {
        bg: style.body,
        border: style.border,
        border_width: style.border_width,
        radius: style.radius,
        pad: 0.0,
        cross: Align::Center,
        ..surface_layout()
    }
}

/// Painted elevation halo for a floating body.
///
/// A flat product still needs to separate a floating surface from arbitrary
/// content beneath it, and it cannot buy that separation from a blur or a
/// shader. Instead each halo step is one inflated rounded rect at a low,
/// pre-multiplied alpha: two steps approximate a soft edge at the cost of two
/// fills, which is what keeps chrome's per-frame cost proportional to its
/// area rather than to a sampling radius. The steps are ordered outermost
/// first, so a caller places them beneath the body in iteration order.
///
/// Reach is capped by the body's own thickness: see the comment inside. A body
/// that is not thicker than twice the nominal blur would otherwise be
/// surrounded by a painted ring instead of lifted above a shadow.
///
/// Returns an empty list for a role that does not float (a tile sits on a
/// panel canvas and must not cast).
#[must_use]
pub fn elevation_halo(
    design: &Design,
    role: SurfaceRole,
    bounds: Rect,
    opacity: f32,
) -> Vec<(Rect, LayoutOpts)> {
    elevation_halo_for_body(
        design,
        role,
        bounds,
        design.surfaces.for_role(role).radius,
        opacity,
    )
}

/// [`elevation_halo`] for a body whose corner radius is its own.
///
/// A role's styled radius is the radius at rest. A body that morphs — the dock
/// capsule is a 2 px stadium at rest and an 18 px panel expanded — has to cast
/// the silhouette it is actually painting, or the halo reads as a second,
/// differently-shaped object behind it.
#[must_use]
pub fn elevation_halo_for_body(
    design: &Design,
    role: SurfaceRole,
    bounds: Rect,
    body_radius: f32,
    opacity: f32,
) -> Vec<(Rect, LayoutOpts)> {
    if !role.floats() {
        return Vec::new();
    }
    let style = design.surfaces.for_role(role);
    if style.shadow_alpha <= 0.0 || style.shadow_blur <= 0.0 || opacity <= 0.0 {
        return Vec::new();
    }
    // A halo is the silhouette of the body that casts it, pushed outward, so it
    // can never be larger than that body. A body thinner than twice the nominal
    // blur cannot own the full reach: the dock's collapsed capsule is 4 px tall,
    // and a 12 px halo around it reached six times the capsule's own thickness
    // on every side. That does not read as elevation underneath the pill, it
    // reads as a dark painted ring around it — and it is why a parked dock had a
    // grey outline that the hairline alone cannot explain.
    //
    // Reach and offset shrink together, in proportion, so the halo stays a
    // contact shadow *of the same shape* rather than a taller body with a
    // misplaced shadow: at 4 px the capsule gets a 2 px skirt one pixel below
    // itself, and the expanded 74 px bar is past the cap and keeps its full
    // reach.
    let thickness = bounds.w.min(bounds.h);
    if thickness <= 0.0 {
        return Vec::new();
    }
    let reach_scale = (thickness * 0.5 / style.shadow_blur).min(1.0);
    // Two steps, each reaching a fraction of the nominal blur: the wider step
    // is fainter so the pair reads as a gradient rather than as two rings.
    let steps = [(1.0, 0.38_f32), (0.55, 0.42_f32)];
    steps
        .iter()
        .map(|(reach, share)| {
            let spread = style.shadow_blur * reach * reach_scale;
            let alpha = (style.shadow_alpha * share * opacity * 255.0)
                .round()
                .clamp(0.0, 255.0);
            let halo = Rect {
                x: bounds.x - spread,
                y: bounds.y - spread + style.shadow_offset_y * reach_scale,
                w: bounds.w + spread * 2.0,
                h: bounds.h + spread * 2.0,
            };
            let material = LayoutOpts {
                bg: design.colors.shadow_ink.with_alpha(alpha as u8),
                border: Color::TRANSPARENT,
                border_width: 0.0,
                radius: body_radius + spread,
                pad: 0.0,
                ..surface_layout()
            };
            (halo, material)
        })
        .collect()
}

/// Place a floating body together with its painted elevation halo.
///
/// The halo steps are placed first (outermost first) and the body last, so the
/// body always paints over its own shadow. `id` names the body; the halo steps
/// derive stable child ids from it, which keeps lens's damage and hit-testing
/// identity stable across frames.
///
/// The halo takes the **body's** corner radius, not the role's styled one: a
/// body that morphs (the dock capsule) or overrides its corners still has to
/// cast its own silhouette, and a halo rounded differently from the body it
/// sits under is visible as a second shape.
pub fn place_floating(
    frame: &mut lens::Frame,
    id: &str,
    role: SurfaceRole,
    design: &Design,
    bounds: Rect,
    opacity: f32,
    body: LayoutOpts,
    place: impl FnOnce(&mut lens::Frame),
) {
    for (index, (rect, material)) in
        elevation_halo_for_body(design, role, bounds, body.radius, opacity)
            .into_iter()
            .enumerate()
    {
        frame.place(
            &format!("{id}-elevation-{index}"),
            &chrome_place(rect, material),
            |_| {},
        );
    }
    frame.place(id, &chrome_place(bounds, body), place);
}

/// Interaction emphasis for content nested inside an existing panel body.
///
/// This is a foreground wash, not another surface. Selection reads through
/// luminance against the parent body; it never creates an outline, a second
/// body, or an accent tint.
#[must_use]
pub fn selection(design: &Design, selected: bool) -> LayoutOpts {
    LayoutOpts {
        bg: if selected {
            design.selection.selected_tint
        } else {
            design.selection.hover_tint
        },
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: design.radii.control,
        pad: 0.0,
        ..surface_layout()
    }
}

/// Base material for settings and system-management cards.
///
/// Callers add component-specific geometry through struct update syntax.
#[must_use]
pub fn card(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: design.colors.card_surface,
        radius: design.radii.card,
        ..Default::default()
    }
}

/// The opaque elevated surface of the control center's own canvas (ADR-0176).
#[must_use]
pub fn canvas_panel(colors: &crate::ControlCenterColors) -> LayoutOpts {
    LayoutOpts {
        bg: colors.surface,
        border: colors.border,
        border_width: 1.0,
        radius: 16.0,
        pad: 0.0,
        ..surface_layout()
    }
}

/// A modal's full-output veil, painted in the chrome band beneath the modal
/// panel.
///
/// A modal dims the whole output so its panel reads as the only live surface.
/// This is a flat fill at the output's extent — there is no backdrop material
/// to blend into, so the veiling is exactly one rounded-rect draw whose alpha
/// the caller drains through `opacity` as the modal leaves.
#[must_use]
pub fn modal_scrim(design: &Design, opacity: f32) -> LayoutOpts {
    let opacity = opacity.clamp(0.0, 1.0);
    LayoutOpts {
        bg: design.colors.modal_scrim.with_alpha(
            (f32::from(design.colors.modal_scrim.components().3) * opacity).round() as u8,
        ),
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        ..surface_layout()
    }
}

/// An invisible container used purely as a layout anchor for its children —
/// the fully-transparent layer the modal prompts (secret, confirm,
/// capability, battery, app picker) stack their painted surfaces on.
#[must_use]
pub fn transparent() -> LayoutOpts {
    LayoutOpts {
        bg: Color::TRANSPARENT,
        pad: 0.0,
        ..surface_layout()
    }
}

/// A fixed-size invisible container. Callers that center their children
/// (launcher icons) layer `cross: Align::Center` on top through struct
/// update syntax.
#[must_use]
pub fn sized(width: f32, height: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        ..Default::default()
    }
}

/// A fixed-size container that paints a rounded `bg` — the reliable
/// filled-rect primitive (lens paints a container's background at its solved
/// size).
#[must_use]
pub fn sized_fill(width: f32, height: f32, color: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        bg: color,
        radius,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use lens::Color;

    use super::*;

    #[test]
    fn popover_material_preserves_the_existing_surface_values() {
        let material = popover(&Design::dark());
        let colors = Design::dark().colors;
        // A popover is a flat body: near-opaque so it reads over any content
        // without a backdrop effect to lean on.
        assert_eq!(material.bg, colors.popover_surface);
        assert_eq!(material.border, colors.popover_border);
        assert_eq!(material.border_width, 1.0);
        assert_eq!(material.radius, 12.0);
        assert_eq!(material.pad, 0.0);
    }

    #[test]
    fn panel_material_carries_the_whole_flat_body() {
        let dark = Design::dark();
        let material = panel(&dark);
        let style = dark.surfaces.for_role(SurfaceRole::FloatingPanel);
        assert_eq!(material.bg, style.body);
        assert_eq!(material.bg.components().3, 242, "the body is opaque");
        assert_eq!(material.border, style.border);
        assert_eq!(material.border_width, dark.strokes.hairline);
        assert_eq!(material.radius, 18.0);
        assert_eq!(material.cross, Align::Center);
    }

    #[test]
    fn elevation_halo_is_two_faint_fills_ordered_outermost_first() {
        let design = Design::dark();
        let bounds = Rect {
            x: 100.0,
            y: 900.0,
            w: 200.0,
            h: 60.0,
        };
        let halo = elevation_halo(&design, SurfaceRole::Dock, bounds, 1.0);
        assert_eq!(halo.len(), 2, "a soft edge from two fills, not a blur pass");
        // Outermost first, so a caller places them in order beneath the body.
        assert!(halo[0].0.w > halo[1].0.w, "the wider step comes first");
        for (rect, material) in &halo {
            assert!(rect.w >= bounds.w && rect.h >= bounds.h);
            let alpha = material.bg.components().3;
            assert!(
                alpha > 0 && alpha < 140,
                "each step is a faint wash: {alpha}"
            );
            assert_eq!(material.border_width, 0.0, "a halo never draws an edge");
            assert_eq!(material.bg.components().0, 0, "a shadow is neutral black");
        }
    }

    #[test]
    fn elevation_halo_is_absent_for_a_role_that_does_not_float() {
        let design = Design::dark();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 40.0,
        };
        assert!(
            elevation_halo(&design, SurfaceRole::Tile, bounds, 1.0).is_empty(),
            "a tile sits on a panel canvas and must not cast"
        );
        assert!(
            elevation_halo(&design, SurfaceRole::Dock, bounds, 0.0).is_empty(),
            "a body faded to nothing casts nothing"
        );
    }

    /// A body thinner than twice the nominal blur cannot own the full reach.
    ///
    /// The dock's resting capsule is 4 px tall; a 12 px halo around it reached
    /// six times the capsule's own thickness on every side, which does not read
    /// as elevation underneath it — it reads as a dark painted ring around it.
    #[test]
    fn a_thin_body_casts_a_contact_shadow_instead_of_a_ring() {
        let design = Design::dark();
        let style = design.surfaces.for_role(SurfaceRole::Dock);
        let capsule = Rect {
            x: 880.0,
            y: 1054.0,
            w: 140.0,
            h: 4.0,
        };
        let halo =
            elevation_halo_for_body(&design, SurfaceRole::Dock, capsule, capsule.h * 0.5, 1.0);
        assert_eq!(halo.len(), 2);
        for (rect, material) in &halo {
            // A halo that is wider than the body's own thickness on each side
            // is a ring, not a shadow.
            let spread = (rect.h - capsule.h) * 0.5;
            assert!(
                spread <= capsule.h * 0.5 + f32::EPSILON,
                "a 4 px capsule cannot carry a {spread} px skirt"
            );
            assert!(spread < style.shadow_blur, "the reach is capped");
            // The corner follows the body it belongs to, so a 2 px stadium does
            // not cast a 18 px-radius rounded rect behind itself.
            let body_radius = capsule.h * 0.5;
            assert!(
                (material.radius - (body_radius + spread)).abs() < 0.001,
                "the halo takes the body's corners: {}",
                material.radius
            );
        }
        // A body past the cap keeps the role's full reach.
        let bar = Rect {
            x: 400.0,
            y: 980.0,
            w: 1120.0,
            h: 74.0,
        };
        let full = elevation_halo(&design, SurfaceRole::Dock, bar, 1.0);
        assert!((full[0].0.h - bar.h - style.shadow_blur * 2.0).abs() < 0.001);
    }

    #[test]
    fn selection_is_neutral_and_never_draws_an_outline() {
        let design = Design::dark();
        let hover = selection(&design, false);
        let selected = selection(&design, true);
        assert_eq!(hover.bg, design.selection.hover_tint);
        assert_eq!(selected.bg, design.selection.selected_tint);
        assert_eq!(hover.border, Color::TRANSPARENT);
        assert_eq!(hover.border_width, 0.0);
        assert_eq!(hover.radius, design.radii.control);
    }

    #[test]
    fn card_material_leaves_component_geometry_unset() {
        let design = Design::dark();
        let material = card(&design);
        assert_eq!(material.bg, design.colors.card_surface);
        assert_eq!(material.radius, 16.0);
        assert_eq!(material.width, 0.0);
        assert_eq!(material.min_height, 0.0);
    }

    #[test]
    fn canvas_panel_material_is_opaque_in_both_schemes() {
        for colors in [
            crate::CommandPanelColors::dark(),
            crate::CommandPanelColors::light(),
        ] {
            let material = canvas_panel(&colors);
            assert_eq!(material.bg, colors.surface);
            assert_eq!(material.border, colors.border);
            assert_eq!(material.border_width, 1.0);
            assert_eq!(material.radius, 16.0);
            assert_eq!(material.pad, 0.0);
            assert_eq!(material.bg.components().3, 255);
        }
    }

    #[test]
    fn transparent_material_paints_nothing_and_keeps_surface_spacing() {
        let material = transparent();
        assert_eq!(material.bg, Color::TRANSPARENT);
        assert_eq!(material.border, Color::TRANSPARENT);
        assert_eq!(material.border_width, 0.0);
        assert_eq!(material.radius, 0.0);
        assert_eq!(material.pad, 0.0);
        assert_eq!(material.gap, surface_layout().gap);
    }

    #[test]
    fn sized_helpers_fix_extent_without_painting() {
        let material = sized(24.0, 12.0);
        assert_eq!(material.width, 24.0);
        assert_eq!(material.height, 12.0);
        assert_eq!(material.bg, Color::TRANSPARENT);

        let fill = Design::dark().colors.menu_surface_hover;
        let filled = sized_fill(8.0, 8.0, fill, 4.0);
        assert_eq!(filled.width, 8.0);
        assert_eq!(filled.height, 8.0);
        assert_eq!(filled.bg, fill);
        assert_eq!(filled.radius, 4.0);
    }
}
