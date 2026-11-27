//! `wp_linux_drm_syncobj_v1` explicit synchronization protocol (ADR-0266).
//!
//! Implements Linux DRM timeline synchronization objects so modern Wayland
//! clients (Chromium/Chrome, Firefox, Mesa EGL/Vulkan WSI) can synchronize
//! DMA-BUF acquire and release points directly on the GPU timeline without
//! falling back to implicit synchronization.

use super::*;
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;

// ----- DRM syncobj UAPI (<drm/drm.h>) ---------------------------------------

const DRM_CAP_SYNCOBJ_TIMELINE: u64 = 0x14;

const DRM_SYNCOBJ_CREATE_SIGNALED: u32 = 1 << 0;
const DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE: u32 = 1 << 0;
const DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE: u32 = 1 << 0;
const DRM_SYNCOBJ_WAIT_FLAGS_WAIT_AVAILABLE: u32 = 1 << 2;

// Exact _IOWR(DRM_IOCTL_BASE='d', nr, struct) encodings verified against
// <drm/drm.h>. Using the 16-byte `DrmSyncobjHandleV1` layout + TRANSFER works
// on every Linux kernel with timeline syncobj support (>= 5.2).
const DRM_IOCTL_GET_CAP: c_ulong = 0xC010_640C;
const DRM_IOCTL_SYNCOBJ_CREATE: c_ulong = 0xC008_64BF;
const DRM_IOCTL_SYNCOBJ_DESTROY: c_ulong = 0xC008_64C0;
const DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD: c_ulong = 0xC010_64C1;
const DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE: c_ulong = 0xC010_64C2;
const DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT: c_ulong = 0xC030_64CA;
const DRM_IOCTL_SYNCOBJ_TRANSFER: c_ulong = 0xC020_64CC;
const DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL: c_ulong = 0xC018_64CD;

#[repr(C)]
struct DrmGetCap {
    capability: u64,
    value: u64,
}

#[repr(C)]
struct DrmSyncobjCreate {
    handle: u32,
    flags: u32,
}

#[repr(C)]
struct DrmSyncobjDestroy {
    handle: u32,
    pad: u32,
}

#[repr(C)]
struct DrmSyncobjHandleV1 {
    handle: u32,
    flags: u32,
    fd: i32,
    pad: u32,
}

#[repr(C)]
struct DrmSyncobjTransfer {
    src_handle: u32,
    dst_handle: u32,
    src_point: u64,
    dst_point: u64,
    flags: u32,
    pad: u32,
}

#[repr(C)]
struct DrmSyncobjTimelineArray {
    handles: u64,
    points: u64,
    count_handles: u32,
    flags: u32,
}

#[repr(C)]
struct DrmSyncobjTimelineWait {
    handles: u64,
    points: u64,
    timeout_nsec: i64,
    count_handles: u32,
    flags: u32,
    first_signaled: u32,
    pad: u32,
    deadline_nsec: u64,
}

const _: () = assert!(std::mem::size_of::<DrmGetCap>() == 16);
const _: () = assert!(std::mem::size_of::<DrmSyncobjCreate>() == 8);
const _: () = assert!(std::mem::size_of::<DrmSyncobjDestroy>() == 8);
const _: () = assert!(std::mem::size_of::<DrmSyncobjHandleV1>() == 16);
const _: () = assert!(std::mem::size_of::<DrmSyncobjTransfer>() == 32);
const _: () = assert!(std::mem::size_of::<DrmSyncobjTimelineArray>() == 24);
const _: () = assert!(std::mem::size_of::<DrmSyncobjTimelineWait>() == 48);

// Protocol error codes (linux-drm-syncobj-v1.xml).
const MANAGER_ERR_SURFACE_EXISTS: u32 = 0;
const MANAGER_ERR_INVALID_TIMELINE: u32 = 1;

