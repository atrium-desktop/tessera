//! Renderer-independent surface descriptions and color metadata.
#![forbid(unsafe_code)]
use tessera_types::{Point, Rect, Size, Transform};
pub mod color;
pub mod dmabuf;
pub mod edid;
pub mod stream;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_rect_to_logical_maps_fractional_scale_viewport() {
        // Chrome on a 2× HiDPI output: buffer 2400×1600 behind a
        // wp_viewport destination of 1200×800 logical px, buffer_scale 1.
        // A 16×16 cursor-adjacent damage rect in buffer px must land in
        // logical px (outward-rounded), not force full damage.
        let geometry = SurfaceGeometry {
            viewport_dst: Some(Size { w: 1200, h: 800 }),
            ..Default::default()
        };
        let mapped = geometry.buffer_rect_to_logical(Rect::new(100, 60, 16, 16), 2400, 1600);
        // 100/2400*1200 = 50 exactly; 60/1600*800 = 30 exactly; the 16px
        // span rounds outward to 8+1 logical px per axis.
        assert_eq!(mapped.origin.x, 50);
        assert_eq!(mapped.origin.y, 30);
        assert!(mapped.size.w >= 8 && mapped.size.w <= 9);
        assert!(mapped.size.h >= 8 && mapped.size.h <= 9);
    }

    #[test]
    fn buffer_rect_to_logical_viewport_crop_offsets() {
        // viewport src crops the buffer: damage inside the crop maps with
        // the crop offset removed and stretched to the destination.
        let geometry = SurfaceGeometry {
            viewport_src: Some(Rect::new(100, 50, 400, 400)),
            viewport_dst: Some(Size { w: 200, h: 200 }),
            ..Default::default()
        };
        let mapped = geometry.buffer_rect_to_logical(Rect::new(100, 50, 400, 400), 800, 600);
        assert_eq!(mapped.origin.x, 0);
        assert_eq!(mapped.origin.y, 0);
        assert_eq!(mapped.size.w, 200);
        assert_eq!(mapped.size.h, 200);
    }

    #[test]
    fn buffer_rect_to_logical_round_trips_logical_to_buffer_scale() {
        // No viewport: the mapping degenerates to the buffer-scale division
        // (`buffer_damage_to_surface` semantics) with outward rounding.
        let geometry = SurfaceGeometry {
            buffer_scale: 2,
            ..Default::default()
        };
        let mapped = geometry.buffer_rect_to_logical(Rect::new(3, 5, 8, 10), 200, 200);
        // Outward-rounded division by the buffer scale, matching
        // `buffer_damage_to_surface`: x: floor(3/2)=1..ceil(11/2)=6 → w=5;
        // y: floor(5/2)=2..ceil(15/2)=8 → h=6.
        assert_eq!(mapped, Rect::new(1, 2, 5, 6));
    }

    #[test]
    fn buffer_rect_to_logical_outward_rounds_partial_spans() {
        let geometry = SurfaceGeometry {
            viewport_dst: Some(Size { w: 1199, h: 799 }),
            ..Default::default()
        };
        // 1 buffer px at 2400→1199 does not land on an integer logical px;
        // outward rounding must cover the partial span on both axes.
        let mapped = geometry.buffer_rect_to_logical(Rect::new(0, 0, 1, 1), 2400, 1600);
        assert_eq!(mapped.origin, Point { x: 0, y: 0 });
        assert!(mapped.size.w >= 1 && mapped.size.h >= 1);
    }

    #[test]
    fn rect_contains_inclusive_origin_exclusive_far_corner() {
        let r = Rect::new(10, 20, 100, 50);
        assert!(r.contains(Point { x: 10, y: 20 })); // top-left inclusive
        assert!(r.contains(Point { x: 109, y: 69 })); // bottom-right - 1
        assert!(!r.contains(Point { x: 110, y: 20 })); // right edge exclusive
        assert!(!r.contains(Point { x: 10, y: 70 })); // bottom edge exclusive
        assert!(!r.contains(Point { x: 9, y: 20 })); // left of origin
        assert!(!r.contains(Point { x: 10, y: 19 })); // above origin
    }

    #[test]
    fn transform_swap_axes_matches_90_and_270_only() {
        let should_swap = [
            Transform::Rotate90,
            Transform::Rotate270,
            Transform::FlipRotate90,
            Transform::FlipRotate270,
        ];
        let should_not_swap = [
            Transform::Normal,
            Transform::Rotate180,
            Transform::FlipHorizontal,
            Transform::FlipRotate180,
        ];
        for t in should_swap {
            assert!(t.swap_axes(), "{t:?} should swap axes");
        }
        for t in should_not_swap {
            assert!(!t.swap_axes(), "{t:?} should not swap axes");
        }
    }

    #[test]
    fn transform_maps_buffer_damage_to_the_renderer_surface_orientation() {
        let rect = Rect::new(10, 15, 20, 10);
        let dims = (100, 60);
        let expected = [
            (Transform::Normal, Rect::new(10, 15, 20, 10)),
            (Transform::Rotate90, Rect::new(35, 10, 10, 20)),
            (Transform::Rotate180, Rect::new(70, 35, 20, 10)),
            (Transform::Rotate270, Rect::new(15, 70, 10, 20)),
            (Transform::FlipHorizontal, Rect::new(70, 15, 20, 10)),
            (Transform::FlipRotate90, Rect::new(15, 10, 10, 20)),
            (Transform::FlipRotate180, Rect::new(10, 35, 20, 10)),
            (Transform::FlipRotate270, Rect::new(35, 70, 10, 20)),
        ];

        for (transform, mapped) in expected {
            assert_eq!(
                transform.map_buffer_rect_to_surface(rect, dims),
                mapped,
                "{transform:?}"
            );
        }
    }

    #[test]
    fn transform_maps_a_full_buffer_to_the_full_transformed_extent() {
        let full = Rect::new(0, 0, 100, 60);
        for transform in [
            Transform::Normal,
            Transform::Rotate180,
            Transform::FlipHorizontal,
            Transform::FlipRotate180,
        ] {
            assert_eq!(
                transform.map_buffer_rect_to_surface(full, (100, 60)),
                Rect::new(0, 0, 100, 60),
                "{transform:?}"
            );
        }
        for transform in [
            Transform::Rotate90,
            Transform::Rotate270,
            Transform::FlipRotate90,
            Transform::FlipRotate270,
        ] {
            assert_eq!(
                transform.map_buffer_rect_to_surface(full, (100, 60)),
                Rect::new(0, 0, 60, 100),
                "{transform:?}"
            );
        }
    }

    #[test]
    fn transform_name_round_trips_through_from_name() {
        let all = [
            Transform::Normal,
            Transform::Rotate90,
            Transform::Rotate180,
            Transform::Rotate270,
            Transform::FlipHorizontal,
            Transform::FlipRotate90,
            Transform::FlipRotate180,
            Transform::FlipRotate270,
        ];
        for t in all {
            assert_eq!(Transform::from_name(t.name()), Some(t), "{t:?}");
        }
    }

    #[test]
    fn transform_from_name_accepts_underscore_aliases_only() {
        assert_eq!(
            Transform::from_name("flipped_90"),
            Some(Transform::FlipRotate90)
        );
        assert_eq!(
            Transform::from_name("flipped_270"),
            Some(Transform::FlipRotate270)
        );
        // Exact lowercase matching: no case folding, no invented synonyms.
        assert_eq!(Transform::from_name("Normal"), None);
        assert_eq!(Transform::from_name("rotate90"), None);
        assert_eq!(Transform::from_name("upside-down"), None);
        assert_eq!(Transform::from_name(""), None);
    }
}

