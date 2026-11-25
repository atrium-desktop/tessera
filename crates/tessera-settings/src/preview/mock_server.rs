//! In-process wire-level mock compositor IPC server for `tessera-settings-preview`
//! (ADR-0257).
//!
//! Serves the real `tessera_ipc::Server` protocol on a local unix socket so
//! `tessera-settings-preview` (and `--mock-server-only` + production
//! `tessera-settings --socket PATH`) exercises the real two-connection `Link`,
//! scope negotiation, revisioned transactions, and `SettingsChanged` event
//! fan-out without launching a compositor.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tessera_desktop::output::{
    DisplayTopology, OutputGeometry, OutputInfo, OutputKind, OutputMode, OutputRole,
    OutputRoleSource, Scale,
};
use tessera_desktop::settings::{
    AccentColor, ColorScheme, Contrast, DesktopPreferences, DisplaySettings, DisplayStatus,
    DockSettings, IdleSettings, Material, MaterialCeiling, MaterialCeilingReason, ResolvedMaterial,
    SettingsAction, SettingsReceipt, SettingsSnapshot, WallpaperSettings, resolve_material,
};
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_ipc::{Handler, Server};
use tessera_primitives::input::{
    InputStatus, KeyboardConfig, MouseCapabilities, MouseConfig, MouseStatus, TouchpadCapabilities,
    TouchpadConfig, TouchpadStatus,
};
use tessera_primitives::{Point, Transform};
use tessera_protocol::journal::JournalSnapshot;
use tessera_protocol::{Command, ConnectionCapabilities, Event, LOCAL_SETTINGS_SCOPE, Scope};

const TENTATIVE_REVERT_WINDOW: Duration = Duration::from_secs(15);

/// Deterministic hardware & configuration snapshots selectable via `--preset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Single 2560x1440@144Hz external monitor, configurable mouse + keyboard.
    Desktop,
    /// Dual-monitor setup (`DP-1` 2560x1440@144Hz primary + `HDMI-A-1`
    /// 1920x1080@60Hz extend) for exercising arrangement, mirror, and 15s preview.
    MultiDisplay,
    /// Built-in HiDPI panel (`eDP-1` 2880x1800@120Hz @ 1.75x) + full touchpad
    /// capabilities + `OnBattery` material ceiling notice.
    Laptop,
    /// Non-configurable display/input state mirroring a nested session.
    Minimal,
}

impl Preset {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "desktop" => Ok(Self::Desktop),
            "multi-display" | "multi" => Ok(Self::MultiDisplay),
            "laptop" => Ok(Self::Laptop),
            "minimal" | "nested" => Ok(Self::Minimal),
            other => Err(format!(
                "unknown --preset {other:?} (expected: desktop, multi-display, laptop, minimal)"
            )),
        }
    }

    pub fn initial_snapshot(self) -> SettingsSnapshot {
        match self {
            Self::Desktop => snapshot_desktop(),
            Self::MultiDisplay => snapshot_multi_display(),
            Self::Laptop => snapshot_laptop(),
            Self::Minimal => snapshot_minimal(),
        }
    }
}

/// Deterministic link / authority modes selectable via `--mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultMode {
    /// Normal read/write Settings scope with revision checks and event broadcast.
    Live,
    /// Refuse the `tessera-settings` built-in scope so Settings degrades to read-only.
    ReadOnly,
    /// Accept the scope but reject every mutation with a stale-revision error.
    RejectStale,
    /// Accept the scope but reject every mutation as if the session were locked.
    RejectLocked,
    /// Do not bind a server socket so Settings renders its disconnected/retrying state.
    Disconnected,
}

impl FaultMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "live" => Ok(Self::Live),
            "read-only" | "readonly" => Ok(Self::ReadOnly),
            "reject-stale" | "stale" => Ok(Self::RejectStale),
            "reject-locked" | "locked" => Ok(Self::RejectLocked),
            "disconnected" | "offline" => Ok(Self::Disconnected),
            other => Err(format!(
                "unknown --mode {other:?} (expected: live, read-only, reject-stale, reject-locked, disconnected)"
            )),
        }
    }
}

