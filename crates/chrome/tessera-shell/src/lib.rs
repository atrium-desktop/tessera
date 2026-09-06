//! Compositor chrome for tessera, built on lens.
//!
//! The shell is split into a **core host** and pluggable **chrome components**.
//! The core ([`Shell`]) owns the lens context, the per-frame snapshot of
//! live toplevels, and the interaction sink ([`ChromeEvents`]); it knows
//! nothing about what the chrome looks like. Each piece of chrome — the
//! launcher, the overview — is a [`Chrome`] implementation registered with
//! [`Shell::add`], and renders itself each frame from the shared snapshot
//! and input. Larger components live in their own crates on top of the same
//! contract (the dock in `tessera-dock`, Prism in `tessera-prism`, the HUD in
//! `tessera-hud`, and the command panel in `tessera-command-panel`). Adding
//! or removing a chrome surface is a component change, not a core change.
//! [`persona`] owns the lightweight personalized-profile convention; its
//! optional `persona` feature adds shared still/VRM portrait and motion
//! handling without imposing that dependency set on every shell consumer.
//!
//! Input the compositor captures is fed here as a snapshot before being routed
//! to clients; component-emitted intents are drained by the main loop into
//! server window-management actions.

use std::os::raw::c_void;

use lens::Ui;

pub mod chrome;
pub mod system;
pub use chrome::{
    AgentFeedback, AppPicker, BatteryAlert, CapabilityPrompt, ConfirmPrompt,
    ControlledWindowGuard, Launcher, Overview, ScreenshotSelector, SecretPrompt, Toast,
    WindowSwitcher,
};
pub use system::{
    BatteryStatus, ChassisKind, DisplaySettings, DisplayStatus, NetworkState, ResourceProbe,
    detect_forked_status, detect_system_status, detect_system_status_lightweight,
};

/// Logical height of the HUD chips (the `tessera-hud` component).
/// Defined here, at the shell seam, so shell-resident chrome that must align
/// with the chips (the notification toast stack) can share the value without
/// depending on the component crate.
pub const HUD_HEIGHT: f32 = 32.0;
// The chrome contract (ADR-0021) lives in `tessera-chrome`; the shell hosts
// it. These re-exports keep existing `tessera_shell::` paths resolving while
// consumers migrate to depend on the contract crate directly.
pub use tessera_chrome::{
    AgentActivity, AgentInputKind, AppCatalog, AppMenu, AppPickParams, BackdropCover,
    BackdropLayer, BackdropLayerId, BackdropLayerSource, BackdropRegion, BackdropWash,
    BatteryAlertParams, CapabilityFamily, CapabilityGroup, CapabilityPickParams,
    CapabilityPickResult, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate,
    CompositionRequirements, ConfirmAnswer, ConfirmPickParams, ConfirmPickStyle, CursorShape,
    HUD_HEIGHT as CONTRACT_HUD_HEIGHT, IconSet, Input, InteractionDomainIntent, Language,
    LiquidGlassAdaptation, LiquidGlassFocus, LiquidGlassRegion, LivePreviewPresentation,
    Localizer, Message, MirrorMove, PickerMode, PinAction, PopupSide, PreviewCard, Reserved,
    ResourceStats, SecretPromptParams, SystemAction, SystemStatus, WindowAction,
    WindowSwitcherPresentation, backdrop_layer_id, backdrop_wash, ellipsize,
    liquid_glass_region_id, modal_scrim_backdrop, place_popup, place_popup_side, truncate,
};
use tessera_model::app::{BuiltInApplication, Entry};
use tessera_model::interaction_domain::InteractionDomainSnapshot;
use tessera_model::window::Window;
use tessera_model::workspace::WorkspaceSnapshot;

/// Whether one component participates in the current shell pass. Ordinary
/// modal overlays preserve persistent decorations; an exclusive presentation
/// temporarily owns the complete chrome band. A held screenshot freeze
/// outranks every other state: the frozen snapshot already contains the other
/// components (an open command panel included), so only the selector itself
/// may draw and receive input until it closes.
fn participates_in_shell_pass(
    component: &dyn Chrome,
    screenshot_freeze: bool,
    modal_active: bool,
    window_switcher_active: bool,
    exclusive_presentation_active: bool,
) -> bool {
    if screenshot_freeze {
        return component.screenshot_active();
    }
    (!window_switcher_active || component.window_switcher_active())
        && (!exclusive_presentation_active || component.exclusive_presentation_active())
        && (!modal_active || component.visible_during_modal())
}

