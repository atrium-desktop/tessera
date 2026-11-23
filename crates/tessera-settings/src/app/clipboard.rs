//! Wayland clipboard integration for optics-ui text fields.
//!
//! optics-ui owns text editing and reports copy/paste intent through its
//! [`optics_ui_clipboard`](optics_ui::sys::optics_ui_clipboard) host callbacks. This module
//! turns those callbacks into one calloop event each, and implements the
//! Wayland side against the seat's `wl_data_device`:
//!
//! - **copy** creates a `wl_data_source`, offers UTF-8 text MIME types, and
//!   answers every `send` with the selected text;
//! - **paste** reads the current `wl_data_offer` on a worker thread with a
//!   bounded timeout, then delivers the payload to optics-ui through `Ui::paste`.
//!
//! The clipboard is explicit-only ([ADR-0043]): no primary selection is
//! advertised or consumed. Both directions use the standard
//! `wl_data_device` family, so the compositor's clipboard manager sees the
//! same selection a Ctrl+C or Ctrl+V in any other client would.
//!
//! [ADR-0043]: ../../../docs/adr/0043-explicit-clipboard-only.md

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::raw::c_char;
use std::slice;
use std::thread;
use std::time::Duration;

use tessera_wl_client::WakeSender as LoopSender;

/// Largest clipboard payload accepted, matching optics-ui's staging buffer. Text
/// fields cannot use more, and an unbounded peer must not grow our heap.
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
/// A clipboard owner that neither sends nor closes its pipe must not pin a
/// worker thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// MIME types a text field can consume, most specific first. `UTF8_STRING`
/// covers X11-style clients; the charset parameter is what GTK and Qt emit.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// Requests optics-ui raises on the host side of the clipboard interface.
#[derive(Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// Ctrl+V: read the current selection.
    Paste,
    /// Ctrl+C or Ctrl+X: own the selection with this payload.
    Copy(String),
}

/// A clipboard read finished (or failed) on a worker thread.
#[derive(Debug)]
pub struct ReadFinished(pub Option<String>);

/// Owns the calloop sender behind optics-ui's C callbacks. The boxed value has a
/// stable address for the lifetime of the window, so the raw user pointer in
/// `optics_ui_desc` stays valid across surface reconfiguration.
pub struct Host {
    events: LoopSender<HostEvent>,
}

impl Host {
    pub fn new(events: LoopSender<HostEvent>) -> Box<Self> {
        Box::new(Self { events })
    }

    /// Stable pointer for `optics_ui_desc.clipboard.user`.
    pub fn as_user_data(&self) -> *mut c_void {
        (self as *const Self).cast_mut().cast()
    }

    fn send(&self, event: HostEvent) {
        let _ = self.events.send(event);
    }
}

/// `optics_ui_clipboard.request_text`: the focused text widget wants a paste.
///
/// # Safety
/// `user` must be the pointer from [`Host::as_user_data`] and remain valid
/// for the call.
pub(crate) unsafe extern "C" fn request_text(user: *mut c_void) {
    // SAFETY: optics-ui echoes the pointer we gave it; it stays owned by `App`.
    let Some(host) = (unsafe { (user as *const Host).as_ref() }) else {
        return;
    };
    host.send(HostEvent::Paste);
}

/// `optics_ui_clipboard.set_text`: a text widget copied or cut a selection.
///
/// # Safety
/// `utf8` must point to `len` readable bytes; `user` must be the pointer from
/// [`Host::as_user_data`]. optics-ui satisfies both for the duration of the call.
pub(crate) unsafe extern "C" fn set_text(utf8: *const c_char, len: usize, user: *mut c_void) {
    if utf8.is_null() || len == 0 {
        return;
    }
    // SAFETY: the callback contract guarantees `len` bytes at `utf8`.
    let bytes = unsafe { slice::from_raw_parts(utf8.cast::<u8>(), len) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        log::warn!("settings: clipboard copy supplied non-UTF-8 text");
        return;
    };
    // SAFETY: see `request_text`.
    let Some(host) = (unsafe { (user as *const Host).as_ref() }) else {
        return;
    };
    let mut end = text.len().min(MAX_CLIPBOARD_BYTES);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    host.send(HostEvent::Copy(text[..end].to_owned()));
}