const SURFACE_ERR_NO_SURFACE: u32 = 1;
const SURFACE_ERR_UNSUPPORTED_BUFFER: u32 = 2;
const SURFACE_ERR_NO_BUFFER: u32 = 3;
const SURFACE_ERR_NO_ACQUIRE_POINT: u32 = 4;
const SURFACE_ERR_NO_RELEASE_POINT: u32 = 5;
const SURFACE_ERR_CONFLICTING_POINTS: u32 = 6;

/// Bounded sample-time wait when a client committed a future acquire timeline
/// point whose GPU batch still had not arrived by render time (2 ms, never
/// called during `wl_surface.commit`).
const SAMPLE_ACQUIRE_AVAILABLE_WAIT_NS: u64 = 2_000_000;

/// Open DRM render node supporting `DRM_CAP_SYNCOBJ_TIMELINE`.
pub(crate) struct SyncobjDevice {
    fd: OwnedFd,
}

impl SyncobjDevice {
    /// Open the DRM render node matching `main_device` (from Vulkan/KMS
    /// dmabuf feedback) and verify `DRM_CAP_SYNCOBJ_TIMELINE == 1`.
    pub(crate) fn open_for_device(main_device: Option<u64>) -> Option<Rc<Self>> {
        let drm_fd = open_render_node(main_device)?;
        if !supports_timeline_syncobj(drm_fd.as_raw_fd()) {
            log::info!("syncobj: DRM render node lacks DRM_CAP_SYNCOBJ_TIMELINE");
            return None;
        }
        Some(Rc::new(Self { fd: drm_fd }))
    }

    fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    fn import_fd(self: &Rc<Self>, syncobj_fd: RawFd) -> Result<Rc<SyncobjTimeline>, i32> {
        if syncobj_fd < 0 {
            return Err(22); // EINVAL
        }
        let mut req = DrmSyncobjHandleV1 {
            handle: 0,
            flags: 0,
            fd: syncobj_fd,
            pad: 0,
        };
        let rc = unsafe { crate::ioctl(self.raw_fd(), DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE, &mut req) };
        if rc != 0 || req.handle == 0 {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(22));
        }
        Ok(Rc::new(SyncobjTimeline {
            device: Rc::clone(self),
            handle: req.handle,
        }))
    }

    fn create_temp_binary(&self, signaled: bool) -> Option<TempSyncobj<'_>> {
        let mut req = DrmSyncobjCreate {
            handle: 0,
            flags: if signaled {
                DRM_SYNCOBJ_CREATE_SIGNALED
            } else {
                0
            },
        };
        let rc = unsafe { crate::ioctl(self.raw_fd(), DRM_IOCTL_SYNCOBJ_CREATE, &mut req) };
        if rc != 0 || req.handle == 0 {
            return None;
        }
        Some(TempSyncobj {
            device: self,
            handle: req.handle,
        })
    }
}

struct TempSyncobj<'a> {
    device: &'a SyncobjDevice,
    handle: u32,
}

impl Drop for TempSyncobj<'_> {
    fn drop(&mut self) {
        let mut req = DrmSyncobjDestroy {
            handle: self.handle,
            pad: 0,
        };
        let _ = unsafe { crate::ioctl(self.device.raw_fd(), DRM_IOCTL_SYNCOBJ_DESTROY, &mut req) };
    }
}

/// Reference-counted kernel DRM syncobj timeline handle.
///
/// Protocol `wp_linux_drm_syncobj_timeline_v1` objects and committed acquire /
/// release points each hold an `Rc<SyncobjTimeline>`, so destroying the client
/// timeline resource never invalidates points already latched by a commit.
pub(crate) struct SyncobjTimeline {
    device: Rc<SyncobjDevice>,
    handle: u32,
}

