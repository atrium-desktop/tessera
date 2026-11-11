use tessera_primitives::Rect;
use tessera_primitives::SurfaceDmabuf;
use tessera_primitives::SurfaceGeometry;
use tessera_primitives::SurfacePixels;

/// Hard bound for one frame's damage set. Wayland surface commits are
/// independently capped upstream, but combining many surfaces could otherwise
/// grow this Vec without bound. Above the cap we conservatively fall back to
/// one bounding box; the engine turns an over-capacity delta into its explicit
/// full verdict.
const MAX_FRAME_DAMAGE_RECTS: usize = 64;

/// Output-level damage for one presentation frame.
///
/// The pipeline is conservative by design: any uncertainty resolves to
/// [`FrameDamage::Full`] (or to not skipping), because a missed region leaves
/// stale pixels on screen while an over-large region only costs a hint.
///
/// `Area` carries a *list* of disjoint physical-pixel rectangles rather than a
/// single bounding box, so two unrelated dirty regions (e.g. a video window
/// plus a status-clock tick) reach the KMS `FB_DAMAGE_CLIPS` hint as two clips
/// instead of being unioned into one rect that spans the whole output — which
/// would defeat PSR2 / panel self-refresh. Vulkan's render pass takes a single
/// `renderArea`, so the renderer feeds it the *union* of the list (see
/// [`FrameDamage::area_union`]); the per-rect fidelity is preserved only for
/// the KMS hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameDamage {
    /// Nothing visible changed: rendering and presentation may be skipped.
    None,
    /// Conservative full-output damage.
    Full,
    /// One or more dirty rectangles in physical desktop (framebuffer) pixels.
    /// Always non-empty; empty damage is represented as [`FrameDamage::None`].
    Area(Vec<Rect>),
}

impl FrameDamage {
    pub fn from_rects(rects: Vec<Rect>) -> Self {
        let rects = normalize_damage_rects(rects);
        if rects.is_empty() {
            FrameDamage::None
        } else {
            FrameDamage::Area(rects)
        }
    }
    /// The single-rect union of the damage, in physical pixels, or `None` when
    /// there is no area damage (None) or the whole output is dirty (Full).
    /// Used wherever a single rectangle is required (Vulkan `renderArea`).
    pub fn area_union(&self) -> Option<Rect> {
        match self {
            FrameDamage::Area(rects) => {
                let mut union: Option<Rect> = None;
                for r in rects {
                    union_bbox(&mut union, *r);
                }
                union
            }
            _ => None,
        }
    }

    /// The list of physical damage rectangles, or `None` for None/Full. The
    /// KMS `FB_DAMAGE_CLIPS` hint consumes this directly. Borrows the rects so
    /// the caller can still inspect the original `FrameDamage` afterwards.
    pub fn area_rects(&self) -> Option<&[Rect]> {
        match self {
            FrameDamage::Area(rects) => Some(rects),
            _ => None,
        }
    }

    /// Add a set of physical rectangles to this damage verdict.
    pub fn with_rects(self, rects: impl IntoIterator<Item = Rect>) -> Self {
        union_frame_damage(self, FrameDamage::from_rects(rects.into_iter().collect()))
    }
}

fn normalize_damage_rects(rects: Vec<Rect>) -> Vec<Rect> {
    // Overflow degrades to the bounding box of the *whole input*, not of the
    // processed prefix. The decomposition below can stop early; rectangles it
    // has not visited yet must still be covered, or a modular overflow would
    // silently strand changed pixels.
    let all_bbox =
        rects
            .iter()
            .copied()
            .filter(|rect| !rect.is_empty())
            .fold(None, |bbox, rect| {
                let mut bbox = bbox;
                union_bbox(&mut bbox, rect);
                bbox
            });
    let mut disjoint: Vec<Rect> = Vec::with_capacity(rects.len());
    let mut pending: Vec<Rect> = Vec::new();
    let mut next: Vec<Rect> = Vec::new();
    let mut overflow = false;

    for rect in rects {
        if rect.is_empty() {
            continue;
        }
        pending.clear();
        pending.push(rect);

        for existing in &disjoint {
            if pending.is_empty() {
                break;
            }
            let ex0 = existing.origin.x;
            let ey0 = existing.origin.y;
            let ex1 = ex0.saturating_add(existing.size.w);
            let ey1 = ey0.saturating_add(existing.size.h);

            next.clear();
            for cand in pending.drain(..) {
                let cx0 = cand.origin.x;
                let cy0 = cand.origin.y;
                let cx1 = cx0.saturating_add(cand.size.w);
                let cy1 = cy0.saturating_add(cand.size.h);

                let ix0 = cx0.max(ex0);
                let iy0 = cy0.max(ey0);
                let ix1 = cx1.min(ex1);
                let iy1 = cy1.min(ey1);

                if !(ix0 < ix1 && iy0 < iy1) {
                    next.push(cand);
                    continue;
                }
                if cy0 < iy0 {
                    next.push(Rect::new(cx0, cy0, cx1 - cx0, iy0 - cy0));
                }
                if iy1 < cy1 {
                    next.push(Rect::new(cx0, iy1, cx1 - cx0, cy1 - iy1));
                }
                if cx0 < ix0 {
                    next.push(Rect::new(cx0, iy0, ix0 - cx0, iy1 - iy0));
                }
                if ix1 < cx1 {
                    next.push(Rect::new(ix1, iy0, cx1 - ix1, iy1 - iy0));
                }
            }
            std::mem::swap(&mut pending, &mut next);
            if disjoint.len() + pending.len() > MAX_FRAME_DAMAGE_RECTS * 4 {
                overflow = true;
                break;
            }
        }

        if overflow {
            return all_bbox.into_iter().collect();
        }
        disjoint.append(&mut pending);
    }

    if disjoint.len() > MAX_FRAME_DAMAGE_RECTS {
        all_bbox.into_iter().collect()
    } else {
        disjoint
            .sort_unstable_by_key(|rect| (rect.origin.y, rect.origin.x, rect.size.h, rect.size.w));
        disjoint
    }
}

/// What one frame's damage assessment concluded.
///
/// There is deliberately one repaint verdict rather than one per consumer.
/// The split that used to live here (`output` vs `backdrop_source`) existed so
/// a Dock hover or a clock tick could not force a re-capture and re-blur of
/// unchanged client pixels. Chrome draws no material at all any more
/// ([ADR-0213](../../../docs/adr/0213-flat-chrome-and-removal-of-the-status-hud.md)),
/// so there is no capture to invalidate: `backdrop_source` was written every
/// frame and read by nobody. Keeping a second damage axis that nothing consumes
/// is a standing invitation for the next reader to believe a backdrop cache
/// still exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessedFrameDamage {
    /// The current frame's delta, in physical output pixels. This is the input
    /// to Flux's epoch ledger; it is not the repaint scope and not the KMS
    /// scanout hint (both are derived by the engine).
    pub output: FrameDamage,
    /// Resource ids of the client surfaces this assessment sampled. The
    /// presented frame consumes exactly these surfaces' damage baselines, so
    /// they — and only they — may be marked presented after a successful
    /// submission (`[INV-ARCH-70]`). A surface the frame never sampled
    /// (hidden workspace, fully occluded, minimized) must keep its latched
    /// `wl_buffer` releasable immediately rather than deferring it to vblank.
    pub sampled_surface_ids: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct DamageAssessment {
    pub had_input: bool,
    /// `had_input` came from pointer motion alone (no buttons, keys, or
    /// touch). Pointer motion is the one input class whose visual effect is
    /// exactly the cursor sprite plus the chrome band it hovers, so the
    /// assessment can degrade its repaint to those regions instead of a
    /// full-output composite.
    pub input_pointer_only: bool,
    pub session_locked: bool,
    pub cursor_hidden: bool,
    pub cursor_shape: u32,
    pub software_cursor: bool,
    /// Current cursor position in output-logical coordinates.
    pub cursor_position: (f32, f32),
    /// Largest cursor sprite edge in logical pixels for the active theme and
    /// scale (hotspot included).
    pub cursor_extent: f32,
    pub scale: f32,
    pub physical_size: (u32, u32),
}

