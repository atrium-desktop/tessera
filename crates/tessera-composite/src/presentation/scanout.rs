//! Direct-scanout (primary-plane) policy: scene eligibility, candidate
//! surface evaluation, plane-ownership state, and rejection telemetry.
//! Everything here is a pure evaluation over `tessera-desktop` surface values
//! and declarative facts the composition root gathers from the live scene.

/// Content most recently committed successfully to the KMS primary plane.
///
/// This is presentation state, not an intent bit: a failed direct-scanout or
/// composited commit must leave it unchanged because the previous framebuffer
/// still owns the physical plane. Nested presentation uses the composited
/// variant as well, which keeps the runtime policy backend-independent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrimaryPlaneState {
    #[default]
    Unassigned,
    Composited,
    DirectScanout {
        surface_id: usize,
    },
}

impl PrimaryPlaneState {
    pub fn is_direct(self) -> bool {
        matches!(self, Self::DirectScanout { .. })
    }

    /// Forget framebuffer ownership when output power, target topology, or the
    /// backend device epoch disappears. No future transition may be inferred
    /// from a framebuffer that the kernel has already retired.
    pub fn invalidate(&mut self) {
        *self = Self::Unassigned;
    }

    /// Record a successful client-buffer commit and report whether this starts
    /// a new direct-scanout ownership session.
    pub fn commit_direct(&mut self, surface_id: usize) -> bool {
        let entered = !matches!(
            *self,
            Self::DirectScanout {
                surface_id: current
            } if current == surface_id
        );
        *self = Self::DirectScanout { surface_id };
        entered
    }

    /// Record a successful compositor-framebuffer commit and report whether
    /// it reclaimed the primary plane from a directly scanned-out client.
    pub fn commit_composited(&mut self) -> bool {
        let left_direct = self.is_direct();
        *self = Self::Composited;
        left_direct
    }
}

/// Complete primary-plane plan for the next frame. Exactly one field is set by
/// the private constructors. A flat representation avoids adding a heap
/// allocation to the per-frame direct candidate path merely to balance enum
/// variant sizes.
pub struct PrimaryPlanePlan {
    direct: Option<tessera_primitives::SurfaceDmabuf>,
    rejection: Option<ScanoutRejection>,
}

impl PrimaryPlanePlan {
    fn direct(candidate: tessera_primitives::SurfaceDmabuf) -> Self {
        Self {
            direct: Some(candidate),
            rejection: None,
        }
    }

    fn composite(rejection: ScanoutRejection) -> Self {
        Self {
            direct: None,
            rejection: Some(rejection),
        }
    }

    pub fn direct_candidate(&self) -> Option<&tessera_primitives::SurfaceDmabuf> {
        self.direct.as_ref()
    }

    pub fn rejection(&self) -> Option<&ScanoutRejection> {
        self.rejection.as_ref()
    }
}

/// Stable, machine-countable reasons why the primary-plane fast path was not
/// selected.  Keeping this policy vocabulary separate from rendering makes a
/// scanout miss observable and prevents future chrome from adding another
/// silent, ad-hoc `return None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScanoutRejectReason {
    SessionLocked,
    CapturePending,
    ScreenshotFreeze,
    SurfaceTransition,
    Overview,
    WindowSwitcher,
    MirrorPresent,
    VisibleShellPixels,
    ClientOverlay,
    SoftwareCursor,
    ShmClientSurface,
    NoDmabufCandidate,
    MultipleDmabufSurfaces,
    NotSingleToplevel,
    NonTrivialTransform,
    NonZeroOrigin,
    Viewport,
    GeometryMismatch,
    NotFullyOpaque,
    PlaneUnsupported,
    KmsRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanoutRejection {
    pub reason: ScanoutRejectReason,
    pub plausible_candidate: bool,
    /// The specific culprit behind a categorical reason, when the
    /// composition root knows one. Today this is the chrome component name
    /// for `visible-shell-pixels`.
    pub detail: Option<&'static str>,
}

impl ScanoutRejection {
    pub fn new(reason: ScanoutRejectReason, plausible_candidate: bool) -> Self {
        Self {
            reason,
            plausible_candidate,
            detail: None,
        }
    }
}

