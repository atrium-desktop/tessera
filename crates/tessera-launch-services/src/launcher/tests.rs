use super::*;
use std::path::PathBuf;

/// Minimal stand-in for an `Entry`, used to exercise the launcher without
/// a desktop scan.
struct Src {
    exec: Option<&'static str>,
    terminal: bool,
    icon: Option<&'static str>,
    wd: Option<PathBuf>,
}
impl LaunchSource for Src {
    fn id(&self) -> &str {
        "test.desktop"
    }
    fn name(&self) -> &str {
        "Test"
    }
    fn exec(&self) -> Option<&str> {
        self.exec
    }
    fn icon(&self) -> Option<&str> {
        self.icon
    }
    fn terminal(&self) -> bool {
        self.terminal
    }
    fn working_dir(&self) -> Option<&Path> {
        self.wd.as_deref()
    }
}

#[test]
fn no_exec_is_an_error() {
    let s = Src {
        exec: None,
        terminal: false,
        icon: None,
        wd: None,
    };
    let err = launch(&s as &dyn LaunchSource, &LaunchOpts::default()).unwrap_err();
    assert!(matches!(err, LaunchError::NoExec(_)), "{err:?}");
}

#[test]
fn detached_child_outlives_parent() {
    // Launch a command that writes a sentinel after a short delay. Because
    // the child is setsid-forked, this process does not wait for it.
    let dir = tempfile_dir();
    let marker = dir.path().join("out.txt");
    let s = Src {
        exec: Some("sh -c %f"),
        terminal: false,
        icon: None,
        wd: Some(dir.path().to_path_buf()),
    };
    let payload = format!("sleep 0.1; echo det > {}", marker.display());
    let opts = LaunchOpts {
        files: vec![payload],
        ..Default::default()
    };
    let report = launch(&s as &dyn LaunchSource, &opts).unwrap();
    assert!(report.pid > 0);

    let mut ok = false;
    for _ in 0..300 {
        if marker.exists() {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ok, "detached child never wrote the marker");
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "det");

    let supervisor = std::path::PathBuf::from(format!("/proc/{}/stat", report.pid));
    for _ in 0..300 {
        if !supervisor.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !supervisor.exists(),
        "setsid supervisor {} was not reaped",
        report.pid
    );
}

#[test]
fn detached_child_inherits_display_env() {
    let dir = tempfile_dir();
    let marker = dir.path().join("env.txt");
    let s = Src {
        exec: Some("sh -c %f"),
        terminal: false,
        icon: None,
        wd: Some(dir.path().to_path_buf()),
    };
    let payload = format!("echo \"$PATH\" > {}", marker.display());
    let opts = LaunchOpts {
        files: vec![payload],
        ..Default::default()
    };
    launch(&s as &dyn LaunchSource, &opts).unwrap();
    let mut got = String::new();
    for _ in 0..300 {
        if let Ok(s) = std::fs::read_to_string(&marker) {
            got = s;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!got.trim().is_empty(), "PATH not inherited by child");
}

#[test]
fn session_bus_and_desktop_vars_are_forwarded() {
    let mut cmd = Command::new("true");
    inherit_display_env(&mut cmd, Some("wayland-9"));
    let forwarded: std::collections::BTreeMap<String, Option<String>> = cmd
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        forwarded.get("WAYLAND_DISPLAY").and_then(|v| v.as_deref()),
        Some("wayland-9")
    );
    // Only read the shared test process environment; never mutate it.
    // Each whitelisted session variable that is set must reach the child.
    for var in [
        "XDG_RUNTIME_DIR",
        "XDG_CURRENT_DESKTOP",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Ok(value) = std::env::var(var) {
            assert_eq!(
                forwarded.get(var).and_then(|v| v.as_deref()),
                Some(value.as_str()),
                "{var} not forwarded"
            );
        }
    }
}