/// Common geometry/metadata carried alongside every surface's pixels, whether
/// the backing store is shm CPU memory or a dma-buf fd. Fields default to
/// "no transform, scale 1, no offset, no clipping" so existing call sites
/// that only populate the buffer itself still produce a visible result.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceGeometry {
    /// Logical position of the surface's top-left corner in the output.
    pub position: Point,
    /// Logical extent the client declared via `xdg_surface.set_window_geometry`.
    /// None means the buffer's own w/h is the geometry.
    pub window_geometry: Option<Rect>,
    /// Buffer transform (rotation/reflection) to apply at composite time.
    pub transform: Transform,
    /// Buffer scale factor (1 = unscaled). HiDPI clients commit at scale N and
    /// expect the compositor to divide destination dimensions by N.
    pub buffer_scale: i32,
    /// `wp_viewport.set_source` rectangle in surface-local pixel coords, or
    /// None for "whole buffer".
    pub viewport_src: Option<Rect>,
    /// `wp_viewport.set_destination` size in logical pixels, or None for
    /// "buffer or source size".
    pub viewport_dst: Option<Size>,
    /// Interpolated size while a window transition (ADR-0029) is in flight.
    /// The renderer draws the root texture scaled to this instead of the
    /// buffer-implied size. `None` outside transitions and for subsurfaces.
    pub transition_size: Option<Size>,
    /// Opacity multiplier while an open/close transition (ADR-0029) fades
    /// the window in or out. `None` means fully opaque — the common case,
    /// including geometry flights. The renderer applies it as a solid-paint
    /// alpha over the whole root texture; subsurface trees inherit it by
    /// drawing through the same modulated path.
    pub transition_opacity: Option<f32>,
    /// Genie minimize/restore warp (ADR-0029 `TransitionEffect::Minimize`).
    /// The renderer draws the root texture as horizontal strips pinched
    /// toward `target` by `progress`. `None` for every other draw.
    pub minimize_warp: Option<MinimizeWarp>,
}

