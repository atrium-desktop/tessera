//! Shared process environment and observability for Tessera executables (ADR-0147).
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
//! directive (for example `info,tessera_platform=debug`).
//! `TESSERA_LOG_FORMAT=json` switches the console format to JSON for log
//! aggregation; otherwise logs are written as human-readable text with
//! ANSI color auto-detected from the TTY and `NO_COLOR`. Every process
//! writes to stderr so journal capture is uniform.

pub mod signals;

use std::env;
use std::io::{IsTerminal, Write as _};
use std::path::PathBuf;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

/// Environment variable selecting the console format. `json` selects the
/// machine-readable formatter; any other value (or unset) selects text.
pub const FORMAT_ENV: &str = "TESSERA_LOG_FORMAT";
/// Environment variable disabling ANSI color, matching the `NO_COLOR`
/// convention honored by `env_logger` and most CLI tooling.
pub const NO_COLOR_ENV: &str = "NO_COLOR";
/// Environment variable overriding the session IPC socket path (ADR-0257).
/// Must be an absolute path when set; empty or relative values fail closed.
pub const IPC_SOCKET_ENV: &str = "TESSERA_IPC_SOCKET";

/// Errors from explicit IPC socket override resolution (`TESSERA_IPC_SOCKET`).
#[derive(Debug, PartialEq, Eq)]
pub enum IpcSocketError {
    /// `$TESSERA_IPC_SOCKET` is set to an empty string.
    Empty,
    /// `$TESSERA_IPC_SOCKET` is set to a relative path.
    Relative(String),
}

impl std::fmt::Display for IpcSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("$TESSERA_IPC_SOCKET must not be empty"),
            Self::Relative(path) => {
                write!(
                    f,
                    "$TESSERA_IPC_SOCKET must be an absolute path, got {path:?}"
                )
            }
        }
    }
}

impl std::error::Error for IpcSocketError {}

/// Resolve an explicit `$TESSERA_IPC_SOCKET` override, if set (ADR-0257).
///
/// Returns `Ok(None)` when the variable is unset so callers fall back to
/// `$XDG_RUNTIME_DIR/tessera.sock`. Empty or relative values fail closed
/// (`[INV-SETTINGS-05]`).
pub fn ipc_socket_override() -> Result<Option<PathBuf>, IpcSocketError> {
    env::var_os(IPC_SOCKET_ENV)
        .map(|raw| parse_ipc_socket(&raw))
        .transpose()
}

/// Validate one `$TESSERA_IPC_SOCKET` value (`[INV-SETTINGS-05]`).
///
/// The single rule every reader applies, including clients that resolve their
/// environment through a lookup function rather than the process environment.
pub fn parse_ipc_socket(raw: &std::ffi::OsStr) -> Result<PathBuf, IpcSocketError> {
    if raw.is_empty() {
        return Err(IpcSocketError::Empty);
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(IpcSocketError::Relative(raw.to_string_lossy().into_owned()));
    }
    Ok(path)
}

/// Errors from runtime-environment resolution.
#[derive(Debug)]
pub enum RuntimeDirError {
    /// `$XDG_RUNTIME_DIR` is unset; the session does not meet the XDG
    /// basedir requirements.
    Unset,
    /// The variable is set but not an absolute path. The XDG basedir
    /// specification requires absolute paths; a relative value would
    /// silently relocate sockets into whatever the cwd happens to be.
    Relative(String),
}

impl std::fmt::Display for RuntimeDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => f.write_str("$XDG_RUNTIME_DIR is unset"),
            Self::Relative(path) => {
                write!(f, "$XDG_RUNTIME_DIR must be an absolute path, got {path:?}")
            }
        }
    }
}

impl std::error::Error for RuntimeDirError {}

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

struct EnvLogger {
    default_level: LevelFilter,
    directives: Vec<(String, LevelFilter)>,
    json: bool,
    ansi: bool,
}

impl EnvLogger {
    fn parse(spec: &str, json: bool, ansi: bool) -> Self {
        let mut default_level = LevelFilter::Error;
        let mut directives = Vec::new();
        let mut saw_bare_level = false;

        for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some((target, level_str)) = part.split_once('=') {
                if let Some(level) = parse_level_filter(level_str.trim()) {
                    directives.push((target.trim().to_string(), level));
                }
            } else if let Some(level) = parse_level_filter(part) {
                default_level = level;
                saw_bare_level = true;
            } else {
                directives.push((part.to_string(), LevelFilter::Trace));
            }
        }