impl SyncobjTimeline {
    /// Non-blocking attempt to materialize `(self.handle, point)` into an owned
    /// Linux `sync_file` fd (used on `wl_surface.commit` so Wayland dispatch
    /// never blocks the 120 Hz frame loop).
    pub(crate) fn try_export_point_sync_file(&self, point: u64) -> Option<OwnedFd> {
        self.export_point_sync_file_inner(point, 0)
    }

    /// Materialize a timeline point at sample time, allowing a brief bounded
    /// `DRM_SYNCOBJ_WAIT_FLAGS_WAIT_AVAILABLE` wait if the client submitted a
    /// future point slightly after `wl_surface.commit`.
    pub(crate) fn export_point_sync_file(&self, point: u64) -> Option<OwnedFd> {
        self.export_point_sync_file_inner(point, SAMPLE_ACQUIRE_AVAILABLE_WAIT_NS)
    }

    fn export_point_sync_file_inner(&self, point: u64, wait_available_ns: u64) -> Option<OwnedFd> {
        let tmp = self.device.create_temp_binary(false)?;
        let transfer = |drm_fd: RawFd, src_handle: u32, dst_handle: u32, src_point: u64| -> c_int {
            let mut xfer = DrmSyncobjTransfer {
                src_handle,
                dst_handle,
                src_point,
                dst_point: 0,
                flags: 0,
                pad: 0,
            };
            unsafe { crate::ioctl(drm_fd, DRM_IOCTL_SYNCOBJ_TRANSFER, &mut xfer) }
        };

        if transfer(self.device.raw_fd(), self.handle, tmp.handle, point) != 0 {
            if wait_available_ns == 0 {
                return None;
            }
            // The fence at `point` was not materialized yet (future submit).
            let handle = self.handle;
            let deadline_ns = crate::monotonic_ns().saturating_add(wait_available_ns);
            let mut wait = DrmSyncobjTimelineWait {
                handles: (&handle as *const u32) as usize as u64,
                points: (&point as *const u64) as usize as u64,
                timeout_nsec: deadline_ns.min(i64::MAX as u64) as i64,
                count_handles: 1,
                flags: DRM_SYNCOBJ_WAIT_FLAGS_WAIT_AVAILABLE,
                first_signaled: 0,
                pad: 0,
                deadline_nsec: 0,
            };
            let _ = unsafe {
                crate::ioctl(
                    self.device.raw_fd(),
                    DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT,
                    &mut wait,
                )
            };
            if transfer(self.device.raw_fd(), self.handle, tmp.handle, point) != 0 {
                log::warn!(
                    "syncobj: failed to materialize acquire point {point} on handle {}: {}",
                    self.handle,
                    std::io::Error::last_os_error()
                );
                return None;
            }
        }

        let mut export = DrmSyncobjHandleV1 {
            handle: tmp.handle,
            flags: DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE,
            fd: -1,
            pad: 0,
        };
        let rc = unsafe {
            crate::ioctl(
                self.device.raw_fd(),
                DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,
                &mut export,
            )
        };
        if rc != 0 || export.fd < 0 {
            log::warn!(
                "syncobj: failed to export sync_file for acquire point {point}: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        // SAFETY: `DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD(EXPORT_SYNC_FILE)` returns a
        // fresh owned sync_file descriptor.
        Some(unsafe { OwnedFd::from_raw_fd(export.fd) })
    }

    /// Signal `point` immediately on the CPU (used when GPU/KMS reads already
    /// completed or when a buffer is released without ever being presented).
    fn signal_point_immediate(&self, point: u64) {
        let handle = self.handle;
        let mut req = DrmSyncobjTimelineArray {
            handles: (&handle as *const u32) as usize as u64,
            points: (&point as *const u64) as usize as u64,
            count_handles: 1,
            flags: 0,
        };
        let rc = unsafe {
            crate::ioctl(
                self.device.raw_fd(),
                DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL,
                &mut req,
            )
        };
        if rc != 0 {
            log::warn!(
                "syncobj: failed to signal timeline point {point} on handle {}: {}",
                self.handle,
                std::io::Error::last_os_error()
            );
        }
    }

    /// Transfer a live `sync_file` completion fence onto `(self.handle, point)`.
    ///
    /// Returns `true` when the kernel accepted the transfer without blocking.
    /// Callers fall back to waiting on `sync_file_fd` and signaling immediately
    /// if this returns `false`.
    fn transfer_sync_file_to_point(&self, point: u64, sync_file_fd: RawFd) -> bool {
        if sync_file_fd < 0 {
            return false;
        }
        let Some(tmp) = self.device.create_temp_binary(false) else {
            return false;
        };
        let mut import = DrmSyncobjHandleV1 {
            handle: tmp.handle,
            flags: DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE,
            fd: sync_file_fd,
            pad: 0,
        };
        if unsafe {
            crate::ioctl(
                self.device.raw_fd(),
                DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE,
                &mut import,
            )
        } != 0
        {
            log::warn!(
                "syncobj: IMPORT_SYNC_FILE failed for release point {point}: {}",
                std::io::Error::last_os_error()
            );
            return false;
        }
        let mut xfer = DrmSyncobjTransfer {
            src_handle: tmp.handle,
            dst_handle: self.handle,
            src_point: 0,
            dst_point: point,
            flags: 0,
            pad: 0,
        };
        if unsafe { crate::ioctl(self.device.raw_fd(), DRM_IOCTL_SYNCOBJ_TRANSFER, &mut xfer) } != 0
        {
            log::warn!(
                "syncobj: TRANSFER to release point {point} failed: {}",
                std::io::Error::last_os_error()
            );
            return false;
        }
        true
    }
}

impl Drop for SyncobjTimeline {
    fn drop(&mut self) {
        let mut req = DrmSyncobjDestroy {
            handle: self.handle,
            pad: 0,
        };
        let _ = unsafe { crate::ioctl(self.device.raw_fd(), DRM_IOCTL_SYNCOBJ_DESTROY, &mut req) };
    }
}

/// Latched release timeline point waiting for compositor reads to complete.
///
/// Always signals on [`Self::signal`] or on [`Drop`] so a torn-down surface or
/// compositor can never deadlock a client awaiting buffer release.
pub(crate) struct SyncobjRelease {
    timeline: Rc<SyncobjTimeline>,
    point: u64,
    signaled: bool,
}

impl SyncobjRelease {
    pub(crate) fn new(timeline: Rc<SyncobjTimeline>, point: u64) -> Self {
        Self {
            timeline,
            point,
            signaled: false,
        }
    }

    /// Signal the release timeline point, pipelining `completion_fence` onto
    /// the DRM timeline when live. If timeline transfer fails while a live
    /// fence is present, waits on `completion_fence` before signaling so the
    /// client never overwrites a buffer still in use.
    pub(crate) fn signal(&mut self, completion_fence: Option<RawFd>) {
        if self.signaled {
            return;
        }
        self.signaled = true;
        if let Some(fence_fd) = completion_fence
            && fence_fd >= 0
        {
            if self
                .timeline
                .transfer_sync_file_to_point(self.point, fence_fd)
            {
                return;
            }
            crate::wait_sync_file_fd(fence_fd, 1_000);
        }
        self.timeline.signal_point_immediate(self.point);
    }
}

impl Drop for SyncobjRelease {
    fn drop(&mut self) {
        if !self.signaled {
            self.signaled = true;
            self.timeline.signal_point_immediate(self.point);
        }
    }
}

fn supports_timeline_syncobj(drm_fd: RawFd) -> bool {
    let mut cap = DrmGetCap {
        capability: DRM_CAP_SYNCOBJ_TIMELINE,
        value: 0,
    };
    let rc = unsafe { crate::ioctl(drm_fd, DRM_IOCTL_GET_CAP, &mut cap) };
    rc == 0 && cap.value != 0
}

fn open_render_node(main_device: Option<u64>) -> Option<OwnedFd> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return None;
    };
    let mut render_paths: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("renderD"))
        })
        .collect();
    render_paths.sort();

    if let Some(target_rdev) = main_device {
        // 1. Exact rdev match (typical: Vulkan reports renderD128 directly).
        for path in &render_paths {
            if let Ok(meta) = std::fs::metadata(path)
                && meta.rdev() == target_rdev
                && let Ok(file) = OpenOptions::new().read(true).write(true).open(path)
            {
                return Some(OwnedFd::from(file));
            }
        }
        // 2. Primary DRM node (major 226, minor < 64) -> matching render node
        //    under the same sysfs device directory.
        let target_major = linux_major(target_rdev);
        let target_minor = linux_minor(target_rdev);
        if target_major == 226 && target_minor < 64 {
            let drm_dir = format!("/sys/dev/char/{target_major}:{target_minor}/device/drm");
            if let Ok(dir) = std::fs::read_dir(drm_dir) {
                for entry in dir.filter_map(Result::ok) {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with("renderD") {
                        let candidate = std::path::Path::new("/dev/dri").join(name);
                        if let Ok(file) = OpenOptions::new().read(true).write(true).open(&candidate)
                        {
                            return Some(OwnedFd::from(file));
                        }
                    }
                }
            }
        }
        return None;
    }

    // No explicit main device requested (e.g. unit tests): open first render node.
    for path in &render_paths {
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(path) {
            return Some(OwnedFd::from(file));
        }
    }
    None
}