/// A genie-style minimize deformation: horizontal slices of the window are
/// pinched horizontally toward the dock icon's centre as `progress` (eased,
/// `0.0..=1.0`) advances. Applies to the root toplevel surface only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimizeWarp {
    /// Eased deformation amount: 0.0 is the undeformed window, 1.0 fully
    /// funnels into `target`.
    pub progress: f32,
    /// The dock icon's centre in output coordinates.
    pub target: Point,
}

impl Default for SurfaceGeometry {
    /// `buffer_scale` defaults to 1 (unscaled), not the i32 default of 0,
    /// so a partially populated `SurfaceGeometry` produces a visible surface
    /// rather than a divide-by-zero in the renderer's destination-size
    /// calculation.
    fn default() -> Self {
        SurfaceGeometry {
            position: Point::default(),
            window_geometry: None,
            transform: Transform::Normal,
            buffer_scale: 1,
            viewport_src: None,
            viewport_dst: None,
            transition_size: None,
            transition_opacity: None,
            minimize_warp: None,
        }
    }
}

impl SurfaceGeometry {
    /// Multiplier from surface-local logical coordinates to buffer pixels for
    /// the committed buffer of `buffer_width`×`buffer_height`, per axis.
    ///
    /// `wl_surface.damage` arrives in surface-local coordinates; both the
    /// compositor's incremental shm snapshot copy and the renderer's
    /// incremental texture upload must scale those rects into buffer pixels
    /// to refresh exactly the changed region. `wl_surface.set_buffer_scale`
    /// alone is *not* the whole story: fractional-scale clients keep
    /// `buffer_scale` at 1 and commit a buffer N× denser than the logical
    /// surface behind a `wp_viewport` destination, so the factor has to be
    /// derived from the viewport when one is set.
    ///
    /// - `viewport_dst` set: the logical surface extent is the destination
    ///   size, so the factor is `buffer ÷ destination` per axis (2.0 for a
    ///   2× fractional-scale client, 1.5 for 150%, ...). A `viewport_src`
    ///   sub-rect narrows the sampled region, so this can over-cover — always
    ///   safe for damage tracking, which tolerates refreshing extra pixels
    ///   but never under-refreshing.
    /// - `viewport_src` only: damage shares the source's surface-local
    ///   coordinate space, so the factor stays `buffer_scale`.
    /// - neither: plain `buffer_scale`.
    pub fn logical_to_buffer_scale(&self, buffer_width: i32, buffer_height: i32) -> (f32, f32) {
        let scale = self.buffer_scale.max(1) as f32;
        if let Some(dst) = self.viewport_dst {
            let (bw, bh) = if self.transform.swap_axes() {
                (buffer_height, buffer_width)
            } else {
                (buffer_width, buffer_height)
            };
            return (
                (bw as f32 / dst.w.max(1) as f32).max(0.01),
                (bh as f32 / dst.h.max(1) as f32).max(0.01),
            );
        }
        (scale, scale)
    }

