//! Deferred effect outputs and the boundary that makes them sampleable.
//!
//! Optics applies a reusable effect (`flux::ShadowFilter`, `flux::BlurFilter`,
//! and the Prism materials) by *registering* a pass (Optics ADR-0147): the
//! output handle is valid the moment `apply` returns, but its pixels are not
//! written until something requires them. Two consequences reach this
//! workspace and they point in opposite directions:
//!
//! - Asking for an effect that nothing samples now costs nothing. The pass is
//!   pruned at submit and never touches the GPU.
//! - Sampling the output from inside a canvas pass is a contract violation and
//!   is *rejected*, because a render pass cannot contain the compute that would
//!   have to run first. Drawing a deferred output without materializing it
//!   fails the pass instead of compositing last frame's pixels or a blank
//!   image.
//!
//! The boundary rule is therefore stated once, here: a consumer that composites
//! an effect output through a canvas pass calls [`materialize_effect_output`]
//! at the pass boundary where it applied the filter, which requires the
//! producer and executes it immediately. Passes are executed at a boundary and
//! only at a boundary, so this call is legal exactly where `apply` was.

/// A borrowed effect output that may or may not have executed yet.
///
/// Implemented for the Optics filter outputs this workspace composites. The
/// trait exists so the materialization seam is one function rather than one
/// copy per effect kind; it deliberately exposes nothing but the raw image, so
/// no caller can use it to sample.
pub trait DeferredEffectOutput {
    /// Raw `flux_image` of the output. Borrowed from the filter's frame slot.
    fn raw_image(&self) -> *mut flux::sys::flux_image;
}

impl DeferredEffectOutput for flux::ShadowedImage<'_> {
    fn raw_image(&self) -> *mut flux::sys::flux_image {
        self.as_raw()
    }
}

impl DeferredEffectOutput for flux::BlurredImage<'_> {
    fn raw_image(&self) -> *mut flux::sys::flux_image {
        self.as_raw()
    }
}

/// Require every deferred pass that produces `output` and execute it, at the
/// caller's pass boundary (Optics ADR-0147).
///
/// Must be called with no canvas pass active. After it returns the image holds
/// this frame's pixels and may be drawn inside a pass; before it, drawing is
/// rejected with `FLUX_ERROR_INVALID_STATE`.
///
/// The cost of an unnecessary call is one `vkCmdExecuteCommands` of work that
/// was going to run anyway — it does not re-run a pass that already executed —
/// so the ordering cannot rot into a per-frame cost when the output was
/// consumed regardless. Skipping the call, by contrast, fails the draw.
pub fn materialize_effect_output(
    frame: &flux::Frame<'_>,
    output: &impl DeferredEffectOutput,
) -> Result<(), flux::Error> {
    let image = output.raw_image();
    if image.is_null() {
        return Err(flux::Error::from_raw(
            flux::sys::flux_result::FLUX_ERROR_INVALID_ARGUMENT,
        ));
    }
    // SAFETY: `image` is a live image on the frame's device — the caller holds
    // the filter borrow that keeps the slot's storage alive — and
    // `Frame::as_raw` yields the recording frame it was applied to.
    flux::Error::check_raw(unsafe {
        flux::sys::flux_frame_materialize_image(frame.as_raw(), image)
    })
}

/// Per-frame accounting of the deferred pass plan, or `None` when the frame
/// cannot report it.
///
/// `pruned` counts the passes nothing required: work an unconditional `apply`
/// asked for and no consumer sampled. Tessera's consumers still materialize
/// everything they composite, so a non-zero `pruned` normally means a filter
/// was driven for an output that never reached the screen, and `fallbacks`
/// means the plan was full and recorded eagerly instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanCounters {
    pub registered: u32,
    pub executed: u32,
    pub pruned: u32,
    pub fallbacks: u32,
}