struct TentativeDisplay {
    previous: DisplayStatus,
    deadline: Instant,
}

struct State {
    snapshot: SettingsSnapshot,
    preset: Preset,
    tentative: Option<TentativeDisplay>,
}

impl State {
    fn reconcile_tentative(&mut self, now: Instant) -> bool {
        let Some(pending) = &self.tentative else {
            self.snapshot.display.pending_revert_seconds = None;
            return false;
        };
        if now >= pending.deadline {
            if let Some(expired) = self.tentative.take() {
                self.snapshot.display = expired.previous;
            }
            self.snapshot.display.pending_revert_seconds = None;
            self.snapshot.revision = self.snapshot.revision.saturating_add(1);
            true
        } else {
            let remaining = pending
                .deadline
                .saturating_duration_since(now)
                .as_secs()
                .max(1) as u32;
            self.snapshot.display.pending_revert_seconds = Some(remaining);
            false
        }
    }

    fn resolve_material_for(&self, prefs: &DesktopPreferences) -> ResolvedMaterial {
        let mut ceilings = Vec::new();
        if prefs.contrast == Contrast::High {
            ceilings.push(MaterialCeiling {
                max: Material::Opaque,
                reason: MaterialCeilingReason::HighContrast,
            });
        }
        if self.preset == Preset::Laptop {
            ceilings.push(MaterialCeiling {
                max: prefs.material_power.on_battery,
                reason: MaterialCeilingReason::OnBattery,
            });
        }
        resolve_material(prefs.material, ceilings)
    }
}

/// Wire-level mock IPC handler backing `tessera-settings-preview`.
pub struct MockSettingsHandler {
    mode: FaultMode,
    state: Mutex<State>,
    server: OnceLock<Arc<Server>>,
}

impl MockSettingsHandler {
    pub fn start(
        socket: &Path,
        preset: Preset,
        mode: FaultMode,
    ) -> io::Result<(Arc<Self>, Arc<Server>)> {
        let handler = Arc::new(Self {
            mode,
            state: Mutex::new(State {
                snapshot: preset.initial_snapshot(),
                preset,
                tentative: None,
            }),
            server: OnceLock::new(),
        });
        let server = Arc::new(Server::start(socket, Arc::clone(&handler))?);
        let _ = handler.server.set(Arc::clone(&server));
        Ok((handler, server))
    }

    fn broadcast_settings_changed(&self, revision: u64) {
        if let Some(server) = self.server.get() {
            server.broadcast(Event::SettingsChanged { revision });
        }
    }
}

impl Handler for MockSettingsHandler {
    fn policy_caps(&self) -> ConnectionCapabilities {
        ConnectionCapabilities {
            query: true,
            session: true,
            ..ConnectionCapabilities::default()
        }
    }

    fn windows(&self) -> Vec<tessera_desktop::window::Window> {
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
        self.settings().display.outputs
    }

    fn journal_since(&self, _since: u64) -> JournalSnapshot {
        JournalSnapshot {
            latest_seq: 0,
            oldest_seq: 0,
            entries: Vec::new(),
        }
    }

