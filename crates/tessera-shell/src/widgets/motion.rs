//! Motion curves, stagger choreography, and easing primitives for Tessera chrome.
//!
//! This module is the shared motion vocabulary every chrome component draws
//! from (ADR-0139, ADR-0172): closed-form analytic springs, decays, easing curves,
//! and the reduced-motion rule, powered by Optics' canonical `transit` motion library
//! (Optics ADR-0077, ADR-0106).
//!
//! It is pure math on caller-owned state — mechanism without a timeline. A component
//! decides *what* moves and *when* (policy); `transit` says *how a scalar travels
//! between two values* (mechanism). Nothing here schedules frames, owns a clock,
//! or draws: components keep driving `dt` from the frame input and retain their own
//! animation state.
//!
//! # Reduced motion
//!
//! The ADR-0029 rule is one global switch, not a per-effect one: when reduced motion
//! is on, every animation resolves to its end state in at most one frame. Every
//! `transit` advance primitive takes it as a final `reduced_motion: bool`, so callers
//! pass the flag straight through ([`approach`], [`decay`], [`blend`], [`Spring::advance`],
//! [`Smoother::step`], [`Hysteresis::step`]) instead of hand-writing a per-site snap.

#[allow(unused_imports)]
pub use transit::{
    approach, decay, dt_clamp, ease_in_cubic, ease_in_out_cubic, ease_out_back, ease_out_cubic,
    smoothstep, Hysteresis, Smoother, Spring, SpringParams,
};

/// One frame's delta time clamped to the range an animation may integrate
/// over. Delegates directly to [`transit::dt_clamp`].
#[inline]
pub fn frame_dt(dt_seconds: f32) -> f32 {
    transit::dt_clamp(dt_seconds)
}

/// The fraction of the remaining distance an exponential follower covers this
/// frame: `1 - e^(-rate·dt)`, frame-rate independent and clamped to `[0, 1]`.
///
/// This is exactly the coefficient [`approach`] applies internally, exposed for
/// callers that drive their own persistence — a rect that eases toward a target
/// through a `lerp`, a per-slot card blend — while still delegating the *math*
/// to Optics `transit` per `[INV-ARCH-49]`. Never re-derive this with a bare
/// `1.0 - (-rate * dt).exp()`: that duplicates mechanism the ADR places in
/// `transit` and loses the NaN/dt clamping the library guarantees. Under
/// reduced motion the coefficient is `1.0` (one-frame resolve).
#[inline]
pub fn blend(rate: f32, dt_seconds: f32, reduced_motion: bool) -> f32 {
    approach(0.0, 1.0, rate, dt_seconds, reduced_motion)
}

