//! Linux `sync_file` fence helpers shared by DRM presentation and runtime
//! buffer-retirement scheduling (ADR-0250, ADR-0264).
//!
//! Kernel UAPI labels are encoded at compile time so the hot presentation loop
//! has zero runtime slice indexing or `copy_from_slice` panic paths. Fence
//! merges return [`CompletionFence`] so a failed `SYNC_IOC_MERGE` ioctl can
//! never masquerade as "already signaled" (`[INV-ARCH-159]`).

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
    )
)]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

#[repr(C)]
struct SyncMergeData {
    name: [u8; 32],
    fd2: i32,
    fence: i32,
    flags: u32,
    pad: u32,
}

/// `SYNC_IOC_MERGE` (`_IOWR('>', 3, struct sync_merge_data)`) from
/// `<linux/sync_file.h>`. The UAPI encoding and `sync_merge_data` layout are
/// stable across Linux architectures supported by Tessera.
const SYNC_IOC_MERGE: libc::c_ulong = 0xC030_3E03;

const _: () = assert!(
    std::mem::size_of::<SyncMergeData>() == 48,
    "struct sync_merge_data ABI size must match <linux/sync_file.h> (0x30)"
);

/// Encode an ASCII label into a NUL-terminated fixed-size kernel UAPI buffer at
/// compile time. Compilation fails if `bytes` cannot leave room for a trailing
/// NUL byte or contains an interior NUL.
#[allow(clippy::indexing_slicing, clippy::panic)]
const fn fixed_cstr<const N: usize>(bytes: &[u8]) -> [u8; N] {
    assert!(
        bytes.len() < N,
        "kernel UAPI label must leave room for trailing NUL"
    );
    let mut out = [0u8; N];
    let mut i = 0;
    while i < bytes.len() {
        assert!(bytes[i] != 0, "kernel UAPI label must not contain NUL");
        out[i] = bytes[i];
        i += 1;
    }
    out
}

/// Debug label passed to `SYNC_IOC_MERGE` (`struct sync_merge_data.name`).
const MERGE_FENCE_NAME: [u8; 32] = fixed_cstr(b"tessera-kms");

/// Typed completion state for GPU/KMS `sync_file` fences (ADR-0264,
/// `[INV-ARCH-159]`).
///
/// Unlike `Option<OwnedFd>`, this type separates "all contributing work is
/// already signaled" from "live fences could not be merged into one fd", making
/// fail-open premature buffer release unrepresentable.
#[derive(Debug, Default)]
pub enum CompletionFence {
    /// No unsignaled fences remain (empty set, implicit-sync path already
    /// waited, or every fence already signaled).
    #[default]
    Signaled,
    /// One live `sync_file` whose signaling proves every contributing GPU/KMS
    /// read has completed.
    Live(OwnedFd),
    /// `SYNC_IOC_MERGE` failed while two or more fences remained live. Callers
    /// must wait for these fences (or synchronize the device) before releasing
    /// retired client buffers.
    Unmerged(Vec<OwnedFd>),
}

impl CompletionFence {
    /// Wrap an optional `sync_file`, collapsing an already-signaled fence to
    /// [`Self::Signaled`].
    pub fn from_option(fence: Option<OwnedFd>) -> Self {
        match fence {
            Some(fd) if !sync_file_is_signaled(fd.as_raw_fd()) => Self::Live(fd),
            _ => Self::Signaled,
        }
    }

    /// Borrow the single live `sync_file` when in [`Self::Live`].
    pub fn live_fd(&self) -> Option<&OwnedFd> {
        match self {
            Self::Live(fd) => Some(fd),
            Self::Signaled | Self::Unmerged(_) => None,
        }
    }

    /// Borrow the raw descriptor when in [`Self::Live`].
    pub fn as_raw_fd(&self) -> Option<RawFd> {
        self.live_fd().map(AsRawFd::as_raw_fd)
    }

