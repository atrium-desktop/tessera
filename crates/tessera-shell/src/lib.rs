//! Desktop shell host and built-in UI components.
//! Feature modules share one component protocol and own their presentation state.

use std::os::raw::c_void;

pub use crate::component::{
    AgentActivity, AppCatalog, AppPickParams, BackdropLayer, BackdropLayerId, BackdropLayerSource,
    BackdropRegion, BatteryAlertParams, CapabilityPickParams, CapabilityPickResult, Chrome,
    ChromeCommand, ChromeEvents, ChromeUpdate, CompositionRequirements, ConfirmAnswer,
    ConfirmPickParams, CursorShape, Input, InteractionDomainIntent, LiquidGlassRegion,
    LivePreviewPresentation, Localizer, MirrorMove, PickerMode, PinAction, Reserved, ResourceStats,
    SecretPromptParams, SystemAction, SystemStatus, WindowAction, WindowSwitcherPresentation,
};
#[cfg(test)]
use lens::Frame;
use lens::Ui;

pub mod chrome;

pub use chrome::{
    AgentFeedback, AppPicker, BatteryAlert, CapabilityPrompt, ConfirmPrompt, ControlledWindowGuard,
    Overview, ScreenshotSelector, SecretPrompt, Toast, WindowSwitcher,
};

pub mod accessibility;

