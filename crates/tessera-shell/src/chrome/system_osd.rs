//! The transient system OSD: a local hardware state change, shown just long
//! enough to be noticed.
//!
//! This is deliberately not a notification. A notification is a message from the
//! outside world that the user may want to act on, so it is queued, readable,
//! and dismissible. An OSD is the machine answering a control the user just
//! touched — the volume they turned, the brightness they moved, the battery that
//! just crossed a level — and the correct response to a completed action is
//! acknowledgement, not a conversation.
//!
//! Three properties follow, and each one is a refusal:
//!
//! * **No pointer, no keyboard.** The OSD never captures input, so a reading that
//!   arrives mid-sentence cannot steal a keystroke. This is why the tiered battery
//!   policy exists: what must not be missed is the barrier
//!   ([`crate::BatteryAlert`]); everything softer is a glance.
//! * **No stack.** A newer reading replaces the current one. Two volume steps in a
//!   row are one volume reading, not two panels.
//! * **No history.** When it leaves, it leaves. Anything worth keeping belongs in
//!   the notification stream or the command panel, which are built for it.
//!
//! It anchors above the dock's edge — where the user's attention already is when
//! they reach for a hardware control — and holds no space and no focus.

use lens::{Align, Frame, Input, LayoutOpts, Rect};

use crate::component::{
    Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, Localizer, OsdParams, ellipsize,
};
use tessera_design::{Design, SurfaceRole, materials};
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;

/// How long an OSD stays on screen.
///
/// Long enough to read a two-word headline and a number, short enough that it is
/// gone before the user's next look. The critical tone does not extend it: a
/// longer bar is a bigger interruption, and the barrier exists for the case that
/// actually needs one.
const OSD_TTL_MS: u64 = 2_600;

/// The reading is held for a beat before the exit fade, so a burst of steps
/// reads as one settled value rather than a flicker.
const OSD_HOLD_MS: u64 = 1_900;

/// Exit fade length. Short: the surface is small and a slow fade draws the eye
/// back to a message that is already stale.
const OSD_FADE_MS: u64 = 700;

/// The OSD's anchoring inset from the output's bottom edge.
///
/// Deeper than [`SCREEN_CORNER_INSET`] because the surface sits above the dock
/// rather than at the corner: it clears the dock's reserved band at its tallest
/// (the panel plus its edge margin) without reading the dock's live geometry,
/// which would couple two components that must stay independent.
const OSD_BOTTOM_INSET: f32 = 108.0;

const OSD_W: f32 = 260.0;
const OSD_H: f32 = 68.0;
const OSD_PAD: f32 = 14.0;
const GAUGE_H: f32 = 6.0;

/// The OSD surface. Holds the current reading and its remaining life; carries no
/// history, because a reading that has left is not a reading any more.
pub struct SystemOsd {
    current: Option<Reading>,
    /// When the current reading was raised, on the compositor clock the host
    /// ticks each frame.
    raised_ms: u64,
    /// Last clock reading seen, so the component can age itself without asking
    /// the host for a second time source.
    now_ms: u64,
    design: Design,
}

/// One live reading.
struct Reading {
    params: OsdParams,
}

impl SystemOsd {
    #[must_use]
    pub fn new() -> SystemOsd {
        SystemOsd {
            current: None,
            raised_ms: 0,
            now_ms: 0,
            design: Design::dark(),
        }
    }

    /// Raise a reading, replacing whatever is on screen.
    pub fn show(&mut self, params: OsdParams) {
        self.current = Some(Reading { params });
        self.raised_ms = self.now_ms;
    }

    /// The current reading, if it is still inside its window.
    fn live(&self) -> Option<&Reading> {
        let reading = self.current.as_ref()?;
        let age = self.now_ms.saturating_sub(self.raised_ms);
        (age < OSD_TTL_MS).then_some(reading)
    }

    /// Surface opacity for the current age: solid through the hold, then a
    /// linear drain. Pure, so the curve is testable without a frame.
    fn opacity_at(age_ms: u64) -> f32 {
        if age_ms <= OSD_HOLD_MS {
            1.0
        } else if age_ms >= OSD_TTL_MS {
            0.0
        } else {
            let remaining = OSD_TTL_MS - age_ms;
            remaining as f32 / OSD_FADE_MS as f32
        }
    }

    /// The OSD's rect for a display: centred, above the dock's resting edge.
    fn rect_for(display: (f32, f32)) -> Rect {
        let w = OSD_W.min((display.0 - OSD_BOTTOM_INSET).max(120.0));
        Rect {
            x: ((display.0 - w) * 0.5).max(0.0),
            y: (display.1 - OSD_BOTTOM_INSET - OSD_H).max(crate::component::SCREEN_CORNER_INSET),
            w,
            h: OSD_H,
        }
    }
}

