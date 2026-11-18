//! Minimal Wayland client event loop, wakeup channel, and xkbcommon keyboard helpers.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use wayland_client::{Connection, EventQueue};
use xkbcommon::xkb;

pub use xkbcommon::xkb::Keysym;

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// Pointer scroll delta for one axis.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AxisScroll {
    pub absolute: f64,
    pub discrete: i32,
}

/// Keyboard modifier state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub caps_lock: bool,
    pub logo: bool,
    pub num_lock: bool,
}

/// Decoded key event from `wl_keyboard` + `xkbcommon`.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    pub raw_code: u32,
    pub keysym: Keysym,
    pub utf8: Option<String>,
    pub time: u32,
}

/// Eventfd-backed wakeup signal for `poll`-based event loops.
#[derive(Clone)]
pub struct WakeSignal {
    fd: Arc<OwnedFd>,
}

impl WakeSignal {
    pub fn new() -> io::Result<Self> {
        let raw = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd: Arc::new(unsafe { OwnedFd::from_raw_fd(raw) }),
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn wake(&self) {
        let val: u64 = 1;
        unsafe {
            libc::write(
                self.fd.as_raw_fd(),
                (&raw const val).cast::<libc::c_void>(),
                std::mem::size_of::<u64>(),
            );
        }
    }

    pub fn drain(&self) {
        let mut val: u64 = 0;
        loop {
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    (&raw mut val).cast::<libc::c_void>(),
                    std::mem::size_of::<u64>(),
                )
            };
            if n <= 0 {
                break;
            }
        }
    }
}

/// MPSC sender that also wakes the Wayland `poll` loop via an `eventfd`.
pub struct WakeSender<T> {
    tx: Sender<T>,
    wake: WakeSignal,
}

impl<T> Clone for WakeSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            wake: self.wake.clone(),
        }
    }
}

impl<T> WakeSender<T> {
    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        self.tx.send(value)?;
        self.wake.wake();
        Ok(())
    }
}

/// MPSC receiver paired with [`WakeSender`].
pub struct WakeReceiver<T> {
    rx: Receiver<T>,
}

impl<T> WakeReceiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.rx.try_recv()
    }
}

/// Create a channel whose `send` wakes `wake`.
pub fn wake_channel<T>(wake: WakeSignal) -> (WakeSender<T>, WakeReceiver<T>) {
    let (tx, rx) = mpsc::channel();
    (WakeSender { tx, wake }, WakeReceiver { rx })
}

/// Create a cloexec pipe pair `(read_end, write_end)`.
pub fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Drive a `wayland_client::EventQueue` with `libc::poll` and an optional wakeup fd.
pub fn run_event_loop<D>(
    connection: &Connection,
    event_queue: &mut EventQueue<D>,
    state: &mut D,
    wake: Option<&WakeSignal>,
    mut on_tick: impl FnMut(&mut D),
    mut should_exit: impl FnMut(&D) -> bool,
    mut poll_timeout: impl FnMut(&D) -> Option<Duration>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        event_queue.dispatch_pending(state)?;
        on_tick(state);
        if should_exit(state) {
            break;
        }
        connection.flush()?;

        let timeout_ms = poll_timeout(state)
            .map(|d| d.as_millis().min(i32::MAX as u128) as libc::c_int)
            .unwrap_or(-1);

        if let Some(guard) = connection.prepare_read() {
            let wl_fd = guard.connection_fd().as_raw_fd();
            let mut pfds = [
                libc::pollfd {
                    fd: wl_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake.map_or(-1, WakeSignal::raw_fd),
                    events: if wake.is_some() { libc::POLLIN } else { 0 },
                    revents: 0,
                },
            ];
            let nfds = if wake.is_some() { 2 } else { 1 };
            let rc = unsafe { libc::poll(pfds.as_mut_ptr(), nfds, timeout_ms) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    drop(guard);
                    return Err(err.into());
                }
            }
            if (pfds[0].revents & libc::POLLIN) != 0 {
                let _ = guard.read();
            } else {
                drop(guard);
            }
            if let Some(w) = wake
                && (pfds[1].revents & libc::POLLIN) != 0
            {
                w.drain();
            }
        }
    }
    Ok(())
}

/// xkbcommon state machine for one `wl_keyboard`.
pub struct XkbKeyboard {
    context: xkb::Context,
    keymap: Option<xkb::Keymap>,
    state: Option<xkb::State>,
    pub modifiers: Modifiers,
    pub active_layout: u32,
    pub layout_names: Vec<String>,
}

impl Default for XkbKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl XkbKeyboard {
    pub fn new() -> Self {
        Self {
            context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            keymap: None,
            state: None,
            modifiers: Modifiers::default(),
            active_layout: 0,
            layout_names: Vec::new(),
        }
    }

    /// Load a keymap from `wl_keyboard::Event::Keymap`.
    pub fn load_keymap(&mut self, fd: BorrowedFd<'_>, size: u32) {
        let len = size as usize;
        if len == 0 {
            return;
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return;
        }
        let slice = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
        let text = match std::ffi::CStr::from_bytes_until_nul(slice) {
            Ok(cstr) => cstr.to_string_lossy().into_owned(),
            Err(_) => String::from_utf8_lossy(slice).into_owned(),
        };
        unsafe {
            libc::munmap(ptr, len);
        }

        if let Some(keymap) = xkb::Keymap::new_from_string(
            &self.context,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        ) {
            let num_layouts = keymap.num_layouts();
            self.layout_names = (0..num_layouts)
                .map(|i| keymap.layout_get_name(i).to_string())
                .collect();
            self.state = Some(xkb::State::new(&keymap));
            self.keymap = Some(keymap);
        }
    }

    /// Update modifier/group state from `wl_keyboard::Event::Modifiers`.
    pub fn update_modifiers(
        &mut self,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
    ) {
        if let Some(ref mut state) = self.state {
            state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
            self.modifiers = Modifiers {
                ctrl: state.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE),
                alt: state.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE),
                shift: state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE),
                caps_lock: state.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE),
                logo: state.mod_name_is_active(xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE),
                num_lock: state.mod_name_is_active(xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE),
            };
        }
        self.active_layout = group;
    }

    /// Translate a `wl_keyboard::Event::Key` evdev keycode (`key + 8` in XKB) into a [`KeyEvent`].
    pub fn key_event(&self, key: u32, time: u32) -> KeyEvent {
        let raw_code = key + 8;
        let (keysym, utf8) = if let Some(ref state) = self.state {
            let sym = state.key_get_one_sym(raw_code.into());
            let s = state.key_get_utf8(raw_code.into());
            let utf8 = if s.is_empty() { None } else { Some(s) };
            (sym, utf8)
        } else {
            (Keysym::NoSymbol, None)
        };
        KeyEvent {
            raw_code,
            keysym,
            utf8,
            time,
        }
    }
}
