//! Transient read-only Glance HUD (ADR-0255).
//!
//! Raised on demand by `Double Super`, a workspace switch, or its mounted Hot
//! Zone (top edge by default). Displays wall-clock time, workspace position,
//! and host-reported ambient machine status (Wi-Fi, Bluetooth, battery, audio
//! level/mute, Do Not Disturb) in a compact floating pill near the top centre,
//! then fades out cleanly with zero idle cost.
//!
//! Invariants enforced:
//! - `[INV-GLANCE-01]` Never captures pointer or keyboard input.
//! - `[INV-GLANCE-02]` Zero idle cost when dismissed (no damage, no frame clock).

use optics_ui::{Align, Frame, Input, LayoutOpts, Rect};
use tessera_design::{Design, SurfaceRole, materials};
use tessera_desktop::hot_zone::{HotZoneCapability, HotZoneTarget};
use tessera_desktop::system::{BluetoothLinkState, SystemStatus, WifiLinkState};
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;

use crate::clock::clock_strings;
use crate::component::{Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, Localizer, Message};

/// Total on-screen lifetime of one Glance HUD raise (ms).
const GLANCE_TTL_MS: u64 = 2_400;
/// Solid hold duration before the exit fade begins (ms).
const GLANCE_HOLD_MS: u64 = 1_800;
/// Exit fade duration (ms).
const GLANCE_FADE_MS: u64 = GLANCE_TTL_MS - GLANCE_HOLD_MS;
/// Top inset below the output top edge (logical px).
const GLANCE_TOP_INSET: f32 = 16.0;
/// Fixed pill height (logical px).
const GLANCE_HEIGHT: f32 = 40.0;
/// Horizontal padding inside the pill.
const GLANCE_PAD_X: f32 = 16.0;
/// Retirement drain frames after the HUD fades to zero (ADR-0227).
const RETIRE_DRAIN_FRAMES: u8 = 4;

/// Read-only transient Glance HUD component.
pub struct GlanceHud {
    active: bool,
    raised_ms: u64,
    now_ms: u64,
    retire_frames: u8,
    last_rect: Option<Rect>,
    system_status: SystemStatus,
    reduced_motion: bool,
    design: Design,
}

impl Default for GlanceHud {
    fn default() -> Self {
        Self::new()
    }
}

