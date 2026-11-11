/// An integer point in compositor (logical) coordinate space.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// An integer size in logical pixels.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

/// An axis-aligned rectangle in logical coordinate space.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { w, h },
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.origin.x
            && p.y >= self.origin.y
            && p.x < self.origin.x + self.size.w
            && p.y < self.origin.y + self.size.h
    }

    /// A degenerate (zero-area) rectangle covers no pixels.
    pub fn is_empty(&self) -> bool {
        self.size.w <= 0 || self.size.h <= 0
    }

    /// The largest rectangle shared with `other`, or `None` if they do not
    /// overlap. Used by occlusion culling to intersect coverage regions.
    pub fn intersect(&self, other: Rect) -> Option<Rect> {
        let x0 = self.origin.x.max(other.origin.x);
        let y0 = self.origin.y.max(other.origin.y);
        let x1 = (self.origin.x + self.size.w).min(other.origin.x + other.size.w);
        let y1 = (self.origin.y + self.size.h).min(other.origin.y + other.size.h);
        if x1 <= x0 || y1 <= y0 {
            None
        } else {
            Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
        }
    }

    /// The smallest rectangle covering both this rectangle and `other`. An
    /// empty operand contributes nothing; two empty rectangles union to an
    /// empty rectangle. Damage accumulation uses this to combine per-source
    /// dirty regions into a partial-repaint footprint.
    pub fn union(&self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.origin.x.min(other.origin.x);
        let y0 = self.origin.y.min(other.origin.y);
        let x1 = (self.origin.x + self.size.w).max(other.origin.x + other.size.w);
        let y1 = (self.origin.y + self.size.h).max(other.origin.y + other.size.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Remove `hole` from this rectangle, returning the (up to four) disjoint
    /// rectangles that remain. An empty result means `hole` fully covered this
    /// rectangle. This is the geometric core of occlusion culling: subtracting
    /// every occluder's coverage from a target window leaves a non-empty set
    /// exactly when some part of the target is still visible.
    pub fn subtract(&self, hole: Rect) -> Vec<Rect> {
        if self.is_empty() || hole.is_empty() {
            return vec![*self];
        }
        let Some(clip) = self.intersect(hole) else {
            // No overlap: nothing removed.
            return vec![*self];
        };
        let mut out = Vec::with_capacity(4);
        let Self {
            origin: Point { x: sx, y: sy },
            size: Size { w: sw, h: sh },
        } = *self;
        let left = clip.origin.x - sx;
        let top = clip.origin.y - sy;
        let right = (clip.origin.x + clip.size.w) - (sx + sw);
        let bottom = (clip.origin.y + clip.size.h) - (sy + sh);
        if left > 0 {
            out.push(Rect::new(sx, sy, left, sh));
        }
        if right < 0 {
            let x = clip.origin.x + clip.size.w;
            out.push(Rect::new(x, sy, -right, sh));
        }
        if top > 0 {
            out.push(Rect::new(clip.origin.x, sy, clip.size.w, top));
        }
        if bottom < 0 {
            let y = clip.origin.y + clip.size.h;
            out.push(Rect::new(clip.origin.x, y, clip.size.w, -bottom));
        }
        out
    }

    /// Whether `self` is entirely covered by the union of `occluders`. A
    /// conservative, exact test built on [`Rect::subtract`]: subtracting every
    /// occluder and exhausting all fragments means no pixel of `self` remains
    /// uncovered. Occluders may overlap; the subtraction handles that.
    pub fn fully_covered_by(self, occluders: &[Rect]) -> bool {
        if self.is_empty() {
            return true;
        }
        let mut fragments = vec![self];
        for &occluder in occluders {
            if occluder.is_empty() {
                continue;
            }
            let mut next = Vec::new();
            for fragment in fragments {
                next.extend(fragment.subtract(occluder));
            }
            fragments = next;
            if fragments.is_empty() {
                return true;
            }
        }
        fragments.is_empty()
    }
}

/// Buffer transform, mirroring `wl_surface.set_buffer_transform`. The eight
/// 90-degree rotations + reflections cover everything the Wayland core
/// protocol defines. Compositing must apply this to the buffer's UVs (or
/// pre-transform into a staging texture) before drawing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum Transform {
    /// Identity. The buffer's first row is its top row, first column its left.
    #[default]
    Normal,
    /// Rotated 90 degrees counter-clockwise.
    Rotate90,
    /// Rotated 180 degrees.
    Rotate180,
    /// Rotated 270 degrees counter-clockwise.
    Rotate270,
    /// Mirrored along the vertical axis, no rotation.
    FlipHorizontal,
    /// Mirrored then rotated 90 degrees counter-clockwise.
    FlipRotate90,
    /// Mirrored then rotated 180 degrees.
    FlipRotate180,
    /// Mirrored then rotated 270 degrees counter-clockwise.
    FlipRotate270,
}

