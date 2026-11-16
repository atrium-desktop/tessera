//! Overlay output-layer candidate policy.
//!
//! This module is a **pure evaluation**. It decides whether one scene surface
//! is a legitimate overlay candidate; it never imports a buffer, allocates a
//! plane, or submits an atomic request. The backend must still prove the
//! complete plane set with `TEST_ONLY` before anything bypasses composition
//! (ADR-0234, ADR-0235).
//!
//! The policy starts deliberately narrow: one topmost, single-surface,
//! opaque dma-buf window, with identity geometry and no chrome or software
//! cursor above it. Video and zero-copy client buffers are the expected first
//! users. Notifications, menus, shadows, wallpaper, always-on-top bands, and
//! chrome are never candidates.
//!
//! The composition root may evaluate this policy for audit even when overlay
//! presentation is disabled. Only `TESSERA_OVERLAY_OFFLOAD=1` promotes the
//! accepted plan into `Host::present_with_overlay`; evaluation never allocates
//! a plane by itself.

use tessera_desktop::window::WindowId;
use tessera_primitives::Rect;
use tessera_primitives::SurfaceDmabuf;

/// Stable rejection vocabulary for overlay audit telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlayRejectReason {
    SessionLocked,
    CapturePending,
    ScreenshotFreeze,
    SurfaceTransition,
    Overview,
    WindowSwitcher,
    NoCandidate,
    CandidateMismatch,
    NotTopmost,
    NotSingleSurface,
    ShmSurface,
    ClientOverlay,
    SceneFeedback,
    NotOpaque,
    NotIdentityMapped,
    ColorManaged,
    ChromeOverlap,
    SoftwareCursorOverlap,
    OutsideOutput,
    NoOverlayPlane,
    PlaneFormat,
    KmsRejected,
}

impl OverlayRejectReason {
    fn label(self) -> &'static str {
        match self {
            Self::SessionLocked => "session-locked",
            Self::CapturePending => "capture-pending",
            Self::ScreenshotFreeze => "screenshot-freeze",
            Self::SurfaceTransition => "surface-transition",
            Self::Overview => "overview",
            Self::WindowSwitcher => "window-switcher",
            Self::NoCandidate => "no-candidate",
            Self::CandidateMismatch => "candidate-mismatch",
            Self::NotTopmost => "not-topmost",
            Self::NotSingleSurface => "not-single-surface",
            Self::ShmSurface => "shm-surface",
            Self::ClientOverlay => "client-overlay",
            Self::SceneFeedback => "scene-feedback",
            Self::NotOpaque => "not-opaque",
            Self::NotIdentityMapped => "not-identity-mapped",
            Self::ColorManaged => "color-managed",
            Self::ChromeOverlap => "chrome-overlap",
            Self::SoftwareCursorOverlap => "software-cursor-overlap",
            Self::OutsideOutput => "outside-output",
            Self::NoOverlayPlane => "no-overlay-plane",
            Self::PlaneFormat => "plane-format",
            Self::KmsRejected => "kms-rejected",
        }
    }
}

impl std::fmt::Display for OverlayRejectReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Scene-level facts gathered by the composition root.
///
/// Counts are for the candidate window only; unrelated windows do not reject
/// overlay offload by themselves. The candidate still has to be topmost and
/// must not intersect chrome or a software cursor.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlaySceneFacts {
    pub session_locked: bool,
    pub capture_pending: bool,
    pub screenshot_freeze: bool,
    pub transition_pending: bool,
    pub overview_active: bool,
    pub window_switcher_active: bool,
    pub candidate_is_topmost: bool,
    pub dma_buf_surfaces_for_window: usize,
    pub shm_surfaces_for_window: usize,
    /// Protocol overlays (IME popups, drag icons, client cursor surfaces)
    /// are above every toplevel; any visible one disqualifies overlay offload.
    pub client_overlay_count: usize,
    pub chrome_overlaps_candidate: bool,
    pub software_cursor_overlaps_candidate: bool,
    /// Physical output rectangle in compositor space.
    pub output: Rect,
}

