use super::*;

// Backdrop effect-cache region algebra and the slot-fingerprint policy live
// in `tessera-presentation` (api tier); the composition root keeps the
// chrome-typed mappings, the DAG planner (core types), and every GPU-owned
// executor.
/// Convert output damage to the physical-pixel damage scope a Canvas pass
/// accepts. Re-exported from the api value layer.
pub(super) use tessera_composite::presentation::frame_damage_scope;

/// Begin a stencil-capable overlay pass over the currently presented content.
///
/// Production compositing merges the desktop scene and shell chrome into a
/// single Canvas pass (`skip_stencil: !draw_chrome`); this helper is retained
/// for pass-boundary unit tests.
#[cfg(test)]
pub(super) fn begin_stencil_frame_overlay<'c, 'surface>(
    canvas: &'c mut flux::Canvas,
    frame: &'c mut flux::Frame<'surface>,
    damage: Vec<flux::CanvasRenderArea>,
) -> Result<flux::CanvasSession<'c, &'c mut flux::Frame<'surface>>, flux::Error> {
    canvas.begin_session(
        frame,
        flux::CanvasPassOptions {
            clear: None,
            antialias: flux::CanvasAntialias::None,
            damage,
            skip_stencil: false,
        },
    )
}

/// Begin a compositor canvas pass without Flux's clear-triggered 4x MSAA —
/// a canvas pass-configuration fact owned by `tessera-presentation`.
pub(super) use tessera_composite::presentation::begin_opaque_frame;

/// Begin an opaque output pass clipped to the accumulated damage for this
/// ring slot. The full scene may still be submitted, but rasterization,
/// texture sampling, blending and framebuffer writes stay inside the scissor.
#[allow(dead_code)]
pub(super) fn begin_opaque_frame_repaint<'c, 'surface>(
    canvas: &'c mut flux::Canvas,
    frame: &'c mut flux::Frame<'surface>,
    size: (u32, u32),
    clear: u32,
    repaint: &FrameDamage,
) -> Result<flux::CanvasSession<'c, &'c mut flux::Frame<'surface>>, flux::Error> {
    debug_assert_eq!(clear >> 24, 0xff, "compositor pass clear must be opaque");
    let damage = frame_damage_scope(repaint, size);
    canvas.begin_session(
        frame,
        flux::CanvasPassOptions {
            clear: Some(clear),
            antialias: flux::CanvasAntialias::None,
            damage,
            skip_stencil: true,
        },
    )
}

/// One slot's frozen trigger-frame snapshot.
struct ScreenshotCapture {
    image: flux::Image,
    size: (u32, u32),
    format: flux::Format,
}

/// Frozen desktop snapshot shown while the screenshot selector is open.
///
/// On the trigger frame the whole frame — desktop scene *and* chrome — is
/// rendered into a full-resolution offscreen image; every later frame
/// samples that image and renders only the selector on top, so the screen
/// keeps showing exactly the trigger frame until the user confirms or
/// cancels. Images are indexed by frame slot for the same in-flight reason
/// as [`BackdropGraphExecutor`]: a slot is rewritten only after `begin_frame`
/// has waited its fence.
///
/// Session flow: `ScreenshotFreeze::request_open` arms a session and
/// defers the selector opening; the capture frame renders the normal frame
/// into the target (the selector is not open yet, so its scrim stays out
/// of the snapshot), then the compositor opens the selector over the
/// frozen image. `ScreenshotFreeze::should_disarm` ends the session
/// once the selector closes.
/// Cursor state sampled at a capture trigger instant — a pure value owned
/// by `tessera-presentation` (shared by one-shot captures and streams).
pub(super) use tessera_composite::presentation::CaptureCursorState;

pub(super) struct ScreenshotFreeze {
    captures: Vec<Option<ScreenshotCapture>>,
    active_slot: Option<usize>,
    /// Logical cursor state sampled synchronously with the trigger, before a
    /// later input batch or capture frame can move it.
    trigger_cursor: Option<CaptureCursorState>,
    /// A freeze session is in progress (requested, capturing, or frozen).
    pub(super) armed: bool,
    /// The snapshot holds the trigger frame.
    pub(super) captured: bool,
    /// Capture failed; the session degrades to the live scene.
    pub(super) failed: bool,
    /// The selector opens once the capture frame has been rendered.
    pub(super) pending_open: bool,
    /// The selector was opened under this session.
    pub(super) opened: bool,
}

impl ScreenshotFreeze {
    pub(super) fn new() -> Self {
        Self {
            captures: Vec::new(),
            active_slot: None,
            trigger_cursor: None,
            armed: false,
            captured: false,
            failed: false,
            pending_open: false,
            opened: false,
        }
    }

