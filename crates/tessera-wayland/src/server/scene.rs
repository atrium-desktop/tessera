use crate::*;

/// What a frame actually put on the display, for presentation bookkeeping.
///
/// `wp_presentation_feedback.presented` and `[INV-ARCH-84]` require that a
/// surface be reported presented *only* from a frame that sampled it. Most
/// frames do not sample every surface: a cursor-plane-only commit redraws
/// nothing, and a no-damage cycle renders no frame at all. Making that
/// distinction a type rather than a `bool` keeps every call site honest —
/// "no frame reached the display" cannot be silently spelled the same way as
/// "this frame drew everything".
#[derive(Clone, Copy)]
pub enum PresentedSurfaces<'a> {
    /// The frame sampled exactly these surface ids. `drew_preview_sources`
    /// means it also drew surfaces outside them (overview thumbnails, the
    /// window switcher's live cards, the workspace rail's miniatures), so the
    /// caller cannot enumerate the set and the conservative fallback applies.
    Sampled {
        ids: &'a [usize],
        drew_preview_sources: bool,
    },
    /// A frame reached the display and rendered the whole presentation-visible
    /// scene for this target, but the caller cannot enumerate which surface
    /// resource ids it drew (per-output offscreen stream capture).
    All,
    /// No frame reached the display in this cycle. Queued presentation
    /// feedback must stay queued — it resolves on the next frame that does
    /// sample the surface, or as `discarded` when the surface goes away.
    Nothing,
}

impl PresentedSurfaces<'_> {
    /// The explicitly sampled id set, or `None` when the fallback applies.
    fn enumerated(&self) -> Option<(&[usize], bool)> {
        match self {
            Self::Sampled {
                ids,
                drew_preview_sources,
            } => Some((ids, *drew_preview_sources)),
            Self::All => Some((&[], true)),
            Self::Nothing => None,
        }
    }
}

/// Hidden surfaces retain a low-rate callback heartbeat so clients which use
/// frame callbacks for housekeeping do not stall forever.  The value mirrors
/// the established compositor practice of roughly one callback per second,
/// while leaving margin for a 1 s maintenance timer to arrive slightly early.
const BACKGROUND_FRAME_CALLBACK_INTERVAL_MS: u32 = 995;

fn background_frame_callback_due(now_ms: u32, previous_ms: u32) -> bool {
    now_ms.wrapping_sub(previous_ms) >= BACKGROUND_FRAME_CALLBACK_INTERVAL_MS
}

/// The shared per-iteration desktop scene summary: the presentation-visible
/// window set, the occlusion verdicts, and the SHM/dma-buf frame lists
/// derived from them.
///
/// Produced by [`Server::desktop_frame_sets`] so the damage assessment, the
/// scanout planner, and the render pass reuse one visibility/occlusion
/// computation instead of each recomputing it (an O(windows × surfaces) walk
/// with per-window allocations) inside the same iteration.
pub struct DesktopFrameSets<'a> {
    /// Presentation-visible toplevel ids (see [`Server::render_visible`]).
    pub visible: std::collections::HashSet<tessera_desktop::window::WindowId>,
    /// Toplevel ids fully covered by opaque windows above them.
    pub occluded: std::collections::HashSet<tessera_desktop::window::WindowId>,
    /// Mapped physical-desktop SHM surfaces (unordered backing store).
    pub shm: Vec<SurfacePixels<'a>>,
    /// Mapped physical-desktop dma-buf surfaces (unordered backing store).
    pub dmabuf: Vec<SurfaceDmabuf>,
}

impl Server {
    /// The authoritative interaction-visible set: current-workspace
    /// toplevels on every output. Presentation may temporarily extend this
    /// through [`Self::render_visible`] during a workspace slide; input,
    /// focus, and chrome never do (ADR-0025).
    pub(crate) fn visible(&self) -> std::collections::HashSet<tessera_desktop::window::WindowId> {
        self.state
            .workspaces
            .visible_toplevels()
            .into_iter()
            .collect()
    }

    fn root_render_delta(&self, root: &SurfaceRec) -> (i32, i32) {
        self.transition_render_rect(root)
            .map(|rect| {
                (
                    rect.origin.x - root.position.x,
                    rect.origin.y - root.position.y,
                )
            })
            .unwrap_or((0, 0))
    }

    /// Toplevel ids whose complete surface tree is covered by opaque pixels in
    /// windows stacked above it. Opacity comes from committed Wayland opaque
    /// regions, alpha-free XRGB/XBGR DMA-BUF formats, or the existing
    /// fullscreen guarantee. Popups and subsurfaces participate on both sides
    /// so culling a root can never make one of its children disappear.
    pub(crate) fn occluded_window_ids(
        &self,
    ) -> std::collections::HashSet<tessera_desktop::window::WindowId> {
        let visible = self.visible();
        self.occluded_window_ids_with(&visible)
    }

    /// [`occluded_window_ids`](Self::occluded_window_ids) for callers that
    /// already hold the presentation-visible window set, so the walk is not
    /// recomputed per consumer.
    pub(crate) fn occluded_window_ids_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> std::collections::HashSet<tessera_desktop::window::WindowId> {
        // Occlusion is a physical-desktop optimization. While the session lock
        // or overview is active the renderer draws a different scene, and
        // excluding windows here would starve lock-screen compositing.
        if self.state.session_lock_phase.is_active() || self.workspace_slide_pending() {
            return std::collections::HashSet::new();
        }
        let live = self.state.live_surfaces().collect::<Vec<_>>();
        // Root order follows the scene's bottom-to-top surface order; reverse
        // it below so coverage is known before each lower window is tested.
        let stacked: Vec<*mut SurfaceRec> = live
            .iter()
            .copied()
            .filter(|pointer| {
                let surface = unsafe { &**pointer };
                surface.mapped
                    && !surface.xdg_toplevel.is_null()
                    && visible.contains(&surface.window.id)
            })
            .collect();
        if stacked.len() < 2 {
            return std::collections::HashSet::new();
        }
        let mut occluded = std::collections::HashSet::new();
        let mut opaque_coverage: Vec<tessera_primitives::Rect> = Vec::new();
        for pointer in stacked.iter().rev() {
            let root = unsafe { &**pointer };
            if self.transition_render_rect(root).is_some() {
                continue;
            }
            let output = self.state.output_rect_for_window(root.window.id);

            let tree = live
                .iter()
                .copied()
                .filter(|surface| unsafe {
                    (**surface).mapped && surface_root_toplevel(*surface) == *pointer
                })
                .collect::<Vec<_>>();
            let painted = tree
                .iter()
                .filter_map(|surface| {
                    let surface = unsafe { &**surface };
                    intersect_rect(
                        tessera_primitives::Rect {
                            origin: surface_draw_origin(surface),
                            size: surface_logical_size(surface),
                        },
                        output,
                    )
                })
                .collect::<Vec<_>>();
            if !painted.is_empty()
                && painted
                    .iter()
                    .all(|rect| rect.fully_covered_by(&opaque_coverage))
            {
                occluded.insert(root.window.id);
                continue;
            }

            if root.window.state.fullscreen {
                opaque_coverage.push(output);
                continue;
            }
            for surface in tree {
                let surface = unsafe { &*surface };
                let surface_rect = tessera_primitives::Rect {
                    origin: surface_draw_origin(surface),
                    size: surface_logical_size(surface),
                };
                let alpha_free_dmabuf = surface.content_is_dmabuf
                    && surface.dmabuf.as_ref().is_some_and(|buffer| {
                        tessera_primitives::dmabuf::is_format_opaque(buffer.drm_format)
                    });
                if alpha_free_dmabuf && let Some(rect) = intersect_rect(surface_rect, output) {
                    opaque_coverage.push(rect);
                }
                let origin = surface_draw_origin(surface);
                for local in surface.opaque_region.iter().flatten() {
                    let translated = tessera_primitives::Rect::new(
                        origin.x.saturating_add(local.origin.x),
                        origin.y.saturating_add(local.origin.y),
                        local.size.w,
                        local.size.h,
                    );
                    if let Some(rect) = intersect_rect(translated, surface_rect)
                        .and_then(|rect| intersect_rect(rect, output))
                    {
                        opaque_coverage.push(rect);
                    }
                }
            }
        }
        occluded
    }

    /// Compatibility alias for
    /// [`client_surface_frame_order`](Self::client_surface_frame_order).
    ///
    /// Callers that only provide xdg-role frame payloads safely ignore the
    /// subsurface ids in the returned order.
    pub fn toplevel_frame_order(&self) -> Vec<usize> {
        self.client_surface_frame_order()
    }

    /// Compatibility alias for
    /// [`interaction_domain_client_surface_frame_order`](Self::interaction_domain_client_surface_frame_order).
    pub fn interaction_domain_toplevel_frame_order(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<usize> {
        self.interaction_domain_client_surface_frame_order(interaction_domain)
    }

    /// Every mapped client surface id in physical-desktop paint order.
    ///
    /// Each toplevel is emitted as an indivisible stacking unit: its
    /// below-parent subsurface trees, the toplevel, its above-parent trees,
    /// then its popup surface trees. Toplevel units remain in desktop z-order.
    /// Consumers must use this order to interleave both shm and dma-buf
    /// frames; global backing-type or subsurface passes break window
    /// occlusion.
    pub fn client_surface_frame_order(&self) -> Vec<usize> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        self.client_surface_frame_order_for_interaction_domain(
            HUMAN_INTERACTION_DOMAIN,
            Some(&visible),
            Some(&occluded),
        )
    }

    /// [`client_surface_frame_order`](Self::client_surface_frame_order) for
    /// callers that already computed the visibility/occlusion snapshot
    /// (see [`desktop_frame_sets`](Self::desktop_frame_sets)).
    pub fn client_surface_frame_order_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<usize> {
        self.client_surface_frame_order_for_interaction_domain(
            HUMAN_INTERACTION_DOMAIN,
            Some(visible),
            Some(occluded),
        )
    }

    /// Every presentation-visible client surface id in paint order, including
    /// windows fully covered on the physical desktop. Offscreen consumers such
    /// as live window previews must not inherit physical-output occlusion: a
    /// covered window still needs pixels when remapped into its own card.
    pub fn client_preview_surface_frame_order(&self) -> Vec<usize> {
        let visible = self.render_visible();
        self.client_surface_frame_order_for_interaction_domain(
            HUMAN_INTERACTION_DOMAIN,
            Some(&visible),
            None,
        )
    }

