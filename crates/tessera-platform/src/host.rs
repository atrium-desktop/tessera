//! Runtime backend selection and the common compositor-host facade.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use crate::Backend;
use crate::drm::{DrmBackend, DrmError};
use crate::nested::{DEVICE_EXTENSIONS, INSTANCE_EXTENSIONS, NestedError, NestedHost};
use tessera_desktop::output::ModeSpec;
use tessera_primitives::Size;
use tessera_primitives::input::InputConfig;
use tessera_primitives::input::InputEvent;
use tessera_primitives::input::InputStatus;
use tessera_primitives::input::PointerGestureEvent;
use tessera_primitives::input::TextInputEvent;
use tessera_primitives::input::TextInputState;

/// Frame slots Flux runs concurrently. ADR-0038's frame pacing assumes three:
/// on DRM the offscreen ring must hold one image per slot so a frame being
/// rendered never aliases the image the CRTC is still scanning out (with the
/// flux default of two, rendering frame k reuses the buffer committed two
/// frames ago — exactly the one on screen until the pending flip lands).
const FRAMES_IN_FLIGHT: u32 = 3;

/// One compositor-owned cursor sprite for a KMS cursor plane. Pixels are
/// premultiplied BGRA8, which is the in-memory byte order of DRM ARGB8888 on
/// little-endian Linux.
pub struct HardwareCursor<'a> {
    pub pixels: &'a [u8],
    pub size: (u32, u32),
    pub hotspot: (u32, u32),
    /// Global physical-pixel hotspot position.
    pub position: (i32, i32),
}

/// User-selected presentation backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Use the outer Wayland compositor when one is present; otherwise use DRM.
    Auto,
    /// Drive atomic DRM/KMS directly.
    Drm,
    /// Present into an outer Wayland toplevel for development/testing.
    Nested,
}

impl FromStr for BackendKind {
    type Err = HostError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "drm" => Ok(Self::Drm),
            "nested" => Ok(Self::Nested),
            _ => Err(HostError::InvalidBackend(value.to_owned())),
        }
    }
}

/// Errors selecting, creating, or presenting through a host.
#[derive(Debug)]
pub enum HostError {
    InvalidBackend(String),
    Nested(NestedError),
    Drm(DrmError),
    Flux(optics_flux::Error),
    DrmRenderer {
        path: PathBuf,
        major: u32,
        minor: u32,
        source: optics_flux::Error,
    },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBackend(name) => {
                write!(f, "unknown backend {name:?}; expected auto, drm, or nested")
            }
            Self::Nested(err) => write!(f, "{err}"),
            Self::Drm(err) => write!(f, "{err}"),
            Self::Flux(err) => write!(f, "{err}"),
            Self::DrmRenderer {
                path,
                major,
                minor,
                source,
            } => write!(
                f,
                "cannot create a Vulkan renderer for KMS device {} ({major}:{minor}): {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Nested(err) => Some(err),
            Self::Drm(err) => Some(err),
            Self::Flux(err) => Some(err),
            Self::DrmRenderer { source, .. } => Some(source),
            Self::InvalidBackend(_) => None,
        }
    }
}

impl From<NestedError> for HostError {
    fn from(err: NestedError) -> Self {
        Self::Nested(err)
    }
}

impl From<DrmError> for HostError {
    fn from(err: DrmError) -> Self {
        Self::Drm(err)
    }
}

impl From<optics_flux::Error> for HostError {
    fn from(err: optics_flux::Error) -> Self {
        Self::Flux(err)
    }
}

/// A duplicate descriptor of the composited frame most recently presented to
/// scanout, for the zero-copy stream fan-out (IPC protocol 25). The backend
/// retains its own reference (the KMS framebuffer's GEM handle); this fd is
/// independently owned by the caller.
pub struct PresentedDmabuf {
    pub fd: OwnedFd,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub modifier: u64,
    /// DRM fourcc of the composed image (for a zero-copy blit import).
    pub fourcc: u32,
    /// Color encoding of the composed image.
    pub color_pipeline: tessera_desktop::output::ColorPipeline,
    /// Producer fence for this image, when explicit sync is active.
    pub acquire_fence: Option<OwnedFd>,
    /// Flux lease keeping the ring slot alive while a mirror target samples
    /// the image. The lease is released with the target's next replacement.
    pub lease: std::rc::Rc<optics_flux::DmabufLease>,
}

