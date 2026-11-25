//! The backdrop beneath chrome for the frosted and liquid-glass materials
//! (ADR-0253).
//!
//! The shell reports where its floating bodies are
//! ([`tessera_shell::Shell::backdrop_bodies`]); this module turns those into a
//! [`BackdropPlan`] in output pixels, captures the desktop beneath the padded
//! body regions into an offscreen image, blurs it, runs the material's Optics
//! filter, and hands back one slab the output pass draws under chrome. The
//! body's own translucent fill is painted over the slab by the shell, so the
//! slab is the optics and the body is the tint.
//!
//! Nothing here exists while the effective material does not sample the
//! backdrop: [`ChromeBackdrop::release`] drops every filter and capture, so an
//! opaque or translucent shell pays for none of it ([INV-MAT-05]).

use super::*;
use tessera_design::materials::BackdropBody;
use tessera_desktop::settings::Material;
use tessera_primitives::Rect;

/// Frost blur width, in logical pixels. Wide enough that text and edges
/// beneath a body dissolve into colour, which is what lets the body's tint
/// be more open than the translucent material's.
const FROST_SIGMA: f32 = 20.0;
/// Liquid glass refracts a lightly blurred image: the lens, not the blur,
/// carries the look.
const GLASS_SIGMA: f32 = 8.0;
/// Interior frost of a chrome glass body. Optics' default (1.0) frosts the
/// centre of a lens by 8 %, which is clear glass: right for a control over
/// a photo, too busy behind a menu's text. 4.0 frosts the interior by about
/// two thirds and leaves the refracting rim and its highlights intact.
const GLASS_FROST: f32 = 4.0;
/// How far the blur samples beyond the pixel it writes, in sigmas.
const FOOTPRINT_SIGMAS: f32 = 3.0;
/// Capture targets grow in steps, so a body that animates its size does not
/// reallocate the capture every frame.
const CAPTURE_BUCKET: u32 = 128;

/// One body in physical output pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlacedBody {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
    pub(super) radius: f32,
    pub(super) opacity: f32,
    /// The body padded by the material's sampling reach and clamped to the
    /// output: every pixel whose change can alter what this body shows.
    pub(super) reach: Rect,
}