impl Transform {
    /// Apply this transform to a (width, height) pair, swapping axes for the
    /// odd rotations. Used by the renderer to compute destination dimensions
    /// that match a transformed source buffer.
    pub fn swap_axes(self) -> bool {
        matches!(
            self,
            Transform::Rotate90
                | Transform::Rotate270
                | Transform::FlipRotate90
                | Transform::FlipRotate270
        )
    }

    /// Map a damage rectangle from **buffer pixel coordinates** to
    /// **surface-local buffer pixel coordinates** (i.e. the coordinate space the
    /// surface occupies *after* this transform but *before* division by
    /// `buffer_scale`). All eight Wayland transforms are pure 90°-multiple
    /// rotations and/or axis flips, so an axis-aligned buffer rectangle maps to
    /// another axis-aligned rectangle; the result is the tight bounding box of
    /// the four transformed corners, rounded outward so no edge pixel is lost.
    ///
    /// `buffer_dims` is the buffer's own `(width, height)` in pixels. Callers
    /// then divide by `buffer_scale` (with outward rounding) to reach
    /// surface-local logical pixels; [`SurfaceGeometry::buffer_rect_to_logical`]
    /// composes that with the viewport for the full conversion.
    ///
    /// This closes the historical "transform ⇒ unmappable ⇒ full damage"
    /// fallback: a rotated/flipped client's `wl_surface.damage_buffer` no longer
    /// forces a whole-output repaint.
    pub fn map_buffer_rect_to_surface(self, rect: Rect, buffer_dims: (i32, i32)) -> Rect {
        let (bw, bh) = buffer_dims;
        // Buffer rectangle corners (inclusive-exclusive spans).
        let (bx0, by0, bx1, by1) = (
            i64::from(rect.origin.x),
            i64::from(rect.origin.y),
            i64::from(rect.origin.x + rect.size.w),
            i64::from(rect.origin.y + rect.size.h),
        );
        let bwi = i64::from(bw.max(1));
        let bhi = i64::from(bh.max(1));
        // Point transform for each of the 8 cases, accumulated as the min/max
        // of the four corner images.
        let mut sx0 = i64::MAX;
        let mut sy0 = i64::MAX;
        let mut sx1 = i64::MIN;
        let mut sy1 = i64::MIN;
        for (px, py) in [(bx0, by0), (bx1, by0), (bx0, by1), (bx1, by1)] {
            let (ux, uy) = match self {
                Transform::Normal => (px, py),
                Transform::Rotate90 => (bhi - py, px),
                Transform::Rotate180 => (bwi - px, bhi - py),
                Transform::Rotate270 => (py, bwi - px),
                Transform::FlipHorizontal => (bwi - px, py),
                Transform::FlipRotate90 => (py, px),
                Transform::FlipRotate180 => (px, bhi - py),
                Transform::FlipRotate270 => (bhi - py, bwi - px),
            };
            sx0 = sx0.min(ux);
            sy0 = sy0.min(uy);
            sx1 = sx1.max(ux);
            sy1 = sy1.max(uy);
        }
        let clamp = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        Rect::new(
            clamp(sx0),
            clamp(sy0),
            clamp(sx1.saturating_sub(sx0)),
            clamp(sy1.saturating_sub(sy0)),
        )
    }