/// One mode-sized output target ready for atomic KMS presentation.
///
/// Sharing `lease` via `Rc` lets the compositor retain a source target's
/// currently scanned-out image for cursor-only mirror re-blits without holding
/// a second Flux ring slot (ADR-0250, `[INV-ARCH-160]`).
pub struct RenderTargetCommit {
    pub connector: String,
    pub dmabuf: optics_flux::SurfaceDmabuf,
    pub lease: std::rc::Rc<optics_flux::DmabufLease>,
}

impl RenderTargetCommit {
    pub fn from_exported(connector: String, exported: optics_flux::ExportedDmabuf) -> Self {
        Self {
            connector,
            dmabuf: exported.dmabuf,
            lease: std::rc::Rc::new(exported.lease),
        }
    }
}

/// Format/modifier pair accepted by an overlay plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayFormatInfo {
    pub fourcc: u32,
    pub modifiers: Vec<u64>,
}

/// Compositor-visible capabilities of one KMS overlay plane.
///
/// This is deliberately a value snapshot: the composition root can plan a
/// candidate without learning about DRM property handles, and the backend
/// still owns the complete-set `TEST_ONLY` proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlaneInfo {
    /// Connector name of the output whose CRTC this plane can drive.
    pub output: String,
    pub formats: Vec<OverlayFormatInfo>,
    pub zpos: Option<(u64, u64)>,
    pub alpha: bool,
    pub in_fence_fd: bool,
    pub fb_damage_clips: bool,
}

impl OverlayPlaneInfo {
    pub fn supports(&self, fourcc: u32, modifier: u64) -> bool {
        self.formats
            .iter()
            .any(|format| format.fourcc == fourcc && format.modifiers.contains(&modifier))
    }
}

/// One proposed overlay output layer for a complete-set `TEST_ONLY` probe.
///
/// `target` is the physical destination rectangle in output-local coordinates.
/// The backend imports the client buffer, programs the overlay plane, and
/// validates primary + cursor + overlay together; it never presents.
pub struct OverlayLayer<'a> {
    pub surface: &'a tessera_primitives::SurfaceDmabuf,
    pub output: &'a str,
    pub target: tessera_primitives::Rect,
    pub zpos: Option<u32>,
}

/// One runtime-selected compositor host.
// There is exactly one process-lifetime Host. Keeping both backends inline
// avoids an allocation and makes ownership explicit; enum stack size is not a
// per-frame cost.
#[allow(clippy::large_enum_variant)]
pub enum Host {
    Nested(NestedHost),
    Drm(DrmBackend),
}

impl Host {
    /// Open the selected backend. `configured_modes` carries the config's
    /// per-connector `mode` requests (ADR-0028); only the DRM path consumes
    /// them, so the very first modeset already honors the configured modes.
    /// `configured_color` carries the per-connector `hdr` / `deep_color`
    /// policy and `configured_icc` the ICC profile paths, likewise DRM-only.
    pub fn open(
        kind: BackendKind,
        title: &str,
        width: i32,
        height: i32,
        configured_modes: HashMap<String, ModeSpec>,
        configured_color: HashMap<String, tessera_desktop::output::ColorPolicy>,
        configured_icc: HashMap<String, String>,
        configured_topology: tessera_desktop::output::DisplayTopology,
        configured_policies: HashMap<String, tessera_desktop::output::OutputPolicy>,
    ) -> Result<Self, HostError> {
        match kind {
            BackendKind::Nested => Ok(Self::Nested(NestedHost::open(title, width, height)?)),
            BackendKind::Drm => Ok(Self::Drm(DrmBackend::open(
                configured_modes,
                configured_color,
                configured_icc,
                configured_topology,
                configured_policies,
            )?)),
            BackendKind::Auto => {
                let outer_wayland =
                    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
                if outer_wayland {
                    log::info!("backend: auto selected nested (WAYLAND_DISPLAY is set)");
                    Ok(Self::Nested(NestedHost::open(title, width, height)?))
                } else {
                    log::info!("backend: auto selected drm (no outer Wayland display)");
                    Ok(Self::Drm(DrmBackend::open(
                        configured_modes,
                        configured_color,
                        configured_icc,
                        configured_topology,
                        configured_policies,
                    )?))
                }
            }
        }
    }

