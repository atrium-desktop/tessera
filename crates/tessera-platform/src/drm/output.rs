use super::*;

/// Pure KMS disable operations built before touching an `AtomicModeReq`.
///
/// Linux's `drm_atomic_crtc_check` requires `MODE_ID = 0` whenever a CRTC has
/// its connectors detached (`ACTIVE = false` alone leaves `crtc_state->enable`
/// true and fails with `EINVAL`). Separating the plan from the ioctl makes that
/// invariant directly unit-testable without a DRM card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisablePlaneOp {
    pub(super) plane: plane::Handle,
    pub(super) fb_id: property::Handle,
    pub(super) crtc_id: property::Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisableConnectorOp {
    pub(super) connector: connector::Handle,
    pub(super) crtc_id: property::Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisableCrtcOp {
    pub(super) crtc: crtc::Handle,
    pub(super) active: property::Handle,
    pub(super) mode_id: property::Handle,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct DisableAtomicPlan {
    pub(super) planes: Vec<DisablePlaneOp>,
    pub(super) connectors: Vec<DisableConnectorOp>,
    pub(super) crtcs: Vec<DisableCrtcOp>,
}

impl DisableAtomicPlan {
    pub(super) fn is_empty(&self) -> bool {
        self.planes.is_empty() && self.connectors.is_empty() && self.crtcs.is_empty()
    }
}

/// Pure output descriptor used to build a disable plan without needing a full
/// `Output` with kernel blobs.
#[derive(Debug, Clone)]
pub(super) struct DisableOutputSpec {
    pub(super) connector: DisableConnectorOp,
    pub(super) crtc: DisableCrtcOp,
    pub(super) planes: Vec<DisablePlaneOp>,
}

pub(super) fn build_disable_plan(
    outputs: &[DisableOutputSpec],
    extra_planes: &[DisablePlaneOp],
    preserved_connectors: &HashSet<connector::Handle>,
    preserved_crtcs: &HashSet<crtc::Handle>,
    preserved_planes: &HashSet<plane::Handle>,
) -> DisableAtomicPlan {
    let mut plan = DisableAtomicPlan::default();
    let mut seen_planes = HashSet::new();
    let mut seen_connectors = HashSet::new();
    let mut seen_crtcs = HashSet::new();

    for plane in extra_planes {
        if !preserved_planes.contains(&plane.plane) && seen_planes.insert(plane.plane) {
            plan.planes.push(*plane);
        }
    }
    for output in outputs {
        for plane in &output.planes {
            if !preserved_planes.contains(&plane.plane) && seen_planes.insert(plane.plane) {
                plan.planes.push(*plane);
            }
        }
        if !preserved_connectors.contains(&output.connector.connector)
            && seen_connectors.insert(output.connector.connector)
        {
            plan.connectors.push(output.connector);
        }
        if !preserved_crtcs.contains(&output.crtc.crtc) && seen_crtcs.insert(output.crtc.crtc) {
            plan.crtcs.push(output.crtc);
        }
    }
    plan
}

pub(super) fn apply_disable_plan(request: &mut atomic::AtomicModeReq, plan: &DisableAtomicPlan) {
    for plane in &plan.planes {
        request.add_property(plane.plane, plane.fb_id, property::Value::Framebuffer(None));
        request.add_property(plane.plane, plane.crtc_id, property::Value::CRTC(None));
    }
    for connector in &plan.connectors {
        request.add_property(
            connector.connector,
            connector.crtc_id,
            property::Value::CRTC(None),
        );
    }
    for crtc in &plan.crtcs {
        request.add_property(crtc.crtc, crtc.mode_id, property::Value::Blob(0));
        request.add_property(crtc.crtc, crtc.active, property::Value::Boolean(false));
    }
}

/// Value-equality signature of a DRM mode timing, used to determine whether a
/// surviving output can keep its live CRTC/mode blob across a hotplug without
/// triggering a redundant modeset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModeSignature {
    pub(super) size: (u16, u16),
    pub(super) clock: u32,
    pub(super) hsync: (u16, u16, u16),
    pub(super) vsync: (u16, u16, u16),
    pub(super) vrefresh: u32,
    pub(super) flags: u32,
}

pub(super) fn mode_signature(mode: &Mode) -> ModeSignature {
    ModeSignature {
        size: mode.size(),
        clock: mode.clock(),
        hsync: mode.hsync(),
        vsync: mode.vsync(),
        vrefresh: mode.vrefresh(),
        flags: mode.flags().bits(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HotplugOutputKey {
    pub(super) connector: connector::Handle,
    pub(super) crtc: crtc::Handle,
    pub(super) primary_plane: plane::Handle,
    pub(super) mode: ModeSignature,
    pub(super) modeset_done: bool,
    pub(super) link_bad: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct HotplugTransitionPlan {
    /// `(old_index, new_index)` pairs whose live CRTC, primary plane, and mode
    /// stay untouched across the hotplug.
    pub(super) preserved: Vec<(usize, usize)>,
    /// Indices in `old` that completed a modeset and must be disabled in KMS
    /// before releasing their framebuffers.
    pub(super) disable_old_indices: Vec<usize>,
    /// Indices in `old` whose scanout slots are not carried forward and must be
    /// released once the disable commit completes.
    pub(super) release_old_indices: Vec<usize>,
}

pub(super) fn plan_hotplug_transition(
    old: &[HotplugOutputKey],
    new: &[HotplugOutputKey],
    same_format_and_color: bool,
) -> HotplugTransitionPlan {
    let mut plan = HotplugTransitionPlan::default();
    let mut matched_new = vec![false; new.len()];
    for (old_index, old_key) in old.iter().enumerate() {
        let preserved_new = if same_format_and_color && old_key.modeset_done {
            new.iter().enumerate().position(|(new_index, new_key)| {
                !matched_new[new_index]
                    && !new_key.link_bad
                    && old_key.connector == new_key.connector
                    && old_key.crtc == new_key.crtc
                    && old_key.primary_plane == new_key.primary_plane
                    && old_key.mode == new_key.mode
            })
        } else {
            None
        };
        if let Some(new_index) = preserved_new {
            matched_new[new_index] = true;
            plan.preserved.push((old_index, new_index));
        } else {
            if old_key.modeset_done {
                plan.disable_old_indices.push(old_index);
            }
            plan.release_old_indices.push(old_index);
        }
    }
    plan
}

fn disable_spec_for_output(
    output: &Output,
    active_overlay: Option<&OverlayScanout>,
) -> DisableOutputSpec {
    let mut planes = Vec::with_capacity(3);
    if let Some(cursor) = output.planes.cursor.or(output.planes.disabled_cursor) {
        planes.push(DisablePlaneOp {
            plane: cursor.handle,
            fb_id: cursor.props.plane_fb_id,
            crtc_id: cursor.props.plane_crtc_id,
        });
    }
    planes.push(DisablePlaneOp {
        plane: output.planes.primary.handle,
        fb_id: output.planes.primary.props.fb_id,
        crtc_id: output.planes.primary.props.crtc_id,
    });
    if let Some(overlay) = active_overlay.filter(|overlay| overlay.output == output.name)
        && let Some(caps) = output
            .planes
            .overlay
            .discovered
            .iter()
            .find(|plane| plane.handle == overlay.plane)
    {
        planes.push(DisablePlaneOp {
            plane: caps.handle,
            fb_id: caps.props.fb_id,
            crtc_id: caps.props.crtc_id,
        });
    }
    DisableOutputSpec {
        connector: DisableConnectorOp {
            connector: output.connector,
            crtc_id: output.props.connector_crtc_id,
        },
        crtc: DisableCrtcOp {
            crtc: output.crtc,
            active: output.props.crtc_active,
            mode_id: output.props.crtc_mode_id,
        },
        planes,
    }
}

impl DrmBackend {
    /// Change KMS scanout power while retaining connector assignment and
    /// input ownership. Turning outputs back on is completed by the next
    /// ordinary modeset frame, so the compositor redraws the still-locked
    /// scene instead of exposing a stale pre-lock framebuffer.
    pub fn set_outputs_powered(&mut self, powered: bool) -> Result<(), DrmError> {
        if self.outputs_powered == powered {
            return Ok(());
        }
        if !self.active {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            // Output power is a presentation-domain transition. Never hide a
            // page-flip wait inside this control path: the main loop must stay
            // live and retry after the owned batch retires.
            return Err(DrmError::Busy);
        }
        if !powered {
            if self.any_modeset_done() {
                self.disable_outputs()?;
            }
            self.reset_modeset_done();
            self.cursor_plane_active = false;
            // The kernel plane is now blank; forgetting the cached sprite
            // position makes the next cursor commit reprogram the plane
            // instead of trusting a stale baseline.
            self.cursor_state = None;
            self.outputs_powered = false;
            log::info!("drm: physical outputs powered off; input remains active");
        } else {
            self.outputs_powered = true;
            self.reset_modeset_done();
            // Re-assert the cursor on the first full commit after wake; the
            // plane was blanked at power-off, so its old placement is void.
            self.cursor_state = None;
            log::info!("drm: physical outputs waking on next secure frame");
        }
        Ok(())
    }

    pub(super) fn reconfigure_outputs(&mut self) {
        self.hotplug_pending = false;
        let existing_assignments = self
            .displays
            .outputs
            .iter()
            .map(|output| {
                (
                    output.connector,
                    (output.crtc, output.planes.primary.handle),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut selected = match select_outputs(
            self.card(),
            &self.configured_modes,
            &self.configured_color,
            &self.configured_icc,
            self.configured_topology,
            &self.configured_policies,
            &existing_assignments,
        ) {
            Ok(displays) => displays,
            Err(DrmError::NoConnector) => {
                log::info!("drm: all outputs disconnected; suspending rendering");
                if self.any_modeset_done() {
                    let _ = self.disable_outputs();
                }
                self.reset_modeset_done();
                self.pending_flips.clear();
                self.clear_plane_slots(true);
                if let Some(overlay) = self.current_overlay.take() {
                    self.release_overlay_scanout(overlay);
                }
                if let Some(overlay) = self.retiring_overlay.take() {
                    self.release_overlay_scanout(overlay);
                }
                self.render_ready = false;
                return;
            }
            Err(error) => {
                log::warn!("drm: hotplug reprobe failed; keeping current layout: {error}");
                return;
            }
        };

        apply_overlay_policy(&mut selected, self.overlay_offload_enabled);
        let any_link_bad = selected.outputs.iter().any(|output| output.link_bad);
        if !any_link_bad && display_signature(&selected) == display_signature(&self.displays) {
            // Probe created fresh mode blobs. The existing display set remains
            // authoritative, so release only the redundant probe resources.
            for output in selected.outputs {
                destroy_output_blobs(self.card(), &output);
            }
            self.render_ready = true;
            return;
        }

        let old_keys = self
            .displays
            .outputs
            .iter()
            .zip(&self.slots)
            .map(|(output, slot)| HotplugOutputKey {
                connector: output.connector,
                crtc: output.crtc,
                primary_plane: output.planes.primary.handle,
                mode: mode_signature(&output.mode),
                modeset_done: slot.modeset_done,
                link_bad: false,
            })
            .collect::<Vec<_>>();
        let new_keys = selected
            .outputs
            .iter()
            .map(|output| HotplugOutputKey {
                connector: output.connector,
                crtc: output.crtc,
                primary_plane: output.planes.primary.handle,
                mode: mode_signature(&output.mode),
                modeset_done: false,
                link_bad: output.link_bad,
            })
            .collect::<Vec<_>>();
        let same_format_and_color = self.displays.format == selected.format
            && self.displays.color_mode == selected.color_mode;
        let transition = plan_hotplug_transition(&old_keys, &new_keys, same_format_and_color);

        let mut preserved_connectors = HashSet::new();
        let mut preserved_crtcs = HashSet::new();
        let mut preserved_planes = HashSet::new();
        let mut extra_planes = Vec::new();
        for &(old_index, new_index) in &transition.preserved {
            let old_output = &self.displays.outputs[old_index];
            let new_output = &selected.outputs[new_index];
            preserved_connectors.insert(new_output.connector);
            preserved_crtcs.insert(new_output.crtc);
            preserved_planes.insert(new_output.planes.primary.handle);
            if let Some(cursor) = new_output.planes.cursor {
                preserved_planes.insert(cursor.handle);
            }
            if let Some(old_cursor) = old_output.planes.cursor
                && new_output.planes.cursor.map(|plane| plane.handle) != Some(old_cursor.handle)
            {
                extra_planes.push(DisablePlaneOp {
                    plane: old_cursor.handle,
                    fb_id: old_cursor.props.plane_fb_id,
                    crtc_id: old_cursor.props.plane_crtc_id,
                });
            }
        }

        let clear_overlay = self.current_overlay.as_ref().is_some_and(|overlay| {
            selected
                .outputs
                .iter()
                .any(|output| output.role.is_mirror())
                || !transition
                    .preserved
                    .iter()
                    .any(|&(old_index, _)| self.displays.outputs[old_index].name == overlay.output)
        });
        if clear_overlay
            && let Some(overlay) = self.current_overlay.as_ref()
            && let Some(owner) = self
                .displays
                .outputs
                .iter()
                .find(|output| output.name == overlay.output)
            && let Some(caps) = owner
                .planes
                .overlay
                .discovered
                .iter()
                .find(|plane| plane.handle == overlay.plane)
        {
            extra_planes.push(DisablePlaneOp {
                plane: caps.handle,
                fb_id: caps.props.fb_id,
                crtc_id: caps.props.crtc_id,
            });
        }

        let disable_specs = transition
            .disable_old_indices
            .iter()
            .map(|&index| {
                disable_spec_for_output(
                    &self.displays.outputs[index],
                    self.current_overlay.as_ref(),
                )
            })
            .collect::<Vec<_>>();
        let disable_plan = build_disable_plan(
            &disable_specs,
            &extra_planes,
            &preserved_connectors,
            &preserved_crtcs,
            &preserved_planes,
        );
        if !disable_plan.is_empty() {
            let mut request = atomic::AtomicModeReq::new();
            apply_disable_plan(&mut request, &disable_plan);
            if let Err(error) = self
                .card()
                .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request)
            {
                log::warn!("drm: failed to disable removed hotplug outputs: {error}");
            }
        }

        // Preserve live mode blobs for unchanged outputs so KMS sees identical
        // `MODE_ID` handles and does not flag a spurious mode change on pipes
        // that did not move. The redundant probe blobs swap into `old` and are
        // destroyed below.
        for &(old_index, new_index) in &transition.preserved {
            std::mem::swap(
                &mut self.displays.outputs[old_index].mode_blob,
                &mut selected.outputs[new_index].mode_blob,
            );
            std::mem::swap(
                &mut self.displays.outputs[old_index].mode_blob_id,
                &mut selected.outputs[new_index].mode_blob_id,
            );
        }

        self.pending_flips.clear();
        let mut old_slots: Vec<Option<PlaneSlot>> = std::mem::take(&mut self.slots)
            .into_iter()
            .map(Some)
            .collect();
        let old = std::mem::replace(&mut self.displays, selected);

        let mut new_slots: Vec<Option<PlaneSlot>> =
            (0..self.displays.outputs.len()).map(|_| None).collect();
        for &(old_index, new_index) in &transition.preserved {
            if let Some(mut slot) = old_slots.get_mut(old_index).and_then(Option::take) {
                slot.crtc = self.displays.outputs[new_index].crtc;
                slot.pending = false;
                slot.modeset_done = true;
                new_slots[new_index] = Some(slot);
            }
        }
        self.slots = self
            .displays
            .outputs
            .iter()
            .zip(new_slots)
            .map(|(output, slot)| {
                slot.unwrap_or(PlaneSlot {
                    crtc: output.crtc,
                    current: None,
                    retiring: None,
                    source: None,
                    pending: false,
                    modeset_done: false,
                })
            })
            .collect();

        // Release scanout framebuffers belonging to removed/reassigned outputs
        // only after their CRTCs and planes were disabled above.
        for mut slot in old_slots.into_iter().flatten() {
            let scanouts = slot.retiring.take().into_iter().chain(slot.current.take());
            for scanout in scanouts {
                self.release_scanout(scanout);
            }
        }
        if clear_overlay {
            if let Some(overlay) = self.current_overlay.take() {
                self.release_overlay_scanout(overlay);
            }
            if let Some(overlay) = self.retiring_overlay.take() {
                self.release_overlay_scanout(overlay);
            }
        }
        for output in old.outputs {
            destroy_output_blobs(self.card(), &output);
        }

        if !self.hardware_cursor_supported() {
            // A mirror removes one output's cursor plane, so the session falls
            // back to the software cursor. Drop any sprite committed before
            // the topology change; otherwise the source CRTC would keep a
            // hardware cursor that the mirror's framebuffer never contains.
            self.cursor_state = None;
            self.cursor_plane_active = false;
        }
        if self.displays.modifiers != self.surface_modifiers
            || self.displays.color_mode != self.surface_color_mode
            || self.displays.icc_profile != self.surface_icc
        {
            // The live Flux surface was created with the old intersection
            // and pixel encoding; resize cannot retcon either — the main
            // loop must recreate it (see Backend::surface_needs_recreate).
            log::info!(
                "drm: modifier intersection or color mode changed; presentation surface must be recreated"
            );
            self.surface_stale = true;
        }
        let (width, height) = self.displays.size;
        self.pointer.0 = self.pointer.0.clamp(0.0, width.saturating_sub(1) as f32);
        self.pointer.1 = self.pointer.1.clamp(0.0, height.saturating_sub(1) as f32);
        self.explicit_sync = self.sync_capable
            && self
                .displays
                .outputs
                .iter()
                .all(|output| output.planes.primary.props.in_fence_fd.is_some());
        self.pending_resize = Some(Size {
            w: width as i32,
            h: height as i32,
        });
        self.render_ready = true;
        log::info!(
            "drm: hotplug layout now has {} output(s) ({} preserved), desktop {}x{}",
            self.displays.outputs.len(),
            transition.preserved.len(),
            width,
            height
        );
    }

    /// Drop every per-output primary slot and optionally release the
    /// framebuffers they reference. `release = false` is the revoked-fd path:
    /// the card is gone, so handles are leaked rather than used.
    pub(super) fn clear_plane_slots(&mut self, release: bool) {
        let slots = std::mem::take(&mut self.slots);
        for mut slot in slots {
            let scanouts = slot.retiring.take().into_iter().chain(slot.current.take());
            for scanout in scanouts {
                if release {
                    self.release_scanout(scanout);
                } else {
                    log::debug!(
                        "drm: forgetting scanout slot {} after device loss",
                        scanout.slot
                    );
                }
            }
        }
    }

    /// Rebuild the primary-plane slots after a display-set change. Callers
    /// must clear the old slots first so their references are dropped.
    pub(super) fn rebuild_plane_slots(&mut self) {
        self.slots = self
            .displays
            .outputs
            .iter()
            .map(|output| PlaneSlot {
                crtc: output.crtc,
                current: None,
                retiring: None,
                source: None,
                pending: false,
                modeset_done: false,
            })
            .collect();
    }

    pub(super) fn any_modeset_done(&self) -> bool {
        self.slots.iter().any(|slot| slot.modeset_done)
    }

    pub(super) fn reset_modeset_done(&mut self) {
        for slot in &mut self.slots {
            slot.modeset_done = false;
        }
    }

    pub(super) fn slot_index_for_connector(&self, connector: &str) -> Option<usize> {
        self.displays
            .outputs
            .iter()
            .position(|output| output.name == connector)
    }

    pub(super) fn release_scanout(&mut self, scanout: PrimaryScanout) {
        self.release_scanout_parts(scanout.framebuffer, scanout.gem, scanout.ownership);
    }

    pub(super) fn release_scanout_options(&mut self, scanouts: &mut [Option<PrimaryScanout>]) {
        for scanout in scanouts.iter_mut() {
            if let Some(scanout) = scanout.take() {
                self.release_scanout(scanout);
            }
        }
    }

    pub(super) fn release_overlay_scanout(&mut self, overlay: OverlayScanout) {
        self.release_scanout_parts(overlay.framebuffer, overlay.gem, overlay.ownership);
    }

    fn release_scanout_parts(
        &mut self,
        framebuffer: control::framebuffer::Handle,
        gem: BufferHandle,
        ownership: PrimaryScanoutOwnership,
    ) {
        match ownership {
            PrimaryScanoutOwnership::TransientCompositor
            | PrimaryScanoutOwnership::TransientClient => {
                let card = self.card();
                if let Err(error) = card.destroy_framebuffer(framebuffer) {
                    log::warn!("DRM: failed to destroy framebuffer: {error}");
                }
                if let Err(error) = card.close_buffer(gem) {
                    log::warn!("DRM: failed to close imported GEM handle: {error}");
                }
            }
            PrimaryScanoutOwnership::Cached(key) => {
                if let Some(entry) = self.composite_fb_cache.get_mut(&key) {
                    debug_assert!(entry.users > 0, "cached scanout released twice");
                    entry.users = entry.users.saturating_sub(1);
                } else {
                    // This is only expected after a revoked DRM fd, whose
                    // scanouts are forgotten rather than released.
                    log::warn!("drm: cached scanout record disappeared before retirement");
                }
                self.reap_composite_fb_cache();
            }
            PrimaryScanoutOwnership::CachedClient(key) => {
                if let Some(entry) = self.client_fb_cache.get_mut(&key) {
                    debug_assert!(entry.users > 0, "cached client scanout released twice");
                    entry.users = entry.users.saturating_sub(1);
                } else {
                    log::warn!("drm: cached client scanout record disappeared before retirement");
                }
                self.reap_client_fb_cache();
            }
        }
    }

    pub(super) fn disable_outputs(&mut self) -> Result<(), DrmError> {
        let specs = self
            .displays
            .outputs
            .iter()
            .map(|output| disable_spec_for_output(output, self.current_overlay.as_ref()))
            .collect::<Vec<_>>();
        let empty_connectors = HashSet::new();
        let empty_crtcs = HashSet::new();
        let empty_planes = HashSet::new();
        let plan = build_disable_plan(&specs, &[], &empty_connectors, &empty_crtcs, &empty_planes);
        if !plan.is_empty() {
            let mut request = atomic::AtomicModeReq::new();
            apply_disable_plan(&mut request, &plan);
            self.card()
                .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request)?;
        }
        self.cursor_plane_active = false;
        Ok(())
    }
}

pub(super) fn candidate_cards() -> Vec<PathBuf> {
    candidate_cards_with_override(std::env::var_os("TESSERA_DRM_DEVICE"))
}

pub(super) fn candidate_cards_with_override(
    override_path: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    if let Some(path) = override_path {
        return vec![PathBuf::from(path)];
    }
    (0..16)
        .map(|index| PathBuf::from(format!("/dev/dri/card{index}")))
        .filter(|path| path.exists())
        .collect()
}

/// Milliseconds until `deadline`, shaped as a `poll(2)` timeout: `None`
/// blocks indefinitely and an already-passed deadline polls without blocking.
pub(super) fn poll_ms_remaining(deadline: Option<std::time::Instant>) -> i32 {
    match deadline {
        None => -1,
        Some(deadline) => deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(i32::MAX as u128) as i32,
    }
}

/// Whether an optional pump deadline has been reached. `None` never expires.
pub(super) fn deadline_passed(deadline: Option<std::time::Instant>) -> bool {
    deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
}

pub(super) fn open_card_and_outputs(
    seat: &Rc<RefCell<libseat::Seat>>,
    configured_modes: &HashMap<String, ModeSpec>,
    configured_color: &HashMap<String, ColorPolicy>,
    configured_icc: &HashMap<String, String>,
    configured_topology: tessera_desktop::output::DisplayTopology,
    configured_policies: &HashMap<String, tessera_desktop::output::OutputPolicy>,
) -> Result<(Card, DisplaySet), DrmError> {
    let candidates = candidate_cards();
    let tried = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for path in candidates {
        match seat.borrow_mut().open_device(&path) {
            Ok(device) => {
                let card = Card { device, path };
                let result = card
                    .set_client_capability(drm::ClientCapability::UniversalPlanes, true)
                    .and_then(|()| card.set_client_capability(drm::ClientCapability::Atomic, true))
                    .map_err(DrmError::from)
                    .and_then(|()| {
                        select_outputs(
                            &card,
                            configured_modes,
                            configured_color,
                            configured_icc,
                            configured_topology,
                            configured_policies,
                            &HashMap::new(),
                        )
                    });
                match result {
                    Ok(output) => return Ok((card, output)),
                    Err(error) => {
                        log::warn!(
                            "drm: skipping unusable card {}: {error}",
                            card.path.display()
                        );
                        if let Err(close_error) = seat.borrow_mut().close_device(card.device) {
                            log::warn!(
                                "libseat: failed to close skipped card {}: {close_error:?}",
                                card.path.display()
                            );
                        }
                    }
                }
            }
            Err(error) => log::warn!("libseat: cannot open {}: {error:?}", path.display()),
        }
    }
    Err(DrmError::NoCard(if tried.is_empty() {
        "/dev/dri/card[0-15] (none exist)".to_owned()
    } else {
        tried
    }))
}

#[derive(Debug, Clone)]
pub(super) struct OutputCandidate {
    pub(super) connector: connector::Handle,
    pub(super) name: String,
    pub(super) mode: Mode,
    pub(super) physical_size_mm: Option<(u32, u32)>,
    pub(super) ppi: Option<f32>,
    pub(super) kind: OutputKind,
    pub(super) scale: Scale,
    pub(super) choices: Vec<OutputChoice>,
    pub(super) preferred_crtc: Option<crtc::Handle>,
    pub(super) preferred_plane: Option<plane::Handle>,
    pub(super) available_modes: Vec<OutputMode>,
    pub(super) color_caps: tessera_primitives::edid::EdidColorCapabilities,
    pub(super) role: tessera_desktop::output::OutputRole,
    pub(super) role_source: tessera_desktop::output::OutputRoleSource,
}

#[derive(Debug, Clone)]
pub(super) struct OutputChoice {
    pub(super) crtc: crtc::Handle,
    pub(super) plane: plane::Handle,
    pub(super) modifiers: Vec<u64>,
}

pub(super) fn select_outputs(
    card: &Card,
    configured_modes: &HashMap<String, ModeSpec>,
    configured_color: &HashMap<String, ColorPolicy>,
    configured_icc: &HashMap<String, String>,
    configured_topology: tessera_desktop::output::DisplayTopology,
    configured_policies: &HashMap<String, tessera_desktop::output::OutputPolicy>,
    existing_assignments: &HashMap<connector::Handle, (crtc::Handle, plane::Handle)>,
) -> Result<DisplaySet, DrmError> {
    let resources = card.resource_handles()?;
    let mut connectors = resources
        .connectors()
        .iter()
        .filter_map(|handle| card.get_connector(*handle, true).ok())
        .filter(|info| info.state() == connector::State::Connected && !info.modes().is_empty())
        .collect::<Vec<_>>();
    connectors.sort_by_key(|info| (info.interface() as u32, info.interface_id()));
    if connectors.is_empty() {
        return Err(DrmError::NoConnector);
    }

    // Session color-mode decision. One shared framebuffer means one pixel
    // encoding across outputs, so HDR (and deep color) engage only when
    // *every* active output both opts in via config and proves support
    // (EDID ST 2084 for HDR; plane format support is checked by the
    // candidate loop below). Anything less stays at the 8-bit SDR default.
    let color_caps: Vec<tessera_primitives::edid::EdidColorCapabilities> = connectors
        .iter()
        .map(|info| connector_color_caps(card, info.handle()))
        .collect();
    let requested_mode = {
        let all_hdr = connectors.iter().zip(&color_caps).all(|(info, caps)| {
            configured_color
                .get(&info.to_string())
                .is_some_and(|policy| policy.hdr)
                && caps.hdr_pq
        });
        let all_deep = connectors.iter().all(|info| {
            configured_color
                .get(&info.to_string())
                .is_some_and(|policy| policy.deep_color)
        });
        if all_hdr {
            DisplayColorMode::Hdr
        } else if all_deep {
            DisplayColorMode::SdrDeepColor
        } else {
            DisplayColorMode::Sdr
        }
    };
    if requested_mode == DisplayColorMode::Hdr {
        log::info!("drm: HDR mode requested (all outputs opt in and advertise ST 2084)");
    }

    // The ICC profile that drives the framebuffer's content space: the
    // first connected connector with a configured profile. HDR mode
    // ignores ICC (BT.2020 PQ is the encoding there).
    let icc_profile = if requested_mode == DisplayColorMode::Hdr {
        None
    } else {
        connectors
            .iter()
            .find_map(|info| configured_icc.get(info.to_string().as_str()).cloned())
    };

    let plane_inventory = discover_plane_inventory(card, &resources)?;

    let mut assignment = None;
    let mut attempted = requested_mode;
    for (index, format) in requested_mode.fb_candidates().iter().enumerate() {
        // Falling off the 10-bit candidates means the deep pipeline cannot
        // be driven; degrade the session mode along with the format.
        if index > 0 && matches!(*format, DrmFourcc::Xrgb8888 | DrmFourcc::Argb8888) {
            attempted = DisplayColorMode::Sdr;
        }
        let mut candidates = Vec::with_capacity(connectors.len());
        for (connector, &caps) in connectors.iter().zip(&color_caps) {
            let name = connector.to_string();
            // (width, height, refresh_mhz, preferred) in connector order, so
            // an index returned by pick_mode addresses connector.modes()
            // directly.
            let tuples = connector
                .modes()
                .iter()
                .map(|mode| {
                    let (width, height) = mode.size();
                    (
                        width as i32,
                        height as i32,
                        mode.vrefresh().saturating_mul(1_000),
                        mode.mode_type().contains(ModeTypeFlags::PREFERRED),
                    )
                })
                .collect::<Vec<_>>();
            let spec = configured_modes.get(&name);
            let picked = match pick_mode(&tuples, spec) {
                Some(index) => index,
                None => {
                    // Only reachable with a spec that matched nothing.
                    if let Some(spec) = spec {
                        log::warn!(
                            "drm: {name}: configured mode {spec:?} matches no advertised mode; using the best available mode"
                        );
                    }
                    pick_mode(&tuples, None).unwrap_or(0)
                }
            };
            let mode = connector.modes()[picked];
            let (mode_width, mode_height) = mode.size();
            let selected_mode = OutputMode {
                width: mode_width as i32,
                height: mode_height as i32,
                refresh_mhz: mode.vrefresh().saturating_mul(1_000),
            };
            let physical_size_mm = connector.size();
            let kind = if matches!(
                connector.interface(),
                connector::Interface::EmbeddedDisplayPort
                    | connector::Interface::LVDS
                    | connector::Interface::DSI
            ) {
                OutputKind::Internal
            } else {
                OutputKind::External
            };
            let ppi = physical_size_mm.and_then(|size| physical_ppi(selected_mode, size));
            let scale = physical_size_mm
                .and_then(|size| automatic_scale(selected_mode, size, kind))
                .unwrap_or(Scale::IDENTITY);
            let existing = existing_assignments.get(&connector.handle()).copied();
            let current_crtc = connector
                .current_encoder()
                .and_then(|encoder| card.get_encoder(encoder).ok())
                .and_then(|encoder| encoder.crtc());
            let preferred_crtc = existing.map(|(crtc, _)| crtc).or(current_crtc);
            let preferred_plane = existing.map(|(_, plane)| plane);
            let mut crtcs = Vec::new();
            if let Some(preferred) = preferred_crtc {
                crtcs.push(preferred);
            }
            if let Some(current) = current_crtc
                && !crtcs.contains(&current)
            {
                crtcs.push(current);
            }
            for encoder in connector.encoders() {
                if let Ok(encoder) = card.get_encoder(*encoder) {
                    for crtc in resources.filter_crtcs(encoder.possible_crtcs()) {
                        if !crtcs.contains(&crtc) {
                            crtcs.push(crtc);
                        }
                    }
                }
            }

            let mut choices = Vec::new();
            for crtc in crtcs {
                for plane in &plane_inventory.primary {
                    if !plane.possible_crtcs.contains(&crtc)
                        || !plane.formats.contains(&(*format as u32))
                    {
                        continue;
                    }
                    let modifiers = plane_modifiers(card, plane.handle, *format)?;
                    if !modifiers.is_empty() {
                        choices.push(OutputChoice {
                            crtc,
                            plane: plane.handle,
                            modifiers,
                        });
                    }
                }
            }
            candidates.push(OutputCandidate {
                connector: connector.handle(),
                name,
                mode,
                physical_size_mm,
                ppi,
                kind,
                scale,
                choices,
                preferred_crtc,
                preferred_plane,
                available_modes: advertised_modes(connector),
                color_caps: caps,
                role: tessera_desktop::output::OutputRole::Extend,
                role_source: tessera_desktop::output::OutputRoleSource::Auto,
            });
        }
        if candidates
            .iter()
            .any(|candidate| candidate.choices.is_empty())
        {
            continue;
        }
        apply_candidate_roles(&mut candidates, configured_topology, configured_policies);
        if let Some((choices, modifiers)) = assign_outputs(&candidates) {
            assignment = Some((*format, attempted, candidates, choices, modifiers));
            break;
        }
    }

    let Some((format, color_mode, candidates, choices, modifiers)) = assignment else {
        return Err(DrmError::NoPlane);
    };
    if requested_mode != DisplayColorMode::Sdr && color_mode == DisplayColorMode::Sdr {
        log::warn!(
            "drm: {requested_mode:?} requested but no 10-bit plane assignment exists; falling back to SDR"
        );
    }
    // Extended outputs own disjoint horizontal slices of the shared
    // framebuffer. A mirror is not a slice of its own: it scans the same
    // source rectangle, optionally scaled by its plane to the mirror's native
    // mode (ADR-0243). Only extended outputs consume desktop width, and the
    // height is bounded by them.
    let placement_inputs = candidates
        .iter()
        .map(|candidate| PlacementInput {
            name: candidate.name.as_str(),
            size: (
                u32::from(candidate.mode.size().0),
                u32::from(candidate.mode.size().1),
            ),
            mirror: candidate.role.mirror_source(),
        })
        .collect::<Vec<_>>();
    let (layout, desktop_width, desktop_height) = layout_outputs(&placement_inputs)?;
    if !resources.supported_fb_width().contains(&desktop_width)
        || !resources.supported_fb_height().contains(&desktop_height)
    {
        return Err(DrmError::DesktopTooLarge(desktop_width, desktop_height));
    }

    let mut outputs: Vec<Output> = Vec::with_capacity(candidates.len());
    for (candidate, choice) in candidates.into_iter().zip(choices) {
        let Some(&(x, source_size)) = layout.get(&candidate.name) else {
            // The resolver cannot emit a dangling mirror, but the backend is
            // the last boundary: degrade to the candidate's own slice.
            for output in &outputs {
                destroy_output_blobs(card, output);
            }
            return Err(DrmError::NoPlane);
        };
        let mode_size = candidate.mode.size();
        let destination = fitted_destination(
            (u32::from(mode_size.0), u32::from(mode_size.1)),
            source_size,
        );
        let result = build_output(card, candidate, choice, x, source_size, destination);
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                for output in &outputs {
                    destroy_output_blobs(card, output);
                }
                return Err(error);
            }
        };
        outputs.push(output);
    }
    // Cursor and overlay are independent of the primary-plane matching above.
    // Cursor allocation is a maximum bipartite matching. Overlay discovery
    // starts disabled and is enabled only by the explicit hardware-validation
    // policy; the commit path still proves the complete set with TEST_ONLY
    // (ADR-0234, ADR-0235).
    assign_cursor_planes(card, &mut outputs, &plane_inventory);
    for output in &mut outputs {
        if output.role.is_mirror() {
            // The mirror's cursor is composited into the source rectangle and
            // arrives through the shared framebuffer. A second hardware
            // cursor plane would need independent coordinate scaling and
            // would visibly duplicate the pointer. Retain the handle in
            // `disabled_cursor` so commits explicitly clear any stale kernel
            // cursor state left on this CRTC instead of stranding a zombie
            // sprite on the mirror.
            output.planes.disabled_cursor = output.planes.cursor.take();
        }
        let discovered = plane_inventory
            .overlay
            .iter()
            .filter(|plane| plane.possible_crtcs.contains(&output.crtc))
            .cloned()
            .collect();
        output.planes.overlay = OverlayPlaneAllocation::disabled(discovered);
    }
    let scanout_formats = match scanout_format_intersection(card, &outputs) {
        Ok(formats) => formats,
        Err(error) => {
            for output in &outputs {
                destroy_output_blobs(card, output);
            }
            return Err(error);
        }
    };
    Ok(DisplaySet {
        outputs,
        size: (desktop_width, desktop_height),
        format,
        modifiers,
        scanout_formats,
        color_mode,
        icc_profile,
    })
}

pub(super) fn intersect_modifier_sets(sets: &[Vec<u64>]) -> Vec<u64> {
    let Some((first, rest)) = sets.split_first() else {
        return Vec::new();
    };
    let mut shared = first.clone();
    shared.retain(|modifier| rest.iter().all(|set| set.contains(modifier)));
    shared.sort_unstable();
    shared.dedup();
    shared
}

fn scanout_format_intersection(
    card: &Card,
    outputs: &[Output],
) -> Result<HashMap<u32, Vec<u64>>, DrmError> {
    let mut result = HashMap::new();
    for format in [
        DrmFourcc::Argb8888,
        DrmFourcc::Xrgb8888,
        DrmFourcc::Abgr8888,
        DrmFourcc::Xbgr8888,
        DrmFourcc::Abgr2101010,
        DrmFourcc::Xbgr2101010,
    ] {
        let mut per_output = Vec::with_capacity(outputs.len());
        for output in outputs {
            let info = card.get_plane(output.planes.primary.handle)?;
            if !info.formats().contains(&(format as u32)) {
                per_output.clear();
                break;
            }
            let modifiers = plane_modifiers(card, output.planes.primary.handle, format)?;
            if modifiers.is_empty() {
                per_output.clear();
                break;
            }
            per_output.push(modifiers);
        }
        if per_output.len() != outputs.len() {
            continue;
        }
        let shared = intersect_modifier_sets(&per_output);
        if !shared.is_empty() {
            result.insert(format as u32, shared);
        }
    }
    Ok(result)
}

fn apply_candidate_roles(
    candidates: &mut [OutputCandidate],
    topology: tessera_desktop::output::DisplayTopology,
    configured_policies: &HashMap<String, tessera_desktop::output::OutputPolicy>,
) {
    let infos = candidates
        .iter()
        .map(|candidate| {
            let (width, height) = candidate.mode.size();
            tessera_desktop::output::OutputInfo {
                connector: candidate.name.clone(),
                geometry: tessera_desktop::output::OutputGeometry {
                    mode: tessera_desktop::output::OutputMode {
                        width: width as i32,
                        height: height as i32,
                        refresh_mhz: candidate.mode.vrefresh().saturating_mul(1_000),
                    },
                    scale: candidate.scale,
                    transform: tessera_primitives::Transform::Normal,
                    logical_origin: tessera_primitives::Point::default(),
                    logical_size_override: None,
                },
                kind: candidate.kind,
                available_modes: candidate.available_modes.clone(),
                color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
                role: tessera_desktop::output::OutputRole::Extend,
                role_source: tessera_desktop::output::OutputRoleSource::Auto,
            }
        })
        .collect::<Vec<_>>();
    let resolved =
        tessera_desktop::output::resolve_output_roles(&infos, configured_policies, topology);
    for (candidate, role) in candidates.iter_mut().zip(resolved) {
        candidate.role = role;
        candidate.role_source = tessera_desktop::output::OutputRoleSource::from_policy(
            configured_policies.get(&candidate.name),
        );
    }
    // Backend boundary: a mirror source must be one of the live candidates and
    // must itself be extended. The resolver already enforces this for the
    // list it was given; keep the check because a caller could pass a partial
    // policy map.
    let sources: std::collections::HashMap<String, bool> = candidates
        .iter()
        .map(|candidate| (candidate.name.clone(), !candidate.role.is_mirror()))
        .collect();
    for candidate in candidates.iter_mut() {
        if let tessera_desktop::output::OutputRole::Mirror { source } = &candidate.role
            && !sources.get(source).copied().unwrap_or(false)
        {
            candidate.role = tessera_desktop::output::OutputRole::Extend;
        }
    }
}

type OutputPlacement = (u32, (u32, u32));
type OutputLayout = (HashMap<String, OutputPlacement>, u32, u32);

/// One connector's contribution to the shared framebuffer layout.
pub(super) struct PlacementInput<'a> {
    pub(super) name: &'a str,
    pub(super) size: (u32, u32),
    pub(super) mirror: Option<&'a str>,
}

/// Resolve every connector's source rectangle `(x, source_size)` and the union
/// framebuffer extent. Extended outputs get disjoint horizontal slices in
/// connector order; mirrors scan their source's slice and contribute no width.
pub(super) fn layout_outputs(inputs: &[PlacementInput<'_>]) -> Result<OutputLayout, DrmError> {
    let mut layout = HashMap::<String, OutputPlacement>::new();
    let mut desktop_width = 0_u32;
    let mut desktop_height = 0_u32;
    for input in inputs {
        if input.mirror.is_some() {
            continue;
        }
        layout.insert(input.name.to_owned(), (desktop_width, input.size));
        desktop_width = desktop_width
            .checked_add(input.size.0)
            .ok_or(DrmError::DesktopTooLarge(u32::MAX, desktop_height))?;
        desktop_height = desktop_height.max(input.size.1);
    }
    if desktop_width == 0 {
        // A session with only mirrors is invalid; treat the first connector as
        // the source rather than failing to light any display.
        if let Some(first) = inputs.first() {
            layout.insert(first.name.to_owned(), (0, first.size));
            desktop_width = first.size.0;
            desktop_height = first.size.1;
        }
    }
    for input in inputs {
        let Some(source) = input.mirror else {
            continue;
        };
        if let Some(&(x, source_size)) = layout.get(source) {
            layout.insert(input.name.to_owned(), (x, source_size));
        }
    }
    Ok((layout, desktop_width, desktop_height))
}

/// Fit `source` into `mode` while preserving the source aspect ratio, centered.
/// Equal aspects produce the full destination; a mismatch produces black bars
/// on the shorter axis instead of a stretched desktop.
pub(super) fn fitted_destination(mode: (u32, u32), source: (u32, u32)) -> (i32, i32, u32, u32) {
    if mode.0 == 0 || mode.1 == 0 || source.0 == 0 || source.1 == 0 {
        return (0, 0, mode.0.max(1), mode.1.max(1));
    }
    let scale = (mode.0 as f32 / source.0 as f32).min(mode.1 as f32 / source.1 as f32);
    let width = ((source.0 as f32 * scale).round() as u32).clamp(1, mode.0);
    let height = ((source.1 as f32 * scale).round() as u32).clamp(1, mode.1);
    (
        ((mode.0 - width) / 2) as i32,
        ((mode.1 - height) / 2) as i32,
        width,
        height,
    )
}

pub(super) fn assign_outputs(
    candidates: &[OutputCandidate],
) -> Option<(Vec<OutputChoice>, Vec<u64>)> {
    // Assign candidates that already own a live CRTC first so a newly plugged
    // connector with a lower interface number (e.g. HDMI-A before eDP) never
    // steals an active display's CRTC and forces a pipe-migration black flash.
    // Backtracking still explores every valid assignment if a hardware
    // constraint requires moving a CRTC.
    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| (candidates[index].preferred_crtc.is_none(), index));

    let ordered_choices: Vec<Vec<&OutputChoice>> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let preferred_by_other: HashSet<crtc::Handle> = candidates
                .iter()
                .enumerate()
                .filter(|&(other_index, _)| other_index != index)
                .filter_map(|(_, other)| other.preferred_crtc)
                .collect();
            let mut choices: Vec<(usize, &OutputChoice)> =
                candidate.choices.iter().enumerate().collect();
            choices.sort_by_key(|&(choice_index, choice)| {
                let crtc_mismatch = candidate
                    .preferred_crtc
                    .is_some_and(|preferred| choice.crtc != preferred);
                let plane_mismatch = candidate
                    .preferred_plane
                    .is_some_and(|preferred| choice.plane != preferred);
                let steals_other = preferred_by_other.contains(&choice.crtc);
                (crtc_mismatch, plane_mismatch, steals_other, choice_index)
            });
            choices.into_iter().map(|(_, choice)| choice).collect()
        })
        .collect();

    fn recurse(
        order: &[usize],
        ordered_choices: &[Vec<&OutputChoice>],
        depth: usize,
        used_crtcs: &mut HashSet<crtc::Handle>,
        used_planes: &mut HashSet<plane::Handle>,
        selected: &mut [Option<OutputChoice>],
        shared: Option<Vec<u64>>,
    ) -> Option<Vec<u64>> {
        if depth == order.len() {
            return Some(shared.unwrap_or_default());
        }
        let candidate_index = order[depth];
        for choice in &ordered_choices[candidate_index] {
            if used_crtcs.contains(&choice.crtc) || used_planes.contains(&choice.plane) {
                continue;
            }
            let next_shared = match &shared {
                Some(current) => current
                    .iter()
                    .copied()
                    .filter(|modifier| choice.modifiers.contains(modifier))
                    .collect::<Vec<_>>(),
                None => choice.modifiers.clone(),
            };
            if next_shared.is_empty() {
                continue;
            }
            used_crtcs.insert(choice.crtc);
            used_planes.insert(choice.plane);
            selected[candidate_index] = Some((*choice).clone());
            if let Some(modifiers) = recurse(
                order,
                ordered_choices,
                depth + 1,
                used_crtcs,
                used_planes,
                selected,
                Some(next_shared),
            ) {
                return Some(modifiers);
            }
            selected[candidate_index] = None;
            used_crtcs.remove(&choice.crtc);
            used_planes.remove(&choice.plane);
        }
        None
    }

    let mut selected = vec![None; candidates.len()];
    let modifiers = recurse(
        &order,
        &ordered_choices,
        0,
        &mut HashSet::new(),
        &mut HashSet::new(),
        &mut selected,
        None,
    )?;
    let resolved = selected.into_iter().collect::<Option<Vec<_>>>()?;
    Some((resolved, modifiers))
}

pub(super) fn build_output(
    card: &Card,
    candidate: OutputCandidate,
    choice: OutputChoice,
    x: u32,
    source_size: (u32, u32),
    destination: (i32, i32, u32, u32),
) -> Result<Output, DrmError> {
    let connector_props = property_map(card, candidate.connector)?;
    let crtc_props = property_map(card, choice.crtc)?;
    let plane_props = property_map(card, choice.plane)?;
    let connector_link_status = optional_prop(&connector_props, "link-status");
    let link_bad = connector_link_status.is_some_and(|prop| {
        card.get_properties(candidate.connector)
            .ok()
            .and_then(|props| {
                props
                    .iter()
                    .find(|&(&id, _)| id == prop)
                    .map(|(_, &value)| value != 0)
            })
            .unwrap_or(false)
    });
    let props = OutputAtomicProperties {
        connector_crtc_id: required_prop(&connector_props, "CRTC_ID")?,
        crtc_mode_id: required_prop(&crtc_props, "MODE_ID")?,
        crtc_active: required_prop(&crtc_props, "ACTIVE")?,
        crtc_out_fence_ptr: optional_prop(&crtc_props, "OUT_FENCE_PTR"),
        connector_colorspace: colorspace_prop(&connector_props),
        connector_hdr_metadata: optional_prop(&connector_props, "HDR_OUTPUT_METADATA"),
        connector_max_bpc: optional_prop(&connector_props, "max bpc"),
        crtc_gamma_lut: gamma_lut_prop(card, choice.crtc, &crtc_props),
        connector_link_status,
    };
    let primary_props = PrimaryPlaneProperties {
        fb_id: required_prop(&plane_props, "FB_ID")?,
        crtc_id: required_prop(&plane_props, "CRTC_ID")?,
        src_x: required_prop(&plane_props, "SRC_X")?,
        src_y: required_prop(&plane_props, "SRC_Y")?,
        src_w: required_prop(&plane_props, "SRC_W")?,
        src_h: required_prop(&plane_props, "SRC_H")?,
        crtc_x: required_prop(&plane_props, "CRTC_X")?,
        crtc_y: required_prop(&plane_props, "CRTC_Y")?,
        crtc_w: required_prop(&plane_props, "CRTC_W")?,
        crtc_h: required_prop(&plane_props, "CRTC_H")?,
        in_fence_fd: optional_prop(&plane_props, "IN_FENCE_FD"),
        fb_damage_clips: optional_prop(&plane_props, "FB_DAMAGE_CLIPS"),
    };
    let mode_blob = card.create_property_blob(&candidate.mode)?;
    let property::Value::Blob(mode_blob_id) = mode_blob else {
        unreachable!("create_property_blob always returns Blob")
    };
    // A reusable clip covering this output's whole framebuffer rectangle for
    // commits whose damage is unknown or spans the output. The blob layout is
    // an array of `drm_mode_rect` — four native i32 values (x1, y1, x2, y2).
    // A mirrored output covers its *source* rectangle, not its own native
    // mode: the plane scales that source to the mirror's CRTC size.
    let (width, height) = source_size;
    let full_damage_blob = if primary_props.fb_damage_clips.is_some() {
        let rect = [[x as i32, 0, (x + width) as i32, height as i32]];
        let blob = card
            .create_property_blob(&rect[..])
            .ok()
            .and_then(|value| match value {
                property::Value::Blob(id) => Some((value, id)),
                _ => None,
            });
        if blob.is_none() {
            log::warn!("drm: FB_DAMAGE_CLIPS blob allocation failed; damage hints disabled");
        }
        blob
    } else {
        None
    };
    Ok(Output {
        connector: candidate.connector,
        name: candidate.name,
        crtc: choice.crtc,
        planes: OutputPlanes {
            primary: PrimaryPlane {
                handle: choice.plane,
                props: primary_props,
                full_damage_blob,
            },
            cursor: None,
            disabled_cursor: None,
            overlay: OverlayPlaneAllocation::disabled(Vec::new()),
        },
        mode: candidate.mode,
        source_size,
        destination,
        role: candidate.role,
        role_source: candidate.role_source,
        mode_blob,
        mode_blob_id,
        x,
        y: 0,
        physical_size_mm: candidate.physical_size_mm,
        ppi: candidate.ppi,
        kind: candidate.kind,
        scale: candidate.scale,
        props,
        available_modes: candidate.available_modes,
        color_caps: candidate.color_caps,
        link_bad,
    })
}

/// The connector's advertised modes, deduplicated by (width, height,
/// refresh) and sorted by pixel count then refresh rate, highest first — the
/// order `tessera display` presents them in.
pub(super) fn advertised_modes(info: &connector::Info) -> Vec<OutputMode> {
    let mut modes: Vec<OutputMode> = info
        .modes()
        .iter()
        .map(|mode| {
            let (width, height) = mode.size();
            OutputMode {
                width: width as i32,
                height: height as i32,
                refresh_mhz: mode.vrefresh().saturating_mul(1_000),
            }
        })
        .collect();
    modes.sort_by(|a, b| {
        (i64::from(b.width) * i64::from(b.height), b.refresh_mhz)
            .cmp(&(i64::from(a.width) * i64::from(a.height), a.refresh_mhz))
    });
    modes.dedup();
    modes
}

/// Choose a mode index out of `modes` — `(width, height, refresh_mhz,
/// preferred)` tuples in connector order — honoring an optional configured
/// spec (ADR-0028). Without a spec, the highest pixel count wins, then the
/// highest refresh rate. With a spec, matches require exact width/height
/// (plus a whole-Hz refresh match when the spec names one), then the highest
/// refresh rate wins. The PREFERRED flag and connector order break otherwise
/// equal ties. `None` means nothing matched (or `modes` is empty); the caller
/// falls back to the no-spec rule.
pub(super) fn pick_mode(modes: &[(i32, i32, u32, bool)], spec: Option<&ModeSpec>) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .filter(|&(_, &(width, height, refresh_mhz, _))| {
            spec.is_none_or(|spec| {
                spec.matches(&OutputMode {
                    width,
                    height,
                    refresh_mhz,
                })
            })
        })
        .max_by_key(|&(index, &(width, height, refresh_mhz, preferred))| {
            let pixels = if spec.is_none() {
                i64::from(width) * i64::from(height)
            } else {
                0
            };
            (pixels, refresh_mhz, preferred, std::cmp::Reverse(index))
        })
        .map(|(index, _)| index)
}