/// Facts about the candidate surface and its placement.
#[derive(Debug, Clone, Copy)]
pub struct OverlaySurfaceFacts {
    pub window: WindowId,
    /// The candidate must cover exactly this output-local rectangle.
    pub target: Rect,
    pub opaque: bool,
    /// The buffer maps 1:1 onto `target`: normal transform, no viewport crop,
    /// and buffer extent equal to the physical target extent.
    pub identity_mapped: bool,
    /// Whether the compositor paints feedback on/around this window (modal
    /// dimming, attention pulse, focus affordance). Such feedback lives in
    /// Primary and would be hidden by an overlay plane.
    pub scene_feedback: bool,
    /// Whether the surface needs a compositor color conversion that the plane
    /// cannot express. `false` is the only accepted value for now.
    pub color_managed: bool,
}

/// Facts about the overlay plane the backend could use.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayPlaneFacts {
    pub available: bool,
    pub format_supported: bool,
    pub modifier_supported: bool,
    /// `zpos` the backend would program, or `None` when the driver keeps its
    /// default plane order (primary below overlay, cursor above overlay).
    /// The backend validates the range; this pure policy does not guess at
    /// driver constraints it cannot observe.
    pub zpos: Option<u32>,
}

/// Complete overlay plan for one frame. Allocation is still the backend's
/// responsibility and must pass a complete-set `TEST_ONLY` request.
pub struct OverlayPlan {
    candidate_surface: Option<usize>,
    window: Option<WindowId>,
    target: Option<Rect>,
    zpos: Option<u32>,
    rejection: Option<OverlayRejection>,
}

impl OverlayPlan {
    fn candidate(
        candidate: &SurfaceDmabuf,
        surface: OverlaySurfaceFacts,
        plane: OverlayPlaneFacts,
    ) -> Self {
        Self {
            candidate_surface: Some(candidate.id),
            window: Some(surface.window),
            target: Some(surface.target),
            zpos: plane.zpos,
            rejection: None,
        }
    }

    fn composite(reason: OverlayRejectReason, plausible_candidate: bool) -> Self {
        Self {
            candidate_surface: None,
            window: None,
            target: None,
            zpos: None,
            rejection: Some(OverlayRejection {
                reason,
                plausible_candidate,
            }),
        }
    }

    /// Resource id of the candidate surface. The caller looks the live frame
    /// up in its own frame list; the plan never owns a client descriptor.
    pub fn candidate_surface_id(&self) -> Option<usize> {
        self.candidate_surface
    }

    pub fn window(&self) -> Option<WindowId> {
        self.window
    }

    pub fn target(&self) -> Option<Rect> {
        self.target
    }

    pub fn zpos(&self) -> Option<u32> {
        self.zpos
    }

    pub fn rejection(&self) -> Option<&OverlayRejection> {
        self.rejection.as_ref()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OverlayRejection {
    pub reason: OverlayRejectReason,
    pub plausible_candidate: bool,
}

/// The placement of one active overlay output layer. `surface_id` changes on
/// every client commit; the placement identity is everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlacement {
    pub window: WindowId,
    pub surface_id: usize,
    pub output: String,
    pub target: Rect,
    pub zpos: Option<u32>,
}

impl OverlayPlacement {
    pub fn from_plan(plan: &OverlayPlan, output: String, target: Rect) -> Option<OverlayPlacement> {
        if plan.rejection().is_some() {
            return None;
        }
        Some(Self {
            window: plan.window()?,
            surface_id: plan.candidate_surface_id()?,
            output,
            target,
            zpos: plan.zpos(),
        })
    }

    fn same_placement(&self, other: &Self) -> bool {
        self.window == other.window
            && self.output == other.output
            && self.target == other.target
            && self.zpos == other.zpos
    }
}

/// Pure transition result for the runtime overlay state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayChange {
    /// No active overlay and no candidate, or the candidate did not change.
    None,
    /// An overlay became active.
    Activated,
    /// Same placement, new buffer generation. Primary needs no repaint.
    Updated,
    /// Placement changed; the old and new Primary rectangles must repaint.
    Replaced,
    /// Overlay stopped being active; Primary must repaint.
    Deactivated,
}