    /// Create the Vulkan device with backend-mandatory extensions. dma-buf is
    /// optional for nested development, but mandatory for direct scanout.
    pub fn create_device(&self) -> Result<optics_flux::Device, HostError> {
        match self {
            Self::Nested(_) => {
                let device = optics_flux::Device::new_with_options(optics_flux::DeviceOptions {
                    headless: false,
                    instance_extensions: &INSTANCE_EXTENSIONS,
                    device_extensions: &DEVICE_EXTENSIONS,
                    frames_in_flight: FRAMES_IN_FLIGHT,
                    optional_features: optics_flux::DeviceFeatures::DMABUF
                        | optics_flux::DeviceFeatures::DMABUF_SYNC_FILE,
                    ..optics_flux::DeviceOptions::default()
                })?;
                let enabled = device.enabled_features();
                if !enabled.contains(optics_flux::DeviceFeatures::DMABUF) {
                    log::warn!(
                        "nested: dma-buf Vulkan capabilities unavailable; using swapchain-only device"
                    );
                } else if !enabled.contains(optics_flux::DeviceFeatures::DMABUF_SYNC_FILE) {
                    log::warn!(
                        "nested: explicit dma-buf sync unavailable; using implicit synchronization"
                    );
                }
                Ok(device)
            }
            Self::Drm(host) => {
                let (kms_path, drm_node) = host.kms_device()?;
                log::info!(
                    "drm: constraining Vulkan renderer to KMS device {} ({}:{})",
                    kms_path.display(),
                    drm_node.major,
                    drm_node.minor
                );
                let device = optics_flux::Device::new_with_options(optics_flux::DeviceOptions {
                    headless: true,
                    frames_in_flight: FRAMES_IN_FLIGHT,
                    drm_node: Some(drm_node),
                    required_features: optics_flux::DeviceFeatures::DMABUF,
                    optional_features: optics_flux::DeviceFeatures::DMABUF_SYNC_FILE,
                    ..optics_flux::DeviceOptions::default()
                })
                .map_err(|source| HostError::DrmRenderer {
                    path: kms_path.clone(),
                    major: drm_node.major,
                    minor: drm_node.minor,
                    source,
                })?;
                if !device
                    .enabled_features()
                    .contains(optics_flux::DeviceFeatures::DMABUF_SYNC_FILE)
                {
                    log::warn!("drm: explicit dma-buf sync unavailable; using the fence-wait path");
                }
                log::info!(
                    "drm: Vulkan renderer matched KMS device {} ({}:{})",
                    kms_path.display(),
                    drm_node.major,
                    drm_node.minor
                );
                Ok(device)
            }
        }
    }

    pub fn create_surface(
        &mut self,
        device: &optics_flux::Device,
    ) -> Result<optics_flux::Surface, HostError> {
        match self {
            Self::Nested(host) => {
                let vk_surface = host.create_vk_surface(device)?;
                let (width, height) = host.physical_size();
                // SAFETY: NestedHost created this surface from device's Vulkan
                // instance and retains it for at least as long as Host.
                // Use non-blocking presentation (`vsync = false`, selecting
                // `VK_PRESENT_MODE_MAILBOX_KHR` on Wayland WSI): `PresentationScheduler`
                // already paces nested submissions to the outer output's refresh
                // interval (`paced: true`), and `NestedHost::dispatch_host_events`
                // absorbs WSI-internal `wl_buffer.release` reads without waking
                // the render loop early. Avoiding Mesa's synchronous FIFO
                // `wl_surface.frame` wait prevents the 1.000s `vkQueuePresentKHR`
                // stall when the nested window is occluded or unfocused.
                Ok(unsafe {
                    optics_flux::Surface::from_vk(device, vk_surface, width, height, false)?
                })
            }
            Self::Drm(host) => Ok(host.create_surface(device)?),
        }
    }