impl PlacedBody {
    fn covered(&self) -> Rect {
        let x0 = self.x.floor() as i32;
        let y0 = self.y.floor() as i32;
        let x1 = (self.x + self.w).ceil() as i32;
        let y1 = (self.y + self.h).ceil() as i32;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// What one output must capture and draw beneath chrome this frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BackdropPlan {
    pub(super) material: Material,
    /// Blur sigma in physical pixels.
    pub(super) sigma: f32,
    pub(super) scale: f32,
    /// Bodies in paint order.
    pub(super) bodies: Vec<PlacedBody>,
    /// The capture's extent: the union of every body's reach, in output
    /// pixels. The capture image covers exactly this rectangle.
    pub(super) extent: Rect,
}

/// Plan the backdrop for `bodies` (logical output pixels) on an output of
/// `output_px` physical pixels at `scale`. `None` when `material` does not
/// sample the backdrop or no body is visible on this output.
pub(super) fn plan(
    material: Material,
    bodies: &[BackdropBody],
    scale: f32,
    output_px: (u32, u32),
) -> Option<BackdropPlan> {
    if !material.samples_backdrop() || bodies.is_empty() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let output = Rect::new(
        0,
        0,
        i32::try_from(output_px.0).unwrap_or(i32::MAX),
        i32::try_from(output_px.1).unwrap_or(i32::MAX),
    );
    let (sigma_logical, lens_reach) = match material {
        Material::LiquidGlass => (
            GLASS_SIGMA,
            optics_material::LiquidGlassParams::default().refraction,
        ),
        _ => (FROST_SIGMA, 0.0),
    };
    let sigma = sigma_logical * scale;
    let pad = ((sigma_logical * FOOTPRINT_SIGMAS + lens_reach) * scale).ceil() as i32;
    let mut placed = Vec::with_capacity(bodies.len());
    let mut extent: Option<Rect> = None;
    for body in bodies {
        let finite = [
            body.bounds.x,
            body.bounds.y,
            body.bounds.w,
            body.bounds.h,
            body.radius,
        ]
        .iter()
        .all(|value| value.is_finite());
        if !finite || body.bounds.w <= 0.0 || body.bounds.h <= 0.0 || body.opacity <= 0.0 {
            continue;
        }
        let mut placed_body = PlacedBody {
            x: body.bounds.x * scale,
            y: body.bounds.y * scale,
            w: body.bounds.w * scale,
            h: body.bounds.h * scale,
            radius: body.radius.max(0.0) * scale,
            opacity: body.opacity.min(1.0),
            reach: Rect::new(0, 0, 0, 0),
        };
        let covered = placed_body.covered();
        let padded = Rect::new(
            covered.origin.x.saturating_sub(pad),
            covered.origin.y.saturating_sub(pad),
            covered.size.w.saturating_add(pad.saturating_mul(2)),
            covered.size.h.saturating_add(pad.saturating_mul(2)),
        );
        let Some(reach) = padded.intersect(output) else {
            continue;
        };
        if covered.intersect(output).is_none() {
            continue;
        }
        placed_body.reach = reach;
        extent = Some(extent.map_or(reach, |extent| extent.union(reach)));
        placed.push(placed_body);
    }
    Some(BackdropPlan {
        material,
        sigma,
        scale,
        bodies: placed,
        extent: extent?,
    })
}

impl BackdropPlan {
    /// Widen `damage` so a body repaints when anything it samples changed.
    ///
    /// A frosted body shows a blur of the pixels around it: a window that
    /// redraws next to the dock changes the dock without touching it. Any
    /// damage inside a body's reach therefore damages the whole body. `Full`
    /// and `None` are returned unchanged — nothing changed means nothing the
    /// body samples changed either.
    pub(super) fn widen(&self, damage: FrameDamage) -> FrameDamage {
        let FrameDamage::Area(rects) = &damage else {
            return damage;
        };
        let owed: Vec<Rect> = self
            .bodies
            .iter()
            .filter(|body| {
                rects
                    .iter()
                    .any(|rect| rect.intersect(body.reach).is_some())
            })
            // The body is what shows the change; its reach only decides
            // whether it must. `reach` is clamped to the output, so the
            // intersection clamps the body too.
            .filter_map(|body| body.covered().intersect(body.reach))
            .collect();
        if owed.is_empty() {
            damage
        } else {
            damage.with_rects(owed)
        }
    }

    /// Keep only the bodies the pass will actually repaint. An empty scope is
    /// the whole output. `None` when no body is inside the scope, so the
    /// frame takes no capture at all.
    pub(super) fn restrict_to(mut self, scope: &[optics_gfx::CanvasRenderArea]) -> Option<Self> {
        if !scope.is_empty() {
            self.bodies.retain(|body| {
                let covered = body.covered();
                scope.iter().any(|area| {
                    let area = Rect::new(
                        area.x.floor() as i32,
                        area.y.floor() as i32,
                        area.width.ceil() as i32,
                        area.height.ceil() as i32,
                    );
                    area.intersect(covered).is_some()
                })
            });
        }
        let first = self.bodies.first()?.reach;
        self.extent = self
            .bodies
            .iter()
            .fold(first, |extent, body| extent.union(body.reach));
        Some(self)
    }

    /// The capture image size: the extent rounded up to a bucket.
    pub(super) fn capture_size(&self) -> (u32, u32) {
        let w = u32::try_from(self.extent.size.w.max(1)).unwrap_or(1);
        let h = u32::try_from(self.extent.size.h.max(1)).unwrap_or(1);
        (
            optics_gfx::bucket_extent(w, CAPTURE_BUCKET),
            optics_gfx::bucket_extent(h, CAPTURE_BUCKET),
        )
    }

    /// Each body's reach in capture-image pixels: the regions the capture
    /// passes write and the blur must keep valid.
    pub(super) fn capture_regions(&self) -> Vec<optics_gfx::BlurRegion> {
        let origin = self.extent.origin;
        self.bodies
            .iter()
            .map(|body| optics_gfx::BlurRegion {
                x: u32::try_from(body.reach.origin.x - origin.x).unwrap_or(0),
                y: u32::try_from(body.reach.origin.y - origin.y).unwrap_or(0),
                width: u32::try_from(body.reach.size.w).unwrap_or(0),
                height: u32::try_from(body.reach.size.h).unwrap_or(0),
            })
            .collect()
    }

