//! Output presentation ownership (ADR-0183).
//!
//! Exactly one actor presents an output: either a client whose committed
//! surface covers the whole output, or the compositor's chrome. This module is
//! the single place that resolves that verdict from *physical* facts —
//! presented extent, declared opacity, and output geometry — instead of the
//! `xdg_toplevel.fullscreen` state bit.
//!
//! [`SurfacePresentation`] is also the tree's only definition of "the logical
//! extent this surface occupies" and of whole-surface opacity. The compositor
//! (`tessera-composite`), the server's hit-test, sizing, and occlusion paths,
//! and the ownership scan all take the extent from it, so a change to
//! `wp_viewport`, buffer-scale, or transform handling lands in one
//! implementation. The occlusion walk keeps its own *per-rect* opacity test on
//! purpose: it answers "which sub-rectangles are opaque", which a
//! whole-surface predicate cannot express.
//!
//! ## Why physical facts, not the protocol state bit
//!
//! `xdg_toplevel.set_fullscreen` is a *client request*. Toolkits that emulate
//! fullscreen by resizing the window to the monitor — GLFW's Wayland backend
//! among them, which is what most Java games ship — never send it. Keying
//! chrome stand-down on the bit therefore leaves the HUD's glass and blur
//! composited over a game that fills the output, disables the wallpaper
//! elision, and permanently disqualifies the frame from direct scanout: a
//! continuous GPU tax on exactly the workload the zero-cost path exists for.
//!
//! The state bit is still honoured as a *declaration*: a client that asked for
//! fullscreen owns the output even during the configure round-trip before its
//! buffer reaches the final extent.

use tessera_primitives::{Point, Rect, Size, SurfaceDmabuf, Transform};

/// The committed declaration a client surface makes about its own pixels.
///
/// Built either from a dma-buf frame ([`SurfacePresentation::from_dmabuf`]) or
/// from a live server surface record, so the compositor and the server share
/// one derivation.
#[derive(Debug, Clone, Copy)]
pub struct SurfacePresentation<'a> {
    /// Draw origin in compositor logical space, already adjusted for a declared
    /// `xdg_surface.set_window_geometry`.
    pub position: Point,
    /// Committed buffer extent in device pixels.
    pub buffer: Size,
    /// `wl_surface.set_buffer_scale`, `1` meaning unscaled.
    pub buffer_scale: i32,
    /// `wl_surface.set_buffer_transform`.
    pub transform: Transform,
    /// `wp_viewport.set_source`, in surface-local *logical* coordinates (the
    /// protocol's surface space, i.e. after `set_buffer_scale`).
    pub viewport_src: Option<Rect>,
    /// `wp_viewport.set_destination`, in logical pixels.
    pub viewport_dst: Option<Size>,
    /// Whether the buffer's `DRM_FORMAT` is opaque by construction
    /// (`XRGB8888`, `XBGR2101010`, …). Always false for SHM-backed surfaces,
    /// whose pixels carry alpha.
    pub format_opaque: bool,
    /// Declared `wl_surface.set_opaque_region`, in surface-local coordinates.
    pub opaque_region: Option<&'a [Rect]>,
}

impl<'a> SurfacePresentation<'a> {
    /// Present the committed content of a dma-buf frame.
    pub fn from_dmabuf(surface: &'a SurfaceDmabuf) -> Self {
        Self {
            position: surface.geometry.position,
            buffer: Size {
                w: surface.width,
                h: surface.height,
            },
            buffer_scale: surface.geometry.buffer_scale,
            transform: surface.geometry.transform,
            viewport_src: surface.geometry.viewport_src,
            viewport_dst: surface.geometry.viewport_dst,
            format_opaque: tessera_primitives::dmabuf::is_format_opaque(surface.drm_format),
            opaque_region: surface.opaque_region.as_deref(),
        }
    }

    /// Logical extent the surface occupies on the output.
    ///
    /// A `wp_viewport` destination wins because it is what the client asked to
    /// occupy; then the source crop; then the buffer extent divided by the
    /// buffer scale, with a 90°/270° transform swapping the axes. Viewport
    /// rectangles are already expressed in post-transform logical space, so
    /// they are not swapped again.
    pub fn logical_extent(&self) -> (i32, i32) {
        if let Some(dst) = self.viewport_dst {
            return (dst.w.max(1), dst.h.max(1));
        }
        if let Some(src) = self.viewport_src {
            return (src.size.w.max(1), src.size.h.max(1));
        }
        let (width, height) = if self.transform.swap_axes() {
            (self.buffer.h, self.buffer.w)
        } else {
            (self.buffer.w, self.buffer.h)
        };
        let scale = self.buffer_scale.max(1) as f32;
        (
            (width as f32 / scale).round().max(1.0) as i32,
            (height as f32 / scale).round().max(1.0) as i32,
        )
    }

    /// Logical rectangle the presented surface covers, anchored at its draw
    /// origin.
    pub fn logical_rect(&self) -> Rect {
        let (width, height) = self.logical_extent();
        Rect::new(self.position.x, self.position.y, width, height)
    }

    /// Whether nothing behind this surface can reach an output pixel through
    /// it, either because the format carries no alpha or because the client
    /// declared an opaque region covering the whole extent.
    pub fn is_opaque(&self) -> bool {
        if self.format_opaque {
            return true;
        }
        if self.buffer.w <= 0 || self.buffer.h <= 0 {
            // No committed content: an opaque region cannot vouch for pixels
            // that do not exist.
            return false;
        }
        let (width, height) = self.logical_extent();
        Rect::new(0, 0, width, height).fully_covered_by(self.opaque_region.unwrap_or_default())
    }