    pub fn present(
        &mut self,
        surface: &optics_flux::Surface,
        frame: optics_flux::SubmittedFrame<'_>,
        damage: Option<&[tessera_primitives::Rect]>,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(host) => {
                // The outer compositor owns scanout and does its own damage
                // tracking; the hint is DRM/KMS-only. Request a Wayland
                // `wl_surface.frame` callback before `vkQueuePresentKHR`
                // commits our surface so `presentation_pending()` tracks the
                // outer compositor's real vblank non-blockingly.
                let _ = damage;
                host.arm_frame_callback();
                if let Err(error) = frame.present() {
                    host.cancel_frame_callback();
                    return Err(error.into());
                }
                Ok(None)
            }
            Self::Drm(host) => Ok(host.present(surface, frame, damage)?),
        }
    }

    /// Record and submit a composite frame without a KMS scanout commit.
    ///
    /// Capture/stream frames use the composed image as a source while their
    /// per-output targets perform the actual presentation. The retained
    /// descriptor and lease keep the composed image alive for those consumers.
    pub fn compose_only(
        &mut self,
        surface: &optics_flux::Surface,
        frame: optics_flux::SubmittedFrame<'_>,
        damage: Option<&[tessera_primitives::Rect]>,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(host) => {
                let _ = damage;
                host.arm_frame_callback();
                if let Err(error) = frame.present() {
                    host.cancel_frame_callback();
                    return Err(error.into());
                }
                Ok(None)
            }
            Self::Drm(host) => Ok(host.compose_only(surface, frame)?),
        }
    }

    /// Present the composed desktop to an explicit subset of outputs.
    /// Outputs not named keep their current scanout; nested presentation owns
    /// a single host surface and ignores the target list.
    pub fn present_to(
        &mut self,
        surface: &optics_flux::Surface,
        frame: optics_flux::SubmittedFrame<'_>,
        damage: Option<&[tessera_primitives::Rect]>,
        targets: &[&str],
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(host) => {
                let _ = targets;
                host.arm_frame_callback();
                if let Err(error) = frame.present() {
                    host.cancel_frame_callback();
                    return Err(error.into());
                }
                Ok(None)
            }
            Self::Drm(host) => Ok(host.present_to(surface, frame, damage, targets)?),
        }
    }

    /// Present the composed desktop and one overlay client buffer together.
    /// Nested presentation owns no overlay plane and rejects the request; the
    /// runtime then composites normally.
    pub fn present_with_overlay(
        &mut self,
        surface: &optics_flux::Surface,
        frame: optics_flux::SubmittedFrame<'_>,
        damage: Option<&[tessera_primitives::Rect]>,
        overlay: OverlayLayer<'_>,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.present_with_overlay(surface, frame, damage, overlay)?),
        }
    }

    /// Duplicate the descriptor of the composited frame most recently
    /// presented to scanout, so zero-copy stream consumers can import exactly
    /// what is on screen (IPC protocol 25). `None` on the nested backend
    /// (its swapchain is never dma-buf exportable) and whenever no composited
    /// frame is on screen (startup, direct client scanout).
    pub fn presented_dmabuf(&self) -> Option<PresentedDmabuf> {
        match self {
            Self::Nested(_) => None,
            Self::Drm(host) => host.presented_dmabuf(),
        }
    }

    /// Create an exportable offscreen surface for a compositor-owned output
    /// target. Only the DRM backend owns such targets; nested presentation
    /// keeps its single host swapchain.
    pub fn create_render_target(
        &self,
        device: &optics_flux::Device,
        width: u32,
        height: u32,
    ) -> Result<optics_flux::Surface, HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.create_render_target(device, width, height)?),
        }
    }

    /// Present an exported compositor target at its native extent, with no
    /// KMS plane scaling. The target surface is already mode-sized.
    pub fn present_render_target(
        &mut self,
        exported: optics_flux::ExportedDmabuf,
        target: &str,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.present_render_target(exported, target)?),
        }
    }

    /// Present a coherent group of mode-sized compositor targets in one
    /// atomic commit. All CRTCs in the group flip together; groups remain
    /// independent of one another. Returns KMS `OUT_FENCE_PTR` when reclaiming
    /// a direct-scanned client plane (ADR-0250, `[INV-ARCH-159]`).
    pub fn present_render_targets(
        &mut self,
        targets: Vec<RenderTargetCommit>,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.present_render_targets(targets)?),
        }
    }

    /// Source rectangle of one output inside the composed union framebuffer.
    /// Used to crop a mirror target from the source output's slice.
    pub fn output_source_rect(&self, connector: &str) -> Option<tessera_primitives::Rect> {
        match self {
            Self::Nested(_) => None,
            Self::Drm(host) => host.output_source_rect(connector),
        }
    }

    /// Page-flip one compositor-selected full-output client buffer directly,
    /// bypassing the composite. Eligibility is based on physical coverage and
    /// opacity rather than an xdg fullscreen state bit.
    /// Nested mode never owns scanout, so it reports unsupported (the runtime
    /// then composites instead).
    pub fn present_scanout(
        &mut self,
        candidate: &tessera_primitives::SurfaceDmabuf,
        target: &str,
        damage: Option<&[tessera_primitives::Rect]>,
    ) -> Result<Option<OwnedFd>, HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.present_scanout(candidate, target, damage)?),
        }
    }

    /// Whether one output's primary-plane slot is still waiting for its page
    /// flip. The runtime skips that target for composite presentation instead
    /// of blocking every other output.
    pub fn output_presentation_pending(&self, connector: &str) -> bool {
        match self {
            Self::Nested(_) => false,
            Self::Drm(host) => host.output_presentation_pending(connector),
        }
    }

    /// Whether compositor render-target exports should use explicit sync fences.
    pub fn explicit_sync(&self) -> bool {
        match self {
            Self::Nested(_) => false,
            Self::Drm(host) => host.explicit_sync(),
        }
    }

    pub fn present_cursor(&mut self) -> Result<(), HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.present_cursor()?),
        }
    }

    /// Confirm completion of the most recently submitted secure frame.
    /// Direct DRM has an exact per-CRTC page-flip barrier. Nested mode is a
    /// development host and can only wait for completion of our Vulkan work;
    /// the outer compositor remains the physical presentation authority.
    pub fn wait_presented(&mut self, device: &optics_flux::Device) -> Result<(), HostError> {
        match self {
            Self::Nested(_) => {
                device.wait_idle();
                Ok(())
            }
            Self::Drm(host) => Ok(host.wait_presented()?),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Nested(_) => "nested",
            Self::Drm(_) => "drm",
        }
    }

    /// Whether presentation goes through a host-owned swapchain (Wayland WSI)
    /// rather than the compositor's own exportable offscreen ring.
    ///
    /// The distinction decides what a frame-acquire timeout can mean. A
    /// swapchain acquire can block because the outer compositor withholds every
    /// image (occluded or suspended window); an offscreen ring has no acquire at
    /// all, so a timeout can only mean the GPU has not retired a slot fence.
    pub fn presents_through_host_swapchain(&self) -> bool {
        matches!(self, Self::Nested(_))
    }

    /// Whether the host still owns an in-flight presentation batch.
    pub fn presentation_pending(&self) -> bool {
        match self {
            Self::Nested(host) => host.presentation_pending(),
            Self::Drm(host) => host.presentation_pending(),
        }
    }

    /// Whether the cursor must be painted into the primary framebuffer.
    /// Direct DRM normally uses dedicated KMS cursor planes; it falls back to
    /// software composition if any active output lacks a compatible plane or
    /// cursor framebuffer allocation failed.
    pub fn uses_software_cursor(&self) -> bool {
        matches!(self, Self::Drm(host) if !host.hardware_cursor_supported())
    }

    pub fn supports_hardware_cursor(&self) -> bool {
        matches!(self, Self::Drm(host) if host.hardware_cursor_supported())
    }

    pub fn disable_hardware_cursor(&mut self) {
        if let Self::Drm(host) = self {
            host.disable_hardware_cursor();
        }
    }

    /// Re-arm a disabled hardware cursor plane once its failure backoff has
    /// elapsed. The next ordinary cursor commit is the probe: a renewed
    /// rejection disables the plane again with a longer backoff, and a
    /// successful cursor-active commit resets the failure count. Nested
    /// presentation owns no KMS planes, so there is nothing to retry.
    pub fn poll_hardware_cursor_retry(&mut self) -> bool {
        match self {
            Self::Nested(_) => false,
            Self::Drm(host) => host.poll_hardware_cursor_retry(),
        }
    }

    /// Reclaim scanout ownership after a lost page-flip event. Nested
    /// presentation keeps no kernel flip bookkeeping, so there is nothing
    /// to recover.
    pub fn recover_lost_presentation(&mut self) {
        if let Self::Drm(host) = self {
            host.recover_lost_presentation();
        }
    }

    /// Update the cursor-plane sprite and placement. `None` disables it.
    /// Returns whether hardware presentation remains available so the caller
    /// can paint a software fallback in the same frame after any failure.
    pub fn set_hardware_cursor(&mut self, cursor: Option<HardwareCursor<'_>>) -> bool {
        match self {
            Self::Nested(_) => false,
            Self::Drm(host) => {
                if !host.hardware_cursor_supported() {
                    return false;
                }
                if let Err(error) = host.set_hardware_cursor(cursor) {
                    log::warn!("drm: hardware cursor failed ({error}); using composited cursor");
                    host.disable_hardware_cursor();
                    false
                } else {
                    true
                }
            }
        }
    }

    /// Whether a client dma-buf `(fourcc, modifier)` can be scanned out
    /// directly on the active DRM primary planes. Nested mode never owns
    /// scanout (the outer compositor does), so it reports `false`.
    pub fn supports_scanout(&self, fourcc: u32, modifier: u64) -> bool {
        match self {
            Self::Nested(_) => false,
            Self::Drm(host) => host.supports_scanout(fourcc, modifier),
        }
    }

    /// Enable the explicit overlay hardware-validation path. Nested
    /// presentation has no KMS planes to enable.
    pub fn set_overlay_offload_enabled(&mut self, enabled: bool) {
        if let Self::Drm(host) = self {
            host.set_overlay_offload_enabled(enabled);
        }
    }

    /// Overlay planes discovered on the active DRM outputs, as value
    /// snapshots. Nested presentation owns no KMS planes and returns empty.
    pub fn overlay_planes(&self) -> Vec<OverlayPlaneInfo> {
        match self {
            Self::Nested(_) => Vec::new(),
            Self::Drm(host) => host.overlay_planes(),
        }
    }

    /// Validate one overlay layer against Primary and Cursor in one atomic
    /// complete-set `TEST_ONLY` request. No pixels change, no page flip is
    /// armed, and the imported client buffer is released immediately.
    /// Requires the compositor framebuffer on Primary (ADR-0237). Nested
    /// presentation owns no KMS planes.
    pub fn probe_overlay(&mut self, layer: &OverlayLayer<'_>) -> Result<(), HostError> {
        match self {
            Self::Nested(_) => Err(HostError::Drm(DrmError::ScanoutUnsupported)),
            Self::Drm(host) => Ok(host.probe_overlay(layer)?),
        }
    }

    /// Format/modifier intersection accepted by every active DRM primary
    /// plane. Nested mode has no local KMS planes and therefore no scanout
    /// tranche to advertise.
    pub fn dmabuf_scanout_formats(&self) -> Vec<tessera_primitives::dmabuf::DmabufFormat> {
        match self {
            Self::Nested(_) => Vec::new(),
            Self::Drm(host) => host.dmabuf_scanout_formats(),
        }
    }

    /// Linux `dev_t` of the KMS node targeted by the SCANOUT feedback tranche.
    pub fn dmabuf_scanout_device(&self) -> Option<u64> {
        match self {
            Self::Nested(_) => None,
            Self::Drm(host) => host.dmabuf_feedback_device(),
        }
    }

    /// Preferred DRM device for linux-dmabuf v4 feedback.
    ///
    /// Query the physical device Flux actually selected, so Mesa clients use
    /// the same GPU even in nested and multi-GPU configurations. Direct DRM
    /// can fall back to its KMS primary node when a Vulkan driver lacks
    /// `VK_EXT_physical_device_drm`.
    pub fn dmabuf_feedback_device(&self, device: &optics_flux::Device) -> Option<u64> {
        if let Some(device) = vulkan_dmabuf_feedback_device(device) {
            return Some(device);
        }
        match self {
            Self::Nested(_) => None,
            Self::Drm(host) => host.dmabuf_feedback_device(),
        }
    }
}

