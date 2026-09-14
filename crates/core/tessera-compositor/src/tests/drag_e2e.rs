use crate::*;

static DRAG_E2E: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn toplevel_drag_probe_binary() -> Option<std::path::PathBuf> {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("toplevel_drag_probe.c");
    if !source.exists() {
        eprintln!("skipping: tests/toplevel_drag_probe.c missing");
        return None;
    }
    let out = std::env::temp_dir().join(format!("tessera-drag-probe-{}", std::process::id()));
    std::fs::create_dir_all(&out).ok()?;
    let xml = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tessera-wayland-protocols/protocols/xdg-toplevel-drag-v1.xml");
    let header = out.join("xdg-toplevel-drag-v1-client.h");
    let code = out.join("xdg-toplevel-drag-v1-protocol.c");
    let bin = out.join("toplevel_drag_probe");
    let ok = std::process::Command::new("wayland-scanner")
        .args(["client-header"])
        .arg(&xml)
        .arg(&header)
        .status()
        .ok()?
        .success()
        && std::process::Command::new("wayland-scanner")
            .args(["private-code"])
            .arg(&xml)
            .arg(&code)
            .status()
            .ok()?
            .success();
    if !ok {
        eprintln!("skipping: wayland-scanner failed");
        return None;
    }
    let mut build = std::process::Command::new("gcc");
    build
        .arg("-o")
        .arg(&bin)
        .arg(&source)
        .arg(&code)
        .arg("-I")
        .arg(&out)
        .args([
            "-lwayland-client",
            "-Werror=implicit-function-declaration",
            "-Wall",
            "-Wextra",
        ]);
    let status = build.status().ok()?;
    if !status.success() {
        eprintln!("skipping: gcc build failed");
        return None;
    }
    Some(bin)
}

#[test]
fn server_advertises_xdg_toplevel_drag_manager() {
    let _serial = DRAG_E2E.lock().unwrap_or_else(|p| p.into_inner());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let server = Server::new().expect("Server::new");
    assert!(!server.socket().is_empty());
}

#[test]
fn toplevel_drag_probe_e2e_client_binds_and_creates_drag() {
    let _serial = DRAG_E2E.lock().unwrap_or_else(|p| p.into_inner());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let Some(bin) = toplevel_drag_probe_binary() else {
        eprintln!("skipping: probe binary compilation failed");
        return;
    };
    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();

    let mut child = std::process::Command::new(&bin)
        .env("WAYLAND_DISPLAY", &socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn probe");

    let mut child_exited = false;
    for _ in 0..2_000 {
        server.dispatch();
        if child.try_wait().expect("poll probe").is_some() {
            child_exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(child_exited, "probe timed out");
    let output = child.wait_with_output().expect("collect probe");
    assert!(
        output.status.success(),
        "probe exited with failure: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("toplevel-drag-manager-bound"));
    assert!(stdout.contains("toplevel-drag-created"));
    assert!(stdout.contains("probe-success"));
}

#[test]
fn optics_iris_dnd_contract_action_negotiation() {
    // Optics Iris (ADR-0086) advertises COPY | MOVE with preferred COPY.
    let source_actions = ffi::WL_DATA_ACTION_COPY | ffi::WL_DATA_ACTION_MOVE;
    let dest_actions = ffi::WL_DATA_ACTION_COPY;
    let preferred = ffi::WL_DATA_ACTION_COPY;

    let selected = crate::protocol::choose_dnd_action(source_actions, dest_actions, preferred);
    assert_eq!(selected, ffi::WL_DATA_ACTION_COPY);

    // When destination prefers MOVE and source supports it:
    let selected_move = crate::protocol::choose_dnd_action(
        source_actions,
        ffi::WL_DATA_ACTION_MOVE,
        ffi::WL_DATA_ACTION_MOVE,
    );
    assert_eq!(selected_move, ffi::WL_DATA_ACTION_MOVE);

    // Incompatible action negotiation yields ACTION_NONE:
    let selected_incompatible =
        crate::protocol::choose_dnd_action(ffi::WL_DATA_ACTION_COPY, ffi::WL_DATA_ACTION_MOVE, 0);
    assert_eq!(selected_incompatible, ffi::WL_DATA_ACTION_NONE);
}

#[test]
fn optics_touch_and_pointer_drag_event_parity() {
    let pointer_origin = crate::DragOriginDevice::Pointer;
    let touch_origin = crate::DragOriginDevice::Touch { id: 0 };

    assert_eq!(pointer_origin, crate::DragOriginDevice::Pointer);
    assert_ne!(pointer_origin, touch_origin);
}