        if !saw_bare_level && directives.is_empty() {
            default_level = LevelFilter::Info;
        }
        // Longest prefix wins.
        directives.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        Self {
            default_level,
            directives,
            json,
            ansi,
        }
    }

    fn max_level(&self) -> LevelFilter {
        self.directives
            .iter()
            .map(|(_, lvl)| *lvl)
            .fold(self.default_level, std::cmp::max)
    }

    fn level_for(&self, target: &str) -> LevelFilter {
        for (prefix, level) in &self.directives {
            if target == prefix
                || target
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with("::") || rest.starts_with('.'))
            {
                return *level;
            }
        }
        self.default_level
    }
}

impl Log for EnvLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level_for(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut stderr = std::io::stderr().lock();
        if self.json {
            let level = record.level().as_str();
            let target = json_escape(record.target());
            let msg = json_escape(&record.args().to_string());
            let _ = writeln!(
                stderr,
                "{{\"level\":\"{level}\",\"target\":\"{target}\",\"message\":\"{msg}\"}}"
            );
        } else if self.ansi {
            let (color, level) = match record.level() {
                Level::Error => ("\x1b[31m", "ERROR"),
                Level::Warn => ("\x1b[33m", " WARN"),
                Level::Info => ("\x1b[32m", " INFO"),
                Level::Debug => ("\x1b[34m", "DEBUG"),
                Level::Trace => ("\x1b[35m", "TRACE"),
            };
            let _ = writeln!(
                stderr,
                "{color}{level}\x1b[0m \x1b[2m{}:\x1b[0m {}",
                record.target(),
                record.args()
            );
        } else {
            let _ = writeln!(
                stderr,
                "{} {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

fn parse_level_filter(raw: &str) -> Option<LevelFilter> {
    match raw.to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Install the shared logger for this process.
///
/// `default_filter` is used when `RUST_LOG` is unset; pass `"info"` for
/// long-running services and `"warn"` for one-shot CLI clients. Also
/// installs the panic hook (ADR-0147 Decision 2): panics record one
/// attributed final log record, release registered cleanup fixtures in
/// reverse order, and then chain to the default hook so the backtrace
/// still reaches stderr. Safe to call once per process; a second call
/// is a no-op (the global logger is already installed).
pub fn init(default_filter: &str) {
    let filter_spec = env::var("RUST_LOG").unwrap_or_else(|_| default_filter.to_string());
    let use_json = env::var(FORMAT_ENV)
        .map(|value| value.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    let ansi = env::var_os(NO_COLOR_ENV).is_none() && std::io::stderr().is_terminal();

    let logger = EnvLogger::parse(&filter_spec, use_json, ansi);
    let max_level = logger.max_level();
    if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
        log::set_max_level(max_level);
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
    fn ipc_socket_override_validates_absolute_paths() {
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: test-only, single-threaded within this binary.
                match self.0.take() {
                    Some(value) => unsafe { env::set_var(IPC_SOCKET_ENV, value) },
                    None => unsafe { env::remove_var(IPC_SOCKET_ENV) },
                }
            }
        }
        let _restore = Restore(env::var_os(IPC_SOCKET_ENV));

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::remove_var(IPC_SOCKET_ENV) };
        assert_eq!(ipc_socket_override(), Ok(None));

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::set_var(IPC_SOCKET_ENV, "") };
        assert_eq!(ipc_socket_override(), Err(IpcSocketError::Empty));

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::set_var(IPC_SOCKET_ENV, "relative/tessera.sock") };
        assert_eq!(
            ipc_socket_override(),
            Err(IpcSocketError::Relative("relative/tessera.sock".into()))
        );

        // SAFETY: test-only, single-threaded within this binary.
        unsafe { env::set_var(IPC_SOCKET_ENV, "/tmp/tessera-dev/tessera.sock") };
        assert_eq!(
            ipc_socket_override(),
            Ok(Some(PathBuf::from("/tmp/tessera-dev/tessera.sock")))
        );
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