    /// Arm a freeze session; the selector opens after the capture frame.
    pub(super) fn request_open(&mut self, cursor: Option<CaptureCursorState>) {
        if self.armed {
            return;
        }
        self.armed = true;
        self.captured = false;
        self.failed = false;
        self.pending_open = true;
        self.opened = false;
        self.active_slot = None;
        self.trigger_cursor = cursor;
    }

    /// Whether this frame must render the scene into the snapshot target.
    pub(super) fn needs_capture(&self) -> bool {
        self.armed && !self.captured && !self.failed
    }

    /// Whether the frozen snapshot replaces the live scene this frame.
    pub(super) fn active(&self) -> bool {
        self.armed && self.captured && !self.failed
    }

    pub(super) fn mark_captured(&mut self, slot: usize) {
        self.active_slot = Some(slot);
        self.captured = true;
    }

    pub(super) fn trigger_cursor(&self) -> Option<CaptureCursorState> {
        self.trigger_cursor
    }

    pub(super) fn mark_opened(&mut self) {
        self.pending_open = false;
        self.opened = true;
    }

    /// The selector closed (confirmed or cancelled); the frame that closed
    /// it still presents the snapshot, so live rendering resumes next frame.
    pub(super) fn should_disarm(&self, selector_active: bool) -> bool {
        self.armed && self.opened && !selector_active
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
        self.captured = false;
        self.failed = false;
        self.pending_open = false;
        self.opened = false;
        self.active_slot = None;
        self.trigger_cursor = None;
        self.captures.clear();
    }

    /// The snapshot image, once the trigger frame has been captured.
    pub(super) fn image(&self) -> Option<&flux::Image> {
        if !self.captured {
            return None;
        }
        self.captures
            .get(self.active_slot?)?
            .as_ref()
            .map(|capture| &capture.image)
    }

    /// This frame's snapshot target, after [`ensure_target`](Self::ensure_target)
    /// succeeded.
    #[allow(dead_code)]
    pub(super) fn target(&self, slot: usize) -> Option<&flux::Image> {
        self.captures
            .get(slot)?
            .as_ref()
            .map(|capture| &capture.image)
    }

    pub(super) fn target_mut(&mut self, slot: usize) -> Option<&mut flux::Image> {
        self.captures
            .get_mut(slot)?
            .as_mut()
            .map(|capture| &mut capture.image)
    }

    /// Allocate (or reuse) this frame slot's full-resolution snapshot target.
    pub(super) fn ensure_target(
        &mut self,
        device: &flux::Device,
        surface: &flux::Surface,
        slot: usize,
        surface_size: (u32, u32),
    ) -> bool {
        let format = match surface.format() {
            flux::Format::Rgba8Unorm | flux::Format::Bgra8Unorm => flux::Format::Rgba8Unorm,
            other => {
                log::warn!(
                    "screenshot: frame freeze unavailable for surface format {other:?}; falling back to live scene"
                );
                return false;
            }
        };
        if surface_size.0 == 0 || surface_size.1 == 0 {
            return false;
        }

        if self.captures.len() <= slot {
            self.captures.resize_with(slot + 1, || None);
        }
        let stale = self.captures[slot]
            .as_ref()
            .is_none_or(|capture| capture.size != surface_size || capture.format != format);
        if stale {
            match flux::Image::render_target(device, surface_size.0, surface_size.1, format) {
                Ok(image) => {
                    self.captures[slot] = Some(ScreenshotCapture {
                        image,
                        size: surface_size,
                        format,
                    });
                }
                Err(error) => {
                    log::warn!(
                        "screenshot: failed to allocate freeze target ({error}); falling back to live scene"
                    );
                    return false;
                }
            }
        }
        true
    }
}

pub(super) fn draw_wallpaper_background(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    wallpaper: &mut Option<wallpaper::Wallpaper>,
    logical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    if let Some(wallpaper) = wallpaper.as_mut() {
        wallpaper.draw(device, canvas, logical_size.0 as f32, logical_size.1 as f32);
    }
    canvas.restore();
}