    /// Every mapped client surface id in paint order for a directed Interaction Domain.
    pub fn interaction_domain_client_surface_frame_order(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<usize> {
        if self.state.session_lock_phase.is_active()
            || self.interaction_domain_output(interaction_domain).is_none()
        {
            return Vec::new();
        }
        self.client_surface_frame_order_for_interaction_domain(interaction_domain, None, None)
    }

    fn client_surface_frame_order_for_interaction_domain(
        &self,
        interaction_domain: InteractionDomainId,
        visible: Option<&std::collections::HashSet<tessera_desktop::window::WindowId>>,
        occluded: Option<&std::collections::HashSet<tessera_desktop::window::WindowId>>,
    ) -> Vec<usize> {
        let roots = self
            .state
            .live_surfaces()
            .filter(|pointer| unsafe {
                let surface = &**pointer;
                surface.mapped
                    && !surface.xdg_toplevel.is_null()
                    && visible.is_none_or(|visible| visible.contains(&surface.window.id))
                    && occluded.is_none_or(|occluded| !occluded.contains(&surface.window.id))
                    && self
                        .state
                        .authority
                        .interaction_domain_observes_window(interaction_domain, surface.window.id)
            })
            .collect::<Vec<_>>();
        let popups = self
            .state
            .live_surfaces()
            .filter(|pointer| unsafe {
                let surface = &**pointer;
                surface.mapped && !surface.xdg_popup.is_null()
            })
            .collect::<Vec<_>>();

        let mut order = Vec::new();
        for root in roots {
            unsafe {
                append_surface_tree_frame_order(root, &mut order, 0);
            }
            // xdg-popups are separate xdg surface trees rather than
            // wl_subsurfaces. Keep every popup with its owning toplevel so a
            // popup from a lower window cannot escape above a higher window.
            for popup in &popups {
                if unsafe { surface_root_toplevel(*popup) == root } {
                    unsafe {
                        append_surface_tree_frame_order(*popup, &mut order, 0);
                    }
                }
            }
        }
        order
    }

    /// Mapped xdg-toplevel and xdg-popup surfaces backed by shm (CPU pixels).
    pub fn toplevel_frames(&self) -> Vec<SurfacePixels<'_>> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        self.toplevel_frames_with(&visible, &occluded)
    }

    /// The genie warp for a root toplevel mid minimize/restore flight, if its
    /// transition carries the genie minimize effect (ADR-0029). Restores play
    /// the deformation in reverse; the direction comes from the minimized
    /// flag, which the restore path clears before recording its transition.
    fn minimize_warp(&self, s: &SurfaceRec) -> Option<tessera_primitives::MinimizeWarp> {
        if s.xdg_toplevel.is_null() {
            return None;
        }
        let transition = s.window.transition?;
        let (style, target) = match transition.effect? {
            tessera_desktop::transition::TransitionEffect::Minimize { style, target } => {
                (style, target)
            }
            // Open/close fades carry no genie deformation.
            tessera_desktop::transition::TransitionEffect::Open
            | tessera_desktop::transition::TransitionEffect::Close => return None,
        };
        if style != tessera_desktop::dock::MinimizeAnimationStyle::Genie {
            return None;
        }
        let t = transition.progress_at(self.state.now_ms())?;
        let progress = if s.window.minimized { t } else { 1.0 - t };
        Some(tessera_primitives::MinimizeWarp { progress, target })
    }

