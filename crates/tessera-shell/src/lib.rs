//! Desktop shell host and built-in UI components.
//! Feature modules share one component protocol and own their presentation state.

use std::os::raw::c_void;

pub use crate::component::{
    AgentActivity, AppCatalog, AppPickParams, BatteryAlertParams, CapabilityPickParams,
    CapabilityPickResult, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate,
    CompositionRequirements, ConfirmAnswer, ConfirmPickParams, CursorShape, Input,
    InteractionDomainIntent, LivePreviewPresentation, Localizer, MirrorMove, OsdKind, OsdParams,
    PickerMode, PinAction, Reserved, ResourceStats, SecretPromptParams, SystemAction, SystemStatus,
    WindowAction, WindowSwitcherPresentation,
};
#[cfg(test)]
use lens::Frame;
use lens::Ui;

mod chrome;
pub mod clock;

pub use chrome::{
    AgentFeedback, AppPicker, BatteryAlert, CapabilityPrompt, ConfirmPrompt, ControlledWindowGuard,
    Overview, ScreenshotSelector, SecretPrompt, SystemOsd, Toast, WindowSwitcher,
};

pub mod accessibility;

/// Upper bound on the rectangles [`Shell::chrome_damage_regions`] collects from a
/// single Lens frame. Matches `LENS_DAMAGE_RECTS_CAPACITY`; fixing it at compile
/// time keeps the damage path allocation-free.
pub const LENS_DAMAGE_CAPACITY: usize = 64;

/// How long a failed chrome-scene build waits before the next attempt
/// (ADR-0223).
///
/// A build failure is normally one degenerate draw command, and flux's encoder
/// makes it sticky for the remainder of that frame — so the retry cannot
/// succeed sooner than the next build with different inputs. 250 ms is the
/// window's own idle-tick order of magnitude: fast enough that a transient
/// failure (a spring overshoot producing a non-finite rect) recovers before the
/// user notices, slow enough that a persistent one costs four rebuild attempts
/// per second instead of one per frame.
pub const BUILD_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);

/// How often a live caret blink clock must be allowed to advance.
///
/// A blink is a *phase* lens integrates from the frame delta, so a host that
/// stopped producing frames could never reach its next flip and the caret
/// would freeze mid-blink (Optics ADR-0151). This is the low idle tick that
/// keeps the clock moving — not a paint cadence: the tick itself draws
/// nothing, and the frame a flip lands on is owed a paint through that
/// frame's own damage.
///
/// 250 ms matches Optics' own `iris` pacer, whose idle tick target is that same
/// value: four samples per second against a 530 ms half-cycle, so a flip is
/// observed and presented within one tick of becoming due. Slower would let the
/// caret sit in the wrong phase for a visible interval; faster is pure wakeup
/// cost, which is the defect that record removed — and the reason it cannot be
/// tied to the refresh interval either, which is exactly the level/edge
/// confusion that record is about.
pub const CARET_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

use tessera_authority::interaction_domain::InteractionDomainSnapshot;
use tessera_desktop::app::BuiltInApplication;
use tessera_desktop::app::Entry;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;

