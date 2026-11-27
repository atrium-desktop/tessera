//! Compositor-owned presentation target for physical outputs.
//!
//! A target is a mode-sized exportable surface with its own gfx ring and
//! canvas. The normal DRM path renders each extended output's local scene into
//! its target; a mirror target is blitted from its source target with a fitted
//! letterbox, and KMS scans every target 1:1. The union path remains only for
//! capture/screenshot frames and as a failure fallback while the remaining
//! capture consumers migrate to output targets.

use super::*;
use std::collections::VecDeque;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

/// Bounded frame-acquire wait for compositor-owned offscreen DRM targets.
///
/// Mirrors the union DRM acquire timeout (`presentation.rs`): offscreen rings
/// have no external swapchain presentation engine, so a 2 ms timeout turns any
/// startup or 4K GPU burst into spurious retry loops and target teardowns.
pub(super) const TARGET_ACQUIRE_TIMEOUT_NS: u64 = 250_000_000;

/// Maximum number of cached source DMA-BUF Vulkan imports per target.
///
/// A source target's gfx ring has 3 slots (`FRAMES_IN_FLIGHT`), plus up to 3
/// union fallback slots during capture/stream transitions. Bounding the cache
/// at 8 covers both rings with zero steady-state churn while preventing
/// unbounded descriptor growth across topology changes (ADR-0250,
/// `[INV-ARCH-160]`).
const MAX_TARGET_IMPORTS: usize = 8;

/// Number of target frames an evicted `optics_gfx::Image` stays alive before drop,
/// exceeding `FRAMES_IN_FLIGHT` (3) so no in-flight command buffer can still
/// reference a destroyed Vulkan image view.
const TARGET_IMPORT_RETIRE_EPOCHS: usize = 4;

/// Identity of one exported DMA-BUF image backing a source or union ring slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TargetImportKey {
    pub(super) st_dev: u64,
    pub(super) st_ino: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: u32,
    pub(super) modifier: u64,
    pub(super) format: optics_gfx::Format,
    pub(super) color_pipeline: tessera_desktop::output::ColorPipeline,
}

/// Borrowed descriptor of a DMA-BUF to sample in a mirror or union-target blit.
pub(super) struct TargetImportDesc<'a> {
    pub(super) fd: &'a OwnedFd,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: u32,
    pub(super) modifier: u64,
    pub(super) format: optics_gfx::Format,
    pub(super) color_pipeline: tessera_desktop::output::ColorPipeline,
    pub(super) acquire_fence: Option<&'a OwnedFd>,
}

struct TargetImportEntry {
    key: TargetImportKey,
    image: optics_gfx::Image,
    last_used: u64,
}

/// Per-target bounded LRU cache of imported source DMA-BUF `optics_gfx::Image`s
/// (ADR-0250, `[INV-ARCH-160]`).
///
/// Eliminates per-frame `vkCreateImage`/`vkAllocateMemory`/layout-transition
/// churn when mirror or capture-blit targets repeatedly sample a 3-slot source
/// ring. Evicted images are retired through a 4-epoch queue so they outlive
/// every in-flight Vulkan submission.
#[derive(Default)]
pub(super) struct TargetImportCache {
    entries: Vec<TargetImportEntry>,
    retired: VecDeque<Vec<optics_gfx::Image>>,
    tick: u64,
}

impl TargetImportCache {
    /// Advance the retirement epoch once per target render.
    pub(super) fn begin_frame(&mut self) {
        self.retired.push_back(Vec::new());
        while self.retired.len() > TARGET_IMPORT_RETIRE_EPOCHS {
            self.retired.pop_front();
        }
    }

    /// Move all cached imports into the current retirement epoch without
    /// destroying any image that may still be referenced by an in-flight GPU
    /// submission.
    pub(super) fn retire_all(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if self.retired.is_empty() {
            self.retired.push_back(Vec::new());
        }
        if let Some(bucket) = self.retired.back_mut() {
            for entry in self.entries.drain(..) {
                bucket.push(entry.image);
            }
        }
    }

    fn retire_image(&mut self, image: optics_gfx::Image) {
        if self.retired.is_empty() {
            self.retired.push_back(Vec::new());
        }
        if let Some(bucket) = self.retired.back_mut() {
            bucket.push(image);
        }
    }

