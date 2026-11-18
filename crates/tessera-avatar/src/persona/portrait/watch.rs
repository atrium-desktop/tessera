//! Debounced observation for the shared portrait configuration.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{PortraitCandidate, PortraitConfig};

const DEBOUNCE: Duration = Duration::from_millis(180);
const RETRY_DELAY: Duration = Duration::from_millis(350);
const MAX_RETRIES: u8 = 3;

const WATCH_MASK: u32 = libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ATTRIB;

#[derive(Debug)]
pub enum WatchError {
    Create(io::Error),
    NoTargets,
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create(_) => f.write_str("create persona portrait filesystem watcher"),
            Self::NoTargets => f.write_str("no persona portrait source directory could be watched"),
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Create(err) => Some(err),
            Self::NoTargets => None,
        }
    }
}

#[derive(Clone, Debug)]
struct RelevantPaths {
    trees: Vec<PathBuf>,
    exact: Vec<PathBuf>,
}

impl RelevantPaths {
    fn from_config(config: &PortraitConfig) -> Self {
        let mut trees = Vec::new();
        let mut exact = Vec::new();
        for candidate in config.candidates() {
            match candidate {
                PortraitCandidate::Still(path) => exact.push(path.clone()),
                PortraitCandidate::Vrm { model, .. } => {
                    if let Some(parent) = model.parent() {
                        trees.push(parent.to_path_buf());
                    }
                }
            }
        }
        trees.sort();
        trees.dedup();
        exact.retain(|path| !trees.iter().any(|tree| path.starts_with(tree)));
        exact.sort();
        exact.dedup();
        Self { trees, exact }
    }

    fn includes(&self, path: &Path) -> bool {
        self.trees
            .iter()
            .any(|tree| path.starts_with(tree) || tree.starts_with(path))
            || self
                .exact
                .iter()
                .any(|exact| path == exact || exact.starts_with(path))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchDepth {
    Direct,
    Recursive,
}

#[derive(Debug, Default)]
struct ReloadSchedule {
    deadline: Option<Instant>,
    retries_remaining: u8,
}

impl ReloadSchedule {
    fn observe(&mut self, now: Instant) {
        self.deadline = Some(now + DEBOUNCE);
        self.retries_remaining = MAX_RETRIES;
    }

    fn take_ready(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| deadline <= now) {
            self.deadline = None;
            true
        } else {
            false
        }
    }

    fn retry(&mut self, now: Instant) -> bool {
        if self.retries_remaining == 0 {
            return false;
        }
        self.retries_remaining -= 1;
        self.deadline = Some(now + RETRY_DELAY);
        true
    }
}

struct InotifyWatcher {
    fd: OwnedFd,
    watches: HashMap<i32, (PathBuf, WatchDepth)>,
}

impl InotifyWatcher {
    fn new() -> io::Result<Self> {
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self {
            fd,
            watches: HashMap::new(),
        })
    }

    fn clear_watches(&mut self) {
        for wd in self.watches.drain().map(|(wd, _)| wd) {
            unsafe {
                libc::inotify_rm_watch(self.fd.as_raw_fd(), wd);
            }
        }
    }

    fn add_watch(&mut self, path: &Path, depth: WatchDepth) -> io::Result<()> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let wd =
            unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c_path.as_ptr(), WATCH_MASK) };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        self.watches.insert(wd, (path.to_path_buf(), depth));
        if depth == WatchDepth::Recursive
            && let Ok(entries) = std::fs::read_dir(path)
        {
            for entry in entries.flatten() {
                let sub = entry.path();
                if sub.is_dir() {
                    let _ = self.add_watch(&sub, WatchDepth::Recursive);
                }
            }
        }
        Ok(())
    }

    fn has_pending_events(&self) -> bool {
        let mut pfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        rc > 0 && (pfd.revents & libc::POLLIN) != 0
    }

    fn drain_relevant(&mut self, relevant: &RelevantPaths) -> bool {
        let mut buf = [0u8; 4096];
        let mut matched = false;
        let mut new_dirs = Vec::new();
        let header_size = std::mem::size_of::<libc::inotify_event>();

        loop {
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buf.as_mut_ptr().cast::<libc::c_void>(),
                    buf.len(),
                )
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                log::warn!("persona portrait: inotify read error: {err}");
                matched = true;
                break;
            }
            if n == 0 {
                break;
            }

            let bytes = &buf[..n as usize];
            let mut offset = 0usize;
            while offset + header_size <= bytes.len() {
                let event = unsafe {
                    std::ptr::read_unaligned(bytes[offset..].as_ptr().cast::<libc::inotify_event>())
                };
                let name_len = event.len as usize;
                let next_offset = offset + header_size + name_len;
                if next_offset > bytes.len() {
                    break;
                }

                if (event.mask & libc::IN_Q_OVERFLOW) != 0 {
                    matched = true;
                } else if let Some((dir, depth)) = self.watches.get(&event.wd) {
                    let full_path = if name_len > 0 {
                        let name_bytes = &bytes[offset + header_size..next_offset];
                        let name = CStr::from_bytes_until_nul(name_bytes)
                            .map(|c| Path::new(std::ffi::OsStr::from_bytes(c.to_bytes())))
                            .unwrap_or_else(|_| Path::new(""));
                        dir.join(name)
                    } else {
                        dir.clone()
                    };

                    if *depth == WatchDepth::Recursive
                        && (event.mask & libc::IN_ISDIR) != 0
                        && (event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO)) != 0
                    {
                        new_dirs.push(full_path.clone());
                    }

                    if relevant.includes(&full_path) {
                        matched = true;
                    }
                }
                offset = next_offset;
            }
        }

        for dir in new_dirs {
            let _ = self.add_watch(&dir, WatchDepth::Recursive);
        }

        matched
    }
}