/// Logical height of the HUD chips (the `tessera-hud` component).
/// Defined here, at the shell seam, so shell-resident chrome that must align
/// with the chips (the notification toast stack) can share the value without
/// depending on the component crate.
pub const HUD_HEIGHT: f32 = 32.0;
use tessera_authority::interaction_domain::InteractionDomainSnapshot;
use tessera_desktop::app::BuiltInApplication;
use tessera_desktop::app::Entry;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;

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
    /// The server-resolved output space use (ADR-0183), fanned out as
    /// [`ChromeUpdate::SpaceUse`]. Chrome consumes this verdict instead of
    /// inferring presentation ownership from the window snapshot.
    space_use: tessera_desktop::window::SpaceUse,
    workspaces: WorkspaceSnapshot,
    i18n: Localizer,
    system_status: SystemStatus,
    /// The most recently published persistent-settings snapshot, seeded into
    /// every registered component (including ones added later) by
    /// [`Shell::add`] and fanned out as [`ChromeUpdate::Settings`].
    settings: Option<tessera_desktop::settings::SettingsSnapshot>,
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
    /// The scene `prepare_frame` built this frame, awaiting [`Shell::submit`]
    /// at draw time (ADR-0203).
    ///
    /// Building the tree and drawing it are two phases of one frame. Damage
    /// assessment must observe the tree the renderer is about to draw — the
    /// lens damage query is only meaningful for a frame already built — but it
    /// cannot run between a canvas pass's begin and its draws. Holding the
    /// built scene here lets `prepare_frame` (before damage assessment) do the
    /// building and `submit` (inside the pass) do only the drawing.
    pending_scene: Option<lens::Snapshot>,
    /// Whether any component, layout, or model state changed requiring
    /// a new UI scene build (ADR-0210, INV-ARCH-103).
    dirty: bool,
    /// Last sampled input cursor coordinates to detect pointer motion.
    last_input_cursor: (f32, f32),
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
                space_use: tessera_desktop::window::SpaceUse::default(),
                workspaces: WorkspaceSnapshot {
                    outputs: Vec::new(),
                },
                i18n: Localizer::from_env(),
                system_status: SystemStatus::default(),
                settings: None,
                resource_stats: ResourceStats::default(),
                interaction_domains:
                    tessera_authority::interaction_domain::InteractionDomainModel::new().snapshot(),
                catalog: AppCatalog::default(),
                events: ChromeEvents::default(),
                components: Vec::new(),
                reduced_motion: false,
                scale: 1.0,
                design: tessera_design::Design::dark(),
                screenshot_freeze: false,
                pending_scene: None,
                dirty: true,
                last_input_cursor: (-1.0, -1.0),
            })
        }
    }

    /// Create a headless shell without a GPU device, suitable for unit tests and headless layout.
    pub fn headless() -> Result<Shell, ShellError> {
        let ui = Ui::headless().map_err(ShellError::Create)?;
        Ok(Shell {
            ui,
            windows: Vec::new(),
            all_windows: Vec::new(),
            space_use: tessera_desktop::window::SpaceUse::default(),
            workspaces: WorkspaceSnapshot {
                outputs: Vec::new(),
            },
            i18n: Localizer::from_env(),
            system_status: SystemStatus::default(),
            settings: None,
            resource_stats: ResourceStats::default(),
            interaction_domains:
                tessera_authority::interaction_domain::InteractionDomainModel::new().snapshot(),
            catalog: AppCatalog::default(),
            events: ChromeEvents::default(),
            components: Vec::new(),
            reduced_motion: false,
            scale: 1.0,
            design: tessera_design::Design::dark(),
            screenshot_freeze: false,
            pending_scene: None,
            dirty: true,
            last_input_cursor: (-1.0, -1.0),
        })
    }

    /// Execute one UI layout frame headlessly without submitting to a GPU canvas.
    pub fn frame_headless(&mut self, input: &Input) {
        if self.components.is_empty() {
            return;
        }
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
    }

    /// Mark the shell scene as dirty, requiring a scene rebuild on the next prepared frame.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, mut component: Box<dyn Chrome>) {
        self.dirty = true;
        component.update(ChromeUpdate::SystemStatus(&self.system_status));
        component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        component.update(ChromeUpdate::InteractionDomains(&self.interaction_domains));
        component.update(ChromeUpdate::ReducedMotion(self.reduced_motion));
        component.update(ChromeUpdate::Scale(self.scale));
        component.update(ChromeUpdate::Appearance(&self.design));
        component.update(ChromeUpdate::AppCatalog(&self.catalog));
        component.update(ChromeUpdate::Windows(&self.windows));
        component.update(ChromeUpdate::AllWindows(&self.all_windows));
        component.update(ChromeUpdate::SpaceUse(self.space_use));
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
        if self.reduced_motion != reduced {
            self.reduced_motion = reduced;
            self.dirty = true;
            self.ui.set_reduced_motion(reduced);
            for component in &mut self.components {
                component.update(ChromeUpdate::ReducedMotion(reduced));
            }
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
        let scale = if scale.is_finite() && scale > 0.0 {
            scale.clamp(0.25, 16.0)
        } else {
            1.0
        };
        self.ui.set_scale(scale);
        if self.scale != scale {
            self.scale = scale;
            self.dirty = true;
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
    pub fn set_color_scheme(&mut self, scheme: tessera_desktop::settings::ColorScheme) {
        let design = tessera_design::Design::for_scheme(scheme);
        if self.design != design {
            self.design = design;
            self.dirty = true;
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
        self.dirty = true;
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
        self.dirty = true;
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
        self.dirty = true;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AllWindows(&self.all_windows));
        }
    }

    /// Adopt the server-resolved output space use for this frame (ADR-0183).
    ///
    /// Pushed by the composition root from `Server::space_use()`; chrome never
    /// derives it, because only the server resolves physical coverage, declared
    /// opacity, and output layout.
    pub fn set_space_use(&mut self, space_use: tessera_desktop::window::SpaceUse) {
        if self.space_use != space_use {
            self.space_use = space_use;
            self.dirty = true;
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::SpaceUse(space_use));
            }
        }
    }

    /// Replace the host's workspace snapshot. Called once per frame by the
    /// main loop with `server.workspace_snapshot()`.
    pub fn set_workspaces(&mut self, workspaces: WorkspaceSnapshot) {
        if self.workspaces != workspaces {
            self.workspaces = workspaces;
            self.dirty = true;
        }
    }

    /// Drain the surface id of the window a component asked to focus this
    /// frame, if any.
    pub fn take_clicked_window(&mut self) -> Option<tessera_desktop::window::WindowId> {
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
        if self.system_status != status {
            self.system_status = status;
            self.dirty = true;
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::SystemStatus(&self.system_status));
            }
        }
    }

    /// Replace the persistent-settings snapshot and notify chrome hosting
    /// settings UI.
    pub fn set_settings(&mut self, snapshot: tessera_desktop::settings::SettingsSnapshot) {
        self.settings = Some(snapshot);
        self.dirty = true;
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
        self.dirty = true;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        }
    }

    /// Replace the Interaction Domain authority snapshot and notify the overview and
    /// Agent Workspaces before their next frame.
    pub fn set_interaction_domains(&mut self, snapshot: InteractionDomainSnapshot) {
        self.interaction_domains = snapshot;
        self.dirty = true;
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
    ) -> Vec<(Option<u64>, tessera_desktop::settings::SettingsAction)> {
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
    pub fn take_dock_position(&mut self) -> Option<tessera_desktop::dock::DockPosition> {
        self.events.dock_position.take()
    }

    /// Drain trusted Interaction Domain-management intents in UI order.
    pub fn take_interaction_domain_intents(&mut self) -> Vec<InteractionDomainIntent> {
        std::mem::take(&mut self.events.interaction_domain_intents)
    }

    /// Drain the workspace id the chrome asked to switch to this frame, if
    /// any (the workspace bar's clicked tile). The main loop forwards it to
    /// `Server::switch_workspace_to`.
    pub fn take_switch_workspace(&mut self) -> Option<tessera_desktop::workspace::WorkspaceId> {
        self.events.switch_workspace.take()
    }

    /// Drain a notification dismissal requested by the toast stack.
    pub fn take_dismissed_notification(&mut self) -> Option<u64> {
        self.events.dismissed_notification.take()
    }

    /// Whether the chrome asked to toggle Pivot this frame (e.g. clicking the
    /// dock's Pivot tile).
    pub fn take_toggle_pivot(&mut self) -> bool {
        std::mem::take(&mut self.events.toggle_pivot)
    }

    /// Drain a pending clipboard copy request from chrome.
    pub fn take_clipboard_copy(&mut self) -> Option<String> {
        self.events.clipboard_copy.take()
    }

    /// Drain a pending natural-language Agent prompt dispatched from Pivot.
    pub fn take_agent_prompt(&mut self) -> Option<String> {
        self.events.agent_prompt.take()
    }

    /// Toggle overview mode on the component that owns it (M9). Mirrors
    /// [`Shell::toggle`]: fanned out to every component; static components
    /// ignore it.
    pub fn toggle_overview(&mut self) {
        let opening = !self.overview_active();
        if opening {
            self.broadcast_command(ChromeCommand::ClosePivot);
            self.broadcast_command(ChromeCommand::CloseControlCenter);
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

    /// Toggle the control center on the component that owns it
    /// (ADR-0080). Mirrors [`Shell::toggle_overview`]: fanned out to every
    /// component; static components ignore it.
    pub fn toggle_control_center(&mut self) {
        let opening = !self.control_center_active();
        if opening {
            self.broadcast_command(ChromeCommand::ClosePivot);
            self.broadcast_command(ChromeCommand::CloseOverview);
        }
        self.broadcast_command(ChromeCommand::ToggleControlCenter);
    }

    /// Whether the control center is currently open.
    pub fn control_center_active(&self) -> bool {
        self.components.iter().any(|c| c.control_center_active())
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
        display: tessera_primitives::Rect,
        windows: &[Window],
        order: &[tessera_desktop::window::WindowId],
        selected: Option<tessera_desktop::window::WindowId>,
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
    pub fn take_overview_pick(&mut self) -> Option<tessera_desktop::window::WindowId> {
        self.events.overview_pick.take()
    }

    /// Window clicked in the held-modifier switcher, if any.
    pub fn take_window_switcher_pick(&mut self) -> Option<tessera_desktop::window::WindowId> {
        self.events.window_switcher_pick.take()
    }

    /// Whether a click-away dismissed the window switcher this frame.
    pub fn take_window_switcher_cancel(&mut self) -> bool {
        std::mem::take(&mut self.events.window_switcher_cancel)
    }

    /// Workspace id the overview's rail asked to switch to, if any.
    pub fn take_overview_switch(&mut self) -> Option<tessera_desktop::workspace::WorkspaceId> {
        self.events.overview_switch.take()
    }

    /// Open the screenshot region selector. No-op if no selector component is
    /// registered.
    pub fn start_screenshot(&mut self) {
        self.broadcast_command(ChromeCommand::OpenBuiltIn(
            tessera_desktop::app::BuiltInApplication::ScreenshotSelector,
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
    pub fn take_screenshot_region(&mut self) -> Option<tessera_primitives::Rect> {
        self.events.screenshot_region.take()
    }

    /// Point the pixel picker was clicked at this frame, if any (ADR-0054).
    pub fn take_picked_point(&mut self) -> Option<tessera_primitives::Point> {
        self.events.picked_point.take()
    }

    /// Window id the window picker was clicked on this frame, if any
    /// (ADR-0054).
    pub fn take_picked_window(&mut self) -> Option<tessera_desktop::window::WindowId> {
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
        if self.screenshot_freeze != frozen {
            self.screenshot_freeze = frozen;
            self.dirty = true;
        }
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
                component
                    .cursor_shape_at(x, y, display, &self.windows, &self.workspaces)
                    .or_else(|| self.cursor_hint())
            })
    }

    /// Semantic cursor shape requested by the UI framework on the last frame, if any.
    pub fn cursor_hint(&self) -> Option<CursorShape> {
        CursorShape::from_hint(self.ui.cursor_hint())
    }

    /// Replace the host application catalog and push it to every registered
    /// component. The shell keeps a copy to seed components added later.
    pub fn set_app_catalog(&mut self, catalog: AppCatalog) {
        self.catalog = catalog;
        self.dirty = true;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AppCatalog(&self.catalog));
        }
    }

    /// Feed one resolved key event to every registered component. Components
    /// with keyboard-owned state, such as the launcher or an application
    /// context menu, override [`Chrome::key_char`]; others no-op.
    pub fn key_char(&mut self, kc: tessera_primitives::input::KeyChar) {
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

    /// Fire the global Pivot hotkey. Opening Pivot closes other modal
    /// surfaces so only one captures keyboard input.
    pub fn toggle_pivot(&mut self) {
        let opening = !self
            .components
            .iter()
            .any(|component| component.pivot_active());
        if opening {
            self.broadcast_command(ChromeCommand::CloseControlCenter);
            self.broadcast_command(ChromeCommand::CloseOverview);
        }
        self.broadcast_command(ChromeCommand::TogglePivot);
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
    /// touch this frame, or `None` when any animated component cannot state
    /// its footprint. The compositor uses this to turn animation-driven
    /// frames into partial repaints instead of full-output composites;
    /// `None` preserves the conservative full-repaint behaviour.
    ///
    /// Lens reports its own eased-value and caret-blink footprints through
    /// [`lens::Ui::damage_rect`], so a settling lens widget narrows the
    /// repaint exactly like a chrome component's [`Chrome::damage_region`]
    /// does. Only a component that returns `None` widens the frame.
    ///
    /// This only describes *chrome* animation damage. Client windows, the
    /// software cursor, and the backdrop-effect source have their own damage
    /// paths and are unioned by the host separately.
    pub fn anim_damage_regions(
        &self,
        display: (f32, f32),
    ) -> Option<Vec<tessera_primitives::Rect>> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        // Rectangles stay disjoint. Unioning them into one bounding box is
        // what makes an animation-driven repaint worthless in practice: the
        // HUD sits at the top of the output and the Dock at the bottom, so
        // their union is the whole panel, and every frame in which both ease
        // becomes a full-output composite again. The compositor's damage list
        // is a list precisely so unrelated regions stay unrelated (ADR-0203).
        let mut regions: Vec<tessera_primitives::Rect> = Vec::new();
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
            // `?` is deliberate: a component that cannot localize its own
            // animation is a genuine "repaint everything" instruction.
            let rect = component.damage_region(&self.windows, display)?;
            if !rect.is_empty() {
                regions.push(rect);
            }
        }
        if self.ui.anim_pending() {
            let lens_damage = self.ui.damage_rect();
            let footprint = tessera_primitives::Rect::new(
                lens_damage.x as i32,
                lens_damage.y as i32,
                lens_damage.w.ceil().max(0.0) as i32,
                lens_damage.h.ceil().max(0.0) as i32,
            );
            if footprint.is_empty() {
                // Lens is animating but cannot name a cell (`damage_rect` is
                // only meaningful for a frame that was built; `prepare_frame`
                // builds one every frame, so reaching here means the query
                // itself found nothing). There is no footprint to add, and
                // inventing one would be worse than adding none.
                return Some(regions);
            }
            let covers_full_display =
                footprint.size.w >= display.0 as i32 && footprint.size.h >= display.1 as i32;
            let immersive_active = modal_active
                || window_switcher_active
                || exclusive_presentation_active
                || self.components.iter().any(|c| {
                    c.pivot_active()
                        || c.control_center_active()
                        || c.command_panel_active()
                        || c.overview_active()
                });
            if covers_full_display && !immersive_active {
                // An easing Lens control fell back to ui->root->final_rect because its
                // internal delta is not localized to a single dirty node bounding box.
                // When only persistent chrome (HUD, Dock) is live, constrain the damage
                // to the decoration bands so client windows (like foot) in the center are
                // not forced into full-output recomposition (ADR-0210, INV-ARCH-98).
                let reserved = self.reserved();
                let top_h = reserved.top.max(36);
                let bottom_h = reserved.bottom.max(80);
                regions.push(tessera_primitives::Rect::new(
                    0,
                    0,
                    display.0 as i32,
                    top_h,
                ));
                regions.push(tessera_primitives::Rect::new(
                    0,
                    (display.1 as i32).saturating_sub(bottom_h),
                    display.0 as i32,
                    bottom_h,
                ));
            } else {
                regions.push(footprint);
            }
        }
        Some(regions)
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

    /// Whether any participating chrome component can touch pixels inside `area`
    /// (in physical pixels). When false, the compositor can skip the secondary
    /// stencil overlay render pass entirely for localized client repaints (ADR-0210).
    pub fn chrome_intersects_render_area(
        &self,
        area: flux::CanvasRenderArea,
        scale: f32,
        logical_size: (u32, u32),
    ) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let immersive_active = self.components.iter().any(|c| {
            c.pivot_active()
                || c.control_center_active()
                || c.command_panel_active()
                || c.overview_active()
        });
        if modal_active || window_switcher_active || exclusive_presentation_active || immersive_active {
            return true;
        }

        let non_persistent = self.components.iter().any(|c| {
            !c.persistent_decoration() && c.requires_composition()
        });
        if non_persistent {
            return true;
        }

        let reserved = self.reserved();
        let top_physical = (reserved.top.max(36) as f32 * scale).ceil() as i32;
        let dock_h = reserved.bottom.max(80) as f32 * scale;
        let bottom_physical = (logical_size.1 as f32 * scale - dock_h).floor() as i32;

        let area_y0 = area.x;
        let _ = area_y0;
        let area_y0 = area.y;
        let area_y1 = area.y + area.height as i32;

        if area_y0 < top_physical {
            return true;
        }
        if area_y1 > bottom_physical {
            return true;
        }

        false
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

    /// The nearest compositor-owned wakeup that is *not* an animation: a timed
    /// visual change (a toast leaving its window) that produces no queue event
    /// and must not pump frames meanwhile. `None` when no component needs one.
    pub fn next_animation_deadline(&self) -> Option<std::time::Duration> {
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
            .filter_map(|component| component.animation_deadline_in())
            .min()
    }

    /// Run the animation clock and geometry prepass, then build this frame's
    /// chrome scene. Call once after input is built and before querying damage
    /// or backdrop regions (ADR-0171, ADR-0178, ADR-0203).
    ///
    /// The scene is built here rather than at draw time so that everything
    /// damage assessment reads — `anim_pending`, `anim_damage_region`, and
    /// lens's own `damage_rect` — describes the tree the renderer is about to
    /// draw. Building at draw time made the lens damage query describe the
    /// *previous* frame, which is why `anim_damage_region` had to fall back to
    /// a full-output repaint for any settling lens widget.
    pub fn prepare_frame(&mut self, input: &Input) {
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
                component.prepare_frame(input, &self.windows, &self.workspaces);
            }
        }

        let raw = input.as_raw();
        let cursor = (raw.cursor.x, raw.cursor.y);
        let input_active = cursor != self.last_input_cursor
            || raw.mouse_pressed.iter().any(|&p| p)
            || raw.mouse_released.iter().any(|&r| r)
            || raw.mouse_down.iter().any(|&d| d)
            || raw.scroll_x != 0.0
            || raw.scroll_y != 0.0
            || raw.scroll_pixels_x != 0.0
            || raw.scroll_pixels_y != 0.0
            || raw.key_count > 0
            || raw.text_utf8[0] != 0
            || raw.preedit_utf8[0] != 0;
        self.last_input_cursor = cursor;

        let needs_build = self.dirty
            || input_active
            || self.anim_pending()
            || self.pending_scene.is_none();

        // SAFETY: the device behind `ui` outlives the Shell (construction
        // contract), and this mutates no host state.
        if needs_build
            && let Err(error) = unsafe { self.build_scene(input) }
        {
            // A failed build leaves `pending_scene` empty; `submit` then draws
            // nothing and the frame degrades to a chrome-less desktop rather
            // than dropping the presentation.
            log::warn!("shell: chrome scene build failed: {error}");
        }
    }

    /// Whether a scene is built and awaiting [`Shell::submit`]. Test and
    /// diagnostic seam for the `prepare_frame` → `submit` split.
    #[doc(hidden)]
    pub fn has_pending_scene(&self) -> bool {
        self.pending_scene.is_some()
    }

    /// Whether any participating component completely occludes the desktop
    /// scene with an opaque full-output canvas this frame.
    pub fn occludes_desktop(&self) -> bool {
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
            .any(|component| component.occludes_desktop())
    }

    /// Whether any modal or exclusive presentation surface is currently active.
    pub fn modal_or_exclusive_active(&self) -> bool {
        self.screenshot_freeze
            || self.window_switcher_active()
            || self.exclusive_presentation_active()
            || self
                .components
                .iter()
                .any(|component| component.modal_active())
    }

    /// Aggregate every component's resting minimize-flight targets (the
    /// dock's tile icons), in output coordinates. The main loop pushes these
    /// into the server each frame so even a client-initiated minimize
    /// animates toward the real icon instead of a hardcoded point.
    pub fn minimize_targets(
        &self,
        display: (f32, f32),
    ) -> Vec<(tessera_desktop::window::WindowId, tessera_primitives::Rect)> {
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

    /// Draw the scene `prepare_frame` built onto `canvas`.
    ///
    /// This is the half of a frame that must run inside an open canvas pass.
    /// All of the building — and therefore every state mutation — happened
    /// earlier, so this performs no layout and touches no chrome state. It
    /// takes exactly the `pending_scene` `prepare_frame` stored; calling it
    /// twice in one frame draws once and then nothing.
    ///
    /// # Safety
    /// `canvas` must be a live `flux_canvas` (from `flux::Canvas::as_raw`)
    /// whose pass is currently open for drawing and whose target outlives the
    /// call. It must outlive every `Snapshot` this `Shell` has produced.
    pub unsafe fn submit(&mut self, canvas: *mut c_void) -> Result<(), ShellError> {
        // Take a copy of the handle rather than moving the field: the scene
        // stays available if drawing fails, so a retry, a diagnostic read of
        // `anim_pending`, or the next `submit` in the same frame still has the
        // tree. `Snapshot` is a refcounted handle, so this is a retain, not a
        // rebuild. Clearing `pending_scene` is the *success* path's job.
        let Some(snapshot) = self.pending_scene.as_ref() else {
            // Nothing was built this frame (no participating chrome). Draw
            // nothing: the pass keeps the desktop it already holds.
            return Ok(());
        };
        // SAFETY: `canvas` is the caller's live `flux_canvas`, open for
        // drawing; the snapshot borrows only its own display list.
        unsafe { snapshot.submit(canvas as *mut lens::sys::flux_canvas) }
            .map_err(ShellError::Render)?;
        self.ui.activate(snapshot).map_err(ShellError::Render)?;
        // Present exactly once: the scene has been drawn and activated.
        self.pending_scene = None;
        Ok(())
    }

    /// Build this frame's chrome scene without drawing it.
    ///
    /// Runs the lens frame and captures the resulting snapshot into
    /// [`Shell::pending_scene`], so [`Shell::anim_damage_region`] can report the
    /// real damage footprint (ADR-0203) and [`Shell::submit`] can draw it later
    /// inside a canvas pass. Every state mutation of a frame happens here.
    unsafe fn build_scene(&mut self, input: &Input) -> Result<(), ShellError> {
        {
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
            // Immersive modal surfaces (Pivot, Command Panel, Overview) own the whole output:
            // persistent decorations (HUD status chips) floating inside the blurred field hide.
            let immersive_active = components.iter().any(|component| {
                component.pivot_active()
                    || component.control_center_active()
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
            let snapshot = self.ui.snapshot().map_err(ShellError::Render)?;
            self.pending_scene = Some(snapshot);
            self.dirty = false;
            Ok(())
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
        let out = r.inset(tessera_primitives::Rect::new(0, 0, 1000, 800));
        assert_eq!(out.origin, tessera_primitives::Point { x: 4, y: 10 });
        assert_eq!(out.size, tessera_primitives::Size { w: 996, h: 714 }); // 800-10-76
    }

    #[test]
    fn reserved_inset_clamps_to_non_negative() {
        let r = Reserved {
            top: 0,
            bottom: 2000,
            left: 0,
            right: 0,
        };
        let out = r.inset(tessera_primitives::Rect::new(0, 0, 100, 100));
        assert_eq!(out.size, tessera_primitives::Size { w: 100, h: 0 });
    }

    #[test]
    fn cursor_shape_from_lens_cursor_hint() {
        assert_eq!(CursorShape::from_hint(lens::CursorHint::Default), None);
        assert_eq!(
            CursorShape::from_hint(lens::CursorHint::Pointer),
            Some(CursorShape::Pointer)
        );
        assert_eq!(
            CursorShape::from_hint(lens::CursorHint::Text),
            Some(CursorShape::Text)
        );
        assert_eq!(
            CursorShape::from_hint(lens::CursorHint::ResizeNs),
            Some(CursorShape::ResizeNs)
        );
        assert_eq!(
            CursorShape::from_hint(lens::CursorHint::ResizeEw),
            Some(CursorShape::ResizeEw)
        );
    }

    #[test]
    fn cursor_shape_at_falls_back_to_ui_cursor_hint() {
        struct CapturingComponent;
        impl Chrome for CapturingComponent {
            fn render(
                &mut self,
                frame: &mut Frame,
                _input: &Input,
                _windows: &[Window],
                _workspaces: &WorkspaceSnapshot,
                _i18n: &Localizer,
                _out: &mut ChromeEvents,
            ) {
                frame.set_cursor_hint(lens::CursorHint::Text);
            }

            fn captures_pointer(
                &self,
                _x: f32,
                _y: f32,
                _display: (f32, f32),
                _windows: &[Window],
                _workspaces: &WorkspaceSnapshot,
            ) -> bool {
                true
            }
        }

        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(CapturingComponent));
        let input = Input::default();
        shell.frame_headless(&input);
        assert_eq!(shell.cursor_hint(), Some(CursorShape::Text));
        assert_eq!(
            shell.cursor_shape_at(10.0, 10.0, (1920.0, 1080.0)),
            Some(CursorShape::Text)
        );
    }

    #[test]
    fn pattern_scaffolding_smoke() {
        use crate::widgets::datagrid::{DataGrid, DataGridCol};
        use crate::widgets::menu::{MenuItem, MenubarItem};
        use crate::widgets::tree::{TreeItem, TreeView};

        let _col = DataGridCol::new("Col").width(100.0);
        let _grid = DataGrid::new("grid");

        let _item = TreeItem::new("Node").depth(0).leaf(true);
        let _tree = TreeView::new("tree");

        let _menu_item = MenuItem::new("Action");
        let _menubar_item = MenubarItem::new("File", "file_menu");
    }

    /// A component that owns a glass recipe but exposes whatever visible
    /// backdrop footprint its current state declares.
    struct GlassChrome {
        visible: bool,
        frost: Vec<BackdropRegion>,
    }

    impl Chrome for GlassChrome {
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

        fn requires_composition(&self) -> bool {
            self.visible
        }

        fn backdrop_blur_sigma(&self) -> f32 {
            if self.visible { 12.0 } else { 0.0 }
        }

        fn backdrop_regions(
            &self,
            _display: (f32, f32),
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
        ) -> Vec<BackdropRegion> {
            self.frost.clone()
        }
    }

    #[test]
    fn hidden_glass_component_does_not_claim_a_live_backdrop() {
        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(GlassChrome {
            visible: false,
            frost: Vec::new(),
        }));
        let requirements = shell.composition_requirements((1920.0, 1080.0));
        assert!(
            !requirements.live_backdrop_effect,
            "a glass recipe on a component with no visible output must not \
             block direct scanout"
        );
        assert!(!requirements.visible_pixels);
    }

    #[test]
    fn visible_glass_component_claims_a_live_backdrop() {
        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(GlassChrome {
            visible: true,
            frost: vec![BackdropRegion {
                x: 0.0,
                y: 1000.0,
                w: 1920.0,
                h: 80.0,
                wash: None,
                opacity: 1.0,
            }],
        }));
        let requirements = shell.composition_requirements((1920.0, 1080.0));
        assert!(requirements.visible_pixels);
        assert!(
            requirements.live_backdrop_effect,
            "visible glass that consumes backdrop pixels must be reported"
        );
    }

    /// A component whose animation footprint is a small band.
    struct LocalizedAnim {
        pending: bool,
        footprint: Option<tessera_primitives::Rect>,
    }

    impl Chrome for LocalizedAnim {
        fn render(
            &mut self,
            _f: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }
        fn anim_pending(&self) -> bool {
            self.pending
        }
        fn damage_region(
            &self,
            _windows: &[Window],
            _display: (f32, f32),
        ) -> Option<tessera_primitives::Rect> {
            self.footprint
        }
    }

    /// `[INV-ARCH-92]`-adjacent: the scene must be **built** by the time damage
    /// is assessed, because lens's damage query only describes a frame that
    /// exists. Confirm `prepare_frame` is what builds it, so a host that
    /// assesses damage between `prepare_frame` and `submit` reads this frame.
    #[test]
    fn prepare_frame_builds_the_scene_damage_assessment_reads() {
        let mut shell = Shell::headless().unwrap();
        // Nothing built until a frame is prepared.
        assert!(!shell.has_pending_scene());
        shell.prepare_frame(&Input::default());
        assert!(
            shell.has_pending_scene(),
            "prepare_frame must build the scene, or damage assessment reports the previous frame"
        );
        // Building is idempotent within a frame: a second call rebuilds rather
        // than accumulating or leaving a stale scene.
        shell.prepare_frame(&Input::default());
        assert!(shell.has_pending_scene());
    }

    #[test]
    fn quiescent_shell_preserves_scene_without_rebuilding() {
        let mut shell = Shell::headless().unwrap();
        shell.prepare_frame(&Input::default());
        assert!(shell.has_pending_scene());
        assert!(!shell.dirty);

        // Calling prepare_frame with quiescent input preserves the existing scene
        shell.prepare_frame(&Input::default());
        assert!(shell.has_pending_scene());

        // Marking dirty triggers rebuild
        shell.mark_dirty();
        assert!(shell.dirty);
        shell.prepare_frame(&Input::default());
        assert!(!shell.dirty);
    }

    /// The scene is a *presentable artifact*, and presenting it clears it — but
    /// only on success. A failed draw must leave it available so the caller can
    /// degrade and still inspect the frame it built (ADR-0203).
    #[test]
    fn a_built_scene_presents_once_and_is_retained_until_it_does() {
        let mut shell = Shell::headless().unwrap();
        shell.prepare_frame(&Input::default());
        assert!(shell.has_pending_scene());

        // A submit against a bogus canvas fails, but the scene survives: the
        // failure path must not consume the frame.
        let mut bogus = 0u8;
        let result = unsafe { shell.submit(&mut bogus as *mut u8 as *mut std::ffi::c_void) };
        assert!(result.is_err(), "a null canvas must not silently succeed");
        assert!(
            shell.has_pending_scene(),
            "a failed draw must not consume the scene"
        );

        // Damage assessment still reads the frame it built, not a cleared one.
        // Headless has no chrome, so the region is empty — but the query must
        // answer rather than panic on a built-but-undrawn scene.
        let _ = shell.anim_damage_regions((1920.0, 1080.0));

        // A successful build followed by a fresh build refreshes in place.
        shell.prepare_frame(&Input::default());
        assert!(shell.has_pending_scene());
    }

    #[test]
    fn a_localized_chrome_animation_narrows_the_repaint() {
        // `[INV-ARCH-93]`-adjacent shell contract: a component that names its
        // footprint must produce a region, so the compositor repaints the band
        // instead of the whole output.
        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(LocalizedAnim {
            pending: true,
            footprint: Some(tessera_primitives::Rect::new(0, 0, 400, 32)),
        }));
        assert_eq!(
            shell.anim_damage_regions((1920.0, 1080.0)),
            Some(vec![tessera_primitives::Rect::new(0, 0, 400, 32)]),
            "a self-localized animation must not widen to a full-output repaint"
        );
    }

    /// Two chrome bodies animating at opposite edges of the output must
    /// produce two rectangles, not one bounding box spanning the panel. The
    /// HUD sits at the top and the Dock at the bottom, so a union is the whole
    /// screen and the narrow repaint is lost (ADR-0203).
    #[test]
    fn disjoint_chrome_animations_stay_disjoint() {
        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(LocalizedAnim {
            pending: true,
            footprint: Some(tessera_primitives::Rect::new(0, 0, 800, 32)),
        }));
        shell.add(Box::new(LocalizedAnim {
            pending: true,
            footprint: Some(tessera_primitives::Rect::new(500, 1040, 900, 40)),
        }));
        let regions = shell
            .anim_damage_regions((1920.0, 1080.0))
            .expect("both components localize their animation");
        assert_eq!(
            regions,
            vec![
                tessera_primitives::Rect::new(0, 0, 800, 32),
                tessera_primitives::Rect::new(500, 1040, 900, 40),
            ],
            "regions must stay disjoint; a union would cover the whole output"
        );
        // And the union of what we did report must be smaller than the panel.
        let total: i64 = regions
            .iter()
            .map(|r| r.size.w as i64 * r.size.h as i64)
            .sum();
        let panel = 1920i64 * 1080;
        assert!(
            total * 10 < panel,
            "reported chrome damage ({total}px^2) should be a small fraction of the panel ({panel}px^2)"
        );
    }

    #[test]
    fn a_component_without_a_footprint_still_forces_full_output() {
        // The conservative escape hatch must survive: `None` still means
        // "repaint everything".
        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(LocalizedAnim {
            pending: true,
            footprint: None,
        }));
        assert_eq!(shell.anim_damage_regions((1920.0, 1080.0)), None);
    }

    #[test]
    fn an_idle_component_contributes_no_region() {
        let mut shell = Shell::headless().unwrap();
        shell.add(Box::new(LocalizedAnim {
            pending: false,
            footprint: Some(tessera_primitives::Rect::new(0, 0, 400, 32)),
        }));
        assert_eq!(
            shell.anim_damage_regions((1920.0, 1080.0)),
            Some(Vec::new()),
            "nothing is animating, so there is no chrome animation damage"
        );
    }
}

pub mod component;
pub mod components;

pub mod widgets;

pub mod layout;

#[cfg(feature = "chrome-tray")]
pub mod tray;