pub fn union_frame_damage(left: FrameDamage, right: FrameDamage) -> FrameDamage {
    match (left, right) {
        (FrameDamage::Full, _) | (_, FrameDamage::Full) => FrameDamage::Full,
        (FrameDamage::None, FrameDamage::Area(rects))
        | (FrameDamage::Area(rects), FrameDamage::None) => FrameDamage::from_rects(rects),
        (FrameDamage::None, FrameDamage::None) => FrameDamage::None,
        (FrameDamage::Area(mut left), FrameDamage::Area(right)) => {
            left.extend(right);
            FrameDamage::from_rects(left)
        }
    }
}

impl FrameDamage {
    /// Convert to the engine's native damage verdict.
    ///
    /// Flux owns the per-image epoch ledger; the composition root supplies the
    /// current frame's change set and receives the exact repaint scope back.
    /// There is deliberately no second per-image vocabulary here.
    pub fn to_flux_damage(&self) -> flux::Damage {
        match self {
            FrameDamage::None => flux::Damage::None,
            FrameDamage::Full => flux::Damage::Full,
            FrameDamage::Area(rects) => flux::Damage::Area(
                rects
                    .iter()
                    .map(|r| flux::CanvasRenderArea {
                        x: r.origin.x as f32,
                        y: r.origin.y as f32,
                        width: r.size.w.max(0) as f32,
                        height: r.size.h.max(0) as f32,
                    })
                    .collect(),
            ),
        }
    }

    /// Convert an engine verdict into the compositor's value type.
    ///
    /// Flux rectangles are physical-pixel floats; the compositor's damage is
    /// integer pixel authority, so map outward to avoid stranding a partially
    /// covered edge pixel.
    pub fn from_flux_damage(damage: flux::Damage) -> Self {
        match damage {
            flux::Damage::None => FrameDamage::None,
            flux::Damage::Full => FrameDamage::Full,
            flux::Damage::Area(areas) => {
                let rects: Vec<Rect> = areas
                    .into_iter()
                    .filter_map(|a| {
                        let x0 = a.x.floor() as i32;
                        let y0 = a.y.floor() as i32;
                        let x1 = (a.x + a.width).ceil() as i32;
                        let y1 = (a.y + a.height).ceil() as i32;
                        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
                    })
                    .collect();
                FrameDamage::from_rects(rects)
            }
        }
    }
}

/// Damage contributed by client surface commits, in compositor logical
/// coordinates. `Area` preserves the *list* of disjoint dirty rectangles so
/// unrelated regions (a video window and a clock tick) are not unioned into
/// one spanning rect before reaching the KMS hint.
///
/// There is no `Full` tier. A surface commit is always expressible as
/// rectangles because [ADR-0203] maps it through `SurfacePlacement`; the only
/// way a commit damages everything is a surface that occupies everything, and
/// that arrives as a full-output `Area` rect. Keeping an unreachable `Full`
/// variant would let a future caller reintroduce the blanket repaint without
/// noticing.
///
/// [ADR-0203]: ../../../../docs/adr/0203-damage-is-a-placement-mapped-rectangle.md
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientDamage {
    None,
    Area(Vec<Rect>),
}

impl ClientDamage {
    /// Map logical damage rectangles onto the physical framebuffer. Each
    /// logical rect is mapped independently, preserving disjoint regions for
    /// the KMS hint. Rectangles outside the visible framebuffer contribute no
    /// pixels and are discarded without degrading the whole frame to Full.
    /// Only invalid coordinate systems (non-finite or non-positive scale, or
    /// zero-sized physical target) degrade to `FrameDamage::Full`.
    pub fn into_frame_damage(self, scale: f32, physical_size: (u32, u32)) -> FrameDamage {
        if !scale.is_finite() || scale <= 0.0 || physical_size.0 == 0 || physical_size.1 == 0 {
            return FrameDamage::Full;
        }
        match self {
            ClientDamage::None => FrameDamage::None,
            ClientDamage::Area(logical) => {
                let mut physical = Vec::with_capacity(logical.len());
                for rect in logical {
                    if let Some(p) = logical_to_physical(rect, scale, physical_size) {
                        physical.push(p);
                    }
                }
                if physical.is_empty() {
                    FrameDamage::None
                } else {
                    FrameDamage::from_rects(physical)
                }
            }
        }
    }
}

/// One presentation-visible client surface, narrowed to the fields the
/// damage baseline diff consumes. The composition root feeds
/// `SurfacePixels`/`SurfaceDmabuf` frame lists (SHM, dma-buf, overlay, and
/// lock surfaces) through this seam; the tracker never sees engine handles
/// or pixel buffers.
pub trait SurfaceDamageFrame {
    fn id(&self) -> usize;
    fn generation(&self) -> u64;
    /// Surface-local logical damage rectangles since the last commit. An
    /// empty list with `damage_full == false` means no visible change.
    fn damage(&self) -> &[Rect];
    /// The explicit whole-surface verdict. This is never inferred from an
    /// empty rectangle list.
    fn damage_full(&self) -> bool;
    /// Where this surface's buffer lands on the output this frame.
    fn placement(&self) -> SurfacePlacement;
}

impl SurfaceDamageFrame for SurfacePixels<'_> {
    fn id(&self) -> usize {
        self.id
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn damage(&self) -> &[Rect] {
        self.damage
    }
    fn damage_full(&self) -> bool {
        self.damage_full
    }
    fn placement(&self) -> SurfacePlacement {
        SurfacePlacement::resolve(&self.geometry, self.width, self.height)
    }
}

