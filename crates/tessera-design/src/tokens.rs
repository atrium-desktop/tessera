//! Semantic visual tokens for the built-in dark and light appearances.

use lens::Color;
use tessera_desktop::settings::ColorScheme;

use crate::colors::{DockColors, ProductColors, SceneColors};

/// The product design snapshot consumed by theme and material factories.
///
/// Components depend on semantic roles rather than literal color values. The
/// value is cheap to copy and leaves room for additional appearance variants
/// without changing component APIs.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Design {
    /// The resolved color scheme this snapshot implements. Always explicit —
    /// `System` never survives [`Design::for_scheme`].
    pub scheme: ColorScheme,
    pub colors: ProductColors,
    pub radii: Radii,
    pub strokes: Strokes,
    pub surfaces: SurfaceStyles,
    pub selection: SelectionStyles,
    pub preview: Preview,
    pub avatars: AvatarStyles,
    /// Contour policy for glyphs and labels drawn straight onto the desktop
    /// or onto chrome whose body cannot guarantee local contrast.
    pub floating_foreground: FloatingForeground,
    pub typography: TypeScale,
    pub scene: SceneColors,
    pub dock: DockColors,
}

impl Design {
    /// The canonical dark appearance currently used by compositor chrome.
    #[must_use]
    pub fn dark() -> Self {
        let colors = crate::colors::dark_appearance();
        let radii = Radii {
            menu_item: 7.0,
            popover: 12.0,
            glass_panel: 18.0,
            control: 12.0,
            card: 16.0,
            scrollbar: 2.5,
            chip: 16.0,
            cell: 22.0,
            application: 24.0,
        };
        Self {
            scheme: ColorScheme::Dark,
            colors: colors.product,
            radii,
            strokes: Strokes {
                hairline: 1.0,
                scrollbar: 5.0,
            },
            surfaces: SurfaceStyles::new(&colors.product, &colors.dock, radii, 1.0),
            selection: SelectionStyles {
                hover_tint: colors.selection_colors.surface_hover,
                selected_tint: colors.selection_colors.surface_selected,
            },
            preview: Preview {
                inactive_content_brightness: 0.74,
                focused: PreviewSelection {
                    scale: 1.0,
                    lift: 0.0,
                },
                staged: PreviewSelection {
                    scale: 1.06,
                    lift: 7.0,
                },
            },
            avatars: AvatarStyles {
                persona_header: AvatarStyle {
                    ring: colors.avatars.persona_header.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.persona_header.fallback_surface,
                    fallback_foreground: colors.avatars.persona_header.fallback_foreground,
                    initials_scale: 22.0 / 72.0,
                },
                lock_hero: AvatarStyle {
                    ring: colors.avatars.lock_hero.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.lock_hero.fallback_surface,
                    fallback_foreground: colors.avatars.lock_hero.fallback_foreground,
                    initials_scale: 0.36,
                },
            },
            floating_foreground: FloatingForeground {
                primary: colors.floating_foreground.primary,
                contour: colors.floating_foreground.contour,
                text_contour_width: 0.75,
                glyph_contour_width: 1.0,
            },
            typography: TypeScale {
                caption: 10.0,
                footnote: 11.0,
                label: 12.0,
                body: 13.0,
                headline: 15.0,
                title: 20.0,
                hero: 24.0,
            },
            scene: colors.scene,
            dock: colors.dock,
        }
    }