    /// Map a damage rectangle from **buffer pixel coordinates** to
    /// **surface-local logical coordinates**, honouring the viewport and the
    /// buffer scale. The exact inverse sampling relationship is affine but
    /// rect endpoints transform non-linearly under it, so this returns the
    /// tight bounding box of the four mapped corners with **outward** rounding
    /// — over-covering damage is always safe (damage tracking tolerates
    /// refreshing extra pixels, never stale ones).
    ///
    /// Pipeline: buffer px → (viewport `src` crop + `dst` stretch, or the
    /// `buffer_scale` division when no viewport destination is set) →
    /// surface-local logical px.
    ///
    /// This closes the "viewport ⇒ unmappable ⇒ full damage" fallback that
    /// made every `wl_surface.damage_buffer` from a fractional-scale client
    /// (Chrome and every GTK4/Electron app on HiDPI) force a whole-buffer
    /// CPU copy plus a whole-texture GPU upload on every commit. The mapped
    /// rect may over-cover when `viewport_src` crops the buffer (the crop's
    /// area outside the surface cannot be addressed in logical coordinates
    /// at all), which is the documented safe direction.
    pub fn buffer_rect_to_logical(
        &self,
        rect: Rect,
        buffer_width: i32,
        buffer_height: i32,
    ) -> Rect {
        // Post-transform buffer space the viewport samples from.
        let full_src = Rect::new(
            0,
            0,
            if self.transform.swap_axes() {
                buffer_height
            } else {
                buffer_width
            },
            if self.transform.swap_axes() {
                buffer_width
            } else {
                buffer_height
            },
        );
        let src = self.viewport_src.unwrap_or(full_src);
        let dst = self.viewport_dst;

        let map_point = |(px, py): (f32, f32)| -> (f32, f32) {
            // Buffer px → source fraction.
            let fx = (px - src.origin.x as f32) / src.size.w.max(1) as f32;
            let fy = (py - src.origin.y as f32) / src.size.h.max(1) as f32;
            match dst {
                Some(dst) => (fx * dst.w as f32, fy * dst.h as f32),
                // No destination: an explicit `viewport_src` already lives
                // in surface-local logical coordinates (wp_viewport: src is
                // in surface coordinate space), so the fraction round-trips
                // through the source extent and only the crop applies. With
                // no viewport at all the logical surface is the buffer
                // divided by `buffer_scale`, so the fraction scales through
                // that logical extent instead.
                None => {
                    let divisor = if self.viewport_src.is_some() {
                        1.0
                    } else {
                        self.buffer_scale.max(1) as f32
                    };
                    (
                        fx * src.size.w.max(1) as f32 / divisor,
                        fy * src.size.h.max(1) as f32 / divisor,
                    )
                }
            }
        };

        let corners = [
            (rect.origin.x as f32, rect.origin.y as f32),
            ((rect.origin.x + rect.size.w) as f32, rect.origin.y as f32),
            (rect.origin.x as f32, (rect.origin.y + rect.size.h) as f32),
            (
                (rect.origin.x + rect.size.w) as f32,
                (rect.origin.y + rect.size.h) as f32,
            ),
        ];
        let mut lo = (f32::MAX, f32::MAX);
        let mut hi = (f32::MIN, f32::MIN);
        for corner in corners {
            let mapped = map_point(corner);
            lo.0 = lo.0.min(mapped.0);
            lo.1 = lo.1.min(mapped.1);
            hi.0 = hi.0.max(mapped.0);
            hi.1 = hi.1.max(mapped.1);
        }
        let clamp_f = |v: f32| v.clamp(i32::MIN as f32, i32::MAX as f32);
        Rect::new(
            clamp_f(lo.0).floor() as i32,
            clamp_f(lo.1).floor() as i32,
            (clamp_f(hi.0).ceil() - clamp_f(lo.0).floor()) as i32,
            (clamp_f(hi.1).ceil() - clamp_f(lo.1).floor()) as i32,
        )
    }
}

/// A borrowed view of a surface's current contents, handed from the server to
/// the renderer. Pixels are tightly packed BGRA8 (`stride == width * 4`).
/// `generation` increments on every new commit so the renderer can cache the
/// uploaded texture and re-upload only when the contents change.
pub struct SurfacePixels<'a> {
    /// Stable identifier for the surface (its record address).
    pub id: usize,
    /// Toplevel the surface belongs to (its root's window id), or `None`
    /// for compositor-owned overlays. Lets the renderer's mapped drawing
    /// (overview) group frames per window.
    pub window: Option<tessera_types::WindowId>,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    pub pixels: &'a [u8],
    pub geometry: SurfaceGeometry,
    /// Damage rectangles accumulated since the last commit, in surface-
    /// local logical coordinates. Empty means "no damage info; renderer may
    /// choose to skip the incremental update and re-upload the whole
    /// texture on the next generation change." Bounded to the surface's
    /// width/height by the server before being surfaced.
    pub damage: &'a [Rect],
    /// Opaque region declared via `wl_surface.set_opaque_region`, in surface-
    /// local logical coordinates, or `None` when the client declared no opaque
    /// region (the surface is treated as fully translucent). Lets the renderer
    /// split the blit so pixels under an opaque sub-rect use SRC-replace (no
    /// framebuffer readback) even for ARGB buffers, matching the occlusion
    /// pass's notion of opaqueness. Empty slice == no opaque region.
    pub opaque_region: Option<&'a [Rect]>,
    /// The buffer's color-space tag (`wp_color_management_v1`); `None` is
    /// the protocol default sRGB. Read when the texture is (re)created.
    pub color: Option<crate::color::ContentColor>,
}