    /// Point form of [`Self::map_buffer_rect_to_surface`]. Raw buffer pixels
    /// become post-transform buffer pixels (before `buffer_scale`).
    fn map_buffer_point_to_surface(self, point: (f64, f64), buffer_dims: (i32, i32)) -> (f64, f64) {
        let (px, py) = point;
        let bwi = buffer_dims.0.max(1) as f64;
        let bhi = buffer_dims.1.max(1) as f64;
        match self {
            Transform::Normal => (px, py),
            Transform::Rotate90 => (bhi - py, px),
            Transform::Rotate180 => (bwi - px, bhi - py),
            Transform::Rotate270 => (py, bwi - px),
            Transform::FlipHorizontal => (bwi - px, py),
            Transform::FlipRotate90 => (py, px),
            Transform::FlipRotate180 => (px, bhi - py),
            Transform::FlipRotate270 => (bhi - py, bwi - px),
        }
    }

    /// Inverse of [`Self::map_buffer_point_to_surface`]: post-transform buffer
    /// pixels back to raw buffer pixels.
    fn map_surface_point_to_buffer(self, point: (f64, f64), buffer_dims: (i32, i32)) -> (f64, f64) {
        let (ux, uy) = point;
        let bwi = buffer_dims.0.max(1) as f64;
        let bhi = buffer_dims.1.max(1) as f64;
        match self {
            Transform::Normal => (ux, uy),
            Transform::Rotate90 => (uy, bhi - ux),
            Transform::Rotate180 => (bwi - ux, bhi - uy),
            Transform::Rotate270 => (bwi - uy, ux),
            Transform::FlipHorizontal => (bwi - ux, uy),
            Transform::FlipRotate90 => (uy, ux),
            Transform::FlipRotate180 => (ux, bhi - uy),
            Transform::FlipRotate270 => (bwi - uy, bhi - ux),
        }
    }

    /// The canonical lowercase name (hyphenated) used in configuration and
    /// documentation. [`Transform::from_name`] accepts these names plus
    /// underscore aliases.
    pub fn name(self) -> &'static str {
        match self {
            Transform::Normal => "normal",
            Transform::Rotate90 => "90",
            Transform::Rotate180 => "180",
            Transform::Rotate270 => "270",
            Transform::FlipHorizontal => "flipped",
            Transform::FlipRotate90 => "flipped-90",
            Transform::FlipRotate180 => "flipped-180",
            Transform::FlipRotate270 => "flipped-270",
        }
    }

    /// Resolve a transform by its config name: the canonical
    /// [`Transform::name`] forms plus underscore aliases (`flipped_90` and
    /// friends). Matching is exact and lowercase; unknown names return
    /// `None` so the caller can diagnose them.
    pub fn from_name(s: &str) -> Option<Transform> {
        Some(match s {
            "normal" => Transform::Normal,
            "90" => Transform::Rotate90,
            "180" => Transform::Rotate180,
            "270" => Transform::Rotate270,
            "flipped" => Transform::FlipHorizontal,
            "flipped-90" | "flipped_90" => Transform::FlipRotate90,
            "flipped-180" | "flipped_180" => Transform::FlipRotate180,
            "flipped-270" | "flipped_270" => Transform::FlipRotate270,
            _ => return None,
        })
    }
}

// --- Surface Geometry & Mapping ---

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

/// The stable surface-local logical extent for a post-transform buffer size.
///
/// This is the one precedence rule: `wp_viewport.set_destination` wins, then
/// the source crop size, then the buffer extent divided by `buffer_scale`.
/// Damage, the renderer's destination arithmetic, and ownership all consume it.
pub fn logical_surface_extent(
    post_transform_dims: (i32, i32),
    buffer_scale: i32,
    viewport_src: Option<Rect>,
    viewport_dst: Option<Size>,
) -> (i32, i32) {
    if let Some(dst) = viewport_dst {
        return (dst.w.max(1), dst.h.max(1));
    }
    if let Some(src) = viewport_src {
        return (src.size.w.max(1), src.size.h.max(1));
    }
    let scale = buffer_scale.max(1) as f32;
    (
        (post_transform_dims.0 as f32 / scale).round().max(1.0) as i32,
        (post_transform_dims.1 as f32 / scale).round().max(1.0) as i32,
    )
}