fn vulkan_dmabuf_feedback_device(device: &optics_flux::Device) -> Option<u64> {
    let Some(identity) = device.drm_identity() else {
        log::warn!("dmabuf: Vulkan device exposes no DRM identity; using backend fallback");
        return None;
    };
    let (kind, node) = identity
        .render
        .map(|node| ("render", node))
        .or_else(|| identity.primary.map(|node| ("primary", node)))?;
    log::info!(
        "dmabuf: v4 main device is Vulkan {kind} node {}:{}",
        node.major,
        node.minor
    );
    Some(libc::makedev(node.major, node.minor))
}

impl Backend for Host {
    fn size(&self) -> Size {
        match self {
            Self::Nested(host) => host.size(),
            Self::Drm(host) => host.size(),
        }
    }

    fn physical_size(&self) -> (u32, u32) {
        match self {
            Self::Nested(host) => host.physical_size(),
            Self::Drm(host) => host.physical_size(),
        }
    }

    fn scale(&self) -> f32 {
        match self {
            Self::Nested(host) => host.scale(),
            Self::Drm(host) => host.scale(),
        }
    }

    fn size_u32(&self) -> (u32, u32) {
        match self {
            Self::Nested(host) => host.size_u32(),
            Self::Drm(host) => host.size_u32(),
        }
    }