impl std::fmt::Display for ScanoutRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.detail {
            Some(detail) => write!(formatter, "{} ({detail})", self.reason),
            None => write!(formatter, "{}", self.reason),
        }
    }
}

impl ScanoutRejectReason {
    fn label(self) -> &'static str {
        match self {
            Self::SessionLocked => "session-locked",
            Self::CapturePending => "capture-pending",
            Self::ScreenshotFreeze => "screenshot-freeze",
            Self::SurfaceTransition => "surface-transition",
            Self::Overview => "overview",
            Self::WindowSwitcher => "window-switcher",
            Self::MirrorPresent => "mirror-present",
            Self::VisibleShellPixels => "visible-shell-pixels",
            Self::ClientOverlay => "client-overlay",
            Self::SoftwareCursor => "software-cursor",
            Self::ShmClientSurface => "shm-client-surface",
            Self::NoDmabufCandidate => "no-dmabuf-candidate",
            Self::MultipleDmabufSurfaces => "multiple-dmabuf-surfaces",
            Self::NotSingleToplevel => "not-single-toplevel",
            Self::NonTrivialTransform => "non-trivial-transform",
            Self::NonZeroOrigin => "non-zero-origin",
            Self::Viewport => "viewport",
            Self::GeometryMismatch => "geometry-mismatch",
            Self::NotFullyOpaque => "not-fully-opaque",
            Self::PlaneUnsupported => "plane-unsupported",
            Self::KmsRejected => "kms-rejected",
        }
    }
}

impl std::fmt::Display for ScanoutRejectReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Facts about scene ownership that are independent of any particular client
/// buffer.  They are deliberately semantic: only pixels/effects that would
/// actually reach the next output frame reject scanout. Shell visibility is
/// carried as two plain flags so this evaluation never depends on the
/// chrome tier; the composition root flattens its composition requirements
/// into them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanoutSceneFacts {
    pub session_locked: bool,
    /// A mirrored physical output shares this scene. Direct scanout would
    /// replace the shared framebuffer on the source CRTC only, leaving the
    /// mirror on a stale composite.
    pub mirror_present: bool,
    pub capture_pending: bool,
    pub screenshot_freeze: bool,
    pub transition_pending: bool,
    pub overview_active: bool,
    pub window_switcher_active: bool,
    pub shell_visible_pixels: bool,
    /// The first chrome component behind `shell_visible_pixels`, named in the
    /// rejection so the log points at the culprit.
    pub shell_blocker: Option<&'static str>,
    pub client_overlay_count: usize,
    pub software_cursor_visible: bool,
    pub shm_surface_count: usize,
    pub dmabuf_surface_count: usize,
    pub toplevel_dmabuf_count: usize,
    pub candidate_matches_scene: bool,
}

impl ScanoutSceneFacts {
    fn plausible_candidate(self) -> bool {
        self.shm_surface_count == 0
            && self.dmabuf_surface_count == 1
            && self.toplevel_dmabuf_count == 1
            && self.candidate_matches_scene
    }
}

pub fn evaluate_scene(facts: ScanoutSceneFacts) -> Result<(), ScanoutRejectReason> {
    if facts.session_locked {
        return Err(ScanoutRejectReason::SessionLocked);
    }
    if facts.mirror_present {
        return Err(ScanoutRejectReason::MirrorPresent);
    }
    if facts.capture_pending {
        return Err(ScanoutRejectReason::CapturePending);
    }
    if facts.screenshot_freeze {
        return Err(ScanoutRejectReason::ScreenshotFreeze);
    }
    if facts.transition_pending {
        return Err(ScanoutRejectReason::SurfaceTransition);
    }
    if facts.overview_active {
        return Err(ScanoutRejectReason::Overview);
    }
    if facts.window_switcher_active {
        return Err(ScanoutRejectReason::WindowSwitcher);
    }
    if facts.shell_visible_pixels {
        return Err(ScanoutRejectReason::VisibleShellPixels);
    }
    if facts.client_overlay_count != 0 {
        return Err(ScanoutRejectReason::ClientOverlay);
    }
    if facts.software_cursor_visible {
        return Err(ScanoutRejectReason::SoftwareCursor);
    }
    if facts.shm_surface_count != 0 {
        return Err(ScanoutRejectReason::ShmClientSurface);
    }
    match facts.dmabuf_surface_count {
        0 => return Err(ScanoutRejectReason::NoDmabufCandidate),
        1 => {}
        _ => return Err(ScanoutRejectReason::MultipleDmabufSurfaces),
    }
    if facts.toplevel_dmabuf_count != 1 || !facts.candidate_matches_scene {
        return Err(ScanoutRejectReason::NotSingleToplevel);
    }
    Ok(())
}