pub(super) fn draw_client_scene(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    scale: f32,
    retain_preview_sources: bool,
    soft_shadows: Option<&tessera_composite::SoftShadowLayer<'_>>,
    shadow_style: tessera_desktop::window::WindowShadowStyle,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let (shm, dmabuf, surface_order) = if retain_preview_sources {
        (
            server.client_preview_surface_frames(),
            server.client_preview_surface_dmabuf_frames(),
            server.client_preview_surface_frame_order(),
        )
    } else {
        // One shared visibility/occlusion pass for both frame lists; the
        // damage assessment and scanout planner used to each recompute the
        // same O(windows × surfaces) walk per presented frame.
        let sets = server.desktop_frame_sets();
        let order = server.client_surface_frame_order_with(&sets.visible, &sets.occluded);
        (sets.shm, sets.dmabuf, order)
    };
    let windows = server.render_windows();
    renderer.gc(server.live_surface_ids());
    if let Some(slide) = server.workspace_slide_presentation() {
        let output = slide.output;
        let animated_windows = slide
            .layers
            .iter()
            .flat_map(|layer| layer.windows.iter().copied())
            .collect::<std::collections::HashSet<_>>();
        let static_windows = shm
            .iter()
            .filter_map(|frame| frame.window)
            .chain(dmabuf.iter().filter_map(|frame| frame.window))
            .filter(|window| !animated_windows.contains(window))
            .collect::<std::collections::HashSet<_>>();
        if !static_windows.is_empty() {
            renderer.draw_workspace_surface_layer(
                device,
                canvas,
                &surface_order,
                &shm,
                &dmabuf,
                tessera_composite::WorkspaceSurfaceLayer::new(&windows, &static_windows),
            );
        }
        for layer in slide.layers {
            let window_filter = layer
                .windows
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            canvas.save();
            // The page clip moves with the page, while the output framebuffer
            // supplies the final viewport clip. Adjacent pages therefore meet
            // at one exact edge and can never paint over one another.
            canvas.clip_rect(
                output.origin.x as f32 + layer.offset_x,
                output.origin.y as f32,
                output.size.w as f32,
                output.size.h as f32,
            );
            canvas.translate(layer.offset_x, 0.0);
            renderer.draw_workspace_surface_layer(
                device,
                canvas,
                &surface_order,
                &shm,
                &dmabuf,
                tessera_composite::WorkspaceSurfaceLayer::new(&windows, &window_filter),
            );
            canvas.restore();
        }
    } else {
        renderer.draw_surfaces_ordered_with_window_shadows(
            device,
            canvas,
            &surface_order,
            &shm,
            &dmabuf,
            &windows,
            soft_shadows,
            shadow_style,
        );
    }
    // ADR-0029 close transitions: fading ghosts of just-closed windows paint
    // above the live desktop (they always unmap topmost relative to their
    // own tree; ghosts cannot occlude anything that is still interactive
    // because nothing below them changes while they fade).
    let ghosts = server.closing_frame_views();
    if !ghosts.is_empty() {
        renderer.draw_closing_frames(device, canvas, &ghosts);
    }
    canvas.restore();
}

