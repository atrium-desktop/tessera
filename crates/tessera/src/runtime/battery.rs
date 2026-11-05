//! Low-battery reporting, in two tiers.
//!
//! A configured `[battery] warn_at` threshold means "tell the user their charge
//! crossed this line". That is not the same as "stop what they are doing", and
//! treating it as though it were is how a status readout turns into an
//! interruption: the previous design raised a full-screen modal barrier on every
//! threshold, so a user typing mid-drop was stopped to be told something a glance
//! would have covered.
//!
//! The tiers are therefore:
//!
//! * **Every threshold above the lowest** raises the transient system OSD
//!   ([`tessera_shell::SystemOsd`]) — a reading in the corner for a couple of
//!   seconds that never takes the pointer or the keyboard.
//! * **The lowest threshold** raises the barrier ([`BatteryAlert`]), because that
//!   is the crossing after which the session actually ends.
//!
//! Once per threshold per discharge cycle, recovery rearm, and charge reset all
//! live in the tested latch (`tessera_desktop::system::BatteryWarningLatches`);
//! this module only gates and forwards.

use super::*;

impl CompositorRuntime<'_> {
    /// Evaluate the latest battery sample against the live `[battery]`
    /// thresholds and open the alert chrome on a fresh crossing.
    ///
    /// Skipped without latching while the session is locked or inactive, or
    /// while another modal owns the chrome layer (the same predicates the
    /// pick/prompt control paths enforce, plus the exclusive keyboard
    /// surfaces); the status cadence retries a skipped warning on the next
    /// sample. An already-open alert is not a gate: a deeper crossing
    /// escalates it in place to the critical wording.
    pub(super) fn poll_battery_warning(&mut self) {
        let Some(battery) = self.system_status.battery else {
            return;
        };
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        if self.pending_pick.is_some()
            || self.pending_app_pick.is_some()
            || self.pending_secret_prompt.is_some()
            || self.pending_confirm_pick.is_some()
            || self.pending_capability_pick.is_some()
            || self.shell.screenshot_active()
            || self.shell.control_center_active()
            || self.shell.window_switcher_active()
        {
            return;
        }
        let thresholds = self
            .config
            .as_ref()
            .map(|config| config.battery.warn_at.as_slice())
            .unwrap_or(&[]);
        let Some(threshold) =
            self.battery_latches
                .poll(battery.percent, battery.charging, thresholds)
        else {
            return;
        };
        // The lowest configured threshold is the one that ends the session.
        let critical = thresholds.iter().min().copied() == Some(threshold);
        if critical {
            self.shell
                .start_battery_alert(tessera_shell::component::BatteryAlertParams {
                    percent: battery.percent,
                    critical,
                });
        } else {
            self.shell.show_osd(tessera_shell::component::OsdParams {
                kind: tessera_shell::component::OsdKind::Battery,
                title: self
                    .shell
                    .localizer()
                    .osd_battery_low(battery.percent, false),
                detail: None,
                level: Some(battery.percent),
                critical: false,
            });
        }
        // Both surfaces are shell mutations outside the signed paths; without
        // damage the tick that carried an unchanged sample would present
        // nothing.
        self.damage.chrome_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use tessera_desktop::settings::BatterySettings;
    use tessera_desktop::system::BatteryWarningLatches;

    /// The tier a crossing belongs to. The compositor's `poll_battery_warning`
    /// reads the same two facts, so this pins the policy the user actually
    /// experiences: a glance for every threshold, a barrier only for the one
    /// that ends the session.
    fn tier_for(crossed: u8, thresholds: &[u8]) -> &'static str {
        if thresholds.iter().min().copied() == Some(crossed) {
            "barrier"
        } else {
            "osd"
        }
    }

    #[test]
    fn every_threshold_above_the_lowest_is_a_glance_not_a_barrier() {
        let thresholds = BatterySettings::default().warn_at;
        assert_eq!(thresholds, vec![20, 5], "the shipped policy is unchanged");
        assert_eq!(tier_for(20, &thresholds), "osd");
        assert_eq!(tier_for(5, &thresholds), "barrier");
    }

    #[test]
    fn a_three_tier_policy_puts_exactly_one_barrier_at_the_end() {
        let thresholds = vec![30, 15, 5];
        assert_eq!(tier_for(30, &thresholds), "osd");
        assert_eq!(tier_for(15, &thresholds), "osd");
        assert_eq!(tier_for(5, &thresholds), "barrier");
        // One barrier per configured policy, no matter how many thresholds.
        let barriers = thresholds
            .iter()
            .filter(|percent| tier_for(**percent, &thresholds) == "barrier")
            .count();
        assert_eq!(barriers, 1);
    }

    #[test]
    fn a_fast_drop_reports_one_reading_at_the_worst_crossing() {
        let thresholds = vec![20, 5];
        let mut latches = BatteryWarningLatches::default();
        // Dropping through both thresholds in one sample reports the lower one
        // once; the softer reading is not owed to the user afterwards.
        assert_eq!(latches.poll(3, false, &thresholds), Some(5));
        assert_eq!(latches.poll(3, false, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), None);
    }

    #[test]
    fn charging_rearms_the_next_discharge() {
        let thresholds = vec![20, 5];
        let mut latches = BatteryWarningLatches::default();
        assert_eq!(latches.poll(20, false, &thresholds), Some(20));
        assert_eq!(latches.poll(20, false, &thresholds), None);
        // Plugging in clears the latches, so the next discharge reports again.
        assert_eq!(latches.poll(90, true, &thresholds), None);
        assert_eq!(latches.poll(20, false, &thresholds), Some(20));
    }

    #[test]
    fn an_empty_threshold_list_reports_nothing() {
        let mut latches = BatteryWarningLatches::default();
        assert_eq!(latches.poll(1, false, &[]), None);
    }
}