/// Whether one component participates in the current shell pass. Ordinary
/// modal overlays preserve persistent decorations; an exclusive presentation
/// temporarily owns the complete chrome band. A held screenshot freeze
/// outranks every other state: the frozen snapshot already contains the other
/// components (an open Control Center included), so only the selector itself
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
///
/// Each variant carries the lens error, which carries flux's own diagnostic
/// (Optics ADR-0153). Formatting that with `{:?}` — as this type used to —
/// prints the binding's `Error` enum structurally and hides the one field a
/// reader needs: the flux entry point, its source line, and the message that
/// says *which* argument or state was wrong. `Display` forwards to it.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("shell create: {0}")]
    Create(#[source] lens::Error),
    #[error("shell render: {0}")]
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
    /// The last scene that was successfully built, retained so a failed build
    /// degrades to the previous chrome instead of to no chrome at all.
    ///
    /// A build failure is almost always one degenerate draw command (a
    /// non-finite rect, radius or opacity from a layout that divided by a
    /// collapsed extent). Flux's encoder makes that sticky for the rest of the
    /// frame, so the whole tree is lost, not the one offending widget. Holding
    /// the last good snapshot keeps the desktop usable while the defect exists,
    /// and — together with the retry backoff below — stops the failure from
    /// becoming a per-frame rebuild loop.
    last_good_scene: Option<lens::Snapshot>,
    /// Backoff gate for rebuilding after a failed build.
    ///
    /// Without it, `pending_scene == None` makes `needs_build` permanently
    /// true, so a persistent build failure rebuilds the entire chrome tree and
    /// logs a warning on *every* frame, at output rate, forever. Retrying on a
    /// deadline keeps the recovery (a transient failure heals on the next
    /// attempt) without paying the cost on every frame.
    build_retry_at: Option<std::time::Instant>,
    /// The caret blink's damage state as lens reported it for the frame just
    /// built (Optics ADR-0151). Both answers are only meaningful between
    /// `lens_end` and the next `lens_begin`, and damage assessment runs after
    /// the build but before draw time — so they are latched here rather than
    /// re-queried at an interval where they would describe nothing.
    caret: CaretBlink,
    /// When the next caret tick is due, or `None` when no blink clock is live.
    ///
    /// A blink clock only advances inside a build (lens integrates the frame
    /// delta in `lens_begin`), so a host that stopped building could never
    /// reach the next flip and the caret would freeze mid-blink. The tick is
    /// the low idle wakeup that keeps the clock moving; it is not a paint
    /// cadence, and the loop hints it through
    /// [`Shell::next_animation_deadline`].
    caret_tick_at: Option<std::time::Instant>,
    /// Whether any component, layout, or model state changed requiring
    /// a new UI scene build (ADR-0210, INV-ARCH-103).
    dirty: bool,
    /// Last sampled input cursor coordinates to detect pointer motion.
    last_input_cursor: (f32, f32),
}