pub(super) fn display_signature(displays: &DisplaySet) -> DisplaySignature {
    let mut scanout_formats = displays
        .scanout_formats
        .iter()
        .map(|(format, modifiers)| (*format, modifiers.clone()))
        .collect::<Vec<_>>();
    scanout_formats.sort_unstable_by_key(|(format, _)| *format);
    (
        displays.format,
        displays.modifiers.clone(),
        scanout_formats,
        displays.color_mode,
        displays.icc_profile.clone(),
        displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                OutputSignature {
                    name: output.name.clone(),
                    width: width as u32,
                    height: height as u32,
                    refresh: output.mode.vrefresh(),
                    scale_bits: output.scale.as_f32().to_bits(),
                    x: output.x,
                    y: output.y,
                    cursor: output.planes.cursor.is_some(),
                    mirror: output.role.is_mirror(),
                    role_source: output.role_source,
                    source_width: output.source_size.0,
                    source_height: output.source_size.1,
                    destination: output.destination,
                    overlay: output.planes.overlay.clone(),
                }
            })
            .collect(),
    )
}

fn destroy_output_blobs(card: &Card, output: &Output) {
    let _ = card.destroy_property_blob(output.mode_blob_id);
    if let Some((_, blob_id)) = output.planes.primary.full_damage_blob.as_ref() {
        let _ = card.destroy_property_blob(*blob_id);
    }
}

