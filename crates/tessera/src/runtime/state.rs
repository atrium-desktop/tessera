use super::*;

type IconSnapshot = std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>>;
pub(super) struct AppScanRequest {
    pub(super) icon_theme: String,
    pub(super) scale: u32,
}
pub(super) type AppScanResult = (
    String,
    u32,
    Vec<tessera_desktop::app::Entry>,
    IconSnapshot,
    Vec<DecodedIcon>,
);
pub(super) type WindowEventSignature = Vec<(
    tessera_desktop::window::WindowId,
    bool,
    bool,
    bool,
    bool,
    bool,
    Option<String>,
)>;

/// Per-gesture state of an in-flight compositor-owned touchpad swipe:
/// finger count, per-axis pixel accumulators, the latched axis, and the
/// bookkeeping of bindings that hold state across one gesture.
#[derive(Default)]
pub(super) struct SwipeState {
    pub(super) fingers: u8,
    pub(super) dx: f32,
    pub(super) dy: f32,
    pub(super) axis: Option<tessera_desktop::gesture::GestureAxis>,
    /// Whether this swipe opened the window switcher (`WindowCycle`).
    pub(super) switcher: bool,
    /// Whether the Control Center binding already fired (`ControlCenter`);
    /// latches until SwipeEnd so a long swipe cannot oscillate the sheet.
    pub(super) control_center_fired: bool,
    /// Whether the overview binding already fired (`Overview`); latches
    /// until SwipeEnd so a long swipe cannot oscillate the picker.
    pub(super) overview_fired: bool,
}

/// Baselines and carry-over owned by the output-damage pipeline.
///
/// Keeping these values together makes invalidation an explicit subsystem
/// boundary instead of a set of unrelated flags on the composition root.
#[derive(Default)]
pub(super) struct DamageTracking {
    /// Double-buffered per-surface generation baseline for the client-damage
    /// pass, owned by `tessera_composite::presentation::ClientDamageTracker` behind the
    /// `SurfaceDamageFrame` seam. The maps swap in place per frame so the
    /// generation diff never freshly heap-allocates.
    pub(super) client_damage: ClientDamageTracker,
    pub(super) last_notif_revision: Option<u64>,
    /// (overview, window switcher, keyboard capture, screenshot selector,
    /// modal/exclusive active, control center active) at the last assessment —
    /// modal chrome changes outside signed paths.
    pub(super) last_chrome_mode: Option<(bool, bool, bool, bool, bool, bool)>,
    pub(super) last_session_locked: bool,
    /// (shape, hidden) as of the last presented frame.
    pub(super) last_presented_cursor: Option<(u32, bool)>,
    /// `(surface_id, generation)` of the client cursor surface committed to
    /// the KMS hardware cursor plane at the last successful present, or `None`
    /// when a theme cursor or no hardware cursor was committed.
    pub(super) last_presented_client_cursor: Option<(usize, u64)>,
    /// Last logged client-cursor hardware-offload rejection `(surface_id, generation, reason)`
    /// so diagnostics report every distinct fallback once without per-frame log spam.
    pub(super) last_client_cursor_reject: Option<(usize, u64, cursor::ClientCursorRejectReason)>,
    pub(super) last_presented_cursor_position: Option<(i32, i32)>,
    /// Sprite identity committed to the hardware cursor plane at the last
    /// successful present: (hotspot, sprite size). The presentation path
    /// uses these to skip redundant backend cache lookups when the cursor
    /// is fully unchanged between frames. These mirror the KMS plane state
    /// and must be reset whenever the plane is reprogrammed externally
    /// (VT switch, backend reconfigure).
    pub(super) last_presented_cursor_hotspot: Option<(u32, u32)>,
    pub(super) last_presented_cursor_pixels: Option<(u32, u32)>,
    /// Assessed damage whose frame never reached gfx's submit point (acquire
    /// timeout, a pre-recording surface failure). gfx owns per-image epoch
    /// history; this is only the pre-submit transaction carry. Unioned into the
    /// next assessment and cleared by the next successful submit.
    pub(super) pending_damage: Option<FrameDamage>,
    /// Wall-clock minute of the last presented frame; a rollover forces one
    /// frame so the status-bar clock cannot go stale while idle.
    pub(super) last_present_minute: Option<u64>,
    /// Shell mutations applied outside the signed paths (status poller,
    /// config reload, app rescan, IPC settings/Interaction Domain control) since the last
    /// assessment.
    pub(super) chrome_dirty: bool,
    /// Set when the output was resized/recreated; the next frame must render
    /// in full regardless of damage.
    pub(super) force_full_redraw: bool,
    /// Previously active window ID; focus change dirties the active border
    /// and shadow of the transitioning windows.
    pub(super) last_focused_window: Option<tessera_desktop::window::WindowId>,
}