/// Errors from the shell.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("shell create: {0:?}")]
    Create(#[source] lens::Error),
    #[error("shell render: {0:?}")]
    Render(#[source] lens::Error),
}

/// The core chrome host.
///
/// Owns a lens context bound to the compositor's flux device, the per-frame
/// window snapshot, the interaction sink, and a registry of [`Chrome`]
/// components. The host renders the chrome into the output canvas each frame by
/// running every registered component inside one `Ui::frame` envelope. It has
/// no built-in chrome of its own; the binary composes it from components.
pub struct Shell {
    ui: Ui,
    windows: Vec<Window>,
    /// Every mapped toplevel across all workspaces; pushed by the host
    /// alongside [`Self::windows`] and fanned out as
    /// [`ChromeUpdate::AllWindows`] for workspace-global components (the dock).
    all_windows: Vec<Window>,
    workspaces: WorkspaceSnapshot,
    i18n: Localizer,
    system_status: SystemStatus,
    /// The most recently published persistent-settings snapshot, seeded into
    /// every registered component (including ones added later) by
    /// [`Shell::add`] and fanned out as [`ChromeUpdate::Settings`].
    settings: Option<tessera_model::settings::SettingsSnapshot>,
    resource_stats: ResourceStats,
    interaction_domains: InteractionDomainSnapshot,
    /// The most recently pushed host application catalog, seeded into every
    /// registered component (including ones added later) by [`Shell::add`].
    catalog: AppCatalog,
    events: ChromeEvents,
    components: Vec<Box<dyn Chrome>>,
    /// Accessibility reduced-motion policy (ADR-0029), fanned out to every
    /// registered component (including ones added later) and to lens.
    reduced_motion: bool,
    /// Most recently reported device-pixel scale, fanned out to components as
    /// [`ChromeUpdate::Scale`].
    scale: f32,
    /// The resolved appearance every component paints from, seeded by
    /// [`Shell::add`] and fanned out as [`ChromeUpdate::Appearance`]. Starts
    /// dark; the compositor pushes the configured scheme after startup.
    design: tessera_design::Design,
    /// While the screenshot freeze holds the screen, only the selector
    /// itself renders; every other component is part of the frozen snapshot
    /// and must not draw (or advance its animations) on top of it.
    screenshot_freeze: bool,
}

impl Shell {
    /// Bind to the compositor's flux device. The host starts with no chrome
    /// registered; add components with [`Shell::add`].
    ///
    /// # Safety
    /// `device` must be a live `flux_device` (from `flux::Device::as_raw`) and
    /// outlive the `Shell`. The pointer crosses from the `flux` bindings' type
    /// to lens's distinct-but-ABI-identical `flux_device`.
    pub unsafe fn new(device: *mut c_void) -> Result<Shell, ShellError> {
        unsafe {
            let ui = Ui::with_device(device as *mut lens::sys::flux_device)
                .map_err(ShellError::Create)?;
            Ok(Shell {
                ui,
                windows: Vec::new(),
                all_windows: Vec::new(),
                workspaces: WorkspaceSnapshot {
                    outputs: Vec::new(),
                },
                i18n: Localizer::from_env(),
                system_status: SystemStatus::default(),
                settings: None,
                resource_stats: ResourceStats::default(),
                interaction_domains: tessera_model::interaction_domain::InteractionDomainModel::new()
                    .snapshot(),
                catalog: AppCatalog::default(),
                events: ChromeEvents::default(),
                components: Vec::new(),
                reduced_motion: false,
                scale: 1.0,
                design: tessera_design::Design::dark(),
                screenshot_freeze: false,
            })
        }
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, mut component: Box<dyn Chrome>) {
        component.update(ChromeUpdate::SystemStatus(&self.system_status));
        component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        component.update(ChromeUpdate::InteractionDomains(&self.interaction_domains));
        component.update(ChromeUpdate::ReducedMotion(self.reduced_motion));
        component.update(ChromeUpdate::Scale(self.scale));
        component.update(ChromeUpdate::Appearance(&self.design));
        component.update(ChromeUpdate::AppCatalog(&self.catalog));
        component.update(ChromeUpdate::Windows(&self.windows));
        component.update(ChromeUpdate::AllWindows(&self.all_windows));
        if let Some(settings) = &self.settings {
            component.update(ChromeUpdate::Settings(settings));
        }
        self.components.push(component);
    }

    fn broadcast_command(&mut self, command: ChromeCommand<'_>) {
        let events = &mut self.events;
        for component in &mut self.components {
            component.command(&command, events);
        }
    }

    /// Set the shell-wide reduced-motion policy (ADR-0029): every component
    /// transition and every lens eased value resolves in one frame when on.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        self.ui.set_reduced_motion(reduced);
        for component in &mut self.components {
            component.update(ChromeUpdate::ReducedMotion(reduced));
        }
    }

    /// Whether the reduced-motion policy is currently enabled.
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Set the device-pixel (HiDPI) scale for the chrome. Layout and input
    /// stay in logical pixels; lens scales the canvas transform on render so
    /// chrome rasterises crisply on a scaled output. The main loop reports the
    /// backend's output scale here each time it changes.
    pub fn set_scale(&mut self, scale: f32) {
        self.ui.set_scale(scale);
        if self.scale != scale {
            self.scale = scale;
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::Scale(scale));
            }
        }
    }

    /// Set the shell-wide desktop color scheme (`[appearance] color_scheme`).
    /// `System` resolves to the dark fallback inside
    /// [`tessera_design::Design::for_scheme`]; every component receives the
    /// resulting design snapshot through [`ChromeUpdate::Appearance`] when it
    /// actually changes.
    pub fn set_color_scheme(&mut self, scheme: tessera_model::settings::ColorScheme) {
        let design = tessera_design::Design::for_scheme(scheme);
        if self.design != design {
            self.design = design;
            // Keep lens's own widget defaults on the same tonal side as the
            // design: bare icons and any unstyled widget draw with the
            // context theme's foreground, which otherwise stays on the
            // creation-time dark token set even in the light appearance
            // (near-white glyphs on white glass).
            self.ui
                .set_theme(tessera_design::themes::application_base(&design));
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::Appearance(&self.design));
            }
        }
    }

    /// The design snapshot components currently paint from.
    pub fn design(&self) -> &tessera_design::Design {
        &self.design
    }

    /// Select chrome translations from a POSIX locale or BCP-47 language tag.
    /// Unsupported locales use the English fallback catalog. The shell starts
    /// with the process message locale (`LC_ALL` > `LC_MESSAGES` > `LANG`).
    pub fn set_locale(&mut self, locale: &str) {
        self.i18n = Localizer::new(locale);
    }

    /// Canonical locale tag of the active shell translation catalog.
    pub fn locale(&self) -> &'static str {
        self.i18n.locale()
    }

    /// Whether any component requested the session to quit this frame.
    pub fn should_quit(&self) -> bool {
        self.events.quit
    }

    /// Drain an immediate-lock request from the chrome (the command panel's
    /// Lock now row). The main loop feeds it to `IdleProcess::lock_now`.
    pub fn take_lock(&mut self) -> bool {
        std::mem::take(&mut self.events.lock)
    }

    /// Replace the host's snapshot of live toplevels. Called once per frame
    /// by the main loop with `server.windows()`.
    pub fn set_windows(&mut self, windows: Vec<Window>) {
        self.windows = windows;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::Windows(&self.windows));
        }
    }

    /// Replace the host's workspace-global snapshot of live toplevels — every
    /// mapped window across all workspaces, from `server.all_windows()`. Only
    /// components with a workspace-global strip (the dock) consume it; the
    /// overview, window switcher, and other chrome keep the visible-set
    /// [`Self::set_windows`] snapshot.
    pub fn set_all_windows(&mut self, windows: Vec<Window>) {
        self.all_windows = windows;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AllWindows(&self.all_windows));
        }
    }

    /// Replace the host's workspace snapshot. Called once per frame by the
    /// main loop with `server.workspace_snapshot()`.
    pub fn set_workspaces(&mut self, workspaces: WorkspaceSnapshot) {
        self.workspaces = workspaces;
    }

    /// Drain the surface id of the window a component asked to focus this
    /// frame, if any.
    pub fn take_clicked_window(&mut self) -> Option<tessera_model::window::WindowId> {
        self.events.clicked.take()
    }

    /// Drain ordered window actions emitted by application context menus.
    pub fn take_window_actions(&mut self) -> Vec<WindowAction> {
        std::mem::take(&mut self.events.window_actions)
    }

    /// Drain the mirror-guard move request of this frame, if any.
    pub fn take_mirror_move(&mut self) -> Option<MirrorMove> {
        self.events.mirror_move.take()
    }

    /// Drain the desktop entry the chrome asked to launch this frame, if any.
    /// The main loop hands it to `tessera-launch`.
    pub fn take_spawn(&mut self) -> Option<Entry> {
        self.events.spawn.take()
    }

    /// Drain the compositor-owned application requested this frame.
    pub fn take_open_builtin(&mut self) -> Option<BuiltInApplication> {
        self.events.open_builtin.take()
    }

    /// Present a compositor-owned application through its registered chrome
    /// component.
    pub fn open_builtin(&mut self, app: BuiltInApplication) {
        self.broadcast_command(ChromeCommand::OpenBuiltIn(app));
    }

    /// Replace the normalized system snapshot and notify interested shell
    /// applications and compact status surfaces.
    pub fn set_system_status(&mut self, status: SystemStatus) {
        self.system_status = status;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::SystemStatus(&self.system_status));
        }
    }

    /// Replace the persistent-settings snapshot and notify chrome hosting
    /// settings UI.
    pub fn set_settings(&mut self, snapshot: tessera_model::settings::SettingsSnapshot) {
        self.settings = Some(snapshot);
        if let Some(settings) = self.settings.as_ref() {
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::Settings(settings));
            }
        }
    }

    /// Replace the host resource-utilisation sample and notify interested
    /// status surfaces.
    pub fn set_resource_stats(&mut self, stats: ResourceStats) {
        self.resource_stats = stats;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        }
    }

    /// Replace the Interaction Domain authority snapshot and notify the overview and
    /// Agent Workspaces before their next frame.
    pub fn set_interaction_domains(&mut self, snapshot: InteractionDomainSnapshot) {
        self.interaction_domains = snapshot;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::InteractionDomains(&self.interaction_domains));
        }
    }

    /// Publish one successfully applied Agent input operation to interested
    /// trusted chrome components.
    pub fn report_agent_activity(&mut self, activity: AgentActivity) {
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AgentActivity(&activity));
        }
    }

    /// Drain ordered system mutations requested by trusted shell UI.
    pub fn take_system_actions(&mut self) -> Vec<SystemAction> {
        std::mem::take(&mut self.events.system_actions)
    }

    /// Drain persistent-settings mutations requested by compositor-owned UI.
    pub fn take_settings_actions(
        &mut self,
    ) -> Vec<(Option<u64>, tessera_model::settings::SettingsAction)> {
        std::mem::take(&mut self.events.settings_actions)
    }

    /// Drain ordered pin/unpin mutations requested this frame.
    pub fn take_dock_pin_actions(&mut self) -> Vec<PinAction> {
        std::mem::take(&mut self.events.dock_pin_actions)
    }

    /// Drain the pinned order committed by a dock tile drag this frame, if
    /// any. The main loop persists it to `DockStateStore`.
    pub fn take_dock_reorder(&mut self) -> Option<Vec<String>> {
        self.events.dock_reorder.take()
    }

    /// Drain the screen edge the dock was dragged to this frame, if any. The
    /// main loop persists it to `DockStateStore`.
    pub fn take_dock_position(&mut self) -> Option<tessera_model::dock::DockPosition> {
        self.events.dock_position.take()
    }

    /// Drain trusted Interaction Domain-management intents in UI order.
    pub fn take_interaction_domain_intents(&mut self) -> Vec<InteractionDomainIntent> {
        std::mem::take(&mut self.events.interaction_domain_intents)
    }

    /// Drain the workspace id the chrome asked to switch to this frame, if
    /// any (the workspace bar's clicked tile). The main loop forwards it to
    /// `Server::switch_workspace_to`.
    pub fn take_switch_workspace(&mut self) -> Option<tessera_model::workspace::WorkspaceId> {
        self.events.switch_workspace.take()
    }

    /// Drain a notification dismissal requested by the toast stack.
    pub fn take_dismissed_notification(&mut self) -> Option<u64> {
        self.events.dismissed_notification.take()
    }

    /// Whether the chrome asked to toggle the launcher this frame (the dock's
    /// Launchpad tile). The main loop calls [`Shell::toggle`] when set.
    pub fn take_toggle_launcher(&mut self) -> bool {
        std::mem::take(&mut self.events.toggle_launcher)
    }

    /// Toggle overview mode on the component that owns it (M9). Mirrors
    /// [`Shell::toggle`]: fanned out to every component; static components
    /// ignore it.
    pub fn toggle_overview(&mut self) {
        let opening = !self.overview_active();
        if opening {
            self.broadcast_command(ChromeCommand::CloseLauncher);
            self.broadcast_command(ChromeCommand::ClosePrism);
            self.broadcast_command(ChromeCommand::CloseCommandPanel);
        }
        self.broadcast_command(ChromeCommand::ToggleOverview);
    }

    /// Whether overview mode is currently open. The main loop swaps the
    /// desktop scene for the overview thumbnail grid while this holds or
    /// the reveal animation is still running (see [`Shell::overview_progress`]).
    pub fn overview_active(&self) -> bool {
        self.components.iter().any(|c| c.overview_active())
    }

    /// The overview's reveal progress (0 = hidden, 1 = fully open), the
    /// maximum across components. Drives the thumbnail pass fly-in.
    pub fn overview_progress(&self) -> f32 {
        self.components
            .iter()
            .map(|c| c.overview_progress())
            .fold(0.0_f32, f32::max)
    }

    /// Toggle the command panel on the component that owns it
    /// (ADR-0080). Mirrors [`Shell::toggle_overview`]: fanned out to every
    /// component; static components ignore it.
    pub fn toggle_command_panel(&mut self) {
        let opening = !self.command_panel_active();
        if opening {
            self.broadcast_command(ChromeCommand::CloseLauncher);
            self.broadcast_command(ChromeCommand::ClosePrism);
            self.broadcast_command(ChromeCommand::CloseOverview);
        }
        self.broadcast_command(ChromeCommand::ToggleCommandPanel);
    }

    /// Whether the command panel is currently open.
    pub fn command_panel_active(&self) -> bool {
        self.components.iter().any(|c| c.command_panel_active())
    }

    /// Open the compositor-owned Super+Tab preview strip.
    pub fn start_window_switcher(&mut self) {
        self.broadcast_command(ChromeCommand::StartWindowSwitcher);
    }

    /// Advance the switcher once and return the exact layout shared by the
    /// compositor's live-preview pass and shell chrome.
    pub fn prepare_window_switcher(
        &mut self,
        input: &Input,
        display: tessera_model::Rect,
        windows: &[Window],
        order: &[tessera_model::window::WindowId],
        selected: Option<tessera_model::window::WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        self.components.iter_mut().find_map(|component| {
            component.prepare_window_switcher(input, display, windows, order, selected)
        })
    }

    /// Collect compositor-rendered live-preview popovers contributed by
    /// ordinary chrome. A vector keeps the contract composable even though the
    /// Dock is currently the only producer.
    pub fn live_preview_presentations(&self) -> Vec<LivePreviewPresentation> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .filter_map(|component| component.live_preview_presentation())
            .collect()
    }

    /// Close the preview strip after the held Super modifier is released.
    pub fn finish_window_switcher(&mut self) {
        self.broadcast_command(ChromeCommand::FinishWindowSwitcher);
    }

    /// Whether the Super+Tab preview strip is active.
    pub fn window_switcher_active(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.window_switcher_active())
    }

    fn exclusive_presentation_active(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.exclusive_presentation_active())
    }

    /// Window id the overview asked to focus this frame, if any.
    pub fn take_overview_pick(&mut self) -> Option<tessera_model::window::WindowId> {
        self.events.overview_pick.take()
    }

    /// Window clicked in the held-modifier switcher, if any.
    pub fn take_window_switcher_pick(&mut self) -> Option<tessera_model::window::WindowId> {
        self.events.window_switcher_pick.take()
    }

    /// Whether a click-away dismissed the window switcher this frame.
    pub fn take_window_switcher_cancel(&mut self) -> bool {
        std::mem::take(&mut self.events.window_switcher_cancel)
    }

    /// Workspace id the overview's rail asked to switch to, if any.
    pub fn take_overview_switch(&mut self) -> Option<tessera_model::workspace::WorkspaceId> {
        self.events.overview_switch.take()
    }

    /// Open the screenshot region selector. No-op if no selector component is
    /// registered.
    pub fn start_screenshot(&mut self) {
        self.broadcast_command(ChromeCommand::OpenBuiltIn(
            tessera_model::app::BuiltInApplication::ScreenshotSelector,
        ));
    }

    /// Open an interactive picker session for a portal IPC request
    /// (ADR-0054). No-op if no picker component is registered.
    pub fn start_pick(&mut self, mode: PickerMode) {
        self.broadcast_command(ChromeCommand::StartPick(mode));
    }

    /// Force-close any IPC picker session (requester gone); the Print-key
    /// flow is unaffected.
    pub fn cancel_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelPick);
    }

    /// Region the screenshot selector asked to capture this frame, if any.
    pub fn take_screenshot_region(&mut self) -> Option<tessera_model::Rect> {
        self.events.screenshot_region.take()
    }

    /// Point the pixel picker was clicked at this frame, if any (ADR-0054).
    pub fn take_picked_point(&mut self) -> Option<tessera_model::Point> {
        self.events.picked_point.take()
    }

    /// Window id the window picker was clicked on this frame, if any
    /// (ADR-0054).
    pub fn take_picked_window(&mut self) -> Option<tessera_model::window::WindowId> {
        self.events.picked_window.take()
    }

    /// Whether the window-picker user chose the whole output this frame.
    pub fn take_pick_output(&mut self) -> bool {
        std::mem::take(&mut self.events.pick_output)
    }

    /// Drain the connector the output picker was clicked on this frame, if
    /// any (version 29, ADR-0128).
    pub fn take_picked_output(&mut self) -> Option<String> {
        self.events.picked_output.take()
    }

    /// Whether an IPC picker session was dismissed without a pick this frame.
    pub fn take_pick_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.events.pick_cancelled)
    }

    /// Open the user-consent application picker for a `PickApp` IPC request.
    /// No-op if no app-picker component is registered.
    pub fn start_app_pick(&mut self, params: AppPickParams) {
        self.broadcast_command(ChromeCommand::StartAppPick(&params));
    }

    /// Force-close the app picker (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_app_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelAppPick);
    }

    /// Whether the app picker is currently open.
    pub fn app_pick_active(&self) -> bool {
        self.components.iter().any(|c| c.app_pick_active())
    }

    /// The desktop file id the app picker confirmed this frame, if any.
    pub fn take_app_pick_confirmed(&mut self) -> Option<String> {
        self.events.app_pick_confirmed.take()
    }

    /// Whether the app picker was dismissed without a pick this frame.
    pub fn take_app_pick_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.events.app_pick_cancelled)
    }

    /// Open the masked secret prompt for a `PromptSecret` IPC request.
    /// No-op if no secret-prompt component is registered.
    pub fn start_secret_prompt(&mut self, params: SecretPromptParams) {
        self.broadcast_command(ChromeCommand::StartSecretPrompt(&params));
    }

    /// Force-close the secret prompt (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_secret_prompt(&mut self) {
        self.broadcast_command(ChromeCommand::CancelSecretPrompt);
    }

    /// Whether the secret prompt is currently open.
    pub fn secret_prompt_active(&self) -> bool {
        self.components.iter().any(|c| c.secret_prompt_active())
    }

    /// The secret value the prompt confirmed this frame, if any.
    pub fn take_secret_prompt_confirmed(&mut self) -> Option<String> {
        self.events.secret_prompt_confirmed.take()
    }

    /// Whether the secret prompt was dismissed without a secret this frame.
    pub fn take_secret_prompt_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.events.secret_prompt_cancelled)
    }

    /// Open the yes/no confirmation dialog for a `PickConfirm` IPC request.
    /// No-op if no confirmation component is registered.
    pub fn start_confirm_pick(&mut self, params: ConfirmPickParams) {
        self.broadcast_command(ChromeCommand::StartConfirmPick(&params));
    }

    /// Force-close the confirmation dialog (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_confirm_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelConfirmPick);
    }

    /// Whether the confirmation dialog is currently open.
    pub fn confirm_pick_active(&self) -> bool {
        self.components.iter().any(|c| c.confirm_pick_active())
    }

    /// The answer the user gave at the confirmation dialog this frame, if
    /// any.
    pub fn take_confirm_pick_answered(&mut self) -> Option<ConfirmAnswer> {
        self.events.confirm_pick_answered.take()
    }

    /// Open the capability-borrowing checklist for a `PairAgent` IPC
    /// request. No-op if no capability-prompt component is registered.
    pub fn start_capability_pick(&mut self, params: CapabilityPickParams) {
        self.broadcast_command(ChromeCommand::StartCapabilityPick(&params));
    }

    /// Force-close the capability checklist (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_capability_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelCapabilityPick);
    }

    /// Whether the capability checklist is currently open.
    pub fn capability_pick_active(&self) -> bool {
        self.components.iter().any(|c| c.capability_pick_active())
    }

    /// The checklist answer the user gave this frame, if any (`approved`
    /// carries the checked group keys; `None` = denied).
    pub fn take_capability_pick_answered(&mut self) -> Option<CapabilityPickResult> {
        self.events.capability_pick_answered.take()
    }

    /// Open or update the low-battery alert. Compositor-owned: dismissal
    /// produces no answer event. No-op if no battery-alert component is
    /// registered.
    pub fn start_battery_alert(&mut self, params: BatteryAlertParams) {
        self.broadcast_command(ChromeCommand::StartBatteryAlert(&params));
    }

    /// Force-close the low-battery alert.
    pub fn cancel_battery_alert(&mut self) {
        self.broadcast_command(ChromeCommand::CancelBatteryAlert);
    }

    /// Force every active high-priority modal overlay to its safe-negative
    /// exit: the compositor panic chord (`Ctrl+Alt+Escape`). Components
    /// answer through their normal cancellation events, so waiting IPC
    /// requests resolve exactly as if the user had pressed `Escape`.
    pub fn dismiss_modal_overlays(&mut self) {
        self.broadcast_command(ChromeCommand::DismissModal);
    }

    /// Whether the low-battery alert is currently open.
    pub fn battery_alert_active(&self) -> bool {
        self.components.iter().any(|c| c.battery_alert_active())
    }

    /// Whether the screenshot selector is currently active.
    pub fn screenshot_active(&self) -> bool {
        self.components.iter().any(|c| c.screenshot_active())
    }

    /// Hold or release the screenshot freeze. While held, [`Shell::render`]
    /// draws only the screenshot selector; every other component is part of
    /// the frozen snapshot the compositor presents underneath.
    pub fn set_screenshot_freeze(&mut self, frozen: bool) {
        self.screenshot_freeze = frozen;
    }

    /// Whether any registered component currently captures keyboard input
    /// (e.g. an open launcher). The main loop checks this to decide whether to
    /// route key events to [`Shell::key_char`] or forward them to the focused
    /// client.
    pub fn captures_keyboard(&self) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components.iter().any(|component| {
            participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) && component.captures_keyboard()
        })
    }

    /// Whether compositor chrome owns pointer input at `(x, y)`. Components
    /// use the same window/workspace snapshot they render, so routing and
    /// visuals agree for the frame.
    pub fn captures_pointer_at(&self, x: f32, y: f32, display: (f32, f32)) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .any(|c| c.captures_pointer(x, y, display, &self.windows, &self.workspaces))
    }

    /// Cursor requested by the topmost chrome component at `(x, y)`, or
    /// `None` when the pointer belongs to a client. Components are visited in
    /// reverse registration order because that is their visual stacking order.
    pub fn cursor_shape_at(&self, x: f32, y: f32, display: (f32, f32)) -> Option<CursorShape> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .rev()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .find(|component| {
                component.captures_pointer(x, y, display, &self.windows, &self.workspaces)
            })
            .and_then(|component| {
                component.cursor_shape_at(x, y, display, &self.windows, &self.workspaces)
            })
    }

    /// Replace the host application catalog and push it to every registered
    /// component. The shell keeps a copy to seed components added later.
    pub fn set_app_catalog(&mut self, catalog: AppCatalog) {
        self.catalog = catalog;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AppCatalog(&self.catalog));
        }
    }

    /// Feed one resolved key event to every registered component. Components
    /// with keyboard-owned state, such as the launcher or an application
    /// context menu, override [`Chrome::key_char`]; others no-op.
    pub fn key_char(&mut self, kc: tessera_model::input::KeyChar) {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let freeze = self.screenshot_freeze;
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            if participates_in_shell_pass(
                component.as_ref(),
                freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) {
                component.key_char(&kc, events);
            }
        }
    }

    /// Fire the global application-launcher hotkey. Opening the launcher
    /// closes other immersive surfaces (Prism, Command Panel, Overview) so
    /// only one modal surface captures input and owns the screen.
    pub fn toggle(&mut self) {
        let opening = !self
            .components
            .iter()
            .any(|component| component.launcher_active());
        if opening {
            self.broadcast_command(ChromeCommand::ClosePrism);
            self.broadcast_command(ChromeCommand::CloseCommandPanel);
            self.broadcast_command(ChromeCommand::CloseOverview);
        }
        self.broadcast_command(ChromeCommand::ToggleLauncher);
    }

    /// Fire the global Prism hotkey. Opening Prism closes other modal
    /// surfaces so only one catalog surface owns keyboard input.
    pub fn toggle_prism(&mut self) {
        let opening = !self
            .components
            .iter()
            .any(|component| component.prism_active());
        if opening {
            self.broadcast_command(ChromeCommand::CloseLauncher);
            self.broadcast_command(ChromeCommand::CloseCommandPanel);
            self.broadcast_command(ChromeCommand::CloseOverview);
        }
        self.broadcast_command(ChromeCommand::TogglePrism);
    }

    /// The union of every component's [`Chrome::reserved`] edges — the space
    /// tiled windows should avoid. Summed per edge.
    pub fn reserved(&self) -> Reserved {
        let mut r = Reserved::default();
        for c in &self.components {
            let c = c.reserved();
            r.top += c.top;
            r.bottom += c.bottom;
            r.left += c.left;
            r.right += c.right;
        }
        r
    }

    /// Whether any registered component has a multi-frame animation in flight.
    /// The main loop consults this to decide whether to keep ticking frames
    /// (advance the animation) or block on the host event queue for the next
    /// wakeup. Also folds in lens's own eased-value state so hover/active
    /// fades on lens widgets (buttons, etc.) settle correctly.
    pub fn anim_pending(&self) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        if self
            .components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .any(|c| c.anim_pending())
        {
            return true;
        }
        self.ui.anim_pending()
    }

    /// The output-logical rectangle every in-flight chrome animation can
    /// touch this frame, or `None` when any animated component (or lens's
    /// own eased state) cannot state its footprint. The compositor uses this
    /// to turn animation-driven frames into partial repaints instead of
    /// full-output composites; `None` preserves the conservative
    /// full-repaint behaviour.
    ///
    /// This only describes *chrome* animation damage. Client windows, the
    /// software cursor, and the backdrop-effect source have their own damage
    /// paths and are unioned by the host separately.
    pub fn anim_damage_region(&self, display: (f32, f32)) -> Option<tessera_model::Rect> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut region: Option<tessera_model::Rect> = None;
        for component in self
            .components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .filter(|c| c.anim_pending())
        {
            let rect = component.damage_region(&self.windows, display)?;
            region = Some(match region {
                Some(existing) => existing.union(rect),
                None => rect,
            });
        }
        // Lens's internal eased values (hover/active fades on buttons, the
        // agent-feedback cursor sprite) have no exposed footprint; while any
        // is still settling the shell cannot narrow the repaint.
        if self.ui.anim_pending() {
            return None;
        }
        region
    }

    /// Whether any component that would participate in the next chrome pass
    /// has visible output. Direct scanout is safe only when this is false.
    pub fn requires_composition(&self) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .any(|component| component.requires_composition())
    }

    /// Return the precise compositor-owned work that would affect the next
    /// output frame.  Direct-scanout policy consumes this instead of treating
    /// registered components, dormant animations, or a non-zero blur setting
    /// as global blockers.
    pub fn composition_requirements(&self, display: (f32, f32)) -> CompositionRequirements {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut requirements = CompositionRequirements::default();
        for component in self.components.iter().filter(|component| {
            participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            )
        }) {
            let visible = component.requires_composition();
            requirements.visible_pixels |= visible;
            // A blur request belonging to visually empty chrome is not live:
            // there are no output pixels that could consume the backdrop.
            let explicit_backdrop = component
                .backdrop_layers(display, &self.windows, &self.workspaces)
                .into_iter()
                .any(|layer| !layer.frost.is_empty() || !layer.glass.is_empty());
            requirements.live_backdrop_effect |=
                visible && (component.backdrop_blur_sigma() > 0.0 || explicit_backdrop);
        }
        requirements
    }

    /// Strongest backdrop blur requested by any registered component, in
    /// logical pixels. The executable converts it to physical pixels before
    /// invoking flux's realtime multi-resolution filter.
    pub fn backdrop_blur_sigma(&self) -> f32 {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .map(|component| component.backdrop_blur_sigma())
            .fold(0.0_f32, f32::max)
    }

    /// Run the backdrop prepass for the components eligible to render this
    /// frame. Call once after input is built and before querying blur regions.
    pub fn prepare_backdrop(&mut self, input: &Input) {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let freeze = self.screenshot_freeze;
        for component in &mut self.components {
            if participates_in_shell_pass(
                component.as_ref(),
                freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) {
                component.prepare_backdrop(input, &self.windows, &self.workspaces);
            }
        }
    }

    /// Aggregate every component's resting minimize-flight targets (the
    /// dock's tile icons), in output coordinates. The main loop pushes these
    /// into the server each frame so even a client-initiated minimize
    /// animates toward the real icon instead of a hardcoded point.
    pub fn minimize_targets(
        &self,
        display: (f32, f32),
    ) -> Vec<(tessera_model::window::WindowId, tessera_model::Rect)> {
        let mut targets = Vec::new();
        for component in &self.components {
            component.minimize_targets(display, &mut targets);
        }
        targets
    }

    /// Glass regions contributed by components that will render this frame.
    /// Ordinary chrome is excluded while a modal is active, matching the
    /// render path below.
    pub fn backdrop_regions(&self, display: (f32, f32)) -> Vec<BackdropRegion> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut regions = Vec::new();
        for component in &self.components {
            if participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) && component.backdrop_blur_sigma() > 0.0
            {
                regions.extend(component.backdrop_regions(
                    display,
                    &self.windows,
                    &self.workspaces,
                ));
            }
        }
        regions
    }

    /// Analytic liquid-glass bodies contributed by components that will
    /// render this frame. Visibility filtering mirrors [`Self::backdrop_regions`].
    pub fn liquid_glass_regions(&self, display: (f32, f32)) -> Vec<LiquidGlassRegion> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut regions = Vec::new();
        for component in &self.components {
            if participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) && component.backdrop_blur_sigma() > 0.0
            {
                regions.extend(component.liquid_glass_regions(
                    display,
                    &self.windows,
                    &self.workspaces,
                ));
            }
        }
        regions
    }

    /// Complete declarative backdrop DAG for this frame.
    ///
    /// Legacy component requests are fused into one root layer. Explicit
    /// layers remain separate so dependencies can accumulate resolved
    /// offscreen results without teaching any material about nesting.
    pub fn backdrop_layers(&self, display: (f32, f32)) -> Vec<BackdropLayer> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut root = BackdropLayer::new(BackdropLayerId::ROOT, BackdropLayerSource::Scene, 0.0);
        let mut explicit = Vec::new();
        for component in &self.components {
            if !participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) {
                continue;
            }
            let sigma = component.backdrop_blur_sigma().max(0.0);
            if sigma > 0.0 {
                root.blur_sigma = root.blur_sigma.max(sigma);
                root.frost.extend(component.backdrop_regions(
                    display,
                    &self.windows,
                    &self.workspaces,
                ));
                root.glass.extend(component.liquid_glass_regions(
                    display,
                    &self.windows,
                    &self.workspaces,
                ));
            }
            explicit.extend(component.backdrop_layers(display, &self.windows, &self.workspaces));
        }
        let root_active =
            root.blur_sigma > 0.0 && (!root.frost.is_empty() || !root.glass.is_empty());
        let mut layers = Vec::with_capacity(explicit.len() + usize::from(root_active));
        if root_active {
            layers.push(root);
        }
        // Unlike the compatibility API, an explicit node with sigma zero is
        // still meaningful: it can resolve an unblurred material or act as a
        // graph boundary sampled by a later node.
        layers.extend(
            explicit
                .into_iter()
                .filter(|layer| !layer.frost.is_empty() || !layer.glass.is_empty()),
        );
        layers
    }

    /// Run every registered component and render the chrome into `canvas`,
    /// using `input` for interaction.
    ///
    /// # Safety
    /// `canvas` must be a live `flux_canvas` (from `flux::Canvas::as_raw`)
    /// currently inside a `begin`/`end` recording pair on the active frame.
    pub unsafe fn render(&mut self, canvas: *mut c_void, input: &Input) -> Result<(), ShellError> {
        unsafe {
            let windows = &self.windows;
            let workspaces = &self.workspaces;
            let i18n = &self.i18n;
            let events = &mut self.events;
            let components = &mut self.components;
            let freeze = self.screenshot_freeze;
            let modal_active = components.iter().any(|component| component.modal_active());
            let window_switcher_active = components
                .iter()
                .any(|component| component.window_switcher_active());
            let exclusive_presentation_active = components
                .iter()
                .any(|component| component.exclusive_presentation_active());
            let modal_reserved = components
                .iter()
                .filter(|component| component.persistent_decoration())
                .filter(|component| {
                    participates_in_shell_pass(
                        component.as_ref(),
                        freeze,
                        modal_active,
                        window_switcher_active,
                        exclusive_presentation_active,
                    )
                })
                .fold(Reserved::default(), |mut total, component| {
                    let edge = component.reserved();
                    total.top += edge.top;
                    total.bottom += edge.bottom;
                    total.left += edge.left;
                    total.right += edge.right;
                    total
                });
            // Immersive modal surfaces (Launcher, Command Panel, Overview) own the whole output:
            // persistent decorations (HUD status chips) floating inside the blurred field hide.
            let immersive_active = components.iter().any(|component| {
                component.launcher_active()
                    || component.command_panel_active()
                    || component.overview_active()
            });
            for component in components.iter_mut() {
                component.update(ChromeUpdate::ModalReserved(modal_reserved));
                component.update(ChromeUpdate::LauncherActive(immersive_active));
            }
            self.ui.frame(input, |f| {
                for component in components.iter_mut() {
                    if participates_in_shell_pass(
                        component.as_ref(),
                        freeze,
                        modal_active,
                        window_switcher_active,
                        exclusive_presentation_active,
                    ) {
                        component.render(f, input, windows, workspaces, i18n, events);
                    }
                }
            });
            self.ui
                .render(canvas as *mut lens::sys::flux_canvas)
                .map_err(ShellError::Render)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OrdinaryChrome;

    impl Chrome for OrdinaryChrome {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }
    }

    struct PersistentDecoration;

    impl Chrome for PersistentDecoration {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn persistent_decoration(&self) -> bool {
            true
        }
    }

    struct ExclusivePresentation;

    impl Chrome for ExclusivePresentation {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn modal_active(&self) -> bool {
            true
        }

        fn visible_during_modal(&self) -> bool {
            true
        }

        fn exclusive_presentation_active(&self) -> bool {
            true
        }
    }

    struct SwitcherPresentation;

    impl Chrome for SwitcherPresentation {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn visible_during_modal(&self) -> bool {
            true
        }

        fn window_switcher_active(&self) -> bool {
            true
        }
    }

    struct ScreenshotSessionChrome;

    impl Chrome for ScreenshotSessionChrome {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn screenshot_active(&self) -> bool {
            true
        }
    }

    #[test]
    fn ordinary_overlays_preserve_decorations_but_exclusive_presentations_do_not() {
        let ordinary = OrdinaryChrome;
        let decoration = PersistentDecoration;
        let exclusive = ExclusivePresentation;
        let switcher = SwitcherPresentation;

        assert!(!participates_in_shell_pass(
            &ordinary, false, true, false, false
        ));
        assert!(participates_in_shell_pass(
            &decoration,
            false,
            true,
            false,
            false
        ));
        assert!(!participates_in_shell_pass(
            &decoration,
            false,
            true,
            false,
            true
        ));
        assert!(participates_in_shell_pass(
            &exclusive, false, true, false, true
        ));
        assert!(!participates_in_shell_pass(
            &exclusive, false, true, true, true
        ));
        assert!(participates_in_shell_pass(
            &switcher, false, true, true, true
        ));
    }

    #[test]
    fn screenshot_freeze_outranks_modal_and_exclusive_presentations() {
        let selector = ScreenshotSessionChrome;
        let decoration = PersistentDecoration;
        let exclusive = ExclusivePresentation;

        // While the freeze holds, only the selector participates — even when
        // an exclusive presentation such as the command panel is still open
        // underneath (its pixels are already part of the frozen snapshot).
        assert!(participates_in_shell_pass(
            &selector, true, true, false, true
        ));
        assert!(!participates_in_shell_pass(
            &exclusive, true, true, false, true
        ));
        assert!(!participates_in_shell_pass(
            &decoration,
            true,
            true,
            false,
            true
        ));
    }

    #[test]
    fn reserved_inset_shrinks_and_clamps() {
        let r = Reserved {
            top: 10,
            bottom: 76,
            left: 4,
            right: 0,
        };
        let out = r.inset(tessera_model::Rect::new(0, 0, 1000, 800));
        assert_eq!(out.origin, tessera_model::Point { x: 4, y: 10 });
        assert_eq!(out.size, tessera_model::Size { w: 996, h: 714 }); // 800-10-76
    }

    #[test]
    fn reserved_inset_clamps_to_non_negative() {
        let r = Reserved {
            top: 0,
            bottom: 2000,
            left: 0,
            right: 0,
        };
        let out = r.inset(tessera_model::Rect::new(0, 0, 100, 100));
        assert_eq!(out.size, tessera_model::Size { w: 100, h: 0 });
    }
}