/// Read the frame's deferred-pass counters. Cheap (no GPU work, no allocation).
pub fn plan_counters(frame: &flux::Frame<'_>) -> Option<PlanCounters> {
    let mut raw = flux::sys::flux_frame_plan_counters::default();
    // SAFETY: `raw` is a fully initialized counter block; the frame is live.
    let rc = unsafe { flux::sys::flux_frame_plan_stats(frame.as_raw(), &mut raw) };
    (rc == flux::sys::flux_result::FLUX_OK).then_some(PlanCounters {
        registered: raw.registered,
        executed: raw.executed,
        pruned: raw.pruned,
        fallbacks: raw.fallbacks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (64, 48);

    /// Fill `target` through a canvas pass, so the blur has real input.
    fn paint(
        canvas: &mut flux::Canvas,
        frame: &mut flux::Frame<'_>,
        target: &mut flux::Image,
        color: flux::sys::flux_color,
    ) {
        let session = canvas
            .begin_session(
                flux::ImageTarget {
                    frame,
                    image: target,
                },
                flux::CanvasPassOptions {
                    clear: Some(color),
                    ..Default::default()
                },
            )
            .unwrap();
        session.end().unwrap();
    }

    /// The counters report this frame's plan, not a running total.
    #[test]
    fn plan_counters_are_per_frame() {
        let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, SIZE.0, SIZE.1).unwrap();
        let frame = surface.begin_frame().unwrap();
        assert_eq!(plan_counters(&frame), Some(PlanCounters::default()));
    }

    /// ADR-0147's rejection: a canvas pass may not sample an effect output whose
    /// pass has not run. The draw is refused and the refusal names the remedy —
    /// so a consumer that skipped materialization sees a missing layer plus a
    /// diagnostic, never a blank or last-frame image composited as if it were
    /// this frame's.
    #[test]
    fn deferred_output_is_rejected_until_materialized() {
        let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, SIZE.0, SIZE.1).unwrap();
        let mut canvas = flux::Canvas::new(&surface).unwrap();
        let mut input =
            flux::Image::render_target(&device, SIZE.0, SIZE.1, surface.format()).unwrap();
        let mut blur = flux::BlurFilter::new(&device).unwrap();

        let mut frame = surface.begin_frame().unwrap();
        paint(
            &mut canvas,
            &mut frame,
            &mut input,
            flux::rgba(90, 140, 200, 255),
        );
        let blurred = blur.apply(&frame, &input, 2.0).unwrap();

        let session = canvas
            .begin_session(
                &mut frame,
                flux::CanvasPassOptions {
                    clear: Some(flux::rgba(0, 0, 0, 255)),
                    ..Default::default()
                },
            )
            .unwrap();
        blurred.draw(&session, 0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32);
        // The sample is refused at the draw, which records the diagnostic on
        // the thread-local slot. The pass itself still closes, because the
        // refusal must not corrupt a command stream the caller is mid-way
        // through; the observable consequence is the missing layer.
        session.end().unwrap();
        let info = flux::ErrorInfo::last().expect("the refusal must be observable");
        assert_eq!(info.code, flux::sys::flux_result::FLUX_ERROR_INVALID_STATE);
        let message = info.message.unwrap_or_default();
        assert!(
            message.contains("materialize"),
            "the diagnostic must name the remedy, not just the violation: {message}"
        );
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
        surface.read_pixels(&mut pixels).unwrap();
        // The refused draw contributed nothing: every pixel is the pass clear.
        // The output transform dithers ±1 LSB (Optics ADR-0069), so "black"
        // means within one quantum, not bit-identical.
        assert!(
            pixels
                .chunks_exact(4)
                .all(|pixel| pixel[..3].iter().all(|channel| *channel <= 1)),
            "an un-materialized output must contribute no pixels"
        );
    }

    /// The same draw succeeds after [`materialize_effect_output`], which executes
    /// the producer at the boundary that had to precede the sample.
    #[test]
    fn materialized_output_composites() {
        let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, SIZE.0, SIZE.1).unwrap();
        let mut canvas = flux::Canvas::new(&surface).unwrap();
        let mut input =
            flux::Image::render_target(&device, SIZE.0, SIZE.1, surface.format()).unwrap();
        let mut blur = flux::BlurFilter::new(&device).unwrap();

        let mut frame = surface.begin_frame().unwrap();
        paint(
            &mut canvas,
            &mut frame,
            &mut input,
            flux::rgba(220, 60, 30, 255),
        );
        let blurred = blur.apply(&frame, &input, 2.0).unwrap();
        materialize_effect_output(&frame, &blurred).unwrap();

        let session = canvas
            .begin_session(
                &mut frame,
                flux::CanvasPassOptions {
                    clear: Some(flux::rgba(0, 0, 0, 255)),
                    ..Default::default()
                },
            )
            .unwrap();
        blurred.draw(&session, 0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32);
        session.end().unwrap();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
        surface.read_pixels(&mut pixels).unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 20 || pixel[1] > 20 || pixel[2] > 20),
            "a materialized blur must reach the output"
        );
    }

    /// An effect nothing samples costs nothing: the pass is registered and then
    /// pruned at submit, and the counters are what make that observable rather
    /// than assumed.
    #[test]
    fn unsampled_effect_pass_is_pruned() {
        let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, SIZE.0, SIZE.1).unwrap();
        let mut canvas = flux::Canvas::new(&surface).unwrap();
        let mut input =
            flux::Image::render_target(&device, SIZE.0, SIZE.1, surface.format()).unwrap();
        let mut blur = flux::BlurFilter::new(&device).unwrap();

        let mut frame = surface.begin_frame().unwrap();
        paint(
            &mut canvas,
            &mut frame,
            &mut input,
            flux::rgba(10, 20, 30, 255),
        );
        let _unused = blur.apply(&frame, &input, 2.0).unwrap();

        let counters = plan_counters(&frame).expect("a recording frame reports its plan");
        assert_eq!(counters.executed, 0, "nothing required the blur");
        assert_eq!(counters.pruned, counters.registered);
        assert!(counters.registered >= 1);
        assert_eq!(counters.fallbacks, 0);
    }
}
