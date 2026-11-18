//! Process signal helpers for graceful shutdown (`SIGINT`/`SIGTERM`).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

static ACTIVE_FLAG: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());

extern "C" fn handle_signal(_sig: libc::c_int) {
    let ptr = ACTIVE_FLAG.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: `ptr` comes from `Arc::into_raw` before `sigaction` and is
        // only reclaimed after a newer pointer is swapped in.
        unsafe { (*ptr).store(true, Ordering::Release) };
    }
}

fn register_signal(sig: libc::c_int) -> io::Result<()> {
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = handle_signal as *const () as usize;
    sa.sa_flags = libc::SA_RESTART;
    unsafe { libc::sigemptyset(&mut sa.sa_mask) };
    if unsafe { libc::sigaction(sig, &sa, std::ptr::null_mut()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Install `SIGINT` and `SIGTERM` handlers that set the returned flag to `true`.
pub fn install_shutdown_flag() -> io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    install_shutdown_flag_into(&flag)?;
    Ok(flag)
}

/// Install `SIGINT` and `SIGTERM` handlers that set `flag` to `true`.
pub fn install_shutdown_flag_into(flag: &Arc<AtomicBool>) -> io::Result<()> {
    let raw = Arc::into_raw(Arc::clone(flag)).cast_mut();
    let prev = ACTIVE_FLAG.swap(raw, Ordering::AcqRel);
    if !prev.is_null() {
        // SAFETY: `prev` came from `Arc::into_raw`.
        unsafe { drop(Arc::from_raw(prev)) };
    }
    register_signal(libc::SIGINT)?;
    register_signal(libc::SIGTERM)?;
    Ok(())
}