/// Input-method popups, drag icons, and client cursor surfaces are protocol
/// overlays, not ordinary client content. Draw them after compositor chrome
/// so a Dock or status bar cannot cover the candidate panel or cursor.
pub(super) fn draw_client_overlays(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    scale: f32,
    include_cursor: bool,
    cursor_position: Option<(f32, f32)>,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let overlay_shm = server.overlay_frames_with_cursor_at(include_cursor, cursor_position);
    let overlay_dmabuf =
        server.overlay_dmabuf_frames_with_cursor_at(include_cursor, cursor_position);
    renderer.draw_toplevels(device, canvas, &overlay_shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &overlay_dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Fail-closed session-lock composition. The opaque physical-pixel fill is
/// emitted after all normal desktop/chrome work, then only surfaces owned by
/// the active lock client (plus its cursor) are allowed above it.
pub(super) fn draw_lock_scene(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    physical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        physical_size.0 as f32,
        physical_size.1 as f32,
        flux::rgba(0, 0, 0, 255),
    );
    canvas.restore();

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.lock_frames();
    let dmabuf = server.lock_dmabuf_frames();
    renderer.gc(server.live_surface_ids());
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Overview scene (M9): the desktop dimmed, then every visible window drawn
/// as a live thumbnail on the shared `tessera_shell::layout::overview` grid — the exact
/// geometry the overview chrome uses for its frames, labels, and hit-testing.
/// Z-order is preserved bottom-to-top so overlapping thumbnails read like
/// the desktop stack. `progress` (0..1) is the chrome's reveal animation:
/// thumbnails interpolate from each window's real geometry into its grid
/// cell (and the scrim fades in) instead of popping onto the grid.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_overview_scene(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    logical_size: (u32, u32),
    scale: f32,
    scheme: tessera_desktop::settings::ColorScheme,
    progress: f32,
) {
    let t = progress.clamp(0.0, 1.0);
    let scrim_alpha = (200.0 * t).round() as u8;
    let (scrim_r, scrim_g, scrim_b) = overview_scrim(scheme);
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(scrim_r, scrim_g, scrim_b, scrim_alpha),
    );
    canvas.restore();

    let windows = server.windows();
    let snapshot = server.workspace_snapshot();
    let rail = snapshot
        .outputs
        .first()
        .map(|o| o.workspaces.len() > 1)
        .unwrap_or(false);
    let interaction_domain_shelf = server
        .interaction_domain_snapshot()
        .interaction_domains
        .iter()
        .any(|interaction_domain| {
            interaction_domain.kind
                == tessera_authority::interaction_domain::InteractionDomainKind::Agent
                && interaction_domain.state
                    != tessera_authority::interaction_domain::InteractionDomainState::Revoked
        });
    let display = tessera_primitives::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);

    // Workspace rail miniatures along the top edge: every workspace on the
    // output drawn live into its tile, independent of the main grid — an
    // empty current workspace must not starve the rail.
    if rail {
        draw_workspace_rail_tiles(
            canvas, device, renderer, server, &snapshot, display, scale, t,
        );
    }

    if windows.is_empty() {
        return;
    }
    let area = tessera_shell::layout::overview::grid_area_with_interaction_domain_shelf(
        display,
        rail,
        interaction_domain_shelf,
    );
    let window_rects: Vec<(tessera_desktop::window::WindowId, tessera_primitives::Rect)> = windows
        .iter()
        .map(|w| {
            (
                w.id,
                tessera_primitives::Rect {
                    origin: w.position,
                    size: w.size,
                },
            )
        })
        .collect();
    // Closest-slot assignment pairs each window with the slot nearest its
    // real position, in input order; the chrome's hit-testing pairs the same
    // list the same way, so cells agree.
    let slots: Vec<tessera_primitives::Rect> =
        tessera_shell::layout::overview::assign_slots(area, &window_rects)
            .into_iter()
            .map(|(_, slot)| slot)
            .collect();
    let cells: std::collections::HashMap<
        tessera_desktop::window::WindowId,
        (
            tessera_primitives::Rect,
            tessera_primitives::Point,
            tessera_primitives::Size,
        ),
    > = windows
        .iter()
        .zip(slots.iter())
        .map(|(w, slot)| {
            // Fly-in: the cell starts at the window's real geometry and
            // lands on its aspect-fitted grid slot as `t` reaches 1.
            let cell = tessera_shell::layout::overview::animated_cell(
                *slot,
                tessera_primitives::Rect {
                    origin: w.position,
                    size: w.size,
                },
                t,
            );
            (w.id, (cell, w.position, w.size))
        })
        .collect();
    let map = move |window: Option<tessera_desktop::window::WindowId>,
                    natural: tessera_primitives::Rect| {
        let Some((cell, base, win_size)) = window.and_then(|id| cells.get(&id)) else {
            return natural;
        };
        let k = cell.size.w as f32 / win_size.w.max(1) as f32;
        let remap = |v: i32, b: i32| (v - b) as f32 * k;
        tessera_primitives::Rect::new(
            cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
            cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
            (natural.size.w as f32 * k).round().max(1.0) as i32,
            (natural.size.h as f32 * k).round().max(1.0) as i32,
        )
    };

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.client_preview_surface_frames();
    let dmabuf = server.client_preview_surface_dmabuf_frames();
    let surface_order = server.client_preview_surface_frame_order();
    renderer.draw_surfaces_ordered_mapped(device, canvas, &surface_order, &shm, &dmabuf, &map);
    canvas.restore();
}