fn linux_major(dev: u64) -> u32 {
    (((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)) as u32
}

fn linux_minor(dev: u64) -> u32 {
    ((dev & 0xff) | ((dev >> 12) & !0xff)) as u32
}

// ----- Wayland protocol handlers --------------------------------------------

struct SyncobjTimelineRec {
    timeline: Rc<SyncobjTimeline>,
}

struct SyncobjPoint {
    timeline: Rc<SyncobjTimeline>,
    point: u64,
}

struct SyncobjSurfaceRec {
    resource: *mut ffi::wl_resource,
    surface: *mut SurfaceRec,
    pending_acquire: Option<SyncobjPoint>,
    pending_release: Option<SyncobjPoint>,
}

static SYNCOBJ_MANAGER_IMPL: ffi::wp_linux_drm_syncobj_manager_v1_interface_impl =
    ffi::wp_linux_drm_syncobj_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_surface: syncobj_manager_get_surface,
        import_timeline: syncobj_manager_import_timeline,
    };

static SYNCOBJ_TIMELINE_IMPL: ffi::wp_linux_drm_syncobj_timeline_v1_interface_impl =
    ffi::wp_linux_drm_syncobj_timeline_v1_interface_impl {
        destroy: crate::res_destroy,
    };

static SYNCOBJ_SURFACE_IMPL: ffi::wp_linux_drm_syncobj_surface_v1_interface_impl =
    ffi::wp_linux_drm_syncobj_surface_v1_interface_impl {
        destroy: crate::res_destroy,
        set_acquire_point: syncobj_surface_set_acquire_point,
        set_release_point: syncobj_surface_set_release_point,
    };