impl SurfaceGeometry {
    /// Post-transform buffer extent before `buffer_scale` is applied. A 90° or
    /// 270° buffer transform swaps the raw buffer's axes.
    fn post_transform_extent(&self, buffer_width: i32, buffer_height: i32) -> (i32, i32) {
        if self.transform.swap_axes() {
            (buffer_height.max(1), buffer_width.max(1))
        } else {
            (buffer_width.max(1), buffer_height.max(1))
        }
    }

    /// Full surface-local logical extent of the committed buffer, before any
    /// `wp_viewport` crop or destination.
    pub fn local_extent(&self, buffer_width: i32, buffer_height: i32) -> (i32, i32) {
        let post_transform = self.post_transform_extent(buffer_width, buffer_height);
        logical_surface_extent(post_transform, self.buffer_scale, None, None)
    }

    /// The source rectangle the renderer samples, in surface-local logical
    /// coordinates (after buffer transform/scale, before viewport crop).
    pub fn surface_source(&self, buffer_width: i32, buffer_height: i32) -> Rect {
        self.viewport_src.unwrap_or_else(|| {
            let (width, height) = self.local_extent(buffer_width, buffer_height);
            Rect::new(0, 0, width, height)
        })
    }

    /// Stable surface-local logical extent: the coordinate space damage
    /// rectangles are expressed in. `wp_viewport.set_destination` wins;
    /// otherwise the source size is the surface size.
    pub fn surface_extent(&self, buffer_width: i32, buffer_height: i32) -> (i32, i32) {
        let post_transform = self.post_transform_extent(buffer_width, buffer_height);
        logical_surface_extent(
            post_transform,
            self.buffer_scale,
            self.viewport_src,
            self.viewport_dst,
        )
    }

    /// Map one surface-local logical point to raw buffer pixels. This is the
    /// inverse of the renderer's sampling pipeline:
    ///
    /// `surface -> source-local (viewport crop/scale) -> post-transform buffer
    /// px (buffer_scale) -> raw buffer px (buffer transform)`.
    fn surface_point_to_buffer(
        &self,
        point: (f64, f64),
        buffer_width: i32,
        buffer_height: i32,
    ) -> (f64, f64) {
        let source = self.surface_source(buffer_width, buffer_height);
        let (surface_w, surface_h) = self.surface_extent(buffer_width, buffer_height);
        let scale = self.buffer_scale.max(1) as f64;
        let local_x = source.origin.x as f64 + point.0 * source.size.w as f64 / surface_w as f64;
        let local_y = source.origin.y as f64 + point.1 * source.size.h as f64 / surface_h as f64;
        self.transform.map_surface_point_to_buffer(
            (local_x * scale, local_y * scale),
            (buffer_width, buffer_height),
        )
    }

    /// Map one raw buffer point to surface-local logical coordinates.
    fn buffer_point_to_surface(
        &self,
        point: (f64, f64),
        buffer_width: i32,
        buffer_height: i32,
    ) -> (f64, f64) {
        let source = self.surface_source(buffer_width, buffer_height);
        let (surface_w, surface_h) = self.surface_extent(buffer_width, buffer_height);
        let scale = self.buffer_scale.max(1) as f64;
        let (tx, ty) = self
            .transform
            .map_buffer_point_to_surface(point, (buffer_width, buffer_height));
        let local_x = tx / scale;
        let local_y = ty / scale;
        (
            (local_x - source.origin.x as f64) * surface_w as f64 / source.size.w.max(1) as f64,
            (local_y - source.origin.y as f64) * surface_h as f64 / source.size.h.max(1) as f64,
        )
    }

