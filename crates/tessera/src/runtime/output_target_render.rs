//! Normal-frame rendering into one native target per physical output.
//!
//! The legacy union swapchain is still used for capture/screenshot frames and
//! as a failure fallback while those consumers migrate, but the ordinary
//! desktop no longer presents a union image. Each extended output renders its
//! own logical rectangle into a mode-sized target; a mirror target receives a
//! blit of its source target with the fitted letterbox. Every target has its
//! own Flux ring, KMS slot, and page flip.

use super::output_target::flux_format_for_drm;
use super::*;
use tessera_wayland::PresentedSurfaces;

struct RenderedSource {
    connector: String,
    format: flux::Format,
    color_pipeline: tessera_desktop::output::ColorPipeline,
    exported: flux::ExportedDmabuf,
    origin: tessera_primitives::Point,
    scale: f32,
    mode: (u32, u32),
}

impl CompositorRuntime<'_> {
    /// Whether the normal frame can use the per-output target path.
    ///
    /// Capture, screenshot freeze, streams, shell overview/switcher, 3D
    /// wallpaper, and soft shadows still have union-coupled state. They fall
    /// back to the union path until their consumer is migrated.
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
            || self.overlay_state.active().is_some()
            || self.streams.any_output_live()
            || self.server.lock_confirmation_pending()
        {
            return false;
        }
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

    /// Map union-physical damage onto one output target, or return `None`
    /// when this output has no changed pixels and already has a complete
    /// previous frame.
    fn target_damage(
        &self,
        output: &tessera_desktop::output::OutputInfo,
        target: &OutputTarget,
        damage: &FrameDamage,
    ) -> Option<Vec<flux::CanvasRenderArea>> {
        if target.force_full || target.initialized < 3 || matches!(damage, FrameDamage::Full) {
            return Some(Vec::new());
        }
        let Some(source) = self.host.output_source_rect(&output.connector) else {
            return Some(Vec::new());
        };
        target_damage_areas(damage, source, target.mode)
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
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let Some(presented) = self.host.presented_dmabuf() else {
            return Ok(Vec::new());
        };
        let groups = self.presentation_groups().to_vec();
        let mut blocked = std::collections::BTreeSet::new();
        for group in &groups {
            if group
                .outputs
                .iter()
                .any(|connector| self.host.output_presentation_pending(connector))
            {
                blocked.extend(group.outputs.iter().cloned());
            }
        }
        let outputs = self.server.output_infos();
        let physical = outputs
            .iter()
            .filter(|output| output.kind.is_physical() && !blocked.contains(&output.connector))
            .cloned()
            .collect::<Vec<_>>();
        let software_cursor = self.host.uses_software_cursor();
        let cursor = self.input_acc.cursor;
        let format = flux_format_for_drm(presented.fourcc);
        let color_space = image_color_space(presented.color_pipeline);
        let mut targets = std::mem::take(&mut self.output_targets);
        let mut exports = std::collections::BTreeMap::new();
        let mut presented_sources = Vec::new();
        let mut failed = Vec::new();
        for output in &physical {
            let Some(target) = targets.get_mut(&output.connector) else {
                continue;
            };
            let Some(source_connector) = output.role.mirror_source().or(Some(&output.connector))
            else {
                continue;
            };
            let Some(source_rect) = self.host.output_source_rect(source_connector) else {
                failed.push(output.connector.clone());
                continue;
            };
            let Ok(fd) = presented.fd.try_clone() else {
                failed.push(output.connector.clone());
                continue;
            };
            let fence = match presented.acquire_fence.as_ref() {
                Some(fence) => match fence.try_clone() {
                    Ok(fence) => Some(fence),
                    Err(error) => {
                        log::warn!(
                            "{}: cannot clone composed fence for target {}: {error}",
                            self.host.name(),
                            output.connector
                        );
                        failed.push(output.connector.clone());
                        continue;
                    }
                },
                None => None,
            };
            let image = unsafe {
                flux::Image::import_dmabuf_with_color_space(
                    self.device,
                    presented.width,
                    presented.height,
                    format,
                    presented.modifier,
                    fd,
                    0,
                    presented.stride,
                    fence,
                    color_space,
                )
            };
            let image = match image {
                Ok(image) => image,
                Err(error) => {
                    log::warn!(
                        "{}: cannot import composed image for target {}: {error}",
                        self.host.name(),
                        output.connector
                    );
                    failed.push(output.connector.clone());
                    continue;
                }
            };
            let mode = target.mode;
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
            let mut frame = match target
                .surface
                .begin_frame_timeout_with_damage(2_000_000, &flux::Damage::Full)
            {
                Ok(frame) => frame,
                Err(error) => {
                    log::debug!(
                        "{}: capture target frame unavailable for {}: {error}",
                        self.host.name(),
                        output.connector
                    );
                    continue;
                }
            };
            let result = (|| -> Result<(), String> {
                let session = target
                    .canvas
                    .begin_session(
                        &mut frame,
                        flux::CanvasPassOptions {
                            clear: Some(flux::rgba(0, 0, 0, 255)),
                            damage: Vec::new(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
                session.draw_image_opaque_sub(
                    &image,
                    dst_x,
                    dst_y,
                    dst_w,
                    dst_h,
                    source_rect.origin.x as f32 / full_w,
                    source_rect.origin.y as f32 / full_h,
                    source_rect.size.w.max(1) as f32 / full_w,
                    source_rect.size.h.max(1) as f32 / full_h,
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
                        let cursor_x = dst_x + (cursor.0 - origin.x as f32) * source_scale * fit;
                        let cursor_y = dst_y + (cursor.1 - origin.y as f32) * source_scale * fit;
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
                frame
                    .submit()
                    .map_err(|error| error.to_string())?
                    .present()
                    .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if let Err(error) = result {
                log::warn!(
                    "{}: capture target render failed for {}: {error}",
                    self.host.name(),
                    output.connector
                );
                failed.push(output.connector.clone());
                continue;
            }
            let exported = match target.surface.export_dmabuf_explicit() {
                Ok(exported) => exported,
                Err(error) => {
                    log::warn!(
                        "{}: capture target export failed for {}: {error}",
                        self.host.name(),
                        output.connector
                    );
                    failed.push(output.connector.clone());
                    continue;
                }
            };
            exports.insert(output.connector.clone(), exported);
        }
        for group in &groups {
            if group
                .outputs
                .iter()
                .any(|connector| blocked.contains(connector))
            {
                continue;
            }
            let mut group_targets = Vec::with_capacity(group.outputs.len());
            let mut complete = true;
            for connector in &group.outputs {
                if let Some(exported) = exports.remove(connector) {
                    group_targets.push((connector.clone(), exported));
                } else {
                    complete = false;
                    break;
                }
            }
            if !complete {
                failed.extend(group.outputs.iter().cloned());
                continue;
            }
            let group_connectors = group.outputs.clone();
            match self.host.present_render_targets(group_targets) {
                Ok(()) => {
                    presented_sources.extend(group_connectors.into_iter().filter(|connector| {
                        outputs.iter().any(|output| {
                            output.connector == *connector && !output.role.is_mirror()
                        })
                    }));
                }
                Err(error) => {
                    log::warn!(
                        "{}: capture target group commit failed: {error}",
                        self.host.name()
                    );
                    failed.extend(group_connectors);
                }
            }
        }
        for (connector, exported) in exports {
            let _ = exported;
            failed.push(connector);
        }
        for connector in failed {
            targets.remove(&connector);
        }
        self.output_targets = targets;
        Ok(presented_sources)
    }

    /// Whether every physical output has a compositor target. Overlay
    /// offload is disabled while this is true because a KMS overlay is not
    /// part of the union source image a target blits.
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

    /// Render and present the ordinary desktop through per-output targets.
    ///
    /// Returns `Retry` when no source target reached KMS. A target that loses
    /// its frame race is left out of this frame; the next frame retries its
    /// existing ring rather than tearing the other outputs down.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_output_targets(
        &mut self,
        damage: &FrameDamage,
        sampled_surface_ids: &[usize],
        cursor_shape: u32,
        cursor_hidden: bool,
        window_switcher: Option<&tessera_shell::component::WindowSwitcherPresentation>,
        live_previews: &[tessera_shell::component::LivePreviewPresentation],
    ) -> Result<PresentationOutcome, Box<dyn std::error::Error>> {
        let groups = self.presentation_groups().to_vec();
        let mut blocked = std::collections::BTreeSet::new();
        for group in &groups {
            if group
                .outputs
                .iter()
                .any(|connector| self.host.output_presentation_pending(connector))
            {
                blocked.extend(group.outputs.iter().cloned());
            }
        }
        let outputs = self.server.output_infos();
        let physical = outputs
            .iter()
            .filter(|output| output.kind.is_physical() && !blocked.contains(&output.connector))
            .cloned()
            .collect::<Vec<_>>();
        let primary = physical
            .iter()
            .find(|output| !output.role.is_mirror())
            .map(|output| output.connector.clone());
        let color_scheme = self.shell.design().scheme;
        let shadow_style = self
            .config
            .as_ref()
            .map(|config| config.ui.window_shadow)
            .unwrap_or_default();
        let overview_progress = self.shell.overview_progress();
        let overview_active = self.shell.overview_active() || overview_progress > 0.001;
        let model_active = self
            .wallpaper
            .as_ref()
            .is_some_and(wallpaper::Wallpaper::has_model);
        let drew_preview_sources = overview_active
            || window_switcher.is_some()
            || !live_previews.is_empty()
            || self.server.workspace_slide_pending();
        let special_full = drew_preview_sources
            || model_active
            || shadow_style == tessera_desktop::window::WindowShadowStyle::Soft;
        let shell_composes = self.shell.requires_composition();
        let software_cursor = self.host.uses_software_cursor();
        let cursor = self.input_acc.cursor;
        if physical.is_empty() {
            for connector in &blocked {
                if let Some(target) = self.output_targets.get_mut(connector) {
                    target.force_full = true;
                }
            }
            self.damage.pending_damage = Some(FrameDamage::Full);
            return Ok(PresentationOutcome::Retry {
                wait: RetryWait::BackendEvent,
            });
        }

        self.direct_outputs.clear();
        self.renderer.begin_frame();
        let mut targets = std::mem::take(&mut self.output_targets);
        for connector in &blocked {
            if let Some(target) = targets.get_mut(connector) {
                target.force_full = true;
            }
        }
        let mut sources: Vec<RenderedSource> = Vec::new();
        let mut mirror_exports: Vec<(String, flux::ExportedDmabuf)> = Vec::new();
        let mut presented_sources: Vec<String> = Vec::new();
        let mut skipped_sources = std::collections::BTreeSet::new();
        let mut failed: Vec<(String, (u32, u32))> = Vec::new();

        // Extended/source targets first. They own the only scene pass that
        // includes chrome; mirrors sample the finished source image.
        for output in physical.iter().filter(|output| !output.role.is_mirror()) {
            let Some(target) = targets.get_mut(&output.connector) else {
                continue;
            };
            let mode = target.mode;
            let logical = output.geometry.logical_size();
            let logical_size = (logical.w.max(1) as u32, logical.h.max(1) as u32);
            let scale = output.geometry.scale.as_f32().max(0.01);
            let origin = output.geometry.logical_origin;
            let target_damage = if special_full {
                Some(Vec::new())
            } else {
                self.target_damage(output, target, damage)
            };
            let Some(canvas_damage) = target_damage else {
                // No changed pixels reached this output. Keep its target and
                // skip the source-to-mirror dependency for this frame.
                skipped_sources.insert(output.connector.clone());
                continue;
            };
            let mut frame = match target
                .surface
                .begin_frame_timeout_with_damage(250_000_000, &flux::Damage::Full)
            {
                Ok(frame) => frame,
                Err(error) => {
                    log::debug!(
                        "{}: target frame unavailable for {}: {error}",
                        self.host.name(),
                        output.connector
                    );
                    // A missing ring slot is transient: keep the target and
                    // retry its own frame next iteration. Mirrors also skip
                    // this frame because their source image was not renewed.
                    skipped_sources.insert(output.connector.clone());
                    continue;
                }
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
                        &mut target.canvas,
                        &mut frame,
                        &windows,
                        scale,
                        shadow_style,
                        target_namespace(&output.connector),
                    )
                } else {
                    Vec::new()
                };
            let soft_shadow_entries: Vec<tessera_composite::SoftShadowEntry<'_>> = rendered_shadows
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
            let soft_shadow_layer =
                (!soft_shadow_entries.is_empty()).then_some(tessera_composite::SoftShadowLayer {
                    entries: soft_shadow_entries.as_slice(),
                });
            let result = (|| -> Result<(), String> {
                let wallpaper_session = target
                    .canvas
                    .begin_session(
                        &mut frame,
                        flux::CanvasPassOptions {
                            clear: Some(self.clear),
                            damage: canvas_damage.clone(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
                draw_wallpaper_background(
                    &wallpaper_session,
                    self.device,
                    &mut self.wallpaper,
                    logical_size,
                    scale,
                );
                wallpaper_session.end().map_err(|error| error.to_string())?;
                if model_active && let Some(wallpaper) = self.wallpaper.as_mut() {
                    wallpaper.draw_model(self.device, &mut frame);
                }
                let session = target
                    .canvas
                    .begin_session(
                        &mut frame,
                        flux::CanvasPassOptions {
                            clear: None,
                            damage: canvas_damage,
                        },
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
                    None,
                )
                .map_err(|error| error.to_string())?;
                session.restore();
                if primary.as_deref() == Some(output.connector.as_str())
                    && shell_composes
                    && let Err(error) = unsafe { self.shell.submit(session.as_raw() as *mut _) }
                {
                    log::warn!(
                        "compositor: shell chrome render failed on {}: {error}",
                        output.connector
                    );
                }
                if primary.as_deref() == Some(output.connector.as_str()) {
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
                    false,
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
                frame
                    .submit()
                    .map_err(|error| error.to_string())?
                    .present()
                    .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if let Err(error) = result {
                log::warn!(
                    "{}: target render failed for {}; retrying next frame: {error}",
                    self.host.name(),
                    output.connector
                );
                failed.push((output.connector.clone(), mode));
                continue;
            }
            let exported = match target.surface.export_dmabuf_explicit() {
                Ok(exported) => exported,
                Err(error) => {
                    log::warn!(
                        "{}: target export failed for {}; retrying next frame: {error}",
                        self.host.name(),
                        output.connector
                    );
                    failed.push((output.connector.clone(), mode));
                    continue;
                }
            };
            target.initialized = target.initialized.saturating_add(1).min(3);
            target.force_full = false;
            sources.push(RenderedSource {
                connector: output.connector.clone(),
                format: target.format,
                color_pipeline: target.color_pipeline,
                exported,
                origin,
                scale,
                mode,
            });
        }

        // Mirror targets are compositor-only: import the finished source
        // target, fit it into the target content rectangle, and commit 1:1.
        for output in physical.iter().filter(|output| output.role.is_mirror()) {
            let Some(target) = targets.get_mut(&output.connector) else {
                continue;
            };
            let Some(source_connector) = output.role.mirror_source() else {
                continue;
            };
            if skipped_sources.contains(source_connector) {
                continue;
            }
            let Some(source) = sources
                .iter()
                .find(|source| source.connector == source_connector)
            else {
                failed.push((output.connector.clone(), target.mode));
                continue;
            };
            let mode = target.mode;
            let Ok(fd) = source.exported.dmabuf.fd.try_clone() else {
                failed.push((output.connector.clone(), mode));
                continue;
            };
            let fence = match source.exported.dmabuf.acquire_fence.as_ref() {
                Some(fence) => match fence.try_clone() {
                    Ok(fence) => Some(fence),
                    Err(error) => {
                        log::warn!(
                            "{}: cannot clone mirror source fence for {}: {error}",
                            self.host.name(),
                            output.connector
                        );
                        failed.push((output.connector.clone(), mode));
                        continue;
                    }
                },
                None => None,
            };
            let image = unsafe {
                flux::Image::import_dmabuf_with_color_space(
                    self.device,
                    source.exported.dmabuf.width,
                    source.exported.dmabuf.height,
                    source.format,
                    source.exported.dmabuf.modifier,
                    fd,
                    0,
                    source.exported.dmabuf.stride,
                    fence,
                    image_color_space(source.color_pipeline),
                )
            };
            let image = match image {
                Ok(image) => image,
                Err(error) => {
                    log::warn!(
                        "{}: cannot import source target for mirror {}: {error}",
                        self.host.name(),
                        output.connector
                    );
                    failed.push((output.connector.clone(), mode));
                    continue;
                }
            };
            let (fx, fy, fw, fh) = output.content_fraction();
            let (dst_x, dst_y) = (fx * mode.0 as f32, fy * mode.1 as f32);
            let (dst_w, dst_h) = (fw * mode.0 as f32, fh * mode.1 as f32);
            let source_mode = source.mode;
            let mut frame = match target
                .surface
                .begin_frame_timeout_with_damage(2_000_000, &flux::Damage::Full)
            {
                Ok(frame) => frame,
                Err(error) => {
                    log::debug!(
                        "{}: mirror target frame unavailable for {}: {error}",
                        self.host.name(),
                        output.connector
                    );
                    continue;
                }
            };
            let result = (|| -> Result<(), String> {
                let session = target
                    .canvas
                    .begin_session(
                        &mut frame,
                        flux::CanvasPassOptions {
                            clear: Some(flux::rgba(0, 0, 0, 255)),
                            damage: Vec::new(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
                session.draw_image_opaque(&image, dst_x, dst_y, dst_w, dst_h);
                if !software_cursor && !cursor_hidden {
                    let source_scale = source.scale;
                    if let Some(cursor_sprite) =
                        self.cursor_cache
                            .get(self.device, cursor_shape, source_scale)
                    {
                        let fit = dst_w / source_mode.0.max(1) as f32;
                        let cursor_x =
                            dst_x + (cursor.0 - source.origin.x as f32) * source_scale * fit;
                        let cursor_y =
                            dst_y + (cursor.1 - source.origin.y as f32) * source_scale * fit;
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
                frame
                    .submit()
                    .map_err(|error| error.to_string())?
                    .present()
                    .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if let Err(error) = result {
                log::warn!(
                    "{}: mirror target render failed for {}; retrying next frame: {error}",
                    self.host.name(),
                    output.connector
                );
                failed.push((output.connector.clone(), mode));
                continue;
            }
            let exported = match target.surface.export_dmabuf_explicit() {
                Ok(exported) => exported,
                Err(error) => {
                    log::warn!(
                        "{}: mirror target export failed for {}; retrying next frame: {error}",
                        self.host.name(),
                        output.connector
                    );
                    failed.push((output.connector.clone(), mode));
                    continue;
                }
            };
            mirror_exports.push((output.connector.clone(), exported));
        }

        // Group commit: every coherent output group flips one atomic batch;
        // separate groups remain independent. Sources and mirrors can share a
        // group because both already have finished target images here.
        let mut exports = std::collections::BTreeMap::new();
        for source in sources {
            let connector = source.connector;
            exports.insert(connector, source.exported);
        }
        for (connector, exported) in mirror_exports {
            exports.insert(connector, exported);
        }
        for group in &groups {
            if group
                .outputs
                .iter()
                .any(|connector| blocked.contains(connector))
            {
                continue;
            }
            let mut group_targets = Vec::with_capacity(group.outputs.len());
            let mut complete = true;
            for connector in &group.outputs {
                if let Some(exported) = exports.remove(connector) {
                    group_targets.push((connector.clone(), exported));
                } else {
                    complete = false;
                    break;
                }
            }
            if !complete {
                failed.extend(group.outputs.iter().map(|connector| {
                    (
                        connector.clone(),
                        targets.get(connector).map_or((1, 1), |target| target.mode),
                    )
                }));
                continue;
            }
            let group_connectors = group.outputs.clone();
            match self.host.present_render_targets(group_targets) {
                Ok(()) => {
                    presented_sources.extend(group_connectors.into_iter().filter(|connector| {
                        !self
                            .server
                            .output_infos()
                            .iter()
                            .any(|o| o.connector == *connector && o.role.is_mirror())
                    }));
                }
                Err(error) => {
                    log::warn!("{}: target group commit failed: {error}", self.host.name());
                    failed.extend(
                        group_connectors
                            .into_iter()
                            .map(|connector| (connector, (1, 1))),
                    );
                }
            }
        }
        // Any export not covered by a group is a planner error; fail it rather
        // than leaking a lease.
        for (connector, exported) in exports {
            let mode = (exported.dmabuf.width, exported.dmabuf.height);
            failed.push((connector, mode));
        }

        for (connector, _mode) in &failed {
            // A render/export/commit failure is transient presentation state,
            // not a permanent capability verdict. Drop the target so the next
            // frame can rebuild it; creation failures remain in
            // `output_target_failures` and are retried on topology change.
            targets.remove(connector);
        }
        self.output_targets = targets;

        if presented_sources.is_empty() {
            self.damage.pending_damage = Some(FrameDamage::Full);
            return Ok(PresentationOutcome::Retry {
                wait: RetryWait::BackendEvent,
            });
        }

        let target_refs = presented_sources
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        self.primary_plane_state.commit_composited();
        self.server.acknowledge_presented_surface_damage_on_outputs(
            sampled_surface_ids.iter(),
            drew_preview_sources,
            Some(&target_refs),
        );
        self.server.send_frame_callbacks_on_outputs(
            self.server.monotonic_ms(),
            PresentedSurfaces::Sampled {
                ids: sampled_surface_ids,
                drew_preview_sources,
            },
            Some(&target_refs),
        );
        if self.server.retired_buffers_pending() {
            let since = self.retired_defer.get_or_insert(self.frame_count);
            if self.frame_count.saturating_sub(*since) >= 4 {
                self.server.release_retired_buffers(None);
                self.retired_defer = None;
            }
        }
        self.damage.last_present_minute = Some(wall_clock_minute());
        self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
        let primary_scale = physical
            .iter()
            .find(|output| !output.role.is_mirror())
            .map_or(self.host.scale(), |output| output.geometry.scale.as_f32());
        self.damage.last_presented_cursor_position = Some((
            (self.input_acc.cursor.0 * primary_scale).round() as i32,
            (self.input_acc.cursor.1 * primary_scale).round() as i32,
        ));
        self.damage.last_presented_cursor_hotspot = (!cursor_hidden).then_some(last_hotspot(
            &mut self.cursor_cache,
            self.device,
            cursor_shape,
            primary_scale,
        ));
        self.damage.last_presented_cursor_pixels = (!cursor_hidden).then_some(last_sprite(
            &mut self.cursor_cache,
            self.device,
            cursor_shape,
            primary_scale,
        ));
        self.damage.force_full_redraw = false;
        self.independent_target_flips = true;
        self.renderer.gc(self.server.live_surface_ids());
        self.frame_count += 1;
        Ok(PresentationOutcome::Submitted)
    }
}

/// Clip union-physical damage to one target and translate it into target
/// local pixels. `None` means this target has no changed pixels.
fn target_damage_areas(
    damage: &FrameDamage,
    source: tessera_primitives::Rect,
    mode: (u32, u32),
) -> Option<Vec<flux::CanvasRenderArea>> {
    if matches!(damage, FrameDamage::Full) {
        return Some(Vec::new());
    }
    let damage_rects = damage.area_rects()?;
    let mut areas = Vec::new();
    for rect in damage_rects {
        let Some(clipped) = rect.intersect(source) else {
            continue;
        };
        let x = clipped.origin.x - source.origin.x;
        let y = clipped.origin.y - source.origin.y;
        let width = clipped.size.w.min(mode.0 as i32 - x);
        let height = clipped.size.h.min(mode.1 as i32 - y);
        if x >= 0 && y >= 0 && width > 0 && height > 0 {
            areas.push(flux::CanvasRenderArea {
                x: x as f32,
                y: y as f32,
                width: width as f32,
                height: height as f32,
            });
        }
    }
    (!areas.is_empty()).then_some(areas)
}

fn target_namespace(connector: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in connector.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn image_color_space(
    pipeline: tessera_desktop::output::ColorPipeline,
) -> flux::ImageColorSpace<'static> {
    let space = match pipeline {
        tessera_desktop::output::ColorPipeline::Hdr => flux::ColorSpace::BT2020_PQ,
        _ => flux::ColorSpace::SRGB,
    };
    flux::ImageColorSpace {
        space: Some(space),
        icc: None,
    }
}

fn last_hotspot(
    cache: &mut cursor::CursorCache,
    device: &flux::Device,
    shape: u32,
    scale: f32,
) -> (u32, u32) {
    cache
        .get(device, shape, scale)
        .map_or((0, 0), |cursor| (cursor.xhot as u32, cursor.yhot as u32))
}

fn last_sprite(
    cache: &mut cursor::CursorCache,
    device: &flux::Device,
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

    #[test]
    fn target_damage_clips_and_translates_union_pixels() {
        let source = tessera_primitives::Rect::new(3840, 0, 2560, 1440);
        let damage = FrameDamage::from_rects(vec![
            tessera_primitives::Rect::new(3900, 100, 200, 300),
            tessera_primitives::Rect::new(10, 10, 100, 100),
        ]);
        let areas = target_damage_areas(&damage, source, (2560, 1440)).unwrap();
        assert_eq!(areas.len(), 1);
        assert_eq!((areas[0].x, areas[0].y), (60.0, 100.0));
        assert_eq!((areas[0].width, areas[0].height), (200.0, 300.0));
    }

    #[test]
    fn target_damage_without_intersection_skips_the_output() {
        let source = tessera_primitives::Rect::new(3840, 0, 2560, 1440);
        let damage = FrameDamage::from_rects(vec![tessera_primitives::Rect::new(10, 10, 100, 100)]);
        assert!(target_damage_areas(&damage, source, (2560, 1440)).is_none());
    }

    #[test]
    fn target_damage_full_forces_a_full_pass() {
        let source = tessera_primitives::Rect::new(0, 0, 1920, 1080);
        assert_eq!(
            target_damage_areas(&FrameDamage::Full, source, (1920, 1080)),
            Some(Vec::new())
        );
    }
}
