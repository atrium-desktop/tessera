//! Direct-display backend built on atomic DRM/KMS, libseat, and libinput.
//!
//! Flux renders into exportable offscreen Vulkan images. Each completed image
//! is imported as a DRM framebuffer and attached to the primary plane with an
//! atomic commit. The device runs three in-flight frame slots (see
//! `host::FRAMES_IN_FLIGHT`), so the slot a new frame renders into was retired
//! from scanout one flip earlier — rendering never aliases the image the CRTC
//! is still scanning. `pending_flips` exposes that ownership boundary to the
//! runtime presentation state machine: event/input dispatch remains live, but
//! a second atomic batch cannot start until every CRTC retires the first one.
//!
//! KMS planes are *output layers*, not scene layers (ADR-0234). The background
//! and always-on-top are compositor z-order semantics; only primary, cursor,
//! and a policy-enabled, `TEST_ONLY`-proven overlay may allocate a plane
//! (ADR-0235). Plane discovery and allocation live in `planes`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::RangeBounds;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

mod device;
mod events;
mod output;
mod planes;

use device::PresentedComposite;
use output::*;
use planes::*;

use drm::buffer::{Buffer, DrmFourcc, DrmModifier, Handle as BufferHandle, PlanarBuffer};
use drm::control::{
    self, AtomicCommitFlags, Device as ControlDevice, FbCmd2Flags, Mode, ModeTypeFlags,
    ResourceHandle, atomic, connector, crtc, plane, property,
};
use drm::{Device as BasicDevice, DriverCapability};
use input::event::EventTrait;
use input::event::device::DeviceEvent;
use input::event::gesture::{
    GestureEndEvent, GestureEvent, GestureEventCoordinates, GestureEventTrait, GestureHoldEvent,
    GesturePinchEvent, GesturePinchEventTrait, GestureSwipeEvent,
};
use input::event::keyboard::{KeyboardEvent, KeyboardEventTrait};
#[allow(deprecated)]
use input::event::pointer::{
    Axis, AxisSource, PointerAxisEvent, PointerEvent, PointerEventTrait, PointerScrollEvent,
    PointerScrollWheelEvent,
};
use input::event::tablet_tool::{
    ProximityState, TabletToolButtonEvent, TabletToolEvent, TabletToolEventTrait, TabletToolType,
    TipState,
};
use input::event::touch::{TouchEvent, TouchEventPosition, TouchEventSlot};
use input::{
    Device, DeviceCapability, DeviceConfigResult, DragLockState, Event, Libinput,
    LibinputInterface, ScrollMethod,
};
use tessera_desktop::output::ColorPolicy;
use tessera_desktop::output::ModeSpec;
use tessera_desktop::output::OutputKind;
use tessera_desktop::output::OutputMode;
use tessera_desktop::output::Scale;
use tessera_desktop::output::automatic_scale;
use tessera_desktop::output::physical_ppi;
use tessera_primitives::Size;
use tessera_primitives::input::ButtonState;
use tessera_primitives::input::InputEvent;
use tessera_primitives::input::InputStatus;
use tessera_primitives::input::MouseCapabilities;
use tessera_primitives::input::MouseConfig;
use tessera_primitives::input::MouseStatus;
use tessera_primitives::input::PointerAxis;
use tessera_primitives::input::PointerAxisFrame;
use tessera_primitives::input::PointerAxisRelativeDirection;
use tessera_primitives::input::PointerAxisSource;
use tessera_primitives::input::PointerGestureEvent;
use tessera_primitives::input::TabletEvent;
use tessera_primitives::input::TabletToolInfo;
use tessera_primitives::input::TouchpadCapabilities;
use tessera_primitives::input::TouchpadConfig;
use tessera_primitives::input::TouchpadScrollMethod;
use tessera_primitives::input::TouchpadStatus;

use crate::Backend;

/// Errors that prevent direct-display operation.
#[derive(Debug)]
pub enum DrmError {
    Seat(String),
    NoCard(String),
    Io(std::io::Error),
    NoConnector,
    NoCrtc,
    NoPlane,
    MalformedFormats(&'static str),
    DesktopTooLarge(u32, u32),
    MissingProperty(&'static str),
    InputSeat(String),
    Flux(flux::Error),
    DmabufUnsupported,
    Inactive,
    Busy,
    FlipTimeout,
    Reconfigured,
    ScanoutUnsupported,
    CursorUnsupported(&'static str),
    CursorFallback,
}

impl std::fmt::Display for DrmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seat(msg) => write!(f, "libseat: {msg}"),
            Self::NoCard(tried) => write!(f, "no usable DRM card was found (tried {tried})"),
            Self::Io(err) => write!(f, "DRM/KMS: {err}"),
            Self::NoConnector => f.write_str("no connected DRM connector with a display mode"),
            Self::NoCrtc => f.write_str("connector has no compatible CRTC"),
            Self::NoPlane => f.write_str(
                "CRTC has no compatible primary plane supporting XRGB8888/ARGB8888 dma-buf scanout",
            ),
            Self::MalformedFormats(msg) => write!(f, "DRM IN_FORMATS blob is malformed: {msg}"),
            Self::DesktopTooLarge(w, h) => {
                write!(f, "combined output framebuffer {w}x{h} exceeds DRM limits")
            }
            Self::MissingProperty(prop) => {
                write!(f, "DRM object is missing required atomic property {prop}")
            }
            Self::InputSeat(seat) => write!(f, "libinput could not bind seat {seat}"),
            Self::Flux(err) => write!(f, "Flux: {err}"),
            Self::DmabufUnsupported => {
                f.write_str("Flux could not create exportable dma-buf render targets")
            }
            Self::Inactive => f.write_str("DRM session is inactive"),
            Self::Busy => f.write_str("previous DRM atomic commit is still being cleaned up"),
            Self::FlipTimeout => f.write_str("timed out waiting for a KMS page flip"),
            Self::Reconfigured => {
                f.write_str("display set changed during presentation; frame skipped")
            }
            Self::ScanoutUnsupported => f.write_str("client buffer cannot be scanned out directly"),
            Self::CursorUnsupported(msg) => write!(f, "hardware cursor unsupported: {msg}"),
            Self::CursorFallback => f.write_str(
                "hardware cursor atomic commit was rejected; retry with software composition",
            ),
        }
    }
}

impl std::error::Error for DrmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Flux(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DrmError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<flux::Error> for DrmError {
    fn from(err: flux::Error) -> Self {
        Self::Flux(err)
    }
}

/// Map an atomic-commit failure to a backend error. EBUSY is an allowed result
/// for a non-blocking atomic update while an earlier update is still pending;
/// a page-flip event marks `flip_done`, which can precede the kernel's terminal
/// `cleanup_done`. EACCES/EPERM means the session lost DRM master to a VT
/// switch whose seat Disable event has not been dispatched yet. All three are
/// transient frame-skip conditions and must never kill the compositor.
fn commit_error(error: std::io::Error) -> DrmError {
    match error.raw_os_error() {
        Some(libc::EBUSY) => DrmError::Busy,
        Some(libc::EACCES) | Some(libc::EPERM) => {
            log::warn!("drm: commit while masterless (VT switch in flight); skipping frame");
            DrmError::Inactive
        }
        _ => DrmError::Io(error),
    }
}

fn commit_error_is_transient(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EBUSY | libc::EACCES | libc::EPERM)
    )
}