impl DamageTracking {
    /// Clear all cached hardware/software cursor baselines so the next frame
    /// unconditionally reprograms the KMS cursor plane or repaints the cursor.
    pub(super) fn reset_cursor_baseline(&mut self) {
        self.last_presented_cursor = None;
        self.last_presented_client_cursor = None;
        self.last_client_cursor_reject = None;
        self.last_presented_cursor_position = None;
        self.last_presented_cursor_hotspot = None;
        self.last_presented_cursor_pixels = None;
    }
}

/// A compositor-owned presentation target for one physical output.
///
/// Field drop order is safety-critical: `last_source_export` and `source_lease`
/// hold `optics_gfx::DmabufLease` handles referencing a `optics_gfx::Surface`, and `canvas`
/// holds pass state referencing `surface`. All of them must be declared before
/// `surface` so Rust drops them first. A mirror target is blitted from its
/// source target; an extended target owns its output-local scene pass.
pub(super) struct OutputTarget {
    /// Retained descriptor of the most recently presented frame of this source
    /// target, populated only while at least one mirror consumer references it.
    /// Allows cursor-only mirror updates without re-rendering the source
    /// (ADR-0250, `[INV-ARCH-160]`).
    pub(super) last_source_export: Option<output_target::RetainedSourceExport>,
    /// Lease on the last source/union image sampled by this mirror target. It
    /// keeps the source ring slot alive until the next target frame replaces it.
    pub(super) source_lease: Option<std::rc::Rc<optics_gfx::DmabufLease>>,
    /// Bounded LRU cache of imported source DMA-BUF images for mirror/capture
    /// blits, avoiding per-frame Vulkan image import and layout transitions
    /// (ADR-0250, `[INV-ARCH-160]`).
    pub(super) import_cache: output_target::TargetImportCache,
    pub(super) canvas: optics_gfx::Canvas,
    pub(super) surface: optics_gfx::Surface,
    pub(super) mode: (u32, u32),
    /// gfx format of the target surface, shared with the union framing.
    pub(super) format: optics_gfx::Format,
    pub(super) color_pipeline: tessera_desktop::output::ColorPipeline,
    /// The source connector for a mirror, or this connector for an extended
    /// output.
    pub(super) source: String,
    /// Whether this target owes at least one presentation (newly created or
    /// invalidated by VT resume / scanout exit / overlay transition). Per-slot
    /// buffer-age damage history is owned by gfx (`begin_frame_timeout_with_damage`
    /// + `frame.repaint_scope()`).
    pub(super) needs_present: bool,
    /// Last hardware-cursor bounding rect drawn into this mirror target in
    /// mirror-local physical pixels (used to localize mirror cursor damage).
    pub(super) last_mirror_cursor_rect: Option<tessera_primitives::Rect>,
}

/// A display edit applied as a preview with an automatic revert deadline.
pub(super) struct PendingDisplayRevert {
    pub(super) settings: tessera_desktop::settings::DisplaySettings,
    pub(super) deadline: std::time::Instant,
}

/// Per-output retained completion fences (ADR-0265).
///
/// Replaces the single global `last_composited_fence` so presenting or entering
/// direct scanout on one output never clobbers another output's retained
/// completion fence on multi-output systems with independent pacing (ADR-0249).
#[derive(Debug, Default)]
pub(super) struct RetainedOutputFences {
    by_output: std::collections::BTreeMap<String, tessera_platform::sync::CompletionFence>,
    unscoped: tessera_platform::sync::CompletionFence,
}