/// Whether nothing behind this surface can show through it. An opaque buffer
/// format qualifies by construction; an alpha format needs a declared opaque
/// region covering the whole presented extent. Delegated to the
/// presentation-domain value type (ADR-0183).
fn surface_is_fully_opaque(surface: &tessera_primitives::SurfaceDmabuf) -> bool {
    tessera_desktop::presentation::SurfacePresentation::from_dmabuf(surface).is_opaque()
}

pub fn evaluate_surface(
    surface: &tessera_primitives::SurfaceDmabuf,
    physical_size: (u32, u32),
    logical_size: (u32, u32),
    plane_supported: bool,
) -> Result<(), ScanoutRejectReason> {
    if surface.geometry.transform != tessera_primitives::Transform::Normal {
        return Err(ScanoutRejectReason::NonTrivialTransform);
    }
    if surface.geometry.position != (tessera_primitives::Point { x: 0, y: 0 }) {
        return Err(ScanoutRejectReason::NonZeroOrigin);
    }
    // A `wp_viewport` whose source covers the complete physical buffer at
    // (0, 0) and whose destination equals the complete logical output (the
    // canonical `wp_fractional_scale_v1` + `wp_viewporter` shape) is a 1:1
    // physical-buffer-to-plane identity mapping and requires no GPU scaling or
    // cropping. Sub-buffer crops and non-output destination sizes still route
    // through the composite path.
    if let Some(src) = surface.geometry.viewport_src {
        let scale = surface.geometry.buffer_scale.max(1);
        let covers_full_buffer = src.origin == (tessera_primitives::Point { x: 0, y: 0 })
            && src.size.w.saturating_mul(scale) == surface.width
            && src.size.h.saturating_mul(scale) == surface.height;
        if !covers_full_buffer {
            return Err(ScanoutRejectReason::Viewport);
        }
    }
    if let Some(dst) = surface.geometry.viewport_dst
        && (dst.w <= 0 || dst.h <= 0 || (dst.w as u32, dst.h as u32) != logical_size)
    {
        return Err(ScanoutRejectReason::Viewport);
    }
    if surface.width <= 0
        || surface.height <= 0
        || (surface.width as u32, surface.height as u32) != physical_size
    {
        return Err(ScanoutRejectReason::GeometryMismatch);
    }
    if !surface_is_fully_opaque(surface) {
        return Err(ScanoutRejectReason::NotFullyOpaque);
    }
    if !plane_supported {
        return Err(ScanoutRejectReason::PlaneUnsupported);
    }
    Ok(())
}

/// Evaluate the gathered scene facts and the scene's single toplevel
/// dma-buf candidate (`None` when the scene has none or it does not match
/// the scene) into a complete primary-plane plan.
pub fn plan_scanout(
    facts: ScanoutSceneFacts,
    physical_size: (u32, u32),
    logical_size: (u32, u32),
    candidate: Option<tessera_primitives::SurfaceDmabuf>,
    plane_supported: bool,
) -> PrimaryPlanePlan {
    let plausible_candidate = facts.plausible_candidate();
    let reject = |reason: ScanoutRejectReason| {
        let detail = match reason {
            ScanoutRejectReason::VisibleShellPixels => facts.shell_blocker,
            _ => None,
        };
        PrimaryPlanePlan::composite(ScanoutRejection {
            reason,
            plausible_candidate,
            detail,
        })
    };
    if let Err(reason) = evaluate_scene(facts) {
        return reject(reason);
    }
    let Some(candidate) = candidate else {
        return reject(ScanoutRejectReason::NoDmabufCandidate);
    };
    if let Err(reason) = evaluate_surface(&candidate, physical_size, logical_size, plane_supported)
    {
        return reject(reason);
    }
    PrimaryPlanePlan::direct(candidate)
}

