//! Physical keyboard accessibility filters: Bounce Keys, Slow Keys, and Sticky Keys.
//!
//! Enforced in accordance with ADR-0166 (Decoupled Native Accessibility Architecture
//! Across Compositor and Optics).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tessera_primitives::accessibility::KeyboardAccessibilityConfig;

/// EVDEV Linux keycodes for common modifier keys.
#[allow(dead_code)]
pub const KEY_LEFTSHIFT: u32 = 42;
#[allow(dead_code)]
pub const KEY_RIGHTSHIFT: u32 = 54;
#[allow(dead_code)]
pub const KEY_LEFTCTRL: u32 = 29;
#[allow(dead_code)]
pub const KEY_RIGHTCTRL: u32 = 97;
#[allow(dead_code)]
pub const KEY_LEFTALT: u32 = 56;
#[allow(dead_code)]
pub const KEY_RIGHTALT: u32 = 100;
#[allow(dead_code)]
pub const KEY_LEFTMETA: u32 = 125;
#[allow(dead_code)]
pub const KEY_RIGHTMETA: u32 = 126;

/// Filtering outcome for an incoming raw key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    /// Deliver key event immediately.
    Accept,
    /// Reject this key event (e.g. bounce key debounce or slow key premature release).
    Reject,
    /// Key press is delayed pending slow-keys hold duration.
    PendingSlowKey { evdev_code: u32, duration: Duration },
}

/// State tracking for sticky modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickyState {
    /// Modifier is released and inactive.
    Inactive,
    /// Modifier is latched for the next single non-modifier key press.
    Latched,
    /// Modifier is locked (double-tapped) until explicitly unlocked.
    Locked,
}

/// State machine tracking Bounce, Slow, and Sticky keys.
#[derive(Debug, Clone, Default)]
pub struct KeyboardAccessibilityFilter {
    config: KeyboardAccessibilityConfig,
    /// Last release timestamp per evdev code (for Bounce Keys).
    last_release: HashMap<u32, Instant>,
    /// Press timestamp per evdev code (for Slow Keys).
    press_time: HashMap<u32, Instant>,
    /// Sticky modifier states per evdev code.
    sticky_modifiers: HashMap<u32, StickyState>,
}

impl KeyboardAccessibilityFilter {
    /// Create a new filter with the specified configuration.
    #[must_use]
    pub fn new(config: KeyboardAccessibilityConfig) -> Self {
        Self {
            config,
            last_release: HashMap::new(),
            press_time: HashMap::new(),
            sticky_modifiers: HashMap::new(),
        }
    }

    /// Update filter configuration at runtime.
    pub fn set_config(&mut self, config: KeyboardAccessibilityConfig) {
        self.config = config;
        if !self.config.sticky_keys {
            self.sticky_modifiers.clear();
        }
    }

    /// Check if an evdev code corresponds to a modifier key.
    #[must_use]
    pub fn is_modifier(evdev_code: u32) -> bool {
        matches!(
            evdev_code,
            KEY_LEFTSHIFT
                | KEY_RIGHTSHIFT
                | KEY_LEFTCTRL
                | KEY_RIGHTCTRL
                | KEY_LEFTALT
                | KEY_RIGHTALT
                | KEY_LEFTMETA
                | KEY_RIGHTMETA
        )
    }

    /// Process a key press event.
    #[must_use]
    pub fn process_press(&mut self, evdev_code: u32, now: Instant) -> FilterOutcome {
        // 1. Bounce Keys: Reject rapid duplicate presses
        if self.config.bounce_keys_ms > 0 {
            if let Some(&last_rel) = self.last_release.get(&evdev_code) {
                let debounce = Duration::from_millis(u64::from(self.config.bounce_keys_ms));
                if now.duration_since(last_rel) < debounce {
                    return FilterOutcome::Reject;
                }
            }
        }

        // 2. Slow Keys: Track press time and delay
        if self.config.slow_keys_ms > 0 {
            self.press_time.insert(evdev_code, now);
            return FilterOutcome::PendingSlowKey {
                evdev_code,
                duration: Duration::from_millis(u64::from(self.config.slow_keys_ms)),
            };
        }

        FilterOutcome::Accept
    }

    /// Process a key release event.
    #[must_use]
    pub fn process_release(&mut self, evdev_code: u32, now: Instant) -> FilterOutcome {
        // 1. Slow Keys: If released before duration threshold, drop the key!
        if self.config.slow_keys_ms > 0 {
            if let Some(press) = self.press_time.remove(&evdev_code) {
                let threshold = Duration::from_millis(u64::from(self.config.slow_keys_ms));
                if now.duration_since(press) < threshold {
                    // Accidental tremor tap: discard!
                    return FilterOutcome::Reject;
                }
            }
        }

        // Record release timestamp for Bounce Keys debounce
        self.last_release.insert(evdev_code, now);

        // 2. Sticky Keys logic for modifiers
        if self.config.sticky_keys && Self::is_modifier(evdev_code) {
            let current = self
                .sticky_modifiers
                .get(&evdev_code)
                .copied()
                .unwrap_or(StickyState::Inactive);

            let next = match current {
                StickyState::Inactive => StickyState::Latched,
                StickyState::Latched => StickyState::Locked,
                StickyState::Locked => StickyState::Inactive,
            };

            self.sticky_modifiers.insert(evdev_code, next);
        }

        FilterOutcome::Accept
    }

