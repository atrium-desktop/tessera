use super::*;
use tessera_composite::presentation::OverlayPlan;
use tessera_composite::presentation::OverlayPlaneFacts;
use tessera_composite::presentation::OverlaySceneFacts;
use tessera_composite::presentation::OverlaySurfaceFacts;
use tessera_composite::presentation::logical_rect_to_physical;

// Direct-scanout policy — scene eligibility, candidate evaluation,
// plane-ownership state, and rejection telemetry — is a pure evaluation
// owned by `tessera-composite` (api tier). This module keeps only the
// composition-root observation: gathering the semantic scene facts from the
// server, shell, and host and handing them to the evaluators.
pub(super) use tessera_composite::presentation::{
    PrimaryPlanePlan, PrimaryPlaneState, ScanoutRejectReason, ScanoutTelemetry,
};

/// The two output-layer plans evaluated for one frame.
///
/// `primary` owns the presentation decision. `overlay` is `Some` only when
/// Primary is composited and either overlay presentation is enabled or audit
/// logging needs the rejection evidence (ADR-0235, ADR-0237). Direct primary
/// scanout excludes overlay for that frame.
pub(super) struct OutputLayerPlans {
    pub(super) primary: PrimaryPlanePlan,
    /// Connector whose CRTC is the direct scanout target when `primary` is a
    /// direct plan. `None` for a composited plan.
    pub(super) scanout_target: Option<String>,
    /// Per-output direct plans when *every* physical output has its own
    /// covering opaque client for this frame. Empty for the ordinary one-target
    /// or composite path.
    pub(super) direct_targets: Vec<(String, PrimaryPlanePlan)>,
    /// `Some` only for a composited Primary while overlay presentation is
    /// enabled or debug audit logging is active. Production does not pay a
    /// per-frame window-clone cost for a plan it cannot present.
    pub(super) overlay: Option<OverlayAudit>,
}

pub(super) struct OverlayAudit {
    pub(super) plan: OverlayPlan,
    /// Connector whose CRTC would own the overlay plane.
    pub(super) output: Option<String>,
    /// Physical destination rectangle in output-local coordinates.
    pub(super) target: tessera_primitives::Rect,
}