    /// Whether every underlying fence has already signaled.
    pub fn is_signaled(&self) -> bool {
        match self {
            Self::Signaled => true,
            Self::Live(fd) => sync_file_is_signaled(fd.as_raw_fd()),
            Self::Unmerged(fds) => fds
                .iter()
                .all(|fence| sync_file_is_signaled(fence.as_raw_fd())),
        }
    }

    /// Drop any fences that have signaled since this value was constructed.
    pub fn prune_signaled(self) -> Self {
        match self {
            Self::Signaled => Self::Signaled,
            Self::Live(fd) => {
                if sync_file_is_signaled(fd.as_raw_fd()) {
                    Self::Signaled
                } else {
                    Self::Live(fd)
                }
            }
            Self::Unmerged(fds) => {
                let mut live: Vec<OwnedFd> = fds
                    .into_iter()
                    .filter(|fence| !sync_file_is_signaled(fence.as_raw_fd()))
                    .collect();
                match live.len() {
                    0 => Self::Signaled,
                    1 => match live.pop() {
                        Some(fd) => Self::Live(fd),
                        None => Self::Signaled,
                    },
                    _ => Self::Unmerged(live),
                }
            }
        }
    }

    /// Move all owned fences into `out` for a subsequent merge.
    pub fn append_to(self, out: &mut Vec<OwnedFd>) {
        match self {
            Self::Signaled => {}
            Self::Live(fd) => out.push(fd),
            Self::Unmerged(fds) => out.extend(fds),
        }
    }

    /// Duplicate the live descriptor(s) held by this completion state.
    /// Returns `None` only if `dup()` fails on an unsignaled descriptor.
    pub fn try_clone(&self) -> Option<Self> {
        match self {
            Self::Signaled => Some(Self::Signaled),
            Self::Live(fd) => {
                if sync_file_is_signaled(fd.as_raw_fd()) {
                    Some(Self::Signaled)
                } else {
                    fd.try_clone().ok().map(Self::Live)
                }
            }
            Self::Unmerged(fds) => {
                let mut cloned = Vec::with_capacity(fds.len());
                for fd in fds {
                    if !sync_file_is_signaled(fd.as_raw_fd()) {
                        cloned.push(fd.try_clone().ok()?);
                    }
                }
                match cloned.len() {
                    0 => Some(Self::Signaled),
                    1 => cloned.pop().map(Self::Live),
                    _ => Some(Self::Unmerged(cloned)),
                }
            }
        }
    }

    /// Block until every live fence signals or `timeout` elapses.
    ///
    /// Returns `true` only when every fence has positively reported readiness
    /// (`POLLIN`). Invalid descriptors, `poll` failures, or timeouts return
    /// `false` so callers can escalate to a broader synchronization fallback.
    pub fn wait_all(&self, timeout: Duration) -> bool {
        match self {
            Self::Signaled => true,
            Self::Live(fd) => wait_raw_fences(&[fd.as_raw_fd()], timeout),
            Self::Unmerged(fds) => {
                let raw: Vec<RawFd> = fds.iter().map(AsRawFd::as_raw_fd).collect();
                wait_raw_fences(&raw, timeout)
            }
        }
    }
}

/// Whether a Linux `sync_file` (or pollable fence fd) has already signaled.
///
/// Uses a non-blocking `poll(POLLIN, timeout = 0)`. Invalid descriptors return
/// `false` so callers fail safe rather than falsely claiming completion.
pub fn sync_file_is_signaled(fd: RawFd) -> bool {
    if fd < 0 {
        return false;
    }
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
    rc > 0 && (pfd.revents & libc::POLLIN) != 0
}

