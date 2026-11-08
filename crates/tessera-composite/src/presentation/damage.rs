use tessera_primitives::Rect;
use tessera_primitives::SurfaceDmabuf;
use tessera_primitives::SurfaceGeometry;
use tessera_primitives::SurfacePixels;

/// Hard bound for damage carried across swapchain-slot history. Wayland
/// surface commits are independently capped upstream, but combining many
/// surfaces and several missed frames could otherwise grow this Vec without
/// bound. Above the cap we conservatively fall back to one bounding box.
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

fn normalize_damage_rects(mut rects: Vec<Rect>) -> Vec<Rect> {
    rects.retain(|rect| !rect.is_empty());
    rects.sort_unstable_by_key(|rect| (rect.origin.y, rect.origin.x, rect.size.h, rect.size.w));
    rects.dedup();

    // Removing a rectangle contained by another is exact and cheaply keeps
    // common repeated/full-surface reports compact.
    let mut index = 0;
    while index < rects.len() {
        let contained = rects.iter().enumerate().any(|(other_index, other)| {
            other_index != index && rects[index].fully_covered_by(&[*other])
        });
        if contained {
            rects.remove(index);
        } else {
            index += 1;
        }
    }

    if rects.len() > MAX_FRAME_DAMAGE_RECTS {
        let mut bbox = None;
        for rect in rects {
            union_bbox(&mut bbox, rect);
        }
        bbox.into_iter().collect()
    } else {
        rects
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
    /// The pixels this frame repaints. Controls the swapchain repaint and the
    /// KMS damage hint alike.
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

/// Damage that must be repainted into `slot`, including every change that
/// happened while another ring image was scanned out.
pub fn composite_repaint_for_slot(
    slots: &mut Vec<FrameDamage>,
    slot: usize,
    current: FrameDamage,
) -> FrameDamage {
    if slots.len() <= slot {
        slots.resize(slot + 1, FrameDamage::Full);
    }
    // `slots[slot]` holds damage accumulated while this image was not scanned
    // out. Take it (leaving None) so the slot starts fresh for the next cycle;
    // `record_composite_present` re-seeds it for other slots.
    let pending = std::mem::replace(&mut slots[slot], FrameDamage::None);
    union_frame_damage(pending, current)
}

/// Advance ring-image damage history only after a successful presentation.
pub fn record_composite_present(slots: &mut Vec<FrameDamage>, slot: usize, current: FrameDamage) {
    if slots.len() <= slot {
        slots.resize(slot + 1, FrameDamage::Full);
    }
    for (index, pending) in slots.iter_mut().enumerate() {
        if index == slot {
            *pending = FrameDamage::None;
        } else {
            // Fold `current` into the other slots. Clone it for each: ring
            // depth is small (2–3 images) and this runs once per presented
            // frame, not per draw.
            *pending = union_frame_damage(
                std::mem::replace(pending, FrameDamage::None),
                current.clone(),
            );
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
    /// Surface-local logical damage rectangles since the last commit; empty
    /// means "no usable damage information; the whole surface changed".
    fn damage(&self) -> &[Rect];
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

/// Where one surface's buffer lands on the output, expressed as the exact
/// affine map every surface-local coordinate — `wl_surface.damage`,
/// `damage_buffer` after its commit-time conversion, an opaque region — takes
/// to reach output-logical pixels (ADR-0203).
///
/// `wp_viewport` and window transitions change this map; they never destroy
/// it. A damage rectangle therefore stays a rectangle. Only a rectangle that
/// reaches no visible content is unmappable, and it contributes no damage
/// rather than forcing a full repaint.
///
/// The map is `p -> origin + (p - source_origin) * dest_extent / source_extent`.
/// The subtraction matters: `wp_viewport.set_source` selects which part of the
/// buffer is visible *and* moves where the visible content begins, so a crop
/// at `(100, 100)` draws surface pixel `(100, 100)` at the surface's output
/// origin — not 200 px into it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SurfacePlacement {
    /// Output-logical origin the visible content is drawn at.
    pub origin: tessera_primitives::Point,
    /// Full post-transform buffer extent in logical pixels (`buffer_scale`
    /// applied). The coordinate space the client's damage rectangles live in.
    pub local: (i32, i32),
    /// Origin of the visible content within `local`: the
    /// `wp_viewport.set_source` origin, else `(0, 0)`.
    pub source_origin: (i32, i32),
    /// Extent of the visible content within `local`: the
    /// `wp_viewport.set_source` size, else the whole `local` extent.
    pub source_extent: (i32, i32),
    /// Extent the visible content is drawn at on the output: an in-flight
    /// transition's interpolated size, else the `wp_viewport.set_destination`
    /// size, else `source_extent`.
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
        let scale = geometry.buffer_scale.max(1) as f32;
        let (bw, bh) = if geometry.transform.swap_axes() {
            (height, width)
        } else {
            (width, height)
        };
        let local = (
            (bw as f32 / scale).round().max(1.0) as i32,
            (bh as f32 / scale).round().max(1.0) as i32,
        );
        // A source crop selects which part of `local` is visible and where the
        // visible content begins. Damage outside it reaches no output pixel.
        let (source_origin, source_extent) = match geometry.viewport_src {
            Some(src) => (
                (src.origin.x, src.origin.y),
                (src.size.w.max(1), src.size.h.max(1)),
            ),
            None => ((0, 0), local),
        };
        // `viewport_dst` wins (it is what the client asked to occupy), else the
        // visible content is drawn at its own size.
        let mut dest_extent = match geometry.viewport_dst {
            Some(dst) => (dst.w.max(1), dst.h.max(1)),
            None => source_extent,
        };
        // An in-flight transition (ADR-0029) draws the texture at the
        // interpolated rect and overrides the viewport-derived size.
        if let Some(size) = geometry.transition_size {
            dest_extent = (size.w.max(1), size.h.max(1));
        }
        Self {
            origin: geometry.position,
            local,
            source_origin,
            source_extent,
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

    /// The surface-local rectangle that is actually visible.
    fn visible_local(&self) -> Rect {
        Rect::new(
            self.source_origin.0,
            self.source_origin.1,
            self.source_extent.0,
            self.source_extent.1,
        )
    }

    /// Map one surface-local damage rectangle onto the output. Outward
    /// rounding keeps every touched output pixel inside the damage. `None`
    /// means the rectangle reaches no output pixel (entirely outside the
    /// visible content).
    pub fn place(&self, local_rect: Rect) -> Option<Rect> {
        if self.source_extent.0 <= 0 || self.source_extent.1 <= 0 {
            return None;
        }
        let clipped = local_rect.intersect(self.visible_local())?;
        let sx = self.dest_extent.0 as f64 / self.source_extent.0 as f64;
        let sy = self.dest_extent.1 as f64 / self.source_extent.1 as f64;
        let ox = self.origin.x as f64 - self.source_origin.0 as f64 * sx;
        let oy = self.origin.y as f64 - self.source_origin.1 as f64 * sy;
        let x0 = (ox + clipped.origin.x as f64 * sx).floor();
        let y0 = (oy + clipped.origin.y as f64 * sy).floor();
        let x1 = (ox + (clipped.origin.x as i64 + clipped.size.w as i64) as f64 * sx).ceil();
        let y1 = (oy + (clipped.origin.y as i64 + clipped.size.h as i64) as f64 * sy).ceil();
        let clamp_i32 = |value: f64| value.clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        let (x0, y0, x1, y1) = (clamp_i32(x0), clamp_i32(y0), clamp_i32(x1), clamp_i32(y1));
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Changed region(s) in compositor logical coordinates: the surface's
    /// damage rects mapped through this placement, or the whole visible content
    /// when the commit carried no usable damage information (`damage` empty).
    /// Each rect is preserved individually rather than unioned, so disjoint
    /// dirty regions stay disjoint for the KMS hint.
    pub fn place_damage(&self, damage: &[Rect]) -> Vec<Rect> {
        if damage.is_empty() {
            // No usable damage information: the whole visible surface changed.
            return self.place(self.visible_local()).into_iter().collect();
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
        rects.push(previous.placement.bounds());
        rects.push(placement.rect());
        return;
    }

    if previous.generation != generation {
        rects.extend(placement.place_damage(damage));
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

        for frame in frames {
            accumulate_surface(
                old,
                new,
                SurfaceDamageSample {
                    id: frame.id(),
                    generation: frame.generation(),
                    damage: frame.damage(),
                    placement: frame.placement(),
                },
                &mut rects,
            );
        }
        for (id, prev) in old {
            if !new.contains_key(id) {
                rects.push(prev.placement.bounds());
            }
        }
        self.scratch.clear();

        if rects.is_empty() {
            ClientDamage::None
        } else {
            ClientDamage::Area(rects)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A placement for a surface whose buffer maps 1:1 onto logical pixels.
    fn placement_at(origin: (i32, i32), width: i32, height: i32) -> SurfacePlacement {
        SurfacePlacement {
            origin: tessera_primitives::Point {
                x: origin.0,
                y: origin.1,
            },
            local: (width, height),
            source_origin: (0, 0),
            source_extent: (width, height),
            dest_extent: (width, height),
        }
    }

    #[test]
    fn test_inv_arch_94_placement_maps_and_clamps_damage_to_the_surface() {
        let placement = placement_at((100, 50), 800, 600);
        // Tight damage translates by the compositor position.
        assert_eq!(
            placement.place_damage(&[Rect::new(10, 20, 30, 40)]),
            vec![Rect::new(110, 70, 30, 40)]
        );
        // Out-of-bounds damage clamps to the surface extent.
        assert_eq!(
            placement.place_damage(&[Rect::new(-20, 590, 900, 50)]),
            vec![Rect::new(100, 640, 800, 10)]
        );
        // Partial negative damage clips by intersection rather than moving the
        // origin to zero while preserving the original width.
        assert_eq!(
            placement_at((10, 20), 800, 600).place_damage(&[Rect::new(-20, 10, 30, 20)]),
            vec![Rect::new(10, 30, 10, 20)]
        );
        // A report wholly outside the surface contributes no invented damage.
        assert!(
            placement
                .place_damage(&[Rect::new(-50, 10, 20, 20)])
                .is_empty()
        );
        // No damage information damages the whole surface.
        assert_eq!(
            placement.place_damage(&[]),
            vec![Rect::new(100, 50, 800, 600)]
        );
        // Fully degenerate damage reports nothing.
        assert!(
            placement
                .place_damage(&[Rect::new(10, 10, 0, 0)])
                .is_empty()
        );
        // Disjoint damage rects are preserved individually, not unioned.
        assert_eq!(
            placement_at((0, 0), 800, 600)
                .place_damage(&[Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)]),
            vec![Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)]
        );
    }

    #[test]
    fn test_inv_arch_93_viewport_destination_scales_damage_instead_of_forcing_full() {
        // A HiDPI client committing a 2048x1536 buffer at scale 2 with a
        // 512x384 wp_viewport destination: the damage rect must land scaled
        // into the viewport rectangle (ADR-0203).
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
        assert_eq!(placement.local, (1024, 768));
        assert_eq!(placement.source_extent, (1024, 768));
        assert_eq!(placement.dest_extent, (512, 384));
        assert_eq!(placement.rect(), Rect::new(100, 50, 512, 384));
        // A full-surface damage covers the whole destination.
        assert_eq!(
            placement.place_damage(&[]),
            vec![Rect::new(100, 50, 512, 384)]
        );
        // Half the surface in local coordinates maps to half the destination.
        assert_eq!(
            placement.place_damage(&[Rect::new(0, 0, 512, 768)]),
            vec![Rect::new(100, 50, 256, 384)]
        );
    }

    /// `wp_viewport.set_source` selects which part of the buffer is visible and
    /// **moves where the visible content begins**. A crop at (100, 100) draws
    /// surface pixel (100, 100) at the surface's output origin, so damage at
    /// the crop's own origin must land at the output origin — not offset by
    /// the crop's coordinates.
    ///
    /// An earlier revision divided by `local` without subtracting
    /// `source_origin`, which placed every cropped surface's damage at
    /// `origin + crop_origin * scale`; the compositor repainted the wrong
    /// pixels and left the changed ones stale.
    #[test]
    fn test_inv_arch_92_viewport_source_crop_rebases_damage_at_the_drawn_origin() {
        let placement = SurfacePlacement::resolve(
            &SurfaceGeometry {
                position: tessera_primitives::Point { x: 0, y: 0 },
                viewport_src: Some(Rect::new(100, 100, 400, 300)),
                ..Default::default()
            },
            800,
            600,
        );
        assert_eq!(placement.local, (800, 600));
        assert_eq!(placement.source_origin, (100, 100));
        assert_eq!(placement.source_extent, (400, 300));
        assert_eq!(placement.dest_extent, (400, 300));
        assert_eq!(placement.rect(), Rect::new(0, 0, 400, 300));

        // Damage at the crop's origin lands at the output origin.
        assert_eq!(
            placement.place(Rect::new(100, 100, 200, 150)),
            Some(Rect::new(0, 0, 200, 150))
        );
        // Damage in the crop's middle lands proportionally inside the rect.
        assert_eq!(
            placement.place(Rect::new(300, 250, 200, 150)),
            Some(Rect::new(200, 150, 200, 150))
        );
        // Damage wholly outside the crop reaches no output pixel.
        assert_eq!(placement.place(Rect::new(0, 0, 100, 100)), None);
        assert_eq!(placement.place(Rect::new(600, 500, 200, 100)), None);
        // A crop straddling the boundary clips to the visible part.
        assert_eq!(
            placement.place(Rect::new(50, 100, 100, 100)),
            Some(Rect::new(0, 0, 50, 100))
        );
        // "No usable damage" means the whole *visible* content.
        assert_eq!(placement.place_damage(&[]), vec![Rect::new(0, 0, 400, 300)]);
    }

    /// A source crop combined with a destination that scales it: the map is
    /// `origin + (p - source_origin) * dest/source`.
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
        // Source (400x300) drawn at 200x150: a 0.5 scale.
        assert_eq!(
            placement.place(Rect::new(200, 100, 400, 300)),
            Some(Rect::new(10, 20, 200, 150))
        );
        assert_eq!(
            placement.place(Rect::new(200, 100, 200, 150)),
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
            placement.place_damage(&[Rect::new(0, 0, 400, 300)]),
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
    fn ring_slot_damage_accumulates_missed_frames() {
        let first = FrameDamage::Area(vec![Rect::new(10, 10, 20, 20)]);
        let second = FrameDamage::Area(vec![Rect::new(40, 40, 10, 10)]);
        let mut slots = Vec::new();

        // Every ring image begins undefined and therefore repaints in full on
        // its first acquisition.
        assert_eq!(
            composite_repaint_for_slot(&mut slots, 0, first.clone()),
            FrameDamage::Full
        );
        record_composite_present(&mut slots, 0, first.clone());
        assert_eq!(slots, [FrameDamage::None]);

        assert_eq!(
            composite_repaint_for_slot(&mut slots, 1, second.clone()),
            FrameDamage::Full
        );
        record_composite_present(&mut slots, 1, second.clone());
        // Slot zero missed the second frame; when it comes around again, its
        // repaint includes both that history and the new frame's damage.
        assert_eq!(slots, [second, FrameDamage::None]);
        let combined = composite_repaint_for_slot(&mut slots, 0, first);
        // The combined damage covers both rects (union spans 10..50).
        assert_eq!(combined.area_union(), Some(Rect::new(10, 10, 40, 40)));
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