pub(crate) unsafe extern "C" fn drm_syncobj_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::checked_resource_create(
            client,
            &ffi::wp_linux_drm_syncobj_manager_v1_interface,
            version.min(1) as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &SYNCOBJ_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn syncobj_manager_get_surface(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    id: u32,
    surface_resource: *mut ffi::wl_resource,
) {
    unsafe {
        let surface = ffi::wl_resource_get_user_data(surface_resource) as *mut SurfaceRec;
        if surface.is_null() {
            return;
        }
        if !(*surface).drm_syncobj.is_null() || !(*surface).explicit_sync.is_null() {
            ffi::wl_resource_post_error(
                resource,
                MANAGER_ERR_SURFACE_EXISTS,
                c"surface already has an explicit synchronization object".as_ptr(),
            );
            return;
        }
        let obj = ffi::checked_resource_create(
            client,
            &ffi::wp_linux_drm_syncobj_surface_v1_interface,
            1,
            id,
        );
        if obj.is_null() {
            return;
        }
        let record = Box::into_raw(Box::new(SyncobjSurfaceRec {
            resource: obj,
            surface,
            pending_acquire: None,
            pending_release: None,
        }));
        (*surface).drm_syncobj = record as *mut c_void;
        ffi::wl_resource_set_implementation(
            obj,
            &SYNCOBJ_SURFACE_IMPL as *const _ as *const c_void,
            record as *mut c_void,
            Some(syncobj_surface_resource_destroy),
        );
    }
}

unsafe extern "C" fn syncobj_manager_import_timeline(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    id: u32,
    fd: c_int,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        let device = if state.is_null() {
            None
        } else {
            (*state).syncobj_device.as_ref().map(Rc::clone)
        };
        let imported = device.and_then(|dev| dev.import_fd(fd).ok());
        // The Wayland protocol transfers ownership of `fd` to the server.
        if fd >= 0 {
            crate::libc_close(fd);
        }
        let Some(timeline) = imported else {
            ffi::wl_resource_post_error(
                resource,
                MANAGER_ERR_INVALID_TIMELINE,
                c"failed to import DRM syncobj timeline fd".as_ptr(),
            );
            return;
        };
        let obj = ffi::checked_resource_create(
            client,
            &ffi::wp_linux_drm_syncobj_timeline_v1_interface,
            1,
            id,
        );
        if obj.is_null() {
            return;
        }
        let record = Box::into_raw(Box::new(SyncobjTimelineRec { timeline }));
        ffi::wl_resource_set_implementation(
            obj,
            &SYNCOBJ_TIMELINE_IMPL as *const _ as *const c_void,
            record as *mut c_void,
            Some(syncobj_timeline_resource_destroy),
        );
    }
}