impl RetainedOutputFences {
    pub(super) fn clear(&mut self) {
        self.by_output.clear();
        self.unscoped = tessera_platform::sync::CompletionFence::Signaled;
    }

    pub(super) fn record(
        &mut self,
        outputs: Option<&[&str]>,
        fence: tessera_platform::sync::CompletionFence,
    ) {
        let fence = fence.prune_signaled();
        match outputs {
            None => {
                self.by_output.clear();
                self.unscoped = fence;
            }
            Some([]) => {}
            Some([only]) => {
                self.by_output.insert((*only).to_owned(), fence);
            }
            Some([first, rest @ ..]) => {
                let mut cloned_rest = Vec::with_capacity(rest.len());
                let mut clone_failed = false;
                for &connector in rest {
                    if let Some(copy) = fence.try_clone() {
                        cloned_rest.push((connector, copy));
                    } else {
                        clone_failed = true;
                        break;
                    }
                }
                if clone_failed {
                    // Preserve the fence unscoped rather than dropping it for
                    // sibling outputs on EMFILE.
                    let prev = std::mem::take(&mut self.unscoped);
                    self.unscoped = tessera_platform::sync::merge_completions([prev, fence]);
                } else {
                    self.by_output.insert((*first).to_owned(), fence);
                    for (connector, copy) in cloned_rest {
                        self.by_output.insert(connector.to_owned(), copy);
                    }
                }
            }
        }
    }

    /// Collect and merge the live retained fences covering `connectors` (and
    /// `unscoped` when `has_unscoped` is set or a connector has no dedicated
    /// per-output entry). Prunes signaled entries along the way.
    pub(super) fn completion_for_idle(
        &mut self,
        has_unscoped: bool,
        connectors: &[String],
    ) -> tessera_platform::sync::CompletionFence {
        let unscoped = std::mem::take(&mut self.unscoped).prune_signaled();
        let mut need_unscoped = has_unscoped;
        let mut collected = Vec::new();

        for connector in connectors {
            if let Some(entry) = self.by_output.remove(connector) {
                let pruned = entry.prune_signaled();
                if let Some(copy) = pruned.try_clone() {
                    collected.push(copy);
                    if !pruned.is_signaled() {
                        self.by_output.insert(connector.clone(), pruned);
                    }
                } else {
                    collected.push(pruned);
                }
            } else {
                need_unscoped = true;
            }
        }

        if has_unscoped {
            let keys: Vec<String> = self.by_output.keys().cloned().collect();
            for key in keys {
                if connectors.contains(&key) {
                    continue;
                }
                if let Some(entry) = self.by_output.remove(&key) {
                    let pruned = entry.prune_signaled();
                    if let Some(copy) = pruned.try_clone() {
                        collected.push(copy);
                        if !pruned.is_signaled() {
                            self.by_output.insert(key, pruned);
                        }
                    } else {
                        collected.push(pruned);
                    }
                }
            }
        }

        if need_unscoped {
            if let Some(copy) = unscoped.try_clone() {
                collected.push(copy);
                self.unscoped = unscoped;
            } else {
                collected.push(unscoped);
            }
        } else {
            self.unscoped = unscoped;
        }

        tessera_platform::sync::merge_completions(collected)
    }
}