pub(super) fn property_map<H: ResourceHandle>(
    card: &Card,
    handle: H,
) -> Result<HashMap<String, property::Info>, DrmError> {
    Ok(card.get_properties(handle)?.as_hashmap(card)?)
}

pub(super) fn required_prop(
    props: &HashMap<String, property::Info>,
    name: &'static str,
) -> Result<property::Handle, DrmError> {
    props
        .get(name)
        .map(property::Info::handle)
        .ok_or(DrmError::MissingProperty(name))
}

pub(super) fn optional_prop(
    props: &HashMap<String, property::Info>,
    name: &str,
) -> Option<property::Handle> {
    props.get(name).map(property::Info::handle)
}

/// Harvest the connector `Colorspace` property handle plus its `Default`
/// and `BT2020_RGB` enum values. `None` when the driver lacks the property
/// or either value (older kernels expose it only on some connectors).
fn colorspace_prop(
    props: &HashMap<String, property::Info>,
) -> Option<(property::Handle, u64, u64)> {
    let info = props.get("Colorspace")?;
    let property::ValueType::Enum(values) = info.value_type() else {
        return None;
    };
    let (_, variants) = values.values();
    let find = |name: &std::ffi::CStr| {
        variants
            .iter()
            .find(|variant| variant.name() == name)
            .map(property::EnumValue::value)
    };
    Some((info.handle(), find(c"Default")?, find(c"BT2020_RGB")?))
}