impl Default for SystemOsd {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for SystemOsd {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        self.now_ms = self
            .now_ms
            .saturating_add((raw.dt_seconds.max(0.0) * 1000.0) as u64);
        let Some(reading) = self.live() else {
            self.current = None;
            return;
        };
        let age = self.now_ms.saturating_sub(self.raised_ms);
        let opacity = Self::opacity_at(age);
        if opacity <= 0.001 {
            self.current = None;
            return;
        }

        let display = (raw.display_size.x, raw.display_size.y);
        let design = self.design;
        let rect = Self::rect_for(display);
        let params = &reading.params;

        frame.set_opacity(opacity);
        let mut body = materials::panel(&design);
        body.radius = design.radii.card;
        materials::place_floating(
            frame,
            "tessera-system-osd",
            SurfaceRole::Chip,
            &design,
            rect,
            opacity,
            body,
            |frame| {
                frame.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: 6.0,
                        pad: OSD_PAD,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        let inner_w = (rect.w - OSD_PAD * 2.0).max(0.0);
        let title = ellipsize(frame, &params.title, design.typography.label, inner_w);
        frame.place(
            "tessera-system-osd-title",
            &materials::chrome_place(
                Rect {
                    x: rect.x + OSD_PAD,
                    y: rect.y + OSD_PAD,
                    w: inner_w,
                    h: design.typography.label + 4.0,
                },
                materials::transparent(),
            ),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.set_theme(frame.theme().with_fg(design.colors.menu_text));
                        frame.label_compact_sized(&title, design.typography.label);
                    },
                );
            },
        );

        if let Some(detail) = params.detail.as_deref() {
            let detail = ellipsize(frame, detail, design.typography.footnote, inner_w);
            frame.place(
                "tessera-system-osd-detail",
                &materials::chrome_place(
                    Rect {
                        x: rect.x + OSD_PAD,
                        y: rect.y + OSD_PAD + design.typography.label + 4.0,
                        w: inner_w,
                        h: design.typography.footnote + 2.0,
                    },
                    materials::transparent(),
                ),
                |frame| {
                    frame.row_ex(
                        &LayoutOpts {
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.set_theme(frame.theme().with_fg(design.colors.menu_text_heading));
                            frame.label_compact_sized(&detail, design.typography.footnote);
                        },
                    );
                },
            );
        }

        // A level reading draws a gauge: the number alone makes the user convert
        // a figure into a feeling, the bar does it for them.
        if params.kind.is_level()
            && let Some(level) = params.level
        {
            let track = Rect {
                x: rect.x + OSD_PAD,
                y: rect.y + rect.h - OSD_PAD - GAUGE_H,
                w: inner_w,
                h: GAUGE_H,
            };
            let fill = (track.w * (f32::from(level.min(100)) / 100.0)).round();
            frame.place(
                "tessera-system-osd-gauge",
                &materials::chrome_place(
                    track,
                    LayoutOpts {
                        bg: design.colors.slider_track,
                        radius: GAUGE_H * 0.5,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |_| {},
            );
            frame.place(
                "tessera-system-osd-gauge-fill",
                &materials::chrome_place(
                    Rect { w: fill, ..track },
                    LayoutOpts {
                        bg: if params.critical {
                            design.colors.critical
                        } else {
                            design.colors.slider_fill
                        },
                        radius: GAUGE_H * 0.5,
                        pad: 0.0,
                        ..materials::surface_layout()
                    },
                ),
                |_| {},
            );
        }

        frame.set_opacity(1.0);
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        if let ChromeUpdate::Appearance(design) = update {
            self.design = *design;
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        if let ChromeCommand::ShowOsd(params) = command {
            self.show((**params).clone());
        }
    }

    fn osd_active(&self) -> bool {
        self.live().is_some()
    }

    fn requires_composition(&self) -> bool {
        self.live().is_some()
    }

    fn anim_pending(&self) -> bool {
        self.live().is_some()
    }

    /// The exact instant the reading crosses its window, so the host schedules
    /// one wake instead of pumping frames to discover it.
    fn animation_deadline_in(&self) -> Option<std::time::Duration> {
        let age = self.now_ms.saturating_sub(self.raised_ms);
        self.current.as_ref()?;
        (age < OSD_TTL_MS)
            .then(|| std::time::Duration::from_millis(OSD_TTL_MS.saturating_sub(age).max(1)))
    }

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        self.live()?;
        let rect = Self::rect_for(display);
        // The painted halo reaches past the body; repaint the whole envelope.
        let style = self.design.surfaces.for_role(SurfaceRole::Chip);
        let spread = style.shadow_blur;
        Some(vec![Rect {
            x: (rect.x - spread).max(0.0),
            y: (rect.y - spread + style.shadow_offset_y).max(0.0),
            w: rect.w + spread * 2.0,
            h: rect.h + spread * 2.0,
        }])
    }
}

#[cfg(test)]
mod tests;
