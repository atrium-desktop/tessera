//! Compositor-owned presentation target for physical outputs.
//!
//! A target is a mode-sized exportable surface with its own Flux ring and
//! canvas. The normal DRM path renders each extended output's local scene into
//! its target; a mirror target is blitted from its source target with a fitted
//! letterbox, and KMS scans every target 1:1. The union path remains only for
//! capture/screenshot frames and as a failure fallback while the remaining
//! capture consumers migrate to output targets.

use super::*;

impl CompositorRuntime<'_> {
    /// Reconcile target surfaces with the server's current mirror topology.
    pub(super) fn sync_output_targets(&mut self) {
        if self.host.name() != "drm" {
            self.output_targets.clear();
            self.output_target_failures.clear();
            return;
        }
        let outputs = self.server.output_infos();
        let desired = outputs
            .iter()
            .filter(|output| output.kind.is_physical())
            .map(|output| {
                let mode = (
                    output.geometry.mode.width.max(1) as u32,
                    output.geometry.mode.height.max(1) as u32,
                );
                let source = output
                    .role
                    .mirror_source()
                    .map_or_else(|| output.connector.clone(), str::to_owned);
                (output.connector.clone(), (mode, source))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        self.output_targets.retain(|connector, target| {
            desired
                .get(connector)
                .is_some_and(|(mode, source)| target.mode == *mode && target.source == *source)
        });
        self.output_target_failures
            .retain(|connector, _| desired.contains_key(connector));
        for (connector, (mode, source)) in &desired {
            if self.output_targets.contains_key(connector)
                || self.output_target_failures.get(connector) == Some(mode)
            {
                continue;
            }
            match self.host.create_render_target(self.device, mode.0, mode.1) {
                Ok(surface) => match flux::Canvas::new(&surface) {
                    Ok(canvas) => {
                        let format = surface.info().format;
                        let color_pipeline = self.host.color_pipeline();
                        self.output_target_failures.remove(connector);
                        self.output_targets.insert(
                            connector.clone(),
                            OutputTarget {
                                canvas,
                                surface,
                                mode: *mode,
                                format,
                                color_pipeline,
                                source: source.clone(),
                                initialized: 0,
                                force_full: false,
                                source_lease: None,
                            },
                        );
                        log::info!(
                            "{}: output target active for {connector} at {}x{}",
                            self.host.name(),
                            mode.0,
                            mode.1
                        );
                    }
                    Err(error) => {
                        log::warn!(
                            "{}: cannot create mirror canvas for {connector}: {error}",
                            self.host.name()
                        );
                        self.output_target_failures.insert(connector.clone(), *mode);
                    }
                },
                Err(error) => {
                    log::warn!(
                        "{}: cannot create mirror target for {connector} at {}x{}: {error}",
                        self.host.name(),
                        mode.0,
                        mode.1
                    );
                    self.output_target_failures.insert(connector.clone(), *mode);
                }
            }
        }
    }

    /// Blit the just-presented union image into every mirror target.
    ///
    /// This is the union fallback path used while a capture or another
    /// non-target frame is being presented. The normal target path renders
    /// source targets directly and does not call this function.
    pub(super) fn present_output_targets(&mut self, cursor_shape: u32, cursor_hidden: bool) {
        if self.output_targets.is_empty() {
            return;
        }
        let Some(presented) = self.host.presented_dmabuf() else {
            // The union frame had no retained descriptor. Keeping the
            // targets would exclude their mirrors from the next union present
            // and freeze them; drop them so the union path owns every output
            // until a target can be rebuilt.
            self.output_targets.clear();
            return;
        };
        let outputs = self.server.output_infos();
        let mut targets = std::mem::take(&mut self.output_targets);
        let mut failed = Vec::new();
        for (connector, target) in &mut targets {
            if !outputs
                .iter()
                .any(|output| output.connector == *connector && output.role.is_mirror())
            {
                failed.push(connector.clone());
                continue;
            }
            let Some(output) = outputs.iter().find(|output| output.connector == *connector) else {
                failed.push(connector.clone());
                continue;
            };
            if !output.role.is_mirror() {
                continue;
            }
            let Some(source_rect) = self.host.output_source_rect(&target.source) else {
                failed.push(connector.clone());
                continue;
            };
            let Ok(fd) = presented.fd.try_clone() else {
                failed.push(connector.clone());
                continue;
            };
            let fence = match presented.acquire_fence.as_ref() {
                Some(fence) => match fence.try_clone() {
                    Ok(fence) => Some(fence),
                    Err(error) => {
                        log::warn!(
                            "{}: cannot duplicate composite fence for mirror {connector}: {error}",
                            self.host.name()
                        );
                        failed.push(connector.clone());
                        continue;
                    }
                },
                None => None,
            };
            let lease = std::rc::Rc::clone(&presented.lease);
            let format = flux_format_for_drm(presented.fourcc);
            let color_space = match presented.color_pipeline {
                tessera_desktop::output::ColorPipeline::Hdr => Some(flux::ColorSpace::BT2020_PQ),
                _ => Some(flux::ColorSpace::SRGB),
            };
            // SAFETY: `fd` describes the composed image with exactly the
            // width/height/stride/modifier recorded in `presented`; the fence
            // (when present) orders the producer's rendering before sampling.
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
                    flux::ImageColorSpace {
                        space: color_space,
                        icc: None,
                    },
                )
            };
            let image = match image {
                Ok(image) => image,
                Err(error) => {
                    log::warn!(
                        "{}: cannot import composite for mirror {connector}: {error}",
                        self.host.name()
                    );
                    failed.push(connector.clone());
                    continue;
                }
            };
            // The union image does not contain the hardware cursor plane.
            // A mirror target must draw the same theme cursor after its blit,
            // scaled into the fitted content rectangle.
            let cursor = if !self.host.uses_software_cursor() && !cursor_hidden {
                outputs
                    .iter()
                    .find(|output| output.connector == target.source)
                    .and_then(|source_output| {
                        let source_scale = source_output.geometry.scale.as_f32().max(0.01);
                        self.cursor_cache
                            .get(self.device, cursor_shape, source_scale)
                            .map(|cursor| {
                                (source_output.geometry.logical_origin, source_scale, cursor)
                            })
                    })
            } else {
                None
            };
            let frame = match target
                .surface
                .begin_frame_timeout_with_damage(2_000_000, &flux::Damage::Full)
            {
                Ok(frame) => frame,
                Err(error) => {
                    log::debug!(
                        "{}: mirror {connector} target frame unavailable: {error}",
                        self.host.name()
                    );
                    continue;
                }
            };
            let mut frame = frame;
            let (mode_w, mode_h) = target.mode;
            let (fx, fy, fw, fh) = output.content_fraction();
            let (dst_x, dst_y) = (fx * mode_w as f32, fy * mode_h as f32);
            let (dst_w, dst_h) = (fw * mode_w as f32, fh * mode_h as f32);
            let full_w = presented.width.max(1) as f32;
            let full_h = presented.height.max(1) as f32;
            let source = flux::CanvasRenderArea {
                x: source_rect.origin.x as f32,
                y: source_rect.origin.y as f32,
                width: source_rect.size.w.max(1) as f32,
                height: source_rect.size.h.max(1) as f32,
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
                    source.x / full_w,
                    source.y / full_h,
                    source.width / full_w,
                    source.height / full_h,
                );
                if let Some((origin, source_scale, cursor)) = cursor {
                    let fit = dst_w / source.width.max(1.0);
                    let cursor_x =
                        dst_x + (self.input_acc.cursor.0 - origin.x as f32) * source_scale * fit;
                    let cursor_y =
                        dst_y + (self.input_acc.cursor.1 - origin.y as f32) * source_scale * fit;
                    let x = cursor_x - cursor.xhot * fit;
                    let y = cursor_y - cursor.yhot * fit;
                    session.draw_image(
                        &cursor.image,
                        x,
                        y,
                        cursor.width * fit,
                        cursor.height * fit,
                    );
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
                    "{}: mirror {connector} blit failed; returning it to the union path: {error}",
                    self.host.name()
                );
                failed.push(connector.clone());
                continue;
            }
            let exported = match target.surface.export_dmabuf_explicit() {
                Ok(exported) => exported,
                Err(error) => {
                    log::warn!(
                        "{}: mirror {connector} export failed; returning it to the union path: {error}",
                        self.host.name()
                    );
                    failed.push(connector.clone());
                    continue;
                }
            };
            match self.host.present_render_target(exported, connector) {
                Ok(_) => target.source_lease = Some(lease),
                Err(error) => {
                    log::warn!(
                        "{}: mirror {connector} target commit failed; returning it to the union path: {error}",
                        self.host.name()
                    );
                    failed.push(connector.clone());
                }
            }
        }
        for connector in failed {
            if let Some(target) = targets.remove(&connector) {
                self.output_target_failures.insert(connector, target.mode);
            }
        }
        self.output_targets = targets;
    }
}