impl GlanceHud {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: false,
            raised_ms: 0,
            now_ms: 0,
            retire_frames: 0,
            last_rect: None,
            system_status: SystemStatus::default(),
            reduced_motion: false,
            design: Design::dark(),
        }
    }

    /// Raise (or refresh the hold timer of) the Glance HUD.
    pub fn show(&mut self) {
        self.active = true;
        self.raised_ms = self.now_ms;
        self.retire_frames = 0;
    }

    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.active && self.now_ms.saturating_sub(self.raised_ms) < GLANCE_TTL_MS
    }

    fn opacity_at(&self, age_ms: u64) -> f32 {
        if age_ms <= GLANCE_HOLD_MS {
            1.0
        } else if age_ms >= GLANCE_TTL_MS {
            0.0
        } else if self.reduced_motion {
            1.0
        } else {
            (GLANCE_TTL_MS - age_ms) as f32 / GLANCE_FADE_MS as f32
        }
    }

    /// Format the compact ambient readout line for the current snapshots.
    ///
    /// Every label goes through `i18n`: the HUD is user-facing chrome and
    /// ships in every catalog locale.
    fn format_summary(
        &self,
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
    ) -> (String, String, String) {
        let (time, _date) = clock_strings();

        let (current, total) = workspaces.outputs.first().map_or((1, 1), |output| {
            let total = output.workspaces.len().max(1);
            let current = output
                .current
                .and_then(|id| output.workspaces.iter().position(|ws| ws.id == id))
                .map_or(1, |i| i + 1);
            (current, total)
        });
        let ws_text = i18n.glance_workspace_position(current, total);

        let status = &self.system_status;
        let mut parts: Vec<String> = Vec::new();
        if let Some(wifi_enabled) = status.wifi_enabled {
            let label = match status.wifi_state {
                WifiLinkState::Connected => status
                    .wifi_ssid
                    .clone()
                    .unwrap_or_else(|| i18n.text(Message::Wifi).to_string()),
                WifiLinkState::Connecting | WifiLinkState::Scanning => {
                    format!("{}…", i18n.text(Message::Wifi))
                }
                WifiLinkState::Disconnected | WifiLinkState::Disabled if wifi_enabled => {
                    i18n.text(Message::Wifi).to_string()
                }
                WifiLinkState::Disconnected | WifiLinkState::Disabled => i18n.wireless_disabled(),
            };
            parts.push(label);
        }
        if let Some(bt_enabled) = status.bluetooth_enabled {
            let connected = status
                .bluetooth_devices
                .iter()
                .filter(|d| d.connected)
                .count();
            let label = match status.bluetooth_state {
                BluetoothLinkState::Connected if connected > 1 => {
                    format!("{} ·{connected}", i18n.text(Message::Bluetooth))
                }
                _ if bt_enabled => i18n.text(Message::Bluetooth).to_string(),
                _ => i18n.bluetooth_disabled(),
            };
            parts.push(label);
        }
        if let Some(bat) = &status.battery {
            parts.push(if bat.charging {
                i18n.charging_battery(bat.percent)
            } else {
                format!("{}%", bat.percent)
            });
        }
        if let Some(vol) = status.volume {
            parts.push(if status.muted {
                i18n.text(Message::Muted).to_string()
            } else {
                i18n.volume_level(vol)
            });
        }
        if status.do_not_disturb {
            parts.push(i18n.text(Message::DoNotDisturb).to_string());
        }

        (time, ws_text, parts.join("  ·  "))
    }

    fn pill_rect(display: (f32, f32), width: f32) -> Rect {
        let w = width.clamp(220.0, (display.0 - 32.0).max(220.0));
        Rect {
            x: ((display.0 - w) * 0.5).max(0.0),
            y: GLANCE_TOP_INSET,
            w,
            h: GLANCE_HEIGHT,
        }
    }
}

impl Chrome for GlanceHud {
    fn persistent_decoration(&self) -> bool {
        true
    }