    fn toplevel_frames_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfacePixels<'_>> {
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                let root =
                    unsafe { surface_root_toplevel(*s as *const SurfaceRec as *mut SurfaceRec) };
                s.mapped
                    && (!s.xdg_toplevel.is_null() || !s.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe {
                        !(*root).window.minimized || self.transition_render_rect(&*root).is_some()
                    }
                    && !s.content_is_dmabuf
                    && !s.pixels.is_empty()
                    && visible.contains(unsafe { &(*root).window.id })
                    && !occluded.contains(unsafe { &(*root).window.id })
                    && self
                        .state
                        .authority
                        .interaction_domain_observes_window(HUMAN_INTERACTION_DOMAIN, unsafe {
                            (*root).window.id
                        })
            })
            .map(|s| {
                // ADR-0029: while a transition is in flight the frame renders
                // at the interpolated rect; the model stays at the target.
                // The origin delta carries the whole subsurface tree with it.
                let render_rect = self.transition_render_rect(s);
                let minimize_warp = self.minimize_warp(s);
                let root =
                    unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut SurfaceRec) };
                let delta = if root.is_null() {
                    (0, 0)
                } else {
                    self.root_render_delta(unsafe { &*root })
                };
                let mut origin = surface_draw_origin(s);
                origin.x += delta.0;
                origin.y += delta.1;
                // ADR-0029 open/close fade: the same transition that drives
                // the interpolated size also drives an opacity multiplier.
                // Subsurfaces inherit it from their root through the shared
                // root delta path; popups below reuse their root's fade.
                let is_attached_to_drag = if root.is_null() {
                    false
                } else if let Some(drag) = self.state.drag
                    && !drag.attached_toplevel.is_null()
                {
                    unsafe { (*root).xdg_toplevel == drag.attached_toplevel }
                } else {
                    false
                };
                let transition_opacity = if is_attached_to_drag {
                    Some(0.92)
                } else if root.is_null() {
                    None
                } else {
                    unsafe { &*root }
                        .window
                        .transition
                        .and_then(|t| t.opacity_at(self.state.now_ms()))
                };
                SurfacePixels {
                    id: s.resource as usize,
                    window: if s.xdg_toplevel.is_null() {
                        let root =
                            unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut _) };
                        if root.is_null() {
                            None
                        } else {
                            Some(unsafe { &*root }.window.id)
                        }
                    } else {
                        Some(s.window.id)
                    },
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    pixels: &s.pixels,
                    geometry: tessera_primitives::SurfaceGeometry {
                        position: origin,
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        transition_size: render_rect.map(|r| r.size),
                        transition_opacity,
                        minimize_warp,
                        ..Default::default()
                    },
                    damage: &s.committed_damage,
                    opaque_region: s.opaque_region.as_deref(),
                    color: s.image_description.clone(),
                }
            })
            .collect()
    }

    /// Mapped xdg-toplevel surfaces backed by a dma-buf, for the renderer to
    /// import zero-copy. The `fd` is borrowed; the renderer duplicates it
    /// before Flux consumes the duplicate. The server keeps ownership until
    /// the backing buffer is replaced or destroyed.
    pub fn toplevel_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        self.toplevel_dmabuf_frames_with(&visible, &occluded)
    }

    /// [`toplevel_dmabuf_frames`](Self::toplevel_dmabuf_frames) for callers
    /// that already computed the visibility/occlusion snapshot (see
    /// [`desktop_frame_sets`](Self::desktop_frame_sets)).
    pub fn toplevel_dmabuf_frames_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfaceDmabuf> {
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                let root =
                    unsafe { surface_root_toplevel(*s as *const SurfaceRec as *mut SurfaceRec) };
                s.mapped
                    && (!s.xdg_toplevel.is_null() || !s.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe {
                        !(*root).window.minimized || self.transition_render_rect(&*root).is_some()
                    }
                    && s.content_is_dmabuf
                    && s.dmabuf.is_some()
                    && visible.contains(unsafe { &(*root).window.id })
                    && !occluded.contains(unsafe { &(*root).window.id })
                    && self
                        .state
                        .authority
                        .interaction_domain_observes_window(HUMAN_INTERACTION_DOMAIN, unsafe {
                            (*root).window.id
                        })
            })
            .filter_map(|s| {
                let db = s.dmabuf.as_ref()?;
                let render_rect = self.transition_render_rect(s);
                let minimize_warp = self.minimize_warp(s);
                let root =
                    unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut SurfaceRec) };
                let delta = if root.is_null() {
                    (0, 0)
                } else {
                    self.root_render_delta(unsafe { &*root })
                };
                let mut origin = surface_draw_origin(s);
                origin.x += delta.0;
                origin.y += delta.1;
                // ADR-0029 open/close fade (see the shm path).
                let is_attached_to_drag = if root.is_null() {
                    false
                } else if let Some(drag) = self.state.drag
                    && !drag.attached_toplevel.is_null()
                {
                    unsafe { (*root).xdg_toplevel == drag.attached_toplevel }
                } else {
                    false
                };
                let transition_opacity = if is_attached_to_drag {
                    Some(0.92)
                } else if root.is_null() {
                    None
                } else {
                    unsafe { &*root }
                        .window
                        .transition
                        .and_then(|t| t.opacity_at(self.state.now_ms()))
                };
                Some(SurfaceDmabuf {
                    id: s.resource as usize,
                    window: if s.xdg_toplevel.is_null() {
                        let root =
                            unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut _) };
                        if root.is_null() {
                            None
                        } else {
                            Some(unsafe { &*root }.window.id)
                        }
                    } else {
                        Some(s.window.id)
                    },
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    damage: s.committed_damage.clone(),
                    buffer_id: db.buffer_id,
                    fd: db.fd,
                    drm_format: db.drm_format,
                    modifier: db.modifier,
                    offset: db.offset,
                    stride: db.stride,
                    acquire_fence: db.acquire_fence,
                    geometry: tessera_primitives::SurfaceGeometry {
                        position: origin,
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        transition_size: render_rect.map(|r| r.size),
                        transition_opacity,
                        minimize_warp,
                        ..Default::default()
                    },
                    opaque_region: s.opaque_region.clone(),
                    color: s.image_description.clone(),
                    presentation_hint: s.presentation_hint,
                })
            })
            .collect()
    }

    /// Every live client surface ID plus in-flight closing ghost frames,
    /// covering all workspaces, previews, overlays, and the lock screen.
    ///
    /// The renderer's texture caches key surfaces by their `wl_resource`
    /// pointer (`SurfacePixels::id`), so the live set must report the same
    /// pointer — not the `SurfaceRec` address. Reporting the record pointer
    /// made every cached texture look dead each frame: the GC dropped all of
    /// them, and the next composite re-created every texture through
    /// `flux_image_create` (a full CPU→GPU copy per surface per frame, the
    /// dominant main-thread cost while dragging windows).
    pub fn live_surface_ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.state
            .live_surfaces()
            // SAFETY: `live_surfaces` yields non-null records owned by this
            // single-threaded server for the duration of the iteration.
            .map(|p| unsafe { (*p).resource as usize })
            .chain(self.state.closing_frames.iter().map(|f| f.id))
    }

    /// All mapped physical-desktop client surfaces backed by shm.
    ///
    /// The returned vector is an unordered backing store. Composite it
    /// against [`client_surface_frame_order`](Self::client_surface_frame_order).
    pub fn client_surface_frames(&self) -> Vec<SurfacePixels<'_>> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        let mut frames = self.toplevel_frames_with(&visible, &occluded);
        frames.extend(self.subsurface_frames_below_with(&visible, &occluded));
        frames.extend(self.subsurface_frames_above_with(&visible, &occluded));
        frames
    }

    /// SHM client surfaces available to offscreen previews. This deliberately
    /// bypasses physical-output occlusion while retaining workspace visibility
    /// and minimize filtering.
    pub fn client_preview_surface_frames(&self) -> Vec<SurfacePixels<'_>> {
        let visible = self.render_visible();
        let occluded = std::collections::HashSet::new();
        let mut frames = self.toplevel_frames_with(&visible, &occluded);
        frames.extend(self.subsurface_frames_below_with(&visible, &occluded));
        frames.extend(self.subsurface_frames_above_with(&visible, &occluded));
        frames
    }

    /// All mapped physical-desktop client surfaces backed by dma-buf.
    ///
    /// The returned vector is an unordered backing store. Composite it
    /// against [`client_surface_frame_order`](Self::client_surface_frame_order).
    pub fn client_surface_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        let mut frames = self.toplevel_dmabuf_frames_with(&visible, &occluded);
        frames.extend(self.subsurface_dmabuf_frames_below_with(&visible, &occluded));
        frames.extend(self.subsurface_dmabuf_frames_above_with(&visible, &occluded));
        frames
    }

    /// Presentation-visible toplevel ids scoped to physical outputs
    /// (`!output.kind.is_virtual()`, `[INV-ARCH-76]`, ADR-0186). Windows
    /// residing exclusively on a virtual extend output are excluded from the
    /// physical union framebuffer and `plan_primary_plane` so they never
    /// disqualify physical direct scanout.
    pub(crate) fn physical_render_visible(
        &self,
    ) -> std::collections::HashSet<tessera_desktop::window::WindowId> {
        let mut visible = self.render_visible();
        let has_virtual = self
            .state
            .output_infos
            .iter()
            .any(|output| output.kind.is_virtual());
        let physical_rects: Vec<tessera_primitives::Rect> = self
            .state
            .output_infos
            .iter()
            .filter(|output| output.kind.is_physical())
            .map(|output| output.geometry.logical_rect())
            .collect();
        if !has_virtual || physical_rects.is_empty() {
            return visible;
        }
        visible.retain(|&window_id| {
            let on_virtual_ws = human_output_connector_for_window(&self.state, window_id)
                .is_some_and(|connector| self.is_virtual_output(connector));
            if !on_virtual_ws {
                return true;
            }
            let rec = self.find_surface_by_window_id(window_id);
            if rec.is_null() {
                return false;
            }
            let win_rect = unsafe {
                let s = &*rec;
                self.transition_render_rect(s)
                    .unwrap_or(tessera_primitives::Rect {
                        origin: s.position,
                        size: if s.window.size.w > 0 && s.window.size.h > 0 {
                            s.window.size
                        } else {
                            surface_logical_size(s)
                        },
                    })
            };
            physical_rects
                .iter()
                .any(|phys| win_rect.intersect(*phys).is_some())
        });
        visible
    }

    /// Presentation-visible toplevel ids on one specific output `connector`
    /// (ADR-0186): windows on `connector`'s current workspace (plus any active
    /// workspace slide on `connector`) or intersecting `connector`'s logical
    /// rectangle.
    pub fn output_visible_windows(
        &self,
        connector: &str,
    ) -> std::collections::HashSet<tessera_desktop::window::WindowId> {
        let Some(output_info) = self
            .state
            .output_infos
            .iter()
            .find(|output| output.connector == connector)
        else {
            return std::collections::HashSet::new();
        };
        let output_rect = output_info.geometry.logical_rect();
        let mut visible = self.render_visible();
        visible.retain(|&window_id| {
            if human_output_connector_for_window(&self.state, window_id) == Some(connector) {
                return true;
            }
            let rec = self.find_surface_by_window_id(window_id);
            if rec.is_null() {
                return false;
            }
            let win_rect = unsafe {
                let s = &*rec;
                self.transition_render_rect(s)
                    .unwrap_or(tessera_primitives::Rect {
                        origin: s.position,
                        size: if s.window.size.w > 0 && s.window.size.h > 0 {
                            s.window.size
                        } else {
                            surface_logical_size(s)
                        },
                    })
            };
            win_rect.intersect(output_rect).is_some()
        });
        visible
    }

    /// Mapped SHM client surfaces visible on `connector`, for offscreen
    /// virtual-output streaming (ADR-0186).
    pub fn output_capture_frames(&self, connector: &str) -> Vec<SurfacePixels<'_>> {
        let visible = self.output_visible_windows(connector);
        let occluded = self.occluded_window_ids_with(&visible);
        let mut frames = self.toplevel_frames_with(&visible, &occluded);
        frames.extend(self.subsurface_frames_below_with(&visible, &occluded));
        frames.extend(self.subsurface_frames_above_with(&visible, &occluded));
        frames
    }

    /// Mapped DMA-BUF client surfaces visible on `connector`, for offscreen
    /// virtual-output streaming (ADR-0186).
    pub fn output_capture_dmabuf_frames(&self, connector: &str) -> Vec<SurfaceDmabuf> {
        let visible = self.output_visible_windows(connector);
        let occluded = self.occluded_window_ids_with(&visible);
        let mut frames = self.toplevel_dmabuf_frames_with(&visible, &occluded);
        frames.extend(self.subsurface_dmabuf_frames_below_with(&visible, &occluded));
        frames.extend(self.subsurface_dmabuf_frames_above_with(&visible, &occluded));
        frames
    }

    /// Paint order for client surfaces visible on `connector` (ADR-0186).
    pub fn output_capture_frame_order(&self, connector: &str) -> Vec<usize> {
        let visible = self.output_visible_windows(connector);
        let occluded = self.occluded_window_ids_with(&visible);
        self.client_surface_frame_order_with(&visible, &occluded)
    }

    /// Fire `wl_surface.frame` callbacks for surfaces visible on `connector`
    /// when a virtual-output stream renders a frame offscreen (ADR-0186).
    pub fn send_output_frame_callbacks(&mut self, connector: &str, time_ms: u32) -> usize {
        if self.state.session_lock_phase.is_active() {
            return 0;
        }
        let visible = self.output_visible_windows(connector);
        if visible.is_empty() {
            return 0;
        }
        let occluded = self.occluded_window_ids_with(&visible);
        // A frame streamed offscreen still presents the surfaces it samples;
        // their presentation feedback completes with it (ADR-0122, ADR-0186).
        // Per-output capture builds a fresh frame list for this connector, so
        // the caller cannot name the individual ids; `All` is the conservative
        // "this frame rendered the connector's whole scene" claim.
        self.complete_presentation_feedback(&visible, &occluded, false, PresentedSurfaces::All);
        let transition_now_ms = self.now_ms();
        let mut sent = 0;
        for &p in &self.state.surfaces {
            if p.is_null() {
                continue;
            }
            let rec = unsafe { &mut *p };
            if !rec.mapped {
                continue;
            }
            let output_visible = unsafe {
                surface_receives_output_frame_callback(
                    p,
                    &visible,
                    &occluded,
                    false,
                    transition_now_ms,
                )
            };
            if !output_visible {
                continue;
            }
            let pending_cb = std::mem::take(&mut rec.frame_callbacks);
            for cb in pending_cb {
                unsafe {
                    ffi::wl_resource_set_implementation(
                        cb,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        None,
                    );
                    ffi::wl_resource_post_event(cb, ffi::WL_CALLBACK_DONE, time_ms);
                    ffi::wl_resource_destroy(cb);
                }
                sent += 1;
            }
        }
        if sent > 0 {
            unsafe { ffi::wl_display_flush_clients(self.state.display) };
        }
        sent
    }

    /// Both frame sets plus the occlusion verdicts in one pass.
    ///
    /// The damage assessment, scanout planner, and the render pass each need
    /// the SHM and dma-buf frame lists, and every one of those previously
    /// recomputed `render_visible()` + `occluded_window_ids()` from scratch —
    /// each an O(windows × surfaces) walk with per-window allocations, so a
    /// single presented frame paid for it roughly eight times while the
    /// surface table cannot change (all of this runs inside one iteration
    /// after `server.dispatch()`). This accessor computes the shared
    /// visibility/occlusion snapshot once and derives both lists from it.
    pub fn desktop_frame_sets(&self) -> DesktopFrameSets<'_> {
        let visible = self.physical_render_visible();
        let occluded = self.occluded_window_ids_with(&visible);
        let mut shm = self.toplevel_frames_with(&visible, &occluded);
        shm.extend(self.subsurface_frames_below_with(&visible, &occluded));
        shm.extend(self.subsurface_frames_above_with(&visible, &occluded));
        let mut dmabuf = self.toplevel_dmabuf_frames_with(&visible, &occluded);
        dmabuf.extend(self.subsurface_dmabuf_frames_below_with(&visible, &occluded));
        dmabuf.extend(self.subsurface_dmabuf_frames_above_with(&visible, &occluded));
        DesktopFrameSets {
            visible,
            occluded,
            shm,
            dmabuf,
        }
    }

    /// DMA-BUF client surfaces available to offscreen previews, including
    /// windows culled from the physical desktop because they are fully covered.
    pub fn client_preview_surface_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let visible = self.render_visible();
        let occluded = std::collections::HashSet::new();
        let mut frames = self.toplevel_dmabuf_frames_with(&visible, &occluded);
        frames.extend(self.subsurface_dmabuf_frames_below_with(&visible, &occluded));
        frames.extend(self.subsurface_dmabuf_frames_above_with(&visible, &occluded));
        frames
    }

    /// SHM client surfaces for an explicit preview window set. The overview's
    /// workspace rail draws live miniatures of workspaces other than the
    /// visible one, so unlike [`client_preview_surface_frames`](Self::client_preview_surface_frames)
    /// the caller supplies the window set instead of the presentation-visible
    /// one. Occlusion is still bypassed.
    pub fn client_preview_frames_for(
        &self,
        windows: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfacePixels<'_>> {
        let occluded = std::collections::HashSet::new();
        let mut frames = self.toplevel_frames_with(windows, &occluded);
        frames.extend(self.subsurface_frames_below_with(windows, &occluded));
        frames.extend(self.subsurface_frames_above_with(windows, &occluded));
        frames
    }

    /// DMA-BUF client surfaces for an explicit preview window set; see
    /// [`client_preview_frames_for`](Self::client_preview_frames_for).
    pub fn client_preview_dmabuf_frames_for(
        &self,
        windows: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfaceDmabuf> {
        let occluded = std::collections::HashSet::new();
        let mut frames = self.toplevel_dmabuf_frames_with(windows, &occluded);
        frames.extend(self.subsurface_dmabuf_frames_below_with(windows, &occluded));
        frames.extend(self.subsurface_dmabuf_frames_above_with(windows, &occluded));
        frames
    }

    /// Paint order for an explicit preview window set; see
    /// [`client_preview_frames_for`](Self::client_preview_frames_for) and
    /// [`client_preview_surface_frame_order`](Self::client_preview_surface_frame_order).
    pub fn client_preview_frame_order_for(
        &self,
        windows: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<usize> {
        self.client_surface_frame_order_for_interaction_domain(
            HUMAN_INTERACTION_DOMAIN,
            Some(windows),
            None,
        )
    }

    /// SHM client surfaces of one window's complete surface tree for
    /// per-window content capture. Unlike the preview enumerations this keeps
    /// minimized, occluded, and foreign-workspace windows: their buffers stay
    /// live regardless of presentation, and authorization happened at the IPC
    /// layer. Geometry stays in compositor logical coordinates; the capture
    /// renderer translates the window origin to (0, 0), so popups extending
    /// past the toplevel bounds are clipped by the offscreen target.
    pub fn window_capture_frames(
        &self,
        window: tessera_desktop::window::WindowId,
    ) -> Vec<SurfacePixels<'_>> {
        let mut frames = self.window_capture_toplevel_frames(window);
        frames.extend(self.collect_window_capture_subsurfaces_shm(window, false));
        frames.extend(self.collect_window_capture_subsurfaces_shm(window, true));
        frames
    }

    /// DMA-BUF counterpart of
    /// [`window_capture_frames`](Self::window_capture_frames).
    pub fn window_capture_dmabuf_frames(
        &self,
        window: tessera_desktop::window::WindowId,
    ) -> Vec<SurfaceDmabuf> {
        let mut frames = self.window_capture_toplevel_dmabuf_frames(window);
        frames.extend(self.collect_window_capture_subsurfaces_dmabuf(window, false));
        frames.extend(self.collect_window_capture_subsurfaces_dmabuf(window, true));
        frames
    }

    /// Paint order for the window-capture frames: the target toplevel's
    /// surface tree plus its popup trees, with no presentation filtering.
    pub fn window_capture_frame_order(
        &self,
        window: tessera_desktop::window::WindowId,
    ) -> Vec<usize> {
        let roots = self
            .state
            .live_surfaces()
            .filter(|pointer| unsafe {
                let surface = &**pointer;
                surface.mapped && !surface.xdg_toplevel.is_null() && surface.window.id == window
            })
            .collect::<Vec<_>>();
        let popups = self
            .state
            .live_surfaces()
            .filter(|pointer| unsafe {
                let surface = &**pointer;
                surface.mapped && !surface.xdg_popup.is_null()
            })
            .collect::<Vec<_>>();
        let mut order = Vec::new();
        for root in roots {
            unsafe {
                append_surface_tree_frame_order(root, &mut order, 0);
            }
            // Keep every popup with its owning toplevel, exactly like the
            // physical-desktop order: parts outside the toplevel's logical
            // rectangle land outside the offscreen target and are clipped.
            for popup in &popups {
                if unsafe { surface_root_toplevel(*popup) == root } {
                    unsafe {
                        append_surface_tree_frame_order(*popup, &mut order, 0);
                    }
                }
            }
        }
        order
    }

    fn window_capture_toplevel_frames(
        &self,
        window: tessera_desktop::window::WindowId,
    ) -> Vec<SurfacePixels<'_>> {
        self.state
            .live_surfaces()
            .map(|pointer| unsafe { &*pointer })
            .filter(|surface| {
                let root = unsafe {
                    surface_root_toplevel(*surface as *const SurfaceRec as *mut SurfaceRec)
                };
                surface.mapped
                    && (!surface.xdg_toplevel.is_null() || !surface.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe { (*root).window.id == window }
                    && !surface.content_is_dmabuf
                    && !surface.pixels.is_empty()
            })
            .map(|surface| SurfacePixels {
                id: surface.resource as usize,
                window: Some(window),
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                pixels: &surface.pixels,
                geometry: tessera_primitives::SurfaceGeometry {
                    position: surface_draw_origin(surface),
                    transform: surface.buffer_transform,
                    buffer_scale: surface.buffer_scale,
                    viewport_src: surface.viewport_src,
                    viewport_dst: surface.viewport_dst,
                    ..Default::default()
                },
                damage: &surface.committed_damage,
                opaque_region: surface.opaque_region.as_deref(),
                color: surface.image_description.clone(),
            })
            .collect()
    }

    fn window_capture_toplevel_dmabuf_frames(
        &self,
        window: tessera_desktop::window::WindowId,
    ) -> Vec<SurfaceDmabuf> {
        self.state
            .live_surfaces()
            .map(|pointer| unsafe { &*pointer })
            .filter(|surface| {
                let root = unsafe {
                    surface_root_toplevel(*surface as *const SurfaceRec as *mut SurfaceRec)
                };
                surface.mapped
                    && (!surface.xdg_toplevel.is_null() || !surface.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe { (*root).window.id == window }
                    && surface.content_is_dmabuf
                    && surface.dmabuf.is_some()
            })
            .filter_map(|surface| {
                let dmabuf = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    id: surface.resource as usize,
                    window: Some(window),
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    damage: surface.committed_damage.clone(),
                    buffer_id: dmabuf.buffer_id,
                    fd: dmabuf.fd,
                    drm_format: dmabuf.drm_format,
                    modifier: dmabuf.modifier,
                    offset: dmabuf.offset,
                    stride: dmabuf.stride,
                    acquire_fence: dmabuf.acquire_fence,
                    geometry: tessera_primitives::SurfaceGeometry {
                        position: surface_draw_origin(surface),
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                    opaque_region: surface.opaque_region.clone(),
                    color: surface.image_description.clone(),
                    presentation_hint: surface.presentation_hint,
                })
            })
            .collect()
    }

    fn collect_window_capture_subsurfaces_shm(
        &self,
        window: tessera_desktop::window::WindowId,
        want_above: bool,
    ) -> Vec<SurfacePixels<'_>> {
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped || root_surface.window.id != window {
                continue;
            }
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_shm(child, &mut out, (0, 0), window, 0);
            }
        }
        out
    }

    fn collect_window_capture_subsurfaces_dmabuf(
        &self,
        window: tessera_desktop::window::WindowId,
        want_above: bool,
    ) -> Vec<SurfaceDmabuf> {
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped || root_surface.window.id != window {
                continue;
            }
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_dmabuf(child, &mut out, (0, 0), window, 0);
            }
        }
        out
    }

    /// Directed offscreen scene for one interaction domain. Unlike the physical desktop,
    /// virtual interaction domains are independent of the user's currently visible
    /// workspace and use interaction domain-local placements on their virtual output.
    pub fn interaction_domain_toplevel_frames(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfacePixels<'_>> {
        if self.state.session_lock_phase.is_active()
            || self.interaction_domain_output(interaction_domain).is_none()
        {
            return Vec::new();
        }
        self.state
            .live_surfaces()
            .map(|pointer| unsafe { &*pointer })
            .filter_map(|surface| {
                let root = unsafe {
                    surface_root_toplevel(surface as *const SurfaceRec as *mut SurfaceRec)
                };
                if !surface.mapped
                    || (surface.xdg_toplevel.is_null() && surface.xdg_popup.is_null())
                    || root.is_null()
                    || unsafe { (*root).window.minimized }
                    || surface.content_is_dmabuf
                    || surface.pixels.is_empty()
                    || !self
                        .state
                        .authority
                        .interaction_domain_observes_window(interaction_domain, unsafe {
                            (*root).window.id
                        })
                {
                    return None;
                }
                Some(SurfacePixels {
                    id: surface.resource as usize,
                    window: Some(unsafe { (*root).window.id }),
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    pixels: &surface.pixels,
                    geometry: self.interaction_domain_surface_geometry(
                        surface,
                        root,
                        interaction_domain,
                    )?,
                    damage: &surface.committed_damage,
                    opaque_region: surface.opaque_region.as_deref(),
                    color: surface.image_description.clone(),
                })
            })
            .collect()
    }

    /// All mapped client surfaces backed by shm for one directed Interaction Domain.
    pub fn interaction_domain_client_surface_frames(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfacePixels<'_>> {
        let mut frames = self.interaction_domain_toplevel_frames(interaction_domain);
        frames.extend(self.interaction_domain_subsurface_frames_below(interaction_domain));
        frames.extend(self.interaction_domain_subsurface_frames_above(interaction_domain));
        frames
    }

    /// All mapped client surfaces backed by dma-buf for one directed Interaction Domain.
    pub fn interaction_domain_client_surface_dmabuf_frames(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfaceDmabuf> {
        let mut frames = self.interaction_domain_toplevel_dmabuf_frames(interaction_domain);
        frames.extend(self.interaction_domain_subsurface_dmabuf_frames_below(interaction_domain));
        frames.extend(self.interaction_domain_subsurface_dmabuf_frames_above(interaction_domain));
        frames
    }

    pub fn interaction_domain_toplevel_dmabuf_frames(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfaceDmabuf> {
        if self.state.session_lock_phase.is_active()
            || self.interaction_domain_output(interaction_domain).is_none()
        {
            return Vec::new();
        }
        self.state
            .live_surfaces()
            .map(|pointer| unsafe { &*pointer })
            .filter_map(|surface| {
                let root = unsafe {
                    surface_root_toplevel(surface as *const SurfaceRec as *mut SurfaceRec)
                };
                if !surface.mapped
                    || (surface.xdg_toplevel.is_null() && surface.xdg_popup.is_null())
                    || root.is_null()
                    || unsafe { (*root).window.minimized }
                    || !surface.content_is_dmabuf
                    || !self
                        .state
                        .authority
                        .interaction_domain_observes_window(interaction_domain, unsafe {
                            (*root).window.id
                        })
                {
                    return None;
                }
                let dmabuf = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    id: surface.resource as usize,
                    window: Some(unsafe { (*root).window.id }),
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    damage: surface.committed_damage.clone(),
                    opaque_region: surface.opaque_region.clone(),
                    color: surface.image_description.clone(),
                    buffer_id: dmabuf.buffer_id,
                    fd: dmabuf.fd,
                    drm_format: dmabuf.drm_format,
                    modifier: dmabuf.modifier,
                    offset: dmabuf.offset,
                    stride: dmabuf.stride,
                    acquire_fence: dmabuf.acquire_fence,
                    geometry: self.interaction_domain_surface_geometry(
                        surface,
                        root,
                        interaction_domain,
                    )?,
                    presentation_hint: surface.presentation_hint,
                })
            })
            .collect()
    }

    pub(crate) fn interaction_domain_surface_geometry(
        &self,
        surface: &SurfaceRec,
        root: *mut SurfaceRec,
        interaction_domain: InteractionDomainId,
    ) -> Option<tessera_primitives::SurfaceGeometry> {
        let root = unsafe { &*root };
        let placement = self
            .state
            .interaction_domain_placements
            .get(&(interaction_domain, root.window.id))?;
        let root_size = if root.window.size.w > 0 && root.window.size.h > 0 {
            root.window.size
        } else {
            surface_logical_size(root)
        };
        if root_size.w <= 0 || root_size.h <= 0 {
            return None;
        }
        let scale = (placement.size.w as f32 / root_size.w as f32)
            .min(placement.size.h as f32 / root_size.h as f32);
        let source_origin = surface_draw_origin(surface);
        let relative_x = source_origin.x - root.position.x;
        let relative_y = source_origin.y - root.position.y;
        let logical_size = surface_logical_size(surface);
        Some(tessera_primitives::SurfaceGeometry {
            position: tessera_primitives::Point {
                x: placement.origin.x + (relative_x as f32 * scale).round() as i32,
                y: placement.origin.y + (relative_y as f32 * scale).round() as i32,
            },
            transform: surface.buffer_transform,
            buffer_scale: surface.buffer_scale,
            viewport_src: surface.viewport_src,
            viewport_dst: surface.viewport_dst,
            transition_size: Some(tessera_primitives::Size {
                w: (logical_size.w as f32 * scale).round().max(1.0) as i32,
                h: (logical_size.h as f32 * scale).round().max(1.0) as i32,
            }),
            ..Default::default()
        })
    }

    /// Mapped session-lock surfaces backed by shm. The compositor renders
    /// these over an opaque fallback and never mixes them with normal clients.
    pub fn lock_frames(&self) -> Vec<SurfacePixels<'_>> {
        let mut frames = self
            .state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| unsafe {
                extensions::is_active_session_lock_surface(
                    self.state.as_ref() as *const State as *mut State,
                    *surface as *const SurfaceRec as *mut SurfaceRec,
                )
            })
            .filter(|surface| {
                surface.mapped && !surface.content_is_dmabuf && !surface.pixels.is_empty()
            })
            .map(|surface| SurfacePixels {
                window: None,
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                pixels: &surface.pixels,
                geometry: tessera_primitives::SurfaceGeometry {
                    position: surface.position,
                    transform: surface.buffer_transform,
                    buffer_scale: surface.buffer_scale,
                    viewport_src: surface.viewport_src,
                    viewport_dst: surface.viewport_dst,
                    ..Default::default()
                },
                damage: &surface.committed_damage,
                opaque_region: surface.opaque_region.as_deref(),
                color: surface.image_description.clone(),
            })
            .collect::<Vec<_>>();
        let cursor = self.state.cursor_surface;
        if !cursor.is_null()
            && unsafe {
                extensions::is_active_session_lock_client_resource(
                    self.state.as_ref() as *const State as *mut State,
                    cursor,
                )
            }
        {
            let surface = unsafe { ffi::wl_resource_get_user_data(cursor) as *mut SurfaceRec };
            if !surface.is_null() {
                let surface = unsafe { &*surface };
                if surface.mapped && !surface.content_is_dmabuf && !surface.pixels.is_empty() {
                    frames.push(SurfacePixels {
                        window: None,
                        id: surface.resource as usize,
                        width: surface.width,
                        height: surface.height,
                        generation: surface.generation,
                        pixels: &surface.pixels,
                        geometry: tessera_primitives::SurfaceGeometry {
                            position: surface.position,
                            transform: surface.buffer_transform,
                            buffer_scale: surface.buffer_scale,
                            viewport_src: surface.viewport_src,
                            viewport_dst: surface.viewport_dst,
                            ..Default::default()
                        },
                        damage: &surface.committed_damage,
                        opaque_region: surface.opaque_region.as_deref(),
                        color: surface.image_description.clone(),
                    });
                }
            }
        }
        frames
    }

    /// dma-buf variant of [`lock_frames`](Self::lock_frames).
    pub fn lock_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let mut frames = self
            .state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| unsafe {
                extensions::is_active_session_lock_surface(
                    self.state.as_ref() as *const State as *mut State,
                    *surface as *const SurfaceRec as *mut SurfaceRec,
                )
            })
            .filter(|surface| surface.mapped && surface.content_is_dmabuf)
            .filter_map(|surface| {
                let buffer = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    damage: surface.committed_damage.clone(),
                    opaque_region: surface.opaque_region.clone(),
                    color: surface.image_description.clone(),
                    buffer_id: buffer.buffer_id,
                    fd: buffer.fd,
                    drm_format: buffer.drm_format,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                    acquire_fence: buffer.acquire_fence,
                    geometry: tessera_primitives::SurfaceGeometry {
                        position: surface.position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                    presentation_hint: tessera_primitives::PresentationHint::Vsync,
                })
            })
            .collect::<Vec<_>>();
        let cursor = self.state.cursor_surface;
        if !cursor.is_null()
            && unsafe {
                extensions::is_active_session_lock_client_resource(
                    self.state.as_ref() as *const State as *mut State,
                    cursor,
                )
            }
        {
            let surface = unsafe { ffi::wl_resource_get_user_data(cursor) as *mut SurfaceRec };
            if !surface.is_null() {
                let surface = unsafe { &*surface };
                if surface.mapped
                    && surface.content_is_dmabuf
                    && let Some(buffer) = surface.dmabuf.as_ref()
                {
                    frames.push(SurfaceDmabuf {
                        window: None,
                        id: surface.resource as usize,
                        width: surface.width,
                        height: surface.height,
                        generation: surface.generation,
                        damage: surface.committed_damage.clone(),
                        opaque_region: surface.opaque_region.clone(),
                        color: surface.image_description.clone(),
                        buffer_id: buffer.buffer_id,
                        fd: buffer.fd,
                        drm_format: buffer.drm_format,
                        modifier: buffer.modifier,
                        offset: buffer.offset,
                        stride: buffer.stride,
                        acquire_fence: buffer.acquire_fence,
                        geometry: tessera_primitives::SurfaceGeometry {
                            position: surface.position,
                            transform: surface.buffer_transform,
                            buffer_scale: surface.buffer_scale,
                            viewport_src: surface.viewport_src,
                            viewport_dst: surface.viewport_dst,
                            ..Default::default()
                        },
                        presentation_hint: tessera_primitives::PresentationHint::Vsync,
                    });
                }
            }
        }
        frames
    }

    /// Input-popup, drag-icon, and cursor role surfaces, composited above all
    /// client toplevels and subsurfaces in that order.
    pub fn overlay_frames(&self) -> Vec<SurfacePixels<'_>> {
        self.overlay_frames_with_cursor(true)
    }

    /// Overlay frames with optional client-cursor inclusion. Input-method
    /// popups and drag icons remain present when the cursor is excluded.
    pub fn overlay_frames_with_cursor(&self, include_cursor: bool) -> Vec<SurfacePixels<'_>> {
        self.overlay_frames_with_cursor_at(include_cursor, None)
    }

    /// Overlay frames with an optional logical cursor-position override.
    /// Screenshot capture uses the override sampled with its request so a
    /// later pointer event cannot move a client-provided cursor surface in
    /// the captured frame.
    pub fn overlay_frames_with_cursor_at(
        &self,
        include_cursor: bool,
        cursor_position: Option<(f32, f32)>,
    ) -> Vec<SurfacePixels<'_>> {
        let drag_icon = self
            .state
            .drag
            .as_ref()
            .map_or(std::ptr::null_mut(), |drag| drag.icon);
        let mut resources = unsafe {
            extensions::input_popup_resources(self.state.as_ref() as *const _ as *mut _, HUMAN_SEAT)
        };
        resources.push(drag_icon);
        if include_cursor {
            resources.push(self.state.cursor_surface);
        }
        resources
            .into_iter()
            .filter(|resource| !resource.is_null())
            .filter_map(|resource| {
                let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
                if rec.is_null() {
                    return None;
                }
                let surface = unsafe { &*rec };
                if !surface.mapped || surface.content_is_dmabuf || surface.pixels.is_empty() {
                    return None;
                }
                let position = if resource == self.state.cursor_surface {
                    cursor_position
                        .map(|position| {
                            cursor_surface_position(
                                position,
                                self.state.cursor_hotspot,
                                surface.attach_offset,
                            )
                        })
                        .unwrap_or(surface.position)
                } else {
                    surface.position
                };
                Some(SurfacePixels {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    pixels: &surface.pixels,
                    geometry: tessera_primitives::SurfaceGeometry {
                        position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                    damage: &surface.committed_damage,
                    opaque_region: surface.opaque_region.as_deref(),
                    color: surface.image_description.clone(),
                })
            })
            .collect()
    }

    /// dma-buf variant of [`overlay_frames`](Self::overlay_frames).
    pub fn overlay_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        self.overlay_dmabuf_frames_with_cursor(true)
    }

    /// dma-buf overlay frames with optional client-cursor inclusion.
    pub fn overlay_dmabuf_frames_with_cursor(&self, include_cursor: bool) -> Vec<SurfaceDmabuf> {
        self.overlay_dmabuf_frames_with_cursor_at(include_cursor, None)
    }

    /// dma-buf overlay frames with an optional logical cursor-position
    /// override. This is the dma-buf counterpart of
    /// [`Self::overlay_frames_with_cursor_at`].
    pub fn overlay_dmabuf_frames_with_cursor_at(
        &self,
        include_cursor: bool,
        cursor_position: Option<(f32, f32)>,
    ) -> Vec<SurfaceDmabuf> {
        let drag_icon = self
            .state
            .drag
            .as_ref()
            .map_or(std::ptr::null_mut(), |drag| drag.icon);
        let mut resources = unsafe {
            extensions::input_popup_resources(self.state.as_ref() as *const _ as *mut _, HUMAN_SEAT)
        };
        resources.push(drag_icon);
        if include_cursor {
            resources.push(self.state.cursor_surface);
        }
        resources
            .into_iter()
            .filter(|resource| !resource.is_null())
            .filter_map(|resource| {
                let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
                if rec.is_null() {
                    return None;
                }
                let surface = unsafe { &*rec };
                if !surface.mapped || !surface.content_is_dmabuf {
                    return None;
                }
                let buffer = surface.dmabuf.as_ref()?;
                let position = if resource == self.state.cursor_surface {
                    cursor_position
                        .map(|position| {
                            cursor_surface_position(
                                position,
                                self.state.cursor_hotspot,
                                surface.attach_offset,
                            )
                        })
                        .unwrap_or(surface.position)
                } else {
                    surface.position
                };
                Some(SurfaceDmabuf {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    damage: surface.committed_damage.clone(),
                    opaque_region: surface.opaque_region.clone(),
                    color: surface.image_description.clone(),
                    buffer_id: buffer.buffer_id,
                    fd: buffer.fd,
                    drm_format: buffer.drm_format,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                    acquire_fence: buffer.acquire_fence,
                    geometry: tessera_primitives::SurfaceGeometry {
                        position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                    presentation_hint: surface.presentation_hint,
                })
            })
            .collect()
    }

    /// Mapped subsurfaces backed by shm whose `place_below` was the most
    /// recent stacking request — these render *under* their parent toplevel.
    /// Nested subsurface chains are walked recursively: each entry carries
    /// its compositor-space draw origin, and the whole subtree of a
    /// below-child renders here, in render order.
    pub fn subsurface_frames_below(&self) -> Vec<SurfacePixels<'_>> {
        self.collect_subsurfaces_shm(false)
    }

    fn subsurface_frames_below_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfacePixels<'_>> {
        self.collect_subsurfaces_shm_with(false, visible, occluded)
    }

    /// As [`subsurface_frames_below`](Self::subsurface_frames_below) for
    /// surfaces whose most recent stacking request was `place_above` (or the
    /// default). These render *over* their parent toplevel.
    pub fn subsurface_frames_above(&self) -> Vec<SurfacePixels<'_>> {
        self.collect_subsurfaces_shm(true)
    }

    fn subsurface_frames_above_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfacePixels<'_>> {
        self.collect_subsurfaces_shm_with(true, visible, occluded)
    }

    /// Mapped dma-buf-backed subsurfaces below their parent.
    pub fn subsurface_dmabuf_frames_below(&self) -> Vec<SurfaceDmabuf> {
        self.collect_subsurfaces_dmabuf(false)
    }

    fn subsurface_dmabuf_frames_below_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfaceDmabuf> {
        self.collect_subsurfaces_dmabuf_with(false, visible, occluded)
    }

    /// Mapped dma-buf-backed subsurfaces above their parent.
    pub fn subsurface_dmabuf_frames_above(&self) -> Vec<SurfaceDmabuf> {
        self.collect_subsurfaces_dmabuf(true)
    }

    fn subsurface_dmabuf_frames_above_with(
        &self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfaceDmabuf> {
        self.collect_subsurfaces_dmabuf_with(true, visible, occluded)
    }

    pub fn interaction_domain_subsurface_frames_below(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfacePixels<'_>> {
        self.collect_interaction_domain_subsurfaces_shm(interaction_domain, false)
    }

    pub fn interaction_domain_subsurface_frames_above(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfacePixels<'_>> {
        self.collect_interaction_domain_subsurfaces_shm(interaction_domain, true)
    }

    pub fn interaction_domain_subsurface_dmabuf_frames_below(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfaceDmabuf> {
        self.collect_interaction_domain_subsurfaces_dmabuf(interaction_domain, false)
    }

    pub fn interaction_domain_subsurface_dmabuf_frames_above(
        &self,
        interaction_domain: InteractionDomainId,
    ) -> Vec<SurfaceDmabuf> {
        self.collect_interaction_domain_subsurfaces_dmabuf(interaction_domain, true)
    }

    pub(crate) fn collect_interaction_domain_subsurfaces_shm(
        &self,
        interaction_domain: InteractionDomainId,
        want_above: bool,
    ) -> Vec<SurfacePixels<'_>> {
        if self.state.session_lock_phase.is_active()
            || self.interaction_domain_output(interaction_domain).is_none()
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !self
                    .state
                    .authority
                    .interaction_domain_observes_window(interaction_domain, root_surface.window.id)
            {
                continue;
            }
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent == want_above {
                    self.collect_interaction_domain_subtree_shm(
                        child,
                        root,
                        interaction_domain,
                        root_surface.window.id,
                        &mut out,
                        0,
                    );
                }
            }
        }
        out
    }

    pub(crate) fn collect_interaction_domain_subtree_shm<'a>(
        &'a self,
        surface: &'a SurfaceRec,
        root: *mut SurfaceRec,
        interaction_domain: InteractionDomainId,
        window: tessera_desktop::window::WindowId,
        out: &mut Vec<SurfacePixels<'a>>,
        depth: u32,
    ) {
        if !surface.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { !(*child_ptr).subsurface_above_parent } {
                self.collect_interaction_domain_subtree_shm(
                    unsafe { &*child_ptr },
                    root,
                    interaction_domain,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
        if !surface.content_is_dmabuf
            && !surface.pixels.is_empty()
            && let Some(geometry) =
                self.interaction_domain_surface_geometry(surface, root, interaction_domain)
        {
            out.push(SurfacePixels {
                window: Some(window),
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                pixels: &surface.pixels,
                geometry,
                damage: &surface.committed_damage,
                opaque_region: surface.opaque_region.as_deref(),
                color: surface.image_description.clone(),
            });
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { (*child_ptr).subsurface_above_parent } {
                self.collect_interaction_domain_subtree_shm(
                    unsafe { &*child_ptr },
                    root,
                    interaction_domain,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
    }

    pub(crate) fn collect_interaction_domain_subsurfaces_dmabuf(
        &self,
        interaction_domain: InteractionDomainId,
        want_above: bool,
    ) -> Vec<SurfaceDmabuf> {
        if self.state.session_lock_phase.is_active()
            || self.interaction_domain_output(interaction_domain).is_none()
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !self
                    .state
                    .authority
                    .interaction_domain_observes_window(interaction_domain, root_surface.window.id)
            {
                continue;
            }
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent == want_above {
                    self.collect_interaction_domain_subtree_dmabuf(
                        child,
                        root,
                        interaction_domain,
                        root_surface.window.id,
                        &mut out,
                        0,
                    );
                }
            }
        }
        out
    }

    pub(crate) fn collect_interaction_domain_subtree_dmabuf(
        &self,
        surface: &SurfaceRec,
        root: *mut SurfaceRec,
        interaction_domain: InteractionDomainId,
        window: tessera_desktop::window::WindowId,
        out: &mut Vec<SurfaceDmabuf>,
        depth: u32,
    ) {
        if !surface.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { !(*child_ptr).subsurface_above_parent } {
                self.collect_interaction_domain_subtree_dmabuf(
                    unsafe { &*child_ptr },
                    root,
                    interaction_domain,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
        if surface.content_is_dmabuf
            && let (Some(dmabuf), Some(geometry)) = (
                surface.dmabuf.as_ref(),
                self.interaction_domain_surface_geometry(surface, root, interaction_domain),
            )
        {
            out.push(SurfaceDmabuf {
                window: Some(window),
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                damage: surface.committed_damage.clone(),
                opaque_region: surface.opaque_region.clone(),
                color: surface.image_description.clone(),
                buffer_id: dmabuf.buffer_id,
                fd: dmabuf.fd,
                drm_format: dmabuf.drm_format,
                modifier: dmabuf.modifier,
                offset: dmabuf.offset,
                stride: dmabuf.stride,
                acquire_fence: dmabuf.acquire_fence,
                geometry,
                presentation_hint: surface.presentation_hint,
            });
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { (*child_ptr).subsurface_above_parent } {
                self.collect_interaction_domain_subtree_dmabuf(
                    unsafe { &*child_ptr },
                    root,
                    interaction_domain,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
    }

    pub(crate) fn collect_subsurfaces_shm(&self, want_above: bool) -> Vec<SurfacePixels<'_>> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        self.collect_subsurfaces_shm_with(want_above, &visible, &occluded)
    }

    fn collect_subsurfaces_shm_with(
        &self,
        want_above: bool,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfacePixels<'_>> {
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !visible.contains(&root_surface.window.id)
                || occluded.contains(&root_surface.window.id)
                || !self.state.authority.interaction_domain_observes_window(
                    HUMAN_INTERACTION_DOMAIN,
                    root_surface.window.id,
                )
            {
                continue;
            }
            // ADR-0029: the toplevel's in-flight transition shifts its whole
            // subsurface tree by the same delta.
            let delta = self.root_render_delta(root_surface);
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_shm(child, &mut out, delta, root_surface.window.id, 0);
            }
        }
        out
    }

    /// Emit one subsurface subtree in render order: the below-children
    /// subtrees, the surface itself, then the above-children subtrees. A
    /// subsurface's descendants render relative to it, so the whole subtree
    /// of a below-child still renders under the root toplevel. An unmapped
    /// node hides its entire subtree, per `wl_subsurface` mapping rules.
    /// `delta` shifts every emitted origin by the root's in-flight geometry
    /// transition (ADR-0029); (0, 0) outside transitions. Workspace motion is
    /// applied later to the complete, independently clipped workspace page.
    pub(crate) fn collect_subtree_shm<'a>(
        s: &'a SurfaceRec,
        out: &mut Vec<SurfacePixels<'a>>,
        delta: (i32, i32),
        window: tessera_desktop::window::WindowId,
        depth: u32,
    ) {
        // The depth cap only breaks reference cycles defensively; children
        // are orphaned on destroy, so live child pointers are always valid.
        if !s.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if !child.subsurface_above_parent {
                Self::collect_subtree_shm(child, out, delta, window, depth + 1);
            }
        }
        if !s.content_is_dmabuf && !s.pixels.is_empty() {
            let origin = surface_draw_origin(s);
            out.push(SurfacePixels {
                window: Some(window),
                id: s.resource as usize,
                width: s.width,
                height: s.height,
                generation: s.generation,
                pixels: &s.pixels,
                geometry: tessera_primitives::SurfaceGeometry {
                    position: tessera_primitives::Point {
                        x: origin.x + delta.0,
                        y: origin.y + delta.1,
                    },
                    transform: s.buffer_transform,
                    buffer_scale: s.buffer_scale,
                    viewport_src: s.viewport_src,
                    viewport_dst: s.viewport_dst,
                    ..Default::default()
                },
                damage: &s.committed_damage,
                opaque_region: s.opaque_region.as_deref(),
                color: s.image_description.clone(),
            });
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent {
                Self::collect_subtree_shm(child, out, delta, window, depth + 1);
            }
        }
    }

    pub(crate) fn collect_subsurfaces_dmabuf(&self, want_above: bool) -> Vec<SurfaceDmabuf> {
        let visible = self.render_visible();
        let occluded = self.occluded_window_ids();
        self.collect_subsurfaces_dmabuf_with(want_above, &visible, &occluded)
    }

    fn collect_subsurfaces_dmabuf_with(
        &self,
        want_above: bool,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    ) -> Vec<SurfaceDmabuf> {
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !visible.contains(&root_surface.window.id)
                || occluded.contains(&root_surface.window.id)
                || !self.state.authority.interaction_domain_observes_window(
                    HUMAN_INTERACTION_DOMAIN,
                    root_surface.window.id,
                )
            {
                continue;
            }
            let delta = self.root_render_delta(root_surface);
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_dmabuf(child, &mut out, delta, root_surface.window.id, 0);
            }
        }
        out
    }

    /// The dma-buf half of [`Self::collect_subtree_shm`]: same render-order
    /// tree walk, emitting only dma-buf-backed surfaces.
    pub(crate) fn collect_subtree_dmabuf(
        s: &SurfaceRec,
        out: &mut Vec<SurfaceDmabuf>,
        delta: (i32, i32),
        window: tessera_desktop::window::WindowId,
        depth: u32,
    ) {
        if !s.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if !child.subsurface_above_parent {
                Self::collect_subtree_dmabuf(child, out, delta, window, depth + 1);
            }
        }
        if s.content_is_dmabuf
            && let Some(db) = s.dmabuf.as_ref()
        {
            let origin = surface_draw_origin(s);
            out.push(SurfaceDmabuf {
                window: Some(window),
                id: s.resource as usize,
                width: s.width,
                height: s.height,
                generation: s.generation,
                damage: s.committed_damage.clone(),
                opaque_region: s.opaque_region.clone(),
                color: s.image_description.clone(),
                buffer_id: db.buffer_id,
                fd: db.fd,
                drm_format: db.drm_format,
                modifier: db.modifier,
                offset: db.offset,
                stride: db.stride,
                acquire_fence: db.acquire_fence,
                geometry: tessera_primitives::SurfaceGeometry {
                    position: tessera_primitives::Point {
                        x: origin.x + delta.0,
                        y: origin.y + delta.1,
                    },
                    transform: s.buffer_transform,
                    buffer_scale: s.buffer_scale,
                    viewport_src: s.viewport_src,
                    viewport_dst: s.viewport_dst,
                    ..Default::default()
                },
                presentation_hint: s.presentation_hint,
            });
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent {
                Self::collect_subtree_dmabuf(child, out, delta, window, depth + 1);
            }
        }
    }

    /// Refresh interval in nanoseconds for `wp_presentation_feedback`.
    ///
    /// This must be the period the presentation domain *actually* presents at,
    /// which is the slowest output of the atomic batch
    /// ([`tessera_desktop::output::presentation_domain_period_ns`]) — the same
    /// derivation the compositor's own redraw cadence uses. Reporting the
    /// fastest panel's period instead would advertise a cadence the
    /// compositor cannot honour on a mixed-refresh desktop, so a client would
    /// over-commit and drop frames by construction (Optics ADR-0122,
    /// tessera ADR-0196, `[INV-ARCH-85]`).
    ///
    /// 0 is the protocol's "unknown", the honest answer for a nested backend
    /// that owns no vblank.
    fn presentation_refresh_ns(&self) -> u32 {
        tessera_desktop::output::presentation_domain_period_ns(&self.state.physical_outputs)
            .map_or(0, |period| period.min(u64::from(u32::MAX)) as u32)
    }

    /// Complete every `wp_presentation_feedback` queued on a surface whose frame
    /// just reached the display (ADR-0122). Feedback for surfaces the presented
    /// frame did not sample stays queued until it is presented or the surface is
    /// destroyed: a client must never be told about a frame it did not see.
    ///
    /// [`PresentedSurfaces::Nothing`] completes nothing. A cursor-plane-only
    /// commit or a no-damage cycle drives no frame that samples client
    /// content, so reporting `presented` there would fabricate a presentation
    /// timestamp for a frame that never existed (`[INV-ARCH-84]`).
    fn complete_presentation_feedback(
        &mut self,
        visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
        session_locked: bool,
        presented: PresentedSurfaces<'_>,
    ) -> usize {
        if matches!(presented, PresentedSurfaces::Nothing) {
            return 0;
        }
        if !extensions::presentation_feedback_pending(&self.state) {
            return 0;
        }
        // One presentation event covers the whole presented frame, so every
        // feedback completed here shares one timestamp and one sequence number.
        let now_ns = crate::monotonic_ns();
        let refresh_ns = self.presentation_refresh_ns();
        let seq = self.state.presentation_seq;
        let transition_now_ms = self.now_ms();
        let sampled = presented.enumerated();
        let mut completed = 0usize;
        for &p in &self.state.surfaces {
            if p.is_null() {
                continue;
            }
            let rec = unsafe { &mut *p };
            if rec.presentation_feedback.is_empty() {
                continue;
            }
            let output_visible = unsafe {
                surface_receives_output_frame_callback(
                    p,
                    visible,
                    occluded,
                    session_locked,
                    transition_now_ms,
                )
            };
            if !output_visible {
                continue;
            }
            if let Some((ids, drew_preview_sources)) = sampled
                && !drew_preview_sources
                && !ids.contains(&(rec.resource as usize))
            {
                // This frame reached the display but did not sample this
                // surface, so its first content frame is still ahead of it.
                continue;
            }
            let pending = std::mem::take(&mut rec.presentation_feedback);
            for fb in pending {
                if fb.is_null() {
                    continue;
                }
                unsafe {
                    ffi::wl_resource_set_implementation(
                        fb,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        None,
                    );
                    post_presentation_presented(fb, now_ns, refresh_ns, seq);
                    ffi::wl_resource_destroy(fb);
                }
                completed += 1;
            }
        }
        if completed > 0 {
            self.state.presentation_seq = seq.wrapping_add(1);
            unsafe { ffi::wl_display_flush_clients(self.state.display) };
        }
        completed
    }

    /// Whether any client is waiting for an output-paced frame callback.
    pub fn frame_callbacks_pending(&self) -> bool {
        self.state
            .surfaces
            .iter()
            .copied()
            .any(|p| !p.is_null() && unsafe { !(*p).frame_callbacks.is_empty() })
    }

    /// Fire pending frame callbacks for surfaces that are actually visible on
    /// the physical output. Covered, minimized, off-workspace and unmapped
    /// surfaces receive only a low-rate compatibility heartbeat.
    ///
    /// This distinction is essential for an occlusion-aware compositor: if a
    /// covered browser keeps receiving callbacks at 120 Hz, a playing video
    /// continues producing buffers and waking the compositor even though none
    /// of those pixels can reach the output. `time_ms` is a millisecond
    /// timestamp from a monotonic clock.
    pub fn send_frame_callbacks(
        &mut self,
        time_ms: u32,
        presented: PresentedSurfaces<'_>,
    ) -> usize {
        let visible = self.visible();
        let occluded = self.occluded_window_ids();
        let session_locked = self.state.session_lock_phase.is_active();
        // Presentation feedback shares the frame-callback completion point: both
        // report the frame that just reached the display (ADR-0122).
        self.complete_presentation_feedback(&visible, &occluded, session_locked, presented);
        let transition_now_ms = self.now_ms();
        let background_due =
            background_frame_callback_due(time_ms, self.state.last_background_frame_callback_ms);
        let mut sent = 0;
        let mut sent_background = false;
        for &p in &self.state.surfaces {
            if p.is_null() {
                continue;
            }
            let rec = unsafe { &mut *p };
            if !rec.mapped {
                continue;
            }
            let physically_visible = unsafe {
                surface_receives_output_frame_callback(
                    p,
                    &visible,
                    &occluded,
                    session_locked,
                    transition_now_ms,
                )
            };
            if !physically_visible && !background_due {
                continue;
            }
            let before = sent;
            let pending_cb = std::mem::take(&mut rec.frame_callbacks);
            for cb in pending_cb {
                unsafe {
                    ffi::wl_resource_set_implementation(
                        cb,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        None,
                    );
                    ffi::wl_resource_post_event(cb, ffi::WL_CALLBACK_DONE, time_ms);
                    ffi::wl_resource_destroy(cb);
                }
                sent += 1;
            }
            sent_background |= !physically_visible && sent != before;
        }
        if sent_background {
            self.state.last_background_frame_callback_ms = time_ms;
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
        sent
    }

    /// The last submitted frame reached the presentation backend, so the
    /// accumulated damage of every surface the frame actually sampled is now
    /// represented by the renderer's retained texture and the scanout
    /// baseline, and its latched dma-buf `wl_buffer` is now owned by the
    /// active presentation epoch.
    ///
    /// `sampled` yields the surface resource ids the frame drew or scanned
    /// out, and `drew_preview_sources` reports whether the frame pulled
    /// additional surfaces outside that set (overview thumbnails, the window
    /// switcher's live cards, or the workspace rail's live miniatures).
    ///
    /// Marking a surface the frame never sampled (hidden workspace, fully
    /// occluded, minimized) would wrongly defer its `wl_buffer` to the next
    /// vblank (`[INV-ARCH-70]`), throttling a client whose commits never reach
    /// the output. Marking one the frame *did* sample is mandatory: the
    /// compositor only duplicated the client's dma-buf, so the client must not
    /// reuse that memory until the frame retires.
    ///
    /// When `drew_preview_sources` is set, or `sampled` is empty because the
    /// client damage pass was skipped, the frame owns buffers the caller cannot
    /// enumerate; the conservative fallback then marks every surface presented.
    /// Deferring a buffer one extra frame is always safe, whereas releasing a
    /// buffer the frame is reading or KMS is scanning out is not.
    pub fn acknowledge_presented_surface_damage<'a>(
        &mut self,
        sampled: impl IntoIterator<Item = &'a usize>,
        drew_preview_sources: bool,
    ) {
        if drew_preview_sources {
            self.acknowledge_all_presented_surfaces();
            return;
        }
        let mut marked_any = false;
        for id in sampled {
            let resource = *id as *mut ffi::wl_resource;
            if resource.is_null() {
                continue;
            }
            let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
            if rec.is_null() {
                continue;
            }
            marked_any = true;
            let rec = unsafe { &mut *rec };
            Self::acknowledge_surface_presented(rec);
        }
        if !marked_any {
            self.acknowledge_all_presented_surfaces();
        }
    }

    fn acknowledge_all_presented_surfaces(&mut self) {
        for &p in &self.state.surfaces {
            if p.is_null() {
                continue;
            }
            Self::acknowledge_surface_presented(unsafe { &mut *p });
        }
    }

    fn acknowledge_surface_presented(rec: &mut SurfaceRec) {
        rec.committed_damage.clear();
        rec.committed_damage_full = false;
        if !rec.current_buffer.is_null() || !rec.current_explicit_release.is_null() {
            rec.current_buffer_presented = true;
        }
    }

    pub fn retired_buffers_pending(&self) -> bool {
        !self.state.retired_buffer_releases.is_empty()
    }

    /// Release replaced dma-bufs after the renderer has submitted a frame
    /// that no longer references them. `completion_fence` is a borrowed Linux
    /// sync_file fd for that submission; `None` means completion was already
    /// waited on by the backend.
    pub fn release_retired_buffers(&mut self, completion_fence: Option<i32>) {
        let retired = std::mem::take(&mut self.state.retired_buffer_releases);
        let mut retry = Vec::new();
        for release in retired {
            let explicit_fd = if release.explicit_release.is_null() {
                None
            } else if let Some(fence) = completion_fence {
                let fd = unsafe { dup(fence) };
                if fd < 0 {
                    retry.push(release);
                    continue;
                }
                Some(fd)
            } else {
                None
            };
            unsafe {
                if !release.buffer.is_null() {
                    ffi::wl_resource_post_event(release.buffer, ffi::WL_BUFFER_RELEASE);
                }
                if !release.explicit_release.is_null() {
                    if let Some(fd) = explicit_fd {
                        ffi::wl_resource_post_event(
                            release.explicit_release,
                            ffi::ZWP_LINUX_BUFFER_RELEASE_V1_FENCED_RELEASE,
                            fd,
                        );
                    } else {
                        ffi::wl_resource_post_event(
                            release.explicit_release,
                            ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
                        );
                    }
                    ffi::wl_resource_destroy(release.explicit_release);
                }
            }
        }
        self.state.retired_buffer_releases.extend(retry);
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }
}