impl SurfaceDamageFrame for SurfaceDmabuf {
    fn id(&self) -> usize {
        self.id
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn damage(&self) -> &[Rect] {
        &self.damage
    }
    fn damage_full(&self) -> bool {
        self.damage_full
    }
    fn placement(&self) -> SurfacePlacement {
        SurfacePlacement::resolve(&self.geometry, self.width, self.height)
    }
}

/// Per-surface state as of the previous damage assessment. The placement
/// belongs in the baseline alongside content generation: moving, resizing, or
/// re-viewporting a surface exposes its old pixels even when the buffer itself
/// did not change.
#[derive(Clone, Copy)]
struct SurfaceDamageBaseline {
    generation: u64,
    placement: SurfacePlacement,
}

/// Where one surface's stable surface-local logical coordinate space lands on
/// the output.
///
/// Every damage rectangle the compositor receives — `wl_surface.damage`, or
/// `wl_surface.damage_buffer` after its commit-time conversion — lives in this
/// space. `wp_viewport` has already determined the surface's size: the source
/// crop selects buffer pixels, and the destination (or source size) *is* the
/// surface extent. Damage therefore maps to output pixels one-for-one, except
/// for an in-flight transition that temporarily draws the same content at a
/// different size.
///
/// The source rectangle is retained as placement identity: a viewport crop
/// change alters which buffer pixels are visible even when the destination
/// extent is unchanged, so it must invalidate the damage baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfacePlacement {
    /// Output-logical origin the visible content is drawn at.
    pub origin: tessera_primitives::Point,
    /// Source rectangle sampled by the renderer, in surface-local logical
    /// coordinates (after buffer transform/scale, before viewport crop).
    pub source: Rect,
    /// Stable surface-local logical extent: the coordinate space damage
    /// rectangles are expressed in.
    pub surface_extent: (i32, i32),
    /// Extent the visible content is drawn at on the output: an in-flight
    /// transition's interpolated size, else the surface extent.
    pub dest_extent: (i32, i32),
}

impl SurfacePlacement {
    /// Resolve the placement the renderer will draw this frame with.
    ///
    /// Mirrors `destination_size` plus the transition override in
    /// `tessera-composite::lib` (the draw path two calls down) and
    /// `SurfacePresentation::logical_extent` in `tessera-desktop` (the
    /// ownership path). The draw path and this function must agree exactly,
    /// because damage validity is precisely "the renderer drew the same
    /// rectangle the damage pass assumed"; `placement_agreement` in
    /// `tessera-composite` asserts that over a matrix of states.
    ///
    /// `logical_extent` deliberately ignores `transition_size` — it answers
    /// "which model rect does this surface own", which does not move while a
    /// transition animates — and agrees on every non-transition case.
    pub fn resolve(geometry: &SurfaceGeometry, width: i32, height: i32) -> Self {
        let source = geometry.surface_source(width, height);
        let surface_extent = geometry.surface_extent(width, height);
        let mut dest_extent = surface_extent;
        // An in-flight transition (ADR-0029) draws the texture at the
        // interpolated rect and overrides the viewport-derived size.
        if let Some(size) = geometry.transition_size {
            dest_extent = (size.w.max(1), size.h.max(1));
        }
        Self {
            origin: geometry.position,
            source,
            surface_extent,
            dest_extent,
        }
    }

    /// The output-logical rectangle this placement draws into.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.dest_extent.0,
            self.dest_extent.1,
        )
    }

    /// Map one surface-local damage rectangle onto the output. Outward
    /// rounding keeps every touched output pixel inside the damage. `None`
    /// means the rectangle reaches no visible surface pixel.
    ///
    /// Viewport scaling is deliberately absent here: the damage coordinate
    /// space is already the post-viewport surface extent. Only an in-flight
    /// transition changes the output scale.
    pub fn place(&self, surface_rect: Rect) -> Option<Rect> {
        if self.surface_extent.0 <= 0 || self.surface_extent.1 <= 0 {
            return None;
        }
        let clipped = surface_rect.intersect(Rect::new(
            0,
            0,
            self.surface_extent.0,
            self.surface_extent.1,
        ))?;
        let sx = self.dest_extent.0 as f64 / self.surface_extent.0 as f64;
        let sy = self.dest_extent.1 as f64 / self.surface_extent.1 as f64;
        let ox = self.origin.x as f64;
        let oy = self.origin.y as f64;
        let x0 = (ox + clipped.origin.x as f64 * sx).floor();
        let y0 = (oy + clipped.origin.y as f64 * sy).floor();
        let x1 = (ox + (clipped.origin.x as i64 + clipped.size.w as i64) as f64 * sx).ceil();
        let y1 = (oy + (clipped.origin.y as i64 + clipped.size.h as i64) as f64 * sy).ceil();
        let clamp_i32 = |value: f64| value.clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        let (x0, y0, x1, y1) = (clamp_i32(x0), clamp_i32(y0), clamp_i32(x1), clamp_i32(y1));
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Changed region(s) in compositor logical coordinates. The caller must
    /// supply the explicit `full` verdict rather than inferring it from an
    /// empty list: empty + not full means no visible change.
    pub fn place_damage(&self, damage: &[Rect], full: bool) -> Vec<Rect> {
        if full {
            return vec![self.rect()];
        }
        damage.iter().filter_map(|rect| self.place(*rect)).collect()
    }

    /// Visual bounds of this placement, inflated by the soft-shadow skirt the
    /// compositor draws around a window. Used whenever a surface's placement
    /// moves or disappears: the pixels it vacated must repaint too.
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.origin.x.saturating_sub(SURFACE_SHADOW_MARGIN),
            self.origin.y.saturating_sub(SURFACE_SHADOW_MARGIN),
            self.dest_extent.0.saturating_add(SURFACE_SHADOW_MARGIN * 2),
            self.dest_extent.1.saturating_add(SURFACE_SHADOW_MARGIN * 2),
        )
    }
}

struct SurfaceDamageSample<'a> {
    id: usize,
    generation: u64,
    damage: &'a [Rect],
    damage_full: bool,
    placement: SurfacePlacement,
}

fn union_bbox(bbox: &mut Option<Rect>, rect: Rect) {
    *bbox = Some(match *bbox {
        Some(b) => {
            let x0 = b.origin.x.min(rect.origin.x);
            let y0 = b.origin.y.min(rect.origin.y);
            let x1 = b
                .origin
                .x
                .saturating_add(b.size.w)
                .max(rect.origin.x.saturating_add(rect.size.w));
            let y1 = b
                .origin
                .y
                .saturating_add(b.size.h)
                .max(rect.origin.y.saturating_add(rect.size.h));
            Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
        }
        None => rect,
    });
}

/// Map a logical bounding box onto the physical framebuffer, rounding
/// outward and clamping to the physical extent. `None` means the mapping is
/// not trustworthy (bad scale/extent) and the caller must fall back to full
/// damage.
pub fn logical_to_physical(rect: Rect, scale: f32, physical: (u32, u32)) -> Option<Rect> {
    if !scale.is_finite() || scale <= 0.0 || physical.0 == 0 || physical.1 == 0 {
        return None;
    }
    let (pw, ph) = (physical.0 as f32, physical.1 as f32);
    let x0 = (rect.origin.x as f32 * scale).floor().clamp(0.0, pw);
    let y0 = (rect.origin.y as f32 * scale).floor().clamp(0.0, ph);
    let x1 = (rect.origin.x.saturating_add(rect.size.w) as f32 * scale)
        .ceil()
        .clamp(0.0, pw);
    let y1 = (rect.origin.y.saturating_add(rect.size.h) as f32 * scale)
        .ceil()
        .clamp(0.0, ph);
    (x1 > x0 && y1 > y0).then_some(Rect::new(
        x0 as i32,
        y0 as i32,
        (x1 - x0) as i32,
        (y1 - y0) as i32,
    ))
}