impl OverlayChange {
    /// Whether this transition requires Primary damage. Buffer-only updates do
    /// not: their pixels belong to the overlay plane.
    pub fn requires_primary_repaint(self) -> bool {
        matches!(self, Self::Activated | Self::Replaced | Self::Deactivated)
    }
}

/// Runtime overlay state. All placement comparison and transition policy lives
/// here, not in the render/present orchestration.
#[derive(Debug, Default)]
pub struct OverlayState {
    active: Option<OverlayPlacement>,
}

impl OverlayState {
    pub fn active(&self) -> Option<&OverlayPlacement> {
        self.active.as_ref()
    }

    pub fn reconcile(&mut self, wanted: Option<OverlayPlacement>) -> OverlayChange {
        match (self.active.take(), wanted) {
            (None, None) => OverlayChange::None,
            (None, Some(wanted)) => {
                self.active = Some(wanted);
                OverlayChange::Activated
            }
            (Some(_), None) => OverlayChange::Deactivated,
            (Some(mut active), Some(wanted)) => {
                let change = if active.same_placement(&wanted) {
                    OverlayChange::Updated
                } else {
                    OverlayChange::Replaced
                };
                active.surface_id = wanted.surface_id;
                active.window = wanted.window;
                active.output = wanted.output;
                active.target = wanted.target;
                active.zpos = wanted.zpos;
                self.active = Some(active);
                change
            }
        }
    }

    /// Drop the active overlay without a candidate. Used when the candidate
    /// disappears between planning and presentation.
    pub fn deactivate(&mut self) -> OverlayChange {
        if self.active.take().is_some() {
            OverlayChange::Deactivated
        } else {
            OverlayChange::None
        }
    }
}

fn rect_is_positive(rect: Rect) -> bool {
    rect.size.w > 0 && rect.size.h > 0
}

fn rect_within(rect: Rect, output: Rect) -> bool {
    rect.origin.x >= output.origin.x
        && rect.origin.y >= output.origin.y
        && rect.origin.x.saturating_add(rect.size.w)
            <= output.origin.x.saturating_add(output.size.w)
        && rect.origin.y.saturating_add(rect.size.h)
            <= output.origin.y.saturating_add(output.size.h)
}

fn rects_intersect(first: Rect, second: Rect) -> bool {
    first.origin.x < second.origin.x.saturating_add(second.size.w)
        && second.origin.x < first.origin.x.saturating_add(first.size.w)
        && first.origin.y < second.origin.y.saturating_add(second.size.h)
        && second.origin.y < first.origin.y.saturating_add(first.size.h)
}