    /// The capture passes' damage scope: the same regions, as pass areas.
    pub(super) fn capture_scope(&self) -> Vec<optics_gfx::CanvasRenderArea> {
        self.capture_regions()
            .into_iter()
            .map(|region| optics_gfx::CanvasRenderArea {
                x: region.x as f32,
                y: region.y as f32,
                width: region.width as f32,
                height: region.height as f32,
            })
            .collect()
    }

    fn local(&self, body: &PlacedBody) -> (f32, f32) {
        (
            body.x - self.extent.origin.x as f32,
            body.y - self.extent.origin.y as f32,
        )
    }

    fn frosted_groups(&self) -> Vec<optics_material::FrostedGroup> {
        self.bodies
            .iter()
            .map(|body| {
                let (x, y) = self.local(body);
                optics_material::FrostedGroup {
                    shape: optics_material::FrostedShape {
                        x,
                        y,
                        width: body.w,
                        height: body.h,
                        corner_radius: body.radius,
                    },
                    opacity: body.opacity,
                    tint_color: [255, 255, 255],
                    // The shell's body is the tint; the filter is the optics.
                    tint_strength: Some(0.0),
                    saturation: None,
                    // The shell paints the elevation halo.
                    shadow_alpha: 0.0,
                    shadow_blur: 0.0,
                    shadow_offset_y: 0.0,
                    noise_intensity: None,
                }
            })
            .collect()
    }

    fn glass_groups(&self) -> Vec<optics_material::LiquidGlassGroup> {
        self.bodies
            .iter()
            .map(|body| {
                let (x, y) = self.local(body);
                optics_material::LiquidGlassGroup {
                    primary: optics_material::LiquidGlassShape {
                        x,
                        y,
                        width: body.w,
                        height: body.h,
                        corner_radius: body.radius,
                    },
                    merged: None,
                    blend_radius: 0.0,
                    opacity: body.opacity,
                    shadow_alpha: 0.0,
                    shadow_blur: 0.0,
                    shadow_offset_y: 0.0,
                    tint_color: [255, 255, 255],
                    focus: None,
                    frost_strength: None,
                    tint_strength: Some(0.0),
                    saturation: None,
                    curvature: None,
                    contact_ao: None,
                    ambient_fresnel: None,
                }
            })
            .collect()
    }

    fn glass_params(&self) -> optics_material::LiquidGlassParams {
        let base = optics_material::LiquidGlassParams::default();
        // The defaults are tuned in logical pixels; the capture is physical.
        optics_material::LiquidGlassParams {
            refraction: base.refraction * self.scale,
            edge_width: base.edge_width * self.scale,
            size_reference: base.size_reference * self.scale,
            tint_strength: 0.0,
            frost_strength: GLASS_FROST,
            ..base
        }
    }
}

/// The material's optics for one frame, ready to draw under chrome. Borrows
/// the filter whose frame slot holds it.
pub(super) enum BackdropSlab<'filter> {
    Frosted(optics_material::FrostedImage<'filter>, (f32, f32)),
    Glass(optics_material::LiquidGlassImage<'filter>, (f32, f32)),
}

impl BackdropSlab<'_> {
    /// Draw the slab at its place on the output. `canvas` must be at the
    /// output's root transform (physical output pixels).
    pub(super) fn draw(&self, canvas: &optics_gfx::CanvasCommands<'_>) {
        canvas.save();
        match self {
            Self::Frosted(image, (x, y)) => {
                canvas.translate(*x, *y);
                image.draw(canvas);
            }
            Self::Glass(image, (x, y)) => {
                canvas.translate(*x, *y);
                image.draw(canvas);
            }
        }
        canvas.restore();
    }
}

/// Owns the backdrop filters and capture targets. Holds nothing until a
/// backdrop-sampling material is first drawn.
#[derive(Default)]
pub(super) struct ChromeBackdrop {
    blur: Option<optics_gfx::BlurFilter>,
    frosted: Option<optics_material::FrostedFilter>,
    glass: Option<optics_material::LiquidGlassFilter>,
    /// Capture targets keyed by `(output namespace, frame slot)`: a capture
    /// is written and sampled within one frame, and another slot's frame may
    /// still be reading its own.
    captures: std::collections::HashMap<(u64, usize), optics_gfx::Image>,
    /// A failure is final for the session: the runtime then draws the
    /// translucent material instead of retrying a broken pass every frame.
    failed: bool,
}

impl ChromeBackdrop {
    /// Whether this renderer can still draw a backdrop-sampling material.
    pub(super) fn available(&self) -> bool {
        !self.failed
    }