impl CompositorRuntime<'_> {
    /// Evaluate the physical output as a primary-plane scene.  This is based
    /// exclusively on actual surface geometry/coverage and current visible
    /// compositor output; xdg fullscreen state is intentionally irrelevant.
    pub(super) fn plan_output_layers(
        &mut self,
        physical_size: (u32, u32),
        logical_size: (u32, u32),
        scale: f32,
        cursor_hidden: bool,
        frame_capture_pending: bool,
    ) -> OutputLayerPlans {
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
        let mirror_present = self
            .server
            .output_infos()
            .iter()
            .any(|output| output.role.is_mirror());
        let facts = tessera_composite::presentation::ScanoutSceneFacts {
            session_locked: self.server.session_locked(),
            mirror_present,
            capture_pending: frame_capture_pending
                || self.pending_capture.is_some()
                || self.pending_interaction_domain_capture.is_some(),
            screenshot_freeze: self.screenshot_freeze.armed,
            transition_pending: self.server.transitions_pending(),
            overview_active: self.shell.overview_active(),
            window_switcher_active: self.shell.window_switcher_active(),
            shell_visible_pixels: shell.visible_pixels,
            client_overlay_count: self.server.overlay_frames().len()
                + self.server.overlay_dmabuf_frames().len(),
            software_cursor_visible: self.host.uses_software_cursor() && !cursor_hidden,
            shm_surface_count,
            dmabuf_surface_count,
            toplevel_dmabuf_count: toplevel_dmabufs.len(),
            candidate_matches_scene,
        };
        let mut candidate = (candidate_matches_scene && dmabuf_surface_count == 1)
            .then(|| toplevel_dmabufs.pop())
            .flatten();
        let outputs = self.server.output_infos();
        // Scanout eligibility is per output, not against the union canvas
        // (ADR-0185 §4). A candidate belongs to the output whose logical rect
        // contains its top-left; fullscreen clients are anchored at that
        // output's origin.
        let target_output = candidate
            .as_ref()
            .and_then(|candidate| {
                outputs
                    .iter()
                    .filter(|output| output.kind.is_physical())
                    .find(|output| {
                        output
                            .geometry
                            .logical_rect()
                            .contains(candidate.geometry.position)
                    })
            })
            .or_else(|| outputs.iter().find(|output| output.kind.is_physical()));
        if let (Some(candidate), Some(output)) = (candidate.as_mut(), target_output) {
            candidate.geometry.position.x = candidate
                .geometry
                .position
                .x
                .saturating_sub(output.geometry.logical_origin.x);
            candidate.geometry.position.y = candidate
                .geometry
                .position
                .y
                .saturating_sub(output.geometry.logical_origin.y);
        }
        let (scanout_physical, scanout_logical) = target_output
            .map(|output| {
                (
                    (
                        output.geometry.mode.width.max(1) as u32,
                        output.geometry.mode.height.max(1) as u32,
                    ),
                    (
                        output.geometry.logical_size().w.max(1) as u32,
                        output.geometry.logical_size().h.max(1) as u32,
                    ),
                )
            })
            .unwrap_or((physical_size, logical_size));
        let plane_supported = candidate.as_ref().is_some_and(|candidate| {
            self.host
                .supports_scanout(candidate.drm_format, candidate.modifier)
        });
        let primary = tessera_composite::presentation::plan_scanout(
            facts,
            scanout_physical,
            scanout_logical,
            candidate,
            plane_supported,
        );
        let scanout_target = primary
            .direct_candidate()
            .and_then(|_| target_output.map(|output| output.connector.clone()));
        // Multiple simultaneous direct scanouts are only planned for the
        // all-outputs case: every physical output must be covered by its own
        // opaque client and no shell/overlay/transition may need composition.
        // Mixed direct+composite remains the single-target path above.
        let mut direct_targets = Vec::new();
        if outputs.len() > 1
            && !facts.mirror_present
            && !facts.session_locked
            && !facts.capture_pending
            && !facts.screenshot_freeze
            && !facts.transition_pending
            && !facts.overview_active
            && !facts.window_switcher_active
            && !facts.shell_visible_pixels
            && facts.client_overlay_count == 0
            && !facts.software_cursor_visible
            && sets.shm.is_empty()
            && sets.dmabuf.len() == outputs.len()
            && toplevel_dmabufs.len() == outputs.len()
        {
            let mut used = std::collections::HashSet::new();
            let mut complete = true;
            for output in outputs.iter().filter(|output| output.kind.is_physical()) {
                let Some(index) = toplevel_dmabufs.iter().position(|candidate| {
                    !used.contains(&candidate.id)
                        && candidate.geometry.position == output.geometry.logical_origin
                }) else {
                    complete = false;
                    break;
                };
                let mut candidate = toplevel_dmabufs[index].clone();
                candidate.geometry.position.x = 0;
                candidate.geometry.position.y = 0;
                let physical = (
                    output.geometry.mode.width.max(1) as u32,
                    output.geometry.mode.height.max(1) as u32,
                );
                let logical = (
                    output.geometry.logical_size().w.max(1) as u32,
                    output.geometry.logical_size().h.max(1) as u32,
                );
                let supported = self
                    .host
                    .supports_scanout(candidate.drm_format, candidate.modifier);
                let mut one = facts;
                one.shm_surface_count = 0;
                one.dmabuf_surface_count = 1;
                one.toplevel_dmabuf_count = 1;
                one.candidate_matches_scene = true;
                one.shell_visible_pixels = false;
                one.client_overlay_count = 0;
                one.software_cursor_visible = false;
                let plan = tessera_composite::presentation::plan_scanout(
                    one,
                    physical,
                    logical,
                    Some(candidate),
                    supported,
                );
                if plan.direct_candidate().is_none() {
                    complete = false;
                    break;
                }
                used.insert(toplevel_dmabufs[index].id);
                direct_targets.push((output.connector.clone(), plan));
            }
            if !complete || direct_targets.len() != outputs.len() {
                direct_targets.clear();
            }
        }

        let overlay = (tessera_composite::presentation::overlay_plan_permitted(&primary)
            && (self.overlay_offload_enabled || log::log_enabled!(log::Level::Debug)))
        .then(|| {
            self.plan_overlay_layer(
                &sets,
                physical_size,
                logical_size,
                scale,
                cursor_hidden,
                frame_capture_pending,
            )
        });
        if !self.overlay_offload_enabled
            && let Some(audit) = &overlay
            && audit.plan.rejection().is_none()
        {
            let candidate = sets
                .dmabuf
                .iter()
                .find(|frame| Some(frame.id) == audit.plan.candidate_surface_id());
            if let (Some(surface), Some(output)) = (candidate, audit.output.as_deref()) {
                let layer = tessera_platform::host::OverlayLayer {
                    surface,
                    output,
                    target: audit.target,
                    zpos: audit.plan.zpos(),
                };
                if let Err(error) = self.host.probe_overlay(&layer) {
                    self.overlay_telemetry.record_rejection(
                        self.host.name(),
                        tessera_composite::presentation::OverlayRejectReason::KmsRejected,
                        true,
                    );
                    log::debug!(
                        "{}: overlay TEST_ONLY rejected for {output}: {error}",
                        self.host.name()
                    );
                }
            }
        }
        OutputLayerPlans {
            primary,
            scanout_target,
            direct_targets,
            overlay,
        }
    }

    fn plan_overlay_layer(
        &self,
        sets: &tessera_wayland::DesktopFrameSets<'_>,
        physical_size: (u32, u32),
        logical_size: (u32, u32),
        scale: f32,
        cursor_hidden: bool,
        frame_capture_pending: bool,
    ) -> OverlayAudit {
        let windows = self.server.render_windows();
        let Some(window) = windows.last() else {
            return OverlayAudit {
                plan: tessera_composite::presentation::plan_overlay(
                    OverlaySceneFacts {
                        output: self.server.output_logical_rect(),
                        ..Default::default()
                    },
                    None,
                    None,
                    OverlayPlaneFacts::default(),
                ),
                output: None,
                target: tessera_primitives::Rect::new(0, 0, 0, 0),
            };
        };
        let target = tessera_primitives::Rect {
            origin: window.position,
            size: window.size,
        };
        let output_infos = self.server.output_infos();
        let (output_rect, output_name) = output_infos
            .iter()
            .find(|info| {
                let output = info.geometry.logical_rect();
                target.origin.x >= output.origin.x
                    && target.origin.y >= output.origin.y
                    && target.origin.x.saturating_add(target.size.w)
                        <= output.origin.x.saturating_add(output.size.w)
                    && target.origin.y.saturating_add(target.size.h)
                        <= output.origin.y.saturating_add(output.size.h)
            })
            .map(|info| (info.geometry.logical_rect(), Some(info.connector.clone())))
            .unwrap_or_else(|| (self.server.output_logical_rect(), None));

        let mut dmabuf = None;
        let mut dmabuf_count = 0;
        for frame in &sets.dmabuf {
            if frame.window == Some(window.id) {
                dmabuf_count += 1;
                dmabuf = Some(frame);
            }
        }
        let shm_count = sets
            .shm
            .iter()
            .filter(|frame| frame.window == Some(window.id))
            .count();
        let candidate_is_topmost = windows.last().map(|window| window.id) == Some(window.id);
        let candidate = (dmabuf_count == 1).then_some(dmabuf).flatten();

        let physical_target =
            logical_rect_to_physical(target, scale, physical_size.0, physical_size.1);
        let identity_mapped = candidate.is_some_and(|frame| {
            frame.geometry.transform == tessera_primitives::Transform::Normal
                && frame.geometry.viewport_src.is_none()
                && frame.geometry.viewport_dst.is_none()
                && frame.width == physical_target.size.w
                && frame.height == physical_target.size.h
        });
        let opaque = candidate.is_some_and(|frame| {
            tessera_desktop::presentation::SurfacePresentation::from_dmabuf(frame).is_opaque()
        });
        let color_managed = candidate.is_some_and(|frame| frame.color.is_some());
        let plane_facts = self.overlay_plane_facts(output_name.as_deref(), candidate);
        let chrome_area = optics_gfx::CanvasRenderArea {
            x: physical_target.origin.x as f32,
            y: physical_target.origin.y as f32,
            width: physical_target.size.w as f32,
            height: physical_target.size.h as f32,
        };
        let chrome_overlaps_candidate = self
            .shell
            .composition_requirements(self.input_acc.display_size)
            .visible_pixels
            && self
                .shell
                .chrome_intersects_render_area(chrome_area, scale, logical_size);
        let software_cursor_visible = self.host.uses_software_cursor() && !cursor_hidden;
        let output_physical =
            logical_rect_to_physical(output_rect, scale, physical_size.0, physical_size.1);
        let local_target = tessera_primitives::Rect::new(
            physical_target.origin.x - output_physical.origin.x,
            physical_target.origin.y - output_physical.origin.y,
            physical_target.size.w,
            physical_target.size.h,
        );

        let plan = tessera_composite::presentation::plan_overlay(
            OverlaySceneFacts {
                session_locked: self.server.session_locked(),
                capture_pending: frame_capture_pending
                    || self.pending_capture.is_some()
                    || self.pending_interaction_domain_capture.is_some(),
                screenshot_freeze: self.screenshot_freeze.armed,
                transition_pending: self.server.transitions_pending(),
                overview_active: self.shell.overview_active(),
                window_switcher_active: self.shell.window_switcher_active(),
                candidate_is_topmost,
                dma_buf_surfaces_for_window: dmabuf_count,
                shm_surfaces_for_window: shm_count,
                client_overlay_count: self.server.overlay_frames().len()
                    + self.server.overlay_dmabuf_frames().len(),
                chrome_overlaps_candidate,
                software_cursor_overlaps_candidate: software_cursor_visible,
                output: output_rect,
            },
            Some(OverlaySurfaceFacts {
                window: window.id,
                target,
                opaque,
                identity_mapped,
                scene_feedback: window.attention_pulse
                    || window.suspended_by_modal
                    || window.transition.is_some()
                    || tessera_composite::window_casts_shadow(window),
                color_managed,
            }),
            candidate,
            plane_facts,
        );
        OverlayAudit {
            plan,
            output: output_name,
            target: local_target,
        }
    }

    fn overlay_plane_facts(
        &self,
        output: Option<&str>,
        candidate: Option<&tessera_primitives::SurfaceDmabuf>,
    ) -> OverlayPlaneFacts {
        let planes = self.host.overlay_planes();
        let plane = output.and_then(|output| planes.iter().find(|plane| plane.output == output));
        let format_supported = candidate.is_some_and(|candidate| {
            plane.is_some_and(|plane| {
                plane
                    .formats
                    .iter()
                    .any(|format| format.fourcc == candidate.drm_format)
            })
        });
        let modifier_supported = candidate.is_some_and(|candidate| {
            plane.is_some_and(|plane| plane.supports(candidate.drm_format, candidate.modifier))
        });
        OverlayPlaneFacts {
            available: plane.is_some(),
            format_supported,
            modifier_supported,
            // Driver default order keeps cursor above overlay; an explicit
            // zpos is chosen only when the cursor plane's value is known.
            // The backend still validates the range in TEST_ONLY.
            zpos: None,
        }
    }
}