/// Evaluate the scene, the surface, and the plane in that order.
///
/// `candidate` is the single dma-buf surface the composition root selected for
/// `surface.window`. A missing candidate is a normal composite path, not an
/// error.
pub fn plan_overlay(
    scene: OverlaySceneFacts,
    surface: Option<OverlaySurfaceFacts>,
    candidate: Option<&SurfaceDmabuf>,
    plane: OverlayPlaneFacts,
) -> OverlayPlan {
    let plausible_candidate = surface.is_some_and(|surface| {
        scene.candidate_is_topmost
            && scene.dma_buf_surfaces_for_window == 1
            && scene.shm_surfaces_for_window == 0
            && surface.opaque
            && rect_is_positive(surface.target)
    });

    if scene.session_locked {
        return OverlayPlan::composite(OverlayRejectReason::SessionLocked, plausible_candidate);
    }
    if scene.capture_pending {
        return OverlayPlan::composite(OverlayRejectReason::CapturePending, plausible_candidate);
    }
    if scene.screenshot_freeze {
        return OverlayPlan::composite(OverlayRejectReason::ScreenshotFreeze, plausible_candidate);
    }
    if scene.transition_pending {
        return OverlayPlan::composite(OverlayRejectReason::SurfaceTransition, plausible_candidate);
    }
    if scene.overview_active {
        return OverlayPlan::composite(OverlayRejectReason::Overview, plausible_candidate);
    }
    if scene.window_switcher_active {
        return OverlayPlan::composite(OverlayRejectReason::WindowSwitcher, plausible_candidate);
    }
    let Some(surface) = surface else {
        return OverlayPlan::composite(OverlayRejectReason::NoCandidate, false);
    };
    let Some(candidate) = candidate else {
        return OverlayPlan::composite(OverlayRejectReason::NoCandidate, plausible_candidate);
    };
    if candidate.window != Some(surface.window) {
        return OverlayPlan::composite(OverlayRejectReason::CandidateMismatch, plausible_candidate);
    }
    if !scene.candidate_is_topmost {
        return OverlayPlan::composite(OverlayRejectReason::NotTopmost, plausible_candidate);
    }
    if scene.shm_surfaces_for_window != 0 {
        return OverlayPlan::composite(OverlayRejectReason::ShmSurface, plausible_candidate);
    }
    if scene.client_overlay_count != 0 {
        return OverlayPlan::composite(OverlayRejectReason::ClientOverlay, plausible_candidate);
    }
    if surface.scene_feedback {
        return OverlayPlan::composite(OverlayRejectReason::SceneFeedback, plausible_candidate);
    }
    if scene.dma_buf_surfaces_for_window != 1 {
        return OverlayPlan::composite(OverlayRejectReason::NotSingleSurface, plausible_candidate);
    }
    if !surface.opaque {
        return OverlayPlan::composite(OverlayRejectReason::NotOpaque, plausible_candidate);
    }
    if !surface.identity_mapped {
        return OverlayPlan::composite(OverlayRejectReason::NotIdentityMapped, plausible_candidate);
    }
    if surface.color_managed {
        return OverlayPlan::composite(OverlayRejectReason::ColorManaged, plausible_candidate);
    }
    if scene.chrome_overlaps_candidate {
        return OverlayPlan::composite(OverlayRejectReason::ChromeOverlap, plausible_candidate);
    }
    if scene.software_cursor_overlaps_candidate {
        return OverlayPlan::composite(
            OverlayRejectReason::SoftwareCursorOverlap,
            plausible_candidate,
        );
    }
    if !rect_is_positive(surface.target) || !rect_within(surface.target, scene.output) {
        return OverlayPlan::composite(OverlayRejectReason::OutsideOutput, plausible_candidate);
    }
    if !plane.available {
        return OverlayPlan::composite(OverlayRejectReason::NoOverlayPlane, plausible_candidate);
    }
    if !plane.format_supported || !plane.modifier_supported {
        return OverlayPlan::composite(OverlayRejectReason::PlaneFormat, plausible_candidate);
    }
    OverlayPlan::candidate(candidate, surface, plane)
}

/// Whether a candidate rectangle intersects any rectangle in `regions`.
pub fn intersects_any(target: Rect, regions: &[Rect]) -> bool {
    regions
        .iter()
        .copied()
        .any(|region| rects_intersect(target, region))
}

/// Primary damage required when the overlay plan changes between frames.
///
/// The old rectangle must be repainted because Primary takes its pixels back;
/// the new rectangle must be repainted before the overlay plane covers it.
/// Identical plans produce no Primary damage: overlay buffer updates belong to
/// the overlay plane alone.
pub fn overlay_transition_damage(previous: Option<Rect>, next: Option<Rect>) -> Vec<Rect> {
    match (previous, next) {
        (Some(previous), Some(next)) if previous == next => Vec::new(),
        (previous, next) => previous.into_iter().chain(next).collect(),
    }
}

const OVERLAY_TELEMETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Rate-limited audit counters for the overlay path.
///
/// Every evaluated candidate explains why it was rejected, whether overlay
/// presentation is disabled or enabled. That data is the evidence for tuning
/// the candidate class without turning the frame loop into a logger.
#[derive(Debug, Default)]
pub struct OverlayTelemetry {
    interval_counts: std::collections::BTreeMap<OverlayRejectReason, u64>,
    last_report: Option<std::time::Instant>,
    last_accepted_window: Option<WindowId>,
}