    /// Map a damage rectangle from **surface-local logical coordinates** to
    /// **raw buffer pixel coordinates**.
    ///
    /// This is the exact inverse of [`Self::buffer_rect_to_logical`]. It is
    /// deliberately rect-aware rather than a scale factor: `wp_viewport` has an
    /// origin, and dropping it copies the wrong buffer region — including
    /// pixels outside the commit's damage, which may still be stale in an
    /// alternating buffer.
    pub fn surface_rect_to_buffer(
        &self,
        rect: Rect,
        buffer_width: i32,
        buffer_height: i32,
    ) -> Option<Rect> {
        let (surface_w, surface_h) = self.surface_extent(buffer_width, buffer_height);
        if surface_w <= 0 || surface_h <= 0 {
            return None;
        }
        let clipped = rect.intersect(Rect::new(0, 0, surface_w, surface_h))?;
        let x0 = clipped.origin.x as f64;
        let y0 = clipped.origin.y as f64;
        let x1 = (clipped.origin.x as i64 + clipped.size.w as i64) as f64;
        let y1 = (clipped.origin.y as i64 + clipped.size.h as i64) as f64;
        let mut bx0 = f64::MAX;
        let mut by0 = f64::MAX;
        let mut bx1 = f64::MIN;
        let mut by1 = f64::MIN;
        for corner in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let (bx, by) = self.surface_point_to_buffer(corner, buffer_width, buffer_height);
            bx0 = bx0.min(bx);
            by0 = by0.min(by);
            bx1 = bx1.max(bx);
            by1 = by1.max(by);
        }
        let clamp = |value: f64, max: i32| value.clamp(0.0, max as f64);
        let bx0 = clamp(bx0.floor(), buffer_width);
        let by0 = clamp(by0.floor(), buffer_height);
        let bx1 = clamp(bx1.ceil(), buffer_width);
        let by1 = clamp(by1.ceil(), buffer_height);
        (bx1 > bx0 && by1 > by0).then(|| {
            Rect::new(
                bx0 as i32,
                by0 as i32,
                (bx1 - bx0) as i32,
                (by1 - by0) as i32,
            )
        })
    }

    /// Map a damage rectangle from **raw buffer pixel coordinates** to
    /// **surface-local logical coordinates**, honouring buffer transform,
    /// buffer scale, and `wp_viewport`.
    ///
    /// Pipeline: raw buffer px -> transform -> buffer scale -> viewport source
    /// crop + destination stretch. The result is clipped to the surface's
    /// logical extent; an empty rectangle means the damage reaches no visible
    /// surface pixel.
    pub fn buffer_rect_to_logical(
        &self,
        rect: Rect,
        buffer_width: i32,
        buffer_height: i32,
    ) -> Rect {
        let (surface_w, surface_h) = self.surface_extent(buffer_width, buffer_height);
        if surface_w <= 0 || surface_h <= 0 {
            return Rect::new(0, 0, 0, 0);
        }
        let x0 = rect.origin.x as f64;
        let y0 = rect.origin.y as f64;
        let x1 = (rect.origin.x as i64 + rect.size.w as i64) as f64;
        let y1 = (rect.origin.y as i64 + rect.size.h as i64) as f64;
        let mut sx0 = f64::MAX;
        let mut sy0 = f64::MAX;
        let mut sx1 = f64::MIN;
        let mut sy1 = f64::MIN;
        for corner in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let (sx, sy) = self.buffer_point_to_surface(corner, buffer_width, buffer_height);
            sx0 = sx0.min(sx);
            sy0 = sy0.min(sy);
            sx1 = sx1.max(sx);
            sy1 = sy1.max(sy);
        }
        let mapped = Rect::new(
            sx0.floor() as i32,
            sy0.floor() as i32,
            (sx1.ceil() - sx0.floor()) as i32,
            (sy1.ceil() - sy0.floor()) as i32,
        );
        mapped
            .intersect(Rect::new(0, 0, surface_w, surface_h))
            .unwrap_or(Rect::new(0, 0, 0, 0))
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
    pub window: Option<crate::WindowId>,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    pub pixels: &'a [u8],
    pub geometry: SurfaceGeometry,
    /// Damage rectangles accumulated since the last commit, in surface-local
    /// logical coordinates — the stable post-viewport extent. `damage_full`
    /// carries the explicit whole-surface verdict; an empty list with
    /// `damage_full == false` means no visible damage, not "unknown".
    pub damage: &'a [Rect],
    /// Whether the surface changed as a whole and cannot be expressed by
    /// [`Self::damage`]. This is the explicit replacement for the historical
    /// "empty list means Full" sentinel.
    pub damage_full: bool,
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
    pub window: Option<crate::WindowId>,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    /// Damage accumulated since the previous presented frame, in surface-local
    /// logical coordinates — the stable post-viewport extent. `damage_full`
    /// carries the explicit whole-surface verdict; an empty list with
    /// `damage_full == false` means no visible damage, not "unknown".
    pub damage: Vec<Rect>,
    /// Whether the surface changed as a whole and cannot be expressed by
    /// [`Self::damage`]. This is the explicit replacement for the historical
    /// "empty list means Full" sentinel.
    pub damage_full: bool,
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
    /// Presentation hint requested by the client (`wp_tearing_control_v1`).
    /// `Async` permits asynchronous page flips (tearing) if and only if
    /// the frame qualifies for direct scanout and hardware supports it.
    pub presentation_hint: PresentationHint,
}