/// Whether an overlay output layer may be evaluated for this primary plan.
///
/// Direct primary scanout owns the complete output and has no compositor
/// framebuffer underneath an overlay, so the two bypass paths are mutually
/// exclusive (ADR-0237). This is the single gate the composition root uses;
/// evaluating overlay against an accepted direct plan would also make the same
/// client surface eligible twice.
pub fn overlay_plan_permitted(primary: &PrimaryPlanePlan) -> bool {
    primary.rejection().is_some()
}

/// How often the rejection-count summary is written (debug level).
const SCANOUT_SUMMARY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a plausible full-output candidate may stay composited for one
/// unchanged reason before a single warning is written. A user who keeps the
/// window switcher or control center open over a game that long is rare.
/// Chrome that never stands down is a defect, and this line names it.
pub const SCANOUT_SUSTAINED_BLOCK_WARNING: std::time::Duration = std::time::Duration::from_secs(30);

/// What a telemetry call wrote to the log. Returned so the policy is testable
/// without a logger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanoutLogEvent {
    /// A plausible candidate became blocked, or its blocker changed (info).
    Blocked(ScanoutRejection),
    /// A plausible candidate stayed blocked for one reason past
    /// [`SCANOUT_SUSTAINED_BLOCK_WARNING`] (warn, once per episode).
    Sustained(ScanoutRejection),
    /// A blocked candidate reached the primary plane (debug; the runtime logs
    /// `direct scanout active` at info on the same frame).
    Resumed,
    /// The periodic rejection-count summary (debug).
    Summary,
}

/// One continuous stretch in which a plausible candidate stayed composited
/// for the same rejection.
#[derive(Debug, Clone, Copy)]
struct BlockEpisode {
    rejection: ScanoutRejection,
    since: std::time::Instant,
    frames: u64,
    warned: bool,
}

/// Monotonic frame counters since compositor start, written with every
/// summary. They answer "how much of this session was direct?" from the
/// journal alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanoutTotals {
    /// Frames presented by direct client scanout.
    pub direct_frames: u64,
    /// Composited frames although a plausible full-output candidate existed.
    pub blocked_candidate_frames: u64,
    /// Composited frames of an ordinary desktop with no candidate.
    pub desktop_frames: u64,
}

/// Direct-scanout observability with a bounded log rate.
///
/// The log is driven by state changes, not frames:
/// - `info`: a plausible full-output candidate becomes blocked or changes
///   blocker (`direct scanout blocked: visible-shell-pixels (Pivot)`). The
///   runtime adds `direct scanout active` when the plane is acquired. A steady
///   game or video writes nothing.
/// - `warn`: once per episode, when one blocker has held a plausible candidate
///   for [`SCANOUT_SUSTAINED_BLOCK_WARNING`]. This line catches chrome that
///   never stands down.
/// - `debug`: rejection counts and [`ScanoutTotals`] every
///   [`SCANOUT_SUMMARY_INTERVAL`], for `RUST_LOG=tessera_composite=debug`.
pub struct ScanoutTelemetry {
    interval_counts: std::collections::BTreeMap<ScanoutRejectReason, u64>,
    last_summary: Option<std::time::Instant>,
    episode: Option<BlockEpisode>,
    totals: ScanoutTotals,
}

impl Default for ScanoutTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanoutTelemetry {
    pub fn new() -> Self {
        Self {
            interval_counts: std::collections::BTreeMap::new(),
            last_summary: None,
            episode: None,
            totals: ScanoutTotals::default(),
        }
    }

    pub fn totals(&self) -> ScanoutTotals {
        self.totals
    }

    /// The rejection currently holding a plausible candidate off the primary
    /// plane, if any.
    pub fn current_block(&self) -> Option<ScanoutRejection> {
        self.episode.map(|episode| episode.rejection)
    }

    pub fn record_rejection(&mut self, host: &str, rejection: ScanoutRejection) {
        for event in self.observe_rejection(rejection, std::time::Instant::now()) {
            self.emit(host, event);
        }
    }

    pub fn record_success(&mut self, host: &str) {
        if let Some(event) = self.observe_success(std::time::Instant::now()) {
            self.emit(host, event);
        }
    }

