//! Normal-frame rendering into one native target per physical output.
//!
//! The legacy union swapchain is still used for capture/screenshot frames and
//! as a failure fallback while those consumers migrate, but the ordinary
//! desktop no longer presents a union image. Each extended output renders its
//! own logical rectangle into a mode-sized target; a mirror target receives a
//! blit of its source target with the fitted letterbox. Every target has its
//! own gfx ring, KMS slot, and page flip.

use super::output_target::{
    RetainedSourceExport, TARGET_ACQUIRE_TIMEOUT_NS, TargetImportCache, TargetImportDesc,
    drm_target_commit_is_transient, export_target_dmabuf, gfx_format_for_drm,
    gfx_target_acquire_is_transient,
};
use super::*;
use std::os::fd::OwnedFd;
use std::time::Duration;
use tessera_platform::host::RenderTargetCommit;
use tessera_platform::sync::{CompletionFence, merge_live_sync_fences};
use tessera_wayland::PresentedSurfaces;

/// Bilinear-filter pixel margin added around projected source damage on mirror
/// targets so upscaling/downscaling filter taps never sample stale border texels
/// (ADR-0250, `[INV-ARCH-160]`).
const MIRROR_DAMAGE_MARGIN_PX: i32 = 2;

/// Bounded wait used only when `SYNC_IOC_MERGE` fails while live fences remain
/// unmerged (ADR-0264, `[INV-ARCH-159]`).
const UNMERGED_FENCE_WAIT_TIMEOUT: Duration = Duration::from_secs(1);

/// Classification of the frame's completion synchronization state for pure
/// retirement-release planning (ADR-0264, `[INV-ARCH-159]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionState {
    /// All contributing GPU/KMS work is already signaled or waited on.
    Signaled,
    /// A single merged live `sync_file` is available for non-blocking handoff.
    LiveFence,
    /// `SYNC_IOC_MERGE` failed while multiple fences remained live; must wait
    /// before releasing retired buffers.
    UnmergedFences,
}

impl CompletionState {
    fn from_fence(fence: &CompletionFence) -> Self {
        match fence {
            CompletionFence::Signaled => Self::Signaled,
            CompletionFence::Live(_) => Self::LiveFence,
            CompletionFence::Unmerged(_) => Self::UnmergedFences,
        }
    }
}

/// Action to release retired client buffers at a presentation boundary
/// (ADR-0250, ADR-0264, `[INV-ARCH-159]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetiredReleaseAction {
    /// No retired buffers are pending.
    None,
    /// Release immediately with `completion_fence = None` (GPU/KMS work is
    /// already known complete).
    ReleaseImmediate,
    /// Release immediately with the merged sync_file completion fence.
    ReleaseWithFence,
    /// Wait for unmerged live fences (escalating to `device.wait_idle()` on
    /// timeout/error) and then release immediately.
    WaitUnmergedAndRelease,
    /// Synchronize the Vulkan device (`wait_idle`) and release immediately
    /// (nested host swapchain, which exposes no exportable completion fence).
    WaitDeviceAndRelease,
}

/// Decide how to release retired client buffers after a composited frame or on
/// a `NoDamage` fast-exit.
///
/// Frame-count deferral (`retired_defer`) is intentionally absent: deferring
/// release across N frames starves 2–4 buffer client swapchains and deadlocks
/// once a blocked client stops producing damage (ADR-0250, ADR-0264,
/// `[INV-ARCH-159]`).
pub(super) fn plan_retired_release(
    retired_pending: bool,
    presents_through_host_swapchain: bool,
    completion: CompletionState,
) -> RetiredReleaseAction {
    if !retired_pending {
        return RetiredReleaseAction::None;
    }
    if presents_through_host_swapchain {
        return RetiredReleaseAction::WaitDeviceAndRelease;
    }
    match completion {
        CompletionState::Signaled => RetiredReleaseAction::ReleaseImmediate,
        CompletionState::LiveFence => RetiredReleaseAction::ReleaseWithFence,
        CompletionState::UnmergedFences => RetiredReleaseAction::WaitUnmergedAndRelease,
    }
}

struct RenderedSource {
    connector: String,
    target_idx: usize,
    format: optics_gfx::Format,
    color_pipeline: tessera_desktop::output::ColorPipeline,
    dmabuf: optics_gfx::SurfaceDmabuf,
    lease: std::rc::Rc<optics_gfx::DmabufLease>,
    origin: tessera_primitives::Point,
    scale: f32,
    mode: (u32, u32),
    source_damage: FrameDamage,
}

struct MirrorSourceView<'a> {
    fd: &'a OwnedFd,
    acquire_fence: Option<&'a OwnedFd>,
    width: u32,
    height: u32,
    stride: u32,
    modifier: u64,
    format: optics_gfx::Format,
    color_pipeline: tessera_desktop::output::ColorPipeline,
    origin: tessera_primitives::Point,
    scale: f32,
    mode: (u32, u32),
    lease: &'a std::rc::Rc<optics_gfx::DmabufLease>,
    source_damage: Option<&'a FrameDamage>,
}

struct TargetMeta {
    connector: String,
    last_source_export: Option<RetainedSourceExport>,
    source_lease: Option<std::rc::Rc<optics_gfx::DmabufLease>>,
    import_cache: TargetImportCache,
    mode: (u32, u32),
    format: optics_gfx::Format,
    color_pipeline: tessera_desktop::output::ColorPipeline,
    source: String,
    needs_present: bool,
    last_mirror_cursor_rect: Option<tessera_primitives::Rect>,
}

/// Temporarily splits a presentation group's targets into disjoint `meta`,
/// `canvases`, and `surfaces` vecs so `SubmittedFrame<'_>` borrows `&surfaces[i]`
/// across the group's entire render + KMS commit sequence while `canvases[i]`
/// and `meta[i]` remain independently borrowable.
///
/// Field drop order is safety-critical: `meta` (holding `last_source_export`,
/// `source_lease`, and `import_cache`) and `canvases` are declared before
/// `surfaces` so all `optics_gfx::DmabufLease` handles and canvas pass states drop
/// before the underlying `optics_gfx::Surface`s even on early exit.
struct GroupTargets {
    meta: Vec<TargetMeta>,
    canvases: Vec<optics_gfx::Canvas>,
    surfaces: Vec<optics_gfx::Surface>,
}

impl GroupTargets {
    fn take_from(
        targets: &mut std::collections::BTreeMap<String, OutputTarget>,
        connectors: &[String],
    ) -> Self {
        let mut meta = Vec::with_capacity(connectors.len());
        let mut canvases = Vec::with_capacity(connectors.len());
        let mut surfaces = Vec::with_capacity(connectors.len());
        for connector in connectors {
            if let Some(target) = targets.remove(connector) {
                meta.push(TargetMeta {
                    connector: connector.clone(),
                    last_source_export: target.last_source_export,
                    source_lease: target.source_lease,
                    import_cache: target.import_cache,
                    mode: target.mode,
                    format: target.format,
                    color_pipeline: target.color_pipeline,
                    source: target.source,
                    needs_present: target.needs_present,
                    last_mirror_cursor_rect: target.last_mirror_cursor_rect,
                });
                canvases.push(target.canvas);
                surfaces.push(target.surface);
            }
        }
        Self {
            meta,
            canvases,
            surfaces,
        }
    }

    fn index_of(&self, connector: &str) -> Option<usize> {
        self.meta
            .iter()
            .position(|entry| entry.connector == connector)
    }