    /// Query the sticky state of a modifier.
    #[must_use]
    pub fn sticky_state(&self, evdev_code: u32) -> StickyState {
        self.sticky_modifiers
            .get(&evdev_code)
            .copied()
            .unwrap_or(StickyState::Inactive)
    }

    /// Consume latched modifiers after delivering a non-modifier key event.
    pub fn consume_latched_modifiers(&mut self) {
        if !self.config.sticky_keys {
            return;
        }

        self.sticky_modifiers.retain(|_, state| match state {
            StickyState::Latched => false, // Unlatch after non-modifier press
            StickyState::Locked => true,   // Remain locked
            StickyState::Inactive => false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounce_keys_rejects_rapid_repeat_presses() {
        let mut filter = KeyboardAccessibilityFilter::new(KeyboardAccessibilityConfig {
            bounce_keys_ms: 100,
            ..Default::default()
        });

        let t0 = Instant::now();
        assert_eq!(filter.process_press(30, t0), FilterOutcome::Accept);
        assert_eq!(filter.process_release(30, t0), FilterOutcome::Accept);

        // Pressed again 30ms later -> Must be REJECTED (< 100ms)
        let t1 = t0 + Duration::from_millis(30);
        assert_eq!(filter.process_press(30, t1), FilterOutcome::Reject);

        // Pressed again 150ms later -> Must be ACCEPTED (>= 100ms)
        let t2 = t0 + Duration::from_millis(150);
        assert_eq!(filter.process_press(30, t2), FilterOutcome::Accept);
    }

    #[test]
    fn slow_keys_rejects_premature_releases() {
        let mut filter = KeyboardAccessibilityFilter::new(KeyboardAccessibilityConfig {
            slow_keys_ms: 300,
            ..Default::default()
        });

        let t0 = Instant::now();
        assert!(matches!(
            filter.process_press(30, t0),
            FilterOutcome::PendingSlowKey { .. }
        ));

        // Released 50ms later -> Must be REJECTED (< 300ms)
        let t1 = t0 + Duration::from_millis(50);
        assert_eq!(filter.process_release(30, t1), FilterOutcome::Reject);

        // Held for 350ms -> Release must be ACCEPTED (>= 300ms)
        let t2 = t0 + Duration::from_millis(500);
        let _ = filter.process_press(30, t2);
        let t3 = t2 + Duration::from_millis(350);
        assert_eq!(filter.process_release(30, t3), FilterOutcome::Accept);
    }

    #[test]
    fn sticky_keys_cycles_latched_and_locked() {
        let mut filter = KeyboardAccessibilityFilter::new(KeyboardAccessibilityConfig {
            sticky_keys: true,
            ..Default::default()
        });

        let t0 = Instant::now();

        // 1st Shift press & release: Latched
        let _ = filter.process_press(KEY_LEFTSHIFT, t0);
        let _ = filter.process_release(KEY_LEFTSHIFT, t0);
        assert_eq!(filter.sticky_state(KEY_LEFTSHIFT), StickyState::Latched);

        // Non-modifier key consumed: Latched is cleared
        filter.consume_latched_modifiers();
        assert_eq!(filter.sticky_state(KEY_LEFTSHIFT), StickyState::Inactive);

        // Double tap: Latched -> Locked
        let _ = filter.process_press(KEY_LEFTSHIFT, t0);
        let _ = filter.process_release(KEY_LEFTSHIFT, t0);
        assert_eq!(filter.sticky_state(KEY_LEFTSHIFT), StickyState::Latched);

        let _ = filter.process_press(KEY_LEFTSHIFT, t0);
        let _ = filter.process_release(KEY_LEFTSHIFT, t0);
        assert_eq!(filter.sticky_state(KEY_LEFTSHIFT), StickyState::Locked);

        // Non-modifier key consumed: Locked REMAINS Locked
        filter.consume_latched_modifiers();
        assert_eq!(filter.sticky_state(KEY_LEFTSHIFT), StickyState::Locked);

        // 3rd press: Unlocked to Inactive
        let _ = filter.process_press(KEY_LEFTSHIFT, t0);
        let _ = filter.process_release(KEY_LEFTSHIFT, t0);
        assert_eq!(filter.sticky_state(KEY_LEFTSHIFT), StickyState::Inactive);
    }
}
