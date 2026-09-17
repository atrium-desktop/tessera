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
    /// surface-local logical pixels — exactly what `buffer_damage_to_surface`
    /// already does for the identity case.
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