/// Pick the best text MIME type from an offer. The comparison is
/// case-insensitive and ignores parameters on the generic `text/plain` entry.
pub(crate) fn pick_text_mime(mime_types: &[String]) -> Option<String> {
    for wanted in TEXT_MIME_TYPES {
        if let Some(found) = mime_types
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(wanted))
        {
            return Some(found.clone());
        }
    }
    mime_types
        .iter()
        .find(|candidate| {
            let mut parts = candidate.split(';');
            let base_is_plain = parts
                .next()
                .is_some_and(|base| base.trim().eq_ignore_ascii_case("text/plain"));
            let charset = parts.find_map(|parameter| {
                let (name, value) = parameter.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("charset")
                    .then_some(value.trim())
            });
            base_is_plain && charset.is_none_or(|value| value.eq_ignore_ascii_case("utf-8"))
        })
        .cloned()
}

/// Read an offer's pipe on a worker thread and wake the window with the
/// UTF-8 payload. The thread is detached; the window may close first.
pub(crate) fn spawn_read(pipe: OwnedFd, done: LoopSender<ReadFinished>) {
    let reader = File::from(pipe);
    let spawned = thread::Builder::new()
        .name("settings-clipboard-read".into())
        .spawn(move || {
            let text = read_bounded(reader).and_then(|bytes| String::from_utf8(bytes).ok());
            let _ = done.send(ReadFinished(text));
        });
    if let Err(error) = spawned {
        log::warn!("settings: could not start clipboard reader: {error}");
    }
}

fn read_bounded(mut reader: File) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if !wait_readable(&reader) {
            log::debug!("settings: clipboard transfer timed out");
            return None;
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Some(bytes),
            Ok(read) => {
                if bytes.len() + read > MAX_CLIPBOARD_BYTES {
                    log::warn!("settings: clipboard payload exceeds {MAX_CLIPBOARD_BYTES} bytes");
                    return None;
                }
                bytes.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                log::debug!("settings: clipboard read failed: {error}");
                return None;
            }
        }
    }
}

fn wait_readable(reader: &File) -> bool {
    let mut poll_fd = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd is supplied and the timeout is a plain int.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, READ_TIMEOUT.as_millis() as i32) };
    if ready > 0 {
        return poll_fd.revents & (libc::POLLIN | libc::POLLHUP) != 0;
    }
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return wait_readable(reader);
        }
        log::debug!("settings: clipboard poll failed: {error}");
    }
    false
}

/// Build a pipe for the clipboard reader tests.
#[cfg(test)]
fn receive_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` points at two writable ints.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 returned two fresh descriptors owned by this function.
    Ok(unsafe {
        (
            <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fds[0]),
            <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fds[1]),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn ui_callbacks_become_loop_events() {
        let wake = tessera_wl_client::WakeSignal::new().unwrap();
        let (events, received) = tessera_wl_client::wake_channel(wake);
        let host = Host::new(events);
        let user = host.as_user_data();
        // SAFETY: `host` owns the user pointer for both calls.
        unsafe {
            request_text(user);
            set_text(c"clipboard text".as_ptr(), 14, user);
        }
        assert_eq!(received.try_recv().unwrap(), HostEvent::Paste);
        assert_eq!(
            received.try_recv().unwrap(),
            HostEvent::Copy("clipboard text".into())
        );
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn mime_pick_prefers_utf8_then_plain_text() {
        let offered = vec!["image/png".to_owned(), "text/plain".to_owned()];
        assert_eq!(pick_text_mime(&offered).as_deref(), Some("text/plain"));
        let utf8 = vec![
            "text/plain".to_owned(),
            "text/plain;charset=UTF-8".to_owned(),
        ];
        assert_eq!(
            pick_text_mime(&utf8).as_deref(),
            Some("text/plain;charset=UTF-8")
        );
        assert_eq!(pick_text_mime(&["image/png".to_owned()]), None);
        assert_eq!(
            pick_text_mime(&["text/plain;charset=iso-8859-1".to_owned()]),
            None,
            "a non-UTF-8 charset would corrupt the text field"
        );
    }

    #[test]
    fn a_selection_read_round_trips_utf8() {
        let (read_fd, write_fd) = receive_pipe().unwrap();
        let mut writer = File::from(write_fd);
        writer.write_all("hello, Tessera".as_bytes()).unwrap();
        drop(writer);
        let bytes = read_bounded(File::from(read_fd)).unwrap();
        assert_eq!(bytes, b"hello, Tessera");
    }

    #[test]
    fn an_oversized_selection_is_refused() {
        let (read_fd, write_fd) = receive_pipe().unwrap();
        let writer = thread::spawn(move || {
            let mut writer = File::from(write_fd);
            // A short write means the reader already stopped; either way the
            // read side must refuse the payload.
            let _ = writer.write_all(&vec![b'x'; MAX_CLIPBOARD_BYTES + 1]);
        });
        assert!(read_bounded(File::from(read_fd)).is_none());
        writer.join().unwrap();
    }
}