/// Map logical damage rectangles into a [`FrameDamage`]. Rectangles outside
/// the physical framebuffer contribute no pixels and are discarded without
/// degrading the whole frame to Full. Only an untrustworthy coordinate system
/// (non-finite or non-positive scale, or zero-sized physical target) degrades to
/// `FrameDamage::Full`.
pub fn logical_rects_to_frame(
    logical: Vec<Rect>,
    scale: f32,
    physical_size: (u32, u32),
) -> FrameDamage {
    if !scale.is_finite() || scale <= 0.0 || physical_size.0 == 0 || physical_size.1 == 0 {
        return FrameDamage::Full;
    }
    let mut physical = Vec::with_capacity(logical.len());
    for rect in logical {
        if let Some(mapped) = logical_to_physical(rect, scale, physical_size) {
            physical.push(mapped);
        }
    }
    if physical.is_empty() {
        FrameDamage::None
    } else {
        FrameDamage::from_rects(physical)
    }
}

/// Map one offscreen stream's own-tree logical damage onto its physical
/// target (ADR-0187). Each logical rect is shifted by `-origin` (the stream's
/// logical origin in the desktop layout) and then mapped to target pixels by
/// [`logical_rects_to_frame`]. This is the offscreen analogue of
/// [`ClientDamage::into_frame_damage`], which assumes logical `(0, 0)` maps to
/// the target origin — false for a window or virtual output that sits at a
/// non-zero desktop position.
pub fn offscreen_damage_to_target(
    damage: ClientDamage,
    origin: tessera_primitives::Point,
    scale: f32,
    physical_size: (u32, u32),
) -> FrameDamage {
    match damage {
        ClientDamage::None => FrameDamage::None,
        ClientDamage::Area(logical) => {
            let shifted: Vec<Rect> = logical
                .into_iter()
                .map(|rect| {
                    Rect::new(
                        rect.origin.x.saturating_sub(origin.x),
                        rect.origin.y.saturating_sub(origin.y),
                        rect.size.w,
                        rect.size.h,
                    )
                })
                .collect();
            logical_rects_to_frame(shifted, scale, physical_size)
        }
    }
}

/// The damage scope of one output pass, clamped to the framebuffer it will be
/// drawn into.
///
/// This is the whole-set counterpart of the old single-rectangle helper, and it
/// preserves the list for the same reason the producer emits one: the bounding
/// box of a top HUD band and a bottom dock is the entire output, so collapsing a
/// set to a box before the pass sees it destroys the localisation the damage
/// pipeline exists to carry [ADR-0203], [ADR-0149].
///
/// Vulkan's dynamic `renderArea` must be fully inside the attachment: both flux
/// backends reject a scope naming no pixel of the target, and canvas.c degrades a
/// set above `flux::FLUX_DAMAGE_CAPACITY` to a full-extent pass. A damage rect
/// reaching the output pass is usually clamped already, but three sources can
/// still hand out a rect that is not:
///
/// * a fully off-screen surface maps to a rect outside the framebuffer,
/// * an `i32`-domain damage rect widened by [`SurfacePlacement`] saturation can
///   exceed the framebuffer extent,
/// * a window straddling the right/bottom edge while its ring-slot history
///   still holds the pre-clamp rect.
///
/// Handing the backend a rejected scope is not a harmless no-op: the pass fails
/// to begin and the frame loses its chrome (the `shell chrome render failed:
/// Render(FLUX_ERROR_INVALID_ARGUMENT)` symptom). Clamping each rectangle here,
/// and answering *empty* — which flux reads as a full-destination pass — when
/// nothing usable survives, keeps the failure in the damage domain where it
/// belongs.
pub fn frame_damage_scope(
    repaint: &FrameDamage,
    physical_size: (u32, u32),
) -> Vec<flux::CanvasRenderArea> {
    let Some(rects) = repaint.area_rects() else {
        /* `Full` (and `None`) both mean "the whole destination": an empty scope
         * is exactly that, and flux already interprets it that way. */
        return Vec::new();
    };
    let mut scope: Vec<flux::CanvasRenderArea> = Vec::with_capacity(rects.len());
    for rect in rects {
        if let Some(area) = clamp_render_area(*rect, physical_size.0, physical_size.1) {
            scope.push(area);
        }
    }
    /* Every named region fell outside the framebuffer. The pass still has to
     * cover something, so fall back to the full destination rather than an empty
     * scope that would be indistinguishable from `Full` arriving by accident. */
    if scope.is_empty() && !rects.is_empty() {
        scope.push(flux::CanvasRenderArea {
            x: 0.0,
            y: 0.0,
            width: physical_size.0 as f32,
            height: physical_size.1 as f32,
        });
    }
    scope
}

/// Intersect one physical rect with the framebuffer and express it as a
/// `CanvasRenderArea`. `None` means "no usable area" — either the rect reaches
/// no framebuffer pixel, or it degenerates in one dimension (which Vulkan would
/// reject, and which visually means "nothing to draw").
fn clamp_render_area(rect: Rect, max_w: u32, max_h: u32) -> Option<flux::CanvasRenderArea> {
    if rect.is_empty() {
        return None;
    }
    let x0 = rect.origin.x.max(0).min(max_w as i32) as i64;
    let y0 = rect.origin.y.max(0).min(max_h as i32) as i64;
    let x1 = (rect.origin.x as i64 + rect.size.w as i64).min(max_w as i64);
    let y1 = (rect.origin.y as i64 + rect.size.h as i64).min(max_h as i64);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(flux::CanvasRenderArea {
        x: x0 as f32,
        y: y0 as f32,
        width: (x1 - x0) as f32,
        height: (y1 - y0) as f32,
    })
}

const SURFACE_SHADOW_MARGIN: i32 = 48;

/// Record one visible surface into the placement+generation diff, pushing the
/// output-logical rectangles it dirties into `rects`.
///
/// There is deliberately no "this surface forces a full repaint" return value.
/// Every surface-local rectangle is expressible on the output through
/// [`SurfacePlacement`], and a surface whose placement collapses contributes no
/// damage instead. A `bool` here would be a standing invitation to reintroduce
/// the full-repaint verdict [ADR-0203] deleted.
///
/// [ADR-0203]: ../../../../docs/adr/0203-damage-is-a-placement-mapped-rectangle.md
fn accumulate_surface(
    old: &std::collections::HashMap<usize, SurfaceDamageBaseline>,
    new: &mut std::collections::HashMap<usize, SurfaceDamageBaseline>,
    sample: SurfaceDamageSample<'_>,
    rects: &mut Vec<Rect>,
) {
    let SurfaceDamageSample {
        id,
        generation,
        damage,
        damage_full,
        placement,
    } = sample;
    let previous = old.get(&id).copied();
    new.insert(
        id,
        SurfaceDamageBaseline {
            generation,
            placement,
        },
    );
    let Some(previous) = previous else {
        // A newly appeared surface: dirty its own visual bounds.
        rects.push(placement.bounds());
        return;
    };

    if previous.placement != placement {
        // The surface moved, resized, re-viewport'd, or changed transition
        // extent. Repaint both the rect it vacated and the one it now covers.
        // Each is the surface's *visual footprint*, not its buffer rectangle:
        // `bounds()` includes the compositor-drawn shadow skirt. Using
        // `placement.rect()` for the new position left the leading edge of a
        // moving floating window's shadow outside the damage scope.
        rects.push(previous.placement.bounds());
        rects.push(placement.bounds());
        return;
    }

    if previous.generation != generation {
        rects.extend(placement.place_damage(damage, damage_full));
    }
}