/// Clip desktop-wide damage rectangles (physical framebuffer pixels) to one
/// output's scanout rectangle, yielding the `drm_mode_rect` fields
/// `(x1, y1, x2, y2)` for each `FB_DAMAGE_CLIPS` entry. `None` damage means
/// "unknown" and covers the whole output; an empty result slice means the
/// output is untouched by this frame's damage. Each input rect is clipped
/// independently so disjoint dirty regions (e.g. a video window plus a clock
/// tick) survive as separate clips instead of being unioned into one spanning
/// rect — which would defeat PSR2 / panel self-refresh.
fn damage_clip_for_output(
    damage: Option<&[tessera_primitives::Rect]>,
    output_x: u32,
    output_y: u32,
    width: u32,
    height: u32,
) -> Vec<[i32; 4]> {
    let (ox, oy) = (output_x as i32, output_y as i32);
    let full = [ox, oy, ox + width as i32, oy + height as i32];
    let Some(rects) = damage else {
        return vec![full];
    };
    let mut out = Vec::with_capacity(rects.len());
    for damage in rects {
        let x1 = damage.origin.x.max(ox);
        let y1 = damage.origin.y.max(oy);
        let x2 = (damage.origin.x.saturating_add(damage.size.w)).min(full[2]);
        let y2 = (damage.origin.y.saturating_add(damage.size.h)).min(full[3]);
        if x2 > x1 && y2 > y1 {
            out.push([x1, y1, x2, y2]);
        }
    }
    out
}

/// Place one cursor sprite on an output when their rectangles intersect. The
/// input position is the global physical-pixel hotspot and destination
/// coordinates are CRTC-local. Cursor planes conventionally require the full
/// source rectangle (Smithay/Niri follow the same rule); negative or
/// edge-crossing destination coordinates let KMS clip it. Returning a
/// placement for every intersected CRTC lets a cursor straddle outputs.
fn cursor_plane_rect(
    position: (i32, i32),
    hotspot: (u32, u32),
    image_size: (u32, u32),
    output_origin: (u32, u32),
    output_size: (u32, u32),
) -> Option<CursorPlaneRect> {
    let left = i64::from(position.0) - i64::from(hotspot.0);
    let top = i64::from(position.1) - i64::from(hotspot.1);
    let right = left + i64::from(image_size.0);
    let bottom = top + i64::from(image_size.1);
    let output_left = i64::from(output_origin.0);
    let output_top = i64::from(output_origin.1);
    let output_right = output_left + i64::from(output_size.0);
    let output_bottom = output_top + i64::from(output_size.1);
    if right <= output_left || bottom <= output_top || left >= output_right || top >= output_bottom
    {
        return None;
    }
    Some(CursorPlaneRect {
        src: (0, 0, image_size.0, image_size.1),
        dst: (
            (left - output_left) as i32,
            (top - output_top) as i32,
            image_size.0,
            image_size.1,
        ),
    })
}