/// Calculate a staggered reveal progress: returns 0.0 until `reveal` exceeds `delay`,
/// then linearly interpolates to 1.0 as `reveal` reaches 1.0.
#[inline]
pub fn stagger(reveal: f32, delay: f32) -> f32 {
    if delay >= 1.0 {
        return if reveal >= 1.0 { 1.0 } else { 0.0 };
    }
    ((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ease_out_cubic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
        // Clamping checks
        assert_eq!(ease_out_cubic(-0.5), 0.0);
        assert_eq!(ease_out_cubic(1.5), 1.0);
    }

    #[test]
    fn test_smoothstep() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
        // Symmetric about the midpoint.
        assert!((smoothstep(0.2) + smoothstep(0.8) - 1.0).abs() < 1e-6);
        // Clamping and monotonicity.
        assert_eq!(smoothstep(-0.5), 0.0);
        assert_eq!(smoothstep(1.5), 1.0);
        assert!(smoothstep(0.3) < smoothstep(0.4));
    }

    #[test]
    fn test_stagger() {
        assert_eq!(stagger(0.0, 0.2), 0.0);
        assert_eq!(stagger(0.2, 0.2), 0.0);
        assert!((stagger(0.6, 0.2) - 0.5).abs() < 1e-6);
        assert_eq!(stagger(1.0, 0.2), 1.0);
    }

    #[test]
    fn test_frame_dt_clamps() {
        assert_eq!(frame_dt(0.0), 0.0);
        assert_eq!(frame_dt(-1.0), 0.0);
        assert!((frame_dt(1.0 / 60.0) - 1.0 / 60.0).abs() < 1e-6);
        assert_eq!(frame_dt(1.0), 1.0 / 30.0);
    }

    #[test]
    fn test_blend_matches_exponential_follow() {
        // Zero time covers no ground; a zero/negative rate snaps to the target.
        assert_eq!(blend(15.0, 0.0, false), 0.0);
        assert_eq!(blend(0.0, 1.0 / 60.0, false), 1.0);
        // The coefficient equals the closed-form exponential follow.
        let rate = 15.0f32;
        let dt = 1.0f32 / 60.0;
        let expected = 1.0 - (-rate * dt).exp();
        assert!((blend(rate, dt, false) - expected).abs() < 1e-6);
        assert!((0.0..=1.0).contains(&blend(rate, dt, false)));
        // Reduced motion resolves in a single frame.
        assert_eq!(blend(rate, dt, true), 1.0);
    }

    #[test]
    fn spring_no_time_elapses_nothing_moves() {
        let mut spring = Spring::new(10.0);
        let params = SpringParams::new(900.0, 0.85);
        assert_eq!(spring.advance(20.0, params, 0.0, false), 10.0);
        assert_eq!(spring.velocity, 0.0);
    }

    #[test]
    fn spring_settles_on_target() {
        let mut spring = Spring::new(10.0);
        let params = SpringParams::new(900.0, 0.85);
        for _ in 0..2000 {
            spring.advance(20.0, params, 1.0 / 120.0, false);
        }
        assert!(
            (spring.value - 20.0).abs() < 0.01,
            "settled at {}",
            spring.value
        );
        assert!(spring.settled(20.0, 0.15, 0.5));
    }

    #[test]
    fn spring_overshoots_then_settles() {
        // Under-damped from rest: crosses the target at least once before
        // settling (the macOS lift-and-bounce).
        let mut spring = Spring::new(0.0);
        let params = SpringParams::new(900.0, 0.85);
        let mut overshot = false;
        for _ in 0..2000 {
            spring.advance(100.0, params, 1.0 / 120.0, false);
            if spring.value > 100.0 {
                overshot = true;
            }
        }
        assert!(overshot, "spring never overshot the target");
        assert!((spring.value - 100.0).abs() < 0.01);
    }

    #[test]
    fn spring_critical_damping_has_no_overshoot() {
        let mut spring = Spring::new(0.0);
        let params = SpringParams::new(360.0, 1.0);
        let mut overshot = false;
        for _ in 0..2000 {
            spring.advance(100.0, params, 1.0 / 120.0, false);
            if spring.value > 100.0 + 1e-3 {
                overshot = true;
            }
        }
        assert!(!overshot, "critically damped spring overshot");
        assert!((spring.value - 100.0).abs() < 0.01);
    }

    #[test]
    fn spring_is_dt_stable() {
        // A single large step (a long frame stall) must not blow up.
        let mut spring = Spring::new(0.0);
        let params = SpringParams::new(900.0, 0.85);
        let value = spring.advance(100.0, params, 1.0 / 5.0, false);
        assert!(value.is_finite(), "value diverged: {value}");
        assert!(
            spring.velocity.is_finite(),
            "velocity diverged: {}",
            spring.velocity
        );
        // dt is clamped, so the value also stays inside a plausible band.
        assert!(value > 0.0 && value < 200.0, "value escaped range: {value}");
    }

    #[test]
    fn spring_remains_bounded_and_settles_at_thirty_fps() {
        let mut spring = Spring::new(56.0);
        let params = SpringParams::new(900.0, 0.85);
        for _ in 0..300 {
            spring.advance(84.0, params, 1.0 / 30.0, false);
            assert!(
                spring.value >= 0.0 && spring.value <= 84.0 * 2.0,
                "spring escaped its visual range: {}",
                spring.value
            );
        }
        assert!((spring.value - 84.0).abs() < 0.01);
        assert!(spring.velocity.abs() < 0.01);
    }

    #[test]
    fn spring_snap_to_resolves_in_one_frame() {
        let mut spring = Spring::new(0.3);
        assert_eq!(spring.snap_to(1.0), 1.0);
        assert_eq!(spring.value, 1.0);
        assert_eq!(spring.velocity, 0.0);
        assert!(spring.settled(1.0, 0.002, 0.02));
    }

    #[test]
    fn reduced_motion_flag_resolves_in_one_frame() {
        let mut spring = Spring::new(0.3);
        let params = SpringParams::new(900.0, 0.85);
        let res = spring.advance(1.0, params, 1.0 / 60.0, true);
        assert_eq!(res, 1.0);
        assert_eq!(spring.value, 1.0);
        assert_eq!(spring.velocity, 0.0);
    }
}