unsafe extern "C" fn syncobj_timeline_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SyncobjTimelineRec;
        if !record.is_null() {
            drop(Box::from_raw(record));
        }
    }
}

fn combine_point(point_hi: u32, point_lo: u32) -> u64 {
    (u64::from(point_hi) << 32) | u64::from(point_lo)
}

unsafe extern "C" fn syncobj_surface_set_acquire_point(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    timeline_resource: *mut ffi::wl_resource,
    point_hi: u32,
    point_lo: u32,
) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SyncobjSurfaceRec;
        if record.is_null() || (*record).surface.is_null() {
            ffi::wl_resource_post_error(
                resource,
                SURFACE_ERR_NO_SURFACE,
                c"associated wl_surface was destroyed".as_ptr(),
            );
            return;
        }
        let tl_rec = ffi::wl_resource_get_user_data(timeline_resource) as *mut SyncobjTimelineRec;
        if tl_rec.is_null() {
            return;
        }
        (*record).pending_acquire = Some(SyncobjPoint {
            timeline: Rc::clone(&(*tl_rec).timeline),
            point: combine_point(point_hi, point_lo),
        });
    }
}

unsafe extern "C" fn syncobj_surface_set_release_point(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    timeline_resource: *mut ffi::wl_resource,
    point_hi: u32,
    point_lo: u32,
) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SyncobjSurfaceRec;
        if record.is_null() || (*record).surface.is_null() {
            ffi::wl_resource_post_error(
                resource,
                SURFACE_ERR_NO_SURFACE,
                c"associated wl_surface was destroyed".as_ptr(),
            );
            return;
        }
        let tl_rec = ffi::wl_resource_get_user_data(timeline_resource) as *mut SyncobjTimelineRec;
        if tl_rec.is_null() {
            return;
        }
        (*record).pending_release = Some(SyncobjPoint {
            timeline: Rc::clone(&(*tl_rec).timeline),
            point: combine_point(point_hi, point_lo),
        });
    }
}