/// Presentation hint requested by a client surface (`wp_tearing_control_v1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresentationHint {
    /// Synchronized to the vertical blanking period (tear-free).
    #[default]
    Vsync,
    /// Asynchronous presentation with minimal latency where tearing is acceptable.
    Async,
}

impl PresentationHint {
    #[inline]
    pub const fn is_async(self) -> bool {
        matches!(self, Self::Async)
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;
    #[cfg(test)]
    mod tests {
        use super::*;
        use proptest::prelude::*;

        fn transform_strategy() -> impl Strategy<Value = Transform> {
            prop_oneof![
                Just(Transform::Normal),
                Just(Transform::Rotate90),
                Just(Transform::Rotate180),
                Just(Transform::Rotate270),
                Just(Transform::FlipHorizontal),
                Just(Transform::FlipRotate90),
                Just(Transform::FlipRotate180),
                Just(Transform::FlipRotate270),
            ]
        }

        fn rect_within(width: i32, height: i32) -> impl Strategy<Value = Rect> {
            let width = width.max(1);
            let height = height.max(1);
            (0..width, 0..height).prop_flat_map(move |(x, y)| {
                (Just(x), Just(y), 1..=(width - x), 1..=(height - y))
                    .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
            })
        }

        fn geometry_case() -> impl Strategy<Value = (SurfaceGeometry, i32, i32)> {
            (1i32..=128, 1i32..=128, transform_strategy(), 1i32..=4).prop_flat_map(
                |(local_width, local_height, transform, buffer_scale)| {
                    // Valid Wayland state: the raw buffer is exactly the
                    // surface-local extent times buffer_scale, with the
                    // transform applied to the axes.
                    let (buffer_width, buffer_height) = if transform.swap_axes() {
                        (local_height * buffer_scale, local_width * buffer_scale)
                    } else {
                        (local_width * buffer_scale, local_height * buffer_scale)
                    };
                    let viewport_src = prop::option::of(rect_within(local_width, local_height));
                    let viewport_dst = prop::option::of(
                        (1i32..=1024, 1i32..=1024).prop_map(|(w, h)| Size { w, h }),
                    );
                    (
                        Just(buffer_width),
                        Just(buffer_height),
                        Just(transform),
                        Just(buffer_scale),
                        viewport_src,
                        viewport_dst,
                    )
                        .prop_map(
                            |(
                                buffer_width,
                                buffer_height,
                                transform,
                                buffer_scale,
                                viewport_src,
                                viewport_dst,
                            )| {
                                (
                                    SurfaceGeometry {
                                        transform,
                                        buffer_scale,
                                        viewport_src,
                                        viewport_dst,
                                        ..Default::default()
                                    },
                                    buffer_width,
                                    buffer_height,
                                )
                            },
                        )
                },
            )
        }

        fn geometry_with_surface_rect() -> impl Strategy<Value = (SurfaceGeometry, i32, i32, Rect)>
        {
            geometry_case().prop_flat_map(|(geometry, buffer_width, buffer_height)| {
                let (width, height) = geometry.surface_extent(buffer_width, buffer_height);
                rect_within(width, height)
                    .prop_map(move |rect| (geometry, buffer_width, buffer_height, rect))
            })
        }

        fn geometry_with_source_rect() -> impl Strategy<Value = (SurfaceGeometry, i32, i32, Rect)> {
            geometry_case().prop_flat_map(|(geometry, buffer_width, buffer_height)| {
                let (width, height) = geometry.surface_extent(buffer_width, buffer_height);
                let visible = geometry
                    .surface_rect_to_buffer(
                        Rect::new(0, 0, width, height),
                        buffer_width,
                        buffer_height,
                    )
                    .expect("the visible surface must map to a buffer rect");
                rect_within(visible.size.w, visible.size.h).prop_map(move |rect| {
                    (
                        geometry,
                        buffer_width,
                        buffer_height,
                        Rect::new(
                            visible.origin.x + rect.origin.x,
                            visible.origin.y + rect.origin.y,
                            rect.size.w,
                            rect.size.h,
                        ),
                    )
                })
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

            #[test]
            fn every_surface_damage_rect_round_trips_through_the_buffer(
                (geometry, buffer_width, buffer_height, rect) in geometry_with_surface_rect()
            ) {
                let raw = geometry
                    .surface_rect_to_buffer(rect, buffer_width, buffer_height)
                    .expect("visible surface damage must map into the buffer");
                let back = geometry.buffer_rect_to_logical(raw, buffer_width, buffer_height);
                prop_assert!(
                    rect.fully_covered_by(&[back]),
                    "surface {:?} -> raw {:?} -> surface {:?} did not cover the input (geometry {:?}, buffer {}x{})",
                    rect, raw, back, geometry, buffer_width, buffer_height
                );
            }

            #[test]
            fn every_visible_buffer_damage_rect_round_trips_through_the_surface(
                (geometry, buffer_width, buffer_height, rect) in geometry_with_source_rect()
            ) {
                let surface = geometry.buffer_rect_to_logical(rect, buffer_width, buffer_height);
                prop_assert!(
                    !surface.is_empty(),
                    "visible buffer damage {:?} mapped to an empty surface rect (geometry {:?}, buffer {}x{})",
                    rect, geometry, buffer_width, buffer_height
                );
                let back = geometry
                    .surface_rect_to_buffer(surface, buffer_width, buffer_height)
                    .expect("surface damage from a visible buffer rect must map back");
                prop_assert!(
                    rect.fully_covered_by(&[back]),
                    "buffer {:?} -> surface {:?} -> buffer {:?} did not cover the input (geometry {:?}, buffer {}x{})",
                    rect, surface, back, geometry, buffer_width, buffer_height
                );
            }
        }

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
        fn buffer_rect_to_logical_round_trips_buffer_scale_with_outward_rounding() {
            // No viewport: the mapping degenerates to the buffer-scale division
            // with outward rounding.
            let geometry = SurfaceGeometry {
                buffer_scale: 2,
                ..Default::default()
            };
            let mapped = geometry.buffer_rect_to_logical(Rect::new(3, 5, 8, 10), 200, 200);
            // x: floor(3/2)=1..ceil(11/2)=6 → w=5;
            // y: floor(5/2)=2..ceil(15/2)=8 → h=6.
            assert_eq!(mapped, Rect::new(1, 2, 5, 6));
        }

        #[test]
        fn surface_rect_to_buffer_maps_fractional_scale_viewport() {
            // Chrome on a 2x output: surface damage is in the 1200x800
            // destination space; the raw buffer is 2400x1600.
            let geometry = SurfaceGeometry {
                viewport_dst: Some(Size { w: 1200, h: 800 }),
                ..Default::default()
            };
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(50, 30, 8, 8), 2400, 1600),
                Some(Rect::new(100, 60, 16, 16))
            );
        }

        #[test]
        fn surface_rect_to_buffer_honours_viewport_source_origin() {
            // A crop at (100, 50) draws the source origin at surface (0, 0).
            // Surface damage must therefore be offset back into the buffer.
            let geometry = SurfaceGeometry {
                viewport_src: Some(Rect::new(100, 50, 400, 400)),
                viewport_dst: Some(Size { w: 200, h: 200 }),
                ..Default::default()
            };
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(0, 0, 200, 200), 800, 600),
                Some(Rect::new(100, 50, 400, 400))
            );
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(0, 0, 100, 100), 800, 600),
                Some(Rect::new(100, 50, 200, 200))
            );
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(200, 0, 10, 10), 800, 600),
                None,
                "damage outside the destination must not invent buffer pixels"
            );
        }

        #[test]
        fn surface_rect_to_buffer_crops_without_a_destination() {
            let geometry = SurfaceGeometry {
                viewport_src: Some(Rect::new(100, 50, 400, 300)),
                ..Default::default()
            };
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(0, 0, 400, 300), 800, 600),
                Some(Rect::new(100, 50, 400, 300))
            );
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(25, 30, 50, 40), 800, 600),
                Some(Rect::new(125, 80, 50, 40))
            );
        }

        #[test]
        fn surface_rect_to_buffer_composes_buffer_scale_and_crop() {
            // buffer_scale 2 turns the 800x600 raw buffer into a 400x300
            // local space; the viewport crop then starts at local (50, 50).
            let geometry = SurfaceGeometry {
                buffer_scale: 2,
                viewport_src: Some(Rect::new(50, 50, 200, 150)),
                viewport_dst: Some(Size { w: 100, h: 75 }),
                ..Default::default()
            };
            assert_eq!(
                geometry.surface_rect_to_buffer(Rect::new(0, 0, 100, 75), 800, 600),
                Some(Rect::new(100, 100, 400, 300))
            );
        }

        #[test]
        fn surface_rect_to_buffer_inverts_the_wayland_transforms() {
            let dims = (100, 60);
            let raw = Rect::new(10, 15, 20, 10);
            for transform in [
                Transform::Normal,
                Transform::Rotate90,
                Transform::Rotate180,
                Transform::Rotate270,
                Transform::FlipHorizontal,
                Transform::FlipRotate90,
                Transform::FlipRotate180,
                Transform::FlipRotate270,
            ] {
                let geometry = SurfaceGeometry {
                    transform,
                    ..Default::default()
                };
                let surface = transform.map_buffer_rect_to_surface(raw, dims);
                assert_eq!(
                    geometry.surface_rect_to_buffer(surface, dims.0, dims.1),
                    Some(raw),
                    "{transform:?}"
                );
            }
        }

        #[test]
        fn buffer_and_surface_damage_conversions_round_trip_under_viewport() {
            let dims = (800, 600);
            let geometry = SurfaceGeometry {
                buffer_scale: 2,
                viewport_src: Some(Rect::new(50, 50, 200, 150)),
                viewport_dst: Some(Size { w: 100, h: 75 }),
                ..Default::default()
            };
            let raw = Rect::new(100, 100, 400, 300);
            // raw -> surface clips to the destination and removes the crop
            // offset; the inverse then puts it back into raw buffer space.
            let surface = geometry.buffer_rect_to_logical(raw, dims.0, dims.1);
            assert_eq!(surface, Rect::new(0, 0, 100, 75));
            assert_eq!(
                geometry.surface_rect_to_buffer(surface, dims.0, dims.1),
                Some(raw)
            );
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
}
