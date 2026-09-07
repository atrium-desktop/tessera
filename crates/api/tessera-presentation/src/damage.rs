use tessera_model::{Rect, SurfaceDmabuf, SurfaceGeometry, SurfacePixels};

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

/// Damage assessment split by consumer.
///
/// `output` includes chrome-only changes (hover, clock, notifications, cursor
/// fallback) and controls the final swapchain repaint. `backdrop_source`
/// contains only changes to the desktop scene sampled by backdrop effects.
/// Keeping these separate prevents a Dock hover or status-clock tick from
/// needlessly recapturing and reblurring otherwise unchanged client pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessedFrameDamage {
    pub output: FrameDamage,
    pub backdrop_source: FrameDamage,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientDamage {
    None,
    Area(Vec<Rect>),
    Full,
}

impl ClientDamage {
    /// Map logical damage rectangles onto the physical framebuffer. Each
    /// logical rect is mapped independently, preserving disjoint regions for
    /// the KMS hint. A logical area that does not map cleanly onto the
    /// framebuffer must not silently become "no damage": if any rect is
    /// unmappable the whole frame is Full.
    pub fn into_frame_damage(self, scale: f32, physical_size: (u32, u32)) -> FrameDamage {
        match self {
            ClientDamage::None => FrameDamage::None,
            ClientDamage::Full => FrameDamage::Full,
            ClientDamage::Area(logical) => {
                let mut physical = Vec::with_capacity(logical.len());
                for rect in logical {
                    match logical_to_physical(rect, scale, physical_size) {
                        Some(p) => physical.push(p),
                        None => return FrameDamage::Full,
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
    fn geometry(&self) -> &SurfaceGeometry;
    fn width(&self) -> i32;
    fn height(&self) -> i32;
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
    fn geometry(&self) -> &SurfaceGeometry {
        &self.geometry
    }
    fn width(&self) -> i32 {
        self.width
    }
    fn height(&self) -> i32 {
        self.height
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
    fn geometry(&self) -> &SurfaceGeometry {
        &self.geometry
    }
    fn width(&self) -> i32 {
        self.width
    }
    fn height(&self) -> i32 {
        self.height
    }
}

/// Per-surface state as of the previous damage assessment. Geometry belongs
/// in the baseline alongside content generation: moving/resizing a surface
/// exposes its old pixels even when the buffer itself did not change.
#[derive(Clone, Copy)]
struct SurfaceDamageBaseline {
    generation: u64,
    geometry: SurfaceGeometry,
    width: i32,
    height: i32,
}

struct SurfaceDamageSample<'a> {
    id: usize,
    generation: u64,
    damage: &'a [Rect],
    geometry: &'a SurfaceGeometry,
    width: i32,
    height: i32,
}

/// One surface's changed region(s) in compositor logical coordinates: the
/// (clamped) damage rects translated by the surface's compositor position, or
/// the whole surface rect when the commit carried no usable damage information
/// (`damage` empty). Each damage rect is preserved individually rather than
/// unioned, so disjoint dirty regions stay disjoint for the KMS hint. The
/// clamping mirrors the renderer's incremental-upload path so both agree on
/// what "usable damage" means.
fn surface_damage_logical(
    position: tessera_model::Point,
    width: i32,
    height: i32,
    damage: &[Rect],
) -> Vec<Rect> {
    if width <= 0 || height <= 0 {
        return Vec::new();
    }
    if damage.is_empty() {
        return vec![Rect::new(position.x, position.y, width, height)];
    }
    let mut out = Vec::with_capacity(damage.len());
    let surface = Rect::new(0, 0, width, height);
    for d in damage {
        let Some(clipped) = d.intersect(surface) else {
            continue;
        };
        out.push(Rect::new(
            position.x.saturating_add(clipped.origin.x),
            position.y.saturating_add(clipped.origin.y),
            clipped.size.w,
            clipped.size.h,
        ));
    }
    out
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

/// Map logical damage rectangles into a [`FrameDamage`], degrading to `Full`
/// when any rectangle cannot be represented on the physical framebuffer —
/// the same policy as client damage mapping.
pub fn logical_rects_to_frame(
    logical: Vec<Rect>,
    scale: f32,
    physical_size: (u32, u32),
) -> FrameDamage {
    let mut physical = Vec::with_capacity(logical.len());
    for rect in logical {
        match logical_to_physical(rect, scale, physical_size) {
            Some(mapped) => physical.push(mapped),
            // Unmappable means "cannot express this damage as rectangles";
            // the only safe output is a full repaint.
            None => return FrameDamage::Full,
        }
    }
    FrameDamage::from_rects(physical)
}

/// Convert output damage to the single physical-pixel rectangle accepted by
/// Vulkan dynamic rendering. `None` deliberately means a full-destination
/// pass: both [`FrameDamage::Full`] and the conservative empty-area fallback
/// must leave scissoring disabled.
pub fn frame_damage_render_area(repaint: &FrameDamage) -> Option<flux::CanvasRenderArea> {
    repaint.area_union().and_then(|rect| {
        (!rect.is_empty()).then_some(flux::CanvasRenderArea {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.w as u32,
            height: rect.size.h as u32,
        })
    })
}

/// Record one visible surface into the generation diff. Returns `true` when
/// this surface alone forces full damage: it appeared since the last
/// assessment, or its contents changed in a way that cannot be mapped to a
/// rectangle (wp_viewport, or an in-flight window transition).
fn accumulate_surface(
    old: &std::collections::HashMap<usize, SurfaceDamageBaseline>,
    new: &mut std::collections::HashMap<usize, SurfaceDamageBaseline>,
    sample: SurfaceDamageSample<'_>,
    rects: &mut Vec<Rect>,
) -> bool {
    let SurfaceDamageSample {
        id,
        generation,
        damage,
        geometry,
        width,
        height,
    } = sample;
    new.insert(
        id,
        SurfaceDamageBaseline {
            generation,
            geometry: *geometry,
            width,
            height,
        },
    );
    let Some(previous) = old.get(&id) else {
        return true;
    };
    let geometry_unchanged = previous.width == width
        && previous.height == height
        && previous.geometry.position == geometry.position
        && previous.geometry.window_geometry == geometry.window_geometry
        && previous.geometry.transform == geometry.transform
        && previous.geometry.buffer_scale == geometry.buffer_scale
        && previous.geometry.viewport_src == geometry.viewport_src
        && previous.geometry.viewport_dst == geometry.viewport_dst
        && previous.geometry.transition_size == geometry.transition_size;
    if !geometry_unchanged {
        // The old rect must be erased as well as the new one painted. Without
        // storing a region history across every stacking/clip case, full
        // damage is the only universally correct answer.
        return true;
    }
    if previous.generation == generation {
        return false;
    }
    // Damage here is already in surface-local logical coordinates — the server
    // applied the buffer transform at commit (see
    // Transform::map_buffer_rect_to_surface) — so a non-identity transform does
    // NOT force a full-output repaint here. Only the cases that genuinely break
    // the logical→physical mapping remain unmappable: wp_viewport
    // crop/destination (they change which buffer pixels map to which surface
    // pixels) and an in-flight transition_size (it draws the surface at a size
    // unrelated to the buffer-implied logical extent).
    let mappable = geometry.viewport_src.is_none()
        && geometry.viewport_dst.is_none()
        && geometry.transition_size.is_none();
    if !mappable {
        return true;
    }
    let scale = geometry.buffer_scale.max(1) as f32;
    // A rotated/flipped buffer's logical extent swaps its axes; use the
    // post-transform dimensions so the damage rect is clamped to the surface's
    // actual visible size, not the raw buffer size.
    let (lw, lh) = if geometry.transform.swap_axes() {
        (height, width)
    } else {
        (width, height)
    };
    let logical_width = (lw as f32 / scale).round().max(1.0) as i32;
    let logical_height = (lh as f32 / scale).round().max(1.0) as i32;
    // Preserve each dirty rect individually rather than unioning, so disjoint
    // regions stay disjoint for the KMS FB_DAMAGE_CLIPS hint.
    rects.extend(surface_damage_logical(
        geometry.position,
        logical_width,
        logical_height,
        damage,
    ));
    false
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
        let mut full = false;

        for frame in frames {
            full |= accumulate_surface(
                old,
                new,
                SurfaceDamageSample {
                    id: frame.id(),
                    generation: frame.generation(),
                    damage: frame.damage(),
                    geometry: frame.geometry(),
                    width: frame.width(),
                    height: frame.height(),
                },
                &mut rects,
            );
        }
        // A surface that vanished since the last assessment uncovers whatever
        // was behind it; only its old rectangle is known to be stale, but its
        // stacking interplay is not tracked, so stay conservative.
        full |= old.keys().any(|id| !new.contains_key(id));
        self.scratch.clear();

        if full {
            ClientDamage::Full
        } else if rects.is_empty() {
            ClientDamage::None
        } else {
            ClientDamage::Area(rects)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_damage_maps_and_clamps_to_surface() {
        // Tight damage translates by the compositor position.
        let area = surface_damage_logical(
            tessera_model::Point { x: 100, y: 50 },
            800,
            600,
            &[Rect::new(10, 20, 30, 40)],
        );
        assert_eq!(area, vec![Rect::new(110, 70, 30, 40)]);
        // Out-of-bounds damage clamps like the renderer's upload path.
        let clamped = surface_damage_logical(
            tessera_model::Point { x: 0, y: 0 },
            800,
            600,
            &[Rect::new(-20, 590, 900, 50)],
        );
        assert_eq!(clamped, vec![Rect::new(0, 590, 800, 10)]);
        // Partial negative damage clips by intersection rather than moving the
        // origin to zero while preserving the original width.
        let partial_negative = surface_damage_logical(
            tessera_model::Point { x: 10, y: 20 },
            800,
            600,
            &[Rect::new(-20, 10, 30, 20)],
        );
        assert_eq!(partial_negative, vec![Rect::new(10, 30, 10, 20)]);
        // A report wholly outside the surface contributes no invented edge
        // damage.
        assert!(
            surface_damage_logical(
                tessera_model::Point { x: 0, y: 0 },
                800,
                600,
                &[Rect::new(-50, 10, 20, 20)],
            )
            .is_empty()
        );
        // No damage information damages the whole surface.
        let whole = surface_damage_logical(tessera_model::Point { x: 5, y: 6 }, 800, 600, &[]);
        assert_eq!(whole, vec![Rect::new(5, 6, 800, 600)]);
        // Fully degenerate damage reports nothing.
        let degenerate = surface_damage_logical(
            tessera_model::Point { x: 0, y: 0 },
            800,
            600,
            &[Rect::new(10, 10, 0, 0)],
        );
        assert!(degenerate.is_empty());
        // Disjoint damage rects are preserved individually, not unioned.
        let disjoint = surface_damage_logical(
            tessera_model::Point { x: 0, y: 0 },
            800,
            600,
            &[Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)],
        );
        assert_eq!(
            disjoint,
            vec![Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)]
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
        // One unmappable rect degrades the whole frame to Full.
        assert_eq!(damage.into_frame_damage(0.0, (100, 100)), FrameDamage::Full);
        assert_eq!(
            ClientDamage::None.into_frame_damage(1.0, (100, 100)),
            FrameDamage::None
        );
        assert_eq!(
            ClientDamage::Full.into_frame_damage(1.0, (100, 100)),
            FrameDamage::Full
        );
    }

    #[test]
    fn tracker_forces_full_on_geometry_change_even_without_new_content() {
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
            fn geometry(&self) -> &SurfaceGeometry {
                &self.geometry
            }
            fn width(&self) -> i32 {
                100
            }
            fn height(&self) -> i32 {
                80
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_model::Point { x: 10, y: 20 },
            ..Default::default()
        };
        let moved = SurfaceGeometry {
            position: tessera_model::Point { x: 11, y: 20 },
            ..geometry
        };

        let mut tracker = ClientDamageTracker::default();
        assert!(
            matches!(
                tracker.assess([&Sample {
                    generation: 3,
                    damage: Vec::new(),
                    geometry,
                } as &dyn SurfaceDamageFrame]),
                ClientDamage::Full
            ),
            "first sighting forces full"
        );
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 3,
                damage: Vec::new(),
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::None
        ));
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 3,
                damage: Vec::new(),
                geometry: moved,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::Full
        ));
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
            fn geometry(&self) -> &SurfaceGeometry {
                &self.geometry
            }
            fn width(&self) -> i32 {
                100
            }
            fn height(&self) -> i32 {
                80
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_model::Point { x: 10, y: 20 },
            ..Default::default()
        };

        let mut tracker = ClientDamageTracker::default();
        assert!(matches!(
            tracker.assess([&Sample {
                generation: 3,
                damage: Vec::new(),
                geometry,
            } as &dyn SurfaceDamageFrame]),
            ClientDamage::Full
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
            fn geometry(&self) -> &SurfaceGeometry {
                &self.geometry
            }
            fn width(&self) -> i32 {
                200
            }
            fn height(&self) -> i32 {
                160
            }
        }
        let geometry = SurfaceGeometry {
            position: tessera_model::Point { x: 10, y: 20 },
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
            ClientDamage::Full
        ));
        let content = tracker.assess([&Sample {
            generation: 4,
            damage: vec![Rect::new(2, 3, 4, 5)],
            geometry,
        } as &dyn SurfaceDamageFrame]);
        assert_eq!(content, ClientDamage::Area(vec![Rect::new(12, 23, 4, 5)]));
    }
}