    fn output_infos(&self) -> Vec<tessera_desktop::output::OutputInfo> {
        match self {
            Self::Nested(host) => host.output_infos(),
            Self::Drm(host) => host.output_infos(),
        }
    }

    fn set_configured_topology(
        &mut self,
        topology: tessera_desktop::output::DisplayTopology,
        policies: HashMap<String, tessera_desktop::output::OutputPolicy>,
    ) {
        match self {
            Self::Nested(host) => host.set_configured_topology(topology, policies),
            Self::Drm(host) => host.set_configured_topology(topology, policies),
        }
    }

    fn set_configured_modes(&mut self, modes: HashMap<String, ModeSpec>) {
        match self {
            Self::Nested(host) => host.set_configured_modes(modes),
            Self::Drm(host) => host.set_configured_modes(modes),
        }
    }

    fn set_configured_color_policies(
        &mut self,
        policies: HashMap<String, tessera_desktop::output::ColorPolicy>,
    ) {
        match self {
            Self::Nested(host) => host.set_configured_color_policies(policies),
            Self::Drm(host) => host.set_configured_color_policies(policies),
        }
    }

    fn set_configured_icc_profiles(&mut self, profiles: HashMap<String, String>) {
        match self {
            Self::Nested(host) => host.set_configured_icc_profiles(profiles),
            Self::Drm(host) => host.set_configured_icc_profiles(profiles),
        }
    }