impl OverlayTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, host: &str, plan: &OverlayPlan) {
        let Some(rejection) = plan.rejection() else {
            if plan.window() != self.last_accepted_window {
                self.last_accepted_window = plan.window();
                log::debug!(
                    "{host}: overlay candidate accepted by policy: {:?} at {:?}",
                    plan.window(),
                    plan.target()
                );
            }
            self.interval_counts.clear();
            return;
        };
        self.record_rejection(host, rejection.reason, rejection.plausible_candidate);
    }

    pub fn record_rejection(
        &mut self,
        host: &str,
        reason: OverlayRejectReason,
        plausible_candidate: bool,
    ) {
        *self.interval_counts.entry(reason).or_default() += 1;
        let now = std::time::Instant::now();
        if self
            .last_report
            .is_some_and(|last| now.saturating_duration_since(last) < OVERLAY_TELEMETRY_INTERVAL)
        {
            return;
        }
        let mut summary = String::new();
        for (index, (reason, count)) in self.interval_counts.iter().enumerate() {
            use std::fmt::Write as _;
            if index != 0 {
                summary.push_str(", ");
            }
            let _ = write!(summary, "{}={count}", reason);
        }
        if plausible_candidate {
            log::info!(
                "{host}: overlay offload blocked ({reason}); recent rejection counts: {summary}"
            );
        } else {
            log::debug!(
                "{host}: overlay offload unavailable ({reason}); recent rejection counts: {summary}"
            );
        }
        self.interval_counts.clear();
        self.last_report = Some(now);
        self.last_accepted_window = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tessera_primitives::Point;
    use tessera_primitives::Size;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect::new(x, y, w, h)
    }

    fn surface() -> OverlaySurfaceFacts {
        OverlaySurfaceFacts {
            window: WindowId(7),
            target: rect(100, 100, 640, 480),
            opaque: true,
            identity_mapped: true,
            scene_feedback: false,
            color_managed: false,
        }
    }

    fn candidate() -> SurfaceDmabuf {
        SurfaceDmabuf {
            id: 11,
            window: Some(WindowId(7)),
            width: 640,
            height: 480,
            generation: 1,
            damage: Vec::new(),
            damage_full: false,
            buffer_id: 1,
            fd: -1,
            drm_format: tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888,
            modifier: 0,
            offset: 0,
            stride: 640 * 4,
            acquire_fence: -1,
            geometry: tessera_primitives::SurfaceGeometry::default(),
            opaque_region: None,
            color: None,
            presentation_hint: tessera_primitives::PresentationHint::Vsync,
        }
    }

    fn scene() -> OverlaySceneFacts {
        OverlaySceneFacts {
            candidate_is_topmost: true,
            dma_buf_surfaces_for_window: 1,
            output: Rect::new(0, 0, 1920, 1080),
            ..Default::default()
        }
    }

    fn plane() -> OverlayPlaneFacts {
        OverlayPlaneFacts {
            available: true,
            format_supported: true,
            modifier_supported: true,
            zpos: Some(2),
        }
    }

    #[test]
    fn topmost_opaque_single_surface_is_a_candidate() {
        let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), plane());
        assert!(plan.rejection().is_none());
        assert_eq!(plan.window(), Some(WindowId(7)));
        assert_eq!(plan.target(), Some(rect(100, 100, 640, 480)));
        assert_eq!(plan.zpos(), Some(2));
        assert_eq!(plan.candidate_surface_id(), Some(11));
    }

    #[test]
    fn candidate_must_belong_to_the_surface_window() {
        let mut buffer = candidate();
        buffer.window = Some(WindowId(99));
        let plan = plan_overlay(scene(), Some(surface()), Some(&buffer), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::CandidateMismatch)
        );
    }

    #[test]
    fn test_inv_arch_126_overlay_requires_one_topmost_single_surface() {
        let mut facts = scene();
        facts.candidate_is_topmost = false;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::NotTopmost)
        );

        let mut facts = scene();
        facts.dma_buf_surfaces_for_window = 2;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::NotSingleSurface)
        );

        let mut facts = scene();
        facts.shm_surfaces_for_window = 1;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::ShmSurface)
        );

        let mut facts = scene();
        facts.client_overlay_count = 1;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::ClientOverlay)
        );
    }

    #[test]
    fn test_inv_arch_129_overlay_rejects_translucent_scaled_or_color_managed() {
        let mut facts = surface();
        facts.opaque = false;
        let plan = plan_overlay(scene(), Some(facts), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::NotOpaque)
        );

        let mut facts = surface();
        facts.identity_mapped = false;
        let plan = plan_overlay(scene(), Some(facts), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::NotIdentityMapped)
        );

        let mut facts = surface();
        facts.scene_feedback = true;
        let plan = plan_overlay(scene(), Some(facts), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::SceneFeedback)
        );

        let mut facts = surface();
        facts.color_managed = true;
        let plan = plan_overlay(scene(), Some(facts), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::ColorManaged)
        );
    }

    #[test]
    fn test_inv_arch_127_overlay_rejects_chrome_and_software_cursor_overlap() {
        let mut facts = scene();
        facts.chrome_overlaps_candidate = true;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::ChromeOverlap)
        );

        let mut facts = scene();
        facts.software_cursor_overlaps_candidate = true;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::SoftwareCursorOverlap)
        );
    }

    #[test]
    fn plane_capability_failures_are_explicit() {
        let mut capabilities = plane();
        capabilities.available = false;
        let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), capabilities);
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::NoOverlayPlane)
        );

        let mut capabilities = plane();
        capabilities.format_supported = false;
        let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), capabilities);
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::PlaneFormat)
        );

        let mut capabilities = plane();
        capabilities.modifier_supported = false;
        let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), capabilities);
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::PlaneFormat)
        );
    }

    #[test]
    fn scene_gates_reject_before_candidate_selection() {
        let mut facts = scene();
        facts.session_locked = true;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::SessionLocked)
        );

        let mut facts = scene();
        facts.capture_pending = true;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::CapturePending)
        );
    }

    #[test]
    fn target_must_stay_inside_the_output() {
        let mut facts = surface();
        facts.target = rect(1800, 100, 640, 480);
        let plan = plan_overlay(scene(), Some(facts), Some(&candidate()), plane());
        assert_eq!(
            plan.rejection().map(|rejection| rejection.reason),
            Some(OverlayRejectReason::OutsideOutput)
        );
    }

    #[test]
    fn overlay_transition_damage_repaints_only_changed_rectangles() {
        let first = rect(10, 20, 100, 80);
        let second = rect(300, 40, 100, 80);
        assert!(overlay_transition_damage(Some(first), Some(first)).is_empty());
        assert_eq!(
            overlay_transition_damage(Some(first), Some(second)),
            vec![first, second]
        );
        assert_eq!(overlay_transition_damage(None, Some(first)), vec![first]);
        assert_eq!(overlay_transition_damage(Some(first), None), vec![first]);
        assert!(overlay_transition_damage(None, None).is_empty());
    }

    fn placement() -> OverlayPlacement {
        OverlayPlacement {
            window: WindowId(7),
            surface_id: 11,
            output: "DP-1".into(),
            target: rect(100, 100, 640, 480),
            zpos: Some(2),
        }
    }

    #[test]
    fn test_inv_arch_130_placement_identity_excludes_buffer_generation() {
        let first = placement();
        let mut state = OverlayState::default();
        assert_eq!(
            state.reconcile(Some(first.clone())),
            OverlayChange::Activated
        );

        let mut updated = first;
        updated.surface_id = 12;
        assert_eq!(state.reconcile(Some(updated)), OverlayChange::Updated);
        assert_eq!(state.active().map(|active| active.surface_id), Some(12));
    }

    #[test]
    fn test_inv_arch_131_buffer_only_updates_do_not_damage_primary() {
        let first = placement();
        let mut state = OverlayState::default();
        assert!(
            state
                .reconcile(Some(first.clone()))
                .requires_primary_repaint()
        );

        let mut updated = first;
        updated.surface_id = 12;
        let change = state.reconcile(Some(updated));
        assert_eq!(change, OverlayChange::Updated);
        assert!(!change.requires_primary_repaint());
    }

    #[test]
    fn test_inv_arch_132_placement_transitions_damage_primary() {
        let first = placement();
        let mut state = OverlayState::default();
        assert!(
            state
                .reconcile(Some(first.clone()))
                .requires_primary_repaint()
        );

        let mut moved = first;
        moved.target = rect(300, 40, 640, 480);
        assert!(state.reconcile(Some(moved)).requires_primary_repaint());

        assert!(state.reconcile(None).requires_primary_repaint());
    }

    #[test]
    fn test_inv_arch_133_rejected_plan_never_enters_overlay_state() {
        let mut facts = scene();
        facts.session_locked = true;
        let plan = plan_overlay(facts, Some(surface()), Some(&candidate()), plane());
        assert!(
            OverlayPlacement::from_plan(&plan, "DP-1".into(), rect(100, 100, 640, 480)).is_none()
        );

        let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), plane());
        let placement =
            OverlayPlacement::from_plan(&plan, "DP-1".into(), rect(100, 100, 640, 480)).unwrap();
        assert_eq!(placement.window, WindowId(7));
        assert_eq!(placement.surface_id, 11);
        assert_eq!(placement.output, "DP-1");
        assert_eq!(placement.zpos, Some(2));
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

        #[test]
        fn accepted_plan_satisfies_every_candidate_invariant(
            topmost in any::<bool>(),
            dma_count in 0usize..3,
            shm_count in 0usize..2,
            client_overlays in 0usize..2,
            opaque in any::<bool>(),
            identity in any::<bool>(),
            feedback in any::<bool>(),
            color_managed in any::<bool>(),
            chrome_overlap in any::<bool>(),
            cursor_overlap in any::<bool>(),
            plane_available in any::<bool>(),
            format_supported in any::<bool>(),
        ) {
            let scene = OverlaySceneFacts {
                candidate_is_topmost: topmost,
                dma_buf_surfaces_for_window: dma_count,
                shm_surfaces_for_window: shm_count,
                client_overlay_count: client_overlays,
                chrome_overlaps_candidate: chrome_overlap,
                software_cursor_overlaps_candidate: cursor_overlap,
                output: rect(0, 0, 1920, 1080),
                ..Default::default()
            };
            let surface = OverlaySurfaceFacts {
                window: WindowId(7),
                target: rect(100, 100, 640, 480),
                opaque,
                identity_mapped: identity,
                scene_feedback: feedback,
                color_managed,
            };
            let capabilities = OverlayPlaneFacts {
                available: plane_available,
                format_supported,
                modifier_supported: format_supported,
                zpos: None,
            };
            let plan = plan_overlay(scene, Some(surface), Some(&candidate()), capabilities);
            if plan.rejection().is_none() {
                prop_assert!(topmost);
                prop_assert_eq!(dma_count, 1);
                prop_assert_eq!(shm_count, 0);
                prop_assert_eq!(client_overlays, 0);
                prop_assert!(opaque);
                prop_assert!(identity);
                prop_assert!(!feedback);
                prop_assert!(!color_managed);
                prop_assert!(!chrome_overlap);
                prop_assert!(!cursor_overlap);
                prop_assert!(plane_available);
                prop_assert!(format_supported);
            }
        }
    }

    #[test]
    fn intersection_helper_ignores_disjoint_regions() {
        let target = Rect {
            origin: Point { x: 10, y: 10 },
            size: Size { w: 10, h: 10 },
        };
        assert!(!intersects_any(target, &[rect(0, 0, 5, 5)]));
        assert!(intersects_any(target, &[rect(15, 15, 10, 10)]));
    }
}