unsafe extern "C" fn syncobj_surface_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SyncobjSurfaceRec;
        if record.is_null() {
            return;
        }
        if !(*record).surface.is_null() && (*(*record).surface).drm_syncobj == record as *mut c_void
        {
            (*(*record).surface).drm_syncobj = std::ptr::null_mut();
        }
        drop(Box::from_raw(record));
    }
}

/// Validate double-buffered `wp_linux_drm_syncobj_surface_v1` state on
/// `wl_surface.commit`, materializing the acquire timeline point into a
/// `sync_file` and latching the release timeline point onto `surface`.
///
/// Returns `false` when a protocol error was posted (caller must abort commit).
pub(crate) unsafe fn drm_syncobj_surface_committed(
    surface: *mut SurfaceRec,
    buffer_set: bool,
    buffer: *mut ffi::wl_resource,
) -> bool {
    unsafe {
        if surface.is_null() || (*surface).drm_syncobj.is_null() {
            return true;
        }
        let record = (*surface).drm_syncobj as *mut SyncobjSurfaceRec;
        let pending_acquire = (*record).pending_acquire.take();
        let pending_release = (*record).pending_release.take();

        let has_buffer = buffer_set && !buffer.is_null();
        if !has_buffer {
            if pending_acquire.is_some() || pending_release.is_some() {
                ffi::wl_resource_post_error(
                    (*record).resource,
                    SURFACE_ERR_NO_BUFFER,
                    c"syncobj acquire or release point set without a buffer".as_ptr(),
                );
                return false;
            }
            return true;
        }

        let is_dmabuf = ffi::wl_resource_instance_of(
            buffer,
            &ffi::wl_buffer_interface,
            &crate::WL_BUFFER_IMPL as *const _ as *const c_void,
        ) != 0;
        if !is_dmabuf {
            ffi::wl_resource_post_error(
                (*record).resource,
                SURFACE_ERR_UNSUPPORTED_BUFFER,
                c"attached buffer does not support linux-drm-syncobj explicit synchronization"
                    .as_ptr(),
            );
            return false;
        }

        let Some(acquire) = pending_acquire else {
            ffi::wl_resource_post_error(
                (*record).resource,
                SURFACE_ERR_NO_ACQUIRE_POINT,
                c"dmabuf committed without a syncobj acquire point".as_ptr(),
            );
            return false;
        };
        let Some(release) = pending_release else {
            ffi::wl_resource_post_error(
                (*record).resource,
                SURFACE_ERR_NO_RELEASE_POINT,
                c"dmabuf committed without a syncobj release point".as_ptr(),
            );
            return false;
        };
        if Rc::ptr_eq(&acquire.timeline, &release.timeline) && acquire.point >= release.point {
            ffi::wl_resource_post_error(
                (*record).resource,
                SURFACE_ERR_CONFLICTING_POINTS,
                c"acquire point must be strictly less than release point on the same timeline"
                    .as_ptr(),
            );
            return false;
        }

        if (*surface).committed_acquire_fence >= 0 {
            crate::libc_close((*surface).committed_acquire_fence);
            (*surface).committed_acquire_fence = -1;
        }
        (*surface).committed_syncobj_acquire = None;
        if let Some(sync_file) = acquire.timeline.try_export_point_sync_file(acquire.point) {
            (*surface).committed_acquire_fence = std::os::fd::IntoRawFd::into_raw_fd(sync_file);
        } else {
            // Fence not materialized yet at commit time: defer to sample time
            // rather than blocking Wayland dispatch.
            (*surface).committed_syncobj_acquire = Some((acquire.timeline, acquire.point));
        }
        (*surface).committed_syncobj_release =
            Some(SyncobjRelease::new(release.timeline, release.point));
        true
    }
}