/// Post `wp_presentation_feedback.presented` for one completed frame.
///
/// The timestamp is nanoseconds on `CLOCK_MONOTONIC` — the clock the
/// presentation global announces — so a client derives presentation latency by
/// subtracting its own reading. `refresh_ns` is the output period (0 =
/// unknown), and `seq` is the monotonic presented-frame counter. `flags` is 0:
/// the compositor makes no versus-clock, hardware-clock, or zero-copy claim it
/// cannot substantiate.
unsafe fn post_presentation_presented(
    fb: *mut ffi::wl_resource,
    now_ns: u64,
    refresh_ns: u32,
    seq: u64,
) {
    let [
        tv_sec_hi,
        tv_sec_lo,
        tv_nsec,
        refresh,
        seq_lo,
        seq_hi,
        flags,
    ] = presented_fields(now_ns, refresh_ns, seq);
    unsafe {
        ffi::wl_resource_post_event(
            fb,
            ffi::WP_PRESENTATION_FEEDBACK_PRESENTED,
            tv_sec_hi,
            tv_sec_lo,
            tv_nsec,
            refresh,
            seq_lo,
            seq_hi,
            flags,
        );
    }
}

/// Wire fields for `wp_presentation_feedback.presented`: the 64-bit timestamp
/// and sequence are each split into a high and low `uint`, and `flags` is 0
/// because the compositor claims no versus-clock, hardware-clock, or zero-copy
/// guarantee it cannot substantiate.
fn presented_fields(now_ns: u64, refresh_ns: u32, seq: u64) -> [u32; 7] {
    let secs = now_ns / 1_000_000_000;
    let nsec = (now_ns % 1_000_000_000) as u32;
    [
        (secs >> 32) as u32,
        (secs & 0xffff_ffff) as u32,
        nsec,
        refresh_ns,
        (seq & 0xffff_ffff) as u32,
        (seq >> 32) as u32,
        0,
    ]
}

