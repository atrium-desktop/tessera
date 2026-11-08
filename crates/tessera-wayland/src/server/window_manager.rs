use crate::*;

/// Whether `rec` is an independent window-switcher candidate (ADR-0148): a
/// mapped, unminimized toplevel that the seat controls and that is not a
/// cross-client dialog. The switcher enumerates trees, not toplevels: a
/// prompter imported over `zxdg_importer_v2` rides the requesting app's
/// card, and the modal-landing rule carries focus to it when that card is
/// chosen. In-app dialogs stay candidates — they are stackable windows of
/// the application itself.
fn switcher_candidate(
    s: &SurfaceRec,
    visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    seat_controls: bool,
) -> bool {
    !s.xdg_toplevel.is_null()
        && s.mapped
        && !s.window.minimized
        && visible.contains(&s.window.id)
        && seat_controls
        && !unsafe { is_cross_client_dialog(s as *const SurfaceRec as *mut SurfaceRec) }
}

impl Server {
    fn switcher_candidates(&self) -> Vec<tessera_desktop::window::WindowId> {
        let visible = self.visible();
        let mut candidates: Vec<_> = self
            .state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| {
                switcher_candidate(
                    s,
                    &visible,
                    self.state
                        .authority
                        .seat_controls_window(self.state.active_seat, s.window.id),
                )
            })
            .map(|s| s.window.id)
            .collect();
        candidates.reverse();
        candidates
    }

    /// Freeze the current most-recently-used order for a held-Super cycle.
    pub fn start_window_switcher(&mut self) {
        if self.state.window_switcher.is_some() {
            return;
        }
        let order = self.switcher_candidates();
        let selected = self
            .focused_toplevel_id()
            .and_then(|id| order.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        self.state.window_switcher = Some(WindowSwitcherSession {
            order,
            selected,
            last_forward: true,
        });
    }

    /// Commit the current preview selection, then end the held-Super session.
    /// This is the only keyboard-switcher path that raises a window or moves
    /// the Wayland keyboard focus.
    pub fn finish_window_switcher(&mut self) {
        let selected = self
            .window_switcher_snapshot()
            .and_then(|(_, selected)| selected);
        self.state.window_switcher = None;
        if let Some(selected) = selected {
            self.focus_surface_by_id(selected);
            self.rehit_pointer_after_stack_change();
        }
    }

    /// Dismiss the held-Super session without applying its preview selection.
    pub fn cancel_window_switcher(&mut self) {
        self.state.window_switcher = None;
    }

    pub fn window_switcher_active(&self) -> bool {
        self.state.window_switcher.is_some()
    }

    /// Update the preview target in a frozen MRU order while a switcher
    /// session is active. Outside a held session this remains an immediate,
    /// one-shot focus command for IPC and non-session callers.
    pub fn cycle_focus(&mut self, forward: bool) {
        if self.state.window_switcher.is_none() {
            let order = self.switcher_candidates();
            if order.len() < 2 {
                return;
            }
            let selected = self
                .focused_toplevel_id()
                .and_then(|id| order.iter().position(|candidate| *candidate == id))
                .unwrap_or(0);
            let next = order[stepped_index(selected, order.len(), forward)];
            self.focus_surface_by_id(next);
            self.rehit_pointer_after_stack_change();
            return;
        }

        self.refresh_window_switcher();
        let Some(session) = self.state.window_switcher.as_mut() else {
            return;
        };
        if session.order.len() < 2 {
            return;
        }
        session.selected = stepped_index(session.selected, session.order.len(), forward);
        session.last_forward = forward;
    }

    /// Return the frozen order and latest preview target for shell rendering.
    /// Closed windows are pruned first; new windows are never inserted into an
    /// active session.
    pub fn window_switcher_snapshot(
        &mut self,
    ) -> Option<(
        Vec<tessera_desktop::window::WindowId>,
        Option<tessera_desktop::window::WindowId>,
    )> {
        self.refresh_window_switcher();
        self.state.window_switcher.as_ref().map(|session| {
            (
                session.order.clone(),
                session.order.get(session.selected).copied(),
            )
        })
    }

    fn refresh_window_switcher(&mut self) {
        let eligible: std::collections::HashSet<_> =
            self.switcher_candidates().into_iter().collect();
        let Some(session) = self.state.window_switcher.as_mut() else {
            return;
        };
        reconcile_switcher_session(session, &eligible);
    }

    /// Switch to an adjacent workspace on the focused output (ADR-0025). The
    /// visible set changes on the next frame; if the focused toplevel is no
    /// longer visible, keyboard focus is dropped (a `wl_keyboard.leave` is
    /// posted) so keystrokes do not route to a hidden window.
    pub fn switch_workspace(&mut self, dir: tessera_desktop::workspace::Switch) {
        let old = self.state.workspaces.current_workspace(self.state.output);
        let new = self.state.workspaces.switch(self.state.output, dir);
        if let (Some(old), Some(new)) = (old, new)
            && old != new
        {
            let direction = match dir {
                tessera_desktop::workspace::Switch::Next => 1,
                tessera_desktop::workspace::Switch::Prev => -1,
            };
            self.begin_workspace_slide(old, new, direction);
        }
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Switch directly to a workspace by id on the output that owns it. Same
    /// focus-drop contract as [`switch_workspace`](Self::switch_workspace).
    pub fn switch_workspace_to(&mut self, id: tessera_desktop::workspace::WorkspaceId) {
        let Some(target_output) = self
            .state
            .workspaces
            .workspace(id)
            .map(|workspace| workspace.output)
        else {
            return;
        };
        let old = self.state.workspaces.current_workspace(target_output);
        let old_index = old.and_then(|workspace| {
            self.state
                .workspaces
                .output(target_output)?
                .workspaces
                .iter()
                .position(|candidate| *candidate == workspace)
        });
        let new_index = self
            .state
            .workspaces
            .output(target_output)
            .and_then(|output| {
                output
                    .workspaces
                    .iter()
                    .position(|candidate| *candidate == id)
            });
        let new = self.state.workspaces.switch_to(id);
        if let (Some(old), Some(new)) = (old, new)
            && old != new
        {
            let direction = if new_index.unwrap_or(0) >= old_index.unwrap_or(0) {
                1
            } else {
                -1
            };
            self.begin_workspace_slide(old, new, direction);
        }
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Move a toplevel to a workspace by id (ADR-0025). If the target is not
    /// the current workspace, the window leaves the visible set. No-op if the
    /// window or workspace is unknown, or the physical session only observes
    /// the window.
    pub fn move_to_workspace(
        &mut self,
        window_id: tessera_desktop::window::WindowId,
        workspace: tessera_desktop::workspace::WorkspaceId,
    ) {
        if !self.human_controls_window(window_id) {
            return;
        }
        let old_connector =
            human_output_connector_for_window(&self.state, window_id).map(str::to_owned);
        let old_rect = self.state.output_rect_for_window(window_id);
        self.state.workspaces.move_toplevel(window_id, workspace);
        let descendants = self.transient_descendant_window_ids(window_id);
        for child_id in &descendants {
            self.state.workspaces.move_toplevel(*child_id, workspace);
        }
        let new_connector =
            human_output_connector_for_window(&self.state, window_id).map(str::to_owned);
        let new_rect = self.state.output_rect_for_window(window_id);
        if old_rect != new_rect {
            self.translate_window_tree_between_outputs(window_id, &descendants, old_rect, new_rect);
        }
        if old_connector != new_connector {
            unsafe {
                post_window_human_output_transition(
                    &self.state,
                    window_id,
                    old_connector.as_deref(),
                    new_connector.as_deref(),
                );
                for child_id in &descendants {
                    post_window_human_output_transition(
                        &self.state,
                        *child_id,
                        old_connector.as_deref(),
                        new_connector.as_deref(),
                    );
                }
            }
        }
        self.drop_focus_if_hidden();
    }

    /// The workspace/output snapshot for the IPC and chrome (ADR-0025/0027).
    pub fn workspace_snapshot(&self) -> tessera_desktop::workspace::WorkspaceSnapshot {
        self.state.workspaces.snapshot()
    }

    /// Cheap content hash of the workspace model. The frame loop compares
    /// this per frame and only rebuilds the owned
    /// [`Self::workspace_snapshot`] when it moves.
    pub fn workspace_signature(&self) -> u64 {
        self.state.workspaces.signature()
    }

    /// Revision of the backend-reported output list, bumped on every
    /// mutation. Lets the frame loop skip re-cloning
    /// [`Self::output_infos`] while unchanged.
    pub fn outputs_revision(&self) -> u64 {
        self.state.outputs_revision
    }

    /// Whether the current workspace is in tiled mode.
    pub fn tiling(&self) -> bool {
        false
    }

    /// Set tiling state (no-op: Tessera is purely floating).
    pub fn set_tiling(&mut self, _on: bool) {}

    /// Set default tiling state (no-op: Tessera is purely floating).
    pub fn set_tiling_default(&mut self, _on: bool) {}

    /// Replace the window rules (ADR-0026). Called at startup and on config
    /// reload. Rules apply to windows mapped after they are set.
    pub fn set_window_rules(&mut self, rules: Vec<tessera_desktop::window_rule::WindowRule>) {
        self.state.window_rules = rules;
    }

    /// Set whether window positions and geometries are remembered across restarts.
    pub fn set_remember_window_positions(&mut self, remember: bool) {
        self.state.remember_window_positions = remember;
    }

    /// Replace the tiling layout parameters (gaps, master ratio) from the
    /// config (ADR-0024/0026). Applied on the next `apply_tiling`.
    pub fn set_layout_params(&mut self, params: tessera_desktop::layout::LayoutParams) {
        self.state.layout_params = params;
    }

    /// Set the reduced-motion policy (ADR-0029, from `[ui] reduced_motion`).
    /// When enabled, in-flight transitions resolve immediately and no new
    /// ones are recorded.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.state.reduced_motion = reduced;
        if reduced {
            self.state.workspace_slide = None;
            for p in self.state.live_surfaces() {
                unsafe { (*p).window.transition = None };
            }
        }
    }

    /// Set the physical keyboard accessibility filter policy (ADR-0166).
    pub fn set_keyboard_accessibility(
        &mut self,
        config: tessera_primitives::accessibility::KeyboardAccessibilityConfig,
    ) {
        self.state.keyboard_accessibility.set_config(config);
    }

    /// Set the minimize flight style (`[dock] minimize_animation`). Applies
    /// to the next minimize/restore; in-flight transitions keep their style.
    pub fn set_minimize_animation(&mut self, style: tessera_desktop::dock::MinimizeAnimationStyle) {
        self.state.minimize_animation = style;
    }

    /// Set the keyboard repeat policy (`[input.keyboard]`), advertised to
    /// clients as `wl_keyboard.repeat_info` (ADR-0010). New keyboards and
    /// input-method grabs bind with the new values immediately;
    /// already-bound version-4+ keyboards receive a fresh `repeat_info`
    /// event, which the protocol explicitly allows at any time. Live grabs
    /// re-grab on the next focus change and pick the values up then.
    pub fn set_keyboard_repeat(&mut self, config: tessera_primitives::input::KeyboardConfig) {
        self.state.keyboard_repeat = config;
        for runtime in self.state.seats.values_mut() {
            for resource in &runtime.keyboard_resources {
                if unsafe { ffi::wl_resource_get_version(*resource) } >= 4 {
                    unsafe {
                        ffi::wl_resource_post_event(
                            *resource,
                            ffi::WL_KEYBOARD_REPEAT_INFO,
                            config.repeat_rate,
                            config.repeat_delay_ms,
                        );
                    };
                }
            }
        }
    }

    /// Replace the minimize flight targets — the resting dock-icon rect per
    /// window — with the shell's latest report. Pushed every frame so even a
    /// client-initiated `xdg_toplevel.set_minimized` flies at the real icon.
    /// The map is rebuilt only when the report actually changed: in the
    /// steady state (no minimize in flight, dock layout static) the report
    /// is identical frame over frame, and rebuilding the HashMap per frame
    /// was pure allocation churn on the render path.
    pub fn set_minimize_targets(
        &mut self,
        targets: Vec<(tessera_desktop::window::WindowId, tessera_primitives::Rect)>,
    ) {
        let unchanged = targets.len() == self.state.minimize_targets.len()
            && targets
                .iter()
                .all(|(id, rect)| self.state.minimize_targets.get(id) == Some(rect));
        if unchanged {
            return;
        }
        self.state.minimize_targets = targets.into_iter().collect();
    }

    /// Apply the desktop-wide xdg-decoration ownership policy.
    ///
    /// Existing decoration-aware toplevels receive a fresh decoration
    /// configure followed by the required xdg-surface configure, so config
    /// reload changes take effect without restarting applications.
    pub fn set_decoration_policy(&mut self, policy: tessera_desktop::window::DecorationPolicy) {
        if self.state.decoration_policy == policy {
            return;
        }
        self.state.decoration_policy = policy;
        for rec in self.state.live_surfaces() {
            unsafe {
                if !(*rec).xdg_decoration.is_null() {
                    extensions::configure_decoration((*rec).xdg_decoration, rec);
                }
            }
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Compositor-relative millisecond timestamp for transition records.
    pub(crate) fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn begin_workspace_slide(
        &mut self,
        outgoing: tessera_desktop::workspace::WorkspaceId,
        incoming: tessera_desktop::workspace::WorkspaceId,
        direction: i32,
    ) {
        if self.state.reduced_motion {
            self.state.workspace_slide = None;
            return;
        }
        let output = self
            .state
            .workspaces
            .workspace(outgoing)
            .and_then(|workspace| self.state.workspaces.output(workspace.output))
            .and_then(|workspace_output| {
                self.state
                    .output_infos
                    .iter()
                    .find(|output| output.connector == workspace_output.connector)
            })
            .map(|output| output.geometry.logical_rect())
            .unwrap_or_else(|| self.output_logical_rect());
        let width = output.size.w.max(1) as f32;
        let now = self.now_ms();
        let previous = self.state.workspace_slide.take();
        let positions = previous
            .as_ref()
            .filter(|slide| slide.output == output && slide.is_active_at(now))
            .map(|slide| {
                slide
                    .layers
                    .iter()
                    .filter_map(|layer| {
                        slide
                            .offset_at(layer.workspace, now)
                            .map(|offset| (layer.workspace, offset))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let layers = retarget_workspace_strip(positions, outgoing, incoming, direction, width);
        self.state.workspace_slide = Some(WorkspaceSlide {
            output,
            layers,
            started_ms: now,
            duration_ms: WORKSPACE_SLIDE_DURATION_MS,
        });
    }

    pub fn workspace_slide_pending(&self) -> bool {
        self.state
            .workspace_slide
            .as_ref()
            .is_some_and(|slide| slide.is_active_at(self.now_ms()))
    }

    pub fn workspace_slide_presentation(&self) -> Option<WorkspaceSlidePresentation> {
        let now = self.now_ms();
        let slide = self
            .state
            .workspace_slide
            .as_ref()
            .filter(|slide| slide.is_active_at(now))?;
        let layers = slide
            .layers
            .iter()
            .filter_map(|layer| {
                let offset_x = slide.offset_at(layer.workspace, now)?;
                let windows = self
                    .state
                    .workspaces
                    .workspace(layer.workspace)
                    .map(|workspace| workspace.toplevels.clone())
                    .unwrap_or_default();
                Some(WorkspaceSlideLayerPresentation { windows, offset_x })
            })
            .collect();
        Some(WorkspaceSlidePresentation {
            output: slide.output,
            layers,
        })
    }

    /// Current workspace surfaces plus the source surfaces retained by a
    /// live workspace slide. This is presentation visibility only; input and
    /// focus continue to use [`Self::visible`].
    pub(crate) fn render_visible(
        &self,
    ) -> std::collections::HashSet<tessera_desktop::window::WindowId> {
        let mut visible = self.visible();
        if let Some(slide) = self.workspace_slide_presentation() {
            visible.extend(slide.layers.into_iter().flat_map(|layer| layer.windows));
        }
        visible
    }

    /// Normalize geometry and workspace transition state after each animation
    /// interval has elapsed. Transition records are state, not history:
    /// keeping settled values makes scene consumers mistake old animation for
    /// live work (notably disabling opaque coverage or retaining source
    /// workspace surfaces).
    ///
    /// The runtime calls this once at the start of every event iteration. An
    /// active transition schedules those iterations until its deadline, so the
    /// terminal tick always retires it even if no client submits another
    /// buffer.
    pub fn settle_finished_transitions(&mut self) -> usize {
        let now = self.now_ms();
        let mut settled = 0;
        // Retire activation tokens past their redemption window. The sweep
        // rides this per-iteration call (it already exists to retire settled
        // animation records) so a client that commits tokens and never
        // activates — then disconnects — cannot pin them forever.
        crate::extensions::activation::expire_activation_tokens(&mut self.state, now);
        if self
            .state
            .workspace_slide
            .as_ref()
            .is_some_and(|slide| !slide.is_active_at(now))
        {
            self.state.workspace_slide = None;
            settled += 1;
        }
        // A retired ghost's last painted pixels are still on the output. The
        // desktop beneath them must be recomposited exactly once, on a frame
        // that reports the ghost's rect as damage and keeps the loop ticking —
        // otherwise the fade's final frame is never overwritten and the window
        // appears to switch off instead of finishing its fade (ADR-0220).
        let mut retired_ghosts: Vec<tessera_primitives::Rect> = Vec::new();
        self.state.closing_frames.retain(|frame| {
            let active = frame.transition.is_active_at(now);
            if !active {
                retired_ghosts.push(frame.rect);
            }
            active
        });
        settled += retired_ghosts.len();
        self.state.note_retired_ghost_damage(&retired_ghosts);
        for pointer in self.state.live_surfaces() {
            // SAFETY: `live_surfaces` yields non-null records owned by this
            // single-threaded server for the duration of the iteration.
            let surface = unsafe { &mut *pointer };
            if surface
                .window
                .transition
                .is_some_and(|transition| !transition.is_active_at(now))
            {
                surface.window.transition = None;
                settled += 1;
            }
        }
        settled
    }

    /// Record a geometry transition for a non-interactive rect change
    /// (tiling, IPC geometry). The model moves to the target immediately;
    /// rendering interpolates from the current on-screen rect — the previous
    /// transition's mid-flight rect when changes come faster than the
    /// duration, else the previous model rect (ADR-0029).
    pub(crate) fn note_transition(&self, rec: *mut SurfaceRec, old: tessera_primitives::Rect) {
        if self.state.reduced_motion || rec.is_null() {
            return;
        }
        let now = self.now_ms();
        unsafe {
            let target = tessera_primitives::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            if old == target {
                (*rec).window.transition = None;
                return;
            }
            let from = (*rec)
                .window
                .transition
                .and_then(|t| t.rect_at(old, now))
                .unwrap_or(old);
            (*rec).window.transition = Some(tessera_desktop::transition::WindowTransition::new(
                from, now,
            ));
        }
    }

    /// Ghost frames for windows that are closing (fading out) right now,
    /// snapshotted for the renderer. Each entry carries its interpolated rect
    /// and fading opacity at the current frame time.
    pub fn closing_frame_views(&self) -> Vec<tessera_primitives::ClosingGhostView<'_>> {
        let now = self.now_ms();
        self.state
            .closing_frames
            .iter()
            .filter_map(|frame| {
                let rect = frame.rect_at(now)?;
                let opacity = frame.opacity_at(now).unwrap_or(0.0);
                let dmabuf = frame.dmabuf.as_ref();
                Some(tessera_primitives::ClosingGhostView {
                    id: frame.id,
                    rect,
                    buffer_width: frame.buffer_width,
                    buffer_height: frame.buffer_height,
                    pixels: &frame.pixels,
                    dmabuf_fd: dmabuf.map_or(-1, |db| db.fd),
                    drm_format: dmabuf.map_or(0, |db| db.drm_format),
                    modifier: dmabuf.map_or(0, |db| db.modifier),
                    stride: dmabuf.map_or(0, |db| db.stride),
                    offset: dmabuf.map_or(0, |db| db.offset),
                    opacity,
                    color: frame.color.clone(),
                })
            })
            .collect()
    }

    /// The rect a surface renders at this frame: `Some(interpolated)` while
    /// its transition is in flight, `None` at the model target (ADR-0029).
    pub(crate) fn transition_render_rect(
        &self,
        s: &SurfaceRec,
    ) -> Option<tessera_primitives::Rect> {
        let target = tessera_primitives::Rect {
            origin: s.position,
            size: s.window.size,
        };
        s.window
            .transition
            .and_then(|t| t.rect_at(target, self.now_ms()))
    }

    /// Whether any animation record is still in flight *or* a settled ghost
    /// still owes its hand-off repaint — the main loop keeps ticking frames
    /// instead of blocking on the host.
    pub fn transitions_pending(&self) -> bool {
        self.live_transitions_pending() || self.state.retired_ghost_damage.pending()
    }

    /// Whether a workspace slide, a closing ghost, or a toplevel geometry
    /// transition is *currently animating*.
    ///
    /// Distinct from [`Self::transitions_pending`] because the two answer
    /// different damage questions. An animating transition can move a window,
    /// its software shadow, and the pixels it uncovers, so a frame it drives has
    /// to be assessed in full. A settled ghost's hand-off frame changes exactly
    /// one thing — the rect the ghost painted — and reporting it as a full-output
    /// repaint would make the recorded hand-off damage dead on arrival
    /// (ADR-0220).
    pub fn live_transitions_pending(&self) -> bool {
        self.workspace_slide_pending()
            || self
                .state
                .closing_frames
                .iter()
                .any(|frame| frame.transition.is_active_at(self.now_ms()))
            || self.state.live_surfaces().any(|p| unsafe {
                !(*p).xdg_toplevel.is_null() && self.transition_render_rect(&*p).is_some()
            })
    }

    /// Bounding damage rectangles of all window geometry transitions currently
    /// in flight (closing ghosts, minimize/restore, and open transitions).
    ///
    /// Also drains the hand-off rects of ghosts that settled since the last
    /// assessment: the frame that retires a ghost must repaint the area it
    /// covered, or its last (near-transparent but not empty) frame stays on the
    /// output until something unrelated happens to dirty it (ADR-0220).
    pub fn transition_damage_rects(&mut self) -> Vec<tessera_primitives::Rect> {
        let mut rects = self.state.retired_ghost_damage.take();
        let now = self.now_ms();
        for frame in &self.state.closing_frames {
            if frame.transition.is_active_at(now) {
                // The ghost shrinks toward its inset target while it fades, so
                // the repaint must cover both the sampling from-rect and the
                // rect it currently occupies.
                match frame.rect_at(now) {
                    Some(r) => rects.push(r.union(frame.rect)),
                    None => rects.push(frame.rect),
                }
            }
        }
        for p in self.state.live_surfaces() {
            unsafe {
                if !(*p).xdg_toplevel.is_null() {
                    let target = tessera_primitives::Rect {
                        origin: (*p).position,
                        size: (*p).window.size,
                    };
                    if let Some(r) = self.transition_render_rect(&*p) {
                        rects.push(r.union(target));
                    }
                }
            }
        }
        rects
    }

    /// Reconcile connector identities and geometries reported by the physical
    /// display backend. Existing connector workspaces survive reordering;
    /// active virtual extend outputs (`VIRTUAL-*`, ADR-0186) survive physical
    /// hotplugs, and removed outputs relocate their workspaces and windows to
    /// the primary physical output.
    pub fn set_outputs(&mut self, mut outputs: Vec<tessera_desktop::output::OutputInfo>) {
        outputs.retain(|output| !output.connector.is_empty());
        let mut seen = std::collections::HashSet::new();
        outputs.retain(|output| seen.insert(output.connector.clone()));
        if outputs.is_empty() && self.state.virtual_outputs.is_empty() {
            return;
        }
        // `set_outputs` is the physical backend entry point: ensure every
        // entry carries a physical OutputKind (`[INV-ARCH-74]`).
        for output in &mut outputs {
            if output.kind.is_virtual() {
                output.kind = tessera_desktop::output::OutputKind::External;
            }
        }
        self.state.physical_outputs = outputs;
        self.reconcile_outputs();
    }

    fn apply_output_policy_to_info(
        policies: &std::collections::HashMap<String, tessera_desktop::output::OutputPolicy>,
        output: &mut tessera_desktop::output::OutputInfo,
    ) {
        let Some(policy) = policies.get(&output.connector) else {
            return;
        };
        if let Some(scale) = policy.scale {
            let configured = tessera_desktop::output::Scale(scale as f32);
            if configured.as_f32() != configured.as_f32().round() {
                // A fractional scale is representable (wp_fractional_scale_v1)
                // but every client that does not speak it renders at
                // ceil(scale) x its logical extent: a fill-rate tax on the
                // whole desktop and a buffer that can never match the output
                // mode, so direct scanout is permanently disqualified for
                // covering clients. See ADR-0184.
                log::warn!(
                    "[server] output '{}': fractional scale {:.2} makes clients without \
                     wp_fractional_scale_v1 render at {}x and blocks direct scanout; \
                     preferred_scale is the only client-opt-in path",
                    output.connector,
                    configured.as_f32(),
                    configured.as_f32().ceil() as i32,
                );
            }
            output.geometry.scale = configured;
        }
        if let Some(position) = policy.position {
            output.geometry.logical_origin = position;
        }
        if let Some(transform) = policy.transform
            && transform != tessera_primitives::Transform::Normal
        {
            log::warn!(
                "[server] output '{}': transform configured but not yet applied \
                     (renderer output-transform support pending)",
                output.connector
            );
        }
    }

    pub(crate) fn reconcile_outputs(&mut self) {
        let previous_rects: std::collections::HashMap<String, tessera_primitives::Rect> = self
            .state
            .output_infos
            .iter()
            .map(|output| (output.connector.clone(), output.geometry.logical_rect()))
            .collect();

        let mut physical = self.state.physical_outputs.clone();
        for output in &mut physical {
            Self::apply_output_policy_to_info(&self.state.output_policies, output);
        }
        // `[INV-ARCH-74]`: A `primary` policy moves its output to index 0 only
        // within the physical output set; a virtual output can never displace a
        // physical output as primary.
        if let Some(primary) = physical.iter().position(|output| {
            self.state
                .output_policies
                .get(&output.connector)
                .is_some_and(|policy| policy.primary)
        }) {
            let output = physical.remove(primary);
            physical.insert(0, output);
        }

        let mut outputs = physical;
        for record in &self.state.virtual_outputs {
            if outputs
                .iter()
                .any(|existing| existing.connector == record.info.connector)
            {
                continue;
            }
            let mut virt = record.info.clone();
            virt.kind = tessera_desktop::output::OutputKind::Virtual;
            let has_policy_position = self
                .state
                .output_policies
                .get(&virt.connector)
                .and_then(|policy| policy.position)
                .is_some();
            Self::apply_output_policy_to_info(&self.state.output_policies, &mut virt);
            if !has_policy_position {
                if let Some(explicit) = record.explicit_origin {
                    virt.geometry.logical_origin = explicit;
                } else {
                    let next_x = outputs
                        .iter()
                        .map(|existing| {
                            let rect = existing.geometry.logical_rect();
                            rect.origin.x.saturating_add(rect.size.w)
                        })
                        .max()
                        .unwrap_or(0);
                    virt.geometry.logical_origin = tessera_primitives::Point { x: next_x, y: 0 };
                }
            }
            outputs.push(virt);
        }
        if outputs.is_empty() {
            return;
        }

        let desired = outputs
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<std::collections::HashSet<_>>();
        for output in &outputs {
            if !self
                .state
                .workspaces
                .outputs()
                .iter()
                .any(|current| current.connector == output.connector)
            {
                self.state.workspaces.add_output(&output.connector);
            }
        }
        let removed_outputs: Vec<(tessera_desktop::workspace::OutputId, String)> = self
            .state
            .workspaces
            .outputs()
            .iter()
            .filter(|output| !desired.contains(output.connector.as_str()))
            .map(|output| (output.id, output.connector.clone()))
            .collect();
        let primary = outputs[0].clone();
        let primary_rect = primary.geometry.logical_rect();
        for (output_id, removed_connector) in removed_outputs {
            self.state.workspaces.remove_output(output_id);
            if let Some(&removed_rect) = previous_rects.get(&removed_connector) {
                self.relocate_windows_from_removed_output(
                    &removed_connector,
                    removed_rect,
                    &primary.connector,
                    primary_rect,
                );
            }
        }

        if let Some(output) = self
            .state
            .workspaces
            .outputs()
            .iter()
            .find(|output| output.connector == primary.connector)
        {
            self.state.output = output.id;
        }
        unsafe { reconcile_output_globals(self.state.as_mut(), &outputs) };
        self.state.output_infos = outputs;
        self.state.outputs_revision = self.state.outputs_revision.wrapping_add(1);
        self.set_output_geometry(primary.geometry);
    }

    /// Create a dynamic virtual extend output in the human interaction domain
    /// (`VIRTUAL-*`, ADR-0186). When `spec.persistent` is `false`, the output
    /// is bound to `conn_id` and reclaimed on disconnect (`[INV-ARCH-75]`).
    pub fn create_virtual_output(
        &mut self,
        conn_id: Option<u64>,
        spec: tessera_desktop::output::VirtualOutputSpec,
    ) -> Result<tessera_desktop::output::OutputInfo, String> {
        spec.validate().map_err(str::to_owned)?;
        let connector = match spec.connector {
            Some(name) => {
                if self
                    .state
                    .output_infos
                    .iter()
                    .any(|output| output.connector == name)
                    || self
                        .state
                        .virtual_outputs
                        .iter()
                        .any(|record| record.info.connector == name)
                {
                    return Err(format!("output '{name}' already exists"));
                }
                name
            }
            None => {
                let mut index = 1_u32;
                loop {
                    let candidate = format!(
                        "{}{index}",
                        tessera_desktop::output::VIRTUAL_CONNECTOR_PREFIX
                    );
                    let taken = self
                        .state
                        .output_infos
                        .iter()
                        .any(|output| output.connector == candidate)
                        || self
                            .state
                            .virtual_outputs
                            .iter()
                            .any(|record| record.info.connector == candidate);
                    if !taken {
                        break candidate;
                    }
                    index = index.saturating_add(1);
                }
            }
        };
        let owner_conn_id = (!spec.persistent).then_some(conn_id).flatten();
        let info = tessera_desktop::output::OutputInfo {
            connector: connector.clone(),
            geometry: tessera_desktop::output::OutputGeometry {
                mode: spec.mode,
                scale: spec.scale,
                transform: tessera_primitives::Transform::Normal,
                logical_origin: spec.logical_origin.unwrap_or_default(),
            },
            kind: tessera_desktop::output::OutputKind::Virtual,
            available_modes: vec![spec.mode],
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
        };
        self.state.virtual_outputs.push(VirtualOutputRecord {
            owner_conn_id,
            explicit_origin: spec.logical_origin,
            info,
        });
        self.reconcile_outputs();
        self.state
            .output_infos
            .iter()
            .find(|output| output.connector == connector)
            .cloned()
            .ok_or_else(|| format!("virtual output '{connector}' failed to register"))
    }

    /// Resize, rescale, or reposition a live virtual extend output
    /// (`VIRTUAL-*`, ADR-0186).
    pub fn resize_virtual_output(
        &mut self,
        conn_id: Option<u64>,
        connector: &str,
        mode: tessera_desktop::output::OutputMode,
        scale: Option<tessera_desktop::output::Scale>,
        logical_origin: Option<tessera_primitives::Point>,
    ) -> Result<tessera_desktop::output::OutputInfo, String> {
        let Some(record) = self
            .state
            .virtual_outputs
            .iter_mut()
            .find(|record| record.info.connector == connector)
        else {
            return Err(format!("unknown virtual output '{connector}'"));
        };
        if let (Some(owner), Some(caller)) = (record.owner_conn_id, conn_id)
            && owner != caller
        {
            return Err(format!(
                "virtual output '{connector}' is owned by another connection"
            ));
        }
        let next_scale = scale.unwrap_or(record.info.geometry.scale);
        tessera_desktop::output::validate_virtual_output_mode_and_scale(mode, next_scale)
            .map_err(str::to_owned)?;
        if let Some(origin) = logical_origin
            && (!(-32_768..=32_768).contains(&origin.x) || !(-32_768..=32_768).contains(&origin.y))
        {
            return Err("virtual output logical origin is out of range".to_owned());
        }
        record.info.geometry.mode = mode;
        record.info.geometry.scale = next_scale;
        record.info.available_modes = vec![mode];
        if let Some(origin) = logical_origin {
            record.explicit_origin = Some(origin);
            record.info.geometry.logical_origin = origin;
        }
        self.reconcile_outputs();
        self.state
            .output_infos
            .iter()
            .find(|output| output.connector == connector)
            .cloned()
            .ok_or_else(|| format!("virtual output '{connector}' disappeared after resize"))
    }

    /// Destroy a live virtual extend output (`VIRTUAL-*`, ADR-0186) and
    /// relocate its workspaces and windows to the primary physical output.
    pub fn destroy_virtual_output(
        &mut self,
        conn_id: Option<u64>,
        connector: &str,
    ) -> Result<(), String> {
        let Some(index) = self
            .state
            .virtual_outputs
            .iter()
            .position(|record| record.info.connector == connector)
        else {
            return Err(format!("unknown virtual output '{connector}'"));
        };
        if let (Some(owner), Some(caller)) =
            (self.state.virtual_outputs[index].owner_conn_id, conn_id)
            && owner != caller
        {
            return Err(format!(
                "virtual output '{connector}' is owned by another connection"
            ));
        }
        self.state.virtual_outputs.remove(index);
        self.reconcile_outputs();
        Ok(())
    }

    /// Reclaim every non-persistent virtual output owned by `conn_id` when
    /// its IPC connection closes (`[INV-ARCH-75]`). Returns the reclaimed
    /// connector names.
    pub fn disconnect_virtual_outputs(&mut self, conn_id: u64) -> Vec<String> {
        let mut reclaimed = Vec::new();
        self.state.virtual_outputs.retain(|record| {
            if record.owner_conn_id == Some(conn_id) {
                reclaimed.push(record.info.connector.clone());
                false
            } else {
                true
            }
        });
        if !reclaimed.is_empty() {
            self.reconcile_outputs();
        }
        reclaimed
    }

    /// Whether `connector` names a live human-domain virtual extend output
    /// (`OutputKind::Virtual`, ADR-0186).
    pub fn is_virtual_output(&self, connector: &str) -> bool {
        self.state
            .output_infos
            .iter()
            .any(|output| output.connector == connector && output.kind.is_virtual())
    }

    /// Whether the logical point `(x, y)` lies on a live virtual extend output
    /// (`OutputKind::Virtual`, ADR-0186). The physical KMS cursor plane has no
    /// CRTC over a virtual output, so the composition root hides its sprite
    /// there and lets the virtual stream's embedded cursor stand in
    /// (`[INV-ARCH-76]`).
    pub fn point_over_virtual_output(&self, x: f32, y: f32) -> bool {
        if !self
            .state
            .output_infos
            .iter()
            .any(|output| output.kind.is_virtual())
        {
            return false;
        }
        let point = tessera_primitives::Point {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        self.state.output_infos.iter().any(|output| {
            output.kind.is_virtual() && output.geometry.logical_rect().contains(point)
        })
    }

    /// Whether `conn_id` is the owner of the live virtual output `connector`.
    pub fn connection_owns_virtual_output(&self, conn_id: u64, connector: &str) -> bool {
        self.state.virtual_outputs.iter().any(|record| {
            record.info.connector == connector && record.owner_conn_id == Some(conn_id)
        })
    }

    /// Bounding rectangle in logical desktop coordinates across all active
    /// human outputs (physical and virtual).
    pub fn desktop_logical_bounds(&self) -> tessera_primitives::Rect {
        let mut iter = self
            .state
            .output_infos
            .iter()
            .map(|output| output.geometry.logical_rect())
            .filter(|rect| rect.size.w > 0 && rect.size.h > 0);
        let Some(first) = iter.next() else {
            return self.output_logical_rect();
        };
        iter.fold(first, |acc, rect| acc.union(rect))
    }

    pub(crate) fn translate_window_tree_between_outputs(
        &mut self,
        window_id: tessera_desktop::window::WindowId,
        descendants: &[tessera_desktop::window::WindowId],
        src_rect: tessera_primitives::Rect,
        dst_rect: tessera_primitives::Rect,
    ) {
        if src_rect == dst_rect || dst_rect.size.w <= 0 || dst_rect.size.h <= 0 {
            return;
        }
        let all_ids = std::iter::once(window_id).chain(descendants.iter().copied());
        for id in all_ids {
            let rec = self.find_surface_by_window_id(id);
            if rec.is_null() {
                continue;
            }
            unsafe {
                let pos = (*rec).position;
                let inside_dst = pos.x >= dst_rect.origin.x
                    && pos.x < dst_rect.origin.x.saturating_add(dst_rect.size.w)
                    && pos.y >= dst_rect.origin.y
                    && pos.y < dst_rect.origin.y.saturating_add(dst_rect.size.h);
                if inside_dst {
                    continue;
                }
                let rel_x = (pos.x - src_rect.origin.x).clamp(0, (dst_rect.size.w - 1).max(0));
                let rel_y = (pos.y - src_rect.origin.y).clamp(0, (dst_rect.size.h - 1).max(0));
                let new_origin = tessera_primitives::Point {
                    x: dst_rect.origin.x.saturating_add(rel_x),
                    y: dst_rect.origin.y.saturating_add(rel_y),
                };
                reposition_toplevel_with_popups(rec, new_origin);
                if let Some(saved) = (*rec).saved_floating_rect.as_mut() {
                    let s_rel_x =
                        (saved.origin.x - src_rect.origin.x).clamp(0, (dst_rect.size.w - 1).max(0));
                    let s_rel_y =
                        (saved.origin.y - src_rect.origin.y).clamp(0, (dst_rect.size.h - 1).max(0));
                    saved.origin = tessera_primitives::Point {
                        x: dst_rect.origin.x.saturating_add(s_rel_x),
                        y: dst_rect.origin.y.saturating_add(s_rel_y),
                    };
                }
            }
        }
    }

    fn relocate_windows_from_removed_output(
        &mut self,
        removed_connector: &str,
        removed_rect: tessera_primitives::Rect,
        survivor_connector: &str,
        survivor_rect: tessera_primitives::Rect,
    ) {
        if removed_rect.size.w <= 0 || removed_rect.size.h <= 0 {
            return;
        }
        let candidate_ids: Vec<tessera_desktop::window::WindowId> = self
            .state
            .live_surfaces()
            .filter_map(|pointer| unsafe {
                let s = &*pointer;
                (!s.xdg_toplevel.is_null() && s.mapped).then_some(s.window.id)
            })
            .collect();
        for window_id in candidate_ids {
            if !self.human_controls_window(window_id) {
                continue;
            }
            let rec = self.find_surface_by_window_id(window_id);
            if rec.is_null() {
                continue;
            }
            let pos = unsafe { (*rec).position };
            let in_removed = pos.x >= removed_rect.origin.x
                && pos.x < removed_rect.origin.x.saturating_add(removed_rect.size.w)
                && pos.y >= removed_rect.origin.y
                && pos.y < removed_rect.origin.y.saturating_add(removed_rect.size.h);
            if in_removed {
                self.translate_window_tree_between_outputs(
                    window_id,
                    &[],
                    removed_rect,
                    survivor_rect,
                );
                unsafe {
                    post_window_human_output_transition(
                        &self.state,
                        window_id,
                        Some(removed_connector),
                        Some(survivor_connector),
                    );
                }
            }
        }
    }

    pub(crate) fn rehome_window_to_containing_output(
        &mut self,
        window_id: tessera_desktop::window::WindowId,
    ) {
        if !self.human_controls_window(window_id) {
            return;
        }
        let rec = self.find_surface_by_window_id(window_id);
        if rec.is_null() {
            return;
        }
        let (pos, size) = unsafe { ((*rec).position, (*rec).window.size) };
        let center = tessera_primitives::Point {
            x: pos.x.saturating_add(size.w.max(0) / 2),
            y: pos.y.saturating_add(size.h.max(0) / 2),
        };
        let Some(target_connector) = self
            .state
            .output_infos
            .iter()
            .find(|output| {
                let rect = output.geometry.logical_rect();
                center.x >= rect.origin.x
                    && center.x < rect.origin.x.saturating_add(rect.size.w)
                    && center.y >= rect.origin.y
                    && center.y < rect.origin.y.saturating_add(rect.size.h)
            })
            .map(|output| output.connector.clone())
        else {
            return;
        };
        let current_connector =
            human_output_connector_for_window(&self.state, window_id).map(str::to_owned);
        if current_connector.as_deref() == Some(target_connector.as_str()) {
            return;
        }
        let Some(target_output_id) = self
            .state
            .workspaces
            .outputs()
            .iter()
            .find(|output| output.connector == target_connector)
            .map(|output| output.id)
        else {
            return;
        };
        let Some(target_ws) = self.state.workspaces.current_workspace(target_output_id) else {
            return;
        };
        self.state.workspaces.move_toplevel(window_id, target_ws);
        let descendants = self.transient_descendant_window_ids(window_id);
        for child_id in &descendants {
            self.state.workspaces.move_toplevel(*child_id, target_ws);
        }
        unsafe {
            post_window_human_output_transition(
                &self.state,
                window_id,
                current_connector.as_deref(),
                Some(target_connector.as_str()),
            );
            for child_id in &descendants {
                post_window_human_output_transition(
                    &self.state,
                    *child_id,
                    current_connector.as_deref(),
                    Some(target_connector.as_str()),
                );
            }
        }
    }

    /// Install the session color pipeline reported by the backend (ADR-0001
    /// boundary: the pixel encoding is a flux capability; this server only
    /// advertises it). On change, broadcast
    /// `wp_color_management_output_v1.image_description_changed` and
    /// `wp_color_management_surface_feedback_v1.preferred_changed`.
    pub fn set_color_pipeline(&mut self, pipeline: tessera_desktop::output::ColorPipeline) {
        if self.state.color_pipeline == pipeline {
            return;
        }
        self.state.color_pipeline = pipeline;
        // The pipeline change retires the old output description record:
        // mint a fresh identity so `preferred_changed` names the new one.
        self.state.color_pipeline_identity = self.state.alloc_color_identity();
        unsafe { crate::extensions::resend_color_pipeline(self.state.as_mut()) };
    }

    /// Set per-connector output policies from the config's `[[output]]`
    /// entries (ADR-0028), and re-apply them to the current output set.
    /// Unmatched connectors are ignored with a log line, so a monitor that is
    /// not plugged in yet still applies once it appears.
    pub fn set_output_policies(
        &mut self,
        policies: std::collections::HashMap<String, tessera_desktop::output::OutputPolicy>,
    ) {
        for connector in policies.keys() {
            if !self
                .state
                .output_infos
                .iter()
                .any(|o| &o.connector == connector)
            {
                log::info!("[server] output policy for '{connector}' matches no current output");
            }
        }
        self.state.output_policies = policies;
        if !self.state.physical_outputs.is_empty() || !self.state.virtual_outputs.is_empty() {
            self.reconcile_outputs();
        }
    }

    /// Replace the focused output's geometry (ADR-0028). The backend calls
    /// this on resize; the tiling work-area is the geometry's logical rect.
    /// Re-sends the wl_output geometry/mode/scale/done sequence to every bound
    /// client so they update their scale and surface buffer scale.
    pub fn set_output_geometry(&mut self, geo: tessera_desktop::output::OutputGeometry) {
        self.state.output_geometry = geo;
        if let Some(primary) = self.state.output_infos.first_mut() {
            primary.geometry = geo;
        }
        if let Some(primary) = self.state.physical_outputs.first_mut() {
            primary.geometry = geo;
        }
        self.state.outputs_revision = self.state.outputs_revision.wrapping_add(1);
        let infos = self.state.output_infos.clone();
        unsafe { reconcile_output_globals(self.state.as_mut(), &infos) };
        // Resend to every bound wl_output resource.
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe { send_output_geometry(res) };
        }
        // Refresh xdg-output logical extents too.
        self.resend_xdg_outputs();
        // Re-send fractional-scale hints so HiDPI-aware clients resize buffers.
        unsafe {
            extensions::resend_fractional_scales(self.state.as_ref() as *const State as *mut State)
        };
        unsafe { extensions::session_lock_outputs_changed(self.state.as_mut()) };
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// The focused output's logical rect (ADR-0028). The chrome-aware
    /// tiling work-area is this inset by the chrome's reserved edges.
    pub fn output_logical_rect(&self) -> tessera_primitives::Rect {
        self.state.output_geometry.logical_rect()
    }

    /// Resend the `zxdg_output_v1` logical geometry events to every bound
    /// xdg-output resource. Called whenever the output's logical extents
    /// change (resize / scale / transform) so clients reposition. Pairs with
    /// the wl_output geometry re-send in [`Server::set_output_geometry`].
    pub(crate) fn resend_xdg_outputs(&self) {
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .xdg_output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe {
                let output = self
                    .state
                    .xdg_output_links
                    .get(&(res as usize))
                    .copied()
                    .unwrap_or(std::ptr::null_mut());
                extensions::send_xdg_output_geometry(
                    res,
                    output,
                    self.state.as_ref() as *const State as *mut State,
                );
                extensions::finish_xdg_output_batch(res, output);
            }
        }
    }

    /// The live backend-reported outputs for IPC and chrome.
    pub fn output_infos(&self) -> Vec<tessera_desktop::output::OutputInfo> {
        self.state.output_infos.clone()
    }

    /// Apply the master-stack tiling policy to the current workspace's
    /// windows when tiled mode is on (ADR-0024). Runs the layout over
    /// `work_area` (the chrome-aware logical rect) and reconfigures only the
    /// windows whose target rect moved, so steady state sends no configure
    /// events. No-op when tiling is off.
    pub fn apply_tiling(&mut self, work_area: tessera_primitives::Rect) {
        self.state.last_work_area = work_area;

        let mut flushed = false;
        for id in self.state.workspaces.visible_toplevels() {
            let rec = self.find_surface_by_window_id(id);
            if rec.is_null() {
                continue;
            }
            let win_screen_rect = self.state.output_rect_for_window(id);
            let win_work_area = self.state.work_area_for_window(id);
            unsafe {
                if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                    continue;
                }
                if (*rec).window.state.fullscreen {
                    if (*rec).saved_floating_rect.is_none() {
                        (*rec).saved_floating_rect = Some(tessera_primitives::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                    }
                    if (*rec).layout_target != Some(win_screen_rect) {
                        let old = tessera_primitives::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        };
                        reposition_toplevel_with_popups(rec, win_screen_rect.origin);
                        (*rec).window.size = win_screen_rect.size;
                        (*rec).layout_target = Some(win_screen_rect);
                        self.note_transition(rec, old);
                        reconfigure_with_size(rec, win_screen_rect.size.w, win_screen_rect.size.h);
                        if !flushed {
                            ffi::wl_display_flush_clients(self.state.display);
                            flushed = true;
                        }
                    }
                } else if (*rec).window.state.maximized {
                    if (*rec).saved_floating_rect.is_none() {
                        (*rec).saved_floating_rect = Some(tessera_primitives::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                    }
                    if (*rec).layout_target != Some(win_work_area) {
                        let old = tessera_primitives::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        };
                        reposition_toplevel_with_popups(rec, win_work_area.origin);
                        (*rec).window.size = win_work_area.size;
                        (*rec).layout_target = Some(win_work_area);
                        self.note_transition(rec, old);
                        reconfigure_with_size(rec, win_work_area.size.w, win_work_area.size.h);
                        if !flushed {
                            ffi::wl_display_flush_clients(self.state.display);
                            flushed = true;
                        }
                    }
                }
            }
        }
    }

    /// If the keyboard-focused surface is not on a visible workspace, clear
    /// focus (post leave, deactivate). Idempotent.
    pub(crate) fn drop_focus_if_hidden(&mut self) {
        let visible = self.visible();
        let Some(wid) = self.focused_toplevel_id() else {
            return;
        };
        if !visible.contains(&wid) {
            self.change_keyboard_focus(std::ptr::null_mut());
        }
    }
}

/// Retarget a partially moved workspace strip without snapping it back to a
/// new two-page animation. Every retained page receives the same translation,
/// preserving the output-width spacing that makes page clips meet exactly.
fn retarget_workspace_strip(
    mut positions: Vec<(tessera_desktop::workspace::WorkspaceId, f32)>,
    outgoing: tessera_desktop::workspace::WorkspaceId,
    incoming: tessera_desktop::workspace::WorkspaceId,
    direction: i32,
    width: f32,
) -> Vec<WorkspaceSlideLayer> {
    if !positions
        .iter()
        .any(|(workspace, _)| *workspace == outgoing)
    {
        positions.push((outgoing, 0.0));
    }
    let outgoing_offset = positions
        .iter()
        .find(|(workspace, _)| *workspace == outgoing)
        .map(|(_, offset)| *offset)
        .unwrap_or(0.0);
    if !positions
        .iter()
        .any(|(workspace, _)| *workspace == incoming)
    {
        positions.push((
            incoming,
            outgoing_offset + direction.signum() as f32 * width,
        ));
    }
    let incoming_offset = positions
        .iter()
        .find(|(workspace, _)| *workspace == incoming)
        .map(|(_, offset)| *offset)
        .unwrap_or(0.0);
    let shift = -incoming_offset;
    positions
        .into_iter()
        .map(|(workspace, from_x)| WorkspaceSlideLayer {
            workspace,
            from_x,
            to_x: from_x + shift,
        })
        .collect()
}

fn stepped_index(current: usize, len: usize, forward: bool) -> usize {
    debug_assert!(len > 0);
    if forward {
        (current + 1) % len
    } else {
        (current + len - 1) % len
    }
}

fn reconcile_switcher_session(
    session: &mut WindowSwitcherSession,
    eligible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
) {
    let selected_id = session.order.get(session.selected).copied();
    let old_index = session.selected.min(session.order.len().saturating_sub(1));
    session.order.retain(|id| eligible.contains(id));
    if session.order.is_empty() {
        session.selected = 0;
        return;
    }
    if let Some(index) =
        selected_id.and_then(|id| session.order.iter().position(|candidate| *candidate == id))
    {
        session.selected = index;
        return;
    }

    session.selected = if session.last_forward {
        old_index % session.order.len()
    } else {
        (old_index + session.order.len() - 1) % session.order.len()
    };
}

#[cfg(test)]
mod window_switcher_tests {
    use super::*;

    #[test]
    fn workspace_slide_moves_old_and_new_desktops_in_the_same_direction() {
        use tessera_desktop::workspace::WorkspaceId;

        let slide = WorkspaceSlide {
            output: tessera_primitives::Rect::new(0, 0, 1000, 800),
            layers: vec![
                WorkspaceSlideLayer {
                    workspace: WorkspaceId(1),
                    from_x: 0.0,
                    to_x: -1000.0,
                },
                WorkspaceSlideLayer {
                    workspace: WorkspaceId(2),
                    from_x: 1000.0,
                    to_x: 0.0,
                },
            ],
            started_ms: 10,
            duration_ms: 100,
        };
        assert_eq!(slide.offset_at(WorkspaceId(1), 10), Some(0.0));
        assert_eq!(slide.offset_at(WorkspaceId(2), 10), Some(1000.0));
        let outgoing_mid = slide.offset_at(WorkspaceId(1), 60).unwrap();
        let incoming_mid = slide.offset_at(WorkspaceId(2), 60).unwrap();
        assert!(outgoing_mid < 0.0);
        assert!(incoming_mid > 0.0);
        assert!((incoming_mid - outgoing_mid - 1000.0).abs() < f32::EPSILON);
        assert_eq!(slide.offset_at(WorkspaceId(1), 110), None);
        assert_eq!(slide.offset_at(WorkspaceId(2), 110), None);
    }

    #[test]
    fn workspace_strip_retargets_without_breaking_page_spacing() {
        use tessera_desktop::workspace::WorkspaceId;

        let layers = retarget_workspace_strip(
            vec![(WorkspaceId(1), -500.0), (WorkspaceId(2), 500.0)],
            WorkspaceId(2),
            WorkspaceId(3),
            1,
            1000.0,
        );
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].to_x, -2000.0);
        assert_eq!(layers[1].to_x, -1000.0);
        assert_eq!(layers[2].to_x, 0.0);
    }

    #[test]
    fn reversing_workspace_slide_keeps_continuous_positions() {
        use tessera_desktop::workspace::WorkspaceId;

        let layers = retarget_workspace_strip(
            vec![(WorkspaceId(1), -400.0), (WorkspaceId(2), 600.0)],
            WorkspaceId(2),
            WorkspaceId(1),
            -1,
            1000.0,
        );
        assert_eq!(layers[0].from_x, -400.0);
        assert_eq!(layers[1].from_x, 600.0);
        assert_eq!(layers[0].to_x, 0.0);
        assert_eq!(layers[1].to_x, 1000.0);
    }

    #[test]
    fn frozen_order_cycles_both_directions() {
        assert_eq!(stepped_index(0, 4, true), 1);
        assert_eq!(stepped_index(3, 4, true), 0);
        assert_eq!(stepped_index(0, 4, false), 3);
        assert_eq!(stepped_index(2, 4, false), 1);
    }

    #[test]
    fn cross_client_prompters_are_not_independent_switcher_candidates() {
        let mut state = State::new(std::ptr::null_mut());
        let mut app = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
        app.state = &mut state;
        app.xdg_toplevel = 0x101usize as *mut ffi::wl_resource;
        app.window.id = tessera_desktop::window::WindowId(1);
        app.mapped = true;

        // The portal prompter riding the app's tree over zxdg_importer_v2.
        let mut prompter = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
        prompter.state = &mut state;
        prompter.xdg_toplevel = 0x201usize as *mut ffi::wl_resource;
        prompter.window.id = tessera_desktop::window::WindowId(2);
        prompter.window.parent = Some(app.as_mut() as *mut SurfaceRec as usize);
        prompter.foreign_parent_owner = 0x900usize as *mut ffi::wl_resource;
        prompter.mapped = true;

        // An in-app dialog parented the ordinary way: a real candidate.
        let mut dialog = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
        dialog.state = &mut state;
        dialog.xdg_toplevel = 0x301usize as *mut ffi::wl_resource;
        dialog.window.id = tessera_desktop::window::WindowId(3);
        dialog.window.parent = Some(app.as_mut() as *mut SurfaceRec as usize);
        dialog.mapped = true;

        state.surfaces = vec![app.as_mut(), prompter.as_mut(), dialog.as_mut()];
        let visible: std::collections::HashSet<_> =
            (1..=3).map(tessera_desktop::window::WindowId).collect();

        assert!(switcher_candidate(&app, &visible, true));
        assert!(
            !switcher_candidate(&prompter, &visible, true),
            "the prompter rides the app's card; the landing rule reaches it"
        );
        assert!(
            switcher_candidate(&dialog, &visible, true),
            "in-app dialogs remain switchable windows"
        );
        assert!(
            !switcher_candidate(&app, &visible, false),
            "a window the seat does not control is never a candidate"
        );
    }

    #[test]
    fn rebuilding_mru_after_one_step_toggles_back() {
        use tessera_desktop::window::WindowId;

        // Bottom-to-top stacking order; C starts focused.
        let mut stack = vec![WindowId(1), WindowId(2), WindowId(3)];
        let first_mru: Vec<_> = stack.iter().rev().copied().collect();
        let target = first_mru[stepped_index(0, first_mru.len(), true)];
        assert_eq!(target, WindowId(2));

        stack.retain(|id| *id != target);
        stack.push(target);
        let second_mru: Vec<_> = stack.iter().rev().copied().collect();
        let target = second_mru[stepped_index(0, second_mru.len(), true)];
        assert_eq!(target, WindowId(3));
    }

    #[test]
    fn closing_the_selection_chooses_the_neighbour_in_the_last_direction() {
        use std::collections::HashSet;
        use tessera_desktop::window::WindowId;

        let eligible = HashSet::from([WindowId(1), WindowId(3), WindowId(4)]);
        let mut forward = WindowSwitcherSession {
            order: vec![WindowId(1), WindowId(2), WindowId(3), WindowId(4)],
            selected: 1,
            last_forward: true,
        };
        reconcile_switcher_session(&mut forward, &eligible);
        assert_eq!(forward.order[forward.selected], WindowId(3));

        let mut backward = WindowSwitcherSession {
            order: vec![WindowId(1), WindowId(2), WindowId(3), WindowId(4)],
            selected: 1,
            last_forward: false,
        };
        reconcile_switcher_session(&mut backward, &eligible);
        assert_eq!(backward.order[backward.selected], WindowId(1));
    }

    #[test]
    fn refreshing_a_session_never_inserts_new_windows() {
        use std::collections::HashSet;
        use tessera_desktop::window::WindowId;

        let mut session = WindowSwitcherSession {
            order: vec![WindowId(1), WindowId(2)],
            selected: 0,
            last_forward: true,
        };
        let eligible = HashSet::from([WindowId(1), WindowId(2), WindowId(3)]);
        reconcile_switcher_session(&mut session, &eligible);
        assert_eq!(session.order, vec![WindowId(1), WindowId(2)]);
    }
}