/// Workspace rail miniatures: each workspace of the output drawn live into
/// its top-rail tile using the shared `tessera_shell::layout::overview` geometry, so
/// the chrome's tile frames, captions, and hit-testing line up. A tile is
/// clipped to its rounded rect so miniature surface trees never spill into
/// neighbouring tiles.
#[allow(clippy::too_many_arguments)]
fn draw_workspace_rail_tiles(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    snapshot: &tessera_desktop::workspace::WorkspaceSnapshot,
    display: tessera_primitives::Rect,
    scale: f32,
    progress: f32,
) {
    let Some(output) = snapshot.outputs.first() else {
        return;
    };
    let tiles = tessera_shell::layout::overview::rail(display, output.workspaces.len());
    let all = server.all_windows();
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    for (entry, tile) in output.workspaces.iter().zip(tiles.iter()) {
        let tile_windows: Vec<&tessera_desktop::window::Window> = entry
            .toplevels
            .iter()
            .filter_map(|id| all.iter().find(|w| w.id == *id))
            .collect();
        if tile_windows.is_empty() {
            continue;
        }
        let content = tessera_shell::layout::overview::tile_content(*tile);
        let slots = tessera_shell::layout::overview::grid_with_spacing(
            content,
            tile_windows.len(),
            tessera_shell::layout::overview::TILE_GRID_MARGIN,
            tessera_shell::layout::overview::TILE_GRID_GAP,
        );
        let cells: std::collections::HashMap<
            tessera_desktop::window::WindowId,
            (
                tessera_primitives::Rect,
                tessera_primitives::Point,
                tessera_primitives::Size,
            ),
        > = tile_windows
            .iter()
            .zip(slots.iter())
            .map(|(window, slot)| {
                (
                    window.id,
                    (
                        tessera_shell::layout::overview::fit(*slot, window.size),
                        window.position,
                        window.size,
                    ),
                )
            })
            .collect();
        let map = move |wid: Option<tessera_desktop::window::WindowId>,
                        natural: tessera_primitives::Rect| {
            let Some((cell, base, win_size)) = wid.and_then(|id| cells.get(&id)) else {
                return natural;
            };
            let k = cell.size.w as f32 / win_size.w.max(1) as f32;
            let remap = |v: i32, b: i32| (v - b) as f32 * k;
            tessera_primitives::Rect::new(
                cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
                cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
                (natural.size.w as f32 * k).round().max(1.0) as i32,
                (natural.size.h as f32 * k).round().max(1.0) as i32,
            )
        };
        let set: std::collections::HashSet<tessera_desktop::window::WindowId> =
            tile_windows.iter().map(|w| w.id).collect();
        let shm = server.client_preview_frames_for(&set);
        let dmabuf = server.client_preview_dmabuf_frames_for(&set);
        let order = server.client_preview_frame_order_for(&set);
        renderer.draw_surfaces_ordered_mapped_with_style(
            device,
            canvas,
            &order,
            &shm,
            &dmabuf,
            &map,
            tessera_composite::MappedSurfaceStyle {
                opacity: progress.clamp(0.0, 1.0),
                brightness: 1.0,
                rounded_clip: *tile,
                corner_radius: 10.0,
            },
        );
    }
    canvas.restore();
}

/// Super+Tab scene: preserve the live desktop underneath a dim scrim, then
/// paint every visible window again into the shared horizontal preview strip.
/// Shell chrome draws labels and the selected-card border over these targets.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_window_switcher_scrim(
    canvas: &flux::CanvasCommands<'_>,
    logical_size: (u32, u32),
    scale: f32,
    presentation: &tessera_shell::component::WindowSwitcherPresentation,
    scheme: tessera_desktop::settings::ColorScheme,
) {
    let scrim_alpha = (145.0 * presentation.visibility.clamp(0.0, 1.0)).round() as u8;
    let (scrim_r, scrim_g, scrim_b) = window_switcher_scrim(scheme);
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(scrim_r, scrim_g, scrim_b, scrim_alpha),
    );
    canvas.restore();
}

pub(super) fn draw_window_switcher_cards(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    scale: f32,
    presentation: &tessera_shell::component::WindowSwitcherPresentation,
) {
    let windows = server.windows();
    if windows.is_empty() || presentation.visibility <= 0.001 {
        return;
    }
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    canvas.clip_rect(
        presentation.panel.origin.x as f32,
        presentation.panel.origin.y as f32,
        presentation.panel.size.w as f32,
        presentation.panel.size.h as f32,
    );
    for card in &presentation.cards {
        let Some(window) = windows.iter().find(|window| window.id == card.window) else {
            continue;
        };
        let target_set: std::collections::HashSet<_> = [card.window].into_iter().collect();
        let shm = server.client_preview_frames_for(&target_set);
        let dmabuf = server.client_preview_dmabuf_frames_for(&target_set);
        let surface_order = server.client_preview_frame_order_for(&target_set);
        let brightness = tessera_shell::component::preview::content_brightness(
            presentation.selected,
            card.window,
            presentation.inactive_content_brightness,
        );
        draw_preview_card_scene(
            canvas,
            device,
            renderer,
            &surface_order,
            &shm,
            &dmabuf,
            window,
            card,
            presentation.visibility,
            brightness,
        );
    }
    canvas.restore();
}

/// Draw compositor-owned live-preview popovers contributed by ordinary shell
/// chrome. Each card gets its own clip and mapping pass so a window's
/// subsurfaces cannot bleed into a neighbouring card and unrelated windows
/// are never redrawn over the popover.
pub(super) fn draw_live_preview_scenes(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    server: &tessera_wayland::Server,
    scale: f32,
    presentations: &[tessera_shell::component::LivePreviewPresentation],
) {
    if presentations.is_empty() {
        return;
    }
    let windows = server.windows();

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    for presentation in presentations {
        if presentation.visibility <= 0.001 {
            continue;
        }
        for card in &presentation.cards {
            let Some(window) = windows.iter().find(|window| window.id == card.window) else {
                continue;
            };
            let target_set: std::collections::HashSet<_> = [card.window].into_iter().collect();
            let shm = server.client_preview_frames_for(&target_set);
            let dmabuf = server.client_preview_dmabuf_frames_for(&target_set);
            let surface_order = server.client_preview_frame_order_for(&target_set);
            let brightness = tessera_shell::component::preview::content_brightness(
                presentation.focused,
                card.window,
                presentation.inactive_content_brightness,
            );
            draw_preview_card_scene(
                canvas,
                device,
                renderer,
                &surface_order,
                &shm,
                &dmabuf,
                window,
                card,
                presentation.visibility,
                brightness,
            );
        }
    }
    canvas.restore();
}

