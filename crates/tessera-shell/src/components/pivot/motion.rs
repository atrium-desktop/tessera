//! Motion dynamics for Pivot (ADR-0172, ADR-0174).
//!
//! Owns the visibility transition scalar, driven via [`crate::widgets::motion::approach`].

const ANIMATION_SPEED: f32 = 22.0;

/// Motion dynamics state for Pivot progressive disclosure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PivotMotion {
    pub visibility: f32,
    pub anim_active: bool,
}

impl Default for PivotMotion {
    fn default() -> Self {
        Self::new()
    }
}

impl PivotMotion {
    /// Construct a new idle motion state at zero visibility.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            visibility: 0.0,
            anim_active: false,
        }
    }

    /// Advance visibility towards `target` given `dt` seconds and `reduced_motion` policy.
    pub fn advance(&mut self, target: f32, dt: f32, reduced_motion: bool) -> f32 {
        self.visibility = crate::widgets::motion::approach(
            self.visibility,
            target,
            ANIMATION_SPEED,
            dt,
            reduced_motion,
        );
        self.anim_active = (self.visibility - target).abs() > 0.002;
        if !self.anim_active {
            self.visibility = target;
        }
        self.visibility.clamp(0.0, 1.0)
    }

    /// Reset visibility immediately to zero.
    pub fn reset(&mut self) {
        self.visibility = 0.0;
        self.anim_active = false;
    }
}