fn wait_raw_fences(raw_fds: &[RawFd], timeout: Duration) -> bool {
    let mut pending: Vec<libc::pollfd> = Vec::with_capacity(raw_fds.len());
    for &fd in raw_fds {
        if fd < 0 {
            return false;
        }
        pending.push(libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
    }
    if pending.is_empty() {
        return true;
    }

    let deadline = Instant::now() + timeout;
    while !pending.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
        let rc = unsafe {
            libc::poll(
                pending.as_mut_ptr(),
                pending.len() as libc::nfds_t,
                timeout_ms,
            )
        };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            log::warn!("sync: poll failed while waiting on completion fences: {err}");
            return false;
        }
        if rc == 0 {
            log::warn!(
                "sync: timed out waiting on {} completion fence(s) after {} ms",
                pending.len(),
                timeout.as_millis()
            );
            return false;
        }
        let mut errored = false;
        pending.retain(|pfd| {
            if (pfd.revents & libc::POLLNVAL) != 0 {
                errored = true;
                return false;
            }
            (pfd.revents & libc::POLLIN) == 0
        });
        if errored {
            log::warn!("sync: encountered invalid fence descriptor during poll wait");
            return false;
        }
        for pfd in &mut pending {
            pfd.revents = 0;
        }
    }
    true
}

fn sync_ioc_merge(fd1: RawFd, fd2: RawFd) -> std::io::Result<OwnedFd> {
    let mut data = SyncMergeData {
        name: MERGE_FENCE_NAME,
        fd2,
        fence: -1,
        flags: 0,
        pad: 0,
    };
    let result = unsafe { libc::ioctl(fd1, SYNC_IOC_MERGE, &mut data as *mut SyncMergeData) };
    if result < 0 || data.fence < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful SYNC_IOC_MERGE returns a fresh owned sync_file fd.
    Ok(unsafe { OwnedFd::from_raw_fd(data.fence) })
}

fn merge_sync_fences_with<F>(fences: Vec<OwnedFd>, mut merge_fd: F) -> CompletionFence
where
    F: FnMut(RawFd, RawFd) -> std::io::Result<OwnedFd>,
{
    let mut iter = fences.into_iter();
    let Some(mut merged) = iter.next() else {
        return CompletionFence::Signaled;
    };
    while let Some(fence) = iter.next() {
        match merge_fd(merged.as_raw_fd(), fence.as_raw_fd()) {
            Ok(next_merged) => {
                merged = next_merged;
            }
            Err(error) => {
                log::warn!("sync: failed to merge completion fences: {error}");
                let mut unmerged = Vec::with_capacity(2 + iter.size_hint().0);
                unmerged.push(merged);
                unmerged.push(fence);
                unmerged.extend(iter);
                return CompletionFence::Unmerged(unmerged).prune_signaled();
            }
        }
    }
    CompletionFence::Live(merged)
}

/// Merge multiple Linux `sync_file` descriptors into one combined fence that
/// signals once every input fence has signaled.
///
/// If `SYNC_IOC_MERGE` fails while live fences remain, returns
/// [`CompletionFence::Unmerged`] containing all remaining unsignaled fences so
/// callers fail safe instead of releasing client buffers prematurely.
pub fn merge_sync_fences(fences: Vec<OwnedFd>) -> CompletionFence {
    merge_sync_fences_with(fences, sync_ioc_merge)
}

/// Drop already-signaled fences and merge only the remaining live fences.
///
/// Avoiding `SYNC_IOC_MERGE` for fences that already signaled keeps steady-state
/// composited frames free of redundant kernel merge ioctls and fd churn.
pub fn merge_live_sync_fences(fences: Vec<OwnedFd>) -> CompletionFence {
    let live: Vec<OwnedFd> = fences
        .into_iter()
        .filter(|fence| !sync_file_is_signaled(fence.as_raw_fd()))
        .collect();
    merge_sync_fences(live)
}