fn add_cursor_plane_to_commit(
    request: &mut atomic::AtomicModeReq,
    output: &Output,
    state: Option<CursorState>,
    buffers: &[CursorBuffer],
) {
    let Some(cursor) = &output.planes.cursor else {
        return;
    };
    let placement = state.and_then(|state| {
        let buffer = buffers.get(state.buffer)?;
        let (width, height) = output.mode.size();
        let rect = cursor_plane_rect(
            state.position,
            state.hotspot,
            buffer.dumb.size(),
            (output.x, output.y),
            (u32::from(width), u32::from(height)),
        )?;
        Some((buffer.framebuffer, rect))
    });
    let Some((framebuffer, rect)) = placement else {
        request.add_property(
            cursor.handle,
            cursor.props.plane_fb_id,
            property::Value::Framebuffer(None),
        );
        request.add_property(
            cursor.handle,
            cursor.props.plane_crtc_id,
            property::Value::CRTC(None),
        );
        return;
    };
    request.add_property(
        cursor.handle,
        cursor.props.plane_fb_id,
        property::Value::Framebuffer(Some(framebuffer)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_id,
        property::Value::CRTC(Some(output.crtc)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_x,
        property::Value::UnsignedRange(u64::from(rect.src.0) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_y,
        property::Value::UnsignedRange(u64::from(rect.src.1) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_w,
        property::Value::UnsignedRange(u64::from(rect.src.2) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_h,
        property::Value::UnsignedRange(u64::from(rect.src.3) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_x,
        property::Value::SignedRange(i64::from(rect.dst.0)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_y,
        property::Value::SignedRange(i64::from(rect.dst.1)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_w,
        property::Value::UnsignedRange(u64::from(rect.dst.2)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_h,
        property::Value::UnsignedRange(u64::from(rect.dst.3)),
    );
}

/// Pure KMS values for one overlay plane, before they are written into an
/// `AtomicModeReq`. Keeping this value separately testable catches property
/// mistakes without a DRM card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayPlaneRequest {
    framebuffer: control::framebuffer::Handle,
    crtc: crtc::Handle,
    src: (u64, u64, u64, u64),
    dst: (i64, i64, u64, u64),
    alpha: Option<u64>,
    zpos: Option<u64>,
    in_fence_fd: Option<i32>,
}

fn overlay_plane_request_supported(
    plane: &OverlayPlaneCapabilities,
    width: i32,
    height: i32,
    target: tessera_primitives::Rect,
    zpos: Option<u32>,
    acquire_fence: Option<&OwnedFd>,
) -> bool {
    let props = &plane.props;
    !(zpos.is_some() && props.zpos.is_none())
        && !(acquire_fence.is_some() && props.in_fence_fd.is_none())
        && width > 0
        && height > 0
        && target.size.w > 0
        && target.size.h > 0
}

fn overlay_plane_request(
    plane: &OverlayPlaneCapabilities,
    framebuffer: control::framebuffer::Handle,
    crtc: crtc::Handle,
    width: i32,
    height: i32,
    target: tessera_primitives::Rect,
    zpos: Option<u32>,
    acquire_fence: Option<&OwnedFd>,
) -> OverlayPlaneRequest {
    let props = &plane.props;
    debug_assert!(
        overlay_plane_request_supported(plane, width, height, target, zpos, acquire_fence),
        "overlay plane request must be validated before commit construction"
    );
    OverlayPlaneRequest {
        framebuffer,
        crtc,
        src: (0, 0, (width as u64) << 16, (height as u64) << 16),
        dst: (
            i64::from(target.origin.x),
            i64::from(target.origin.y),
            target.size.w as u64,
            target.size.h as u64,
        ),
        // The candidate is proven fully opaque; 0xffff is the KMS opaque
        // endpoint and does not depend on the plane's pixel-blend default.
        alpha: props.alpha.map(|_| 0xffff),
        zpos: props.zpos.and(zpos).map(u64::from),
        in_fence_fd: props.in_fence_fd.and(acquire_fence.map(AsRawFd::as_raw_fd)),
    }
}

/// Program one overlay plane in a complete-set request.
///
/// The caller has already selected a plane that supports the candidate's
/// format/modifier and zpos. This helper only expresses the KMS properties;
/// it does not allocate, import, or present anything.
fn add_overlay_plane_to_commit(
    request: &mut atomic::AtomicModeReq,
    output: &Output,
    plane: &OverlayPlaneCapabilities,
    framebuffer: control::framebuffer::Handle,
    acquire_fence: Option<&OwnedFd>,
    width: i32,
    height: i32,
    target: tessera_primitives::Rect,
    zpos: Option<u32>,
) {
    let values = overlay_plane_request(
        plane,
        framebuffer,
        output.crtc,
        width,
        height,
        target,
        zpos,
        acquire_fence,
    );
    let props = &plane.props;
    request.add_property(
        plane.handle,
        props.fb_id,
        property::Value::Framebuffer(Some(values.framebuffer)),
    );
    request.add_property(
        plane.handle,
        props.crtc_id,
        property::Value::CRTC(Some(values.crtc)),
    );
    request.add_property(
        plane.handle,
        props.src_x,
        property::Value::UnsignedRange(values.src.0),
    );
    request.add_property(
        plane.handle,
        props.src_y,
        property::Value::UnsignedRange(values.src.1),
    );
    request.add_property(
        plane.handle,
        props.src_w,
        property::Value::UnsignedRange(values.src.2),
    );
    request.add_property(
        plane.handle,
        props.src_h,
        property::Value::UnsignedRange(values.src.3),
    );
    request.add_property(
        plane.handle,
        props.crtc_x,
        property::Value::SignedRange(values.dst.0),
    );
    request.add_property(
        plane.handle,
        props.crtc_y,
        property::Value::SignedRange(values.dst.1),
    );
    request.add_property(
        plane.handle,
        props.crtc_w,
        property::Value::UnsignedRange(values.dst.2),
    );
    request.add_property(
        plane.handle,
        props.crtc_h,
        property::Value::UnsignedRange(values.dst.3),
    );
    if let (Some(alpha), Some(alpha_prop)) = (values.alpha, props.alpha) {
        request.add_property(
            plane.handle,
            alpha_prop,
            property::Value::UnsignedRange(alpha),
        );
    }
    if let (Some(zpos), Some(zpos_prop)) = (values.zpos, props.zpos) {
        request.add_property(
            plane.handle,
            zpos_prop,
            property::Value::UnsignedRange(zpos),
        );
    }
    if let (Some(fd), Some(in_fence)) = (values.in_fence_fd, props.in_fence_fd) {
        request.add_property(
            plane.handle,
            in_fence,
            property::Value::SignedRange(i64::from(fd)),
        );
    }
    if let Some(damage_clips) = props.fb_damage_clips {
        // The probe gives no damage information; the kernel treats a NULL
        // blob as full damage, which is the safe conservatism for validation.
        request.add_property(plane.handle, damage_clips, property::Value::Blob(0));
    }
}

#[derive(Debug)]
struct Card {
    device: libseat::Device,
    path: PathBuf,
}

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}

#[derive(Debug, Clone, Copy)]
struct OutputAtomicProperties {
    connector_crtc_id: property::Handle,
    crtc_mode_id: property::Handle,
    crtc_active: property::Handle,
    /// `OUT_FENCE_PTR`: produces a sync_file for completion of an atomic
    /// commit. Direct scanout uses it for correct Wayland buffer release.
    crtc_out_fence_ptr: Option<property::Handle>,
    /// Connector `Colorspace` property plus the harvested enum values for
    /// `Default` and `BT2020_RGB` (None when the driver exposes neither the
    /// property nor those values). Set on every commit — the property is
    /// sticky, so an SDR commit after HDR must reset it explicitly.
    connector_colorspace: Option<(property::Handle, u64, u64)>,
    /// Connector `HDR_OUTPUT_METADATA` blob property (CTA-861.3 static
    /// metadata type 1). Set for HDR commits, cleared (blob 0) for SDR.
    connector_hdr_metadata: Option<property::Handle>,
    /// Connector `max bpc` range property. Driven to 10 for deep-color/HDR
    /// modes, 8 for SDR — drivers otherwise clamp it on their own.
    connector_max_bpc: Option<property::Handle>,
    /// CRTC `GAMMA_LUT` blob property and the driver's table size
    /// (`GAMMA_LUT_SIZE`, typically 256 or 1024 entries). Night light
    /// programs a per-channel gain ramp here; absent on legacy drivers.
    crtc_gamma_lut: Option<(property::Handle, u32)>,
}

#[derive(Debug)]
struct Output {
    connector: connector::Handle,
    name: String,
    crtc: crtc::Handle,
    /// Output-layer plane allocation for this CRTC. The primary plane carries
    /// either a compositor output or one eligible client dma-buf; cursor is
    /// optional; overlay is allocated only when its policy is enabled and the
    /// complete set passes `TEST_ONLY` (ADR-0234, ADR-0235).
    planes: OutputPlanes,
    mode: Mode,
    /// Rectangle of the shared framebuffer this output scans. For an extended
    /// output this is its own mode; for a mirror it is the source output's
    /// rectangle, scaled by the plane to this output's destination rectangle.
    source_size: (u32, u32),
    /// Plane destination in CRTC coordinates `(x, y, w, h)`. Extended outputs
    /// fill the mode; a mirror preserves the source aspect ratio and leaves
    /// black bars when the two aspects differ.
    destination: (i32, i32, u32, u32),
    /// Extended output or presentation alias (ADR-0243).
    role: tessera_desktop::output::OutputRole,
    /// Whether `role` came from explicit configuration or the session policy.
    role_source: tessera_desktop::output::OutputRoleSource,
    mode_blob: property::Value<'static>,
    mode_blob_id: u64,
    x: u32,
    y: u32,
    /// Physical display dimensions reported by the DRM connector/EDID.
    physical_size_mm: Option<(u32, u32)>,
    /// Validated diagonal density for diagnostics. `None` means the physical
    /// dimensions were absent or failed the plausibility checks.
    ppi: Option<f32>,
    kind: OutputKind,
    /// Hardware-derived default. A `[[output]] scale` policy can still
    /// override this after the backend snapshot reaches the compositor.
    scale: Scale,
    props: OutputAtomicProperties,
    /// The connector's advertised modes at selection time (deduplicated,
    /// highest resolution first), surfaced through `output_infos`.
    available_modes: Vec<OutputMode>,
    /// HDR/wide-gamut capabilities parsed from the connector's EDID.
    color_caps: tessera_primitives::edid::EdidColorCapabilities,
}

/// The session-wide color pipeline mode. The compositor renders one shared
/// framebuffer for every output, so the pixel encoding is uniform: HDR
/// engages only when all active outputs allow and support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayColorMode {
    /// 8-bit sRGB (the default).
    Sdr,
    /// 10-bit sRGB in an RGB10A2-class container — deep color for banding.
    SdrDeepColor,
    /// BT.2020 PQ (HDR10-class) in an RGB10A2-class container.
    Hdr,
}

impl DisplayColorMode {
    /// The framebuffer fourcc candidates in preference order.
    fn fb_candidates(self) -> &'static [DrmFourcc] {
        match self {
            DisplayColorMode::Sdr => &[DrmFourcc::Xrgb8888, DrmFourcc::Argb8888],
            DisplayColorMode::SdrDeepColor | DisplayColorMode::Hdr => &[
                DrmFourcc::Xbgr2101010,
                DrmFourcc::Abgr2101010,
                DrmFourcc::Xrgb8888,
                DrmFourcc::Argb8888,
            ],
        }
    }
}

#[derive(Debug)]
struct DisplaySet {
    outputs: Vec<Output>,
    size: (u32, u32),
    format: DrmFourcc,
    modifiers: Vec<u64>,
    /// Exact format/modifier intersections accepted by every selected
    /// primary plane. This is broader than the compositor render-target
    /// format above and governs client direct scanout.
    scanout_formats: HashMap<u32, Vec<u64>>,
    /// The negotiated session color pipeline. Derived from `format` and the
    /// requested policy: an 8-bit format means SDR regardless of intent.
    color_mode: DisplayColorMode,
    /// The ICC profile driving the framebuffer's content space (SDR modes
    /// only): the first connected connector with an `icc_profile` config
    /// entry, in connector order.
    icc_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct OutputSignature {
    name: String,
    width: u32,
    height: u32,
    refresh: u32,
    scale_bits: u32,
    x: u32,
    y: u32,
    cursor: bool,
    mirror: bool,
    role_source: tessera_desktop::output::OutputRoleSource,
    source_width: u32,
    source_height: u32,
    destination: (i32, i32, u32, u32),
    overlay: OverlayPlaneAllocation,
}
type DisplaySignature = (
    DrmFourcc,
    Vec<u64>,
    Vec<(u32, Vec<u64>)>,
    DisplayColorMode,
    Option<String>,
    Vec<OutputSignature>,
);

#[derive(Debug)]
struct PrimaryScanout {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    slot: u32,
    acquire_fence: Option<OwnedFd>,
    ownership: PrimaryScanoutOwnership,
    /// Flux image lease shared by every CRTC scanning this composited frame.
    /// The ring slot becomes reusable only when the last slot referencing the
    /// image retires and drops its lease (ADR-0185).
    lease: Option<std::rc::Rc<flux::DmabufLease>>,
}

#[derive(Debug)]
struct PlaneSlot {
    crtc: crtc::Handle,
    /// Framebuffer currently programmed on this CRTC's primary plane.
    current: Option<PrimaryScanout>,
    /// Previous framebuffer whose page-flip event has not arrived.
    retiring: Option<PrimaryScanout>,
    /// Semantic source of `current`.
    source: Option<PrimaryPlaneSource>,
    /// A flip is committed and its page-flip event has not arrived.
    pending: bool,
}

/// A client buffer scanned out by one overlay plane. The buffer shares the
/// same import cache and retirement rules as a direct-scanout primary buffer,
/// but it never owns Primary.
#[derive(Debug)]
struct OverlayScanout {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    acquire_fence: Option<OwnedFd>,
    ownership: PrimaryScanoutOwnership,
    output: String,
    plane: plane::Handle,
    width: i32,
    height: i32,
    target: tessera_primitives::Rect,
    zpos: Option<u32>,
}

/// Semantic source selected for the primary plane in this atomic commit.
/// Keeping this explicit prevents a boolean parameter from silently mixing
/// compositor framebuffer lifetime with client `wl_buffer` release rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimaryPlaneSource {
    Composited,
    DirectClient,
}

impl PrimaryPlaneSource {
    fn needs_kms_completion_fence(self) -> bool {
        matches!(self, Self::DirectClient)
    }
}

/// An overlay layer is only legal above the compositor framebuffer. A direct
/// client owns the complete output and leaves no Primary pixels underneath an
/// overlay (ADR-0237).
fn overlay_source_supported(source: PrimaryPlaneSource) -> bool {
    source == PrimaryPlaneSource::Composited
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimaryScanoutOwnership {
    /// An uncacheable compositor output import. It is destroyed when the
    /// scanout retires but does not use client `wl_buffer` release semantics.
    TransientCompositor,
    /// An uncacheable client direct-scanout import (when `fstat` identity is
    /// unavailable). Destroyed when the scanout retires or its commit fails.
    TransientClient,
    /// A compositor-output import is owned by the backend cache. PrimaryScanout
    /// retirement only drops this reference; the cache destroys the DRM
    /// framebuffer and GEM handle after invalidation and the last reference.
    Cached(CompositeFbKey),
    /// A client direct-scanout import owned by the bounded client scanout
    /// cache, avoiding per-frame `PRIME_FD_TO_HANDLE` + `ADDFB2` + `RMFB` +
    /// `GEM_CLOSE` ioctl churn across swapchain rotations.
    CachedClient(ClientFbKey),
}

impl PrimaryScanoutOwnership {
    fn matches_source(self, source: PrimaryPlaneSource) -> bool {
        matches!(
            (self, source),
            (
                Self::TransientCompositor | Self::Cached(_),
                PrimaryPlaneSource::Composited
            ) | (
                Self::TransientClient | Self::CachedClient(_),
                PrimaryPlaneSource::DirectClient
            )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DmabufIdentity {
    device: u64,
    inode: u64,
}

/// Complete identity of one Flux compositor-output swapchain image.
///
/// A slot number alone is insufficient: resize or surface recreation reuses
/// slot numbers for new dma-buf objects. The inode identity distinguishes the
/// backing object, while the layout fields prevent reuse after a format or
/// modifier reconfigure. `epoch` makes invalidation explicit even on drivers
/// whose dma-buf inode identity is unexpectedly recycled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CompositeFbKey {
    epoch: u64,
    slot: u32,
    identity: DmabufIdentity,
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    modifier: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ClientFbKey {
    identity: DmabufIdentity,
    width: u32,
    height: u32,
    stride: u32,
    offset: u32,
    fourcc: u32,
    modifier: u64,
}

#[derive(Debug)]
struct CachedCompositeFb {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    /// Number of submitted/imported `PrimaryScanout` values that still reference
    /// this entry. This includes failed commits until their error path releases
    /// the scanout, current scanout, and page-flip-retiring scanout.
    users: u32,
    /// False once another dma-buf replaces the same slot or the surface epoch
    /// is invalidated. Non-reusable entries are destroyed at users == 0.
    reusable: bool,
}

#[derive(Debug)]
struct CachedClientFb {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    users: u32,
    reusable: bool,
    last_used: u64,
}

const MAX_CLIENT_SCANOUT_FBS: usize = 16;

#[derive(Debug)]
struct CursorBuffer {
    framebuffer: control::framebuffer::Handle,
    dumb: control::dumbbuffer::DumbBuffer,
    pixels: Vec<u8>,
    /// Size of the actual cursor art inside the fixed-size transparent KMS
    /// buffer. Plane programming uses the full driver-advertised dumb-buffer
    /// extent, while this value keeps cache lookup exact.
    content_size: (u32, u32),
    /// Cheap pre-filter for the pixel comparison below: sprites with
    /// different lengths or sizes are rejected without touching memory.
    len: usize,
}

impl CursorBuffer {
    fn matches(&self, size: (u32, u32), pixels: &[u8]) -> bool {
        self.content_size == size && self.len == pixels.len() && self.pixels.as_slice() == pixels
    }
}

/// Upper bound on distinct cursor sprites retained as KMS dumb buffers.
/// The set of distinct (shape, quantized-scale) pairs is bounded by the
/// theme (~tens) times active scales; entries beyond this bound are evicted
/// oldest-first so the cache cannot drift upward over a long session.
const MAX_CURSOR_BUFFERS: usize = 64;

#[derive(Debug, Clone, Copy)]
struct CursorState {
    buffer: usize,
    position: (i32, i32),
    hotspot: (u32, u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorPlaneRect {
    src: (u32, u32, u32, u32),
    dst: (i32, i32, u32, u32),
}

#[derive(Debug)]
struct ImportedBuffer {
    size: (u32, u32),
    stride: u32,
    modifier: DrmModifier,
    format: DrmFourcc,
    /// Byte offset of the first pixel within the dma-buf object. Zero for the
    /// compositor's own export; a directly-scanned-out client buffer may carry
    /// a non-zero plane offset.
    offset: u32,
    gem: BufferHandle,
}

/// Layout descriptor for a directly-scanned-out client dma-buf. Groups the
/// fields `import_scanout_client` needs beyond the duplicated fd, so the
/// import call stays readable.
struct ClientScanoutDesc {
    width: u32,
    height: u32,
    stride: u32,
    offset: u32,
    format: DrmFourcc,
    modifier: DrmModifier,
}

impl PlanarBuffer for ImportedBuffer {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn format(&self) -> DrmFourcc {
        // VK_FORMAT_B8G8R8A8_UNORM has X/ARGB8888 byte layout on little-endian
        // Linux systems. The selected plane format decides whether alpha is
        // ignored or interpreted.
        self.format
    }

    fn modifier(&self) -> Option<DrmModifier> {
        (self.modifier != DrmModifier::Invalid).then_some(self.modifier)
    }

    fn pitches(&self) -> [u32; 4] {
        [self.stride, 0, 0, 0]
    }

    fn handles(&self) -> [Option<BufferHandle>; 4] {
        [Some(self.gem), None, None, None]
    }

    fn offsets(&self) -> [u32; 4] {
        [self.offset, 0, 0, 0]
    }
}

#[derive(Debug, Clone, Copy)]
enum PendingSeatEvent {
    Enable,
    Disable,
}

#[derive(Clone)]
struct SeatInputInterface {
    seat: Rc<RefCell<libseat::Seat>>,
    devices: Rc<RefCell<HashMap<RawFd, libseat::Device>>>,
}

impl LibinputInterface for SeatInputInterface {
    fn open_restricted(&mut self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        let device = self
            .seat
            .borrow_mut()
            .open_device(&path)
            .map_err(|error| error.0)?;
        let raw = device.as_fd().as_raw_fd();
        self.devices.borrow_mut().insert(raw, device);
        // SAFETY: libinput immediately consumes this OwnedFd into its C
        // context. The matching close callback removes the libseat device and
        // hands closure back to libseat without allowing Rust to close it too.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        let raw = fd.into_raw_fd();
        if let Some(device) = self.devices.borrow_mut().remove(&raw) {
            if let Err(error) = self.seat.borrow_mut().close_device(device) {
                log::warn!("libseat: failed to close input device fd {raw}: {error:?}");
            }
        } else {
            // Defensive fallback for a descriptor not created by this
            // interface. Ownership arrived through OwnedFd and must be closed.
            unsafe { libc::close(raw) };
        }
    }
}

/// Atomic KMS presentation and libinput event source for all connected outputs
/// on one DRM card. Extended outputs scan disjoint source rectangles of a
/// shared dma-buf framebuffer; mirrored outputs scan the same rectangle as
/// their source, scaled by the plane to their native CRTC mode. Every CRTC is
/// committed in one atomic batch, so a frame is visually coherent across
/// outputs.
pub struct DrmBackend {
    seat: Rc<RefCell<libseat::Seat>>,
    seat_event: Rc<Cell<Option<PendingSeatEvent>>>,
    input_devices: Rc<RefCell<HashMap<RawFd, libseat::Device>>>,
    input: Option<Libinput>,
    hotplug: Option<udev::MonitorSocket>,
    card: Option<Card>,
    displays: DisplaySet,
    active: bool,
    failed: bool,
    modeset_done: bool,
    pending_flips: HashSet<crtc::Handle>,
    /// One primary-plane slot per output. A slot owns the framebuffer it is
    /// scanning until its own page-flip event retires it, so a partially
    /// committed batch can release one CRTC without invalidating another
    /// CRTC's scanout (ADR-0185).
    slots: Vec<PlaneSlot>,
    /// Overlay plane currently scanning out a client buffer. Kept separate
    /// from Primary because its buffer lifetime and disable path are its own.
    current_overlay: Option<OverlayScanout>,
    retiring_overlay: Option<OverlayScanout>,
    /// Hardware-validation gate for overlay allocation. Policy is reapplied
    /// after every hotplug reprobe so a selected topology cannot silently
    /// enable or disable it.
    overlay_offload_enabled: bool,
    /// KMS framebuffer imports for Flux compositor-output swapchain images.
    composite_fb_cache: HashMap<CompositeFbKey, CachedCompositeFb>,
    /// Generation of the live Flux surface/storage. Resize and recreation
    /// advance it before the next exported frame can be imported.
    composite_fb_epoch: u64,
    /// Reused scratch storage for cache reaping; invalidation is rare, but it
    /// should not require a fresh allocation every time.
    composite_fb_reap: Vec<CompositeFbKey>,
    /// Bounded LRU cache of KMS framebuffer imports for directly scanned-out
    /// client dma-bufs.
    client_fb_cache: HashMap<ClientFbKey, CachedClientFb>,
    client_fb_reap: Vec<ClientFbKey>,
    client_fb_clock: u64,
    cursor_buffers: Vec<CursorBuffer>,
    cursor_state: Option<CursorState>,
    /// Whether the last successful atomic commit left any cursor plane
    /// enabled. This intentionally survives a software-fallback decision
    /// until a later commit has actually disabled the kernel plane.
    cursor_plane_active: bool,
    cursor_extent: (u32, u32),
    hardware_cursor_failed: bool,
    /// When a disabled cursor plane may be probed again. Cursor commit
    /// rejections are usually transient (a modeset or hotplug in flight), so
    /// the software fallback is not terminal: each consecutive failure
    /// doubles the backoff, and the first successful cursor-active commit
    /// resets the count.
    hardware_cursor_retry_at: Option<std::time::Instant>,
    hardware_cursor_failures: u32,
    input_events: Vec<InputEvent>,
    gesture_events: Vec<PointerGestureEvent>,
    pointer: (f32, f32),
    explicit_sync: bool,
    sync_capable: bool,
    /// Whether the DRM driver supports atomic asynchronous page flips (`DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`).
    atomic_async_page_flip: bool,
    render_ready: bool,
    /// KMS scanout power requested by the session idle policy. Input and
    /// Wayland dispatch remain active while this is false.
    outputs_powered: bool,
    /// Descriptor duplicate of the most recently presented composited
    /// frame, retained so zero-copy stream consumers (IPC protocol 25) can
    /// import exactly what is on screen. Cleared when a direct client
    /// scanout owns the primary plane and on surface recreation.
    presented_composite: Option<PresentedComposite>,
    hotplug_pending: bool,
    pending_resize: Option<Size>,
    /// Modifier intersection the live Flux surface was created with.
    surface_modifiers: Vec<u64>,
    /// Color pipeline mode the live Flux surface was created with.
    surface_color_mode: DisplayColorMode,
    /// ICC profile path the live Flux surface's content space came from.
    surface_icc: Option<String>,
    /// Live `GAMMA_LUT` blob id (night light). Replaced on each new table;
    /// destroyed when neutral restores the driver default ramp.
    gamma_blob: Option<u64>,
    /// Pre-created `HDR_OUTPUT_METADATA` blob (CTA-861.3 static metadata
    /// type 1, BT.2020 primaries, PQ EOTF) shared by every connector in
    /// HDR mode; created/destroyed with the HDR surface.
    hdr_metadata_blob: Option<(property::Value<'static>, u64)>,
    /// Set when a hotplug changed that intersection; the surface must be
    /// recreated (resize alone cannot change a surface's modifier).
    surface_stale: bool,
    /// Per-connector display-mode requests from the config's `[[output]]`
    /// entries (ADR-0028). Consulted on every output (re)selection: startup,
    /// hotplug, and session resume.
    configured_modes: HashMap<String, ModeSpec>,
    /// Session topology policy for connectors without an explicit role
    /// (ADR-0243).
    configured_topology: tessera_desktop::output::DisplayTopology,
    /// Per-connector output policies from `[[output]]` (ADR-0243): explicit
    /// role and primary flag participate in topology resolution.
    configured_policies: HashMap<String, tessera_desktop::output::OutputPolicy>,
    /// Per-connector color policy (`hdr` / `deep_color`) from the config's
    /// `[[output]]` entries. Same consultation cadence as `configured_modes`.
    configured_color: HashMap<String, ColorPolicy>,
    /// Per-connector ICC profile paths from the config's `[[output]]`
    /// entries. The chosen profile drives the framebuffer's content space
    /// in SDR modes.
    configured_icc: HashMap<String, String>,
    /// Retained libinput handles for touchpads currently on the seat.
    touchpads: HashMap<String, Device>,
    touchpad_config: TouchpadConfig,
    /// Retained libinput handles for plain mice (non-touchpad pointers).
    mice: HashMap<String, Device>,
    mouse_config: MouseConfig,
    /// Absolute input-device bindings from `[[input.mapping]]`. Resolved
    /// against the live DisplaySet on every event, so a topology change
    /// cannot leave a stale rectangle behind.
    input_mappings: Vec<tessera_primitives::input::InputMapping>,
    /// The compositor's Wayland server event-loop fd, registered via
    /// `Backend::set_wakeup_fd`. Polled for readability only — the main loop
    /// dispatches the server itself once the wait wakes.
    wakeup_fd: Option<RawFd>,
}

impl DrmBackend {
    /// Enable or disable the explicit overlay hardware-validation path.
    ///
    /// Enabling the policy allocates nothing by itself: the next overlay
    /// commit still proves the complete plane set with `TEST_ONLY` (ADR-0235).
    pub fn set_overlay_offload_enabled(&mut self, enabled: bool) {
        self.overlay_offload_enabled = enabled;
        apply_overlay_policy(&mut self.displays, enabled);
        log::info!(
            "drm: overlay offload {} (complete-set TEST_ONLY still required)",
            if enabled {
                "enabled for hardware validation"
            } else {
                "disabled"
            }
        );
    }

    /// Overlay planes discovered on every active output, with the
    /// format/modifier and zpos facts an overlay planner needs. Allocation
    /// remains governed by `OverlayPlanePolicy`.
    pub fn overlay_planes(&self) -> Vec<crate::host::OverlayPlaneInfo> {
        self.displays
            .outputs
            .iter()
            .flat_map(|output| {
                output
                    .planes
                    .overlay
                    .discovered
                    .iter()
                    .map(|plane| crate::host::OverlayPlaneInfo {
                        output: output.name.clone(),
                        formats: plane
                            .formats
                            .iter()
                            .map(|format| crate::host::OverlayFormatInfo {
                                fourcc: format.fourcc,
                                modifiers: format.modifiers.clone(),
                            })
                            .collect(),
                        zpos: plane.zpos,
                        alpha: plane.alpha,
                        in_fence_fd: plane.in_fence_fd,
                        fb_damage_clips: plane.fb_damage_clips,
                    })
            })
            .collect()
    }

    /// Format/modifier pairs accepted by every active primary plane.
    ///
    /// This is the conservative set suitable for linux-dmabuf feedback
    /// SCANOUT tranche: a client choosing one of these pairs can still be
    /// scanned out when the desktop spans more than one selected output.
    pub fn dmabuf_scanout_formats(&self) -> Vec<tessera_primitives::dmabuf::DmabufFormat> {
        let mut formats = self
            .displays
            .scanout_formats
            .iter()
            .map(
                |(&fourcc, modifiers)| tessera_primitives::dmabuf::DmabufFormat {
                    fourcc,
                    modifiers: modifiers.clone(),
                },
            )
            .collect::<Vec<_>>();
        formats.sort_by_key(|format| format.fourcc);
        formats
    }

    /// Path and character-device identity of the libseat-owned KMS node.
    ///
    /// The identity comes from the live descriptor rather than a second path
    /// lookup, so Vulkan selection is bound to the exact device libseat
    /// granted even if `/dev/dri` changes concurrently.
    pub fn kms_device(&self) -> Result<(PathBuf, flux::DrmNode), DrmError> {
        let card = self.card.as_ref().ok_or(DrmError::Inactive)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(card.as_fd().as_raw_fd(), stat.as_mut_ptr()) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFCHR {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a character device", card.path.display()),
            )
            .into());
        }
        Ok((
            card.path.clone(),
            flux::DrmNode {
                major: libc::major(stat.st_rdev),
                minor: libc::minor(stat.st_rdev),
            },
        ))
    }

    /// Linux `dev_t` for the DRM node that owns the active KMS resources.
    ///
    /// linux-dmabuf feedback accepts either a primary or render node. The
    /// primary node is the strongest identity available from this backend and
    /// is sufficient for Mesa to resolve the matching render node. Returning
    /// `None` keeps the compositor on the legacy v3 protocol rather than
    /// sending an invalid device identity.
    pub fn dmabuf_feedback_device(&self) -> Option<u64> {
        let path = &self.card.as_ref()?.path;
        match self.kms_device() {
            Ok((_, node)) => Some(libc::makedev(node.major, node.minor)),
            Err(error) => {
                log::warn!(
                    "drm: cannot identify live KMS device {} for linux-dmabuf feedback: {error}",
                    path.display()
                );
                None
            }
        }
    }
}

/// Scale one high-resolution wheel axis value (`value120`) by a scroll
/// multiplier, keeping the three representations consistent: the continuous
/// value (10 surface units per detent), whole steps for legacy v5–7 clients,
/// and the original 120ths for high-resolution clients.
///
/// `discrete` reports only whole scaled detents — high-resolution wheels
/// legitimately emit fractions of a detent per frame and legacy clients must
/// not see a full click for each — except a click slowed below one detent,
/// which still steps once so a slowed wheel never feels dead to a legacy
/// client. A scale that rounds the frame to zero 120ths suppresses it.
fn scaled_wheel_value(value120: i32, factor: f32) -> PointerAxis {
    let scaled = value120 as f32 * factor;
    let scaled120 = scaled.round() as i32;
    if scaled120 == 0 {
        return PointerAxis::default();
    }
    let whole_detents = scaled120 / 120;
    let discrete = if whole_detents == 0 && value120.abs() >= 120 {
        // One deliberate click, slowed below one detent: step once.
        Some(scaled120.signum())
    } else {
        (whole_detents != 0).then_some(whole_detents)
    };
    PointerAxis {
        value: Some(scaled / 12.0),
        discrete,
        value120: Some(scaled120),
        ..PointerAxis::default()
    }
}

fn wheel_axis(event: &PointerScrollWheelEvent, axis: Axis, factor: f32) -> PointerAxis {
    if !event.has_axis(axis) {
        return PointerAxis::default();
    }
    let value120 = event.scroll_value_v120(axis).round() as i32;
    if value120 == 0 {
        return PointerAxis::default();
    }
    scaled_wheel_value(value120, factor)
}

fn sequence_axis<T: PointerScrollEvent>(
    event: &T,
    axis: Axis,
    inverted: bool,
    factor: f32,
) -> PointerAxis {
    if !event.has_axis(axis) {
        return PointerAxis::default();
    }
    let value = event.scroll_value(axis) as f32 * factor;
    PointerAxis {
        value: (value != 0.0).then_some(value),
        stop: event.scroll_value(axis) == 0.0,
        relative_direction: (value != 0.0).then_some(if inverted {
            PointerAxisRelativeDirection::Inverted
        } else {
            PointerAxisRelativeDirection::Identical
        }),
        ..PointerAxis::default()
    }
}

#[allow(deprecated)]
fn legacy_axis(
    event: &PointerAxisEvent,
    axis: Axis,
    source: PointerAxisSource,
    inverted: bool,
    factor: f32,
) -> PointerAxis {
    if !event.has_axis(axis) {
        return PointerAxis::default();
    }
    let raw = event.axis_value(axis) as f32;
    let discrete = event
        .axis_value_discrete(axis)
        .map(|value| value.round() as i32)
        .filter(|value| *value != 0);
    let value = match (source, discrete) {
        (PointerAxisSource::Wheel | PointerAxisSource::WheelTilt, Some(steps)) => {
            steps as f32 * 10.0 * factor
        }
        _ => raw * factor,
    };
    PointerAxis {
        value: (value != 0.0).then_some(value),
        discrete,
        stop: value == 0.0
            && matches!(
                source,
                PointerAxisSource::Finger | PointerAxisSource::Continuous
            ),
        relative_direction: (value != 0.0).then_some(if inverted {
            PointerAxisRelativeDirection::Inverted
        } else {
            PointerAxisRelativeDirection::Identical
        }),
        ..PointerAxis::default()
    }
}

impl Backend for DrmBackend {
    fn size(&self) -> Size {
        let (width, height) = self.displays.size;
        Size {
            w: width as i32,
            h: height as i32,
        }
    }

    fn output_infos(&self) -> Vec<tessera_desktop::output::OutputInfo> {
        self.displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                tessera_desktop::output::OutputInfo {
                    connector: output.name.clone(),
                    geometry: tessera_desktop::output::OutputGeometry {
                        mode: tessera_desktop::output::OutputMode {
                            width: width as i32,
                            height: height as i32,
                            refresh_mhz: output.mode.vrefresh().saturating_mul(1_000),
                        },
                        scale: output.scale,
                        transform: tessera_primitives::Transform::Normal,
                        logical_origin: tessera_primitives::Point {
                            x: output.x as i32,
                            y: output.y as i32,
                        },
                        logical_size_override: None,
                    },
                    kind: output.kind,
                    available_modes: output.available_modes.clone(),
                    color_caps: output.color_caps,
                    role: output.role.clone(),
                    role_source: output.role_source,
                }
            })
            .collect()
    }

    fn set_input_config(&mut self, config: tessera_primitives::input::InputConfig) -> InputStatus {
        self.touchpad_config = config.touchpad;
        for device in self.touchpads.values_mut() {
            Self::apply_touchpad_profile(device, config.touchpad);
        }
        self.mouse_config = config.mouse;
        for device in self.mice.values_mut() {
            Self::apply_mouse_profile(device, config.mouse);
        }
        self.input_mappings = config.mappings;
        let mut status = self.current_input_status();
        status.keyboard = config.keyboard;
        status
    }

    fn input_status(&self) -> InputStatus {
        self.current_input_status()
    }

    fn set_configured_topology(
        &mut self,
        topology: tessera_desktop::output::DisplayTopology,
        policies: HashMap<String, tessera_desktop::output::OutputPolicy>,
    ) {
        if self.configured_topology == topology && self.configured_policies == policies {
            return;
        }
        self.configured_topology = topology;
        self.configured_policies = policies;
        // A topology change is a scanout-layout change: re-probe through the
        // hotplug path so the union framebuffer and the plane source rects
        // move only after every in-flight page flip retires.
        self.hotplug_pending = true;
    }

    fn set_configured_modes(&mut self, modes: HashMap<String, ModeSpec>) {
        if self.configured_modes == modes {
            return;
        }
        self.configured_modes = modes;
        // Re-use the hotplug reconciliation path: it waits until every
        // in-flight page flip has retired, probes only advertised modes, and
        // hands a size/modifier change back to the main loop before another
        // frame is acquired. This makes a System Settings mode edit live
        // without racing scanout ownership.
        self.hotplug_pending = true;
    }

    fn set_configured_color_policies(&mut self, policies: HashMap<String, ColorPolicy>) {
        if self.configured_color == policies {
            return;
        }
        self.configured_color = policies;
        // Same reconciliation path as mode changes: re-selection re-runs the
        // color-mode negotiation, and a framebuffer format change flags the
        // Flux surface stale for recreation.
        self.hotplug_pending = true;
    }

    fn color_pipeline(&self) -> tessera_desktop::output::ColorPipeline {
        match self.displays.color_mode {
            DisplayColorMode::Sdr => tessera_desktop::output::ColorPipeline::Sdr,
            DisplayColorMode::SdrDeepColor => tessera_desktop::output::ColorPipeline::SdrDeepColor,
            DisplayColorMode::Hdr => tessera_desktop::output::ColorPipeline::Hdr,
        }
    }

    fn set_configured_icc_profiles(&mut self, profiles: HashMap<String, String>) {
        if self.configured_icc == profiles {
            return;
        }
        self.configured_icc = profiles;
        // A content-space change needs a surface rebuild, same as a
        // color-mode change.
        self.hotplug_pending = true;
    }

    fn set_gamma_gains(&mut self, gains: Option<[f32; 3]>) {
        let mut request = atomic::AtomicModeReq::new();
        let mut touched = false;
        for output in &self.displays.outputs {
            let Some((prop, size)) = output.props.crtc_gamma_lut else {
                continue;
            };
            let value = match gains {
                Some([red, green, blue]) => {
                    // drm_color_lut { red, green, blue, __reserved } per
                    // entry, linear ramp scaled by the channel gains.
                    let entries = size.max(2) as usize;
                    let mut lut = Vec::with_capacity(entries * 8);
                    for index in 0..entries {
                        let base = index as f32 / (entries - 1) as f32;
                        for gain in [red, green, blue] {
                            let scaled = (base * gain.clamp(0.0, 1.0) * 65535.0).round() as u16;
                            lut.extend_from_slice(&scaled.to_ne_bytes());
                        }
                        lut.extend_from_slice(&0u16.to_ne_bytes());
                    }
                    match self.card().create_property_blob(&lut[..]) {
                        Ok(value @ property::Value::Blob(id)) => {
                            if let Some(previous) = self.gamma_blob.replace(id) {
                                let _ = self.card().destroy_property_blob(previous);
                            }
                            value
                        }
                        other => {
                            log::warn!("drm: GAMMA_LUT blob allocation failed: {other:?}");
                            continue;
                        }
                    }
                }
                // NULL blob restores the driver's default (linear) ramp.
                None => property::Value::Blob(0),
            };
            request.add_property(output.crtc, prop, value);
            touched = true;
        }
        if !touched {
            return;
        }
        // Property-only commit: no modeset, no page-flip event.
        if let Err(error) = self
            .card()
            .atomic_commit(AtomicCommitFlags::empty(), request)
        {
            log::warn!("drm: GAMMA_LUT commit failed: {error}");
        }
        if gains.is_none()
            && let Some(previous) = self.gamma_blob.take()
        {
            let _ = self.card().destroy_property_blob(previous);
        }
    }

    fn dispatch(&mut self) -> bool {
        // Presentation ownership is exposed through `presentation_pending`.
        // Ordinary dispatch returns for *any* event so input and client
        // commits remain responsive while a page flip is in flight.
        self.pump(None, false)
    }

    fn dispatch_nonblocking(&mut self) -> bool {
        self.pump(Some(Duration::ZERO), false)
    }

    fn dispatch_timeout(&mut self, timeout: Duration) -> bool {
        self.pump(Some(timeout), false)
    }

    fn set_wakeup_fd(&mut self, fd: RawFd) {
        self.wakeup_fd = Some(fd);
    }

    fn take_input(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.input_events)
    }

    fn take_resize(&mut self) -> Option<Size> {
        let resize = self.pending_resize.take();
        if resize.is_some() {
            // Runtime will resize or recreate the Flux surface immediately
            // after consuming this event. Prevent same-slot reuse across that
            // storage boundary while retaining any still-scanned framebuffer.
            self.invalidate_composite_fb_cache();
        }
        resize
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        std::mem::take(&mut self.gesture_events)
    }

    fn is_active(&self) -> bool {
        DrmBackend::is_active(self)
    }

    fn outputs_powered(&self) -> bool {
        self.outputs_powered
    }

    fn presentation_target_ready(&self) -> bool {
        self.render_ready
    }

    fn set_outputs_powered(&mut self, powered: bool) -> Result<(), String> {
        self.set_outputs_powered(powered)
            .map_err(|error| error.to_string())
    }

    fn output_presentation_pending(&self, connector: &str) -> bool {
        self.slot_index_for_connector(connector)
            .is_some_and(|index| self.slots[index].pending)
    }

    fn presentation_pending(&self) -> bool {
        // Only a composite flip blocks the next composite frame: the Flux
        // ring image is shared by every composite output. A direct client
        // flip owns an independent buffer and may remain in flight while the
        // next composite targets other CRTCs (ADR-0185 §3).
        // Pending is only a scheduler gate when *no* composite output can
        // advance. If a fast output is idle, the runtime may present it while
        // a slower output still retires its own flip; Flux leases protect the
        // ring image that the slow CRTC is reading.
        let mut any_composite = false;
        let mut any_ready = false;
        for slot in &self.slots {
            if matches!(slot.source, Some(PrimaryPlaneSource::Composited)) {
                any_composite = true;
                any_ready |= !slot.pending;
            }
        }
        any_composite && !any_ready
    }

    fn switch_vt(&mut self, vt: i32) {
        // libseat asks the session manager (logind/seatd) to activate the
        // target VT; our own Disable/Enable cycle follows on the seat fd.
        if let Err(error) = self.seat.borrow_mut().switch_session(vt) {
            log::warn!("drm: VT switch to {vt} failed: {error}");
        }
    }

    fn surface_needs_recreate(&self) -> bool {
        self.surface_stale
    }
}

impl Drop for DrmBackend {
    fn drop(&mut self) {
        if self.active
            && !self.pending_flips.is_empty()
            && let Err(error) = self.wait_for_flip(Duration::from_secs(1))
        {
            log::warn!("drm: page flip did not settle during shutdown: {error}");
        }
        if self.active
            && self.modeset_done
            && let Err(error) = self.disable_outputs()
        {
            log::warn!("drm: failed to disable output during shutdown: {error}");
        }

        // Dropping/suspending libinput invokes close_restricted; the seat must
        // still exist while those libseat device IDs are returned.
        if let Some(input) = self.input.take() {
            input.suspend();
            drop(input);
        }
        debug_assert!(self.input_devices.borrow().is_empty());

        if self.active {
            self.clear_plane_slots(true);
            if let Some(overlay) = self.current_overlay.take() {
                self.release_overlay_scanout(overlay);
            }
            if let Some(overlay) = self.retiring_overlay.take() {
                self.release_overlay_scanout(overlay);
            }
            // Cache owns compositor-output framebuffer/GEM resources; release
            // all of them exactly once while the originating card fd is live.
            if self.card.is_some() {
                self.destroy_composite_fb_cache();
            }
        } else {
            // libseat has revoked this fd. Kernel resources died with it, and
            // cleanup ioctls must not be sent to a future/reused handle.
            self.clear_plane_slots(false);
            self.current_overlay = None;
            self.retiring_overlay = None;
            self.forget_composite_fb_cache();
        }
        if self.active
            && let Some(card) = self.card.as_ref()
        {
            for cursor in self.cursor_buffers.drain(..) {
                if let Err(error) = card.destroy_framebuffer(cursor.framebuffer) {
                    log::warn!("DRM: failed to destroy cursor framebuffer: {error}");
                }
                if let Err(error) = card.destroy_dumb_buffer(cursor.dumb) {
                    log::warn!("DRM: failed to destroy cursor buffer: {error}");
                }
            }
        } else {
            // Revoked card: the kernel destroyed these records with the fd.
            self.cursor_buffers.clear();
        }
        if let Some(card) = self.card.take() {
            if self.active {
                for output in &self.displays.outputs {
                    let _ = card.destroy_property_blob(output.mode_blob_id);
                    if let Some((_, blob_id)) = output.planes.primary.full_damage_blob.as_ref() {
                        let _ = card.destroy_property_blob(*blob_id);
                    }
                }
            }
            if let Err(error) = self.seat.borrow_mut().close_device(card.device) {
                log::warn!(
                    "libseat: failed to close {}: {error:?}",
                    card.path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