    fn settings(&self) -> SettingsSnapshot {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let reverted = guard.reconcile_tentative(Instant::now());
        let snapshot = guard.snapshot.clone();
        drop(guard);
        if reverted {
            self.broadcast_settings_changed(snapshot.revision);
        }
        snapshot
    }

    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        if name == LOCAL_SETTINGS_SCOPE {
            Some(Scope {
                ops: Some(Vec::new()),
                ..Scope::default()
            })
        } else {
            None
        }
    }

    fn builtin_scope_permitted(&self, name: &str, _peer_exe: Option<&Path>) -> bool {
        if name == LOCAL_SETTINGS_SCOPE && self.mode == FaultMode::ReadOnly {
            return false;
        }
        true
    }

    fn command(&self, _conn_id: u64, _subject: Option<&str>, _cmd: Command) {}

    fn settings_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        expected_revision: Option<u64>,
        action: SettingsAction,
    ) -> Result<SettingsReceipt, String> {
        action.validate().map_err(str::to_owned)?;

        match self.mode {
            FaultMode::RejectStale => {
                let current = self
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot
                    .revision;
                return Err(format!(
                    "settings revision mismatch: expected {}, current {}",
                    expected_revision.unwrap_or(0),
                    current.saturating_add(1)
                ));
            }
            FaultMode::RejectLocked => {
                return Err("settings cannot be changed while the session is locked".into());
            }
            FaultMode::ReadOnly => {
                return Err("settings scope is read-only".into());
            }
            FaultMode::Live | FaultMode::Disconnected => {}
        }

        let revision = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.reconcile_tentative(Instant::now());

            if let Some(expected) = expected_revision
                && expected != guard.snapshot.revision
            {
                return Err(format!(
                    "settings revision mismatch: expected {expected}, current {}",
                    guard.snapshot.revision
                ));
            }

            apply_action(&mut guard, action)?;
            guard.snapshot.revision = guard.snapshot.revision.saturating_add(1);
            guard.snapshot.revision
        };

        self.broadcast_settings_changed(revision);
        Ok(SettingsReceipt { revision })
    }
}

fn apply_action(state: &mut State, action: SettingsAction) -> Result<(), String> {
    match action {
        SettingsAction::SetInput { config } => {
            if !state.snapshot.input.configurable {
                return Err("input devices are owned by the outer compositor".into());
            }
            state.snapshot.input.touchpad.config = config.touchpad;
            state.snapshot.input.mouse.config = config.mouse;
            state.snapshot.input.keyboard = config.keyboard;
            state.snapshot.input.mappings = config.mappings;
        }
        SettingsAction::SetDisplay { settings } => {
            if !state.snapshot.display.configurable {
                return Err("physical outputs are owned by the outer compositor".into());
            }
            if settings.tentative {
                let previous = state.snapshot.display.clone();
                apply_display_edit(&mut state.snapshot.display, &settings)?;
                state.tentative = Some(TentativeDisplay {
                    previous,
                    deadline: Instant::now() + TENTATIVE_REVERT_WINDOW,
                });
                state.snapshot.display.pending_revert_seconds =
                    Some(TENTATIVE_REVERT_WINDOW.as_secs() as u32);
            } else {
                apply_display_edit(&mut state.snapshot.display, &settings)?;
                state.tentative = None;
                state.snapshot.display.pending_revert_seconds = None;
            }
            state.snapshot.display.error = None;
        }
        SettingsAction::SetDisplayTopology { topology } => {
            if !state.snapshot.display.configurable {
                return Err("physical outputs are owned by the outer compositor".into());
            }
            state.snapshot.display.topology = topology;
        }
        SettingsAction::ConfirmDisplay => {
            state.tentative = None;
            state.snapshot.display.pending_revert_seconds = None;
        }
        SettingsAction::SetDesktopPreferences { preferences } => {
            let resolved = state.resolve_material_for(&preferences);
            state.snapshot.preferences = preferences;
            state.snapshot.material = Some(resolved);
        }
        SettingsAction::SetIdle { settings } => {
            state.snapshot.idle = settings;
        }
        SettingsAction::SetDock { settings } => {
            state.snapshot.dock = settings;
        }
        SettingsAction::SetWallpaper { settings } => {
            state.snapshot.wallpaper = settings;
        }
    }
    Ok(())
}