/// Flatten and merge multiple [`CompletionFence`] results into one.
pub fn merge_completions(
    completions: impl IntoIterator<Item = CompletionFence>,
) -> CompletionFence {
    let mut all = Vec::new();
    for completion in completions {
        completion.append_to(&mut all);
    }
    merge_live_sync_fences(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_eventfd(initval: u32) -> OwnedFd {
        let raw = unsafe { libc::eventfd(initval, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        assert!(raw >= 0, "eventfd creation must succeed");
        unsafe { OwnedFd::from_raw_fd(raw) }
    }

    fn signal_eventfd(fd: RawFd) {
        let value: u64 = 1;
        let written = unsafe {
            libc::write(
                fd,
                (&value as *const u64).cast::<libc::c_void>(),
                std::mem::size_of::<u64>(),
            )
        };
        assert_eq!(written as usize, std::mem::size_of::<u64>());
    }

    #[test]
    fn sync_merge_data_abi_and_label_are_valid() {
        assert_eq!(std::mem::size_of::<SyncMergeData>(), 48);
        assert_eq!(MERGE_FENCE_NAME.len(), 32);
        assert_eq!(&MERGE_FENCE_NAME[..11], b"tessera-kms");
        assert!(MERGE_FENCE_NAME[11..].iter().all(|&b| b == 0));
    }

    #[test]
    fn negative_fd_is_not_signaled() {
        assert!(!sync_file_is_signaled(-1));
    }

    #[test]
    fn pollable_eventfd_reports_unsignaled_then_signaled() {
        let fd = new_eventfd(0);
        assert!(
            !sync_file_is_signaled(fd.as_raw_fd()),
            "unread eventfd with counter 0 must report unsignaled"
        );
        signal_eventfd(fd.as_raw_fd());
        assert!(
            sync_file_is_signaled(fd.as_raw_fd()),
            "signaled eventfd must report signaled"
        );
    }

    #[test]
    fn merge_live_sync_fences_drops_signaled_entries() {
        let signaled = new_eventfd(1);
        assert!(matches!(
            merge_live_sync_fences(vec![signaled]),
            CompletionFence::Signaled
        ));
    }

    #[test]
    fn merge_sync_fences_single_live_skips_ioctl() {
        let live = new_eventfd(0);
        let raw = live.as_raw_fd();
        let merged = merge_sync_fences_with(vec![live], |_, _| {
            panic!("single fence must not invoke merge ioctl");
        });
        match merged {
            CompletionFence::Live(fd) => assert_eq!(fd.as_raw_fd(), raw),
            other => panic!("expected Live(fd), got {other:?}"),
        }
    }

    #[test]
    fn merge_sync_fences_chains_multiple_fences_in_order() {
        let f1 = new_eventfd(0);
        let f2 = new_eventfd(0);
        let f3 = new_eventfd(0);
        let mut pairs = Vec::new();
        let merged = merge_sync_fences_with(vec![f1, f2, f3], |a, b| {
            pairs.push((a, b));
            Ok(new_eventfd(0))
        });
        assert_eq!(pairs.len(), 2, "three fences require two pairwise merges");
        assert!(matches!(merged, CompletionFence::Live(_)));
    }

    #[test]
    fn test_inv_arch_159_merge_failure_preserves_all_remaining_live_fences() {
        let f1 = new_eventfd(0);
        let f2 = new_eventfd(0);
        let f3 = new_eventfd(0);
        let mut calls = 0usize;
        let result = merge_sync_fences_with(vec![f1, f2, f3], |_, _| {
            calls += 1;
            if calls == 1 {
                Ok(new_eventfd(0))
            } else {
                Err(std::io::Error::from_raw_os_error(libc::EMFILE))
            }
        });
        match result {
            CompletionFence::Unmerged(fds) => {
                assert_eq!(
                    fds.len(),
                    2,
                    "both the intermediate merged fence and the unmerged third fence must survive"
                );
            }
            other => panic!("merge failure must fail safe as Unmerged, got {other:?}"),
        }
    }

    #[test]
    fn test_inv_arch_159_real_sync_ioc_merge_two_fences_never_panics_and_fails_safe() {
        // Two unsignaled non-sync_file fds force the real SYNC_IOC_MERGE ioctl
        // loop to execute: it must never panic on label setup and must preserve
        // both live fds as Unmerged rather than returning Signaled.
        let f1 = new_eventfd(0);
        let f2 = new_eventfd(0);
        let result = merge_live_sync_fences(vec![f1, f2]);
        match result {
            CompletionFence::Unmerged(fds) => assert_eq!(fds.len(), 2),
            other => panic!("expected fail-safe Unmerged(2), got {other:?}"),
        }
    }

    #[test]
    fn wait_all_succeeds_once_all_unmerged_fences_signal() {
        let f1 = new_eventfd(0);
        let f2 = new_eventfd(0);
        let f1_dup = f1.try_clone().expect("dup f1");
        let f2_dup = f2.try_clone().expect("dup f2");
        let completion = CompletionFence::Unmerged(vec![f1, f2]);

        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(15));
            signal_eventfd(f1_dup.as_raw_fd());
            std::thread::sleep(Duration::from_millis(15));
            signal_eventfd(f2_dup.as_raw_fd());
        });

        assert!(completion.wait_all(Duration::from_millis(500)));
        assert!(completion.is_signaled());
        handle.join().expect("signaler thread");
    }

    #[test]
    fn wait_all_returns_false_on_timeout() {
        let f1 = new_eventfd(0);
        let completion = CompletionFence::Live(f1);
        assert!(!completion.wait_all(Duration::from_millis(10)));
    }

    #[test]
    fn test_inv_arch_159_real_kernel_sync_ioc_merge_succeeds_on_drm_sync_files() {
        use std::fs::OpenOptions;

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
        const DRM_SYNCOBJ_CREATE_SIGNALED: u32 = 1 << 0;
        const DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE: u32 = 1 << 0;
        const DRM_IOCTL_SYNCOBJ_CREATE: libc::c_ulong = 0xC008_64BF;
        const DRM_IOCTL_SYNCOBJ_DESTROY: libc::c_ulong = 0xC008_64C0;
        const DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD: libc::c_ulong = 0xC010_64C1;

        let Ok(render_dir) = std::fs::read_dir("/dev/dri") else {
            return;
        };
        let Some(drm) = render_dir
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("renderD"))
            })
            .find_map(|path| OpenOptions::new().read(true).write(true).open(path).ok())
        else {
            return;
        };

        let export_real_sync_file = |drm_fd: RawFd| -> Option<OwnedFd> {
            let mut create = DrmSyncobjCreate {
                handle: 0,
                flags: DRM_SYNCOBJ_CREATE_SIGNALED,
            };
            if unsafe { libc::ioctl(drm_fd, DRM_IOCTL_SYNCOBJ_CREATE, &mut create) } < 0 {
                return None;
            }
            let mut export = DrmSyncobjHandleV1 {
                handle: create.handle,
                flags: DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE,
                fd: -1,
                pad: 0,
            };
            let rc = unsafe { libc::ioctl(drm_fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &mut export) };
            let mut destroy = DrmSyncobjDestroy {
                handle: create.handle,
                pad: 0,
            };
            let _ = unsafe { libc::ioctl(drm_fd, DRM_IOCTL_SYNCOBJ_DESTROY, &mut destroy) };
            if rc < 0 || export.fd < 0 {
                return None;
            }
            Some(unsafe { OwnedFd::from_raw_fd(export.fd) })
        };

        let Some(sf1) = export_real_sync_file(drm.as_raw_fd()) else {
            return;
        };
        let Some(sf2) = export_real_sync_file(drm.as_raw_fd()) else {
            return;
        };

        // Call `merge_sync_fences` directly (bypassing `merge_live_sync_fences`
        // pre-filtering) so the real kernel `SYNC_IOC_MERGE` ioctl executes and
        // proves ABI layout, ioctl number, and NUL-terminated label end-to-end.
        match merge_sync_fences(vec![sf1, sf2]) {
            CompletionFence::Live(merged) => {
                assert!(
                    sync_file_is_signaled(merged.as_raw_fd()),
                    "merged fence of two signaled sync_files must report signaled"
                );
            }
            other => panic!("real kernel SYNC_IOC_MERGE must return Live(merged), got {other:?}"),
        }
    }
}
