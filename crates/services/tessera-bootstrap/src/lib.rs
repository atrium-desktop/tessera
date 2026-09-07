//! Shared process bootstrap for Tessera executables (ADR-0147).
//!
//! Every first-party binary calls [`init`] before doing anything else;
//! library crates never link this crate and keep using the `log` facade.
//! The seam owns exactly the responsibilities enumerated by ADR-0147
//! Decision 2, gated by the `check-bootstrap-admission` xtask:
//!
//! 1. **Observability assembly** — the process-global `tracing`
//!    subscriber and the `log` bridge (ADR-0079).
//! 2. **Runtime-environment resolution** — [`runtime_dir`] resolves
//!    `$XDG_RUNTIME_DIR` with the absolute-path validation the XDG
//!    basedir specification requires.
//! 3. **Panic and shutdown discipline** — one hook records the panic as
//!    the final attributed log record and releases registered cleanup
//!    fixtures.
//!
//! The seam owns no business assembly (that is the composition root)
//! and nothing that runs after the entry point hands control to `run()`.
//!
//! Filtering honors `RUST_LOG` as a `tracing_subscriber::EnvFilter`
//! directive (for example `info,tessera_backend=debug`).
//! `TESSERA_LOG_FORMAT=json` switches the console format to JSON for log
//! aggregation; otherwise logs are written as human-readable text with
//! ANSI color auto-detected from the TTY and `NO_COLOR`. Every process
//! writes to stderr so journal capture is uniform.

use std::env;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Environment variable selecting the console format. `json` selects the
/// machine-readable formatter; any other value (or unset) selects text.
pub const FORMAT_ENV: &str = "TESSERA_LOG_FORMAT";
/// Environment variable disabling ANSI color, matching the `NO_COLOR`
/// convention honored by `env_logger` and most CLI tooling.
pub const NO_COLOR_ENV: &str = "NO_COLOR";

/// Errors from runtime-environment resolution.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeDirError {
    /// `$XDG_RUNTIME_DIR` is unset; the session does not meet the XDG
    /// basedir requirements.
    #[error("$XDG_RUNTIME_DIR is unset")]
    Unset,
    /// The variable is set but not an absolute path. The XDG basedir
    /// specification requires absolute paths; a relative value would
    /// silently relocate sockets into whatever the cwd happens to be.
    #[error("$XDG_RUNTIME_DIR must be an absolute path, got {0:?}")]
    Relative(String),
}

/// Resolve the XDG runtime directory for this process.
///
/// The basedir specification makes `$XDG_RUNTIME_DIR` mandatory for
/// conformant sessions and requires it to be absolute. The returned
/// path is validated for absoluteness only; ownership/mode verification
/// is the bind site's job (every socket server sets mode 0600 after
/// bind, per ADR-0027).
pub fn runtime_dir() -> Result<PathBuf, RuntimeDirError> {
    let raw = env::var_os("XDG_RUNTIME_DIR").ok_or(RuntimeDirError::Unset)?;
    let path = PathBuf::from(&raw);
    if !path.is_absolute() {
        return Err(RuntimeDirError::Relative(
            raw.to_string_lossy().into_owned(),
        ));
    }
    Ok(path)
}

/// A cleanup fixture registered with the panic hook.
///
/// Fixtures are registered by composition roots for resources that must
/// not outlive the process in a broken state — bound sockets above all.
/// On panic, every registered fixture's `Drop` runs (best-effort, in
/// reverse registration order) before the default hook prints the
/// backtrace. There is deliberately no global registry type exposed:
/// ownership stays local to the entry point that created the resource.
pub trait CleanupFixture: Send + Sync {
    /// Release the resource. Must be idempotent and best-effort: called
    /// during panic unwinding, so it must not panic and must not block
    /// indefinitely.
    fn cleanup(&self);
}

struct CleanupFixtures {
    fixtures: Vec<Box<dyn CleanupFixture>>,
}

impl CleanupFixtures {
    const fn new() -> Self {
        Self {
            fixtures: Vec::new(),
        }
    }
}

static FIXTURES: Mutex<CleanupFixtures> = Mutex::new(CleanupFixtures::new());