    /// The canonical light appearance: the same geometry and optical identity
    /// as [`Design::dark`], re-toned as dark ink on white frosted glass.
    ///
    /// The lens light theme is generalized to whole-product surfaces: white
    /// bodies keep the glass whisper, tints become dark washes, and shadows
    /// deepen slightly so pale panels still separate from bright content.
    /// Radii, strokes, and preview policy are scheme-invariant and shared
    /// with the dark appearance.
    #[must_use]
    pub fn light() -> Self {
        let colors = crate::colors::light_appearance();
        Self {
            scheme: ColorScheme::Light,
            colors: colors.product,
            surfaces: SurfaceStyles::new(&colors.product, &colors.dock, Design::dark().radii, 1.0),
            selection: SelectionStyles {
                hover_tint: colors.selection_colors.surface_hover,
                selected_tint: colors.selection_colors.surface_selected,
            },
            avatars: AvatarStyles {
                persona_header: AvatarStyle {
                    ring: colors.avatars.persona_header.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.persona_header.fallback_surface,
                    fallback_foreground: colors.avatars.persona_header.fallback_foreground,
                    initials_scale: 22.0 / 72.0,
                },
                lock_hero: AvatarStyle {
                    ring: colors.avatars.lock_hero.ring,
                    ring_width: 1.0,
                    fallback_surface: colors.avatars.lock_hero.fallback_surface,
                    fallback_foreground: colors.avatars.lock_hero.fallback_foreground,
                    initials_scale: 0.36,
                },
            },
            // The HUD core turns dark on light; the contour inverts with it
            // so legibility survives arbitrary bright wallpaper the same way
            // the dark appearance survives dark regions.
            floating_foreground: FloatingForeground {
                primary: colors.floating_foreground.primary,
                contour: colors.floating_foreground.contour,
                text_contour_width: 0.75,
                glyph_contour_width: 1.0,
            },
            // The scene floor inverts to the light surface gray; scrims stay
            // pale so bright content recedes behind modal chrome the same
            // way the dark appearance dims it.
            scene: colors.scene,
            dock: colors.dock,
            ..Self::dark()
        }
    }

    /// The design snapshot for one desktop color-scheme preference. `System`
    /// resolves through [`ColorScheme::or_dark`], so the returned snapshot's
    /// [`Design::scheme`] is always explicit.
    #[must_use]
    pub fn for_scheme(scheme: ColorScheme) -> Self {
        match scheme.or_dark() {
            ColorScheme::Dark | ColorScheme::System => Self::dark(),
            ColorScheme::Light => Self::light(),
        }
    }

    /// Whether this snapshot implements the light appearance.
    #[must_use]
    pub fn is_light(&self) -> bool {
        self.scheme == ColorScheme::Light
    }
}

impl Default for Design {
    fn default() -> Self {
        Self::dark()
    }
}

/// Shared radii in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Radii {
    pub menu_item: f32,
    pub popover: f32,
    pub glass_panel: f32,
    pub control: f32,
    pub card: f32,
    pub scrollbar: f32,
    /// HUD chips and pills.
    pub chip: f32,
    /// Launcher grid cells.
    pub cell: f32,
    /// Application-level modal panels.
    pub application: f32,
}

/// Shared stroke widths in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Strokes {
    pub hairline: f32,
    pub scrollbar: f32,
}

/// The only permitted label sizes in chrome, in logical pixels. Components
/// snap their text to the nearest step instead of choosing arbitrary sizes,
/// keeping every surface on one readable hierarchy. Scheme-invariant.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TypeScale {
    /// Smallest auxiliary text (badges, timestamps, fine print).
    pub caption: f32,
    /// Secondary annotations below body text.
    pub footnote: f32,
    /// Control and row labels.
    pub label: f32,
    /// Default reading size.
    pub body: f32,
    /// Section headers and emphasized rows.
    pub headline: f32,
    /// Panel and dialog titles.
    pub title: f32,
    /// The single largest display step, reserved for hero text.
    pub hero: f32,
}

/// Semantic role of one analytic Liquid Glass body.
///
/// Roles name a chrome surface's elevation and use, never a material
/// intensity. Every role resolves to a flat painted body: an opaque fill and a
/// hairline boundary. There is no backdrop capture, blur, refraction, rim
/// light, or adaptive tint anywhere in the product, so a role cannot vary
/// optics that no longer exist — it varies only the fill, the boundary, and
/// the shadow that separates the surface from arbitrary content beneath it.
///
/// The reason is the one ADR-0226 records: an analytic glass body costs a
/// full-resolution scene capture plus a per-pixel lens shader on every frame
/// its contents move, which on a laptop panel is the single largest
/// steady-state GPU cost the compositor has, and it buys a decorative effect
/// that competes with the content it floats over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// Compact floating status pills (the desktop widget shelf).
    Chip,
    /// A one-line name bubble anchored to an owner.
    Tooltip,
    /// Text-bearing transient surfaces (context menus).
    Menu,
    /// Floating panels that carry content: the switcher, live previews.
    FloatingPanel,
    /// The panel that owns attention: prompts, pickers, the pivot.
    ProminentPanel,
    /// The dock bar and its collapsed capsule.
    Dock,
    /// A square chrome cell that holds an icon or a reading: Bento tiles.
    Tile,
}

impl SurfaceRole {
    /// Whether this role floats above arbitrary desktop content rather than
    /// sitting on the panel canvas, and therefore needs a separating shadow.
    #[must_use]
    pub fn floats(self) -> bool {
        !matches!(self, SurfaceRole::Tile)
    }
}