#[test]
fn foreground_waits_for_completion() {
    let dir = tempfile_dir();
    let marker = dir.path().join("foreground.txt");
    let s = Src {
        exec: Some("sh -c %f"),
        terminal: false,
        icon: None,
        wd: Some(dir.path().to_path_buf()),
    };
    let opts = LaunchOpts {
        files: vec!["printf done > foreground.txt".into()],
        foreground: true,
        ..Default::default()
    };
    let report = launch(&s as &dyn LaunchSource, &opts).unwrap();
    assert!(report.pid > 0);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "done");
}

#[test]
fn foreground_reports_nonzero_exit() {
    let s = Src {
        exec: Some("exit 7"),
        terminal: false,
        icon: None,
        wd: None,
    };
    let opts = LaunchOpts {
        foreground: true,
        ..Default::default()
    };
    let err = launch(&s as &dyn LaunchSource, &opts).unwrap_err();
    assert!(matches!(err, LaunchError::Exit { .. }), "{err:?}");
}

#[test]
fn terminal_wrapping_runs_headlessly() {
    let s = Src {
        exec: Some("true"),
        terminal: true,
        icon: None,
        wd: None,
    };
    let opts = LaunchOpts {
        foreground: true,
        // `true` accepts the generated `-e <command>` arguments without
        // opening a real terminal window or depending on the host's
        // graphical session.
        terminal: Some("true".into()),
        ..Default::default()
    };
    launch(&s as &dyn LaunchSource, &opts).expect("headless terminal wrapper");
}

#[test]
fn terminal_emulator_precedence() {
    assert_eq!(
        terminal_emulator_with_env(&LaunchOpts::default(), None),
        "xterm"
    );
    assert_eq!(
        terminal_emulator_with_env(&LaunchOpts::default(), Some("foot")),
        "foot"
    );
    assert_eq!(
        terminal_emulator_with_env(
            &LaunchOpts {
                terminal: Some("foot --".into()),
                ..Default::default()
            },
            Some("ignored"),
        ),
        "foot --"
    );
}

#[test]
fn interaction_domain_sandbox_is_fail_closed_and_parent_bound() {
    if !Path::new(BWRAP).is_file() {
        return;
    }
    let (_portal_dir, portal, portal_path) = wayland_portal();
    let sandbox = InteractionDomainSandbox {
        interaction_domain_id: 9,
        wayland_listener: portal,
        wayland_socket_path: portal_path,
        app_id: "test.desktop".into(),
        limits: InteractionDomainResourceLimits::default(),
    };
    let mut command = Command::new(BWRAP);
    append_bubblewrap_args(&mut command, "true", &sandbox, true).unwrap();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.iter().any(|arg| arg == "--unshare-all"));
    assert!(args.iter().any(|arg| arg == "--unshare-user"));
    assert!(args.iter().any(|arg| arg == "--disable-userns"));
    assert!(args.iter().any(|arg| arg == "--assert-userns-disabled"));
    assert!(args.iter().any(|arg| arg == "--die-with-parent"));
    assert!(args.iter().any(|arg| arg == "--cap-drop"));
    assert!(!args.iter().any(|arg| arg == "--share-net"));
    assert!(args.windows(2).any(|pair| pair == ["--tmpfs", "/dev/shm"]));
}

#[test]
fn sandbox_portal_is_host_unlinked_and_accepts_multiple_connections() {
    if !Path::new(BWRAP).is_file() || !Path::new("/usr/bin/wayland-info").is_file() {
        return;
    }
    if let Err(error) = prepare_interaction_domain_host() {
        eprintln!("skipping real Interaction Domain portal test: {error}");
        return;
    }
    let (_portal_dir, portal, portal_path) = wayland_portal();
    let compositor_listener = portal.try_clone().unwrap();
    let removed_path = portal_path.clone();
    let mut pre_gate_connection = std::os::unix::net::UnixStream::connect(&portal_path)
        .expect("model a same-UID pre-gate connection");
    let source = Src {
        exec: Some("sh -c %f"),
        terminal: false,
        icon: None,
        wd: None,
    };
    let opts = LaunchOpts {
        files: vec!["wayland-info & wayland-info & wait".into()],
        sandbox: Some(InteractionDomainSandbox {
            interaction_domain_id: 10,
            wayland_listener: portal,
            wayland_socket_path: portal_path,
            app_id: "test.desktop".into(),
            limits: InteractionDomainResourceLimits::default(),
        }),
        ..Default::default()
    };
    let mut launch = launch_managed(&source, &opts).unwrap();
    assert!(
        !removed_path.exists(),
        "portal must have no host pathname after the sandbox gate opens"
    );
    pre_gate_connection
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    let mut discarded = [0u8; 1];
    match pre_gate_connection.read(&mut discarded) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ) => {}
        result => panic!("pre-gate connection remained usable: {result:?}"),
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut accepted = 0;
    while accepted < 2 && std::time::Instant::now() < deadline {
        match compositor_listener.accept() {
            Ok((connection, _)) => {
                drop(connection);
                accepted += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => panic!("portal accept failed: {error}"),
        }
    }
    assert_eq!(
        accepted, 2,
        "one sandboxed application instance must be able to open multiple Wayland connections"
    );
    launch.terminate().unwrap();
}