/// A rendered view of one closing-window ghost frame (ADR-0029 close
/// transition), borrowed from the scene for the renderer. `rect` and
/// `opacity` are the interpolated values at the frame's presentation time.
/// Lives in the model — like [`SurfacePixels`] — so the renderer can consume
/// it without depending on the compositor crate.
pub struct ClosingGhostView<'a> {
    /// Presentation identity; never reused, so renderer texture caches can
    /// never collide with a previous ghost.
    pub id: usize,
    /// Interpolated rect the ghost paints at this frame.
    pub rect: Rect,
    /// The buffer's pixel dimensions at close time (pre-scale).
    pub buffer_width: i32,
    pub buffer_height: i32,
    /// Snapshotted BGRA8 pixels (shm clients); empty when `dmabuf_fd >= 0`.
    pub pixels: &'a [u8],
    /// Snapshotted compositor-owned dma-buf: raw fd and layout. `-1` for shm
    /// clients. The fd is borrowed for this frame only; the compositor's
    /// ghost record keeps ownership until the transition settles.
    pub dmabuf_fd: i32,
    pub drm_format: u32,
    pub modifier: u64,
    pub stride: u32,
    pub offset: u32,
    /// Fade-out opacity at this frame (`1.0 → 0.0`).
    pub opacity: f32,
    /// The surface's committed image description at close time
    /// (`wp_color_management_v1` tag); `None` = sRGB.
    pub color: Option<crate::color::ContentColor>,
}

/// A single-plane dma-buf-backed surface, handed from the server to the
/// renderer for zero-copy import. The `fd` is borrowed: the renderer (flux)
/// duplicates it before import; the server keeps ownership and closes it when
/// the surface backing is replaced or destroyed. `drm_format` is a DRM fourcc.
pub struct SurfaceDmabuf {
    pub id: usize,
    /// Toplevel the surface belongs to (its root's window id), or `None`
    /// for compositor-owned overlays. Lets the renderer's mapped drawing
    /// (overview) group frames per window.
    pub window: Option<tessera_types::WindowId>,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    /// Damage accumulated since the previous presented frame, in
    /// surface-local logical coordinates. Empty means the damage is unknown
    /// and consumers must conservatively treat the whole surface as changed.
    pub damage: Vec<Rect>,
    /// Stable monotonic identity of the backing `wl_buffer`, or 0 when
    /// unknown. Unlike `generation` — which is bumped on every commit — this
    /// stays constant across frames that reuse the same buffer and is not
    /// vulnerable to allocator reuse of a destroyed `wl_resource` address.
    /// `fd` cannot serve that role: it is re-`dup`ed per commit.
    pub buffer_id: u64,
    pub fd: i32,
    pub drm_format: u32,
    pub modifier: u64,
    pub offset: u32,
    pub stride: u32,
    /// Borrowed Linux sync_file fd for this generation, or -1 when implicit
    /// synchronization applies. The renderer duplicates it before import.
    pub acquire_fence: i32,
    pub geometry: SurfaceGeometry,
    /// Opaque region declared via `wl_surface.set_opaque_region`, in surface-
    /// local logical coordinates, or `None` when the client declared no opaque
    /// region. Owned (like `damage`) because the dma-buf frame may outlive the
    /// server borrow. Lets the renderer split the blit into SRC-replace opaque
    /// sub-rects and source-over remainder, matching the occlusion pass.
    pub opaque_region: Option<Vec<Rect>>,
    /// The buffer's color-space tag (`wp_color_management_v1`); `None` is
    /// the protocol default sRGB. Read when the texture is (re)created.
    pub color: Option<crate::color::ContentColor>,
}