fn apply_display_edit(display: &mut DisplayStatus, edit: &DisplaySettings) -> Result<(), String> {
    let Some(index) = display
        .outputs
        .iter()
        .position(|output| output.connector == edit.connector)
    else {
        return Err(format!("unknown display connector {:?}", edit.connector));
    };

    if edit.primary && index != 0 {
        let chosen = display.outputs.remove(index);
        display.outputs.insert(0, chosen);
    }

    let Some(target) = display
        .outputs
        .iter_mut()
        .find(|output| output.connector == edit.connector)
    else {
        return Err(format!("unknown display connector {:?}", edit.connector));
    };

    match &edit.role {
        None => {
            target.role = OutputRole::Extend;
            target.role_source = OutputRoleSource::Auto;
        }
        Some(role) => {
            target.role = role.clone();
            target.role_source = OutputRoleSource::Explicit;
        }
    }

    if let Some(mode_spec) = edit.mode {
        if let Some(matched) = target
            .available_modes
            .iter()
            .copied()
            .find(|mode| mode_spec.matches(mode))
        {
            target.geometry.mode = matched;
        } else {
            target.geometry.mode = OutputMode {
                width: mode_spec.width,
                height: mode_spec.height,
                refresh_mhz: mode_spec.refresh_hz.unwrap_or(60) * 1000,
            };
        }
    }
    if let Some(scale) = edit.scale {
        target.geometry.scale = Scale(scale as f32);
    }
    if let Some(position) = edit.position {
        target.geometry.logical_origin = position;
    }
    Ok(())
}

fn make_output(
    connector: &str,
    kind: OutputKind,
    mode: OutputMode,
    available_modes: Vec<OutputMode>,
    scale: f32,
    origin: Point,
) -> OutputInfo {
    OutputInfo {
        connector: connector.to_owned(),
        geometry: OutputGeometry {
            mode,
            scale: Scale(scale),
            transform: Transform::Normal,
            logical_origin: origin,
            logical_size_override: None,
        },
        kind,
        available_modes,
        color_caps: Default::default(),
        role: OutputRole::Extend,
        role_source: OutputRoleSource::Auto,
    }
}

fn base_preferences(material: Material) -> DesktopPreferences {
    DesktopPreferences {
        color_scheme: ColorScheme::Dark,
        accent_color: Some(AccentColor {
            red: 0x3B,
            green: 0x82,
            blue: 0xF6,
        }),
        contrast: Contrast::Normal,
        material,
        ..DesktopPreferences::default()
    }
}

fn snapshot_desktop() -> SettingsSnapshot {
    let modes_dp1 = vec![
        OutputMode {
            width: 2560,
            height: 1440,
            refresh_mhz: 144_000,
        },
        OutputMode {
            width: 2560,
            height: 1440,
            refresh_mhz: 60_000,
        },
        OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        },
    ];
    let preferences = base_preferences(Material::Frosted);
    SettingsSnapshot {
        revision: 1,
        input: InputStatus {
            configurable: true,
            touchpad: TouchpadStatus::default(),
            mouse: MouseStatus {
                configurable: true,
                device_names: vec!["Logitech G Pro Wireless".into()],
                capabilities: MouseCapabilities {
                    natural_scroll: true,
                    pointer_speed: true,
                },
                config: MouseConfig::default(),
            },
            keyboard: KeyboardConfig::default(),
            mappings: Vec::new(),
        },
        display: DisplayStatus {
            configurable: true,
            topology: DisplayTopology::Auto,
            outputs: vec![make_output(
                "DP-1",
                OutputKind::External,
                modes_dp1[0],
                modes_dp1,
                1.0,
                Point { x: 0, y: 0 },
            )],
            error: None,
            pending_revert_seconds: None,
        },
        material: Some(resolve_material(preferences.material, [])),
        preferences,
        idle: IdleSettings::default(),
        dock: DockSettings::default(),
        wallpaper: WallpaperSettings::default(),
    }
}

fn snapshot_multi_display() -> SettingsSnapshot {
    let mut snapshot = snapshot_desktop();
    let modes_hdmi = vec![
        OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        },
        OutputMode {
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
        },
    ];
    snapshot.display.outputs[0].geometry.scale = Scale(1.25);
    snapshot.display.outputs.push(make_output(
        "HDMI-A-1",
        OutputKind::External,
        modes_hdmi[0],
        modes_hdmi,
        1.0,
        Point { x: 2048, y: 0 },
    ));
    snapshot
}