    /// Return a cached `optics_gfx::Image` for `desc`, importing it on cache miss.
    ///
    /// On a **cache miss**, `desc.acquire_fence` is handed to
    /// `optics_gfx::Image::import_dmabuf_with_color_space` so the initial Vulkan
    /// layout transition waits on the producer, and `None` is returned as the
    /// pending acquire fence.
    ///
    /// On a **cache hit**, a duplicated `acquire_fence` (when present and not
    /// already signaled) is returned so the caller attaches it to the active
    /// canvas session via `session.wait_dmabuf_acquire(image, fence)`.
    pub(super) fn get_or_import(
        &mut self,
        device: &optics_gfx::Device,
        desc: TargetImportDesc<'_>,
    ) -> Result<(&optics_gfx::Image, Option<OwnedFd>), String> {
        let (st_dev, st_ino) =
            dmabuf_inode(desc.fd.as_raw_fd()).ok_or_else(|| "fstat dma-buf failed".to_owned())?;
        let key = TargetImportKey {
            st_dev,
            st_ino,
            width: desc.width,
            height: desc.height,
            stride: desc.stride,
            modifier: desc.modifier,
            format: desc.format,
            color_pipeline: desc.color_pipeline,
        };
        let live_fence = desc
            .acquire_fence
            .filter(|fence| !tessera_platform::sync::sync_file_is_signaled(fence.as_raw_fd()));

        if let Some(pos) = self.entries.iter().position(|entry| entry.key == key) {
            self.tick = self.tick.wrapping_add(1);
            self.entries[pos].last_used = self.tick;
            let pending_fence = match live_fence {
                Some(fence) => Some(
                    fence
                        .try_clone()
                        .map_err(|error| format!("cannot clone source acquire fence: {error}"))?,
                ),
                None => None,
            };
            return Ok((&self.entries[pos].image, pending_fence));
        }

        let fd = desc
            .fd
            .try_clone()
            .map_err(|error| format!("cannot clone source dma-buf fd: {error}"))?;
        let import_fence = match live_fence {
            Some(fence) => Some(
                fence
                    .try_clone()
                    .map_err(|error| format!("cannot clone source acquire fence: {error}"))?,
            ),
            None => None,
        };
        let color_space = match desc.color_pipeline {
            tessera_desktop::output::ColorPipeline::Hdr => Some(optics_gfx::ColorSpace::BT2020_PQ),
            _ => Some(optics_gfx::ColorSpace::SRGB),
        };
        // SAFETY: `fd` describes a valid single-plane DMA-BUF with the exact
        // dimensions, stride, format, and modifier recorded in `desc`; the
        // optional `import_fence` orders producer rendering before sampling.
        let image = unsafe {
            optics_gfx::Image::import_dmabuf_with_color_space(
                device,
                desc.width,
                desc.height,
                desc.format,
                desc.modifier,
                fd,
                0,
                desc.stride,
                import_fence,
                optics_gfx::ImageColorSpace {
                    space: color_space,
                    icc: None,
                },
            )
        }
        .map_err(|error| error.to_string())?;

        if self.entries.len() >= MAX_TARGET_IMPORTS
            && let Some(evict_idx) = select_lru_eviction_index(&self.entries)
        {
            let evicted = self.entries.swap_remove(evict_idx);
            self.retire_image(evicted.image);
        }
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        self.entries.push(TargetImportEntry {
            key,
            image,
            last_used: tick,
        });
        let last = self.entries.len() - 1;
        Ok((&self.entries[last].image, None))
    }
}

fn select_lru_eviction_index(entries: &[TargetImportEntry]) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(index, _)| index)
}