    /// Drop every filter and capture target. Called whenever the effective
    /// material stops sampling the backdrop.
    pub(super) fn release(&mut self) {
        self.blur = None;
        self.frosted = None;
        self.glass = None;
        self.captures.clear();
    }

    /// Record that drawing the desktop into the capture failed.
    pub(super) fn capture_failed(&mut self, error: impl std::fmt::Display) {
        self.fail("capturing the desktop beneath chrome", error);
    }

    fn fail(&mut self, stage: &str, error: impl std::fmt::Display) {
        if self.failed {
            return;
        }
        log::warn!(
            "backdrop: {stage} failed ({error}); chrome falls back to the translucent material"
        );
        self.failed = true;
        self.release();
    }

    /// The capture target for this output and frame slot, sized for `plan`.
    pub(super) fn capture_target(
        &mut self,
        device: &optics_gfx::Device,
        namespace: u64,
        slot: usize,
        plan: &BackdropPlan,
    ) -> Option<&mut optics_gfx::Image> {
        if self.failed {
            return None;
        }
        let size = plan.capture_size();
        let key = (namespace, slot);
        if self
            .captures
            .get(&key)
            .is_none_or(|image| image.size() != size)
        {
            match optics_gfx::Image::render_target(
                device,
                size.0,
                size.1,
                optics_gfx::Format::Rgba8Unorm,
            ) {
                Ok(image) => {
                    self.captures.insert(key, image);
                }
                Err(error) => {
                    self.fail("allocating the capture target", error);
                    return None;
                }
            }
        }
        self.captures.get_mut(&key)
    }

    /// Blur this frame's capture and run the material's filter over it.
    /// Call after the capture pass ended and before the output pass opens.
    ///
    /// The slab is drawable inside the output pass without a separate
    /// materialize step: an Optics material filter requires its own deferred
    /// pass on `apply`, because the host drawing its slab is the terminal
    /// consumer (and it materializes the blurred input it reads).
    pub(super) fn compose<'filter>(
        &'filter mut self,
        device: &optics_gfx::Device,
        frame: &optics_gfx::Frame<'_>,
        namespace: u64,
        plan: &BackdropPlan,
    ) -> Option<BackdropSlab<'filter>> {
        if self.failed {
            return None;
        }
        let key = (namespace, frame.index() as usize);
        if self.blur.is_none() {
            match optics_gfx::BlurFilter::new(device) {
                Ok(filter) => self.blur = Some(filter),
                Err(error) => {
                    self.fail("creating the blur filter", error);
                    return None;
                }
            }
        }
        match plan.material {
            Material::Frosted if self.frosted.is_none() => {
                match optics_material::FrostedFilter::new(device) {
                    Ok(filter) => self.frosted = Some(filter),
                    Err(error) => {
                        self.fail("creating the frosted filter", error);
                        return None;
                    }
                }
            }
            Material::LiquidGlass if self.glass.is_none() => {
                match optics_material::LiquidGlassFilter::new(device) {
                    Ok(filter) => self.glass = Some(filter),
                    Err(error) => {
                        self.fail("creating the liquid-glass filter", error);
                        return None;
                    }
                }
            }
            _ => {}
        }
        // Only the filter in use stays resident.
        match plan.material {
            Material::Frosted => self.glass = None,
            Material::LiquidGlass => self.frosted = None,
            _ => return None,
        }
        let origin = (plan.extent.origin.x as f32, plan.extent.origin.y as f32);
        let regions = plan.capture_regions();
        let Self {
            blur,
            frosted,
            glass,
            captures,
            failed,
        } = self;
        let capture = captures.get(&key)?;
        let blur = blur.as_mut()?;
        let result = dispatch(
            blur,
            frosted.as_mut(),
            glass.as_mut(),
            frame,
            capture,
            plan,
            &regions,
            origin,
        );
        match result {
            Ok(slab) => Some(slab),
            Err((stage, error)) => {
                // The filters stay borrowed for the slab's lifetime, so the
                // failure is recorded through the field; the runtime demotes
                // the material on the next frame and `release` frees them.
                log::warn!(
                    "backdrop: {stage} failed ({error}); chrome falls back to the translucent material"
                );
                *failed = true;
                None
            }
        }
    }
}