fn snapshot_laptop() -> SettingsSnapshot {
    let modes_edp = vec![
        OutputMode {
            width: 2880,
            height: 1800,
            refresh_mhz: 120_000,
        },
        OutputMode {
            width: 2880,
            height: 1800,
            refresh_mhz: 60_000,
        },
        OutputMode {
            width: 1920,
            height: 1200,
            refresh_mhz: 60_000,
        },
    ];
    let mut preferences = base_preferences(Material::LiquidGlass);
    preferences.material_power.on_battery = Material::Frosted;
    let material = resolve_material(
        preferences.material,
        [MaterialCeiling {
            max: preferences.material_power.on_battery,
            reason: MaterialCeilingReason::OnBattery,
        }],
    );
    SettingsSnapshot {
        revision: 1,
        input: InputStatus {
            configurable: true,
            touchpad: TouchpadStatus {
                configurable: true,
                device_names: vec!["ELAN0676:00 Touchpad".into()],
                capabilities: TouchpadCapabilities {
                    natural_scroll: true,
                    tap_to_click: true,
                    tap_and_drag: true,
                    drag_lock: true,
                    disable_while_typing: true,
                    pointer_speed: true,
                    two_finger_scroll: true,
                    edge_scroll: true,
                },
                config: TouchpadConfig::default(),
            },
            mouse: MouseStatus {
                configurable: true,
                device_names: vec!["Bluetooth Travel Mouse".into()],
                capabilities: MouseCapabilities {
                    natural_scroll: true,
                    pointer_speed: true,
                },
                config: MouseConfig::default(),
            },
            keyboard: KeyboardConfig::default(),
            mappings: Vec::new(),
        },
        display: DisplayStatus {
            configurable: true,
            topology: DisplayTopology::Auto,
            outputs: vec![make_output(
                "eDP-1",
                OutputKind::Internal,
                modes_edp[0],
                modes_edp,
                1.75,
                Point { x: 0, y: 0 },
            )],
            error: None,
            pending_revert_seconds: None,
        },
        material: Some(material),
        preferences,
        idle: IdleSettings::default(),
        dock: DockSettings::default(),
        wallpaper: WallpaperSettings::default(),
    }
}