    fn set_gamma_gains(&mut self, gains: Option<[f32; 3]>) {
        match self {
            Self::Nested(host) => host.set_gamma_gains(gains),
            Self::Drm(host) => host.set_gamma_gains(gains),
        }
    }

    fn color_pipeline(&self) -> tessera_desktop::output::ColorPipeline {
        match self {
            Self::Nested(host) => host.color_pipeline(),
            Self::Drm(host) => host.color_pipeline(),
        }
    }

    fn set_input_config(&mut self, config: InputConfig) -> InputStatus {
        match self {
            Self::Nested(host) => host.set_input_config(config),
            Self::Drm(host) => host.set_input_config(config),
        }
    }

    fn input_status(&self) -> InputStatus {
        match self {
            Self::Nested(host) => host.input_status(),
            Self::Drm(host) => host.input_status(),
        }
    }

    fn dispatch(&mut self) -> bool {
        match self {
            Self::Nested(host) => host.dispatch(),
            Self::Drm(host) => host.dispatch(),
        }
    }

    fn dispatch_nonblocking(&mut self) -> bool {
        match self {
            Self::Nested(host) => host.dispatch_nonblocking(),
            Self::Drm(host) => host.dispatch_nonblocking(),
        }
    }

    fn dispatch_timeout(&mut self, timeout: Duration) -> bool {
        match self {
            Self::Nested(host) => host.dispatch_timeout(timeout),
            Self::Drm(host) => host.dispatch_timeout(timeout),
        }
    }

    fn set_wakeup_fd(&mut self, fd: std::os::fd::RawFd) {
        match self {
            Self::Nested(host) => host.set_wakeup_fd(fd),
            Self::Drm(host) => host.set_wakeup_fd(fd),
        }
    }

    fn take_input(&mut self) -> Vec<InputEvent> {
        match self {
            Self::Nested(host) => host.take_input(),
            Self::Drm(host) => host.take_input(),
        }
    }