/// Whether a surface contributes pixels to the physical output whose vblank
/// paces this callback. The root toplevel owns workspace/minimize/occlusion
/// visibility for its complete popup/subsurface tree. Protocol-owned overlay
/// roles without a toplevel root are visible only while mapped. During a
/// session lock, only the lock surface is output-visible.
unsafe fn surface_receives_output_frame_callback(
    surface: *mut SurfaceRec,
    visible: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    occluded: &std::collections::HashSet<tessera_desktop::window::WindowId>,
    session_locked: bool,
    transition_now_ms: u64,
) -> bool {
    // SAFETY: callers iterate `State::surfaces`, whose non-null entries remain
    // live for the duration of this single-threaded server operation.
    let rec = unsafe { &*surface };
    if !rec.mapped {
        return false;
    }
    if !rec.session_lock_surface.is_null() {
        return session_locked;
    }
    if session_locked {
        return false;
    }
    // SAFETY: the live surface tree owns every parent/root pointer until its
    // destroy handler detaches children.
    let root = unsafe { surface_root_toplevel(surface) };
    if !root.is_null() {
        // SAFETY: `root` belongs to the same live surface tree as `surface`.
        let root = unsafe { &*root };
        return root.mapped
            && (!root.window.minimized
                || root
                    .window
                    .transition
                    .is_some_and(|transition| transition.is_active_at(transition_now_ms)))
            && visible.contains(&root.window.id)
            && !occluded.contains(&root.window.id);
    }
    rec.cursor_role
        || rec.drag_icon_role
        || rec.input_popup_role
        || !rec.input_popup_surface.is_null()
}