/// Per-role flat surface policy.
///
/// `body` is the opaque fill, `border` the hairline stroke, and the shadow is
/// configured in logical pixels (`shadow_alpha` 0 disables it). Everything
/// here is a painted value: a role's cost is one rounded-rect fill, which is
/// the reason the product can hold a steady frame time with chrome on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SurfaceStyle {
    pub body: Color,
    pub border: Color,
    pub border_width: f32,
    pub radius: f32,
    pub shadow_alpha: f32,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
}

impl SurfaceStyle {
    const fn new(
        body: Color,
        border: Color,
        border_width: f32,
        radius: f32,
        shadow_alpha: f32,
        shadow_blur: f32,
        shadow_offset_y: f32,
    ) -> Self {
        Self {
            body,
            border,
            border_width,
            radius,
            shadow_alpha,
            shadow_blur,
            shadow_offset_y,
        }
    }

    /// The same surface with a different corner radius — a role that morphs
    /// (the dock capsule) keeps one fill and changes only its corners.
    #[must_use]
    pub const fn with_radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// The same surface with a different body, for a role that swaps its tone
    /// between rest and activity without changing its geometry.
    #[must_use]
    pub const fn with_body(mut self, body: Color) -> Self {
        self.body = body;
        self
    }
}

/// Role-indexed flat surface policies for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SurfaceStyles {
    pub chip: SurfaceStyle,
    pub tooltip: SurfaceStyle,
    pub menu: SurfaceStyle,
    pub floating_panel: SurfaceStyle,
    pub prominent_panel: SurfaceStyle,
    pub dock: SurfaceStyle,
    pub tile: SurfaceStyle,
}

impl SurfaceStyles {
    /// Resolve the flat surface for every role from one palette.
    fn new(colors: &ProductColors, dock: &DockColors, radii: Radii, hairline: f32) -> Self {
        Self {
            chip: SurfaceStyle::new(
                colors.panel_surface,
                colors.panel_border,
                hairline,
                radii.chip,
                0.16,
                4.0,
                2.0,
            ),
            tooltip: SurfaceStyle::new(
                colors.panel_surface,
                colors.panel_border,
                hairline,
                radii.chip,
                0.14,
                10.0,
                5.0,
            ),
            menu: SurfaceStyle::new(
                colors.popover_surface,
                colors.popover_border,
                hairline,
                radii.popover,
                0.18,
                16.0,
                8.0,
            ),
            floating_panel: SurfaceStyle::new(
                colors.panel_surface,
                colors.panel_border,
                hairline,
                radii.glass_panel,
                0.18,
                16.0,
                8.0,
            ),
            prominent_panel: SurfaceStyle::new(
                colors.panel_surface,
                colors.panel_border,
                hairline,
                radii.glass_panel,
                0.20,
                18.0,
                9.0,
            ),
            dock: SurfaceStyle::new(
                dock.bar_surface_expanded,
                colors.panel_border,
                hairline,
                radii.glass_panel,
                0.20,
                12.0,
                6.0,
            ),
            tile: SurfaceStyle::new(
                colors.card_surface,
                Color::TRANSPARENT,
                0.0,
                radii.control,
                0.0,
                0.0,
                0.0,
            ),
        }
    }

    #[must_use]
    pub fn for_role(self, role: SurfaceRole) -> SurfaceStyle {
        match role {
            SurfaceRole::Chip => self.chip,
            SurfaceRole::Tooltip => self.tooltip,
            SurfaceRole::Menu => self.menu,
            SurfaceRole::FloatingPanel => self.floating_panel,
            SurfaceRole::ProminentPanel => self.prominent_panel,
            SurfaceRole::Dock => self.dock,
            SurfaceRole::Tile => self.tile,
        }
    }
}

/// Paint hierarchy for interactive content hosted inside one surface.
///
/// Hover and selection are quiet painted washes on top of the parent body.
/// Neither creates a second surface, an outline, or an accent tint: the
/// selection reads through luminance against the role's own fill.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct SelectionStyles {
    pub hover_tint: Color,
    pub selected_tint: Color,
}

/// Shared presentation policy for live window previews.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Preview {
    /// Opaque brightness of siblings while one preview is focused.
    pub inactive_content_brightness: f32,
    /// Quiet selection used inside an anchored preview panel.
    pub focused: PreviewSelection,
    /// Foreground staging used by the held-modifier window switcher.
    pub staged: PreviewSelection,
}

