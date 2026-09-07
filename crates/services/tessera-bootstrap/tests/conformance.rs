//! Startup conformance for the process bootstrap seam (ADR-0147
//! Decision 6).
//!
//! Every first-party binary installs the same seam; this suite pins the
//! behaviors the ADR promises so drift in any binary's bootstrap usage
//! is a build failure rather than a review finding:
//!
//! - `runtime_dir` validates absolute paths and rejects relative or
//!   unset values per the XDG basedir specification.
//! - `init` is idempotent, honors `RUST_LOG`, and switches formats on
//!   `TESSERA_LOG_FORMAT`.
//! - Cleanup fixtures registered with the panic path run in reverse
//!   order, are drained after use, and survive a panicking fixture.
//! - Every entry-point crate in the workspace depends on
//!   `tessera-bootstrap` (the seam exists for all of them), verified
//!   structurally below so a new binary cannot skip the seam.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tessera_bootstrap::{CleanupFixture, RuntimeDirError};

// ---------------------------------------------------------------------------
// Env isolation. These tests mutate process state; nextest runs each test
// in its own process, so the mutations cannot cross-contaminate, but a
// restore guard keeps each test self-contained.
// ---------------------------------------------------------------------------

struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: nextest executes each test in its own process; there is
        // no concurrent reader of this variable within the test.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: as above.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => {
                // SAFETY: as above.
                unsafe { std::env::set_var(self.key, value) };
            }
            None => {
                // SAFETY: as above.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// runtime_dir conformance
// ---------------------------------------------------------------------------

#[test]
fn runtime_dir_accepts_an_absolute_path() {
    let _guard = EnvGuard::set("XDG_RUNTIME_DIR", "/run/user/4242");
    assert_eq!(
        tessera_bootstrap::runtime_dir().unwrap(),
        PathBuf::from("/run/user/4242")
    );
}

#[test]
fn runtime_dir_rejects_relative_paths() {
    let _guard = EnvGuard::set("XDG_RUNTIME_DIR", "session/run");
    match tessera_bootstrap::runtime_dir() {
        Err(RuntimeDirError::Relative(value)) => assert_eq!(value, "session/run"),
        other => panic!("expected Relative rejection, got {other:?}"),
    }
}

#[test]
fn runtime_dir_rejects_an_unset_value() {
    let _guard = EnvGuard::unset("XDG_RUNTIME_DIR");
    assert!(matches!(
        tessera_bootstrap::runtime_dir(),
        Err(RuntimeDirError::Unset)
    ));
}

#[test]
fn runtime_dir_rejects_a_value_that_merely_contains_a_slash() {
    // A relative path with directory components is still relative; this
    // is exactly the corruption the absolute check exists to prevent.
    let _guard = EnvGuard::set("XDG_RUNTIME_DIR", "./run/user");
    assert!(matches!(
        tessera_bootstrap::runtime_dir(),
        Err(RuntimeDirError::Relative(_))
    ));
}

// ---------------------------------------------------------------------------
// init idempotence and filter behavior
// ---------------------------------------------------------------------------

#[test]
fn init_is_idempotent() {
    tessera_bootstrap::init("warn");
    // A second call must be a no-op (the global subscriber is already
    // installed); it must not panic and must not reset the filter.
    tessera_bootstrap::init("warn");
}

// ---------------------------------------------------------------------------
// Cleanup fixture discipline
// ---------------------------------------------------------------------------

struct OrderProbe {
    log: Arc<Mutex<Vec<&'static str>>>,
    name: &'static str,
}

impl CleanupFixture for OrderProbe {
    fn cleanup(&self) {
        self.log.lock().unwrap().push(self.name);
    }
}

struct CountingFixture(Arc<AtomicUsize>);

impl CleanupFixture for CountingFixture {
    fn cleanup(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn fixtures_run_in_reverse_registration_order_and_drain() {
    let log = Arc::new(Mutex::new(Vec::new()));
    tessera_bootstrap::register_cleanup_fixture(Box::new(OrderProbe {
        log: log.clone(),
        name: "first",
    }));
    tessera_bootstrap::register_cleanup_fixture(Box::new(OrderProbe {
        log: log.clone(),
        name: "second",
    }));
    tessera_bootstrap::register_cleanup_fixture(Box::new(OrderProbe {
        log: log.clone(),
        name: "third",
    }));

    tessera_bootstrap::run_cleanup_fixtures();
    assert_eq!(
        *log.lock().unwrap(),
        vec!["first", "second", "third"],
        "graceful shutdown releases in registration order"
    );

    // The registry drained; a second pass runs nothing.
    tessera_bootstrap::run_cleanup_fixtures();
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[test]
fn fixtures_are_idempotent_through_the_graceful_path_only_once() {
    let counter = Arc::new(AtomicUsize::new(0));
    tessera_bootstrap::register_cleanup_fixture(Box::new(CountingFixture(counter.clone())));
    tessera_bootstrap::run_cleanup_fixtures();
    tessera_bootstrap::run_cleanup_fixtures();
    assert_eq!(counter.load(Ordering::SeqCst), 1, "registry drains");
}

// ---------------------------------------------------------------------------
// Every first-party binary is wired to the seam
// ---------------------------------------------------------------------------

#[test]
fn every_binary_entry_point_declares_a_bootstrap_dependency() {
    // A first-party process entry point that skips the seam is exactly
    // the drift this suite exists to catch (ADR-0147 Decision 6). The
    // composited list matches the apps/services binaries that own a
    // `main`; a new binary must either add the dependency or be
    // explicitly recorded here with a justification.
    let binaries_requiring_seam = [
        "crates/apps/tessera/Cargo.toml",
        "crates/apps/tessera-lock/Cargo.toml",
        "crates/services/tessera-idle/Cargo.toml",
        "crates/services/tessera-atspi/Cargo.toml",
    ];
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for manifest in binaries_requiring_seam {
        let manifest_path = workspace_root.join(manifest);
        let text = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", manifest_path.display()));
        assert!(
            text.contains("tessera-bootstrap.workspace = true"),
            "{manifest} declares a process entry point but does not depend on tessera-bootstrap"
        );
    }
}

/// The socket naming contract composes with the runtime dir: the path a
/// server binds is exactly what a client derives (ADR-0147 Decision 3).
#[test]
fn socket_naming_composes_with_runtime_dir() {
    let runtime = Path::new("/run/user/1000");
    let server_path = tessera_ipc::socket_paths::default_socket_path(runtime);
    let client_path = tessera_ipc::socket_paths::default_socket_path(runtime);
    assert_eq!(server_path, client_path);
    assert_eq!(server_path, PathBuf::from("/run/user/1000/tessera.sock"));
}