type DispatchError = (&'static str, optics_gfx::Error);

/// Blur `capture` and run the plan's material filter over it.
#[allow(clippy::too_many_arguments)]
fn dispatch<'filter>(
    blur: &mut optics_gfx::BlurFilter,
    frosted: Option<&'filter mut optics_material::FrostedFilter>,
    glass: Option<&'filter mut optics_material::LiquidGlassFilter>,
    frame: &optics_gfx::Frame<'_>,
    capture: &optics_gfx::Image,
    plan: &BackdropPlan,
    regions: &[optics_gfx::BlurRegion],
    origin: (f32, f32),
) -> Result<BackdropSlab<'filter>, DispatchError> {
    let missing = |stage| {
        (
            stage,
            optics_gfx::Error(optics_gfx::sys::optics_gfx_result::OPTICS_GFX_ERROR_INVALID_STATE),
        )
    };
    let blurred = blur
        .apply_regions(frame, capture, plan.sigma, regions)
        .map_err(|error| ("blurring the capture", error))?;
    match plan.material {
        Material::Frosted => {
            let params = optics_material::FrostedParams {
                saturation: 1.35,
                tint_strength: 0.0,
                ..optics_material::FrostedParams::default()
            };
            frosted
                .ok_or_else(|| missing("the frosted filter"))?
                .apply(frame, capture, &blurred, &plan.frosted_groups(), params)
                .map(|image| BackdropSlab::Frosted(image, origin))
                .map_err(|error| ("the frosted dispatch", error))
        }
        Material::LiquidGlass => glass
            .ok_or_else(|| missing("the liquid-glass filter"))?
            .apply(
                frame,
                capture,
                &blurred,
                &plan.glass_groups(),
                plan.glass_params(),
            )
            .map(|image| BackdropSlab::Glass(image, origin))
            .map_err(|error| ("the liquid-glass dispatch", error)),
        _ => Err(missing("a backdrop-sampling material")),
    }
}