/// Map the DRM fourcc of the union framebuffer to a Flux sampled format.
pub(super) fn flux_format_for_drm(fourcc: u32) -> flux::Format {
    match fourcc {
        // DRM_FORMAT_XRGB8888 / ARGB8888: little-endian words are B,G,R,X.
        0x3432_5258 | 0x3432_5241 => flux::Format::Bgra8Unorm,
        // DRM_FORMAT_XBGR8888 / ABGR8888: little-endian words are R,G,B,X.
        0x3432_4258 | 0x3432_4241 => flux::Format::Rgba8Unorm,
        // 10-bit RGB targets use the RGB10A2 container. The 8-bit fallback
        // is conservative when a driver exposes a packed order Flux cannot
        // swizzle; the target frame will still be rejected by KMS rather than
        // showing wrong colors.
        0x3033_5258 | 0x3033_5241 | 0x3033_4258 | 0x3033_4241 => flux::Format::Rgb10a2Unorm,
        _ => flux::Format::Bgra8Unorm,
    }
}

#[cfg(test)]
mod tests {
    use super::flux_format_for_drm;

    #[test]
    fn drm_fourcc_to_flux_format_is_byte_order_correct() {
        assert_eq!(flux_format_for_drm(0x3432_5258), flux::Format::Bgra8Unorm);
        assert_eq!(flux_format_for_drm(0x3432_5241), flux::Format::Bgra8Unorm);
        assert_eq!(flux_format_for_drm(0x3432_4258), flux::Format::Rgba8Unorm);
        assert_eq!(flux_format_for_drm(0x3432_4241), flux::Format::Rgba8Unorm);
        assert_eq!(flux_format_for_drm(0x3033_5258), flux::Format::Rgb10a2Unorm);
    }
}