fn snapshot_minimal() -> SettingsSnapshot {
    let preferences = base_preferences(Material::Opaque);
    SettingsSnapshot {
        revision: 1,
        input: InputStatus {
            configurable: false,
            ..InputStatus::default()
        },
        display: DisplayStatus {
            configurable: false,
            topology: DisplayTopology::Auto,
            outputs: vec![make_output(
                "nested",
                OutputKind::External,
                OutputMode {
                    width: 1280,
                    height: 800,
                    refresh_mhz: 60_000,
                },
                Vec::new(),
                1.0,
                Point { x: 0, y: 0 },
            )],
            error: None,
            pending_revert_seconds: None,
        },
        material: Some(resolve_material(preferences.material, [])),
        preferences,
        idle: IdleSettings::default(),
        dock: DockSettings::default(),
        wallpaper: WallpaperSettings::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::dock::MinimizeAnimationStyle;
    use tessera_ipc::Client;

    fn temp_socket(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tessera-settings-mock-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("tessera.sock");
        (dir, sock)
    }

    fn connect_settings(socket: &Path) -> io::Result<Client> {
        Client::connect_scoped_with_timeout(
            socket,
            ConnectionCapabilities {
                query: true,
                session: true,
                ..ConnectionCapabilities::default()
            },
            LOCAL_SETTINGS_SCOPE,
            Duration::from_secs(2),
        )
    }

    #[test]
    fn live_mode_applies_transactions_increments_revision_and_broadcasts() {
        let (dir, sock) = temp_socket("live");
        let (_handler, _server) =
            MockSettingsHandler::start(&sock, Preset::MultiDisplay, FaultMode::Live).unwrap();

        let mut watcher = Client::connect_with_timeout(
            &sock,
            ConnectionCapabilities::QUERY,
            Duration::from_secs(2),
        )
        .unwrap();
        watcher
            .set_io_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        watcher.subscribe().unwrap();

        let mut client = connect_settings(&sock).unwrap();
        let initial = client.settings().unwrap();
        assert_eq!(initial.revision, 1);
        assert_eq!(initial.display.outputs.len(), 2);

        let receipt = client
            .apply_settings(
                Some(initial.revision),
                SettingsAction::SetDock {
                    settings: DockSettings {
                        minimize_animation: MinimizeAnimationStyle::Genie,
                    },
                },
            )
            .unwrap();
        assert_eq!(receipt.revision, 2);

        let event = watcher.next_event().unwrap();
        assert_eq!(event, Event::SettingsChanged { revision: 2 });

        let updated = client.settings().unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(
            updated.dock.minimize_animation,
            MinimizeAnimationStyle::Genie
        );

        // Stale revision must be rejected.
        let stale = client.apply_settings(
            Some(1),
            SettingsAction::SetDock {
                settings: DockSettings::default(),
            },
        );
        assert!(stale.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tentative_display_edit_sets_countdown_and_confirm_clears_it() {
        let (dir, sock) = temp_socket("tentative");
        let (_handler, _server) =
            MockSettingsHandler::start(&sock, Preset::MultiDisplay, FaultMode::Live).unwrap();
        let mut client = connect_settings(&sock).unwrap();
        let initial = client.settings().unwrap();

        let receipt = client
            .apply_settings(
                Some(initial.revision),
                SettingsAction::SetDisplay {
                    settings: DisplaySettings {
                        connector: "HDMI-A-1".into(),
                        role: Some(OutputRole::Extend),
                        mode: None,
                        scale: Some(1.5),
                        position: Some(Point { x: 2048, y: 0 }),
                        primary: false,
                        tentative: true,
                    },
                },
            )
            .unwrap();
        let previewing = client.settings().unwrap();
        assert!(previewing.display.pending_revert_seconds.is_some());
        assert_eq!(previewing.display.outputs[1].geometry.scale, Scale(1.5));

        client
            .apply_settings(Some(receipt.revision), SettingsAction::ConfirmDisplay)
            .unwrap();
        let confirmed = client.settings().unwrap();
        assert_eq!(confirmed.display.pending_revert_seconds, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_mode_refuses_settings_scope_and_keeps_query_working() {
        let (dir, sock) = temp_socket("readonly");
        let (_handler, _server) =
            MockSettingsHandler::start(&sock, Preset::Laptop, FaultMode::ReadOnly).unwrap();

        assert!(connect_settings(&sock).is_err());

        let mut query = Client::connect_with_timeout(
            &sock,
            ConnectionCapabilities::QUERY,
            Duration::from_secs(2),
        )
        .unwrap();
        let snapshot = query.settings().unwrap();
        assert!(snapshot.input.touchpad.configurable);
        assert_eq!(
            snapshot.material.and_then(|m| m.limited_by),
            Some(MaterialCeilingReason::OnBattery)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reject_locked_and_reject_stale_modes_return_authoritative_errors() {
        let (dir, sock) = temp_socket("locked");
        let (_handler, _server) =
            MockSettingsHandler::start(&sock, Preset::Desktop, FaultMode::RejectLocked).unwrap();
        let mut client = connect_settings(&sock).unwrap();
        let err = client
            .apply_settings(
                Some(1),
                SettingsAction::SetDock {
                    settings: DockSettings::default(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("locked"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);

        let (dir2, sock2) = temp_socket("stale");
        let (_handler2, _server2) =
            MockSettingsHandler::start(&sock2, Preset::Desktop, FaultMode::RejectStale).unwrap();
        let mut client2 = connect_settings(&sock2).unwrap();
        let err2 = client2
            .apply_settings(
                Some(1),
                SettingsAction::SetDock {
                    settings: DockSettings::default(),
                },
            )
            .unwrap_err();
        assert!(err2.to_string().contains("revision mismatch"), "{err2}");
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