/// Draw the desktop beneath `plan`'s bodies into `capture`.
///
/// The wallpaper, then `scene` (the same windows the output pass draws), in
/// **one** pass scoped to the bodies' reach, so the cost is the scene
/// re-drawn under chrome, not a second full frame. One pass is also a
/// correctness requirement: Optics links an effect to the first pass that
/// wrote its input, and a frame whose capture has a second writer the blur
/// does not depend on is rejected when the output pass opens. That is why the
/// 3D wallpaper model, which needs a pass of its own, is not captured.
/// `scene` runs with the canvas translated so output pixel
/// `plan.extent.origin` lands on the capture's origin.
#[allow(clippy::too_many_arguments)]
pub(super) fn capture_desktop(
    canvas: &mut optics_gfx::Canvas,
    device: &optics_gfx::Device,
    frame: &mut optics_gfx::Frame<'_>,
    capture: &mut optics_gfx::Image,
    wallpaper: &mut Option<wallpaper::Wallpaper>,
    plan: &BackdropPlan,
    logical_size: (u32, u32),
    clear: u32,
    draw_wallpaper: bool,
    scene: impl FnOnce(&optics_gfx::CanvasCommands<'_>) -> Result<(), optics_gfx::Error>,
) -> Result<(), optics_gfx::Error> {
    let session = canvas.begin_session(
        optics_gfx::ImageTarget {
            frame,
            image: capture,
        },
        optics_gfx::CanvasPassOptions {
            clear: Some(clear),
            damage: plan.capture_scope(),
        },
    )?;
    session.save();
    session.translate(
        -(plan.extent.origin.x as f32),
        -(plan.extent.origin.y as f32),
    );
    if draw_wallpaper {
        draw_wallpaper_background(&session, device, wallpaper, logical_size, plan.scale);
    }
    let drawn = scene(&session);
    session.restore();
    let ended = session.end();
    drawn.and(ended)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(x: f32, y: f32, w: f32, h: f32) -> BackdropBody {
        BackdropBody {
            bounds: optics_ui::Rect { x, y, w, h },
            radius: 18.0,
            opacity: 1.0,
        }
    }

    const OUTPUT: (u32, u32) = (1920, 1080);

    #[test]
    fn only_a_sampled_material_with_a_visible_body_plans_a_capture() {
        let dock = [body(600.0, 1000.0, 720.0, 64.0)];
        assert!(plan(Material::Opaque, &dock, 1.0, OUTPUT).is_none());
        assert!(plan(Material::Translucent, &dock, 1.0, OUTPUT).is_none());
        assert!(plan(Material::Frosted, &[], 1.0, OUTPUT).is_none());
        assert!(
            plan(
                Material::Frosted,
                &[body(4000.0, 0.0, 10.0, 10.0)],
                1.0,
                OUTPUT
            )
            .is_none()
        );
        assert!(plan(Material::Frosted, &dock, 1.0, OUTPUT).is_some());
        assert!(plan(Material::LiquidGlass, &dock, 1.0, OUTPUT).is_some());
    }

    #[test]
    fn the_capture_covers_each_body_and_its_blur_reach_clamped_to_the_output() {
        let plan = plan(
            Material::Frosted,
            &[body(600.0, 1000.0, 720.0, 64.0)],
            2.0,
            (3840, 2160),
        )
        .expect("a dock plans a capture");
        let dock = &plan.bodies[0];
        assert_eq!(
            (dock.x, dock.y, dock.w, dock.h),
            (1200.0, 2000.0, 1440.0, 128.0)
        );
        let pad = (FROST_SIGMA * FOOTPRINT_SIGMAS * 2.0) as i32;
        assert_eq!(dock.reach.origin.x, 1200 - pad);
        assert_eq!(dock.reach.origin.y, 2000 - pad);
        assert_eq!(
            dock.reach.origin.y + dock.reach.size.h,
            2160,
            "the reach stops at the output edge"
        );
        assert_eq!(plan.extent, dock.reach);
        // A dock-only capture is a strip, not a full-output target.
        let (w, h) = plan.capture_size();
        assert!(w >= dock.reach.size.w as u32 && h >= dock.reach.size.h as u32);
        assert!(h < 2160 / 2, "{w}x{h}");
    }

    #[test]
    fn damage_beside_a_body_repaints_the_body_and_damage_elsewhere_does_not() {
        let plan = plan(
            Material::Frosted,
            &[body(600.0, 1000.0, 720.0, 64.0)],
            1.0,
            OUTPUT,
        )
        .expect("a dock plans a capture");
        let reach = plan.bodies[0].reach;
        // A window redrawing just above the dock: outside the body, inside
        // the blur's reach.
        let beside = FrameDamage::from_rects(vec![Rect::new(700, 960, 40, 20)]);
        let widened = plan.widen(beside);
        let rects = widened.area_rects().expect("still partial damage");
        let covers = |x: i32, y: i32| {
            rects
                .iter()
                .any(|rect| rect.contains(tessera_primitives::Point { x, y }))
        };
        assert!(
            covers(600, 1000) && covers(1319, 1063),
            "the whole body repaints"
        );
        assert!(
            !covers(reach.origin.x, reach.origin.y),
            "the reach decides, it is not itself repainted"
        );
        // A window redrawing far from any body owes the body nothing.
        let far = FrameDamage::from_rects(vec![Rect::new(10, 10, 40, 20)]);
        assert_eq!(plan.widen(far.clone()), far);
        assert_eq!(plan.widen(FrameDamage::None), FrameDamage::None);
        assert_eq!(plan.widen(FrameDamage::Full), FrameDamage::Full);
    }

    #[test]
    fn a_pass_that_does_not_touch_a_body_takes_no_capture() {
        let bodies = [
            body(600.0, 1000.0, 720.0, 64.0),
            body(1500.0, 40.0, 360.0, 120.0),
        ];
        let plan = plan(Material::Frosted, &bodies, 1.0, OUTPUT).expect("two bodies");
        let toast_only = vec![optics_gfx::CanvasRenderArea {
            x: 1500.0,
            y: 40.0,
            width: 10.0,
            height: 10.0,
        }];
        let restricted = plan.clone().restrict_to(&toast_only).expect("the toast");
        assert_eq!(restricted.bodies.len(), 1);
        assert_eq!(restricted.extent, restricted.bodies[0].reach);
        let nowhere = vec![optics_gfx::CanvasRenderArea {
            x: 10.0,
            y: 500.0,
            width: 10.0,
            height: 10.0,
        }];
        assert!(plan.clone().restrict_to(&nowhere).is_none());
        assert_eq!(plan.clone().restrict_to(&[]), Some(plan));
    }

    #[test]
    fn filter_shapes_are_in_capture_pixels_and_carry_each_body_fade() {
        let mut toast = body(1500.0, 40.0, 360.0, 120.0);
        toast.opacity = 0.5;
        let plan = plan(Material::Frosted, &[toast], 1.0, OUTPUT).expect("a toast");
        let group = plan.frosted_groups()[0];
        assert_eq!(group.shape.x, 1500.0 - plan.extent.origin.x as f32);
        assert_eq!(group.shape.y, 40.0 - plan.extent.origin.y as f32);
        assert_eq!(group.shape.corner_radius, 18.0);
        assert_eq!(group.opacity, 0.5);
        assert_eq!(
            group.shadow_alpha, 0.0,
            "the shell paints the elevation halo"
        );
        let regions = plan.capture_regions();
        let (w, h) = plan.capture_size();
        for region in regions {
            assert!(region.x + region.width <= w && region.y + region.height <= h);
        }
    }

    /// Draw `material` over a body on sharp 4 px stripes, on the real GPU, and
    /// return the frame as RGBA8 rows.
    fn render_stripes_under(material: Material) -> Option<(Vec<u8>, u32)> {
        const W: u32 = 256;
        const H: u32 = 128;
        let device = optics_gfx::Device::new(true, &[], &[], 0).ok()?;
        let surface = optics_gfx::Surface::offscreen_readback(&device, W, H).ok()?;
        let mut canvas = optics_gfx::Canvas::new(&surface).unwrap();
        let mut backdrop = ChromeBackdrop::default();
        let plan = plan(material, &[body(64.0, 32.0, 128.0, 64.0)], 1.0, (W, H))
            .expect("the body plans a capture");
        let stripes = |canvas: &optics_gfx::CanvasCommands<'_>| {
            for x in (0..W).step_by(8) {
                canvas.fill_rect(
                    x as f32,
                    0.0,
                    4.0,
                    H as f32,
                    optics_gfx::rgba(255, 255, 255, 255),
                );
            }
        };

        let mut frame = surface.begin_frame().unwrap();
        let slot = frame.index() as usize;
        let capture = backdrop.capture_target(&device, 0, slot, &plan).unwrap();
        let mut wallpaper = None;
        capture_desktop(
            &mut canvas,
            &device,
            &mut frame,
            capture,
            &mut wallpaper,
            &plan,
            (W, H),
            optics_gfx::rgba(0, 0, 0, 255),
            false,
            |canvas| {
                stripes(canvas);
                Ok(())
            },
        )
        .expect("the desktop is captured beneath the body");
        let slab = backdrop
            .compose(&device, &frame, 0, &plan)
            .expect("the material filter runs over the capture");
        let session = canvas
            .begin_session(
                &mut frame,
                optics_gfx::CanvasPassOptions {
                    clear: Some(optics_gfx::rgba(0, 0, 0, 255)),
                    ..Default::default()
                },
            )
            .unwrap();
        stripes(&session);
        slab.draw(&session);
        session.end().unwrap();
        frame.submit().unwrap().present().unwrap();
        let mut pixels = vec![0u8; (W * H * 4) as usize];
        surface.read_pixels(&mut pixels).unwrap();
        assert!(backdrop.available());
        Some((pixels, W))
    }

    /// The spread of the red channel along one row segment.
    fn spread(pixels: &[u8], width: u32, y: u32, xs: std::ops::Range<u32>) -> u8 {
        let red = xs.map(|x| pixels[((y * width + x) * 4) as usize]);
        let (lo, hi) = red.fold((u8::MAX, u8::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        hi - lo
    }

    #[test]
    fn a_glass_body_shows_the_desktop_beneath_it_blurred_and_nowhere_else() {
        for material in [Material::Frosted, Material::LiquidGlass] {
            let Some((pixels, width)) = render_stripes_under(material) else {
                return;
            };
            let outside = spread(&pixels, width, 64, 8..48);
            let inside = spread(&pixels, width, 64, 112..144);
            assert!(
                outside > 200,
                "{material:?}: the desktop stays sharp: {outside}"
            );
            assert!(
                inside < outside / 2,
                "{material:?}: the body shows a blur of the stripes: {inside} vs {outside}"
            );
            let centre = pixels[((64 * width + 128) * 4) as usize];
            assert!(
                (40..=215).contains(&centre),
                "{material:?}: the stripes average to grey under the body: {centre}"
            );
        }
    }
}