/// Flatten one xdg-role surface and its wl_subsurface descendants into Wayland
/// paint order. The server owns the tree; the renderer only receives resource
/// ids and frame payloads.
unsafe fn append_surface_tree_frame_order(
    surface: *mut SurfaceRec,
    order: &mut Vec<usize>,
    depth: u32,
) {
    unsafe {
        if surface.is_null() || !(*surface).mapped || depth >= 32 {
            return;
        }
        for &child in &(*surface).children {
            if !child.is_null() && !(*child).subsurface_above_parent {
                append_surface_tree_frame_order(child, order, depth + 1);
            }
        }
        order.push((*surface).resource as usize);
        for &child in &(*surface).children {
            if !child.is_null() && (*child).subsurface_above_parent {
                append_surface_tree_frame_order(child, order, depth + 1);
            }
        }
    }
}

fn cursor_surface_position(
    pointer: (f32, f32),
    hotspot: tessera_primitives::Point,
    attach_offset: tessera_primitives::Point,
) -> tessera_primitives::Point {
    tessera_primitives::Point {
        x: pointer.0.round() as i32 - hotspot.x + attach_offset.x,
        y: pointer.1.round() as i32 - hotspot.y + attach_offset.y,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACKGROUND_FRAME_CALLBACK_INTERVAL_MS, PresentedSurfaces, background_frame_callback_due,
        cursor_surface_position, presented_fields, surface_receives_output_frame_callback,
    };
    use tessera_desktop::transition::WindowTransition;
    use tessera_desktop::window::WindowId;
    use tessera_primitives::Point;

    /// The presented event's two 64-bit values are split into high/low `uint`s
    /// in wire order, the nanosecond remainder is preserved, and no capability
    /// flag is claimed.
    #[test]
    fn presented_event_splits_timestamp_and_sequence() {
        let now_ns = (0x1_0000_0002u64) * 1_000_000_000 + 123_456_789;
        let seq = 0x3_0000_0004u64;
        assert_eq!(
            presented_fields(now_ns, 16_666_666, seq),
            [1, 2, 123_456_789, 16_666_666, 4, 3, 0],
        );
        // The unknown-refresh and zero-sequence case stays a well-formed event.
        assert_eq!(presented_fields(0, 0, 0), [0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn cursor_surface_override_preserves_trigger_position_and_hotspot() {
        assert_eq!(
            cursor_surface_position((120.6, 79.4), Point { x: 7, y: 11 }, Point { x: 2, y: -3 },),
            Point { x: 116, y: 65 },
        );
    }

    #[test]
    fn hidden_frame_callback_heartbeat_is_rate_limited_and_wrap_safe() {
        assert!(!background_frame_callback_due(
            BACKGROUND_FRAME_CALLBACK_INTERVAL_MS - 1,
            0,
        ));
        assert!(background_frame_callback_due(
            BACKGROUND_FRAME_CALLBACK_INTERVAL_MS,
            0,
        ));
        let previous = u32::MAX - 200;
        assert!(!background_frame_callback_due(700, previous));
        assert!(background_frame_callback_due(800, previous));
    }

    #[test]
    fn nothing_presented_enumerates_no_surface() {
        // [INV-ARCH-84]: a cursor-only commit or a no-damage cycle drives no
        // frame that samples client content, so it must complete no
        // `wp_presentation_feedback` at all. `enumerated() == None` is what
        // makes `complete_presentation_feedback` skip every surface.
        assert!(PresentedSurfaces::Nothing.enumerated().is_none());
    }

    #[test]
    fn sampled_presentation_completes_only_the_sampled_surfaces() {
        // Two surfaces, a resource id base and a sampled set naming only the
        // first. The predicate must complete the first and leave the second
        // queued: reporting `presented` for a surface the frame never sampled
        // would fabricate a presentation timestamp.
        let base = std::ptr::dangling_mut::<crate::ffi::wl_resource>() as usize;
        let first = base;
        let second = base + 0x40;
        let ids = [first];

        let sampled = PresentedSurfaces::Sampled {
            ids: &ids,
            drew_preview_sources: false,
        };
        let (enumerated, preview_sources) = sampled.enumerated().expect("sampled is enumerated");
        assert!(!preview_sources);
        assert!(enumerated.contains(&first), "the sampled surface completes");
        assert!(
            !enumerated.contains(&second),
            "an unsampled surface must stay queued, not be reported presented"
        );
    }

    #[test]
    fn preview_sources_force_the_conservative_all_surfaces_fallback() {
        // A frame that drew overview thumbnails or switcher cards sampled
        // surfaces the caller cannot enumerate, so every visible surface
        // completes rather than stranding the ones outside the id list.
        let ids = [0usize];
        let sampled = PresentedSurfaces::Sampled {
            ids: &ids,
            drew_preview_sources: true,
        };
        let (_, preview_sources) = sampled.enumerated().expect("sampled is enumerated");
        assert!(
            preview_sources,
            "drew_preview_sources must disable the id filter"
        );
        // `All` reports the same conservative answer without naming ids.
        let (all_ids, all_preview) = PresentedSurfaces::All
            .enumerated()
            .expect("All is enumerated");
        assert!(all_preview && all_ids.is_empty());
    }

    #[test]
    fn minimized_surface_is_callback_visible_only_during_an_active_transition() {
        let mut surface = Box::new(crate::SurfaceRec::new(std::ptr::dangling_mut::<
            crate::ffi::wl_resource,
        >()));
        surface.mapped = true;
        surface.xdg_toplevel = surface.resource;
        surface.window.id = WindowId(7);
        surface.window.minimized = true;
        let visible = std::collections::HashSet::from([surface.window.id]);
        let occluded = std::collections::HashSet::new();

        surface.window.transition = Some(WindowTransition {
            from: tessera_primitives::Rect::new(0, 0, 100, 100),
            started_ms: 10,
            duration_ms: 100,
            easing: tessera_desktop::transition::Easing::EaseOutCubic,
            effect: None,
        });
        assert!(unsafe {
            surface_receives_output_frame_callback(surface.as_mut(), &visible, &occluded, false, 50)
        });
        assert!(!unsafe {
            surface_receives_output_frame_callback(
                surface.as_mut(),
                &visible,
                &occluded,
                false,
                110,
            )
        });
    }
}
