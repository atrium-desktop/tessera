//! Linux `sync_file` fence helpers shared by DRM presentation and runtime
//! buffer-retirement scheduling (ADR-0250).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

#[repr(C)]
struct SyncMergeData {
    name: [u8; 32],
    fd2: i32,
    fence: i32,
    flags: u32,
    pad: u32,
}

/// `SYNC_IOC_MERGE` from `<linux/sync_file.h>`. The UAPI encoding and
/// `sync_merge_data` layout are stable across Linux architectures supported by
/// Tessera.
const SYNC_IOC_MERGE: libc::c_ulong = 0xC030_3E03;

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

/// Merge multiple Linux `sync_file` descriptors into one combined fence that
/// signals once every input fence has signaled.
///
/// Returns `None` when `fences` is empty or the kernel rejects `SYNC_IOC_MERGE`.
pub fn merge_sync_fences(mut fences: Vec<OwnedFd>) -> Option<OwnedFd> {
    let mut merged = fences.pop()?;
    for fence in fences {
        let mut data = SyncMergeData {
            name: [0; 32],
            fd2: fence.as_raw_fd(),
            fence: -1,
            flags: 0,
            pad: 0,
        };
        data.name[..10].copy_from_slice(b"tessera-kms");
        let result = unsafe {
            libc::ioctl(
                merged.as_raw_fd(),
                SYNC_IOC_MERGE,
                &mut data as *mut SyncMergeData,
            )
        };
        if result < 0 || data.fence < 0 {
            log::warn!(
                "sync: failed to merge completion fences: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        // SAFETY: a successful SYNC_IOC_MERGE returns a new owned sync_file.
        merged = unsafe { OwnedFd::from_raw_fd(data.fence) };
    }
    Some(merged)
}

/// Drop already-signaled fences and merge only the remaining live fences.
///
/// Avoiding `SYNC_IOC_MERGE` for fences that already signaled keeps steady-state
/// composited frames free of redundant kernel merge ioctls and fd churn.
pub fn merge_live_sync_fences(fences: Vec<OwnedFd>) -> Option<OwnedFd> {
    let live: Vec<OwnedFd> = fences
        .into_iter()
        .filter(|fence| !sync_file_is_signaled(fence.as_raw_fd()))
        .collect();
    merge_sync_fences(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_fd_is_not_signaled() {
        assert!(!sync_file_is_signaled(-1));
    }

    #[test]
    fn pollable_eventfd_reports_unsignaled_then_signaled() {
        let raw = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        assert!(raw >= 0, "eventfd creation must succeed");
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        assert!(
            !sync_file_is_signaled(fd.as_raw_fd()),
            "unread eventfd with counter 0 must report unsignaled"
        );
        let value: u64 = 1;
        let written = unsafe {
            libc::write(
                fd.as_raw_fd(),
                (&value as *const u64).cast::<libc::c_void>(),
                std::mem::size_of::<u64>(),
            )
        };
        assert_eq!(written as usize, std::mem::size_of::<u64>());
        assert!(
            sync_file_is_signaled(fd.as_raw_fd()),
            "signaled eventfd must report signaled"
        );
    }

    #[test]
    fn merge_live_sync_fences_drops_signaled_entries() {
        let raw = unsafe { libc::eventfd(1, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        assert!(raw >= 0);
        let signaled = unsafe { OwnedFd::from_raw_fd(raw) };
        assert!(merge_live_sync_fences(vec![signaled]).is_none());
    }
}