/// Owns the mutable composition state used by the compositor event loop.
///
/// Startup builds this value once, after which event-loop phases borrow only
/// the state they need. Keeping ownership here makes those phases independently
/// testable and prevents the composition root from accumulating more locals.
pub(super) struct CompositorRuntime<'engine> {
    pub(super) notif_queue:
        std::sync::Arc<std::sync::Mutex<tessera_desktop::notify::NotificationQueue>>,
    pub(super) config_path: Option<std::path::PathBuf>,
    pub(super) config: Option<tessera_config::Config>,
    /// The scheduling plan applied to the frame loop and its background
    /// threads. Resolved once at startup from `[compositor]`; coherent across
    /// the frame/feeder/background split.
    pub(super) sched_plan: crate::runtime::sched::SchedPlan,
    /// Deadline watchdog for the frame loop, armed if `[compositor]` realtime
    /// scheduling is enabled. Observes frame stalls so a starved loop is
    /// never silent.
    pub(super) sched_watchdog: Option<crate::runtime::sched::DeadlineWatchdog>,
    // Rust drops fields in declaration order. The canvas and its Vulkan
    // surface must disappear before a nested host tears down the Wayland
    // display that owns the swapchain's wl_buffers. gfx resources retain the
    // device themselves, so keeping the explicit device handle last is safe.
    pub(super) canvas: &'engine mut optics_gfx::Canvas,
    pub(super) surface: &'engine mut optics_gfx::Surface,
    pub(super) host: &'engine mut Host,
    pub(super) device: &'engine mut optics_gfx::Device,
    /// Compositor-side blurred window shadows (ADR-0139): owns the Optics
    /// shadow filter and per-slot mask targets; renders at the frame's pass
    /// boundary, composites inside the output pass.
    pub(super) window_shadows: WindowShadowRenderer,
    /// The frosted / liquid-glass backdrop beneath chrome (ADR-0253). Holds
    /// no filter or target while the effective material is painted.
    pub(super) backdrop: super::backdrop::ChromeBackdrop,
    /// The same, for the screenshot freeze's capture frame: separate images
    /// and filter slots from the live one, so neither gains a second writer.
    pub(super) freeze_backdrop: super::backdrop::ChromeBackdrop,
    /// The frozen trigger-frame snapshot the screenshot selector samples.
    pub(super) screenshot_freeze: ScreenshotFreeze,
    pub(super) pending_capture: Option<PendingCapture>,
    pub(super) capture_worker: CaptureWorker,
    /// Live output frame streams (ADR-0052) and the in-flight flag bounding
    /// the main-loop → worker stream lane to one frame.
    pub(super) streams: OutputStreams,
    /// Per-offscreen-stream client-damage baselines (ADR-0187), keyed by
    /// `stream_id`. Window and virtual-output streams derive their frame
    /// damage from their own surface tree through these trackers, never from
    /// the physical desktop accumulator. An entry is dropped when its stream
    /// stops, freezes, or renegotiates (a rebuilt target reports full damage
    /// on its first frame).
    pub(super) offscreen_damage:
        std::collections::HashMap<u64, tessera_composite::presentation::ClientDamageTracker>,
    pub(super) stream_job_in_flight: bool,
    pub(super) cursor_cache: cursor::CursorCache,
    pub(super) screenshot_dir: std::path::PathBuf,
    pub(super) server: &'engine mut tessera_wayland::Server,
    pub(super) icon_theme: String,
    pub(super) icon_scale: u32,
    pub(super) launcher_apps: Vec<tessera_desktop::app::Entry>,
    pub(super) icon_cache: IconCache,
    pub(super) icon_snapshot: IconSnapshot,
    pub(super) shell: tessera_shell::Shell,
    pub(super) input_acc: InputAccumulator,
    /// Touchpad swipe bindings: the built-in defaults layered with the
    /// configuration's `[[gesture]]` entries, consulted when a swipe begins
    /// (ADR-0080, ADR-0082).
    pub(super) gesture_map: tessera_desktop::gesture::GestureMap,
    /// State of the in-flight compositor-owned swipe; `None` when no claimed
    /// gesture is running.
    pub(super) swipe: Option<SwipeState>,
    pub(super) renderer: &'engine mut tessera_composite::Renderer,
    pub(super) interaction_domain_processes: InteractionDomainProcesses,
    pub(super) interaction_domain_render_targets: std::collections::BTreeMap<
        tessera_authority::interaction_domain::InteractionDomainId,
        InteractionDomainRenderTarget,
    >,
    pub(super) pending_interaction_domain_capture: Option<PendingInteractionDomainCapture>,
    pub(super) pending_window_capture: Option<PendingWindowCapture>,
    pub(super) interaction_domain_damage_sequence: u64,
    pub(super) agent_activity_sequence: u64,
    pub(super) start: std::time::Instant,
    pub(super) wallpaper: Option<wallpaper::Wallpaper>,
    pub(super) clear: u32,
    pub(super) frame_count: u64,
    /// Per-output retained GPU/KMS completion fences for `NoDamage` fast-exit
    /// retired-buffer release (ADR-0250, ADR-0264, ADR-0265, `[INV-ARCH-159]`).
    pub(super) retained_output_fences: RetainedOutputFences,
    /// Content owner of the primary plane after the last successful present.
    /// Failed submissions never advance this state.
    pub(super) primary_plane_state: PrimaryPlaneState,
    /// Outputs currently owned by direct client scanouts. Other outputs keep
    /// presenting the composite; the next composite that changes one of these
    /// outputs reclaims it (ADR-0185).
    pub(super) direct_outputs: std::collections::BTreeSet<String>,
    /// Physical outputs with a mode-sized compositor target. A mirror not in
    /// this map falls back to the union framebuffer's plane scaling; an
    /// extended output without a target falls back to the union path.
    pub(super) output_targets: std::collections::BTreeMap<String, OutputTarget>,
    /// Mode for which target creation last failed, to avoid a per-frame
    /// allocation storm. A topology/mode change clears the entry.
    pub(super) output_target_failures: std::collections::BTreeMap<String, (u32, u32)>,
    /// A tentative display edit awaiting confirmation. On deadline the
    /// compositor re-applies the previous geometry without starting a new
    /// preview (ADR-0244).
    pub(super) pending_display_revert: Option<PendingDisplayRevert>,
    /// Bounded primary-plane rejection counters and rate-limited diagnostics.
    /// Kept independently from `primary_plane_state` so a persistent video client
    /// explains why it is composited instead of failing silently every frame.
    pub(super) scanout_telemetry: ScanoutTelemetry,
    /// Overlay-candidate audit counters. This records why candidates are
    /// rejected so the opt-in policy remains evidence-based (ADR-0235).
    pub(super) overlay_telemetry: tessera_composite::presentation::OverlayTelemetry,
    /// Explicit hardware-validation gate for overlay presentation. The audit
    /// planner always exists; the commit path is enabled only by
    /// `TESSERA_OVERLAY_OFFLOAD=1` until a real KMS system test has passed.
    pub(super) overlay_offload_enabled: bool,
    /// Overlay placement and transition policy. The pure state machine owns
    /// activation, buffer-only updates, replacement, and deactivation.
    pub(super) overlay_state: tessera_composite::presentation::OverlayState,
    pub(super) keyboard_capture: tessera_primitives::input::KeyboardCaptureState,
    pub(super) double_super: tessera_desktop::keybind::DoubleSuperDetector,
    pub(super) keymap: tessera_desktop::keybind::Keymap,
    pub(super) system_status: tessera_shell::component::SystemStatus,
    pub(super) network: crate::network::NetworkHandle,
    pub(super) bluetooth: crate::bluetooth::BluetoothHandle,
    pub(super) casting: crate::casting::CastHandle,
    pub(super) status_rx: std::sync::mpsc::Receiver<tessera_shell::component::SystemStatus>,
    /// Wakes the status poller for an out-of-cycle refresh after a system
    /// action, so the HUD reconciles optimistic values without the main loop
    /// ever blocking on a probe subprocess.
    pub(super) status_refresh_tx: std::sync::mpsc::Sender<()>,
    /// Handle to the serialized config-write worker. Typed edits share one
    /// path-bound store and reach the TOML file in submission order. Dock
    /// edits are fire-and-forget; settings edits wait for an accurate IPC
    /// receipt.
    pub(super) config_writer: ConfigWriter,
    pub(super) dock_state: tessera_wayland::DockStateStore,
    pub(super) dock_state_path: std::path::PathBuf,
    pub(super) reload: Option<tessera_config::ReloadWatcher>,
    /// Supervised ext-idle-notify policy client for this session.
    pub(super) idle_process: session::IdleProcess,
    pub(super) quit_requested: bool,
    pub(super) ipc_cmd_rx: std::sync::mpsc::Receiver<IpcCommandRequest>,
    pub(super) transact_rx: std::sync::mpsc::Receiver<TransactRequest>,
    pub(super) system_control_rx: std::sync::mpsc::Receiver<SystemControlRequest>,
    pub(super) capture_rx: std::sync::mpsc::Receiver<CaptureRequest>,
    pub(super) interaction_domain_control_rx:
        std::sync::mpsc::Receiver<InteractionDomainControlRequest>,
    pub(super) settings_control_rx: std::sync::mpsc::Receiver<SettingsControlRequest>,
    pub(super) wallpaper_control_rx: std::sync::mpsc::Receiver<WallpaperControlRequest>,
    pub(super) interaction_domain_capture_rx:
        std::sync::mpsc::Receiver<InteractionDomainCaptureRequest>,
    pub(super) window_capture_rx: std::sync::mpsc::Receiver<WindowCaptureRequest>,
    pub(super) interaction_domain_observe_rx:
        std::sync::mpsc::Receiver<InteractionDomainObserveRequest>,
    pub(super) actor_action_rx: std::sync::mpsc::Receiver<InteractionDomainActorActionRequest>,
    pub(super) observation_discard_rx: std::sync::mpsc::Receiver<ObservationDiscardRequest>,
    pub(super) actor_disconnect_rx: std::sync::mpsc::Receiver<u64>,
    pub(super) observations: ObservationRegistry,
    pub(super) stream_control_rx: std::sync::mpsc::Receiver<StreamControlRequest>,
    pub(super) idle_control_rx: std::sync::mpsc::Receiver<IdleControlRequest>,
    /// Interactive-pick controls from IPC connection threads (ADR-0054), the
    /// pick waiting for user interaction, and the pick kind whose chrome
    /// opens once the freeze holds.
    pub(super) pick_rx: std::sync::mpsc::Receiver<PickControlRequest>,
    pub(super) pending_pick: Option<PendingPick>,
    pub(super) pending_pick_open: Option<tessera_protocol::PickKind>,
    pub(super) app_pick_rx: std::sync::mpsc::Receiver<AppPickControlRequest>,
    pub(super) pending_app_pick: Option<PendingAppPick>,
    pub(super) secret_prompt_rx: std::sync::mpsc::Receiver<SecretPromptControlRequest>,
    pub(super) pending_secret_prompt: Option<PendingSecretPrompt>,
    pub(super) confirm_pick_rx: std::sync::mpsc::Receiver<ConfirmPickControlRequest>,
    pub(super) pending_confirm_pick: Option<PendingConfirmPick>,
    /// Destructive session actions (power off / reboot / suspend) requested
    /// by chrome during a frame, waiting to open the system-level
    /// confirmation dialog. Collected inside the render pass (where the
    /// frame's borrow forbids the `&mut self` consent call) and drained by
    /// the iteration loop right after.
    pub(super) system_confirm_requests: Vec<tessera_desktop::system::SystemAction>,
    /// A destructive session action (power off / reboot / suspend) waiting
    /// behind the system-level confirmation dialog. Chrome surfaces only
    /// ever *request* these transitions; the runtime owns the consent
    /// architecture, so the action stays parked here until the user
    /// confirms or cancels.
    pub(super) pending_system_action: Option<tessera_desktop::system::SystemAction>,
    pub(super) capability_pick_rx: std::sync::mpsc::Receiver<CapabilityPickControlRequest>,
    pub(super) pending_capability_pick: Option<PendingCapabilityPick>,
    /// Once-per-discharge-cycle memory of the low-battery warnings already
    /// shown (`battery.rs`).
    pub(super) battery_latches: tessera_desktop::system::BatteryWarningLatches,
    /// IPC connections currently holding a surfaceless idle inhibitor.
    pub(super) ipc_idle_inhibits: IdleInhibits,
    pub(super) journal:
        std::sync::Arc<std::sync::Mutex<tessera_audit::AuditLog<tessera_audit::journal::Entry>>>,
    pub(super) live: std::sync::Arc<LiveState>,
    pub(super) ipc: Option<tessera_ipc::Server>,
    pub(super) last_win_sig: Option<WindowEventSignature>,
    /// Presentation ownership resolved for the current frame (ADR-0183).
    ///
    /// `Server::space_use()` is scanned exactly once per frame, at the top of
    /// `sync_shell_and_ipc_snapshots`; every consumer in that frame (chrome,
    /// the wallpaper terms, direct-scanout eligibility) reads this field rather
    /// than re-scanning the scene. `[INV-ARCH-72]` is "one resolution", not
    /// "one resolver called from anywhere".
    pub(super) space_use: tessera_desktop::window::SpaceUse,
    /// The verdict last announced to chrome and IPC subscribers, kept only so
    /// a repeated verdict does not re-broadcast.
    pub(super) published_space_use: Option<tessera_desktop::window::SpaceUse>,
    /// Content hash of the last fanned-out window snapshot; the full
    /// `Server::windows()` clone only happens when this changes.
    pub(super) last_windows_hash: Option<u64>,
    /// Same gate for the workspace-global `Server::all_windows()` snapshot
    /// pushed to the dock.
    pub(super) last_all_windows_hash: Option<u64>,
    pub(super) last_ws_sig: Option<u64>,
    pub(super) last_interaction_domain_revision: Option<u64>,
    pub(super) last_outputs_revision: Option<u64>,
    /// Cached `atomic_domain_interval` keyed by `outputs_revision`.
    /// `presentation_interval` sits on the per-frame pacing path (render
    /// follow-up and estimated-vblank wake) and used to clone the full
    /// `Vec<OutputInfo>` twice per frame through `Server::output_infos`;
    /// outputs only change on hotplug/mode switches, so the interval is
    /// recomputed only when the revision moves.
    pub(super) cached_presentation_interval: (u64, std::time::Duration),
    /// Cached presentation-group plan keyed by output revision.
    pub(super) cached_presentation_groups:
        (u64, u64, Vec<tessera_desktop::output::PresentationGroup>),
    /// Presentation groups whose in-flight page flips are currently gating the
    /// next redraw when multiple disjoint groups exist. Empty means wait on all
    /// groups (every group busy). For single-group sessions (1 output, mirror,
    /// or same-refresh touching outputs) the scheduler always waits on that
    /// group's hardware vblank directly.
    pub(super) vblank_wait_groups: Vec<Vec<String>>,
    /// Output damage baselines and buffer-age carry-over (runtime/damage.rs).
    pub(super) damage: DamageTracking,
    /// Explicit redraw/presentation lifecycle for the host's atomic commit
    /// domain, plus input edges accumulated while a frame is in flight.
    pub(super) presentation: PresentationScheduler,
    pub(super) pending_frame: Option<FrameState>,
    pub(super) settings_revision: u64,
    pub(super) previous_agent_suspended: bool,
    pub(super) automatically_paused_interaction_domains:
        std::collections::BTreeSet<tessera_authority::interaction_domain::InteractionDomainId>,
    pub(super) animating: bool,
    pub(super) chrome_pointer_captured: bool,
    pub(super) synthetic_pointer_active: bool,
    pub(super) last_cursor_shape: u32,
    pub(super) last_cursor_hidden: bool,
    pub(super) next_app_scan: std::time::Instant,
    pub(super) scan_req_tx: std::sync::mpsc::Sender<AppScanRequest>,
    pub(super) scan_result_rx: std::sync::mpsc::Receiver<AppScanResult>,
    pub(super) previous_render_at: std::time::Instant,
    /// Night-light live state and its 1 Hz evaluation cadence
    /// (tessera_desktop::night_light; applied through the backend's GAMMA_LUT).
    pub(super) night_light: tessera_desktop::night_light::NightLight,
    pub(super) night_light_last_eval: std::time::Instant,
    /// Last probe of `Host::input_status`. The probe interrogates every
    /// libinput device (several config queries each) and allocates a
    /// `Vec<String>` of device names, so it must not run per frame — the
    /// device set only moves on hotplug, which the same event iteration
    /// observes anyway. Probed at `TOUCHPAD_PROBE_INTERVAL` instead.
    pub(super) input_status_last_probe: std::time::Instant,
    pub(super) host_system: std::sync::Arc<dyn crate::host_system::HostSystem>,
    pub(super) _preview_sandbox: Option<PreviewSandbox>,
}

pub(super) struct PreviewSandbox(pub(super) std::path::PathBuf);
impl Drop for PreviewSandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