#[test]
fn managed_interaction_domain_process_can_be_paused_resumed_and_revoked() {
    if !Path::new(BWRAP).is_file() {
        return;
    }
    if let Err(error) = prepare_interaction_domain_host() {
        eprintln!("skipping real Interaction Domain cgroup test: {error}");
        return;
    }
    let (_portal_dir, portal, portal_path) = wayland_portal();
    let source = Src {
        exec: Some("sh -c %f"),
        terminal: false,
        icon: None,
        wd: None,
    };
    let opts = LaunchOpts {
        // The worker deliberately escapes the bubblewrap supervisor's
        // process group/session. Only the cgroup freezer/kill boundary can
        // still control it as one unit.
        files: vec!["setsid sh -c 'while :; do sleep 1; done' & wait".into()],
        sandbox: Some(InteractionDomainSandbox {
            interaction_domain_id: 11,
            wayland_listener: portal,
            wayland_socket_path: portal_path,
            app_id: "test.desktop".into(),
            limits: InteractionDomainResourceLimits::default(),
        }),
        ..Default::default()
    };
    let mut launch = launch_managed(&source, &opts).unwrap();
    assert!(
        launch.report().resource_limits_enforced,
        "managed launches must fail instead of dropping resource controls"
    );
    assert_eq!(
        std::fs::read_to_string(launch.cgroup.path.join("memory.max"))
            .unwrap()
            .trim(),
        InteractionDomainResourceLimits::default()
            .memory_max_bytes
            .to_string()
    );
    assert_eq!(
        std::fs::read_to_string(launch.cgroup.path.join("pids.max"))
            .unwrap()
            .trim(),
        InteractionDomainResourceLimits::default()
            .pids_max
            .to_string()
    );
    assert_eq!(
        std::fs::read_to_string(launch.cgroup.path.join("cpu.weight"))
            .unwrap()
            .trim(),
        InteractionDomainResourceLimits::default()
            .cpu_weight
            .to_string()
    );
    assert_eq!(
        std::fs::read_to_string(launch.cgroup.path.join("memory.oom.group"))
            .unwrap()
            .trim(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(launch.cgroup.path.join("memory.swap.max"))
            .unwrap()
            .trim(),
        "0"
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        launch.is_running().unwrap(),
        "bubblewrap exited during sandbox setup"
    );
    launch.pause().unwrap();
    assert!(
        std::fs::read_to_string(launch.cgroup.path.join("cgroup.events"))
            .unwrap()
            .lines()
            .any(|line| line == "frozen 1"),
        "complete Interaction Domain cgroup was not frozen"
    );
    launch.resume().unwrap();
    assert!(
        std::fs::read_to_string(launch.cgroup.path.join("cgroup.events"))
            .unwrap()
            .lines()
            .any(|line| line == "frozen 0"),
        "complete Interaction Domain cgroup did not resume"
    );
    launch.terminate().unwrap();
    assert!(!launch.is_running().unwrap());
}

fn tempfile_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn wayland_portal() -> (tempfile::TempDir, UnixListener, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("wayland");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    (directory, listener, path)
}