/// Query the kernel `(st_dev, st_ino)` identity of a DMA-BUF file descriptor.
pub(super) fn dmabuf_inode(fd: RawFd) -> Option<(u64, u64)> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `st` points to writable `libc::stat` storage; `fstat` validates `fd`.
    let rc = unsafe { libc::fstat(fd, st.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: `fstat` succeeded, initializing all fields of `st`.
    let st = unsafe { st.assume_init() };
    Some((st.st_dev, st.st_ino))
}

/// Retained descriptor of the most recently presented frame of a source output
/// target, held only while at least one mirror consumer references it.
///
/// The `lease` shares ownership with DRM's active scanout slot (`Rc::clone`) so
/// it never consumes an additional gfx ring slot (ADR-0250, `[INV-ARCH-160]`).
pub(super) struct RetainedSourceExport {
    pub(super) fd: OwnedFd,
    pub(super) acquire_fence: Option<OwnedFd>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: u32,
    pub(super) modifier: u64,
    pub(super) format: optics_gfx::Format,
    pub(super) color_pipeline: tessera_desktop::output::ColorPipeline,
    pub(super) origin: tessera_primitives::Point,
    pub(super) scale: f32,
    pub(super) mode: (u32, u32),
    pub(super) lease: std::rc::Rc<optics_gfx::DmabufLease>,
}

/// Export the most recently submitted target frame using the backend's
/// negotiated synchronization mode (`IN_FENCE_FD` explicit sync vs. CPU fence
/// wait).
pub(super) fn export_target_dmabuf(
    surface: &optics_gfx::Surface,
    explicit_sync: bool,
) -> Result<optics_gfx::ExportedDmabuf, optics_gfx::Error> {
    if explicit_sync {
        surface.export_dmabuf_explicit()
    } else {
        surface.export_dmabuf()
    }
}

/// Whether a gfx target acquire failure is transient (an in-flight lease or
/// GPU fence not yet retired) and must retry without destroying the target.
pub(super) fn gfx_target_acquire_is_transient(error: &optics_gfx::Error) -> bool {
    error.0 == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY
        || error.0 == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_TIMEOUT
}

/// Whether a DRM target commit failure is transient or a recoverable cursor
/// fallback and must preserve existing `OutputTarget` Vulkan rings.
pub(super) fn drm_target_commit_is_transient(error: &HostError) -> bool {
    matches!(
        error,
        HostError::Drm(
            DrmError::Busy
                | DrmError::FlipTimeout
                | DrmError::Inactive
                | DrmError::Reconfigured
                | DrmError::CursorFallback
        )
    )
}

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
        let mut recreated_sources = std::collections::BTreeSet::new();
        self.output_targets.retain(|connector, target| {
            let keep = desired
                .get(connector)
                .is_some_and(|(mode, source)| target.mode == *mode && target.source == *source);
            if !keep {
                recreated_sources.insert(connector.clone());
            }
            keep
        });
        let mirrored_sources: std::collections::BTreeSet<String> = outputs
            .iter()
            .filter_map(|output| output.role.mirror_source().map(str::to_owned))
            .collect();
        for (connector, target) in &mut self.output_targets {
            if !mirrored_sources.contains(connector) {
                target.last_source_export = None;
            }
            if recreated_sources.contains(&target.source) {
                target.import_cache.retire_all();
                target.last_mirror_cursor_rect = None;
                target.needs_present = true;
            }
        }
        self.output_target_failures
            .retain(|connector, _| desired.contains_key(connector));
        for (connector, (mode, source)) in &desired {
            if self.output_targets.contains_key(connector)
                || self.output_target_failures.get(connector) == Some(mode)
            {
                continue;
            }
            match self.host.create_render_target(self.device, mode.0, mode.1) {
                Ok(surface) => match optics_gfx::Canvas::new(&surface) {
                    Ok(canvas) => {
                        let format = surface.info().format;
                        let color_pipeline = self.host.color_pipeline();
                        self.output_target_failures.remove(connector);
                        self.output_targets.insert(
                            connector.clone(),
                            OutputTarget {
                                last_source_export: None,
                                source_lease: None,
                                import_cache: TargetImportCache::default(),
                                canvas,
                                surface,
                                mode: *mode,
                                format,
                                color_pipeline,
                                source: source.clone(),
                                needs_present: true,
                                last_mirror_cursor_rect: None,
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
        let explicit_sync = self.host.explicit_sync();
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
            let lease = std::rc::Rc::clone(&presented.lease);
            let format = gfx_format_for_drm(presented.fourcc);
            target.import_cache.begin_frame();
            let (image, pending_acquire_fence) = match target.import_cache.get_or_import(
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
            let frame = match target.surface.begin_frame_timeout_with_damage(
                TARGET_ACQUIRE_TIMEOUT_NS,
                &optics_gfx::Damage::Full,
            ) {
                Ok(frame) => frame,
                Err(error) => {
                    log::debug!(
                        "{}: mirror {connector} target frame unavailable: {error}",
                        self.host.name()
                    );
                    if !gfx_target_acquire_is_transient(&error) {
                        failed.push(connector.clone());
                    }
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
            let source = optics_gfx::CanvasRenderArea {
                x: source_rect.origin.x as f32,
                y: source_rect.origin.y as f32,
                width: source_rect.size.w.max(1) as f32,
                height: source_rect.size.h.max(1) as f32,
            };
            let submitted = (|| -> Result<optics_gfx::SubmittedFrame<'_>, String> {
                let session = target
                    .canvas
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
                            source.x / full_w,
                            source.y / full_h,
                            source.width / full_w,
                            source.height / full_h,
                        )),
                        opaque_only: true,
                        ..Default::default()
                    },
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
                frame.submit().map_err(|error| error.to_string())
            })();
            let submitted = match submitted {
                Ok(submitted) => submitted,
                Err(error) => {
                    log::warn!(
                        "{}: mirror {connector} blit failed; returning it to the union path: {error}",
                        self.host.name()
                    );
                    failed.push(connector.clone());
                    continue;
                }
            };
            let exported = match export_target_dmabuf(&target.surface, explicit_sync) {
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
                Ok(_) => {
                    if let Err(error) = submitted.present() {
                        log::warn!(
                            "{}: mirror {connector} ring advance failed: {error}",
                            self.host.name()
                        );
                        failed.push(connector.clone());
                    } else {
                        target.needs_present = false;
                        target.source_lease = Some(lease);
                    }
                }
                Err(error) if drm_target_commit_is_transient(&error) => {
                    log::debug!(
                        "{}: mirror {connector} target commit deferred: {error}",
                        self.host.name()
                    );
                }
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

/// Map the DRM fourcc of the union framebuffer to a gfx sampled format.
pub(super) fn gfx_format_for_drm(fourcc: u32) -> optics_gfx::Format {
    match fourcc {
        // DRM_FORMAT_XRGB8888 / ARGB8888: little-endian words are B,G,R,X.
        0x3432_5258 | 0x3432_5241 => optics_gfx::Format::Bgra8Unorm,
        // DRM_FORMAT_XBGR8888 / ABGR8888: little-endian words are R,G,B,X.
        0x3432_4258 | 0x3432_4241 => optics_gfx::Format::Rgba8Unorm,
        // 10-bit RGB targets use the RGB10A2 container. The 8-bit fallback
        // is conservative when a driver exposes a packed order gfx cannot
        // swizzle; the target frame will still be rejected by KMS rather than
        // showing wrong colors.
        0x3033_5258 | 0x3033_5241 | 0x3033_4258 | 0x3033_4241 => optics_gfx::Format::Rgb10a2Unorm,
        _ => optics_gfx::Format::Bgra8Unorm,
    }
}

#[cfg(test)]
mod tests {
    use super::{TargetImportKey, dmabuf_inode, gfx_format_for_drm};
    use std::os::fd::{AsRawFd, FromRawFd};

    #[test]
    fn drm_fourcc_to_gfx_format_is_byte_order_correct() {
        assert_eq!(
            gfx_format_for_drm(0x3432_5258),
            optics_gfx::Format::Bgra8Unorm
        );
        assert_eq!(
            gfx_format_for_drm(0x3432_5241),
            optics_gfx::Format::Bgra8Unorm
        );
        assert_eq!(
            gfx_format_for_drm(0x3432_4258),
            optics_gfx::Format::Rgba8Unorm
        );
        assert_eq!(
            gfx_format_for_drm(0x3432_4241),
            optics_gfx::Format::Rgba8Unorm
        );
        assert_eq!(
            gfx_format_for_drm(0x3033_5258),
            optics_gfx::Format::Rgb10a2Unorm
        );
    }

    #[test]
    fn test_inv_arch_160_dmabuf_inode_identity_tracks_dup_and_distinct_files() {
        let mut pipe_a = [0i32; 2];
        let mut pipe_b = [0i32; 2];
        // SAFETY: `pipe` initializes the two-element fd arrays.
        assert_eq!(unsafe { libc::pipe(pipe_a.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::pipe(pipe_b.as_mut_ptr()) }, 0);
        // SAFETY: Both pipes succeeded; wrap fds in OwnedFd for automatic cleanup.
        let a_read = unsafe { std::os::fd::OwnedFd::from_raw_fd(pipe_a[0]) };
        let _a_write = unsafe { std::os::fd::OwnedFd::from_raw_fd(pipe_a[1]) };
        let b_read = unsafe { std::os::fd::OwnedFd::from_raw_fd(pipe_b[0]) };
        let _b_write = unsafe { std::os::fd::OwnedFd::from_raw_fd(pipe_b[1]) };
        let a_dup = a_read.try_clone().unwrap();

        let id_a = dmabuf_inode(a_read.as_raw_fd()).unwrap();
        let id_a_dup = dmabuf_inode(a_dup.as_raw_fd()).unwrap();
        let id_b = dmabuf_inode(b_read.as_raw_fd()).unwrap();
        assert_eq!(
            id_a, id_a_dup,
            "duped fd must resolve to the same cache key"
        );
        assert_ne!(
            id_a, id_b,
            "distinct files must resolve to distinct cache keys"
        );
        assert!(dmabuf_inode(-1).is_none());

        let key1 = TargetImportKey {
            st_dev: id_a.0,
            st_ino: id_a.1,
            width: 3072,
            height: 1920,
            stride: 12288,
            modifier: 0,
            format: optics_gfx::Format::Bgra8Unorm,
            color_pipeline: tessera_desktop::output::ColorPipeline::Sdr,
        };
        let key2 = TargetImportKey {
            modifier: 1,
            ..key1
        };
        assert_ne!(key1, key2);
    }
}