impl Preview {
    #[must_use]
    pub fn selection(self, style: PreviewSelectionStyle) -> PreviewSelection {
        match style {
            PreviewSelectionStyle::Focused => self.focused,
            PreviewSelectionStyle::Staged => self.staged,
        }
    }
}

/// Named selection treatments for preview cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSelectionStyle {
    /// Optical focus only; card geometry remains stationary.
    Focused,
    /// Optical focus plus a restrained scale and upward lift.
    Staged,
}

/// Geometry adjustment associated with a preview selection treatment.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PreviewSelection {
    pub scale: f32,
    pub lift: f32,
}

/// Semantic role of a persona portrait within product chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarRole {
    PersonaHeader,
    LockHero,
}

/// Host-rendered frame and fallback policy for a persona portrait.
///
/// Portrait content, source precedence, and animation do not belong here;
/// they remain owned by `tessera-shell::persona`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AvatarStyle {
    pub ring: Color,
    pub ring_width: f32,
    pub fallback_surface: Color,
    pub fallback_foreground: Color,
    /// Initials font size as a fraction of the host-provided portrait size.
    pub initials_scale: f32,
}

/// Role-indexed persona portrait styles for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AvatarStyles {
    pub persona_header: AvatarStyle,
    pub lock_hero: AvatarStyle,
}

impl AvatarStyles {
    #[must_use]
    pub fn for_role(self, role: AvatarRole) -> AvatarStyle {
        match role {
            AvatarRole::PersonaHeader => self.persona_header,
            AvatarRole::LockHero => self.lock_hero,
        }
    }
}