/// Double-buffered per-surface generation baseline for the client-damage
/// pass. Owns the baseline maps so the per-frame diff never freshly
/// heap-allocates: the maps swap in place and the now-old one is cleared and
/// refilled.
///
/// The composition root feeds every presentation-visible surface frame per
/// frame — SHM and dma-buf toplevels, overlays (including the client cursor
/// surface), and lock surfaces — through [`Self::assess`].
#[derive(Default)]
pub struct ClientDamageTracker {
    last: std::collections::HashMap<usize, SurfaceDamageBaseline>,
    scratch: std::collections::HashMap<usize, SurfaceDamageBaseline>,
}

impl ClientDamageTracker {
    /// Damage from client surface commits since the previous assessment,
    /// tracked by the per-surface content generations the renderer already
    /// uses for texture-upload gating.
    pub fn assess<'f>(
        &mut self,
        frames: impl IntoIterator<Item = &'f dyn SurfaceDamageFrame>,
    ) -> ClientDamage {
        // Swap in place instead of allocating a fresh HashMap every frame.
        std::mem::swap(&mut self.last, &mut self.scratch);
        let old = &self.scratch;
        let new = &mut self.last;
        new.clear();
        new.reserve(old.len());
        let mut rects: Vec<Rect> = Vec::new();
        let mut surfaces = 0u64;

        for frame in frames {
            surfaces += 1;
            let sample = SurfaceDamageSample {
                id: frame.id(),
                generation: frame.generation(),
                damage: frame.damage(),
                damage_full: frame.damage_full(),
                placement: frame.placement(),
            };
            if tracing::enabled!(target: "tessera.damage.assess", tracing::Level::TRACE) {
                tracing::trace!(
                    target: "tessera.damage.assess",
                    metric = "damage.surface",
                    count = 1u64,
                    id = sample.id,
                    generation = sample.generation,
                    damage_full = sample.damage_full,
                    damage_rects = sample.damage.len(),
                    placement = ?sample.placement,
                    "surface damage sample"
                );
            }
            accumulate_surface(old, new, sample, &mut rects);
        }
        for (id, prev) in old {
            if !new.contains_key(id) {
                rects.push(prev.placement.bounds());
            }
        }
        self.scratch.clear();

        let damage = if rects.is_empty() {
            ClientDamage::None
        } else {
            ClientDamage::Area(rects)
        };
        if tracing::enabled!(target: "tessera.damage.assess", tracing::Level::TRACE) {
            let (rect_count, area) = match &damage {
                ClientDamage::None => (0usize, 0i64),
                ClientDamage::Area(rects) => (
                    rects.len(),
                    rects
                        .iter()
                        .map(|r| (r.size.w.max(0) as i64) * (r.size.h.max(0) as i64))
                        .sum(),
                ),
            };
            tracing::trace!(
                target: "tessera.damage.assess",
                metric = "damage.assess",
                count = 1u64,
                surfaces,
                rects = rect_count,
                area,
                "client damage assessment"
            );
        }
        damage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tessera_primitives::Transform;

    fn placement_transform_strategy() -> impl Strategy<Value = Transform> {
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

    fn placement_rect_within(width: i32, height: i32) -> impl Strategy<Value = Rect> {
        let width = width.max(1);
        let height = height.max(1);
        (0..width, 0..height).prop_flat_map(move |(x, y)| {
            (Just(x), Just(y), 1..=(width - x), 1..=(height - y))
                .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
        })
    }

    fn placement_geometry() -> impl Strategy<Value = (SurfaceGeometry, i32, i32)> {
        (
            1i32..=64,
            1i32..=64,
            placement_transform_strategy(),
            1i32..=4,
        )
            .prop_flat_map(|(local_width, local_height, transform, buffer_scale)| {
                let (buffer_width, buffer_height) = if transform.swap_axes() {
                    (local_height * buffer_scale, local_width * buffer_scale)
                } else {
                    (local_width * buffer_scale, local_height * buffer_scale)
                };
                let viewport_src =
                    prop::option::of(placement_rect_within(local_width, local_height));
                let viewport_dst = prop::option::of(
                    (1i32..=256, 1i32..=256).prop_map(|(w, h)| tessera_primitives::Size { w, h }),
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
            })
    }

    fn placement_case() -> impl Strategy<Value = (SurfaceGeometry, i32, i32, Rect)> {
        placement_geometry().prop_flat_map(|(geometry, buffer_width, buffer_height)| {
            let (width, height) = geometry.surface_extent(buffer_width, buffer_height);
            placement_rect_within(width, height)
                .prop_map(move |rect| (geometry, buffer_width, buffer_height, rect))
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        #[test]
        fn placement_maps_full_surface_damage_to_exactly_the_drawn_rect(
            (geometry, buffer_width, buffer_height) in placement_geometry()
        ) {
            let placement = SurfacePlacement::resolve(&geometry, buffer_width, buffer_height);
            let (surface_width, surface_height) =
                geometry.surface_extent(buffer_width, buffer_height);
            prop_assert_eq!(placement.place_damage(&[], true), vec![placement.rect()]);
            prop_assert_eq!(
                placement.place(Rect::new(0, 0, surface_width, surface_height)),
                Some(placement.rect()),
                "geometry {:?}, buffer {}x{}",
                geometry, buffer_width, buffer_height
            );
        }

        #[test]
        fn placement_lands_every_surface_damage_inside_the_drawn_rect(
            (geometry, buffer_width, buffer_height, damage) in placement_case()
        ) {
            let placement = SurfacePlacement::resolve(&geometry, buffer_width, buffer_height);
            let landed = placement
                .place(damage)
                .expect("surface damage within the extent must land on the output");
            let drawn = placement.rect();
            prop_assert!(
                landed.fully_covered_by(&[drawn]),
                "damage {:?} landed at {:?}, outside the drawn rect {:?} (geometry {:?})",
                damage, landed, drawn, geometry
            );
        }
    }

    /// A placement for a surface whose buffer maps 1:1 onto logical pixels.
    fn placement_at(origin: (i32, i32), width: i32, height: i32) -> SurfacePlacement {
        SurfacePlacement {
            origin: tessera_primitives::Point {
                x: origin.0,
                y: origin.1,
            },
            source: Rect::new(0, 0, width, height),
            surface_extent: (width, height),
            dest_extent: (width, height),
        }
    }

    #[test]
    fn test_inv_arch_94_placement_maps_and_clamps_damage_to_the_surface() {
        let placement = placement_at((100, 50), 800, 600);
        // Tight damage translates by the compositor position.
        assert_eq!(
            placement.place_damage(&[Rect::new(10, 20, 30, 40)], false),
            vec![Rect::new(110, 70, 30, 40)]
        );
        // Out-of-bounds damage clamps to the surface extent.
        assert_eq!(
            placement.place_damage(&[Rect::new(-20, 590, 900, 50)], false),
            vec![Rect::new(100, 640, 800, 10)]
        );
        // Partial negative damage clips by intersection rather than moving the
        // origin to zero while preserving the original width.
        assert_eq!(
            placement_at((10, 20), 800, 600).place_damage(&[Rect::new(-20, 10, 30, 20)], false),
            vec![Rect::new(10, 30, 10, 20)]
        );
        // A report wholly outside the surface contributes no invented damage.
        assert!(
            placement
                .place_damage(&[Rect::new(-50, 10, 20, 20)], false)
                .is_empty()
        );
        // No damage information damages the whole surface.
        assert_eq!(
            placement.place_damage(&[], true),
            vec![Rect::new(100, 50, 800, 600)]
        );
        // Fully degenerate damage reports nothing.
        assert!(
            placement
                .place_damage(&[Rect::new(10, 10, 0, 0)], false)
                .is_empty()
        );
        // Disjoint damage rects are preserved individually, not unioned.
        assert_eq!(
            placement_at((0, 0), 800, 600)
                .place_damage(&[Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)], false),
            vec![Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)]
        );
    }

    #[test]
    fn test_inv_arch_93_viewport_destination_is_the_damage_space_not_a_second_scale() {
        // A HiDPI client committing a 2048x1536 buffer at scale 2 with a
        // 512x384 wp_viewport destination: wl_surface.damage is already in the
        // 512x384 surface space. Mapping it through dest/source again would
        // repaint only a quarter of the changed pixels.
        let placement = SurfacePlacement::resolve(
            &SurfaceGeometry {
                position: tessera_primitives::Point { x: 100, y: 50 },
                buffer_scale: 2,
                viewport_dst: Some(tessera_primitives::Size { w: 512, h: 384 }),
                ..Default::default()
            },
            2048,
            1536,
        );
        assert_eq!(placement.source, Rect::new(0, 0, 1024, 768));
        assert_eq!(placement.surface_extent, (512, 384));
        assert_eq!(placement.dest_extent, (512, 384));
        assert_eq!(placement.rect(), Rect::new(100, 50, 512, 384));
        // A full-surface damage covers the whole destination.
        assert_eq!(
            placement.place_damage(&[], true),
            vec![Rect::new(100, 50, 512, 384)]
        );
        // Half the surface maps to half the destination, not a quarter.
        assert_eq!(
            placement.place_damage(&[Rect::new(0, 0, 256, 384)], false),
            vec![Rect::new(100, 50, 256, 384)]
        );
    }

    /// `wp_viewport.set_source` changes which buffer pixels are sampled, not
    /// where the surface is drawn. Damage is in surface coordinates, so it maps
    /// to output coordinates directly; the crop origin belongs to the
    /// surface→buffer conversion, not to `place`.
    #[test]
    fn test_inv_arch_92_viewport_source_crop_does_not_shift_output_damage() {
        let placement = SurfacePlacement::resolve(
            &SurfaceGeometry {
                position: tessera_primitives::Point { x: 0, y: 0 },
                viewport_src: Some(Rect::new(100, 100, 400, 300)),
                ..Default::default()
            },
            800,
            600,
        );
        assert_eq!(placement.source, Rect::new(100, 100, 400, 300));
        assert_eq!(placement.surface_extent, (400, 300));
        assert_eq!(placement.dest_extent, (400, 300));
        assert_eq!(placement.rect(), Rect::new(0, 0, 400, 300));

        // Damage is already in the 400x300 surface space.
        assert_eq!(
            placement.place(Rect::new(0, 0, 200, 150)),
            Some(Rect::new(0, 0, 200, 150))
        );
        assert_eq!(
            placement.place(Rect::new(200, 150, 200, 150)),
            Some(Rect::new(200, 150, 200, 150))
        );
        // Damage outside the surface reaches no output pixel.
        assert_eq!(placement.place(Rect::new(400, 0, 10, 10)), None);
        assert_eq!(placement.place(Rect::new(-200, 0, 100, 100)), None);
        // A report straddling the boundary clips to the visible part.
        assert_eq!(
            placement.place(Rect::new(-50, 0, 100, 100)),
            Some(Rect::new(0, 0, 50, 100))
        );
        // The explicit full verdict covers the whole surface content.
        assert_eq!(
            placement.place_damage(&[], true),
            vec![Rect::new(0, 0, 400, 300)]
        );
    }

    /// A source crop combined with a destination that scales it: the surface is
    /// already 200x150, so damage maps 1:1 into that drawn rect.
    #[test]
    fn test_inv_arch_92_viewport_source_and_destination_compose() {
        let placement = SurfacePlacement::resolve(
            &SurfaceGeometry {
                position: tessera_primitives::Point { x: 10, y: 20 },
                viewport_src: Some(Rect::new(200, 100, 400, 300)),
                viewport_dst: Some(tessera_primitives::Size { w: 200, h: 150 }),
                ..Default::default()
            },
            800,
            600,
        );
        assert_eq!(placement.surface_extent, (200, 150));
        assert_eq!(
            placement.place(Rect::new(0, 0, 200, 150)),
            Some(Rect::new(10, 20, 200, 150))
        );
        assert_eq!(
            placement.place(Rect::new(0, 0, 100, 75)),
            Some(Rect::new(10, 20, 100, 75))
        );
    }

    #[test]
    fn test_inv_arch_93_transition_extent_overrides_the_viewport_destination() {
        // ADR-0029: an in-flight transition draws the texture at the
        // interpolated size, so damage must follow that rect, not the
        // viewport destination the client declared.
        let placement = SurfacePlacement::resolve(
            &SurfaceGeometry {
                position: tessera_primitives::Point { x: 10, y: 10 },
                viewport_dst: Some(tessera_primitives::Size { w: 400, h: 300 }),
                transition_size: Some(tessera_primitives::Size { w: 200, h: 150 }),
                ..Default::default()
            },
            400,
            300,
        );
        assert_eq!(placement.dest_extent, (200, 150));
        assert_eq!(
            placement.place_damage(&[Rect::new(0, 0, 400, 300)], false),
            vec![Rect::new(10, 10, 200, 150)]
        );
    }

    #[test]
    fn union_bbox_grows_to_cover() {
        let mut bbox = None;
        union_bbox(&mut bbox, Rect::new(10, 10, 20, 20));
        union_bbox(&mut bbox, Rect::new(0, 40, 15, 10));
        assert_eq!(bbox, Some(Rect::new(0, 10, 30, 40)));
    }

    #[test]
    fn frame_damage_union_preserves_full_and_joins_areas() {
        let a = FrameDamage::Area(vec![Rect::new(10, 20, 30, 40)]);
        let b = FrameDamage::Area(vec![Rect::new(0, 50, 20, 20)]);
        // Lists concatenate; the union is recoverable via area_union.
        let joined = union_frame_damage(a, b);
        assert_eq!(joined.area_union(), Some(Rect::new(0, 20, 40, 50)));
        assert_eq!(
            union_frame_damage(FrameDamage::None, joined.clone()),
            joined
        );
        assert_eq!(
            union_frame_damage(FrameDamage::Full, joined),
            FrameDamage::Full
        );
    }

    #[test]
    fn damage_normalization_overflow_covers_every_input_rect() {
        // Force the disjoint decomposition past its work budget. The fallback
        // must be the bounding box of the whole input, not of the prefix that
        // happened to fit before the budget was exhausted.
        let rects: Vec<Rect> = (0..512)
            .map(|index| {
                let offset = index * 3;
                Rect::new(offset, offset / 2, 600, 400)
            })
            .collect();
        let expected = {
            let mut bbox = None;
            for rect in &rects {
                union_bbox(&mut bbox, *rect);
            }
            bbox.expect("non-empty input")
        };
        let normalized = FrameDamage::from_rects(rects);
        assert_eq!(normalized.area_union(), Some(expected));
        assert_eq!(
            normalized.area_rects().map(<[Rect]>::len),
            Some(1),
            "overflow must degrade to one full-coverage bounding box"
        );
    }

    #[test]
    fn frame_damage_history_is_deduplicated_and_bounded() {
        let repeated = vec![Rect::new(1, 2, 3, 4); 100];
        assert_eq!(
            FrameDamage::from_rects(repeated),
            FrameDamage::Area(vec![Rect::new(1, 2, 3, 4)])
        );

        let disjoint: Vec<_> = (0..=MAX_FRAME_DAMAGE_RECTS)
            .map(|index| Rect::new(index as i32 * 2, 0, 1, 1))
            .collect();
        assert_eq!(
            FrameDamage::from_rects(disjoint),
            FrameDamage::Area(vec![Rect::new(
                0,
                0,
                (MAX_FRAME_DAMAGE_RECTS as i32) * 2 + 1,
                1,
            )])
        );
    }

    #[test]
    fn logical_to_physical_rounds_outward_and_clamps() {
        // scale 2: outward rounding keeps every touched physical pixel.
        let mapped = logical_to_physical(Rect::new(5, 5, 11, 11), 2.0, (1920, 1080));
        assert_eq!(mapped, Some(Rect::new(10, 10, 22, 22)));
        // Extending past the framebuffer clamps to the physical extent.
        let clamped = logical_to_physical(Rect::new(900, 500, 100, 100), 2.0, (1920, 1080));
        assert_eq!(clamped, Some(Rect::new(1800, 1000, 120, 80)));
        // Fully off-screen and unusable scales report no mapping.
        assert_eq!(
            logical_to_physical(Rect::new(-500, 0, 100, 100), 1.0, (1920, 1080)),
            None
        );
        assert_eq!(
            logical_to_physical(Rect::new(0, 0, 10, 10), 0.0, (1920, 1080)),
            None
        );
    }

    #[test]
    fn frame_damage_from_rects_decomposes_overlaps_into_pairwise_disjoint_rects() {
        let a = Rect::new(10, 10, 20, 20);
        let b = Rect::new(20, 20, 20, 20);
        let damage = FrameDamage::from_rects(vec![a, b]);
        let rects = damage.area_rects().expect("must remain Area");
        assert_eq!(rects.len(), 3);
        let mut total_area = 0;
        for (i, r1) in rects.iter().enumerate() {
            total_area += r1.size.w * r1.size.h;
            for r2 in &rects[i + 1..] {
                assert!(
                    r1.intersect(*r2).is_none(),
                    "overlapping rects in normalized damage: {r1:?} vs {r2:?}"
                );
            }
        }
        assert_eq!(total_area, 400 + 400 - 100);
    }

    #[test]
    fn client_damage_maps_degrading_to_full() {
        // Disjoint logical rects stay disjoint through the mapping.
        let damage = ClientDamage::Area(vec![Rect::new(0, 0, 5, 5), Rect::new(10, 10, 5, 5)]);
        assert_eq!(
            damage.clone().into_frame_damage(1.0, (100, 100)),
            FrameDamage::Area(vec![Rect::new(0, 0, 5, 5), Rect::new(10, 10, 5, 5)])
        );
        // Offscreen rectangles contribute zero physical damage without degrading to Full (INV-ARCH-99).
        let with_offscreen =
            ClientDamage::Area(vec![Rect::new(0, 0, 5, 5), Rect::new(-50, -50, 20, 20)]);
        assert_eq!(
            with_offscreen.into_frame_damage(1.0, (100, 100)),
            FrameDamage::Area(vec![Rect::new(0, 0, 5, 5)])
        );
        let all_offscreen = ClientDamage::Area(vec![Rect::new(-50, -50, 20, 20)]);
        assert_eq!(
            all_offscreen.into_frame_damage(1.0, (100, 100)),
            FrameDamage::None
        );
        // An invalid scale degrades the whole frame to Full.
        assert_eq!(damage.into_frame_damage(0.0, (100, 100)), FrameDamage::Full);
        assert_eq!(
            ClientDamage::None.into_frame_damage(1.0, (100, 100)),
            FrameDamage::None
        );
    }

    #[test]
    fn flux_damage_round_trip_preserves_the_set_and_rounds_outward() {
        let damage = FrameDamage::Area(vec![Rect::new(10, 20, 30, 40), Rect::new(100, 120, 5, 7)]);
        let round_trip = FrameDamage::from_flux_damage(damage.to_flux_damage());
        assert_eq!(round_trip, damage);
        assert_eq!(
            FrameDamage::from_flux_damage(flux::Damage::None),
            FrameDamage::None
        );
        assert_eq!(
            FrameDamage::from_flux_damage(flux::Damage::Area(Vec::new())),
            FrameDamage::None
        );
        assert_eq!(
            FrameDamage::from_flux_damage(flux::Damage::Area(vec![flux::CanvasRenderArea {
                x: 10.4,
                y: 20.6,
                width: 3.2,
                height: 4.1,
            }])),
            FrameDamage::Area(vec![Rect::new(10, 20, 4, 5)])
        );
    }

    #[test]
    fn explicit_full_verdict_is_not_inferred_from_an_empty_list() {
        struct Sample {
            generation: u64,
            damage_full: bool,
            geometry: SurfaceGeometry,
        }
        impl SurfaceDamageFrame for Sample {
            fn id(&self) -> usize {
                11
            }
            fn generation(&self) -> u64 {
                self.generation
            }
            fn damage(&self) -> &[Rect] {
                &[]
            }
            fn damage_full(&self) -> bool {
                self.damage_full
            }
            fn placement(&self) -> SurfacePlacement {
                SurfacePlacement::resolve(&self.geometry, 100, 80)
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            ..Default::default()
        };
        let mut tracker = ClientDamageTracker::default();
        // First sighting dirties the surface footprint.
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 1,
                damage_full: false,
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::Area(_)
        ));
        // A generation change with no damage and full=false is a visible no-op.
        assert_eq!(
            tracker.assess([&Sample {
                generation: 2,
                damage_full: false,
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::None
        );
        // The explicit full verdict is the only thing that dirties the whole
        // surface when the rectangle list is empty.
        assert_eq!(
            tracker.assess([&Sample {
                generation: 3,
                damage_full: true,
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::Area(vec![Rect::new(10, 20, 100, 80)])
        );
    }

    #[test]
    fn tracker_maps_geometry_change_as_localized_area_damage() {
        struct Sample {
            generation: u64,
            damage: Vec<Rect>,
            geometry: SurfaceGeometry,
        }
        impl SurfaceDamageFrame for Sample {
            fn id(&self) -> usize {
                7
            }
            fn generation(&self) -> u64 {
                self.generation
            }
            fn damage(&self) -> &[Rect] {
                &self.damage
            }
            fn damage_full(&self) -> bool {
                false
            }
            fn placement(&self) -> SurfacePlacement {
                SurfacePlacement::resolve(&self.geometry, 100, 80)
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            ..Default::default()
        };
        let moved = SurfaceGeometry {
            position: tessera_primitives::Point { x: 11, y: 20 },
            ..geometry
        };

        let mut tracker = ClientDamageTracker::default();
        let initial = tracker.assess([&Sample {
            generation: 3,
            damage: Vec::new(),
            geometry,
        } as &dyn SurfaceDamageFrame]);
        assert!(
            matches!(initial, ClientDamage::Area(_)),
            "first sighting emits surface area"
        );
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 3,
                damage: Vec::new(),
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::None
        ));
        let moved_damage = tracker.assess([&Sample {
            generation: 3,
            damage: Vec::new(),
            geometry: moved,
        } as &dyn SurfaceDamageFrame]);
        assert!(matches!(moved_damage, ClientDamage::Area(_)));
    }

    /// A placement change must report the *visual footprint* on both sides, not
    /// the bare buffer rectangle. The compositor-drawn shadow reaches past the
    /// window; using `placement.rect()` for the destination side clipped the
    /// leading shadow edge of a moving floating window.
    #[test]
    fn a_moved_surface_repaints_the_shadow_skirt_on_both_sides() {
        struct Sample {
            geometry: SurfaceGeometry,
        }
        impl SurfaceDamageFrame for Sample {
            fn id(&self) -> usize {
                7
            }
            fn generation(&self) -> u64 {
                1
            }
            fn damage(&self) -> &[Rect] {
                &[]
            }
            fn damage_full(&self) -> bool {
                false
            }
            fn placement(&self) -> SurfacePlacement {
                SurfacePlacement::resolve(&self.geometry, 100, 80)
            }
        }

        let before = SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            ..Default::default()
        };
        let after = SurfaceGeometry {
            position: tessera_primitives::Point { x: 40, y: 60 },
            ..before
        };
        let old_footprint = SurfacePlacement::resolve(&before, 100, 80).bounds();
        let new_footprint = SurfacePlacement::resolve(&after, 100, 80).bounds();

        let mut tracker = ClientDamageTracker::default();
        let _ = tracker.assess([&Sample { geometry: before } as &dyn SurfaceDamageFrame]);
        let damage = tracker.assess([&Sample { geometry: after } as &dyn SurfaceDamageFrame]);
        let ClientDamage::Area(rects) = damage else {
            panic!("a move must produce localized damage");
        };
        assert!(rects.contains(&old_footprint), "vacated shadow skirt");
        assert!(rects.contains(&new_footprint), "new shadow skirt");
    }

    #[test]
    fn tracker_maps_new_content_damage_for_unchanged_geometry() {
        struct Sample {
            generation: u64,
            damage: Vec<Rect>,
            geometry: SurfaceGeometry,
        }
        impl SurfaceDamageFrame for Sample {
            fn id(&self) -> usize {
                7
            }
            fn generation(&self) -> u64 {
                self.generation
            }
            fn damage(&self) -> &[Rect] {
                &self.damage
            }
            fn damage_full(&self) -> bool {
                false
            }
            fn placement(&self) -> SurfacePlacement {
                SurfacePlacement::resolve(&self.geometry, 100, 80)
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            ..Default::default()
        };

        let mut tracker = ClientDamageTracker::default();
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 3,
                damage: Vec::new(),
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::Area(_)
        ));
        let unchanged = tracker.assess([&Sample {
            generation: 3,
            damage: Vec::new(),
            geometry,
        } as &dyn SurfaceDamageFrame]);
        assert_eq!(unchanged, ClientDamage::None);
        let content = tracker.assess([&Sample {
            generation: 4,
            damage: vec![Rect::new(2, 3, 4, 5)],
            geometry,
        } as &dyn SurfaceDamageFrame]);
        assert_eq!(content, ClientDamage::Area(vec![Rect::new(12, 23, 4, 5)]));
    }

    #[test]
    fn hidpi_surface_maps_logical_damage_without_forcing_full() {
        struct Sample {
            generation: u64,
            damage: Vec<Rect>,
            geometry: SurfaceGeometry,
        }
        impl SurfaceDamageFrame for Sample {
            fn id(&self) -> usize {
                7
            }
            fn generation(&self) -> u64 {
                self.generation
            }
            fn damage(&self) -> &[Rect] {
                &self.damage
            }
            fn damage_full(&self) -> bool {
                false
            }
            fn placement(&self) -> SurfacePlacement {
                SurfacePlacement::resolve(&self.geometry, 200, 160)
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            buffer_scale: 2,
            ..Default::default()
        };

        let mut tracker = ClientDamageTracker::default();
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 3,
                damage: Vec::new(),
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::Area(_)
        ));
        let content = tracker.assess([&Sample {
            generation: 4,
            damage: vec![Rect::new(2, 3, 4, 5)],
            geometry,
        } as &dyn SurfaceDamageFrame]);
        assert_eq!(content, ClientDamage::Area(vec![Rect::new(12, 23, 4, 5)]));
    }

    #[test]
    fn viewport_destination_damage_is_not_scaled_twice() {
        struct Sample {
            generation: u64,
            damage: Vec<Rect>,
            geometry: SurfaceGeometry,
        }
        impl SurfaceDamageFrame for Sample {
            fn id(&self) -> usize {
                9
            }
            fn generation(&self) -> u64 {
                self.generation
            }
            fn damage(&self) -> &[Rect] {
                &self.damage
            }
            fn damage_full(&self) -> bool {
                false
            }
            fn placement(&self) -> SurfacePlacement {
                SurfacePlacement::resolve(&self.geometry, 200, 160)
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_primitives::Point { x: 10, y: 20 },
            viewport_dst: Some(tessera_primitives::Size { w: 100, h: 80 }),
            ..Default::default()
        };
        let mut tracker = ClientDamageTracker::default();
        let _ = tracker.assess([&Sample {
            generation: 1,
            damage: Vec::new(),
            geometry,
        } as &dyn SurfaceDamageFrame]);
        let content = tracker.assess([&Sample {
            generation: 2,
            damage: vec![Rect::new(10, 10, 20, 20)],
            geometry,
        } as &dyn SurfaceDamageFrame]);
        // The surface is already 100x80; the raw buffer being 200x160 must not
        // shrink the damage into the top-left quadrant.
        assert_eq!(content, ClientDamage::Area(vec![Rect::new(20, 30, 20, 20)]));
    }

    #[test]
    fn offscreen_damage_translates_origin_then_maps_to_target() {
        // A surface at desktop-logical (2005, 45) inside a virtual output
        // whose origin is (2000, 0): the target-relative rect starts at
        // (5, 45) (`[INV-ARCH-79]`).
        let mapped = offscreen_damage_to_target(
            ClientDamage::Area(vec![Rect::new(2005, 45, 10, 10)]),
            tessera_primitives::Point { x: 2000, y: 0 },
            1.0,
            (64, 64),
        );
        assert_eq!(mapped.area_rects(), Some(&[Rect::new(5, 45, 10, 10)][..]));

        // A 2x target maps logical to physical pixels after the shift.
        let scaled = offscreen_damage_to_target(
            ClientDamage::Area(vec![Rect::new(2005, 45, 10, 10)]),
            tessera_primitives::Point { x: 2000, y: 0 },
            2.0,
            (128, 128),
        );
        assert_eq!(scaled.area_rects(), Some(&[Rect::new(10, 90, 20, 20)][..]));

        // Non-area verdicts pass through unchanged.
        assert!(matches!(
            offscreen_damage_to_target(
                ClientDamage::None,
                tessera_primitives::Point { x: 0, y: 0 },
                1.0,
                (64, 64)
            ),
            FrameDamage::None
        ));
    }
}