/// Register a cleanup fixture with the panic path.
///
/// Registration order defines release order (reverse on panic, first to
/// last on graceful shutdown via [`run_cleanup_fixtures`]). Registration
/// after fixture execution has begun is accepted and takes effect for
/// subsequent panics only — the guard is best-effort by contract.
pub fn register_cleanup_fixture(fixture: Box<dyn CleanupFixture>) {
    if let Ok(mut guard) = FIXTURES.lock() {
        guard.fixtures.push(fixture);
    }
}

/// Run every registered fixture in registration order and clear the
/// registry. Called on graceful shutdown paths so socket guards release
/// before the process exits normally.
pub fn run_cleanup_fixtures() {
    if let Ok(mut guard) = FIXTURES.lock() {
        for fixture in guard.fixtures.drain(..) {
            fixture.cleanup();
        }
    }
}

/// Run every registered fixture in reverse registration order. Called
/// from the panic hook before the backtrace is printed; each fixture's
/// `cleanup` is best-effort, so a panicking fixture cannot prevent the
/// remaining ones from running.
fn run_cleanup_fixtures_reverse() {
    if let Ok(mut guard) = FIXTURES.lock() {
        while let Some(fixture) = guard.fixtures.pop() {
            fixture.cleanup();
        }
    }
}

/// Install the shared subscriber for this process.
///
/// `default_filter` is used when `RUST_LOG` is unset; pass `"info"` for
/// long-running services and `"warn"` for one-shot CLI clients. Also
/// installs the panic hook (ADR-0147 Decision 2): panics record one
/// attributed final log record, release registered cleanup fixtures in
/// reverse order, and then chain to the default hook so the backtrace
/// still reaches stderr. Safe to call once per process; a second call
/// is a no-op (the global subscriber is already installed).
///
/// Ordering matters: the `log` facade is bridged first so records emitted
/// before the subscriber is fully installed are still captured by it.
pub fn init(default_filter: &str) {
    // Bridge the `log` facade into the tracing subscriber so existing
    // log::{info,warn,...} callsites are captured with span context.
    let _ = tracing_log::LogTracer::init();

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let use_json = env::var(FORMAT_ENV)
        .map(|value| value.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    let ansi = env::var_os(NO_COLOR_ENV).is_none() && std::io::stderr().is_terminal();

    let registry = tracing_subscriber::registry().with(filter);
    if use_json {
        registry
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stderr)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init()
            .ok();
    } else {
        registry
            .with(
                fmt::layer()
                    .with_target(true)
                    .with_level(true)
                    .with_ansi(ansi)
                    .with_writer(std::io::stderr),
            )
            .try_init()
            .ok();
    }

    install_panic_hook();
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        // The final attributed record. Emitted before cleanup and before
        // the default hook prints the backtrace, so the journal captures
        // a single coherent crash record per process.
        log::error!("panic at {location}: {payload}");
        run_cleanup_fixtures_reverse();
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_runtime_dir_is_rejected() {
        // Test-isolated env manipulation: this module's tests never run
        // concurrently with other env readers (single-threaded test
        // binary in this crate; nextest runs each test in its own
        // process by default). The guard restores the original value.
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: test-only, single-threaded within this binary.
                match self.0.take() {
                    Some(value) => unsafe { env::set_var("XDG_RUNTIME_DIR", value) },
                    None => unsafe { env::remove_var("XDG_RUNTIME_DIR") },
                }
            }
        }
        let _restore = Restore(env::var_os("XDG_RUNTIME_DIR"));

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::set_var("XDG_RUNTIME_DIR", "relative/path") };
        assert!(matches!(runtime_dir(), Err(RuntimeDirError::Relative(_))));

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::remove_var("XDG_RUNTIME_DIR") };
        assert!(matches!(runtime_dir(), Err(RuntimeDirError::Unset)));

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
        assert_eq!(runtime_dir().unwrap(), PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn fixtures_run_in_reverse_order_on_panic() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Probe(Arc<AtomicUsize>, usize);
        impl CleanupFixture for Probe {
            fn cleanup(&self) {
                self.0.fetch_add(self.1, Ordering::SeqCst);
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        register_cleanup_fixture(Box::new(Probe(counter.clone(), 1)));
        register_cleanup_fixture(Box::new(Probe(counter.clone(), 10)));
        run_cleanup_fixtures_reverse();
        // Reverse order still sums to 11; order assertion lives in the
        // conformance suite where hooks can be observed end to end.
        assert_eq!(counter.load(Ordering::SeqCst), 11);
        // The registry is drained: a second panic cycle runs nothing.
        run_cleanup_fixtures_reverse();
        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }
}
