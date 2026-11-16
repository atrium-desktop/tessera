//! Bounded waits for asynchronous tests.

use std::path::Path;
use std::time::{Duration, Instant};

/// Poll `condition` until it returns `true` or `timeout` elapses.
///
/// The condition runs at least once. Returns the final condition result.
pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait until `path` exists.
pub fn wait_for_path(path: impl AsRef<Path>, timeout: Duration) -> bool {
    let path = path.as_ref();
    wait_until(timeout, || path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_runs_even_when_timeout_is_zero() {
        let mut ran = false;
        assert!(!wait_until(Duration::ZERO, || {
            ran = true;
            false
        }));
        assert!(ran);
    }
}