    fn take_resize(&mut self) -> Option<Size> {
        match self {
            Self::Nested(host) => host.take_resize(),
            Self::Drm(host) => host.take_resize(),
        }
    }

    fn surface_needs_recreate(&self) -> bool {
        match self {
            Self::Nested(host) => host.surface_needs_recreate(),
            Self::Drm(host) => host.surface_needs_recreate(),
        }
    }

    fn set_text_input_state(&mut self, state: TextInputState) {
        match self {
            Self::Nested(host) => host.set_text_input_state(state),
            Self::Drm(host) => host.set_text_input_state(state),
        }
    }

    fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        match self {
            Self::Nested(host) => host.take_text_input(),
            Self::Drm(host) => host.take_text_input(),
        }
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        match self {
            Self::Nested(host) => host.take_pointer_gestures(),
            Self::Drm(host) => host.take_pointer_gestures(),
        }
    }

    fn set_cursor_shape(&mut self, shape: u32) {
        match self {
            Self::Nested(host) => host.set_cursor_shape(shape),
            Self::Drm(host) => host.set_cursor_shape(shape),
        }
    }

    fn hide_cursor(&mut self) {
        match self {
            Self::Nested(host) => host.hide_cursor(),
            Self::Drm(host) => host.hide_cursor(),
        }
    }

    fn set_buffer_scale(&self) {
        match self {
            Self::Nested(host) => host.set_buffer_scale(),
            Self::Drm(host) => host.set_buffer_scale(),
        }
    }

    fn is_active(&self) -> bool {
        match self {
            Self::Nested(host) => host.is_active(),
            Self::Drm(host) => host.is_active(),
        }
    }

    fn outputs_powered(&self) -> bool {
        match self {
            Self::Nested(_) => true,
            Self::Drm(host) => host.outputs_powered(),
        }
    }

    fn presentation_target_ready(&self) -> bool {
        match self {
            Self::Nested(_) => true,
            Self::Drm(host) => host.presentation_target_ready(),
        }
    }

    fn set_outputs_powered(&mut self, powered: bool) -> Result<(), String> {
        match self {
            Self::Nested(_) if powered => Ok(()),
            Self::Nested(_) => Err("output power control is unavailable in nested mode".into()),
            Self::Drm(host) => host
                .set_outputs_powered(powered)
                .map_err(|error| error.to_string()),
        }
    }

    fn switch_vt(&mut self, vt: i32) {
        match self {
            // The nested backend shares the host's session; VT switching is
            // the host compositor's business.
            Self::Nested(_) => {}
            Self::Drm(host) => host.switch_vt(vt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_parser_is_strict() {
        assert_eq!("auto".parse::<BackendKind>().unwrap(), BackendKind::Auto);
        assert_eq!("drm".parse::<BackendKind>().unwrap(), BackendKind::Drm);
        assert_eq!(
            "nested".parse::<BackendKind>().unwrap(),
            BackendKind::Nested
        );
        assert!("x11".parse::<BackendKind>().is_err());
    }

    #[test]
    fn overlay_plane_support_matches_format_and_modifier_pair() {
        let plane = OverlayPlaneInfo {
            output: "DP-1".into(),
            formats: vec![OverlayFormatInfo {
                fourcc: 0x34325258,
                modifiers: vec![0, 7],
            }],
            zpos: Some((1, 3)),
            alpha: true,
            in_fence_fd: true,
            fb_damage_clips: false,
        };
        assert!(plane.supports(0x34325258, 0));
        assert!(plane.supports(0x34325258, 7));
        assert!(!plane.supports(0x34325258, 9));
        assert!(!plane.supports(0x34325241, 0));
    }

    #[test]
    fn drm_transient_errors_stay_matchable_through_host_error() {
        // main.rs treats exactly these as skip-the-frame conditions; pinning
        // the shape keeps a future enum edit from silently making them fatal.
        for error in [
            DrmError::Busy,
            DrmError::FlipTimeout,
            DrmError::Inactive,
            DrmError::Reconfigured,
        ] {
            let host_error = HostError::from(error);
            assert!(matches!(
                host_error,
                HostError::Drm(
                    DrmError::Busy
                        | DrmError::FlipTimeout
                        | DrmError::Inactive
                        | DrmError::Reconfigured
                )
            ));
        }
    }
}