    fn hot_zone_capability(&self) -> Option<HotZoneCapability> {
        Some(HotZoneTarget::GlanceHud.capability())
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        if matches!(command, ChromeCommand::ShowGlanceHud) {
            self.show();
        }
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::SystemStatus(status) => self.system_status = status.clone(),
            ChromeUpdate::ReducedMotion(reduced) => self.reduced_motion = reduced,
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn prepare_frame(
        &mut self,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
        let dt_ms = (input.as_raw().dt_seconds.max(0.0) * 1000.0) as u64;
        self.now_ms = self.now_ms.saturating_add(dt_ms);
        if self.active {
            let age = self.now_ms.saturating_sub(self.raised_ms);
            if age >= GLANCE_TTL_MS {
                self.active = false;
                self.retire_frames = RETIRE_DRAIN_FRAMES;
            }
        } else if self.retire_frames > 0 {
            self.retire_frames -= 1;
            if self.retire_frames == 0 {
                self.last_rect = None;
            }
        }
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        if !self.active {
            return;
        }
        let age = self.now_ms.saturating_sub(self.raised_ms);
        let opacity = self.opacity_at(age);
        if opacity <= 0.001 {
            return;
        }

        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let (time, ws_text, status_text) = self.format_summary(workspaces, i18n);
        let full_line = if status_text.is_empty() {
            format!("{time}   |   {ws_text}")
        } else {
            format!("{time}   |   {ws_text}   |   {status_text}")
        };

        let est_w = (full_line.chars().count() as f32 * 8.2 + GLANCE_PAD_X * 2.0 + 16.0)
            .clamp(220.0, (display.0 - 32.0).max(220.0));
        let rect = Self::pill_rect(display, est_w);
        self.last_rect = Some(rect);

        let design = self.design;
        frame.set_opacity(opacity);
        let mut body = materials::panel(&design);
        body.radius = GLANCE_HEIGHT * 0.5;
        materials::place_floating(
            frame,
            "tessera-glance-hud",
            SurfaceRole::Chip,
            &design,
            rect,
            opacity,
            body,
            |frame| {
                let original = frame.theme();
                frame.set_theme(original.with_fg(design.colors.menu_text));
                frame.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        pad: GLANCE_PAD_X,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.label_compact_sized(&full_line, design.typography.label);
                    },
                );
                frame.set_theme(original);
            },
        );
        frame.set_opacity(1.0);
    }

    fn anim_pending(&self) -> bool {
        if !self.active {
            return false;
        }
        let age = self.now_ms.saturating_sub(self.raised_ms);
        (GLANCE_HOLD_MS..GLANCE_TTL_MS).contains(&age)
    }

    fn damage_pending(&self) -> bool {
        self.retire_frames > 0
    }

    fn requires_composition(&self) -> bool {
        self.active || self.retire_frames > 0
    }

    fn animation_deadline_in(&self) -> Option<std::time::Duration> {
        if !self.active {
            return None;
        }
        let age = self.now_ms.saturating_sub(self.raised_ms);
        if age < GLANCE_HOLD_MS {
            Some(std::time::Duration::from_millis(GLANCE_HOLD_MS - age))
        } else {
            None
        }
    }

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        if !self.active && self.retire_frames == 0 {
            return Some(Vec::new());
        }
        let mut r = self
            .last_rect
            .unwrap_or_else(|| Self::pill_rect(display, 480.0));
        // Pad for elevation halo.
        let pad = 20.0;
        r.x = (r.x - pad).max(0.0);
        r.y = (r.y - pad).max(0.0);
        r.w = (r.w + pad * 2.0).min(display.0 - r.x);
        r.h = (r.h + pad * 2.0).min(display.1 - r.y);
        Some(vec![r])
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        false
    }

    fn captures_keyboard(&self) -> bool {
        false
    }

    fn osd_active(&self) -> bool {
        self.is_visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glance_hud_starts_idle_and_never_captures_input() {
        let hud = GlanceHud::new();
        assert!(!hud.is_visible());
        assert!(!hud.anim_pending());
        assert!(!hud.requires_composition());
        assert!(!hud.captures_keyboard());
        assert!(!hud.captures_pointer(
            960.0,
            24.0,
            (1920.0, 1080.0),
            &[],
            &WorkspaceSnapshot {
                outputs: Vec::new(),
            }
        ));
    }

    #[test]
    fn show_holds_then_fades_then_retires_cleanly() {
        let mut hud = GlanceHud::new();
        hud.show();
        assert!(hud.is_visible());
        assert!(hud.animation_deadline_in().is_some());
        assert!(!hud.anim_pending());

        // Advance into the fade window.
        hud.now_ms = GLANCE_HOLD_MS + 100;
        assert!(hud.anim_pending());
        assert!(hud.opacity_at(GLANCE_HOLD_MS + 100) < 1.0);

        // Advance past TTL.
        hud.now_ms = GLANCE_TTL_MS + 10;
        assert!(!hud.is_visible());
    }

    #[test]
    fn readout_is_localized_in_every_catalog_language() {
        use tessera_desktop::system::BatteryStatus;

        let mut hud = GlanceHud::new();
        hud.system_status.wifi_enabled = Some(false);
        hud.system_status.wifi_state = WifiLinkState::Disabled;
        hud.system_status.volume = Some(40);
        hud.system_status.muted = true;
        hud.system_status.do_not_disturb = true;
        hud.system_status.battery = Some(BatteryStatus {
            percent: 80,
            charging: true,
        });
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };

        let zh = Localizer::new("zh-CN");
        let (_, ws, status) = hud.format_summary(&workspaces, &zh);
        assert_eq!(ws, zh.glance_workspace_position(1, 1));
        assert!(status.contains(&zh.wireless_disabled()), "{status}");
        assert!(status.contains(&zh.charging_battery(80)), "{status}");
        assert!(status.contains(zh.text(Message::Muted)), "{status}");
        assert!(status.contains(zh.text(Message::DoNotDisturb)), "{status}");
        for english in ["Wi-Fi Off", "Muted", "DND", "WS "] {
            assert!(
                !status.contains(english) && !ws.contains(english),
                "{english} leaked: {ws} {status}"
            );
        }
    }
}