/// Harvest the CRTC's `GAMMA_LUT` handle and `GAMMA_LUT_SIZE` value (the
/// driver's table entry count). `None` on drivers without gamma tables.
fn gamma_lut_prop(
    card: &Card,
    crtc: crtc::Handle,
    props: &HashMap<String, property::Info>,
) -> Option<(property::Handle, u32)> {
    let handle = optional_prop(props, "GAMMA_LUT")?;
    let properties = card.get_properties(crtc).ok()?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id).ok()?;
        if info.name() == c"GAMMA_LUT_SIZE" {
            return Some((handle, value as u32));
        }
    }
    None
}

/// Read the connector's EDID blob and parse its HDR/wide-gamut
/// capabilities. Missing or unreadable EDID yields all-false (SDR).
fn connector_color_caps(
    card: &Card,
    handle: connector::Handle,
) -> tessera_primitives::edid::EdidColorCapabilities {
    let read = (|| {
        let props = card.get_properties(handle).ok()?;
        for (&id, &value) in props.iter() {
            let info = card.get_property(id).ok()?;
            // The raw value of a blob property is the blob id.
            if info.name() == c"EDID" {
                let blob = card.get_property_blob(value).ok()?;
                return Some(tessera_primitives::edid::edid_color_capabilities(&blob));
            }
        }
        None
    })();
    read.unwrap_or_default()
}