#[allow(clippy::too_many_arguments)]
fn draw_preview_card_scene(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    renderer: &mut tessera_composite::Renderer,
    surface_order: &[usize],
    shm: &[tessera_primitives::SurfacePixels<'_>],
    dmabuf: &[tessera_primitives::SurfaceDmabuf],
    window: &tessera_desktop::window::Window,
    card: &tessera_shell::component::PreviewCard,
    opacity: f32,
    brightness: f32,
) {
    let cell = tessera_shell::layout::overview::fit(card.geometry.preview, window.size);
    let base = window.position;
    let window_size = window.size;
    let target = window.id;
    let map = move |id: Option<tessera_desktop::window::WindowId>,
                    natural: tessera_primitives::Rect| {
        if id != Some(target) {
            return tessera_primitives::Rect::new(-100_000, -100_000, 1, 1);
        }
        let factor = (cell.size.w as f32 / window_size.w.max(1) as f32)
            .min(cell.size.h as f32 / window_size.h.max(1) as f32);
        let remap = |value: i32, origin: i32| (value - origin) as f32 * factor;
        tessera_primitives::Rect::new(
            cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
            cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
            (natural.size.w as f32 * factor).round().max(1.0) as i32,
            (natural.size.h as f32 * factor).round().max(1.0) as i32,
        )
    };
    canvas.save();
    canvas.clip_rect(
        card.geometry.preview.origin.x as f32,
        card.geometry.preview.origin.y as f32,
        card.geometry.preview.size.w as f32,
        card.geometry.preview.size.h as f32,
    );
    renderer.draw_surfaces_ordered_mapped_with_style(
        device,
        canvas,
        surface_order,
        shm,
        dmabuf,
        &map,
        tessera_composite::MappedSurfaceStyle {
            opacity: opacity.clamp(0.0, 1.0),
            brightness: brightness.clamp(0.0, 1.0),
            rounded_clip: card.geometry.preview,
            corner_radius: card.corner_radius,
        },
    );
    canvas.restore();
}

/// Software cursor for direct KMS, sourced exclusively from the XDG cursor
/// theme (`$XCURSOR_THEME`/`$XCURSOR_SIZE`, inheritance included) via
/// [`cursor::CursorCache`].
/// Client-provided cursor surfaces are already composited by `tessera-server`;
/// this covers the cursor-shape protocol and compositor-owned cursors.
pub(super) fn draw_software_cursor(
    canvas: &flux::CanvasCommands<'_>,
    device: &flux::Device,
    cache: &mut cursor::CursorCache,
    position: (f32, f32),
    shape: u32,
    scale: f32,
) {
    if let Some(cursor) = cache.get(device, shape, scale) {
        canvas.save();
        canvas.scale(scale, scale);
        let inv = 1.0 / scale.max(0.25);
        canvas.draw_image(
            &cursor.image,
            position.0 - cursor.xhot * inv,
            position.1 - cursor.yhot * inv,
            cursor.width * inv,
            cursor.height * inv,
        );
        canvas.restore();
    }
}

#[cfg(test)]
mod tests;

/// Compositor-side blurred drop shadows for floating windows (ADR-0139):
/// the Optics `flux_shadow_filter` renders a rounded-rect mask through a
/// Gaussian blur; this wrapper owns the filter, renders each window's mask
/// into a per-slot capture target, and records the shadow passes at a pass
/// boundary. `tessera-composite` then composites the borrowed outputs beneath
/// each window tree.
///
/// Ownership mirrors `BackdropGraphExecutor`: one filter for the surface's frame
/// stream, per-slot mask images, no transient-pool leases, no device-wide
/// waits. The mask is re-rendered every frame a shadow is visible (the
/// windows move), so no cache invalidation tracking is needed beyond the
/// compositor's own damage (any geometry change already forces a full
/// repaint through the window signature).
#[derive(Clone, Copy, PartialEq, Eq)]
struct ShadowSlotKey {
    window_w: i32,
    window_h: i32,
    scale_bits: u32,
    activated: bool,
}

#[derive(Clone, Copy)]
struct ShadowSlotCache {
    key: ShadowSlotKey,
    raw: *mut flux::sys::flux_image,
    w: f32,
    h: f32,
}

struct WindowShadowEntry {
    filter: flux::ShadowFilter,
    cached: Vec<Option<ShadowSlotCache>>,
}

pub(super) struct WindowShadowRenderer {
    /// Independent shadow filters per window.
    /// Eliminates cross-window filter thrashing and dangling raw output pointers.
    entries: std::collections::HashMap<tessera_desktop::window::WindowId, WindowShadowEntry>,
    /// Shared scratch masks per frame slot. Reused across windows during rebuilds,
    /// eliminating per-window mask VRAM duplication.
    scratch_masks: Vec<Option<flux::Image>>,
}

/// A rendered shadow ready for compositing: the borrowed filter output for
/// one window this frame, plus its physical-pixel placement.
pub(super) struct RenderedShadow {
    /// The window this shadow belongs to.
    pub window: tessera_desktop::window::WindowId,
    /// Raw `flux_image` borrowed from the shadow filter's current frame
    /// slot. Lifetime: valid until the same slot is applied again, which
    /// happens on the next frame rotation — strictly after this frame's
    /// canvas passes and submit (ADR-0074 frame-slot lifetime). The
    /// composition root applies the filter exactly once per frame, before
    /// any output pass opens, so every composite of this pointer precedes
    /// the next apply on this slot.
    pub raw: *mut flux::sys::flux_image,
    /// Physical-pixel placement (already includes the blur margin).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl WindowShadowRenderer {
    pub(super) fn new(_device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            entries: std::collections::HashMap::new(),
            scratch_masks: Vec::new(),
        })
    }

    /// Render and record shadows for `windows` into this frame's slot.
    /// Must be called at a pass boundary (no active canvas pass). The
    /// returned placements carry raw image pointers borrowed from the
    /// filter — composite them within this frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        &mut self,
        device: &flux::Device,
        canvas: &mut flux::Canvas,
        frame: &mut flux::Frame<'_>,
        windows: &[tessera_desktop::window::Window],
        scale: f32,
        style: tessera_desktop::window::WindowShadowStyle,
    ) -> Vec<RenderedShadow> {
        if style != tessera_desktop::window::WindowShadowStyle::Soft {
            return Vec::new();
        }
        let slot = frame.index() as usize;
        let mut out = Vec::new();

        // Evict masks and filters for closed/unshadowed windows
        let live_shadow_windows: std::collections::HashSet<_> = windows
            .iter()
            .filter(|w| tessera_composite::window_casts_shadow(w))
            .map(|w| w.id)
            .collect();
        self.entries
            .retain(|win_id, _| live_shadow_windows.contains(win_id));
        // The guard tracks the *live* set, not the entry map: on the first
        // frame the map is still empty, so testing it would return before any
        // entry is created and no window would ever cast a shadow.
        if live_shadow_windows.is_empty() {
            self.scratch_masks.clear();
            return Vec::new();
        }

        if self.scratch_masks.len() <= slot {
            self.scratch_masks.resize_with(slot + 1, || None);
        }

        for window in windows
            .iter()
            .filter(|w| tessera_composite::window_casts_shadow(w))
        {
            // Physical extent: the window rect inflated by the blur
            // footprint so the Gaussian skirt fits inside the image.
            let margin = (SHADOW_BLUR_SIGMA * 3.0 + SHADOW_OFFSET_Y.abs() + 2.0) * scale;
            let px_w = (window.size.w as f32 * scale + margin * 2.0)
                .ceil()
                .max(1.0) as u32;
            let px_h = (window.size.h as f32 * scale + margin * 2.0)
                .ceil()
                .max(1.0) as u32;
            let bucket_w = flux::bucket_extent(px_w, 128);
            let bucket_h = flux::bucket_extent(px_h, 128);

            let entry = match self.entries.entry(window.id) {
                std::collections::hash_map::Entry::Occupied(occ) => occ.into_mut(),
                std::collections::hash_map::Entry::Vacant(vac) => {
                    let Ok(filter) = flux::ShadowFilter::new(device) else {
                        continue;
                    };
                    vac.insert(WindowShadowEntry {
                        filter,
                        cached: Vec::new(),
                    })
                }
            };

            if entry.cached.len() <= slot {
                entry.cached.resize_with(slot + 1, || None);
            }
            let key = ShadowSlotKey {
                window_w: window.size.w,
                window_h: window.size.h,
                scale_bits: scale.to_bits(),
                activated: window.state.activated,
            };
            // Shadow mask and Gaussian blur output are in window-local
            // coordinates and depend only on `(size, scale, activated)`.
            // Reuse the slot's persistent filter output across steady-state
            // frames and window translations without re-running the offscreen
            // mask pass or the 3-pass Gaussian blur compute shaders.
            if let Some(cached) = entry.cached[slot]
                && cached.key == key
            {
                out.push(RenderedShadow {
                    window: window.id,
                    raw: cached.raw,
                    x: window.position.x as f32 * scale - margin,
                    y: window.position.y as f32 * scale - margin,
                    w: cached.w,
                    h: cached.h,
                });
                continue;
            }

            let mask_stale = self.scratch_masks[slot]
                .as_ref()
                .is_none_or(|img| img.size() != (bucket_w, bucket_h));
            if mask_stale {
                let Ok(image) = flux::Image::render_target(
                    device,
                    bucket_w,
                    bucket_h,
                    flux::Format::Rgba8Unorm,
                ) else {
                    continue;
                };
                self.scratch_masks[slot] = Some(image);
                entry.cached[slot] = None;
            }
            let Some(mask) = self.scratch_masks[slot].as_mut() else {
                continue;
            };

            // Draw the mask: opaque rounded rect at the window's position
            // within the padded extent.
            let clear = flux::rgba(0, 0, 0, 0);
            let draw_mask_ok = {
                let session = canvas.begin_session(
                    flux::ImageTarget { frame, image: mask },
                    flux::CanvasPassOptions {
                        clear: Some(clear),
                        antialias: flux::CanvasAntialias::Auto,
                        damage: Vec::new(),
                        skip_stencil: true,
                    },
                );
                if let Ok(session) = session {
                    let inv = 1.0 / scale;
                    session.save();
                    session.scale(scale, scale);
                    session.fill_rrect(
                        inv * margin,
                        inv * margin,
                        window.size.w as f32,
                        window.size.h as f32,
                        SHADOW_CORNER_RADIUS,
                        flux::rgba(255, 255, 255, 255),
                    );
                    session.restore();
                    session.end().is_ok()
                } else {
                    false
                }
            };
            if !draw_mask_ok {
                entry.cached[slot] = None;
                continue;
            }

            let params = flux::ShadowParams {
                blur: SHADOW_BLUR_SIGMA * scale,
                offset_x: 0.0,
                offset_y: SHADOW_OFFSET_Y * scale,
                tint_red: 0.0,
                tint_green: 0.0,
                tint_blue: 0.0,
                alpha: if window.state.activated {
                    SHADOW_ALPHA_FOCUS
                } else {
                    SHADOW_ALPHA_IDLE
                },
            };
            match entry.filter.apply(frame, mask, params) {
                Ok(shadow) => {
                    // Optics ADR-0147: `apply` registers a deferred pass; the
                    // output handle is valid but its pixels are not written
                    // until something requires them. Two reasons this boundary
                    // is where that happens:
                    //
                    // - The pointer escapes this function. `tessera-composite`
                    //   draws it inside a canvas pass, and a pass that samples
                    //   an unexecuted deferred output is rejected outright
                    //   (`FLUX_ERROR_INVALID_STATE`), so a shadow composited
                    //   without this call fails the frame instead of painting.
                    // - The slot cache stores the pointer for later frames. A
                    //   pruned pass leaves the image unwritten, so caching
                    //   first and materializing later would freeze an
                    //   un-executed texture into the steady-state path.
                    //
                    // We are at a pass boundary by contract (`prepare` is
                    // called before any output pass opens), which is the only
                    // place the flush is legal.
                    if let Err(error) =
                        tessera_composite::effects::materialize_effect_output(frame, &shadow)
                    {
                        entry.cached[slot] = None;
                        log::debug!("window shadow: materialize failed ({error}); skipping");
                        continue;
                    }
                    let raw = shadow.as_raw();
                    let w = mask.size().0 as f32;
                    let h = mask.size().1 as f32;
                    entry.cached[slot] = Some(ShadowSlotCache { key, raw, w, h });
                    out.push(RenderedShadow {
                        window: window.id,
                        raw,
                        x: window.position.x as f32 * scale - margin,
                        y: window.position.y as f32 * scale - margin,
                        w,
                        h,
                    });
                }
                Err(error) => {
                    entry.cached[slot] = None;
                    log::debug!("window shadow: filter apply failed ({error}); skipping");
                }
            }
        }
        out
    }
}

/// Shadow design constants (policy values, ADR-0139: data, not mechanism).
pub(super) const SHADOW_BLUR_SIGMA: f32 = 10.0;
pub(super) const SHADOW_OFFSET_Y: f32 = 12.0;
pub(super) const SHADOW_CORNER_RADIUS: f32 = 12.0;
pub(super) const SHADOW_ALPHA_FOCUS: f32 = 0.45;
pub(super) const SHADOW_ALPHA_IDLE: f32 = 0.30;