    /// Pure state transition for one composited frame. Returns the log events
    /// it implies, in order.
    pub fn observe_rejection(
        &mut self,
        rejection: ScanoutRejection,
        now: std::time::Instant,
    ) -> Vec<ScanoutLogEvent> {
        let mut events = Vec::new();
        *self.interval_counts.entry(rejection.reason).or_default() += 1;
        if rejection.plausible_candidate {
            self.totals.blocked_candidate_frames += 1;
            match &mut self.episode {
                Some(episode) if episode.rejection == rejection => {
                    episode.frames += 1;
                    if !episode.warned
                        && now.saturating_duration_since(episode.since)
                            >= SCANOUT_SUSTAINED_BLOCK_WARNING
                    {
                        episode.warned = true;
                        events.push(ScanoutLogEvent::Sustained(rejection));
                    }
                }
                _ => {
                    self.episode = Some(BlockEpisode {
                        rejection,
                        since: now,
                        frames: 1,
                        warned: false,
                    });
                    events.push(ScanoutLogEvent::Blocked(rejection));
                }
            }
        } else {
            // No full-output candidate: an ordinary desktop. Any episode
            // ends quietly; the user left fullscreen rather than resuming it.
            self.totals.desktop_frames += 1;
            self.episode = None;
        }
        if self
            .last_summary
            .is_none_or(|last| now.saturating_duration_since(last) >= SCANOUT_SUMMARY_INTERVAL)
        {
            self.last_summary = Some(now);
            events.push(ScanoutLogEvent::Summary);
        }
        events
    }

    /// Pure state transition for one directly scanned-out frame.
    pub fn observe_success(&mut self, _now: std::time::Instant) -> Option<ScanoutLogEvent> {
        self.totals.direct_frames += 1;
        self.interval_counts.clear();
        self.episode.take().map(|_| ScanoutLogEvent::Resumed)
    }