    /// Whether this surface alone presents the whole `output`.
    pub fn owns(&self, output: Rect) -> bool {
        self.is_opaque() && output.fully_covered_by(&[self.logical_rect()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_primitives::{Point, Transform};

    fn dmabuf(
        format: u32,
        width: i32,
        height: i32,
        position: Point,
        buffer_scale: i32,
    ) -> SurfaceDmabuf {
        SurfaceDmabuf {
            id: 1,
            window: None,
            width,
            height,
            generation: 1,
            damage: Vec::new(),
            buffer_id: 1,
            fd: -1,
            drm_format: format,
            modifier: 0,
            offset: 0,
            stride: width as u32 * 4,
            acquire_fence: -1,
            geometry: tessera_primitives::SurfaceGeometry {
                position,
                buffer_scale,
                transform: Transform::Normal,
                ..Default::default()
            },
            opaque_region: None,
            color: None,
        }
    }

    const XRGB: u32 = 0x3432_5258; // DRM_FORMAT_XRGB8888
    const ARGB: u32 = 0x3432_5241; // DRM_FORMAT_ARGB8888

    #[test]
    fn test_inv_arch_72_ownership_requires_opaque_coverage() {
        let output = Rect::new(0, 0, 1920, 1080);
        // Opaque but too small.
        assert!(
            !SurfacePresentation::from_dmabuf(&dmabuf(XRGB, 1280, 720, Point { x: 0, y: 0 }, 1))
                .owns(output)
        );
        // Covering but not opaque.
        assert!(
            !SurfacePresentation::from_dmabuf(&dmabuf(ARGB, 1920, 1080, Point { x: 0, y: 0 }, 1))
                .owns(output)
        );
        // Opaque and covering.
        assert!(
            SurfacePresentation::from_dmabuf(&dmabuf(XRGB, 1920, 1080, Point { x: 0, y: 0 }, 1))
                .owns(output)
        );
    }

    #[test]
    fn opaque_format_owns_the_output_at_its_origin() {
        let output = Rect::new(0, 0, 2560, 1440);
        let surface = dmabuf(XRGB, 2560, 1440, Point { x: 0, y: 0 }, 1);
        assert!(SurfacePresentation::from_dmabuf(&surface).owns(output));
    }

    #[test]
    fn a_surface_that_does_not_cover_the_output_does_not_own_it() {
        let output = Rect::new(0, 0, 2560, 1440);
        let surface = dmabuf(XRGB, 1920, 1080, Point { x: 0, y: 0 }, 1);
        assert!(!SurfacePresentation::from_dmabuf(&surface).owns(output));
    }

    #[test]
    fn a_surface_on_a_second_output_owns_that_output_only() {
        let first = Rect::new(0, 0, 2560, 1440);
        let second = Rect::new(2560, 0, 1920, 1080);
        let surface = dmabuf(XRGB, 1920, 1080, Point { x: 2560, y: 0 }, 1);
        let presentation = SurfacePresentation::from_dmabuf(&surface);
        assert!(presentation.owns(second));
        assert!(!presentation.owns(first));
    }

    #[test]
    fn alpha_format_needs_a_declared_opaque_region_to_own_the_output() {
        let output = Rect::new(0, 0, 1920, 1080);
        let mut surface = dmabuf(ARGB, 1920, 1080, Point { x: 0, y: 0 }, 1);
        assert!(!SurfacePresentation::from_dmabuf(&surface).owns(output));
        surface.opaque_region = Some(vec![Rect::new(0, 0, 1920, 1080)]);
        assert!(SurfacePresentation::from_dmabuf(&surface).owns(output));
    }

    #[test]
    fn buffer_scale_two_shrinks_the_logical_extent_and_keeps_ownership() {
        let output = Rect::new(0, 0, 1920, 1080);
        let surface = dmabuf(XRGB, 3840, 2160, Point { x: 0, y: 0 }, 2);
        let presentation = SurfacePresentation::from_dmabuf(&surface);
        assert_eq!(presentation.logical_extent(), (1920, 1080));
        assert!(presentation.owns(output));
    }

    #[test]
    fn a_quarter_turn_swaps_the_extent_axes() {
        let output = Rect::new(0, 0, 1920, 1080);
        let mut surface = dmabuf(XRGB, 1080, 1920, Point { x: 0, y: 0 }, 1);
        surface.geometry.transform = Transform::Rotate90;
        let presentation = SurfacePresentation::from_dmabuf(&surface);
        assert_eq!(presentation.logical_extent(), (1920, 1080));
        assert!(presentation.owns(output));
    }

    #[test]
    fn viewport_destination_defines_the_occupied_extent() {
        let output = Rect::new(0, 0, 2560, 1440);
        let mut surface = dmabuf(ARGB, 2560, 1440, Point { x: 0, y: 0 }, 1);
        surface.geometry.viewport_dst = Some(Size { w: 1707, h: 960 });
        surface.opaque_region = Some(vec![Rect::new(0, 0, 1707, 960)]);
        let presentation = SurfacePresentation::from_dmabuf(&surface);
        assert_eq!(presentation.logical_extent(), (1707, 960));
        assert!(!presentation.owns(output));
    }
}