    fn return_to(
        self,
        targets: &mut std::collections::BTreeMap<String, OutputTarget>,
        permanently_failed: &std::collections::BTreeSet<String>,
    ) {
        for ((meta, canvas), surface) in self.meta.into_iter().zip(self.canvases).zip(self.surfaces)
        {
            if permanently_failed.contains(&meta.connector) {
                continue;
            }
            targets.insert(
                meta.connector,
                OutputTarget {
                    last_source_export: meta.last_source_export,
                    source_lease: meta.source_lease,
                    import_cache: meta.import_cache,
                    canvas,
                    surface,
                    mode: meta.mode,
                    format: meta.format,
                    color_pipeline: meta.color_pipeline,
                    source: meta.source,
                    needs_present: meta.needs_present,
                    last_mirror_cursor_rect: meta.last_mirror_cursor_rect,
                },
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetCommitFailure {
    CursorFallback,
    ReconfiguredOrInactive,
    TransientBusy,
    Permanent,
}

fn classify_target_commit_error(error: &HostError) -> TargetCommitFailure {
    match error {
        HostError::Drm(DrmError::CursorFallback) => TargetCommitFailure::CursorFallback,
        HostError::Drm(DrmError::Inactive | DrmError::Reconfigured) => {
            TargetCommitFailure::ReconfiguredOrInactive
        }
        HostError::Drm(DrmError::Busy | DrmError::FlipTimeout) => {
            TargetCommitFailure::TransientBusy
        }
        _ => TargetCommitFailure::Permanent,
    }
}

/// Planned render work for one non-pending presentation group.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupRenderPlan {
    sources: Vec<(String, FrameDamage)>,
    mirrors: Vec<String>,
}

/// Decide which outputs inside one presentation group actually need to render
/// and flip this frame.
///
/// An untouched peer source in a multi-source group is simply omitted rather
/// than treated as a failure. Mirrors render whenever their source renders,
/// when they owe an initial presentation, or when a hardware-cursor change
/// dirties the mirror (`mirror_cursor_dirty`). When the source already retains
/// its last presented frame (`source_has_retained_export`), a cursor-only
/// mirror update runs without forcing the source to re-render (ADR-0250,
/// `[INV-ARCH-160]`).
fn plan_group_render(
    group_outputs: &[String],
    outputs: &[tessera_desktop::output::OutputInfo],
    mut source_damage: impl FnMut(&tessera_desktop::output::OutputInfo) -> Option<FrameDamage>,
    mirror_needs_present: impl Fn(&str) -> bool,
    mirror_cursor_dirty: bool,
    source_has_retained_export: impl Fn(&str) -> bool,
) -> GroupRenderPlan {
    let mut mirror_pull_sources = std::collections::BTreeSet::new();
    let mut cursor_or_init_mirrors = std::collections::BTreeSet::new();
    for connector in group_outputs {
        if let Some(output) = outputs.iter().find(|o| o.connector == *connector)
            && let Some(source) = output.role.mirror_source()
        {
            let needs_init = mirror_needs_present(connector);
            if needs_init || mirror_cursor_dirty {
                cursor_or_init_mirrors.insert(connector.clone());
                if needs_init || !source_has_retained_export(source) {
                    mirror_pull_sources.insert(source.to_owned());
                }
            }
        }
    }

    let mut sources = Vec::new();
    let mut rendered_sources = std::collections::BTreeSet::new();
    for connector in group_outputs {
        let Some(output) = outputs.iter().find(|o| o.connector == *connector) else {
            continue;
        };
        if output.role.is_mirror() {
            continue;
        }
        let dmg = match source_damage(output) {
            Some(dmg) => Some(dmg),
            None if mirror_pull_sources.contains(connector) => Some(FrameDamage::Full),
            None => None,
        };
        if let Some(dmg) = dmg {
            rendered_sources.insert(connector.clone());
            sources.push((connector.clone(), dmg));
        }
    }

    let mut mirrors = Vec::new();
    for connector in group_outputs {
        let Some(output) = outputs.iter().find(|o| o.connector == *connector) else {
            continue;
        };
        if let Some(source) = output.role.mirror_source()
            && (rendered_sources.contains(source) || cursor_or_init_mirrors.contains(connector))
        {
            mirrors.push(connector.clone());
        }
    }

    GroupRenderPlan { sources, mirrors }
}

impl CompositorRuntime<'_> {
    /// Synchronize both the presentation backend (KMS page-flip barrier on DRM)
    /// and the Vulkan device when an unmerged fence wait times out or fails
    /// (ADR-0264, `[INV-ARCH-159]`).
    fn wait_unmerged_or_sync_all(&mut self, fence: &CompletionFence) {
        if fence.wait_all(UNMERGED_FENCE_WAIT_TIMEOUT) {
            return;
        }
        log::warn!(
            "{}: unmerged completion fence wait did not finish cleanly; synchronizing backend and Vulkan device",
            self.host.name()
        );
        if let Err(error) = self.host.wait_presented(self.device) {
            log::warn!(
                "{}: backend presentation wait failed during fence fallback: {error}",
                self.host.name()
            );
        }
        self.device.wait_idle();
    }

    /// Release retired client buffers immediately after a composited frame
    /// for `presented_outputs` (`None` = whole desktop) using the most accurate
    /// completion fence available (ADR-0250, ADR-0264, ADR-0265,
    /// `[INV-ARCH-159]`).
    ///
    /// Retains the remaining live completion fence per output in
    /// `self.retained_output_fences` so sibling outputs with independent pacing
    /// are never clobbered and subsequent `NoDamage` frames release retired
    /// buffers immediately.
    pub(super) fn release_retired_buffers_after_composited_frame(
        &mut self,
        presented_outputs: Option<&[&str]>,
        completion_fence: CompletionFence,
    ) {
        let mut completion_fence = completion_fence.prune_signaled();
        let completion_state = CompletionState::from_fence(&completion_fence);
        let retired_pending = self
            .server
            .retired_buffers_pending_on_outputs(presented_outputs);
        match plan_retired_release(
            retired_pending,
            self.host.presents_through_host_swapchain(),
            completion_state,
        ) {
            RetiredReleaseAction::None => {}
            RetiredReleaseAction::ReleaseImmediate => {
                self.server
                    .release_retired_buffers_on_outputs(presented_outputs, None);
            }
            RetiredReleaseAction::ReleaseWithFence => {
                self.server.release_retired_buffers_on_outputs(
                    presented_outputs,
                    completion_fence.as_raw_fd(),
                );
            }
            RetiredReleaseAction::WaitUnmergedAndRelease => {
                self.wait_unmerged_or_sync_all(&completion_fence);
                self.server
                    .release_retired_buffers_on_outputs(presented_outputs, None);
                completion_fence = CompletionFence::Signaled;
            }
            RetiredReleaseAction::WaitDeviceAndRelease => {
                self.device.wait_idle();
                self.server
                    .release_retired_buffers_on_outputs(presented_outputs, None);
                completion_fence = CompletionFence::Signaled;
            }
        }
        if self.host.presents_through_host_swapchain() {
            self.retained_output_fences.clear();
        } else {
            self.retained_output_fences
                .record(presented_outputs, completion_fence);
        }
    }

    /// Release retired client buffers after a direct-scanout commit for
    /// `presented_outputs` using its KMS completion fence, failing safe if
    /// multi-CRTC out-fences could not be merged (ADR-0264, ADR-0265,
    /// `[INV-ARCH-159]`).
    pub(super) fn release_retired_buffers_after_direct_scanout(
        &mut self,
        presented_outputs: &[&str],
        completion_fence: CompletionFence,
    ) {
        let mut completion_fence = completion_fence.prune_signaled();
        if self
            .server
            .retired_buffers_pending_on_outputs(Some(presented_outputs))
        {
            match &completion_fence {
                CompletionFence::Signaled => {
                    self.server
                        .release_retired_buffers_on_outputs(Some(presented_outputs), None);
                }
                CompletionFence::Live(_) => {
                    self.server.release_retired_buffers_on_outputs(
                        Some(presented_outputs),
                        completion_fence.as_raw_fd(),
                    );
                }
                CompletionFence::Unmerged(_) => {
                    self.wait_unmerged_or_sync_all(&completion_fence);
                    self.server
                        .release_retired_buffers_on_outputs(Some(presented_outputs), None);
                    completion_fence = CompletionFence::Signaled;
                }
            }
        }
        self.retained_output_fences
            .record(Some(presented_outputs), completion_fence);
    }

    /// Release retired client buffers when no new composited frame was
    /// submitted (e.g. `NoDamage` fast-exit or cursor-only mirror update).
    ///
    /// Collects only the retained fences for the outputs that actually have
    /// pending retired buffers so sibling outputs remain isolated (ADR-0250,
    /// ADR-0264, ADR-0265, `[INV-ARCH-159]`).
    pub(super) fn release_idle_retired_buffers(&mut self) {
        if !self.server.retired_buffers_pending() {
            return;
        }
        if self.host.presents_through_host_swapchain() {
            self.device.wait_idle();
            self.server.release_retired_buffers(None);
            return;
        }
        let (has_unscoped, pending_outputs) = self.server.retired_buffer_pending_outputs();
        let retained = self
            .retained_output_fences
            .completion_for_idle(has_unscoped, &pending_outputs);
        let completion_state = CompletionState::from_fence(&retained);
        match plan_retired_release(true, false, completion_state) {
            RetiredReleaseAction::None => {}
            RetiredReleaseAction::ReleaseImmediate => {
                self.server.release_retired_buffers(None);
            }
            RetiredReleaseAction::ReleaseWithFence => {
                self.server.release_retired_buffers(retained.as_raw_fd());
            }
            RetiredReleaseAction::WaitUnmergedAndRelease => {
                self.wait_unmerged_or_sync_all(&retained);
                self.server.release_retired_buffers(None);
            }
            RetiredReleaseAction::WaitDeviceAndRelease => {
                self.device.wait_idle();
                self.server.release_retired_buffers(None);
            }
        }
    }
    /// Whether the normal frame can use the per-output target path.
    ///
    /// Capture, screenshot freeze, streams, and session lock still compose into
    /// the union surface before presenting through targets. Active KMS overlay
    /// offload runs natively on per-output targets (ADR-0268).
    pub(super) fn can_render_output_targets(&self) -> bool {
        if self.host.name() != "drm" || !self.host.is_active() {
            return false;
        }
        if self.server.session_locked()
            || self.screenshot_freeze.armed
            || self.screenshot_freeze.active()
            || self.pending_capture.is_some()
            || self.pending_interaction_domain_capture.is_some()
            || self.pending_window_capture.is_some()
            || self.pending_pick.is_some()
            || self.streams.any_output_live()
            || self.server.lock_confirmation_pending()
        {
            return false;
        }
        self.output_targets_cover_all_physical()
    }

    /// Map union-physical damage onto one output target's local pixel
    /// coordinates, or return `None` when this output has no changed pixels and
    /// does not owe an initial presentation.
    fn target_damage(
        &self,
        output: &tessera_desktop::output::OutputInfo,
        target: &OutputTarget,
        damage: &FrameDamage,
    ) -> Option<FrameDamage> {
        if target.needs_present || matches!(damage, FrameDamage::Full) {
            return Some(FrameDamage::Full);
        }
        let Some(source) = self.host.output_source_rect(&output.connector) else {
            return Some(FrameDamage::Full);
        };
        target_damage_frame(damage, source, target.mode)
    }

    /// Present a composed union image through every physical output target.
    ///
    /// Capture, screenshot freeze, and active streams need one desktop-wide
    /// image as their source. This method keeps that image as a capture
    /// composition while each CRTC still presents a mode-sized target with
    /// independent flip/callback scope. It returns the non-mirror connectors
    /// that reached KMS; mirrors are presentation aliases and do not receive a
    /// second callback.
    pub(super) fn present_union_to_targets(
        &mut self,
        cursor_shape: u32,
        cursor_hidden: bool,
    ) -> Result<(Vec<String>, CompletionFence), Box<dyn std::error::Error>> {
        let Some(presented) = self.host.presented_dmabuf() else {
            return Ok((Vec::new(), CompletionFence::Signaled));
        };
        let groups = self.presentation_groups().to_vec();
        let outputs = self.server.output_infos();
        let software_cursor = self.host.uses_software_cursor();
        let explicit_sync = self.host.explicit_sync();
        let cursor = self.input_acc.cursor;
        let format = gfx_format_for_drm(presented.fourcc);

        let mut targets = std::mem::take(&mut self.output_targets);
        let mut presented_sources = Vec::new();
        let mut submitted_groups = Vec::new();
        let mut blocked_groups = Vec::new();
        let mut kms_fences: Vec<OwnedFd> = Vec::new();

        for group in &groups {
            if group
                .outputs
                .iter()
                .any(|connector| self.host.output_presentation_pending(connector))
            {
                blocked_groups.push(group.outputs.clone());
                continue;
            }
            let mut group_targets = GroupTargets::take_from(&mut targets, &group.outputs);
            let mut permanently_failed = std::collections::BTreeSet::new();
            let mut group_ok = true;

            {
                let mut submitted_frames: Vec<(usize, optics_gfx::SubmittedFrame<'_>)> =
                    Vec::with_capacity(group.outputs.len());
                let mut commit_batch: Vec<RenderTargetCommit> =
                    Vec::with_capacity(group.outputs.len());

                for connector in &group.outputs {
                    let Some(idx) = group_targets.index_of(connector) else {
                        group_ok = false;
                        break;
                    };
                    let Some(output) = outputs.iter().find(|o| o.connector == *connector) else {
                        permanently_failed.insert(connector.clone());
                        group_ok = false;
                        break;
                    };
                    let Some(source_connector) =
                        output.role.mirror_source().or(Some(&output.connector))
                    else {
                        group_ok = false;
                        break;
                    };
                    let Some(source_rect) = self.host.output_source_rect(source_connector) else {
                        permanently_failed.insert(connector.clone());
                        group_ok = false;
                        break;
                    };
                    let mode = group_targets.meta[idx].mode;
                    group_targets.meta[idx].import_cache.begin_frame();
                    let (image, pending_acquire_fence) = match group_targets.meta[idx]
                        .import_cache
                        .get_or_import(
                            self.device,
                            TargetImportDesc {
                                fd: &presented.fd,
                                width: presented.width,
                                height: presented.height,
                                stride: presented.stride,
                                modifier: presented.modifier,
                                format,
                                color_pipeline: presented.color_pipeline,
                                acquire_fence: presented.acquire_fence.as_ref(),
                            },
                        ) {
                        Ok(imported) => imported,
                        Err(error) => {
                            log::warn!(
                                "{}: cannot import composed image for target {connector}: {error}",
                                self.host.name()
                            );
                            permanently_failed.insert(connector.clone());
                            group_ok = false;
                            break;
                        }
                    };
                    let (fx, fy, fw, fh) = if output.role.is_mirror() {
                        output.content_fraction()
                    } else {
                        (0.0, 0.0, 1.0, 1.0)
                    };
                    let dst_x = fx * mode.0 as f32;
                    let dst_y = fy * mode.1 as f32;
                    let dst_w = fw * mode.0 as f32;
                    let dst_h = fh * mode.1 as f32;
                    let full_w = presented.width.max(1) as f32;
                    let full_h = presented.height.max(1) as f32;
                    let mut frame = match group_targets.surfaces[idx]
                        .begin_frame_timeout_with_damage(
                            TARGET_ACQUIRE_TIMEOUT_NS,
                            &optics_gfx::Damage::Full,
                        ) {
                        Ok(frame) => frame,
                        Err(error) => {
                            log::debug!(
                                "{}: capture target frame unavailable for {connector}: {error}",
                                self.host.name()
                            );
                            if !gfx_target_acquire_is_transient(&error) {
                                permanently_failed.insert(connector.clone());
                            }
                            group_ok = false;
                            break;
                        }
                    };
                    let submitted = (|| -> Result<optics_gfx::SubmittedFrame<'_>, String> {
                        let session = group_targets.canvases[idx]
                            .begin_session(
                                &mut frame,
                                optics_gfx::CanvasPassOptions {
                                    clear: Some(optics_gfx::rgba(0, 0, 0, 255)),
                                    damage: Vec::new(),
                                },
                            )
                            .map_err(|error| error.to_string())?;
                        if let Some(fence) = pending_acquire_fence {
                            session
                                .wait_dmabuf_acquire(image, fence)
                                .map_err(|error| error.to_string())?;
                        }
                        session.draw_image_with(
                            image,
                            (dst_x, dst_y, dst_w, dst_h),
                            &optics_gfx::ImageDrawOpts {
                                src: Some((
                                    source_rect.origin.x as f32 / full_w,
                                    source_rect.origin.y as f32 / full_h,
                                    source_rect.size.w.max(1) as f32 / full_w,
                                    source_rect.size.h.max(1) as f32 / full_h,
                                )),
                                opaque_only: true,
                                ..Default::default()
                            },
                        );
                        if output.role.is_mirror() && !software_cursor && !cursor_hidden {
                            let source_scale = outputs
                                .iter()
                                .find(|candidate| candidate.connector == *source_connector)
                                .map_or(1.0, |source| source.geometry.scale.as_f32());
                            if let Some(cursor_sprite) =
                                self.cursor_cache
                                    .get(self.device, cursor_shape, source_scale)
                            {
                                let fit = dst_w / source_rect.size.w.max(1) as f32;
                                let origin = outputs
                                    .iter()
                                    .find(|candidate| candidate.connector == *source_connector)
                                    .map_or(tessera_primitives::Point::default(), |source| {
                                        source.geometry.logical_origin
                                    });
                                let cursor_x =
                                    dst_x + (cursor.0 - origin.x as f32) * source_scale * fit;
                                let cursor_y =
                                    dst_y + (cursor.1 - origin.y as f32) * source_scale * fit;
                                session.draw_image(
                                    &cursor_sprite.image,
                                    cursor_x - cursor_sprite.xhot * fit,
                                    cursor_y - cursor_sprite.yhot * fit,
                                    cursor_sprite.width * fit,
                                    cursor_sprite.height * fit,
                                );
                            }
                        }
                        session.end().map_err(|error| error.to_string())?;
                        frame.submit().map_err(|error| error.to_string())
                    })();
                    let submitted = match submitted {
                        Ok(submitted) => submitted,
                        Err(error) => {
                            log::warn!(
                                "{}: capture target render failed for {connector}: {error}",
                                self.host.name()
                            );
                            permanently_failed.insert(connector.clone());
                            group_ok = false;
                            break;
                        }
                    };
                    let exported =
                        match export_target_dmabuf(&group_targets.surfaces[idx], explicit_sync) {
                            Ok(exported) => exported,
                            Err(error) => {
                                log::warn!(
                                    "{}: capture target export failed for {connector}: {error}",
                                    self.host.name()
                                );
                                permanently_failed.insert(connector.clone());
                                group_ok = false;
                                break;
                            }
                        };
                    submitted_frames.push((idx, submitted));
                    commit_batch.push(RenderTargetCommit::from_exported(
                        connector.clone(),
                        exported,
                    ));
                }

                if group_ok && !commit_batch.is_empty() {
                    match self.host.present_render_targets(commit_batch) {
                        Ok(kms_fence) => {
                            kms_fence.append_to(&mut kms_fences);
                            for (idx, submitted) in submitted_frames {
                                if let Err(error) = submitted.present() {
                                    log::warn!(
                                        "{}: capture target ring advance failed for {}: {error}",
                                        self.host.name(),
                                        group_targets.meta[idx].connector
                                    );
                                    permanently_failed
                                        .insert(group_targets.meta[idx].connector.clone());
                                } else {
                                    group_targets.meta[idx].needs_present = false;
                                    group_targets.meta[idx].last_source_export = None;
                                    group_targets.meta[idx].last_mirror_cursor_rect = None;
                                }
                            }
                            submitted_groups.push(group.outputs.clone());
                            presented_sources.extend(
                                group
                                    .outputs
                                    .iter()
                                    .filter(|connector| {
                                        outputs.iter().any(|output| {
                                            output.connector == **connector
                                                && !output.role.is_mirror()
                                        })
                                    })
                                    .cloned(),
                            );
                        }
                        Err(error) => {
                            drop(submitted_frames);
                            if drm_target_commit_is_transient(&error) {
                                log::debug!(
                                    "{}: capture target group commit deferred: {error}",
                                    self.host.name()
                                );
                            } else {
                                log::warn!(
                                    "{}: capture target group commit failed: {error}",
                                    self.host.name()
                                );
                                permanently_failed.extend(group.outputs.iter().cloned());
                            }
                        }
                    }
                }
            }

            group_targets.return_to(&mut targets, &permanently_failed);
        }

        self.output_targets = targets;
        if groups.len() > 1 {
            if !submitted_groups.is_empty() {
                self.vblank_wait_groups = submitted_groups;
            } else if !blocked_groups.is_empty() {
                self.vblank_wait_groups = blocked_groups;
            }
        } else {
            self.vblank_wait_groups.clear();
        }
        Ok((presented_sources, merge_live_sync_fences(kms_fences)))
    }

    /// Whether every physical output has a compositor target.
    pub(super) fn output_targets_cover_all_physical(&self) -> bool {
        let physical = self
            .server
            .output_infos()
            .into_iter()
            .filter(|output| output.kind.is_physical())
            .collect::<Vec<_>>();
        !physical.is_empty()
            && physical
                .iter()
                .all(|output| self.output_targets.contains_key(&output.connector))
    }

    /// Whether a union-composed frame (capture, freeze, stream, or fallback
    /// chrome state) can present through every physical output target.
    pub(super) fn can_present_union_targets(&self) -> bool {
        if self.host.name() != "drm" || self.overlay_state.active().is_some() {
            return false;
        }
        self.output_targets_cover_all_physical()
    }

    /// Resolve the live DMA-BUF descriptor for the currently active KMS overlay
    /// placement, if its surface is still mapped and visible.
    pub(super) fn active_overlay_dmabuf(
        &self,
    ) -> Option<(
        tessera_composite::presentation::OverlayPlacement,
        tessera_primitives::SurfaceDmabuf,
    )> {
        let active = self.overlay_state.active()?.clone();
        let sets = self.server.desktop_frame_sets();
        let frame = self
            .server
            .toplevel_dmabuf_frames_with(&sets.visible, &sets.occluded)
            .into_iter()
            .find(|frame| frame.id == active.surface_id)?;
        Some((active, frame))
    }

    /// Render and present the ordinary desktop through per-output targets.
    ///
    /// Processes each presentation group independently:
    /// - Groups with an in-flight KMS flip are skipped without invalidating
    ///   their damage history or tearing down their targets.
    /// - Undamaged groups (and undamaged peers inside a multi-source group)
    ///   skip rendering cleanly.
    /// - Source targets use gfx's native damage ledger (`repaint_scope()`)
    ///   and single-pass desktop rendering (`begin_desktop_output`).
    /// - `SubmittedFrame`s are held until `present_render_targets()` accepts
    ///   the atomic commit; transient KMS/acquire errors abandon the slot
    ///   without advancing the ring or destroying Vulkan surfaces.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_output_targets(
        &mut self,
        damage: &FrameDamage,
        primary_damage: &FrameDamage,
        overlay_buffer_changed: bool,
        sampled_surface_ids: &[usize],
        cursor_shape: u32,
        cursor_hidden: bool,
        client_cursor_on_hw_plane: bool,
        client_cursor_key: Option<(usize, u64)>,
        last_committed_hotspot: (u32, u32),
        last_committed_sprite_size: (u32, u32),
        mirror_cursor_dirty: bool,
        window_switcher: Option<&tessera_shell::component::WindowSwitcherPresentation>,
        live_previews: &[tessera_shell::component::LivePreviewPresentation],
    ) -> Result<PresentationOutcome, Box<dyn std::error::Error>> {
        let overlay_pair = self.active_overlay_dmabuf();
        if self.overlay_state.active().is_some() && overlay_pair.is_none() {
            self.overlay_state.deactivate();
            self.damage.force_full_redraw = true;
            self.invalidate_all_surface_damage();
            return Ok(PresentationOutcome::Retry {
                wait: RetryWait::BackendEvent,
            });
        }
        let (active_overlay, overlay_frame) = match overlay_pair {
            Some((active, frame)) => (Some(active), Some(frame)),
            None => (None, None),
        };
        let excluded_overlay_window = active_overlay.as_ref().map(|overlay| overlay.window);
        let target_primary_damage = if active_overlay.is_some() {
            primary_damage
        } else {
            damage
        };

        let groups = self.presentation_groups().to_vec();
        let outputs = self.server.output_infos();
        let primary = outputs
            .iter()
            .find(|output| output.kind.is_physical() && !output.role.is_mirror())
            .map(|output| output.connector.clone());
        let mirrored_sources: std::collections::BTreeSet<String> = outputs
            .iter()
            .filter_map(|output| output.role.mirror_source().map(str::to_owned))
            .collect();
        let color_scheme = self.shell.design().scheme;
        let shadow_style = self
            .config
            .as_ref()
            .map(|config| config.ui.window_shadow)
            .unwrap_or_default();
        let overview_progress = self.shell.overview_progress();
        let overview_active = self.shell.overview_active() || overview_progress > 0.001;
        let wallpaper_occluded = !overview_active
            && !self.server.workspace_slide_pending()
            && !self.server.transitions_pending()
            && self.space_use.client_owns_output();
        let model_active = !wallpaper_occluded
            && self
                .wallpaper
                .as_ref()
                .is_some_and(wallpaper::Wallpaper::has_model);
        let drew_preview_sources = overview_active
            || window_switcher.is_some()
            || !live_previews.is_empty()
            || self.server.workspace_slide_pending();
        let special_full = drew_preview_sources || model_active;
        let shell_composes = self.shell.requires_composition();
        let backdrop_material = self.shell.design().material;
        let software_cursor = self.host.uses_software_cursor();
        let explicit_sync = self.host.explicit_sync();
        let cursor = self.input_acc.cursor;

        let mut blocked_groups: Vec<Vec<String>> = Vec::new();
        let mut submitted_groups: Vec<Vec<String>> = Vec::new();
        let mut presented_sources: Vec<String> = Vec::new();
        let mut committed_fences: Vec<OwnedFd> = Vec::new();
        let mut transient_retry: Option<RetryWait> = None;
        let mut any_permanent_failure = false;
        let mut cursor_fallback = false;
        let mut reconfigured_or_inactive = false;
        let mut overlay_rejected = false;
        let mut began_renderer_frame = false;

        let mut targets = std::mem::take(&mut self.output_targets);

        for group in &groups {
            if group
                .outputs
                .iter()
                .any(|connector| self.host.output_presentation_pending(connector))
            {
                blocked_groups.push(group.outputs.clone());
                continue;
            }

            let plan = plan_group_render(
                &group.outputs,
                &outputs,
                |output| {
                    let target = targets.get(&output.connector)?;
                    if special_full {
                        Some(FrameDamage::Full)
                    } else {
                        let damage = self.target_damage(output, target, target_primary_damage)?;
                        // Chrome is composited on the primary only; its
                        // frosted / glass bodies repaint when anything they
                        // sample changed (ADR-0253).
                        if primary.as_deref() == Some(output.connector.as_str())
                            && let Some(plan) = backdrop::plan(
                                backdrop_material,
                                self.shell.backdrop_bodies(),
                                output.geometry.scale.as_f32().max(0.01),
                                target.mode,
                            )
                        {
                            Some(plan.widen(damage))
                        } else {
                            Some(damage)
                        }
                    }
                },
                |connector| {
                    targets
                        .get(connector)
                        .is_some_and(|target| target.needs_present)
                },
                mirror_cursor_dirty,
                |source| {
                    targets
                        .get(source)
                        .is_some_and(|target| target.last_source_export.is_some())
                },
            );
            if plan.sources.is_empty() && plan.mirrors.is_empty() {
                // Multi-output steady-state overlay-only update (ADR-0268): if
                // this group's Primary pixels did not change while another
                // group rendered, swap only the overlay plane here.
                if overlay_buffer_changed
                    && let (Some(active), Some(surface)) =
                        (active_overlay.as_ref(), overlay_frame.as_ref())
                    && group.outputs.contains(&active.output)
                {
                    match self
                        .host
                        .present_overlay_only(tessera_platform::host::OverlayLayer {
                            surface,
                            output: &active.output,
                            target: active.target,
                            zpos: active.zpos,
                        }) {
                        Ok(kms_fence) => {
                            kms_fence.append_to(&mut committed_fences);
                            presented_sources.push(active.output.clone());
                            submitted_groups.push(vec![active.output.clone()]);
                        }
                        Err(HostError::Drm(DrmError::ScanoutUnsupported)) => {
                            overlay_rejected = true;
                        }
                        Err(error) => match classify_target_commit_error(&error) {
                            TargetCommitFailure::CursorFallback => cursor_fallback = true,
                            TargetCommitFailure::ReconfiguredOrInactive => {
                                reconfigured_or_inactive = true;
                            }
                            TargetCommitFailure::TransientBusy => {
                                transient_retry = Some(RetryWait::BackendEvent);
                            }
                            TargetCommitFailure::Permanent => {
                                overlay_rejected = true;
                            }
                        },
                    }
                }
                continue;
            }

            if !began_renderer_frame {
                self.direct_outputs.clear();
                self.renderer.begin_frame();
                began_renderer_frame = true;
            }

            let active_connectors: Vec<String> = plan
                .sources
                .iter()
                .map(|(connector, _)| connector.clone())
                .chain(plan.mirrors.iter().cloned())
                .collect();
            let mut group_targets = GroupTargets::take_from(&mut targets, &active_connectors);
            let mut permanently_failed = std::collections::BTreeSet::new();
            let mut group_ok = true;

            {
                let mut submitted_frames: Vec<(usize, optics_gfx::SubmittedFrame<'_>)> =
                    Vec::with_capacity(active_connectors.len());
                let mut rendered_sources: Vec<RenderedSource> =
                    Vec::with_capacity(plan.sources.len());
                let mut mirror_commits: Vec<RenderTargetCommit> =
                    Vec::with_capacity(plan.mirrors.len());
                let mut mirror_updates: Vec<(
                    usize,
                    std::rc::Rc<optics_gfx::DmabufLease>,
                    Option<tessera_primitives::Rect>,
                )> = Vec::with_capacity(plan.mirrors.len());

                // 1. Extended/source targets in this group.
                for (connector, target_current_damage) in &plan.sources {
                    let Some(idx) = group_targets.index_of(connector) else {
                        group_ok = false;
                        break;
                    };
                    let Some(output) = outputs.iter().find(|o| o.connector == *connector) else {
                        permanently_failed.insert(connector.clone());
                        group_ok = false;
                        break;
                    };
                    let mode = group_targets.meta[idx].mode;
                    let logical = output.geometry.logical_size();
                    let logical_size = (logical.w.max(1) as u32, logical.h.max(1) as u32);
                    let scale = output.geometry.scale.as_f32().max(0.01);
                    let origin = output.geometry.logical_origin;
                    let gfx_damage = target_current_damage.to_gfx_damage();
                    let mut frame = match group_targets.surfaces[idx]
                        .begin_frame_timeout_with_damage(TARGET_ACQUIRE_TIMEOUT_NS, &gfx_damage)
                    {
                        Ok(frame) => frame,
                        Err(error) => {
                            log::debug!(
                                "{}: target frame unavailable for {connector}: {error}",
                                self.host.name()
                            );
                            if error.0 == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY {
                                transient_retry = Some(RetryWait::BackendEvent);
                            } else if error.0
                                == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_TIMEOUT
                            {
                                transient_retry.get_or_insert(RetryWait::Deadline);
                            } else {
                                permanently_failed.insert(connector.clone());
                            }
                            group_ok = false;
                            break;
                        }
                    };
                    let repaint = match frame.repaint_scope() {
                        Ok(scope) => FrameDamage::from_gfx_damage(scope),
                        Err(error) => {
                            log::warn!(
                                "{}: repaint_scope query failed for {connector}: {error}",
                                self.host.name()
                            );
                            FrameDamage::Full
                        }
                    };
                    let canvas_damage = frame_damage_scope(&repaint, mode);
                    let chrome_in_damage_scope = if canvas_damage.is_empty() {
                        true
                    } else {
                        canvas_damage.iter().any(|area| {
                            self.shell
                                .chrome_intersects_render_area(*area, scale, logical_size)
                        })
                    };
                    let windows = if overview_active {
                        self.server.render_windows()
                    } else {
                        self.server.unoccluded_render_windows()
                    };
                    let rendered_shadows =
                        if shadow_style == tessera_desktop::window::WindowShadowStyle::Soft {
                            self.window_shadows.prepare(
                                self.device,
                                &mut group_targets.canvases[idx],
                                &mut frame,
                                &windows,
                                scale,
                                shadow_style,
                                target_namespace(connector),
                            )
                        } else {
                            Vec::new()
                        };
                    let soft_shadow_entries: Vec<tessera_composite::SoftShadowEntry<'_>> =
                        rendered_shadows
                            .iter()
                            .map(|shadow| tessera_composite::SoftShadowEntry {
                                window: shadow.window,
                                raw: shadow.raw,
                                _borrow: std::marker::PhantomData,
                                x: shadow.x,
                                y: shadow.y,
                                w: shadow.w,
                                h: shadow.h,
                            })
                            .collect();
                    let soft_shadow_layer = (!soft_shadow_entries.is_empty()).then_some(
                        tessera_composite::SoftShadowLayer {
                            entries: soft_shadow_entries.as_slice(),
                        },
                    );
                    let is_primary = primary.as_deref() == Some(connector.as_str());
                    // The backdrop beneath frosted / glass chrome (ADR-0253),
                    // captured and filtered at this pass boundary.
                    let backdrop_plan = if is_primary && shell_composes && chrome_in_damage_scope {
                        backdrop::plan(backdrop_material, self.shell.backdrop_bodies(), scale, mode)
                            .and_then(|plan| plan.restrict_to(&canvas_damage))
                    } else {
                        None
                    };
                    let namespace = target_namespace(connector);
                    let backdrop_slab = match backdrop_plan.as_ref() {
                        Some(plan) => {
                            let captured = match self.backdrop.capture_target(
                                self.device,
                                namespace,
                                frame.index() as usize,
                                plan,
                            ) {
                                Some(capture) => backdrop::capture_desktop(
                                    &mut group_targets.canvases[idx],
                                    self.device,
                                    &mut frame,
                                    capture,
                                    &mut self.wallpaper,
                                    plan,
                                    logical_size,
                                    self.clear,
                                    !wallpaper_occluded,
                                    |canvas| {
                                        canvas.save();
                                        canvas.translate(-(origin.x as f32), -(origin.y as f32));
                                        let drawn = draw_direct_desktop_scene(
                                            canvas,
                                            self.device,
                                            self.renderer,
                                            self.server,
                                            RenderGeometry {
                                                logical_size,
                                                scale,
                                            },
                                            overview_active,
                                            overview_progress,
                                            window_switcher,
                                            color_scheme,
                                            soft_shadow_layer.as_ref(),
                                            shadow_style,
                                            excluded_overlay_window,
                                        );
                                        canvas.restore();
                                        drawn
                                    },
                                ),
                                None => Err(optics_gfx::Error(
                                    optics_gfx::sys::optics_gfx_result::OPTICS_GFX_ERROR_OUT_OF_MEMORY,
                                )),
                            };
                            match captured {
                                Ok(()) => {
                                    self.backdrop.compose(self.device, &frame, namespace, plan)
                                }
                                Err(error) => {
                                    self.backdrop.capture_failed(error);
                                    None
                                }
                            }
                        }
                        None => None,
                    };
                    let submitted = (|| -> Result<optics_gfx::SubmittedFrame<'_>, String> {
                        let session = super::presentation::begin_desktop_output(
                            &mut group_targets.canvases[idx],
                            self.device,
                            &mut frame,
                            &mut self.wallpaper,
                            logical_size,
                            scale,
                            self.clear,
                            canvas_damage,
                            model_active,
                            wallpaper_occluded,
                        )
                        .map_err(|error| error.to_string())?;
                        session.save();
                        session.translate(-(origin.x as f32), -(origin.y as f32));
                        draw_direct_desktop_scene(
                            &session,
                            self.device,
                            self.renderer,
                            self.server,
                            RenderGeometry {
                                logical_size,
                                scale,
                            },
                            overview_active,
                            overview_progress,
                            window_switcher,
                            color_scheme,
                            soft_shadow_layer.as_ref(),
                            shadow_style,
                            excluded_overlay_window,
                        )
                        .map_err(|error| error.to_string())?;
                        session.restore();
                        if let Some(slab) = backdrop_slab.as_ref() {
                            slab.draw(&session);
                        }
                        if is_primary && shell_composes && chrome_in_damage_scope {
                            let _uploads = self.device.uploads_begin();
                            if let Err(error) =
                                unsafe { self.shell.submit(session.as_raw() as *mut _) }
                            {
                                log::warn!(
                                    "compositor: shell chrome render failed on {connector}: {error}"
                                );
                            }
                        }
                        if is_primary {
                            if let Some(presentation) = window_switcher {
                                draw_window_switcher_cards(
                                    &session,
                                    self.device,
                                    self.renderer,
                                    self.server,
                                    scale,
                                    presentation,
                                );
                            } else {
                                draw_live_preview_scenes(
                                    &session,
                                    self.device,
                                    self.renderer,
                                    self.server,
                                    scale,
                                    live_previews,
                                );
                            }
                        }
                        session.save();
                        session.translate(-(origin.x as f32), -(origin.y as f32));
                        draw_client_overlays(
                            &session,
                            self.device,
                            self.renderer,
                            self.server,
                            scale,
                            !client_cursor_on_hw_plane,
                            None,
                        );
                        if software_cursor && !cursor_hidden {
                            draw_software_cursor(
                                &session,
                                self.device,
                                &mut self.cursor_cache,
                                cursor,
                                cursor_shape,
                                scale,
                            );
                        }
                        session.restore();
                        session.end().map_err(|error| error.to_string())?;
                        frame.submit().map_err(|error| error.to_string())
                    })();
                    let submitted = match submitted {
                        Ok(submitted) => submitted,
                        Err(error) => {
                            log::warn!(
                                "{}: target render failed for {connector}; rebuilding next frame: {error}",
                                self.host.name()
                            );
                            permanently_failed.insert(connector.clone());
                            group_ok = false;
                            break;
                        }
                    };
                    let exported =
                        match export_target_dmabuf(&group_targets.surfaces[idx], explicit_sync) {
                            Ok(exported) => exported,
                            Err(error) => {
                                log::warn!(
                                    "{}: target export failed for {connector}: {error}",
                                    self.host.name()
                                );
                                if error.0
                                    == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY
                                {
                                    transient_retry = Some(RetryWait::BackendEvent);
                                } else {
                                    permanently_failed.insert(connector.clone());
                                }
                                group_ok = false;
                                break;
                            }
                        };
                    submitted_frames.push((idx, submitted));
                    rendered_sources.push(RenderedSource {
                        connector: connector.clone(),
                        target_idx: idx,
                        format: group_targets.meta[idx].format,
                        color_pipeline: group_targets.meta[idx].color_pipeline,
                        dmabuf: exported.dmabuf,
                        lease: std::rc::Rc::new(exported.lease),
                        origin,
                        scale,
                        mode,
                        source_damage: target_current_damage.clone(),
                    });
                }

                // 2. Mirror targets in this group: sample cached source target
                //    with projected partial damage (ADR-0250, `[INV-ARCH-160]`).
                if group_ok {
                    for connector in &plan.mirrors {
                        let Some(idx) = group_targets.index_of(connector) else {
                            group_ok = false;
                            break;
                        };
                        let Some(output) = outputs.iter().find(|o| o.connector == *connector)
                        else {
                            permanently_failed.insert(connector.clone());
                            group_ok = false;
                            break;
                        };
                        let Some(source_connector) = output.role.mirror_source() else {
                            group_ok = false;
                            break;
                        };
                        let source_view = if let Some(source) = rendered_sources
                            .iter()
                            .find(|source| source.connector == source_connector)
                        {
                            MirrorSourceView {
                                fd: &source.dmabuf.fd,
                                acquire_fence: source.dmabuf.acquire_fence.as_ref(),
                                width: source.dmabuf.width,
                                height: source.dmabuf.height,
                                stride: source.dmabuf.stride,
                                modifier: source.dmabuf.modifier,
                                format: source.format,
                                color_pipeline: source.color_pipeline,
                                origin: source.origin,
                                scale: source.scale,
                                mode: source.mode,
                                lease: &source.lease,
                                source_damage: Some(&source.source_damage),
                            }
                        } else if let Some(retained) = targets
                            .get(source_connector)
                            .and_then(|target| target.last_source_export.as_ref())
                        {
                            MirrorSourceView {
                                fd: &retained.fd,
                                acquire_fence: retained.acquire_fence.as_ref(),
                                width: retained.width,
                                height: retained.height,
                                stride: retained.stride,
                                modifier: retained.modifier,
                                format: retained.format,
                                color_pipeline: retained.color_pipeline,
                                origin: retained.origin,
                                scale: retained.scale,
                                mode: retained.mode,
                                lease: &retained.lease,
                                source_damage: None,
                            }
                        } else {
                            group_ok = false;
                            break;
                        };
                        let mode = group_targets.meta[idx].mode;
                        let (fx, fy, fw, fh) = output.content_fraction();
                        let (dst_x, dst_y) = (fx * mode.0 as f32, fy * mode.1 as f32);
                        let (dst_w, dst_h) = (fw * mode.0 as f32, fh * mode.1 as f32);
                        let source_mode = source_view.mode;
                        let fit = dst_w / source_mode.0.max(1) as f32;
                        let cursor_draw = if !software_cursor && !cursor_hidden {
                            self.cursor_cache
                                .get(self.device, cursor_shape, source_view.scale)
                                .map(|cursor_sprite| {
                                    let cursor_x = dst_x
                                        + (cursor.0 - source_view.origin.x as f32)
                                            * source_view.scale
                                            * fit;
                                    let cursor_y = dst_y
                                        + (cursor.1 - source_view.origin.y as f32)
                                            * source_view.scale
                                            * fit;
                                    let draw_x = cursor_x - cursor_sprite.xhot * fit;
                                    let draw_y = cursor_y - cursor_sprite.yhot * fit;
                                    let draw_w = cursor_sprite.width * fit;
                                    let draw_h = cursor_sprite.height * fit;
                                    let bounds =
                                        mirror_cursor_bounds(draw_x, draw_y, draw_w, draw_h, mode);
                                    (cursor_sprite, draw_x, draw_y, draw_w, draw_h, bounds)
                                })
                        } else {
                            None
                        };
                        let new_mirror_cursor_rect = cursor_draw.and_then(|(_, _, _, _, _, b)| b);
                        let mirror_current_damage = compute_mirror_damage(
                            group_targets.meta[idx].needs_present,
                            source_view.source_damage,
                            source_mode,
                            (dst_x, dst_y, dst_w, dst_h),
                            mode,
                            !software_cursor,
                            mirror_cursor_dirty,
                            group_targets.meta[idx].last_mirror_cursor_rect,
                            new_mirror_cursor_rect,
                        );
                        if matches!(mirror_current_damage, FrameDamage::None) {
                            continue;
                        }
                        let source_lease = std::rc::Rc::clone(source_view.lease);
                        group_targets.meta[idx].import_cache.begin_frame();
                        let (image, pending_acquire_fence) = match group_targets.meta[idx]
                            .import_cache
                            .get_or_import(
                                self.device,
                                TargetImportDesc {
                                    fd: source_view.fd,
                                    width: source_view.width,
                                    height: source_view.height,
                                    stride: source_view.stride,
                                    modifier: source_view.modifier,
                                    format: source_view.format,
                                    color_pipeline: source_view.color_pipeline,
                                    acquire_fence: source_view.acquire_fence,
                                },
                            ) {
                            Ok(imported) => imported,
                            Err(error) => {
                                log::warn!(
                                    "{}: cannot import source target for mirror {connector}: {error}",
                                    self.host.name()
                                );
                                permanently_failed.insert(connector.clone());
                                group_ok = false;
                                break;
                            }
                        };
                        let mirror_gfx_damage = mirror_current_damage.to_gfx_damage();
                        let mut frame = match group_targets.surfaces[idx]
                            .begin_frame_timeout_with_damage(
                                TARGET_ACQUIRE_TIMEOUT_NS,
                                &mirror_gfx_damage,
                            ) {
                            Ok(frame) => frame,
                            Err(error) => {
                                log::debug!(
                                    "{}: mirror target frame unavailable for {connector}: {error}",
                                    self.host.name()
                                );
                                if error.0
                                    == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY
                                {
                                    transient_retry = Some(RetryWait::BackendEvent);
                                } else if error.0
                                    == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_TIMEOUT
                                {
                                    transient_retry.get_or_insert(RetryWait::Deadline);
                                } else {
                                    permanently_failed.insert(connector.clone());
                                }
                                group_ok = false;
                                break;
                            }
                        };
                        let mirror_repaint = match frame.repaint_scope() {
                            Ok(scope) => FrameDamage::from_gfx_damage(scope),
                            Err(error) => {
                                log::warn!(
                                    "{}: mirror repaint_scope query failed for {connector}: {error}",
                                    self.host.name()
                                );
                                FrameDamage::Full
                            }
                        };
                        let mirror_canvas_damage = frame_damage_scope(&mirror_repaint, mode);
                        let submitted = (|| -> Result<optics_gfx::SubmittedFrame<'_>, String> {
                            let session = group_targets.canvases[idx]
                                .begin_session(
                                    &mut frame,
                                    optics_gfx::CanvasPassOptions {
                                        clear: Some(optics_gfx::rgba(0, 0, 0, 255)),
                                        damage: mirror_canvas_damage,
                                    },
                                )
                                .map_err(|error| error.to_string())?;
                            if let Some(fence) = pending_acquire_fence {
                                session
                                    .wait_dmabuf_acquire(image, fence)
                                    .map_err(|error| error.to_string())?;
                            }
                            session.draw_image_with(
                                image,
                                (dst_x, dst_y, dst_w, dst_h),
                                &optics_gfx::ImageDrawOpts {
                                    opaque_only: true,
                                    ..Default::default()
                                },
                            );
                            if let Some((cursor_sprite, draw_x, draw_y, draw_w, draw_h, _)) =
                                cursor_draw
                            {
                                session.draw_image(
                                    &cursor_sprite.image,
                                    draw_x,
                                    draw_y,
                                    draw_w,
                                    draw_h,
                                );
                            }
                            session.end().map_err(|error| error.to_string())?;
                            frame.submit().map_err(|error| error.to_string())
                        })();
                        let submitted = match submitted {
                            Ok(submitted) => submitted,
                            Err(error) => {
                                log::warn!(
                                    "{}: mirror target render failed for {connector}; rebuilding next frame: {error}",
                                    self.host.name()
                                );
                                permanently_failed.insert(connector.clone());
                                group_ok = false;
                                break;
                            }
                        };
                        let exported =
                            match export_target_dmabuf(&group_targets.surfaces[idx], explicit_sync)
                            {
                                Ok(exported) => exported,
                                Err(error) => {
                                    log::warn!(
                                        "{}: mirror target export failed for {connector}: {error}",
                                        self.host.name()
                                    );
                                    if error.0
                                        == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY
                                    {
                                        transient_retry = Some(RetryWait::BackendEvent);
                                    } else {
                                        permanently_failed.insert(connector.clone());
                                    }
                                    group_ok = false;
                                    break;
                                }
                            };
                        submitted_frames.push((idx, submitted));
                        mirror_commits.push(RenderTargetCommit::from_exported(
                            connector.clone(),
                            exported,
                        ));
                        mirror_updates.push((idx, source_lease, new_mirror_cursor_rect));
                    }
                }

                // 3. Atomic KMS commit for the active outputs in this group.
                if group_ok && (!rendered_sources.is_empty() || !mirror_commits.is_empty()) {
                    let mut commit_batch =
                        Vec::with_capacity(rendered_sources.len() + mirror_commits.len());
                    let mut source_connectors = Vec::with_capacity(rendered_sources.len());
                    let mut group_render_fences: Vec<OwnedFd> =
                        Vec::with_capacity(rendered_sources.len());
                    let mut pending_retained: Vec<(usize, RetainedSourceExport)> = Vec::new();
                    for source in rendered_sources {
                        if let Some(fence) = source.dmabuf.acquire_fence.as_ref()
                            && let Ok(dup) = fence.try_clone()
                        {
                            group_render_fences.push(dup);
                        }
                        if mirrored_sources.contains(&source.connector)
                            && let Ok(fd) = source.dmabuf.fd.try_clone()
                        {
                            let acquire_fence = source
                                .dmabuf
                                .acquire_fence
                                .as_ref()
                                .and_then(|fence| fence.try_clone().ok());
                            pending_retained.push((
                                source.target_idx,
                                RetainedSourceExport {
                                    fd,
                                    acquire_fence,
                                    width: source.dmabuf.width,
                                    height: source.dmabuf.height,
                                    stride: source.dmabuf.stride,
                                    modifier: source.dmabuf.modifier,
                                    format: source.format,
                                    color_pipeline: source.color_pipeline,
                                    origin: source.origin,
                                    scale: source.scale,
                                    mode: source.mode,
                                    lease: std::rc::Rc::clone(&source.lease),
                                },
                            ));
                        }
                        source_connectors.push(source.connector.clone());
                        commit_batch.push(RenderTargetCommit {
                            connector: source.connector,
                            dmabuf: source.dmabuf,
                            lease: source.lease,
                        });
                    }
                    commit_batch.extend(mirror_commits);
                    let committed_group_outputs: Vec<String> = commit_batch
                        .iter()
                        .map(|commit| commit.connector.clone())
                        .collect();

                    let group_overlay = match (active_overlay.as_ref(), overlay_frame.as_ref()) {
                        (Some(active), Some(surface))
                            if committed_group_outputs.contains(&active.output) =>
                        {
                            Some(tessera_platform::host::OverlayLayer {
                                surface,
                                output: &active.output,
                                target: active.target,
                                zpos: active.zpos,
                            })
                        }
                        _ => None,
                    };
                    let had_group_overlay = group_overlay.is_some();

                    match self
                        .host
                        .present_render_targets_with_overlay(commit_batch, group_overlay)
                    {
                        Ok(kms_fence) => {
                            kms_fence.append_to(&mut committed_fences);
                            committed_fences.extend(group_render_fences);
                            for (idx, submitted) in submitted_frames {
                                if let Err(error) = submitted.present() {
                                    log::warn!(
                                        "{}: target ring advance failed for {}: {error}",
                                        self.host.name(),
                                        group_targets.meta[idx].connector
                                    );
                                    permanently_failed
                                        .insert(group_targets.meta[idx].connector.clone());
                                } else {
                                    group_targets.meta[idx].needs_present = false;
                                }
                            }
                            for (idx, retained) in pending_retained {
                                if !permanently_failed.contains(&group_targets.meta[idx].connector)
                                {
                                    group_targets.meta[idx].last_source_export = Some(retained);
                                }
                            }
                            for (idx, lease, cursor_rect) in mirror_updates {
                                if !permanently_failed.contains(&group_targets.meta[idx].connector)
                                {
                                    group_targets.meta[idx].source_lease = Some(lease);
                                    group_targets.meta[idx].last_mirror_cursor_rect = cursor_rect;
                                }
                            }
                            presented_sources.extend(source_connectors);
                            submitted_groups.push(committed_group_outputs);
                        }
                        Err(error) => {
                            // Drop submitted frames without advancing the gfx
                            // ring so the abandoned slot is reused cleanly.
                            drop(submitted_frames);
                            if had_group_overlay
                                && matches!(error, HostError::Drm(DrmError::ScanoutUnsupported))
                            {
                                log::warn!(
                                    "{}: per-output overlay presentation rejected; falling back to composition: {error}",
                                    self.host.name()
                                );
                                overlay_rejected = true;
                            } else {
                                match classify_target_commit_error(&error) {
                                    TargetCommitFailure::CursorFallback => {
                                        log::warn!(
                                            "{}: hardware cursor rejected on target commit; repainting with software cursor",
                                            self.host.name()
                                        );
                                        cursor_fallback = true;
                                    }
                                    TargetCommitFailure::ReconfiguredOrInactive => {
                                        log::warn!(
                                            "{}: transient target commit failure; retrying: {error}",
                                            self.host.name()
                                        );
                                        reconfigured_or_inactive = true;
                                    }
                                    TargetCommitFailure::TransientBusy => {
                                        log::debug!(
                                            "{}: target group commit still busy; coalescing frame",
                                            self.host.name()
                                        );
                                        transient_retry = Some(RetryWait::BackendEvent);
                                    }
                                    TargetCommitFailure::Permanent => {
                                        log::warn!(
                                            "{}: target group commit failed: {error}",
                                            self.host.name()
                                        );
                                        permanently_failed.extend(committed_group_outputs);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !permanently_failed.is_empty() {
                any_permanent_failure = true;
            }
            group_targets.return_to(&mut targets, &permanently_failed);
        }

        self.output_targets = targets;

        if overlay_rejected {
            self.overlay_state.deactivate();
            self.damage.force_full_redraw = true;
            self.invalidate_all_surface_damage();
            return Ok(PresentationOutcome::Retry {
                wait: RetryWait::BackendEvent,
            });
        }
        if cursor_fallback {
            self.damage.reset_cursor_baseline();
            self.damage.force_full_redraw = true;
            self.invalidate_all_surface_damage();
            return Ok(PresentationOutcome::Retry {
                wait: RetryWait::BackendEvent,
            });
        }
        if reconfigured_or_inactive {
            self.damage.force_full_redraw = true;
            self.invalidate_all_surface_damage();
            return Ok(PresentationOutcome::Retry {
                wait: RetryWait::BackendEvent,
            });
        }

        if presented_sources.is_empty() && submitted_groups.is_empty() {
            let has_blocked = !blocked_groups.is_empty();
            if has_blocked || transient_retry.is_some() || any_permanent_failure {
                self.damage.pending_damage = Some(match self.damage.pending_damage.take() {
                    Some(pending) => union_frame_damage(pending, damage.clone()),
                    None => damage.clone(),
                });
                if groups.len() > 1 && has_blocked {
                    self.vblank_wait_groups = blocked_groups;
                }
                let wait = if has_blocked {
                    RetryWait::BackendEvent
                } else {
                    transient_retry.unwrap_or(RetryWait::BackendEvent)
                };
                return Ok(PresentationOutcome::Retry { wait });
            }
            // No output target intersected the damage rects: skip presentation,
            // and release any retired client buffers immediately using the
            // retained completion fence so swapchains never deadlock (ADR-0250,
            // `[INV-ARCH-159]`).
            self.release_idle_retired_buffers();
            let callbacks_sent = self.presentation.frame_callbacks_allowed()
                && self
                    .server
                    .send_frame_callbacks(self.server.monotonic_ms(), PresentedSurfaces::Nothing)
                    > 0;
            self.renderer.gc(self.server.live_surface_ids());
            if !began_renderer_frame {
                self.renderer.begin_frame();
            }
            return Ok(PresentationOutcome::NoDamage { callbacks_sent });
        }

        if groups.len() > 1 {
            self.vblank_wait_groups = submitted_groups;
        } else {
            self.vblank_wait_groups.clear();
        }

        if presented_sources.is_empty() {
            // Cursor-only mirror update: keep the source's hardware cursor
            // plane in lockstep when hardware cursor is active, pace clients
            // without claiming any surface was resampled, and release any
            // retired buffers that became idle.
            if mirror_cursor_dirty
                && self.host.supports_hardware_cursor()
                && let Err(HostError::Drm(DrmError::CursorFallback)) = self.host.present_cursor()
            {
                self.damage.force_full_redraw = true;
                self.invalidate_all_surface_damage();
            }
            self.release_idle_retired_buffers();
            self.server
                .send_frame_callbacks(self.server.monotonic_ms(), PresentedSurfaces::Nothing);
        } else {
            let mut effective_sampled: Vec<usize> = sampled_surface_ids.to_vec();
            if let Some(active) = active_overlay.as_ref()
                && presented_sources.contains(&active.output)
                && !effective_sampled.contains(&active.surface_id)
            {
                effective_sampled.push(active.surface_id);
            }
            let target_refs = presented_sources
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            self.primary_plane_state.commit_composited();
            self.server.acknowledge_presented_surface_damage_on_outputs(
                effective_sampled.iter(),
                drew_preview_sources,
                Some(&target_refs),
            );
            self.release_retired_buffers_after_composited_frame(
                Some(&target_refs),
                merge_live_sync_fences(committed_fences),
            );
            self.server.send_frame_callbacks_on_outputs(
                self.server.monotonic_ms(),
                PresentedSurfaces::Sampled {
                    ids: &effective_sampled,
                    drew_preview_sources,
                },
                Some(&target_refs),
            );
        }
        self.damage.last_present_minute = Some(wall_clock_minute());
        self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
        self.damage.last_presented_client_cursor = client_cursor_key;
        let primary_scale = outputs
            .iter()
            .find(|output| output.kind.is_physical() && !output.role.is_mirror())
            .map_or(self.host.scale(), |output| output.geometry.scale.as_f32());
        self.damage.last_presented_cursor_position = Some((
            (self.input_acc.cursor.0 * primary_scale).round() as i32,
            (self.input_acc.cursor.1 * primary_scale).round() as i32,
        ));
        let hw_cursor_visible = !cursor_hidden || client_cursor_on_hw_plane;
        self.damage.last_presented_cursor_hotspot = if client_cursor_on_hw_plane {
            Some(last_committed_hotspot)
        } else {
            hw_cursor_visible.then_some(last_hotspot(
                &mut self.cursor_cache,
                self.device,
                cursor_shape,
                primary_scale,
            ))
        };
        self.damage.last_presented_cursor_pixels = if client_cursor_on_hw_plane {
            Some(last_committed_sprite_size)
        } else {
            hw_cursor_visible.then_some(last_sprite(
                &mut self.cursor_cache,
                self.device,
                cursor_shape,
                primary_scale,
            ))
        };
        self.damage.force_full_redraw = false;
        self.renderer.gc(self.server.live_surface_ids());
        self.frame_count += 1;
        if self.frame_count == 1 {
            log::info!(
                "{}: first frame presented (per-output targets)",
                self.host.name()
            );
        }
        Ok(PresentationOutcome::Submitted)
    }
}

/// Clip union-physical damage to one target and translate it into target-local
/// `FrameDamage`. `None` means this target has no changed pixels.
fn target_damage_frame(
    damage: &FrameDamage,
    source: tessera_primitives::Rect,
    mode: (u32, u32),
) -> Option<FrameDamage> {
    if matches!(damage, FrameDamage::Full) {
        return Some(FrameDamage::Full);
    }
    let damage_rects = damage.area_rects()?;
    let mut local_rects = Vec::new();
    for rect in damage_rects {
        let Some(clipped) = rect.intersect(source) else {
            continue;
        };
        let x = clipped.origin.x - source.origin.x;
        let y = clipped.origin.y - source.origin.y;
        let width = clipped.size.w.min(mode.0 as i32 - x);
        let height = clipped.size.h.min(mode.1 as i32 - y);
        if x >= 0 && y >= 0 && width > 0 && height > 0 {
            local_rects.push(tessera_primitives::Rect::new(x, y, width, height));
        }
    }
    (!local_rects.is_empty()).then_some(FrameDamage::Area(local_rects))
}

/// Project source-target pixel damage into mirror-target physical pixels using
/// the mirror's fitted content rectangle, expanded by `MIRROR_DAMAGE_MARGIN_PX`
/// to cover bilinear filter taps (ADR-0250, `[INV-ARCH-160]`).
fn project_source_damage_to_mirror(
    source_damage: &FrameDamage,
    source_mode: (u32, u32),
    dst: (f32, f32, f32, f32),
    mirror_mode: (u32, u32),
) -> FrameDamage {
    match source_damage {
        FrameDamage::Full => FrameDamage::Full,
        FrameDamage::None => FrameDamage::None,
        FrameDamage::Area(rects) => {
            let (dst_x, dst_y, dst_w, dst_h) = dst;
            let sx = dst_w / source_mode.0.max(1) as f32;
            let sy = dst_h / source_mode.1.max(1) as f32;
            let max_w = mirror_mode.0 as i32;
            let max_h = mirror_mode.1 as i32;
            let mut projected = Vec::with_capacity(rects.len());
            for rect in rects {
                let x0 = ((dst_x + rect.origin.x as f32 * sx).floor() as i32
                    - MIRROR_DAMAGE_MARGIN_PX)
                    .clamp(0, max_w);
                let y0 = ((dst_y + rect.origin.y as f32 * sy).floor() as i32
                    - MIRROR_DAMAGE_MARGIN_PX)
                    .clamp(0, max_h);
                let x1 = ((dst_x + (rect.origin.x + rect.size.w) as f32 * sx).ceil() as i32
                    + MIRROR_DAMAGE_MARGIN_PX)
                    .clamp(0, max_w);
                let y1 = ((dst_y + (rect.origin.y + rect.size.h) as f32 * sy).ceil() as i32
                    + MIRROR_DAMAGE_MARGIN_PX)
                    .clamp(0, max_h);
                if x1 > x0 && y1 > y0 {
                    projected.push(tessera_primitives::Rect::new(x0, y0, x1 - x0, y1 - y0));
                }
            }
            if projected.is_empty() {
                FrameDamage::None
            } else {
                FrameDamage::from_rects(projected)
            }
        }
    }
}

/// Integer bounding box for a cursor sprite drawn on a mirror target, padded
/// by `MIRROR_DAMAGE_MARGIN_PX` and clamped to `mirror_mode`.
fn mirror_cursor_bounds(
    draw_x: f32,
    draw_y: f32,
    draw_w: f32,
    draw_h: f32,
    mirror_mode: (u32, u32),
) -> Option<tessera_primitives::Rect> {
    if draw_w <= 0.0 || draw_h <= 0.0 {
        return None;
    }
    let max_w = mirror_mode.0 as i32;
    let max_h = mirror_mode.1 as i32;
    let x0 = (draw_x.floor() as i32 - MIRROR_DAMAGE_MARGIN_PX).clamp(0, max_w);
    let y0 = (draw_y.floor() as i32 - MIRROR_DAMAGE_MARGIN_PX).clamp(0, max_h);
    let x1 = ((draw_x + draw_w).ceil() as i32 + MIRROR_DAMAGE_MARGIN_PX).clamp(0, max_w);
    let y1 = ((draw_y + draw_h).ceil() as i32 + MIRROR_DAMAGE_MARGIN_PX).clamp(0, max_h);
    (x1 > x0 && y1 > y0).then(|| tessera_primitives::Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Compute the mirror-local `FrameDamage` for one mirror target frame.
#[allow(clippy::too_many_arguments)]
fn compute_mirror_damage(
    needs_present: bool,
    source_damage: Option<&FrameDamage>,
    source_mode: (u32, u32),
    dst: (f32, f32, f32, f32),
    mirror_mode: (u32, u32),
    draws_mirror_cursor: bool,
    mirror_cursor_dirty: bool,
    previous_cursor_rect: Option<tessera_primitives::Rect>,
    new_cursor_rect: Option<tessera_primitives::Rect>,
) -> FrameDamage {
    if needs_present {
        return FrameDamage::Full;
    }
    let mut damage = source_damage.map_or(FrameDamage::None, |src| {
        project_source_damage_to_mirror(src, source_mode, dst, mirror_mode)
    });
    if draws_mirror_cursor && (mirror_cursor_dirty || previous_cursor_rect != new_cursor_rect) {
        let mut cursor_rects = Vec::with_capacity(2);
        if let Some(prev) = previous_cursor_rect {
            cursor_rects.push(prev);
        }
        if let Some(next) = new_cursor_rect {
            cursor_rects.push(next);
        }
        if !cursor_rects.is_empty() {
            damage = union_frame_damage(damage, FrameDamage::from_rects(cursor_rects));
        }
    }
    damage
}

fn target_namespace(connector: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in connector.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn last_hotspot(
    cache: &mut cursor::CursorCache,
    device: &optics_gfx::Device,
    shape: u32,
    scale: f32,
) -> (u32, u32) {
    cache
        .get(device, shape, scale)
        .map_or((0, 0), |cursor| (cursor.xhot as u32, cursor.yhot as u32))
}

fn last_sprite(
    cache: &mut cursor::CursorCache,
    device: &optics_gfx::Device,
    shape: u32,
    scale: f32,
) -> (u32, u32) {
    cache
        .get(device, shape, scale)
        .map_or((0, 0), |cursor| (cursor.width as u32, cursor.height as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::output::{
        OutputGeometry, OutputInfo, OutputKind, OutputMode, OutputRole, OutputRoleSource,
    };

    fn make_output(connector: &str, role: OutputRole) -> OutputInfo {
        OutputInfo {
            connector: connector.to_owned(),
            geometry: OutputGeometry {
                mode: OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60_000,
                },
                logical_size_override: None,
                ..Default::default()
            },
            kind: OutputKind::External,
            available_modes: Vec::new(),
            color_caps: Default::default(),
            role,
            role_source: OutputRoleSource::Auto,
        }
    }

    #[test]
    fn target_damage_clips_and_translates_union_pixels() {
        let source = tessera_primitives::Rect::new(3840, 0, 2560, 1440);
        let damage = FrameDamage::from_rects(vec![
            tessera_primitives::Rect::new(3900, 100, 200, 300),
            tessera_primitives::Rect::new(10, 10, 100, 100),
        ]);
        let local = target_damage_frame(&damage, source, (2560, 1440)).unwrap();
        assert_eq!(
            local,
            FrameDamage::Area(vec![tessera_primitives::Rect::new(60, 100, 200, 300)])
        );
    }

    #[test]
    fn target_damage_without_intersection_skips_the_output() {
        let source = tessera_primitives::Rect::new(3840, 0, 2560, 1440);
        let damage = FrameDamage::from_rects(vec![tessera_primitives::Rect::new(10, 10, 100, 100)]);
        assert!(target_damage_frame(&damage, source, (2560, 1440)).is_none());
    }

    #[test]
    fn target_damage_full_forces_a_full_pass() {
        let source = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        assert_eq!(
            target_damage_frame(&FrameDamage::Full, source, (1920, 1080)),
            Some(FrameDamage::Full)
        );
    }

    #[test]
    fn partial_damage_on_one_peer_in_a_multi_source_group_skips_untouched_peer() {
        let outputs = vec![
            make_output("DP-1", OutputRole::Extend),
            make_output("DP-2", OutputRole::Extend),
        ];
        let group = vec!["DP-1".to_owned(), "DP-2".to_owned()];
        let rect = tessera_primitives::Rect::new(10, 20, 100, 100);
        let plan = plan_group_render(
            &group,
            &outputs,
            |output| (output.connector == "DP-1").then(|| FrameDamage::Area(vec![rect])),
            |_| false,
            false,
            |_| false,
        );
        assert_eq!(
            plan.sources,
            vec![("DP-1".to_owned(), FrameDamage::Area(vec![rect]))]
        );
        assert!(plan.mirrors.is_empty());
    }

    #[test]
    fn mirror_needing_initial_present_pulls_its_source_into_the_group_plan() {
        let outputs = vec![
            make_output("eDP-1", OutputRole::Extend),
            make_output(
                "HDMI-A-1",
                OutputRole::Mirror {
                    source: "eDP-1".into(),
                },
            ),
        ];
        let group = vec!["eDP-1".to_owned(), "HDMI-A-1".to_owned()];
        let plan = plan_group_render(
            &group,
            &outputs,
            |_| None,
            |connector| connector == "HDMI-A-1",
            false,
            |_| false,
        );
        assert_eq!(plan.sources, vec![("eDP-1".to_owned(), FrameDamage::Full)]);
        assert_eq!(plan.mirrors, vec!["HDMI-A-1".to_owned()]);
    }

    #[test]
    fn test_inv_arch_159_retired_release_never_defers_across_frames() {
        assert_eq!(
            plan_retired_release(false, false, CompletionState::LiveFence),
            RetiredReleaseAction::None
        );
        assert_eq!(
            plan_retired_release(false, true, CompletionState::Signaled),
            RetiredReleaseAction::None
        );
        // Explicit-sync DRM (per-output targets, union, direct-scanout exit, or
        // NoDamage with live retained fence) releases immediately with fence.
        assert_eq!(
            plan_retired_release(true, false, CompletionState::LiveFence),
            RetiredReleaseAction::ReleaseWithFence
        );
        // Implicit-sync DRM (export_dmabuf waited) or signaled retained fence
        // releases immediately without deferring 4 frames.
        assert_eq!(
            plan_retired_release(true, false, CompletionState::Signaled),
            RetiredReleaseAction::ReleaseImmediate
        );
        // Failed SYNC_IOC_MERGE while live fences remain must never fail open
        // into ReleaseImmediate (ADR-0264).
        assert_eq!(
            plan_retired_release(true, false, CompletionState::UnmergedFences),
            RetiredReleaseAction::WaitUnmergedAndRelease
        );
        // Nested host swapchain synchronizes the device and releases immediately
        // instead of deferring 4 frames and starving 2-3 buffer swapchains.
        assert_eq!(
            plan_retired_release(true, true, CompletionState::Signaled),
            RetiredReleaseAction::WaitDeviceAndRelease
        );
        assert_eq!(
            plan_retired_release(true, true, CompletionState::LiveFence),
            RetiredReleaseAction::WaitDeviceAndRelease
        );
        assert_eq!(
            plan_retired_release(true, true, CompletionState::UnmergedFences),
            RetiredReleaseAction::WaitDeviceAndRelease
        );
    }

    #[test]
    fn test_inv_arch_159_retained_output_fences_isolate_independent_outputs() {
        use std::os::fd::FromRawFd;

        let new_live_fd = || {
            let raw = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
            assert!(raw >= 0);
            unsafe { OwnedFd::from_raw_fd(raw) }
        };

        let mut retained = RetainedOutputFences::default();
        retained.record(Some(&["eDP-1"]), CompletionFence::Live(new_live_fd()));
        retained.record(Some(&["HDMI-A-1"]), CompletionFence::Live(new_live_fd()));

        // Clearing or overwriting eDP-1 (e.g. entering direct scanout on eDP-1)
        // must not disturb HDMI-A-1's retained fence.
        retained.record(Some(&["eDP-1"]), CompletionFence::Signaled);
        let edp_idle = retained.completion_for_idle(false, &["eDP-1".to_owned()]);
        assert!(matches!(edp_idle, CompletionFence::Signaled));

        let hdmi_idle = retained.completion_for_idle(false, &["HDMI-A-1".to_owned()]);
        assert!(matches!(hdmi_idle, CompletionFence::Live(_)));
    }

    #[test]
    fn test_inv_arch_160_mirror_cursor_only_update_skips_source_rerender_when_retained() {
        let outputs = vec![
            make_output("eDP-1", OutputRole::Extend),
            make_output(
                "HDMI-A-1",
                OutputRole::Mirror {
                    source: "eDP-1".into(),
                },
            ),
        ];
        let group = vec!["eDP-1".to_owned(), "HDMI-A-1".to_owned()];

        // Source already retains its last presented frame: mirror updates its
        // cursor alone without forcing eDP-1 to re-render.
        let plan = plan_group_render(
            &group,
            &outputs,
            |_| None,
            |_| false,
            true,
            |src| src == "eDP-1",
        );
        assert!(plan.sources.is_empty());
        assert_eq!(plan.mirrors, vec!["HDMI-A-1".to_owned()]);

        // Without a retained source export (e.g. after VT resume), pull source.
        let fallback_plan =
            plan_group_render(&group, &outputs, |_| None, |_| false, true, |_| false);
        assert_eq!(
            fallback_plan.sources,
            vec![("eDP-1".to_owned(), FrameDamage::Full)]
        );
        assert_eq!(fallback_plan.mirrors, vec!["HDMI-A-1".to_owned()]);
    }

    #[test]
    fn test_inv_arch_160_mirror_damage_projection_scales_and_localizes_with_margin() {
        // 3072x1920 (16:10) fitted into 3840x2160 (16:9): 3456x2160 at x=192.
        let src_rect = tessera_primitives::Rect::new(100, 200, 300, 150);
        let projected = project_source_damage_to_mirror(
            &FrameDamage::Area(vec![src_rect]),
            (3072, 1920),
            (192.0, 0.0, 3456.0, 2160.0),
            (3840, 2160),
        );
        let FrameDamage::Area(rects) = projected else {
            panic!("expected partial mirror damage, got {projected:?}");
        };
        assert_eq!(rects.len(), 1);
        let r = rects[0];
        // sx = 3456/3072 = 1.125, sy = 2160/1920 = 1.125
        // x0 = floor(192 + 112.5) - 2 = 302, x1 = ceil(192 + 450) + 2 = 644 -> w = 342
        // y0 = floor(225) - 2 = 223, y1 = ceil(393.75) + 2 = 396 -> h = 173
        assert_eq!(r, tessera_primitives::Rect::new(302, 223, 342, 173));

        // Cursor-only mirror damage covers only old + new cursor rects, never Full.
        let old_cursor = tessera_primitives::Rect::new(400, 400, 48, 48);
        let new_cursor = tessera_primitives::Rect::new(460, 420, 48, 48);
        let cursor_only = compute_mirror_damage(
            false,
            None,
            (3072, 1920),
            (192.0, 0.0, 3456.0, 2160.0),
            (3840, 2160),
            true,
            true,
            Some(old_cursor),
            Some(new_cursor),
        );
        let FrameDamage::Area(cursor_rects) = cursor_only else {
            panic!("expected localized mirror cursor damage, got {cursor_only:?}");
        };
        assert!(cursor_rects.iter().all(|rect| rect.size.w < 3840));
        assert!(cursor_rects.contains(&old_cursor));
        assert!(cursor_rects.contains(&new_cursor));
    }

    #[test]
    fn cursor_only_mirror_check_ignores_non_mirror_output_targets() {
        let single = vec![make_output("eDP-1", OutputRole::Extend)];
        assert!(!super::presentation::has_active_mirror_target(
            &single,
            |_| true
        ));

        let extended = vec![
            make_output("eDP-1", OutputRole::Extend),
            make_output("HDMI-A-1", OutputRole::Extend),
        ];
        assert!(!super::presentation::has_active_mirror_target(
            &extended,
            |_| true
        ));

        let mirrored = vec![
            make_output("eDP-1", OutputRole::Extend),
            make_output(
                "HDMI-A-1",
                OutputRole::Mirror {
                    source: "eDP-1".into(),
                },
            ),
        ];
        assert!(super::presentation::has_active_mirror_target(
            &mirrored,
            |_| true
        ));
        assert!(!super::presentation::has_active_mirror_target(
            &mirrored,
            |c| c == "eDP-1"
        ));
    }

    #[test]
    fn transient_drm_and_gfx_errors_preserve_output_targets() {
        assert_eq!(
            classify_target_commit_error(&HostError::Drm(DrmError::Busy)),
            TargetCommitFailure::TransientBusy
        );
        assert_eq!(
            classify_target_commit_error(&HostError::Drm(DrmError::FlipTimeout)),
            TargetCommitFailure::TransientBusy
        );
        assert_eq!(
            classify_target_commit_error(&HostError::Drm(DrmError::CursorFallback)),
            TargetCommitFailure::CursorFallback
        );
        assert_eq!(
            classify_target_commit_error(&HostError::Drm(DrmError::Inactive)),
            TargetCommitFailure::ReconfiguredOrInactive
        );
        assert_eq!(
            classify_target_commit_error(&HostError::Drm(DrmError::Reconfigured)),
            TargetCommitFailure::ReconfiguredOrInactive
        );
        assert_eq!(
            classify_target_commit_error(&HostError::Drm(DrmError::ScanoutUnsupported)),
            TargetCommitFailure::Permanent
        );
        assert!(gfx_target_acquire_is_transient(&optics_gfx::Error(
            optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY
        )));
        assert!(gfx_target_acquire_is_transient(&optics_gfx::Error(
            optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_TIMEOUT
        )));
        assert!(!gfx_target_acquire_is_transient(&optics_gfx::Error(
            optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_SURFACE_LOST
        )));
    }

    #[test]
    fn offscreen_target_damage_ledger_tracks_initial_full_partial_and_invalidation() {
        const RING_DEPTH: usize = 3;
        let Ok(device) = optics_gfx::Device::new(true, &[], &[], RING_DEPTH as u32) else {
            return;
        };
        let size = (64, 64);
        let surface = optics_gfx::Surface::offscreen(&device, size.0, size.1).unwrap();
        let mut canvas = optics_gfx::Canvas::new(&surface).unwrap();

        // Frame 1 (initial Full): must report Full repaint scope.
        {
            let mut frame = surface
                .begin_frame_timeout_with_damage(
                    TARGET_ACQUIRE_TIMEOUT_NS,
                    &optics_gfx::Damage::Full,
                )
                .unwrap();
            let repaint = FrameDamage::from_gfx_damage(frame.repaint_scope().unwrap());
            assert_eq!(repaint, FrameDamage::Full);
            let session =
                begin_opaque_frame(&mut canvas, &mut frame, optics_gfx::rgba(0, 0, 0, 255))
                    .unwrap();
            session.end().unwrap();
            frame.submit().unwrap().present().unwrap();
        }

        // Remaining uninitialized slots receive partial current-frame damage;
        // gfx automatically forces Full repaint on each slot's first use while
        // recording only the partial damage in the epoch ledger.
        let rect = tessera_primitives::Rect::new(8, 12, 16, 16);
        let local = FrameDamage::Area(vec![rect]);
        for _ in 1..RING_DEPTH {
            let mut frame = surface
                .begin_frame_timeout_with_damage(TARGET_ACQUIRE_TIMEOUT_NS, &local.to_gfx_damage())
                .unwrap();
            let repaint = FrameDamage::from_gfx_damage(frame.repaint_scope().unwrap());
            assert_eq!(repaint, FrameDamage::Full);
            let session =
                begin_opaque_frame(&mut canvas, &mut frame, optics_gfx::rgba(0, 0, 0, 255))
                    .unwrap();
            session.end().unwrap();
            frame.submit().unwrap().present().unwrap();
        }

        // Now every slot has been rendered at least once and every intervening
        // epoch after slot 0 was partial: reusing slot 0 must yield a scoped
        // Area repaint, NOT Full.
        let mut frame = surface
            .begin_frame_timeout_with_damage(TARGET_ACQUIRE_TIMEOUT_NS, &local.to_gfx_damage())
            .unwrap();
        let repaint = FrameDamage::from_gfx_damage(frame.repaint_scope().unwrap());
        assert!(
            matches!(repaint, FrameDamage::Area(ref rects) if !rects.is_empty()),
            "expected scoped repaint area on primed offscreen target, got {repaint:?}"
        );
        let areas = frame_damage_scope(&repaint, size);
        let session = canvas
            .begin_session(
                &mut frame,
                optics_gfx::CanvasPassOptions {
                    clear: Some(optics_gfx::rgba(10, 20, 30, 255)),
                    damage: areas,
                },
            )
            .unwrap();
        session.end().unwrap();
        frame.submit().unwrap().present().unwrap();

        // Invalidating damage (VT resume / scanout exit / cursor fallback)
        // must force a Full repaint on the next acquire.
        surface.invalidate_damage();
        let mut frame = surface
            .begin_frame_timeout_with_damage(TARGET_ACQUIRE_TIMEOUT_NS, &local.to_gfx_damage())
            .unwrap();
        let repaint = FrameDamage::from_gfx_damage(frame.repaint_scope().unwrap());
        assert_eq!(repaint, FrameDamage::Full);
        let session =
            begin_opaque_frame(&mut canvas, &mut frame, optics_gfx::rgba(0, 0, 0, 255)).unwrap();
        session.end().unwrap();
        frame.submit().unwrap().present().unwrap();
    }

    #[test]
    fn abandoned_target_submission_preserves_ring_slot_on_kms_rejection() {
        let Ok(device) = optics_gfx::Device::new(true, &[], &[], 0) else {
            return;
        };
        let surface = optics_gfx::Surface::offscreen(&device, 64, 64).unwrap();
        let mut canvas = optics_gfx::Canvas::new(&surface).unwrap();

        let mut frame = surface
            .begin_frame_timeout_with_damage(TARGET_ACQUIRE_TIMEOUT_NS, &optics_gfx::Damage::Full)
            .unwrap();
        let slot_before = frame.index();
        let session =
            begin_opaque_frame(&mut canvas, &mut frame, optics_gfx::rgba(0, 0, 0, 255)).unwrap();
        session.end().unwrap();
        // Simulate KMS rejecting present_render_targets(Busy): drop without
        // calling present(), so the ring slot must not advance.
        let submitted = frame.submit().unwrap();
        drop(submitted);

        let mut retry_frame = surface
            .begin_frame_timeout_with_damage(TARGET_ACQUIRE_TIMEOUT_NS, &optics_gfx::Damage::Full)
            .unwrap();
        assert_eq!(
            retry_frame.index(),
            slot_before,
            "abandoned submission must reuse the same ring slot on retry"
        );
        let session = begin_opaque_frame(
            &mut canvas,
            &mut retry_frame,
            optics_gfx::rgba(0, 0, 0, 255),
        )
        .unwrap();
        session.end().unwrap();
        retry_frame.submit().unwrap().present().unwrap();
    }
}