pub(crate) unsafe fn drm_syncobj_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).drm_syncobj.is_null() {
            return;
        }
        let record = (*surface).drm_syncobj as *mut SyncobjSurfaceRec;
        (*record).surface = std::ptr::null_mut();
        (*surface).drm_syncobj = std::ptr::null_mut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_timeline_point(timeline: &SyncobjTimeline) -> u64 {
        const DRM_IOCTL_SYNCOBJ_QUERY: c_ulong = 0xC018_64CB;
        let handle = timeline.handle;
        let mut point: u64 = 0;
        let mut req = DrmSyncobjTimelineArray {
            handles: (&handle as *const u32) as usize as u64,
            points: (&mut point as *mut u64) as usize as u64,
            count_handles: 1,
            flags: 0,
        };
        let rc =
            unsafe { crate::ioctl(timeline.device.raw_fd(), DRM_IOCTL_SYNCOBJ_QUERY, &mut req) };
        assert_eq!(rc, 0, "DRM_IOCTL_SYNCOBJ_QUERY must succeed");
        point
    }

    #[test]
    fn combine_point_packs_hi_and_lo_words() {
        assert_eq!(combine_point(0, 1), 1);
        assert_eq!(
            combine_point(0x1234_5678, 0x9ABC_DEF0),
            0x1234_5678_9ABC_DEF0
        );
    }

    #[test]
    fn test_inv_arch_159_syncobj_timeline_acquire_export_and_release_signal_roundtrip() {
        let Some(device) = SyncobjDevice::open_for_device(None) else {
            return;
        };
        // Create a real timeline syncobj, export its fd, import it through
        // `SyncobjDevice::import_fd` (identical to `import_timeline`), signal
        // point 3, export a sync_file for point 3, then use that sync_file to
        // signal release point 4 via `SyncobjRelease::signal(Some(fd))`.
        let mut create = DrmSyncobjCreate {
            handle: 0,
            flags: 0,
        };
        assert_eq!(
            unsafe { crate::ioctl(device.raw_fd(), DRM_IOCTL_SYNCOBJ_CREATE, &mut create) },
            0
        );
        let mut to_fd = DrmSyncobjHandleV1 {
            handle: create.handle,
            flags: 0,
            fd: -1,
            pad: 0,
        };
        assert_eq!(
            unsafe { crate::ioctl(device.raw_fd(), DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &mut to_fd) },
            0
        );
        let mut destroy_orig = DrmSyncobjDestroy {
            handle: create.handle,
            pad: 0,
        };
        let _ = unsafe {
            crate::ioctl(
                device.raw_fd(),
                DRM_IOCTL_SYNCOBJ_DESTROY,
                &mut destroy_orig,
            )
        };

        let timeline = device
            .import_fd(to_fd.fd)
            .expect("import_fd must import valid syncobj fd");
        unsafe { crate::libc_close(to_fd.fd) };

        // Signal acquire point 3 and export a real sync_file from it.
        timeline.signal_point_immediate(3);
        let sf = timeline
            .export_point_sync_file(3)
            .expect("export_point_sync_file must succeed on signaled point");
        assert!(sf.as_raw_fd() >= 0);

        // Signal release point 4 using the live sync_file transfer path.
        let mut rel = SyncobjRelease::new(Rc::clone(&timeline), 4);
        rel.signal(Some(sf.as_raw_fd()));
        assert_eq!(
            query_timeline_point(&timeline),
            4,
            "release transfer must advance kernel timeline to point 4"
        );

        // Dropping an unsignaled SyncobjRelease must signal point 5 rather than
        // deadlocking the client.
        {
            let _orphan = SyncobjRelease::new(Rc::clone(&timeline), 5);
        }
        assert_eq!(
            query_timeline_point(&timeline),
            5,
            "dropping an unsignaled SyncobjRelease must signal its release point"
        );
    }
}