/// Watches every source described by one immutable [`PortraitConfig`].
pub struct PortraitWatcher {
    watcher: InotifyWatcher,
    relevant: RelevantPaths,
    schedule: ReloadSchedule,
}

impl PortraitWatcher {
    pub fn new(config: &PortraitConfig) -> Result<Self, WatchError> {
        Self::from_relevant(RelevantPaths::from_config(config))
    }

    fn from_relevant(relevant: RelevantPaths) -> Result<Self, WatchError> {
        let watcher = InotifyWatcher::new().map_err(WatchError::Create)?;
        let mut this = Self {
            watcher,
            relevant,
            schedule: ReloadSchedule::default(),
        };
        this.refresh()?;
        Ok(this)
    }

    pub fn refresh(&mut self) -> Result<(), WatchError> {
        let desired = watch_targets(&self.relevant);
        self.watcher.clear_watches();
        for (path, depth) in desired {
            if let Err(error) = self.watcher.add_watch(&path, depth) {
                log::warn!("persona portrait: could not watch {path:?}: {error}");
            }
        }
        if self.watcher.watches.is_empty() {
            Err(WatchError::NoTargets)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn needs_poll(&self) -> bool {
        self.schedule.deadline.is_some() || self.watcher.has_pending_events()
    }

    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        if self.watcher.drain_relevant(&self.relevant) {
            self.schedule.observe(now);
        }
        self.schedule.take_ready(now)
    }

    pub fn retry(&mut self) -> bool {
        self.schedule.retry(Instant::now())
    }
}

fn watch_targets(relevant: &RelevantPaths) -> HashMap<PathBuf, WatchDepth> {
    let mut targets = HashMap::new();
    for tree in &relevant.trees {
        if tree.is_dir() {
            insert_target(&mut targets, tree.clone(), WatchDepth::Recursive);
            if let Some(parent) = tree.parent().filter(|parent| parent.is_dir()) {
                insert_target(&mut targets, parent.to_path_buf(), WatchDepth::Direct);
            }
        } else if let Some(existing) = nearest_existing_directory(tree) {
            insert_target(&mut targets, existing, WatchDepth::Direct);
        }
    }
    for path in &relevant.exact {
        if let Some(parent) = path.parent().and_then(nearest_existing_directory) {
            insert_target(&mut targets, parent, WatchDepth::Direct);
        }
    }
    targets
}

fn insert_target(targets: &mut HashMap<PathBuf, WatchDepth>, path: PathBuf, depth: WatchDepth) {
    targets
        .entry(path)
        .and_modify(|current| {
            if depth == WatchDepth::Recursive {
                *current = depth;
            }
        })
        .or_insert(depth);
}

fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|path| path.is_dir())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_config_drives_still_and_motion_observation() {
        let relevant = RelevantPaths::from_config(&PortraitConfig::new(vec![
            PortraitCandidate::Still(PathBuf::from("/home/test/.face")),
            PortraitCandidate::Vrm {
                model: PathBuf::from("/data/tessera/avatars/avatar.vrm"),
                legacy_motion: PathBuf::from("/data/tessera/avatars/avatar.vrma"),
            },
        ]));
        assert!(relevant.includes(Path::new("/home/test/.face")));
        assert!(relevant.includes(Path::new(
            "/data/tessera/avatars/motions/actions/greeting.vrma"
        )));
        assert!(!relevant.includes(Path::new("/home/test/Downloads/photo.png")));
    }

    #[test]
    fn retry_schedule_is_bounded() {
        let start = Instant::now();
        let mut schedule = ReloadSchedule::default();
        schedule.observe(start);
        assert!(!schedule.take_ready(start + DEBOUNCE - Duration::from_millis(1)));
        assert!(schedule.take_ready(start + DEBOUNCE));
        for retry in 0..MAX_RETRIES {
            assert!(schedule.retry(start + Duration::from_secs(u64::from(retry))));
        }
        assert!(!schedule.retry(start + Duration::from_secs(10)));
    }
}
