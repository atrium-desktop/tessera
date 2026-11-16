//! Process-environment isolation for tests.
//!
//! `std::env::set_var` and `remove_var` are `unsafe` in edition 2024 because
//! the environment is process-global. The workspace runs under nextest, which
//! gives each test its own process, but the guards here still serialize access
//! so a test also behaves correctly under a thread-per-test runner or when it
//! starts helper threads.

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

/// Serializes every environment mutation in the current test process.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the process-wide environment lock.
///
/// A panicking test can poison the mutex. Its guard has already restored the
/// variable, so recovering the lock is correct and keeps later tests running
/// instead of cascading into spurious failures.
pub fn lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A restoring environment mutation. The variable is restored when the guard
/// drops, while the environment lock is still held.
pub struct EnvGuard {
    key: String,
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Set `key` to `value` for the guard's lifetime.
    pub fn set(key: impl Into<String>, value: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.into();
        let previous = std::env::var_os(&key);
        // SAFETY: `lock` is held for the whole mutation and the matching
        // restore in `Drop`, so no racing `std::env` access is started here.
        unsafe { std::env::set_var(&key, value) };
        Self {
            key,
            previous,
            _lock: lock,
        }
    }

    /// Remove `key` for the guard's lifetime.
    pub fn unset(key: impl Into<String>) -> Self {
        let lock = lock();
        let key = key.into();
        let previous = std::env::var_os(&key);
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(&key) };
        Self {
            key,
            previous,
            _lock: lock,
        }
    }

    /// The variable name controlled by this guard.
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: `_lock` is still held; fields drop after this method.
            Some(value) => unsafe { std::env::set_var(&self.key, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

/// Run `f` with `key` set to `value`, restoring the previous value afterwards.
pub fn with_var<T>(key: impl Into<String>, value: impl AsRef<OsStr>, f: impl FnOnce() -> T) -> T {
    let _guard = EnvGuard::set(key, value);
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_restores_previous_value() {
        let key = "TESSERA_TEST_SUPPORT_ENV_RESTORE";
        let previous = std::env::var_os(key);
        {
            let _guard = EnvGuard::set(key, "inside");
            assert_eq!(std::env::var(key).as_deref(), Ok("inside"));
        }
        assert_eq!(std::env::var_os(key), previous);
    }

    #[test]
    fn guard_restores_unset_state() {
        let key = "TESSERA_TEST_SUPPORT_ENV_UNSET";
        unsafe { std::env::remove_var(key) };
        {
            let _guard = EnvGuard::unset(key);
            assert!(std::env::var_os(key).is_none());
        }
        assert!(std::env::var_os(key).is_none());
    }
}