/// Return modifiers accepted by `plane` for `format`. Drivers predating the
/// IN_FORMATS property expose only the legacy implicit-layout contract, whose
/// portable dma-buf representation is linear.
pub(super) fn plane_modifiers(
    card: &Card,
    plane: plane::Handle,
    format: DrmFourcc,
) -> Result<Vec<u64>, DrmError> {
    let properties = card.get_properties(plane)?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id)?;
        if info.name().to_bytes() == b"IN_FORMATS" {
            if value == 0 {
                return Ok(vec![u64::from(DrmModifier::Linear)]);
            }
            let blob = card.get_property_blob(value)?;
            return parse_format_modifiers(&blob, format as u32);
        }
    }
    Ok(vec![u64::from(DrmModifier::Linear)])
}

/// Parse Linux's `drm_format_modifier_blob` without casting untrusted kernel
/// offsets to native structs. All bounds and arithmetic are checked first.
pub(super) fn parse_format_modifiers(blob: &[u8], format: u32) -> Result<Vec<u64>, DrmError> {
    const HEADER: usize = 24;
    const MODIFIER_RECORD: usize = 24;
    if blob.len() < HEADER {
        return Err(DrmError::MalformedFormats("short header"));
    }
    let read_u32 = |offset: usize| -> Result<u32, DrmError> {
        let bytes = blob
            .get(offset..offset + 4)
            .ok_or(DrmError::MalformedFormats("u32 outside blob"))?;
        Ok(u32::from_ne_bytes(bytes.try_into().unwrap()))
    };
    let read_u64 = |offset: usize| -> Result<u64, DrmError> {
        let bytes = blob
            .get(offset..offset + 8)
            .ok_or(DrmError::MalformedFormats("u64 outside blob"))?;
        Ok(u64::from_ne_bytes(bytes.try_into().unwrap()))
    };

    let count_formats = read_u32(8)? as usize;
    let formats_offset = read_u32(12)? as usize;
    let count_modifiers = read_u32(16)? as usize;
    let modifiers_offset = read_u32(20)? as usize;
    let formats_bytes = count_formats
        .checked_mul(4)
        .and_then(|size| formats_offset.checked_add(size))
        .ok_or(DrmError::MalformedFormats("format array overflow"))?;
    let modifiers_bytes = count_modifiers
        .checked_mul(MODIFIER_RECORD)
        .and_then(|size| modifiers_offset.checked_add(size))
        .ok_or(DrmError::MalformedFormats("modifier array overflow"))?;
    if formats_offset < HEADER || formats_bytes > blob.len() {
        return Err(DrmError::MalformedFormats("format array outside blob"));
    }
    if modifiers_offset < HEADER || modifiers_bytes > blob.len() {
        return Err(DrmError::MalformedFormats("modifier array outside blob"));
    }

    let Some(format_index) =
        (0..count_formats).find(|index| read_u32(formats_offset + index * 4).ok() == Some(format))
    else {
        return Ok(Vec::new());
    };
    let mut modifiers = Vec::new();
    for index in 0..count_modifiers {
        let base = modifiers_offset + index * MODIFIER_RECORD;
        let formats = read_u64(base)?;
        let offset = read_u32(base + 8)? as usize;
        if format_index >= offset
            && format_index - offset < 64
            && formats & (1_u64 << (format_index - offset)) != 0
        {
            let modifier = read_u64(base + 16)?;
            if modifier != u64::from(DrmModifier::Invalid) && !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
        }
    }
    // Prefer a device-native tiled layout for the compositor render target.
    // LINEAR is the compatibility fallback: making it the first choice turns
    // every animated full-output frame into an uncompressed linear write,
    // which is particularly expensive at HiDPI/high refresh rates. Keep the
    // ordering deterministic while placing LINEAR after every native layout;
    // Flux consumes this list as a producer preference order.
    modifiers.sort_by_key(|modifier| (*modifier == u64::from(DrmModifier::Linear), *modifier));
    Ok(modifiers)
}