/// Foreground-separation policy for text and glyphs that float on arbitrary
/// desktop content.
///
/// A single foreground color cannot guarantee local contrast over an
/// arbitrary wallpaper or application surface. A restrained contour keeps the
/// core legible on bright or visually busy regions while disappearing
/// naturally over dark regions. Text and glyphs share one contour color; only
/// their geometry-specific widths differ. The dock's collapsed capsule and
/// its workspace ticks are the surfaces that still need this.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct FloatingForeground {
    /// Core tone shared by floating labels, symbols, and active indicators.
    pub primary: Color,
    /// Contour/underlay shared by every floating foreground form.
    pub contour: Color,
    /// Fine contour for compact text, in logical pixels.
    pub text_contour_width: f32,
    /// Contour for vector, raster, and geometric glyphs, in logical pixels.
    pub glyph_contour_width: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_design_uses_the_dark_color_appearance() {
        let design = Design::dark();
        assert_eq!(design.scheme, ColorScheme::Dark);
        assert!(!design.is_light());
        assert_eq!(design.colors, crate::colors::dark_appearance().product);
        assert_eq!(design.radii.menu_item, 7.0);
    }

    #[test]
    fn for_scheme_resolves_system_to_the_dark_fallback() {
        assert_eq!(Design::for_scheme(ColorScheme::System), Design::dark());
        assert_eq!(Design::for_scheme(ColorScheme::Dark), Design::dark());
        assert_eq!(Design::for_scheme(ColorScheme::Light), Design::light());
        assert_eq!(Design::light().scheme, ColorScheme::Light);
        assert!(Design::light().is_light());
    }

    #[test]
    fn light_palette_inverts_ink_and_surface_while_sharing_geometry() {
        let light = Design::light();
        let dark = Design::dark();
        // Dark ink on white-ish surfaces.
        let (r, g, b, a) = light.colors.application_text.components();
        assert!(r < 80 && g < 80 && b < 90 && a == 255);
        let (r, g, b, a) = light.colors.application_surface.components();
        assert!(r > 220 && g > 220 && b > 220 && a == 255);
        // Tints and tracks become dark washes instead of white ones.
        let (r, g, b, _) = light.colors.menu_surface_hover.components();
        assert!(r < 64 && g < 64 && b < 64);
        // Radii, strokes, and preview policy are scheme-invariant.
        assert_eq!(light.radii, dark.radii);
        assert_eq!(light.strokes, dark.strokes);
        assert_eq!(light.preview, dark.preview);
        // Every semantic role stays populated.
        assert_eq!(light.colors.slider_fill, light.colors.application_accent);
        let (_, _, _, popover_alpha) = light.colors.popover_surface.components();
        assert!(popover_alpha > 200);
    }

    #[test]
    fn modal_scrim_text_stays_light_in_both_appearances() {
        // The scrim veil stays a dark wash in both appearances, so content
        // drawn directly on it keeps a light tone in both too — the light
        // scheme's page-appropriate ink would be illegible over the wash.
        for design in [Design::dark(), Design::light()] {
            let (r, g, b, a) = design.colors.modal_scrim_text.components();
            assert!(r > 220 && g > 220 && b > 220 && a == 255);
        }
    }

    #[test]
    fn parent_modal_scrim_and_attention_pulse_tokens_are_valid() {
        for design in [Design::dark(), Design::light()] {
            let (_, _, _, scrim_alpha) = design.colors.parent_modal_scrim.components();
            assert!((100..=160).contains(&scrim_alpha));
            let (_, _, _, pulse_alpha) = design.colors.attention_pulse_border.components();
            assert!(pulse_alpha >= 200);
        }
    }

    #[test]
    fn generic_icon_surface_is_scheme_invariant() {
        // The no-icon app chip is a mid-tone slate in both appearances.
        assert_eq!(
            Design::light().colors.generic_icon_surface,
            Design::dark().colors.generic_icon_surface
        );
        let (r, g, b, a) = Design::dark().colors.generic_icon_surface.components();
        assert!(r > 40 && r < 130 && g > 50 && g < 140 && b > 90 && b < 190 && a > 200);
    }

    #[test]
    fn launcher_field_body_follows_the_scheme() {
        // Dark appearance: a dark body following the scheme.
        // (Colors store premultiplied bytes, so the assertions divide back out the alpha.)
        let dark = Design::dark().colors;
        let (r, g, b, a) = dark.launcher_field_surface.components();
        let unmul = |c: u8| u32::from(c) * 255 / u32::from(a.max(1));
        assert!(
            unmul(r) < 60 && unmul(g) < 60 && unmul(b) < 80,
            "dark field is a dark body: {}",
            dark.launcher_field_surface.components().0
        );
        // The search field sits over the full-screen launcher sheet, which is
        // itself a flat painted cover, so the field must be opaque enough to
        // separate from it without a backdrop effect.
        assert!(a >= 230, "and opaque enough to stand alone: {a}");

        // Light appearance: a light body, matching its page tone.
        let light = Design::light().colors;
        let (r, g, b, a) = light.launcher_field_surface.components();
        let unmul = |c: u8| u32::from(c) * 255 / u32::from(a.max(1));
        assert!(
            unmul(r) > 240 && unmul(g) > 240 && unmul(b) > 240,
            "light field is a light body"
        );

        // The grid selection wash follows the scheme the same way: dark ink
        // in the dark appearance, white in the light one. (Stored bytes are
        // premultiplied, so divide the alpha back out.)
        let (dr, dg, db, da) = dark.launcher_selection_surface.components();
        let dunmul = |c: u8| u32::from(c) * 255 / u32::from(da.max(1));
        assert!(
            dunmul(dr) < 40 && dunmul(dg) < 40 && dunmul(db) < 50,
            "dark selection is a dark wash"
        );
        let (lr, lg, lb, la) = light.launcher_selection_surface.components();
        let lunmul = |c: u8| u32::from(c) * 255 / u32::from(la.max(1));
        assert!(
            lunmul(lr) > 240 && lunmul(lg) > 240 && lunmul(lb) > 240,
            "light selection is a light wash"
        );
    }

    #[test]
    fn popover_surface_and_menu_contrast_follow_scheme() {
        // Dark appearance: a near-opaque dark body with bright, high-contrast text.
        let dark = Design::dark();
        let (dr, dg, db, da) = dark.colors.popover_surface.components();
        let dunmul = |c: u8| u32::from(c) * 255 / u32::from(da.max(1));
        assert!(
            dunmul(dr) < 50 && dunmul(dg) < 50 && dunmul(db) < 60,
            "dark popover is a dark body: ({}, {}, {})",
            dunmul(dr),
            dunmul(dg),
            dunmul(db)
        );
        assert!(
            da > 200,
            "popover surface has solid legibility opacity: {da}"
        );
        let (mr, mg, mb, ma) = dark.colors.menu_text.components();
        assert!(mr > 220 && mg > 220 && mb > 220 && ma == 255);
        let (hr, hg, hb, ha) = dark.colors.menu_text_heading.components();
        assert!(hr > 150 && hg > 150 && hb > 180 && ha == 255);

        // Light appearance: a near-opaque light body with dark, high-contrast ink.
        let light = Design::light();
        let (lr, lg, lb, la) = light.colors.popover_surface.components();
        let lunmul = |c: u8| u32::from(c) * 255 / u32::from(la.max(1));
        assert!(
            lunmul(lr) > 240 && lunmul(lg) > 240 && lunmul(lb) > 240,
            "light popover is a light body: ({}, {}, {})",
            lunmul(lr),
            lunmul(lg),
            lunmul(lb)
        );
        assert!(
            la > 200,
            "light popover surface has solid legibility opacity: {la}"
        );
        let (mr, mg, mb, ma) = light.colors.menu_text.components();
        assert!(mr < 50 && mg < 50 && mb < 60 && ma == 255);
        let (hr, hg, hb, ha) = light.colors.menu_text_heading.components();
        assert!(hr < 120 && hg < 120 && hb < 140 && ha == 255);
    }

    #[test]
    fn floating_foreground_uses_one_restrained_contour_family() {
        let floating = Design::dark().floating_foreground;
        let colors = crate::colors::dark_appearance().floating_foreground;
        assert_eq!(floating.primary, colors.primary);
        assert_eq!(floating.contour, colors.contour);
        assert!(floating.text_contour_width > 0.0);
        assert!(floating.text_contour_width < floating.glyph_contour_width);
        assert!(floating.glyph_contour_width <= 1.0);
    }

    #[test]
    fn selection_is_neutral_borderless_policy() {
        let design = Design::dark();
        let selection = design.selection;
        let colors = crate::colors::dark_appearance().selection_colors;
        assert_eq!(selection.hover_tint, colors.surface_hover);
        assert_eq!(selection.selected_tint, colors.surface_selected);
        assert_eq!(design.preview.inactive_content_brightness, 0.74);
    }

    #[test]
    fn surface_roles_name_elevation_through_shadow_and_fill_only() {
        let surfaces = Design::dark().surfaces;
        assert!(surfaces.chip.shadow_blur < surfaces.floating_panel.shadow_blur);
        assert!(surfaces.floating_panel.shadow_blur < surfaces.prominent_panel.shadow_blur);
        // Every floating role has a separating shadow; the tile role sits on
        // a panel canvas and must not cast one.
        for role in [
            SurfaceRole::Chip,
            SurfaceRole::Tooltip,
            SurfaceRole::Menu,
            SurfaceRole::FloatingPanel,
            SurfaceRole::ProminentPanel,
            SurfaceRole::Dock,
        ] {
            assert!(role.floats(), "{role:?} floats above arbitrary content");
            assert!(surfaces.for_role(role).shadow_alpha > 0.0);
            assert!(surfaces.for_role(role).shadow_blur > 0.0);
        }
        assert!(!SurfaceRole::Tile.floats());
        assert_eq!(surfaces.for_role(SurfaceRole::Tile).shadow_alpha, 0.0);
    }

    #[test]
    fn every_surface_role_resolves_to_an_opaque_painted_body() {
        // The product has no material pass: a chrome body that leaned on a
        // backdrop effect would be a translucent smear over application
        // content. Every role must therefore be readable on its own fill.
        for design in [Design::dark(), Design::light()] {
            for role in [
                SurfaceRole::Chip,
                SurfaceRole::Tooltip,
                SurfaceRole::Menu,
                SurfaceRole::FloatingPanel,
                SurfaceRole::ProminentPanel,
                SurfaceRole::Dock,
            ] {
                let style = design.surfaces.for_role(role);
                let alpha = style.body.components().3;
                assert!(
                    alpha >= 236,
                    "{role:?} in {:?} is opaque enough to stand alone: {alpha}",
                    design.scheme
                );
                assert!(
                    style.border_width > 0.0,
                    "{role:?} keeps a hairline boundary so its edge is defined \
                     without an optical rim"
                );
                assert!(style.radius > 0.0, "{role:?} has a rounded body");
            }
        }
    }

    #[test]
    fn text_bearing_surfaces_resolve_to_the_legible_popover_body() {
        // Menus and tooltips float over arbitrary content and carry text, so
        // they take the palettes' highest-contrast body rather than the shared
        // neutral panel tone.
        let dark = Design::dark();
        assert_eq!(dark.surfaces.menu.body, dark.colors.popover_surface);
        assert_eq!(
            dark.surfaces.menu.body.components().3,
            248,
            "dark menus are near-opaque"
        );
        let light = Design::light();
        assert_eq!(light.surfaces.menu.body, light.colors.popover_surface);
        assert_eq!(
            light.surfaces.menu.body.components().3,
            250,
            "light menus are near-opaque"
        );
        // The tooltip shares the panel body but keeps a tighter shadow: it is
        // one line and must not read as a panel.
        assert!(dark.surfaces.tooltip.shadow_blur < dark.surfaces.menu.shadow_blur);
    }

    #[test]
    fn staged_preview_adds_geometry_without_inventing_a_second_focus_policy() {
        let preview = Design::dark().preview;
        assert_eq!(
            preview.selection(PreviewSelectionStyle::Focused),
            PreviewSelection {
                scale: 1.0,
                lift: 0.0
            }
        );
        assert_eq!(
            preview.selection(PreviewSelectionStyle::Staged),
            PreviewSelection {
                scale: 1.06,
                lift: 7.0
            }
        );
    }

    #[test]
    fn avatar_roles_keep_content_out_of_the_design_contract() {
        let avatars = Design::dark().avatars;
        let colors = crate::colors::dark_appearance().avatars;
        let header = avatars.for_role(AvatarRole::PersonaHeader);
        let lock = avatars.for_role(AvatarRole::LockHero);
        assert_eq!(header.ring, colors.persona_header.ring);
        assert_eq!(lock.ring, colors.lock_hero.ring);
        assert!(header.initials_scale > 0.0);
        assert!(lock.initials_scale > 0.0);
    }

    #[test]
    fn type_scale_is_monotonic_with_exactly_the_spec_steps() {
        let type_scale = Design::dark().typography;
        assert_eq!(type_scale.caption, 10.0);
        assert_eq!(type_scale.footnote, 11.0);
        assert_eq!(type_scale.label, 12.0);
        assert_eq!(type_scale.body, 13.0);
        assert_eq!(type_scale.headline, 15.0);
        assert_eq!(type_scale.title, 20.0);
        assert_eq!(type_scale.hero, 24.0);
        let steps = [
            type_scale.caption,
            type_scale.footnote,
            type_scale.label,
            type_scale.body,
            type_scale.headline,
            type_scale.title,
            type_scale.hero,
        ];
        assert!(steps.windows(2).all(|pair| pair[0] < pair[1]));
        // Typography is scheme-invariant.
        assert_eq!(Design::light().typography, type_scale);
    }

    #[test]
    fn new_radii_are_present_and_scheme_invariant() {
        let dark = Design::dark();
        assert_eq!(dark.radii.chip, 16.0);
        assert_eq!(dark.radii.cell, 22.0);
        assert_eq!(dark.radii.application, 24.0);
        assert_eq!(Design::light().radii, dark.radii);
    }

    #[test]
    fn critical_and_validation_stay_in_hue_family_per_scheme() {
        let dark = Design::dark();
        let light = Design::light();
        // Same hue families, darkened so they read on light surfaces.
        let (r, g, b, a) = light.colors.critical.components();
        assert!(r > g && r > b && a == 255);
        let (r, g, b, a) = light.colors.validation.components();
        assert!(b > r && b > g && a == 255);
        assert_ne!(light.colors.critical, dark.colors.critical);
        assert_ne!(light.colors.validation, dark.colors.validation);
    }

    #[test]
    fn dark_scene_colors_match_the_compositor_palette() {
        let scene = Design::dark().scene;
        assert_eq!(scene, crate::colors::dark_appearance().scene);
        // `System` resolves through the dark arm, as in `runtime::scheme`.
        assert_eq!(Design::for_scheme(ColorScheme::System).scene, scene);
    }

    #[test]
    fn light_scene_colors_match_the_compositor_palette() {
        let scene = Design::light().scene;
        assert_eq!(scene, crate::colors::light_appearance().scene);
    }

    #[test]
    fn dock_palette_follows_the_scheme() {
        let dark = Design::dark().dock;
        let light = Design::light().dock;
        assert_eq!(dark, crate::colors::dark_appearance().dock);
        assert_eq!(light, crate::colors::light_appearance().dock);
        // Schemes tune dock indicators and dividers to contrast with their plate polarity.
        assert_ne!(dark.running_dot_active, light.running_dot_active);
        assert_ne!(dark.section_divider, light.section_divider);
    }
}
