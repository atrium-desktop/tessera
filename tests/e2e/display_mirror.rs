//! End-to-end tests for default HDMI mirroring and real DRM display output.

use std::sync::{Arc, Mutex};

use tessera_desktop::output::{
    DisplayTopology, OutputGeometry, OutputInfo, OutputKind, OutputMode, OutputRole,
    OutputRoleSource, resolve_output_roles,
};
use tessera_desktop::settings::{SettingsAction, SettingsReceipt};
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_ipc::{Handler, Server};
use tessera_primitives::{Point, Transform};
use tessera_protocol::{
    Command, ConnectionCapabilities, JournalSnapshot, LOCAL_OWNER_ADMIN_SCOPE, Scope,
};

struct DisplayState {
    topology: DisplayTopology,
    outputs: Vec<OutputInfo>,
    revision: u64,
}

impl DisplayState {
    fn new() -> Self {
        let mut state = Self {
            topology: DisplayTopology::default(),
            outputs: vec![
                make_output("eDP-1", OutputKind::Internal, 3072, 1920, 120_000),
                make_output("HDMI-A-1", OutputKind::External, 3840, 2160, 60_000),
            ],
            revision: 1,
        };
        state.recompute_roles();
        state
    }

    fn recompute_roles(&mut self) {
        let roles = resolve_output_roles(&self.outputs, &Default::default(), self.topology);
        for (output, role) in self.outputs.iter_mut().zip(roles) {
            output.role = role;
            output.role_source = OutputRoleSource::Auto;
        }
    }
}

fn make_output(
    connector: &str,
    kind: OutputKind,
    width: i32,
    height: i32,
    refresh_mhz: u32,
) -> OutputInfo {
    let mode = OutputMode {
        width,
        height,
        refresh_mhz,
    };
    OutputInfo {
        connector: connector.into(),
        geometry: OutputGeometry {
            mode,
            scale: Default::default(),
            transform: Transform::Normal,
            logical_origin: Point::default(),
            logical_size_override: None,
        },
        kind,
        available_modes: vec![mode],
        color_caps: Default::default(),
        role: OutputRole::Extend,
        role_source: OutputRoleSource::Auto,
    }
}

struct DisplayHandler {
    state: Mutex<DisplayState>,
}

impl Handler for DisplayHandler {
    fn policy_caps(&self) -> ConnectionCapabilities {
        ConnectionCapabilities {
            query: true,
            session: true,
            ..ConnectionCapabilities::QUERY
        }
    }

    fn windows(&self) -> Vec<Window> {
        Vec::new()
    }

    fn workspaces(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            outputs: Vec::new(),
        }
    }

    fn notifications(&self) -> Vec<tessera_desktop::notify::Notification> {
        Vec::new()
    }

    fn outputs(&self) -> Vec<OutputInfo> {
        self.state.lock().unwrap().outputs.clone()
    }

    fn journal_since(&self, _since: u64) -> JournalSnapshot {
        JournalSnapshot {
            entries: Vec::new(),
            oldest_seq: 0,
            latest_seq: 0,
        }
    }

    fn command(&self, _conn_id: u64, _subject: Option<&str>, _command: Command) {}

    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        (name == LOCAL_OWNER_ADMIN_SCOPE).then_some(Scope {
            windows: None,
            workspaces: None,
            outputs: None,
            interaction_domains: None,
            ops: None,
            ask_ops: None,
        })
    }

    fn builtin_scope_permitted(
        &self,
        name: &str,
        peer_executable: Option<&std::path::Path>,
    ) -> bool {
        name == LOCAL_OWNER_ADMIN_SCOPE
            && peer_executable == Some(crate::common::binary("tessera").as_path())
    }

    fn settings_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        expected_revision: Option<u64>,
        action: SettingsAction,
    ) -> Result<SettingsReceipt, String> {
        let mut state = self.state.lock().unwrap();
        if expected_revision.is_some_and(|expected| expected != state.revision) {
            return Err(format!(
                "settings revision conflict: expected {}, actual {}",
                expected_revision.unwrap(),
                state.revision
            ));
        }
        if let SettingsAction::SetDisplayTopology { topology } = action {
            state.topology = topology;
            state.recompute_roles();
        }
        state.revision += 1;
        Ok(SettingsReceipt {
            revision: state.revision,
        })
    }
}

fn run_display_json(runtime_dir: &std::path::Path) -> serde_json::Value {
    let output = std::process::Command::new(crate::common::binary("tessera"))
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .args(["display", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tessera display --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn default_display_topology_mirrors_connected_hdmi_via_cli() {
    let runtime = tempfile::tempdir().unwrap();
    let socket = runtime.path().join("tessera.sock");
    let _server = Server::start(
        &socket,
        Arc::new(DisplayHandler {
            state: Mutex::new(DisplayState::new()),
        }),
    )
    .unwrap();

    let outputs = run_display_json(runtime.path());
    let rows = outputs.as_array().unwrap();
    let hdmi = rows
        .iter()
        .find(|row| row["connector"] == "HDMI-A-1")
        .expect("HDMI-A-1 present");
    assert_eq!(hdmi["role"]["mode"], "mirror");
    assert_eq!(hdmi["role"]["source"], "eDP-1");

    let extend = std::process::Command::new(crate::common::binary("tessera"))
        .env("XDG_RUNTIME_DIR", runtime.path())
        .args(["display", "topology", "extend"])
        .output()
        .unwrap();
    assert!(
        extend.status.success(),
        "tessera display topology extend failed: {}",
        String::from_utf8_lossy(&extend.stderr)
    );
    let outputs = run_display_json(runtime.path());
    let hdmi = outputs
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["connector"] == "HDMI-A-1")
        .unwrap();
    assert_eq!(hdmi["role"]["mode"], "extend");

    let mirror = std::process::Command::new(crate::common::binary("tessera"))
        .env("XDG_RUNTIME_DIR", runtime.path())
        .args(["display", "topology", "mirror"])
        .output()
        .unwrap();
    assert!(
        mirror.status.success(),
        "tessera display topology mirror failed: {}",
        String::from_utf8_lossy(&mirror.stderr)
    );
    let outputs = run_display_json(runtime.path());
    let hdmi = outputs
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["connector"] == "HDMI-A-1")
        .unwrap();
    assert_eq!(hdmi["role"]["mode"], "mirror");
    assert_eq!(hdmi["role"]["source"], "eDP-1");
}

#[test]
#[ignore = "requires a live physical DRM session with >= 2 connected outputs (e.g. eDP-1 + HDMI-A-1)"]
fn real_drm_hdmi_session_defaults_to_mirror() {
    let connected: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .expect("/sys/class/drm available")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let status_path = entry.path().join("status");
            let status = std::fs::read_to_string(status_path).ok()?;
            (status.trim() == "connected").then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        connected.len() >= 2,
        "expected >= 2 connected DRM outputs, found {connected:?}"
    );

    let output = std::process::Command::new(crate::common::binary("tessera"))
        .args(["display", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tessera display --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        rows.len() >= 2,
        "expected >= 2 outputs from running compositor, got {rows:?}"
    );
    let has_mirror = rows
        .iter()
        .any(|row| row["role"]["mode"] == "mirror" && row["role"]["source"].is_string());
    assert!(
        has_mirror,
        "expected at least one connected external output to mirror by default, got {rows:?}"
    );
}
