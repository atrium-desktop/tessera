use super::*;

// Direct-scanout policy — scene eligibility, candidate evaluation,
// plane-ownership state, and rejection telemetry — is a pure evaluation
// owned by `tessera-presentation` (api tier). This module keeps only the
// composition-root observation: gathering the semantic scene facts from the
// server, shell, and host and handing them to the evaluator.
pub(super) use tessera_presentation::{
    PrimaryPlanePlan, PrimaryPlaneState, ScanoutRejectReason, ScanoutTelemetry,
};

impl CompositorRuntime {
    /// Evaluate the physical output as a primary-plane scene.  This is based
    /// exclusively on actual surface geometry/coverage and current visible
    /// compositor output; xdg fullscreen state is intentionally irrelevant.
    pub(super) fn plan_primary_plane(
        &self,
        physical_size: (u32, u32),
        cursor_hidden: bool,
        frame_capture_pending: bool,
    ) -> PrimaryPlanePlan {
        let shell = self
            .shell
            .composition_requirements(self.input_acc.display_size);
        // One shared visibility/occlusion snapshot for every count here: the
        // scanout planner previously rebuilt both full frame lists (each an
        // O(windows × surfaces) occlusion walk plus per-surface Vec clones of
        // damage/opaque regions) only to read `.len()` from them — and it ran
        // before the cheap SHM rejection, so a pure-SHM desktop paid the full
        // dma-buf collection every frame for a candidate that always fails.
        let sets = self.server.desktop_frame_sets();
        let visible = &sets.visible;
        let occluded = &sets.occluded;
        let shm_surface_count = sets.shm.len();
        let dmabuf_surface_count = sets.dmabuf.len();
        let mut toplevel_dmabufs = self.server.toplevel_dmabuf_frames_with(visible, occluded);
        let candidate_matches_scene = dmabuf_surface_count == 1
            && toplevel_dmabufs.len() == 1
            && sets.dmabuf[0].id == toplevel_dmabufs[0].id;
        let facts = tessera_presentation::ScanoutSceneFacts {
            session_locked: self.server.session_locked(),
            capture_pending: frame_capture_pending
                || self.pending_capture.is_some()
                || self.pending_interaction_domain_capture.is_some(),
            screenshot_freeze: self.screenshot_freeze.armed,
            transition_pending: self.server.transitions_pending(),
            overview_active: self.shell.overview_active(),
            window_switcher_active: self.shell.window_switcher_active(),
            shell_visible_pixels: shell.visible_pixels,
            shell_live_backdrop_effect: shell.live_backdrop_effect,
            client_overlay_count: self.server.overlay_frames().len()
                + self.server.overlay_dmabuf_frames().len(),
            software_cursor_visible: self.host.uses_software_cursor() && !cursor_hidden,
            shm_surface_count,
            dmabuf_surface_count,
            toplevel_dmabuf_count: toplevel_dmabufs.len(),
            candidate_matches_scene,
        };
        let candidate = (candidate_matches_scene && dmabuf_surface_count == 1)
            .then(|| toplevel_dmabufs.pop())
            .flatten();
        let plane_supported = candidate
            .as_ref()
            .is_some_and(|candidate| {
                self.host
                    .supports_scanout(candidate.drm_format, candidate.modifier)
            });
        tessera_presentation::plan_scanout(facts, physical_size, candidate, plane_supported)
    }
}