    fn emit(&mut self, host: &str, event: ScanoutLogEvent) {
        match event {
            ScanoutLogEvent::Blocked(rejection) => {
                log::info!("{host}: direct scanout blocked: {rejection}");
            }
            ScanoutLogEvent::Sustained(rejection) => {
                let frames = self.episode.map_or(0, |episode| episode.frames);
                log::warn!(
                    "{host}: direct scanout blocked for over {}s by {rejection} ({frames} \
                     full-output frames composited); if nothing visible explains it, \
                     the blocker is not standing down",
                    SCANOUT_SUSTAINED_BLOCK_WARNING.as_secs()
                );
            }
            ScanoutLogEvent::Resumed => {
                // The runtime writes the info-level `direct scanout active`
                // line with the buffer format on the same transition.
                log::debug!("{host}: direct scanout resumed; block episode closed");
            }
            ScanoutLogEvent::Summary => {
                if !log::log_enabled!(log::Level::Debug) {
                    self.interval_counts.clear();
                    return;
                }
                let mut summary = String::new();
                for (index, (reason, count)) in self.interval_counts.iter().enumerate() {
                    use std::fmt::Write as _;
                    if index != 0 {
                        summary.push_str(", ");
                    }
                    let _ = write!(summary, "{}={count}", reason.label());
                }
                let ScanoutTotals {
                    direct_frames,
                    blocked_candidate_frames,
                    desktop_frames,
                } = self.totals;
                let current = self.episode.map_or_else(
                    || "none".to_owned(),
                    |episode| episode.rejection.to_string(),
                );
                log::debug!(
                    "{host}: scanout summary: current block: {current}; recent rejections: \
                     {summary}; totals: direct={direct_frames} \
                     blocked-candidate={blocked_candidate_frames} desktop={desktop_frames}"
                );
                self.interval_counts.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(format: u32) -> tessera_primitives::SurfaceDmabuf {
        tessera_primitives::SurfaceDmabuf {
            id: 7,
            window: Some(tessera_desktop::window::WindowId(9)),
            width: 3072,
            height: 1920,
            generation: 1,
            damage: Vec::new(),
            damage_full: false,
            buffer_id: 1,
            fd: -1,
            drm_format: format,
            modifier: 0,
            offset: 0,
            stride: 3072 * 4,
            acquire_fence: -1,
            geometry: tessera_primitives::SurfaceGeometry {
                buffer_scale: 2,
                ..Default::default()
            },
            opaque_region: None,
            color: None,
            presentation_hint: tessera_primitives::PresentationHint::Vsync,
        }
    }

    fn eligible_scene() -> ScanoutSceneFacts {
        ScanoutSceneFacts {
            dmabuf_surface_count: 1,
            toplevel_dmabuf_count: 1,
            candidate_matches_scene: true,
            ..Default::default()
        }
    }

    #[test]
    fn maximized_xrgb_geometry_is_scanout_eligible_without_fullscreen_state() {
        let surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888);
        // SurfaceDmabuf contains no xdg fullscreen flag: actual output
        // placement, extent, opacity, and plane support are the entire policy.
        assert_eq!(evaluate_scene(eligible_scene()), Ok(()));
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Ok(())
        );
    }

    #[test]
    fn mirrored_output_prevents_direct_scanout() {
        let facts = ScanoutSceneFacts {
            mirror_present: true,
            ..eligible_scene()
        };
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::MirrorPresent)
        );
    }

    #[test]
    fn test_inv_arch_134_direct_primary_excludes_overlay() {
        let direct = plan_scanout(
            eligible_scene(),
            (3072, 1920),
            (1536, 960),
            Some(candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888)),
            true,
        );
        assert!(direct.direct_candidate().is_some());
        assert!(!overlay_plan_permitted(&direct));

        let composited = plan_scanout(
            ScanoutSceneFacts {
                client_overlay_count: 1,
                ..eligible_scene()
            },
            (3072, 1920),
            (1536, 960),
            Some(candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888)),
            true,
        );
        assert!(composited.rejection().is_some());
        assert!(overlay_plan_permitted(&composited));
    }

    #[test]
    fn argb_full_opaque_region_is_scanout_eligible() {
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_ARGB8888);
        surface.opaque_region = Some(vec![tessera_primitives::Rect::new(0, 0, 1536, 960)]);
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Ok(())
        );
    }

    #[test]
    fn incomplete_argb_opaque_region_is_rejected() {
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_ARGB8888);
        surface.opaque_region = Some(vec![tessera_primitives::Rect::new(0, 0, 1535, 960)]);
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Err(ScanoutRejectReason::NotFullyOpaque)
        );
    }

    #[test]
    fn output_geometry_must_match_exactly() {
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888);
        surface.width -= 1;
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Err(ScanoutRejectReason::GeometryMismatch)
        );
    }

    #[test]
    fn viewport_identity_full_buffer_to_logical_is_scanout_eligible() {
        // The canonical fractional-scale shape — the buffer is the full
        // physical mode, `viewport_src` covers the whole buffer at (0, 0), and
        // `viewport_dst` equals the complete logical output — is a 1:1 identity
        // mapping and must scan out directly.
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888);
        surface.geometry.buffer_scale = 1;
        surface.geometry.viewport_src = Some(tessera_primitives::Rect::new(0, 0, 3072, 1920));
        surface.geometry.viewport_dst = Some(tessera_primitives::Size { w: 1536, h: 960 });
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Ok(())
        );
    }

    #[test]
    fn viewport_sub_buffer_crop_is_rejected() {
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888);
        surface.geometry.buffer_scale = 1;
        surface.geometry.viewport_src = Some(tessera_primitives::Rect::new(0, 0, 3000, 1920));
        surface.geometry.viewport_dst = Some(tessera_primitives::Size { w: 1536, h: 960 });
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Err(ScanoutRejectReason::Viewport)
        );
    }

    #[test]
    fn viewport_destination_must_span_the_logical_output() {
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888);
        surface.geometry.buffer_scale = 1;
        surface.geometry.viewport_src = Some(tessera_primitives::Rect::new(0, 0, 3072, 1920));
        // A destination that does not equal the complete logical output still
        // requires GPU scaling and must not be scanned out.
        surface.geometry.viewport_dst = Some(tessera_primitives::Size { w: 1000, h: 960 });
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Err(ScanoutRejectReason::Viewport)
        );
    }

    #[test]
    fn viewport_argb_opaque_region_maps_through_the_destination() {
        // An ARGB buffer under `wp_viewport` declares its opaque region in
        // destination (logical) coordinates; the opacity check must compare
        // against the viewport destination extent, not the raw buffer extent
        // divided by `buffer_scale`.
        let mut surface = candidate(tessera_primitives::dmabuf::DRM_FORMAT_ARGB8888);
        surface.geometry.buffer_scale = 1;
        surface.geometry.viewport_src = Some(tessera_primitives::Rect::new(0, 0, 3072, 1920));
        surface.geometry.viewport_dst = Some(tessera_primitives::Size { w: 1536, h: 960 });
        surface.opaque_region = Some(vec![tessera_primitives::Rect::new(0, 0, 1536, 960)]);
        assert_eq!(
            evaluate_surface(&surface, (3072, 1920), (1536, 960), true),
            Ok(())
        );
    }

    #[test]
    fn popup_or_subsurface_blocks_scanout() {
        let facts = ScanoutSceneFacts {
            dmabuf_surface_count: 2,
            toplevel_dmabuf_count: 1,
            candidate_matches_scene: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::MultipleDmabufSurfaces)
        );
        let facts = ScanoutSceneFacts {
            shm_surface_count: 1,
            dmabuf_surface_count: 1,
            toplevel_dmabuf_count: 1,
            candidate_matches_scene: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::ShmClientSurface)
        );
    }

    #[test]
    fn visible_shell_pixels_block_but_hidden_dock_does_not() {
        let mut facts = eligible_scene();
        facts.shell_visible_pixels = true;
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::VisibleShellPixels)
        );

        facts.shell_visible_pixels = false;
        assert_eq!(evaluate_scene(facts), Ok(()));
    }

    #[test]
    fn client_overlay_never_disappears_under_scanout() {
        let mut facts = eligible_scene();
        facts.client_overlay_count = 1;
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::ClientOverlay)
        );
    }

    #[test]
    fn window_switcher_reclaims_primary_plane_without_mutating_fullscreen_state() {
        let mut facts = eligible_scene();
        facts.window_switcher_active = true;
        assert_eq!(
            evaluate_scene(facts),
            Err(ScanoutRejectReason::WindowSwitcher)
        );

        let mut state = PrimaryPlaneState::default();
        assert!(state.commit_direct(7));
        assert!(state.is_direct());
        // Planning a composite does not lie about physical ownership. The
        // state changes only after that compositor framebuffer really lands.
        assert!(state.commit_composited());
        assert_eq!(state, PrimaryPlaneState::Composited);
    }

    #[test]
    fn primary_plane_state_tracks_successful_owner_changes() {
        let mut state = PrimaryPlaneState::default();
        assert!(state.commit_direct(7));
        assert!(!state.commit_direct(7));
        assert!(state.commit_direct(9));
        assert_eq!(state, PrimaryPlaneState::DirectScanout { surface_id: 9 });
        assert!(state.commit_composited());
        assert!(!state.commit_composited());
        state.invalidate();
        assert_eq!(state, PrimaryPlaneState::Unassigned);
    }

    fn blocked(reason: ScanoutRejectReason, detail: Option<&'static str>) -> ScanoutRejection {
        ScanoutRejection {
            reason,
            plausible_candidate: true,
            detail,
        }
    }

    fn blocked_events(events: &[ScanoutLogEvent]) -> Vec<ScanoutLogEvent> {
        events
            .iter()
            .copied()
            .filter(|event| !matches!(event, ScanoutLogEvent::Summary))
            .collect()
    }

    #[test]
    fn shell_rejection_names_the_blocking_component() {
        let plan = plan_scanout(
            ScanoutSceneFacts {
                shell_visible_pixels: true,
                shell_blocker: Some("Pivot"),
                ..eligible_scene()
            },
            (3072, 1920),
            (1536, 960),
            Some(candidate(tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888)),
            true,
        );
        let rejection = *plan.rejection().expect("composited");
        assert_eq!(rejection.reason, ScanoutRejectReason::VisibleShellPixels);
        assert_eq!(rejection.detail, Some("Pivot"));
        assert!(rejection.plausible_candidate);
        assert_eq!(rejection.to_string(), "visible-shell-pixels (Pivot)");
    }

    #[test]
    fn detail_is_only_attached_to_the_reason_it_explains() {
        let plan = plan_scanout(
            ScanoutSceneFacts {
                overview_active: true,
                shell_visible_pixels: true,
                shell_blocker: Some("Overview"),
                ..eligible_scene()
            },
            (3072, 1920),
            (1536, 960),
            None,
            true,
        );
        let rejection = plan.rejection().expect("composited");
        assert_eq!(rejection.reason, ScanoutRejectReason::Overview);
        assert_eq!(rejection.detail, None);
    }

    /// A steady blocked fullscreen video logs once, not every frame or every
    /// few seconds.
    #[test]
    fn telemetry_logs_block_transitions_not_frames() {
        let mut telemetry = ScanoutTelemetry::new();
        let start = std::time::Instant::now();
        let pivot = blocked(ScanoutRejectReason::VisibleShellPixels, Some("Pivot"));

        let first = telemetry.observe_rejection(pivot, start);
        assert_eq!(
            blocked_events(&first),
            vec![ScanoutLogEvent::Blocked(pivot)]
        );
        for frame in 1..600u64 {
            let at = start + std::time::Duration::from_millis(frame * 16);
            assert!(blocked_events(&telemetry.observe_rejection(pivot, at)).is_empty());
        }
        assert_eq!(telemetry.current_block(), Some(pivot));

        // A different blocker is a new state and is logged once.
        let switcher = blocked(ScanoutRejectReason::WindowSwitcher, None);
        let at = start + std::time::Duration::from_secs(10);
        assert_eq!(
            blocked_events(&telemetry.observe_rejection(switcher, at)),
            vec![ScanoutLogEvent::Blocked(switcher)]
        );

        assert_eq!(
            telemetry.observe_success(at),
            Some(ScanoutLogEvent::Resumed)
        );
        assert_eq!(
            telemetry.observe_success(at),
            None,
            "steady direct scanout is silent"
        );
        assert_eq!(telemetry.current_block(), None);
        let totals = telemetry.totals();
        assert_eq!(totals.blocked_candidate_frames, 601);
        assert_eq!(totals.direct_frames, 2);
    }

    /// The defect class behind the Pivot regression: one blocker that never
    /// stands down. It gets exactly one warning per episode.
    #[test]
    fn telemetry_warns_once_for_a_sustained_block() {
        let mut telemetry = ScanoutTelemetry::new();
        let start = std::time::Instant::now();
        let pivot = blocked(ScanoutRejectReason::VisibleShellPixels, Some("Pivot"));
        let mut sustained = 0;
        let frames = (SCANOUT_SUSTAINED_BLOCK_WARNING.as_millis() as u64 / 16) * 3;
        for frame in 0..frames {
            let at = start + std::time::Duration::from_millis(frame * 16);
            sustained += telemetry
                .observe_rejection(pivot, at)
                .iter()
                .filter(|event| matches!(event, ScanoutLogEvent::Sustained(_)))
                .count();
        }
        assert_eq!(sustained, 1);
    }

    /// Leaving fullscreen ends the episode without a `resumed` claim, and
    /// re-entering a blocked fullscreen logs the block again.
    #[test]
    fn telemetry_ends_episode_quietly_when_the_candidate_disappears() {
        let mut telemetry = ScanoutTelemetry::new();
        let start = std::time::Instant::now();
        let pivot = blocked(ScanoutRejectReason::VisibleShellPixels, Some("Pivot"));
        telemetry.observe_rejection(pivot, start);
        let desktop = ScanoutRejection::new(ScanoutRejectReason::MultipleDmabufSurfaces, false);
        assert!(blocked_events(&telemetry.observe_rejection(desktop, start)).is_empty());
        assert_eq!(telemetry.current_block(), None);
        assert_eq!(
            blocked_events(&telemetry.observe_rejection(pivot, start)),
            vec![ScanoutLogEvent::Blocked(pivot)]
        );
        assert_eq!(telemetry.totals().desktop_frames, 1);
    }

    #[test]
    fn telemetry_summary_is_rate_limited() {
        let mut telemetry = ScanoutTelemetry::new();
        let start = std::time::Instant::now();
        let desktop = ScanoutRejection::new(ScanoutRejectReason::NoDmabufCandidate, false);
        let mut summaries = 0;
        for frame in 0..(60 * 12u64) {
            let at = start + std::time::Duration::from_millis(frame * 1000 / 60);
            summaries += telemetry
                .observe_rejection(desktop, at)
                .iter()
                .filter(|event| matches!(event, ScanoutLogEvent::Summary))
                .count();
        }
        assert_eq!(summaries, 3, "one summary at 0 s, 5 s, and 10 s");
    }
}