/// What the last chrome build said about the caret (Optics ADR-0151).
///
/// Lens owns the blink clock — it integrates the phase from the frame delta and
/// compares it against the phase a *presented* snapshot put on screen — so the
/// shell must neither model a blink nor derive one. The two facts it reports
/// are separate because they call for different host behaviour:
///
/// * [`Self::flip_pending`] — the phase changed and has not reached the display
///   yet, so one frame is owed a *paint*;
/// * [`Self::clock_live`] — a blink clock is running, so the loop must keep a
///   low idle *tick* alive or the phase could never advance again.
///
/// Collapsing them, or reporting "a caret exists" as damage, is exactly the
/// defect that record removed: it made every focused text field a permanent
/// animation, and the compositor amplified that into a full composite per
/// display interval for a window displaying a static document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CaretBlink {
    /// `lens_caret_flip_pending`: the current phase differs from the presented
    /// one.
    flip_pending: bool,
    /// `lens_caret_clock_live`: a text widget holds focus and the blink is
    /// enabled.
    clock_live: bool,
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
                last_good_scene: None,
                build_retry_at: None,
                caret: CaretBlink::default(),
                caret_tick_at: None,
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
            last_good_scene: None,
            build_retry_at: None,
            caret: CaretBlink::default(),
            caret_tick_at: None,
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
    ///
    /// The caret blink is part of that policy and belongs to lens (Optics
    /// ADR-0151): the host owns the *preference*, lens owns the clock. Reducing
    /// motion disables the blink outright rather than slowing it, so the caret
    /// stays solid and reports no damage at all — a blink is motion, and
    /// "resolve it in one frame" has no meaning for a state that repeats
    /// forever.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        if self.reduced_motion != reduced {
            self.reduced_motion = reduced;
            self.dirty = true;
            self.ui.set_reduced_motion(reduced);
            self.apply_caret_blink();
            for component in &mut self.components {
                component.update(ChromeUpdate::ReducedMotion(reduced));
            }
        }
    }

    /// Push the reduced-motion preference into lens's caret blink clock.
    ///
    /// `period <= 0` is lens's spelling of "no blink" and there is no
    /// "restore the library default" call, so re-enabling names the binding's
    /// [`lens::DEFAULT_CARET_BLINK_PERIOD_S`] — the library's own value, not a
    /// number copied here, which is what keeps a preference toggle from
    /// silently changing the blink rate when Optics tunes it.
    fn apply_caret_blink(&mut self) {
        let period = if self.reduced_motion {
            0.0
        } else {
            lens::DEFAULT_CARET_BLINK_PERIOD_S
        };
        self.ui.set_caret_blink(period);
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
            // (near-white glyphs on a near-white body).
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

    /// The active translation catalog, so the composition root's own surfaces
    /// (the battery policy) name things the way chrome does instead of carrying
    /// a second copy of the strings.
    #[must_use]
    pub fn localizer(&self) -> Localizer {
        self.i18n
    }

    /// Whether any component requested the session to quit this frame.
    pub fn should_quit(&self) -> bool {
        self.events.quit
    }

    /// Drain an immediate-lock request from the chrome (the Control Center's
    /// Lock now row). The main loop feeds it to `IdleProcess::lock_now`.
    pub fn take_lock(&mut self) -> bool {
        std::mem::take(&mut self.events.lock)
    }

    /// Drain a dock reading row's request to open the control center (ADR-0218).
    pub fn take_open_control_center(&mut self) -> bool {
        std::mem::take(&mut self.events.open_control_center)
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
    ///
    /// Guarded on equality like the other snapshot setters: the host republishes
    /// this every frame from the status poller, so an unconditional dirty flag
    /// would rebuild the whole chrome scene each frame and defeat the quiescent
    /// elision that keeps a static desktop from recompiling display lists.
    pub fn set_settings(&mut self, snapshot: tessera_desktop::settings::SettingsSnapshot) {
        if self.settings.as_ref() == Some(&snapshot) {
            return;
        }
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
    ///
    /// Guarded on equality: the sample arrives on a poller tick many times a
    /// second, and a `dirty` flag for an unchanged sample rebuilds every
    /// component's scene for no pixel difference.
    pub fn set_resource_stats(&mut self, stats: ResourceStats) {
        if self.resource_stats == stats {
            return;
        }
        self.resource_stats = stats;
        self.dirty = true;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        }
    }

    /// Replace the Interaction Domain authority snapshot and notify the overview and
    /// Agent Workspaces before their next frame.
    pub fn set_interaction_domains(&mut self, snapshot: InteractionDomainSnapshot) {
        if self.interaction_domains == snapshot {
            return;
        }
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

    /// Drain the notification dismissals the Control Center requested.
    pub fn take_dismissed_notifications(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.events.dismissed_notifications)
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
    /// [`Self::toggle_overview`]: fanned out to every component; static
    /// components ignore it.
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

    /// Open the control center on the component that owns it, if it is not
    /// already open (ADR-0218).
    ///
    /// The dock's reading column answers a press with this rather than with
    /// [`Shell::toggle_control_center`]: a reading is a shortcut into the surface
    /// that owns the reading, so pressing one while the panel is already open
    /// must not close it. Nothing is broadcast when the panel is already open, so
    /// the press is a no-op rather than a toggle.
    pub fn open_control_center(&mut self) {
        if self.control_center_active() {
            return;
        }
        self.broadcast_command(ChromeCommand::ClosePivot);
        self.broadcast_command(ChromeCommand::CloseOverview);
        self.broadcast_command(ChromeCommand::ToggleControlCenter);
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
    /// Test-only: the window switcher's own animation state.
    #[cfg(test)]
    pub fn presentation_command_probe(
        &self,
    ) -> Option<(
        bool,
        bool,
        Option<tessera_desktop::window::WindowId>,
        f32,
        bool,
    )> {
        self.components
            .iter()
            .find_map(|component| component.window_switcher_probe())
    }

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
    /// Raise a transient system OSD, replacing whatever reading is on screen.
    pub fn show_osd(&mut self, params: OsdParams) {
        self.broadcast_command(ChromeCommand::ShowOsd(&params));
    }

    /// Whether a transient system OSD is currently visible.
    #[must_use]
    pub fn osd_active(&self) -> bool {
        self.components.iter().any(|c| c.osd_active())
    }

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

    /// Hold or release the screenshot freeze. While held, `Chrome::render`
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

    /// The output-logical rectangles a pointer move to `(x, y)` can repaint, or
    /// `None` when that cannot be bounded.
    ///
    /// A pointer move is the one input with no pixel footprint of its own: it
    /// only matters because chrome under the cursor reacts to it. The damage
    /// assessment therefore needs to know *where* that reaction can land. It
    /// used to answer with [`None`] whenever any component captured the
    /// pointer, on the premise that "chrome reacts somewhere in its own band".
    /// That premise holds for a bar and fails for a component that owns most of
    /// the output — an open Control Center, Pivot — where a single mouse
    /// movement repainted the whole screen at the panel's refresh rate.
    ///
    /// Components that capture the pointer answer through
    /// [`Chrome::pointer_damage_regions`]. A component that cannot bound its
    /// own hover reaction keeps returning [`None`], which stays the
    /// conservative full-output verdict — the point is not to guess a band, it
    /// is to let chrome that already knows its footprint say so.
    #[must_use]
    pub fn pointer_damage_regions(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
    ) -> Option<Vec<tessera_primitives::Rect>> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut regions: Vec<tessera_primitives::Rect> = Vec::new();
        for component in self.components.iter().filter(|component| {
            participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            )
        }) {
            if !component.captures_pointer(x, y, display, &self.windows, &self.workspaces) {
                continue;
            }
            // `?` is deliberate: one component that cannot localize its hover
            // reaction is a genuine full-output instruction.
            for rect in
                component.pointer_damage_regions(x, y, display, &self.windows, &self.workspaces)?
            {
                let x0 = rect.x.floor();
                let y0 = rect.y.floor();
                let x1 = (rect.x + rect.w).ceil();
                let y1 = (rect.y + rect.h).ceil();
                let rect = tessera_primitives::Rect::new(
                    x0 as i32,
                    y0 as i32,
                    (x1 - x0).max(0.0) as i32,
                    (y1 - y0).max(0.0) as i32,
                );
                if !rect.is_empty() {
                    regions.push(rect);
                }
            }
        }
        Some(regions)
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
        let immersive_active = modal_active
            || window_switcher_active
            || self.components.iter().any(|component| {
                component.pivot_active()
                    || component.control_center_active()
                    || component.overview_active()
            });
        for component in &mut self.components {
            component.update(ChromeUpdate::LauncherActive(immersive_active));
        }
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
    ///
    /// A caret blink is deliberately **absent** (Optics ADR-0151). This query
    /// means "the display is being driven at its own rate", and a 2 Hz blink
    /// does not justify that: wiring it in here would make every focused text
    /// field render at the panel refresh rate. The caret contributes a *paint*
    /// through [`Self::caret_flip_pending`] and an idle *tick* through
    /// [`Self::next_animation_deadline`], which are the two facts it actually
    /// owns.
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

    /// Whether any participating component owes a bounded **damage-repair**
    /// frame (a retirement window for pixels it just removed). Kept separate
    /// from [`Self::anim_pending`]: a repair budget must not be mistaken for a
    /// value still in motion, and it must go false on its own.
    #[must_use]
    pub fn damage_pending(&self) -> bool {
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
            .any(|component| component.damage_pending())
    }

    /// The output-logical rectangles this frame's chrome scene repaints, or
    /// `None` when a participating component cannot state its footprint.
    ///
    /// Chrome damage has two independent triggers and both are answered here:
    ///
    /// * `scene_changed` — a component mutated its own presentation structure
    ///   (a dock tile's context menu opening or closing). The whole scene was
    ///   rebuilt, so every participating component repainted its pixels.
    /// * otherwise — only components with an animation in flight repaint, plus
    ///   Lens's own eased values and caret blink.
    ///
    /// Lens reports its damage through [`lens::Ui::damage_rects`], which
    /// localizes per placed body and per drawn leaf, so a settling widget
    /// narrows the repaint exactly like a component's [`Chrome::damage_regions`].
    /// Only a component that returns `None` widens the frame — that is a genuine
    /// "repaint everything" instruction and is preserved as such.
    ///
    /// This describes *chrome* damage only. Client windows, the software cursor,
    /// and the backdrop-effect source have their own paths and are unioned by the
    /// host separately.
    pub fn chrome_damage_regions(
        &self,
        display: (f32, f32),
        scene_changed: bool,
    ) -> Option<Vec<tessera_primitives::Rect>> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        // Rectangles stay disjoint. Unioning them into one bounding box is what
        // makes a regional repaint worthless in practice: the HUD sits at the top
        // of the output and the Dock at the bottom, so their union is the whole
        // panel. The compositor's damage list is a list precisely so unrelated
        // regions stay unrelated (ADR-0203).
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
            .filter(|component| {
                scene_changed || component.anim_pending() || component.damage_pending()
            })
        {
            // `?` is deliberate: a component that cannot localize its own paint
            // is a genuine "repaint everything" instruction.
            for rect in component.damage_regions(&self.windows, display)? {
                /* Outward on both edges: flooring the origin and ceiling only the
                 * extent leaves the far edge short, which under-covers. */
                let x0 = rect.x.floor();
                let y0 = rect.y.floor();
                let x1 = (rect.x + rect.w).ceil();
                let y1 = (rect.y + rect.h).ceil();
                let rect = tessera_primitives::Rect::new(
                    x0 as i32,
                    y0 as i32,
                    (x1 - x0).max(0.0) as i32,
                    (y1 - y0).max(0.0) as i32,
                );
                if !rect.is_empty() {
                    regions.push(rect);
                }
            }
        }
        // Lens's own damage list is the answer for both a settling eased value
        // and the caret's blink *edge* (Optics ADR-0151). `anim_pending` covers
        // only the eased values, so the query is also asked on a flip — that is
        // the one frame on which the caret's rect is the whole answer.
        if scene_changed || self.ui.anim_pending() || self.caret.flip_pending {
            // Lens answers with the frame's regional damage list, already
            // localized per drawn leaf and per placed body. Taking it as-is is
            // the whole point of the list contract: a direction that must be
            // second-guessed here (band substitution, "looks too big" clamping)
            // would reintroduce exactly the guessed footprint the list replaces.
            let mut buffer = [lens::Rect::default(); LENS_DAMAGE_CAPACITY];
            let count = self.ui.damage_rects(&mut buffer[..]);
            for damage in buffer.iter().take(count) {
                /* Lens already rounds its damage outward to whole logical pixels,
                 * so a truncating conversion is exact here. */
                let rect = tessera_primitives::Rect::new(
                    damage.x as i32,
                    damage.y as i32,
                    damage.w as i32,
                    damage.h as i32,
                );
                if !rect.is_empty() {
                    regions.push(rect);
                }
            }
        }
        Some(regions)
    }

    /// Whether a caret blink flip still has to reach the display (Optics
    /// ADR-0151).
    ///
    /// This is the caret's *only* paint signal. It is true for exactly the
    /// frames on which the phase lens painted differs from the phase a
    /// presented snapshot put on screen, so a settled desktop with a focused
    /// text field alternates between "one frame owed" and "nothing owed" — not
    /// permanently true, which is what made every focused field an animation
    /// and cost a full composite per display interval.
    ///
    /// Read from the last [`Shell::prepare_frame`] build; the answer is only
    /// meaningful between that build and the next one.
    #[must_use]
    pub fn caret_flip_pending(&self) -> bool {
        self.caret.flip_pending
    }

    /// Whether lens has a caret blink clock running (Optics ADR-0151).
    ///
    /// A level, and deliberately not a paint request: it says the loop must
    /// keep its low idle tick alive, because lens advances the blink phase by
    /// the frame delta inside a build. Exposed for the loop's diagnostics and
    /// for tests; the pacing hint itself is folded into
    /// [`Self::next_animation_deadline`].
    #[must_use]
    pub fn caret_clock_live(&self) -> bool {
        self.caret.clock_live
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
    /// (in physical pixels). When false, the compositor skips the chrome work
    /// entirely for an otherwise-localized client repaint
    /// (ADR-0210, `[INV-ARCH-102]`).
    ///
    /// The test is the chrome's *own* union footprint, not a per-edge band
    /// guess. Persistent chrome is not guaranteed to occupy an edge strip: the
    /// HUD floats inside the top band, and a dock can sit on any of four edges,
    /// so a test that only compared `area.y` against a top and bottom threshold
    /// answered "no chrome here" for a left or right dock and clipped its pixels
    /// away. Testing the real footprint keeps the answer correct on every dock
    /// position and needs no `reserved` arithmetic in physical space.
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
        let immersive_active = self
            .components
            .iter()
            .any(|c| c.pivot_active() || c.control_center_active() || c.overview_active());
        if modal_active
            || window_switcher_active
            || exclusive_presentation_active
            || immersive_active
        {
            return true;
        }
        // Any chrome that is not a persistent decoration owns arbitrary pixels
        // while it is visible: it cannot be bounded by an edge strip, so it
        // disqualifies the elision outright.
        if self
            .components
            .iter()
            .any(|c| !c.persistent_decoration() && c.requires_composition())
        {
            return true;
        }
        let Some(footprint) = self.persistent_chrome_footprint(scale, logical_size) else {
            // No persistent chrome is visible this frame, so no chrome pixel can
            // be inside the render area.
            return false;
        };
        // `footprint` is `(x0, y0, x1, y1)` in physical pixels; the render area
        // is in physical pixels too but carried as floats. Compare in one
        // domain: widening the footprint to `f32` is exact for any coordinate a
        // framebuffer can hold, whereas truncating the area would make a
        // half-pixel overlap read as a miss and clip the chrome pixel away.
        let (x0, y0, x1, y1) = footprint;
        let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        area.x < x1 && area.x + area.width > x0 && area.y < y1 && area.y + area.height > y0
    }

    /// Union of every visible persistent decoration's footprint, in physical
    /// pixels, as `(x0, y0, x1, y1)`. `None` when no persistent decoration is
    /// currently visible.
    ///
    /// A decoration reports its footprint through
    /// [`Chrome::damage_regions`], which is the same rectangle its animation
    /// repaints — for the Dock that is the whole magnified strip envelope plus
    /// its shadow skirt, which is exactly the extent the painted body can reach.
    fn persistent_chrome_footprint(
        &self,
        scale: f32,
        logical_size: (u32, u32),
    ) -> Option<(i32, i32, i32, i32)> {
        let display = (logical_size.0 as f32, logical_size.1 as f32);
        let mut union: Option<(i32, i32, i32, i32)> = None;
        for component in &self.components {
            if !component.persistent_decoration() || !component.requires_composition() {
                continue;
            }
            let Some(component_regions) = component.damage_regions(&self.windows, display) else {
                continue;
            };
            for region in component_regions {
                let x0 = region.x.floor() as i32;
                let y0 = region.y.floor() as i32;
                let x1 = (region.x + region.w).ceil() as i32;
                let y1 = (region.y + region.h).ceil() as i32;
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                union = Some(match union {
                    None => (x0, y0, x1, y1),
                    Some((ax0, ay0, ax1, ay1)) => {
                        (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1))
                    }
                });
            }
        }
        let bounds = union?;
        let s = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Some((
            (bounds.0 as f32 * s).floor() as i32,
            (bounds.1 as f32 * s).floor() as i32,
            (bounds.2 as f32 * s).ceil() as i32,
            (bounds.3 as f32 * s).ceil() as i32,
        ))
    }

    /// Return the precise compositor-owned work that would affect the next
    /// output frame.  Direct-scanout policy consumes this instead of treating
    /// registered components, dormant animations, or a non-zero blur setting
    /// as global blockers.
    pub fn composition_requirements(&self, _display: (f32, f32)) -> CompositionRequirements {
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
        }
        requirements
    }

    /// The nearest compositor-owned wakeup that is *not* an animation: a timed
    /// visual change (a toast leaving its window), or the caret blink's idle
    /// tick, that produce no queue event and must not pump frames meanwhile.
    /// `None` when nothing needs one.
    ///
    /// The caret contributes here and **not** to [`Self::anim_pending`] (Optics
    /// ADR-0151). A blink is a phase lens integrates from the frame delta, so a
    /// loop that stopped building could never reach the next flip and the caret
    /// would freeze mid-blink; but that is a reason to *tick*, not a reason to
    /// paint at the panel rate. The tick draws nothing — the frame a flip lands
    /// on is owed a paint through [`Self::caret_flip_pending`] and the damage
    /// list.
    pub fn next_animation_deadline(&self) -> Option<std::time::Duration> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let component_deadline = self
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
            .filter_map(|component| component.animation_deadline_in())
            .min();
        let caret_deadline = self
            .caret_tick_at
            .map(|at| at.saturating_duration_since(std::time::Instant::now()));
        match (component_deadline, caret_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Run the animation clock and geometry prepass, then build this frame's
    /// chrome scene. Call once after input is built and before querying damage
    /// or backdrop regions (ADR-0171, ADR-0178, ADR-0203).
    ///
    /// The scene is built here rather than at draw time so that everything
    /// damage assessment reads — `anim_pending`, `chrome_damage_regions`, and
    /// lens's own `damage_rect` — describes the tree the renderer is about to
    /// draw. Building at draw time made the lens damage query describe the
    /// *previous* frame, which is why `chrome_damage_regions` had to fall back to
    /// a full-output repaint for any settling lens widget.
    pub fn prepare_frame(&mut self, input: &Input) {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let immersive_active = modal_active
            || window_switcher_active
            || self.components.iter().any(|component| {
                component.pivot_active()
                    || component.control_center_active()
                    || component.overview_active()
            });
        for component in &mut self.components {
            component.update(ChromeUpdate::LauncherActive(immersive_active));
        }
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
        let display = (raw.display_size.x, raw.display_size.y);
        let pointer_moved = cursor != self.last_input_cursor;
        // Pure pointer motion outside every chrome component's capture area
        // (and when neither the old nor the new cursor position was captured by
        // chrome) cannot alter any widget's hover state, so it must not force a
        // full Lens scene rebuild on every mouse report over client windows or
        // the desktop wallpaper. Any edge trigger handled by `component.prepare_frame`
        // above (such as auto-hide dock edge reveal) sets `anim_pending()`,
        // which independently triggers `needs_build` below.
        let pointer_captured = pointer_moved
            && (self.captures_pointer_at(cursor.0, cursor.1, display)
                || self.captures_pointer_at(
                    self.last_input_cursor.0,
                    self.last_input_cursor.1,
                    display,
                ));
        let input_active = pointer_captured
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

        // The caret blink advances only inside a build (lens integrates the
        // frame delta in `lens_begin`), so a live clock makes the next *tick* a
        // reason to build — not a reason to paint. The distinction is the whole
        // point of Optics ADR-0151: building is layout plus a display-list
        // capture, painting is a full composite, and a blink only needs the
        // latter when its phase actually flipped.
        let now = std::time::Instant::now();
        let caret_tick_due = self.caret_tick_at.is_some_and(|at| at <= now);

        // A caret *flip* is deliberately not a rebuild trigger: it changes what
        // the next paint shows, not which widgets exist. It is read below, from
        // the frame that was just built, and reaches damage assessment as that
        // frame's own damage.
        let needs_build = self.dirty
            || input_active
            || self.anim_pending()
            || self.damage_pending()
            || caret_tick_due
            || self.pending_scene.is_none();

        // A failed build must not become a per-frame rebuild loop (ADR-0223).
        // `pending_scene == None` makes `needs_build` permanently true, so
        // without a gate one degenerate chrome draw command would rebuild the
        // whole tree and log a warning on every frame, forever. Retry on a
        // deadline instead: the recovery still happens (a transient failure
        // heals on the next attempt) but the cost is bounded, and `submit`
        // keeps drawing the last good scene in the meantime.
        if needs_build
            && self
                .build_retry_at
                .is_some_and(|at| std::time::Instant::now() < at)
        {
            // The caret's tick deadline has to move forward even though nothing
            // was built: leaving it in the past would turn the loop's pacing
            // hint into a zero-length sleep, i.e. a busy spin for as long as the
            // backoff lasts. The caret holds its phase instead — a frozen caret
            // is part of "chrome is degraded", which the caller already logs.
            if caret_tick_due {
                self.caret_tick_at = Some(now + CARET_TICK_INTERVAL);
            }
            return;
        }

        // SAFETY: the device behind `ui` outlives the Shell (construction
        // contract), and this mutates no host state.
        if needs_build && let Err(error) = unsafe { self.build_scene(input) } {
            // The build publishes into `pending_scene`; on failure it is left
            // untouched, so `submit` falls back to the last scene that did
            // build. A chrome-less desktop is a much worse failure mode than
            // slightly stale chrome: the dock is how the user recovers.
            self.build_retry_at = Some(std::time::Instant::now() + BUILD_RETRY_BACKOFF);
            // The error carries flux's own diagnostic — which entry point
            // rejected the call, at which source line, and why. Without it this
            // log line said only `FLUX_ERROR_INVALID_ARGUMENT`, which cannot
            // distinguish a non-finite rect from an out-of-bounds render area
            // from a double-begun pass, so no reader could act on it.
            log::warn!(
                "shell: chrome scene build failed: {error}; drawing the last good scene and \
                 retrying in {:?}",
                BUILD_RETRY_BACKOFF
            );
        }

        if !needs_build {
            // Nothing was built, so lens qualified nothing new; the answers
            // latched by the last build still describe the frame on screen, and
            // the caret deadline it armed is still correct.
            return;
        }

        // Read the caret's damage state from the frame that was just built.
        // This has to happen here, on the same frame: both answers are
        // *transition* queries, valid only between `lens_end` and the next
        // `lens_begin`, and damage assessment runs after the build but before
        // draw time.
        //
        // `as_raw` is the binding's documented escape hatch: `lens-rs` exposes
        // no accessor for these queries yet, and modelling a blink host-side is
        // precisely what Optics ADR-0151 forbids — two clocks in one system
        // drift, and the host cannot know which phase actually reached the
        // display.
        self.caret = CaretBlink {
            flip_pending: self.ui.caret_flip_pending(),
            clock_live: self.ui.caret_clock_live(),
        };
        // A live clock owes the next tick (ADR-0151), and only a live clock
        // does: `None` releases the deadline the moment no caret is focused,
        // which is how a settled desktop reaches quiescence instead of waking
        // four times a second forever.
        self.caret_tick_at = self
            .caret
            .clock_live
            .then(|| std::time::Instant::now() + CARET_TICK_INTERVAL);
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

    /// Whether a participating component paints an opaque full-output cover
    /// at full strength this frame, so no desktop pixel can survive it.
    ///
    /// The compositor consumes this to skip the base desktop pass. A cover
    /// mid-fade is *not* opaque: its body is still draining, so the desktop
    /// beneath it is visible and must be drawn.
    pub fn opaque_cover_active(&self) -> bool {
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
            .any(|component| component.paints_opaque_cover() && component.cover_opacity() >= 0.999)
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
        // The scene to draw: this frame's build when there is one, else the
        // last scene that built successfully (ADR-0223). A failed build leaves
        // `pending_scene` empty, and drawing nothing would take the dock — the
        // user's means of recovery — off the screen. Slightly stale chrome is
        // strictly better than no chrome.
        let Some(scene) = self
            .pending_scene
            .as_ref()
            .or(self.last_good_scene.as_ref())
        else {
            // Nothing has ever been built (no participating chrome). Draw
            // nothing: the pass keeps the desktop it already holds.
            return Ok(());
        };
        // SAFETY: `canvas` is the caller's live `flux_canvas`, open for
        // drawing; the snapshot borrows only its own display list.
        unsafe { scene.submit(canvas as *mut lens::sys::flux_canvas) }
            .map_err(ShellError::Render)?;
        self.ui.activate(scene).map_err(ShellError::Render)?;
        // Present exactly once: the scene has been drawn and activated. Only a
        // *built* scene is consumed; a fallback to `last_good_scene` keeps the
        // reference so it can be drawn again on the next frame.
        self.pending_scene = None;
        Ok(())
    }

    /// Build this frame's chrome scene without drawing it.
    ///
    /// Runs the lens frame and captures the resulting snapshot into
    /// [`Shell::pending_scene`], so [`Shell::chrome_damage_regions`] can report the
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
            // Immersive modal surfaces (Pivot, Control Center, Overview, Window Switcher)
            // own the whole output: persistent decorations floating inside the field
            // hide or shed their transient popovers.
            let immersive_active = modal_active
                || window_switcher_active
                || components.iter().any(|component| {
                    component.pivot_active()
                        || component.control_center_active()
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
            // Retain the scene that is about to be presented so a later failed
            // build can fall back to it (ADR-0223). `Snapshot` is refcounted,
            // so this is a retain, not a copy of the display list.
            self.last_good_scene = Some(snapshot.retain());
            self.build_retry_at = None;
            self.pending_scene = Some(snapshot);
            self.dirty = false;
            Ok(())
        }
    }

    /// Take the one-shot "a component changed its own presentation structure"
    /// report from the last scene build.
    ///
    /// Consumed by damage assessment so a popup that opened or closed without an
    /// animation still repaints its footprint on this frame. Reading clears it:
    /// the rebuild it asks for has already happened, so re-reporting it would
    /// keep the frame loop awake forever.
    pub fn take_scene_changed(&mut self) -> bool {
        std::mem::take(&mut self.events.scene_changed)
    }
}

#[cfg(test)]
mod tests;

pub mod component;
pub mod components;

pub mod widgets;

pub mod layout;

#[cfg(feature = "chrome-tray")]
pub mod tray;
