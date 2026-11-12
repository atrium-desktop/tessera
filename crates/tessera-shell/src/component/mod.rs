//! The Chrome contract for tessera (ADR-0021).
//!
//! This crate is the seam between the compositor's chrome host and every
//! chrome component surface. It contains exactly three things:
//!
//! 1. The [`Chrome`] component trait — what a surface must answer per frame.
//! 2. The host-owned types that flow across the seam: snapshot updates
//!    ([`ChromeUpdate`]), lifecycle commands ([`ChromeCommand`]), the
//!    interaction event sink ([`ChromeEvents`]), and the app catalog.
//! 3. Shared chrome declaration geometry: popup placement, preview-card
//!    geometry, and the shared application context menu.
//!
//! # Boundary
//!
//! The contract is `#![forbid(unsafe_code)]` and stateless: the host that
//! binds lens to the compositor's flux device lives in `tessera-shell`.
//! Component crates implement [`Chrome`] against this crate alone and must
//! never depend on the host or on each other; anything two surfaces share
//! belongs here. Design values come from `tessera-design`; this crate holds
//! no product policy of its own.

#![forbid(unsafe_code)]

mod app_menu;
pub mod popup;
pub mod preview;
mod text;

pub use app_menu::AppMenu;
pub use popup::{POPUP_GAP, POPUP_MARGIN, PopupSide, place_popup, place_popup_side};
pub use preview::{LivePreviewPresentation, PreviewCard, WindowSwitcherPresentation};
pub use text::{ellipsize, truncate};

/// Backward compatibility alias for legacy imports.
pub use tessera_i18n::{Language, Localizer, Message};

pub use tessera_desktop::settings::SettingsAction;
pub use tessera_desktop::system::ResourceStats;
pub use tessera_desktop::system::SystemAction;
pub use tessera_desktop::system::SystemStatus;

use std::collections::HashMap;
use std::os::raw::c_void;

/// Safe-area inset from the output's top and right edges for transient chrome
/// that hugs a corner — the notification toast stack.
///
/// The top of the screen belongs to windows now: the status band that used to
/// reserve it was removed with the HUD (ADR-0213), so floating corner chrome
/// anchors to a symmetric inset rather than to the tallest thing that used to
/// be there.
pub const SCREEN_CORNER_INSET: f32 = 12.0;

use tessera_authority::interaction_domain::InteractionDomainId;
use tessera_authority::interaction_domain::InteractionDomainSnapshot;
use tessera_desktop::app::ApplicationTarget;
use tessera_desktop::app::BuiltInApplication;
use tessera_desktop::app::Entry;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;

/// One successfully applied Agent input operation, projected onto trusted
/// compositor chrome for the physical user.
///
/// This is deliberately presentation-only: it grants no authority, carries
/// no key contents, and is never part of an Agent Interaction Domain capture.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentActivity {
    /// Monotonic compositor-local ordering token.
    pub sequence: u64,
    /// Interaction Domain whose independent logical seat applied the operation.
    pub interaction_domain: InteractionDomainId,
    /// Human-readable Interaction Domain label captured with the operation.
    pub interaction_domain_label: String,
    /// Toplevel that received the operation.
    pub window: tessera_desktop::window::WindowId,
    /// Applied compositor-global pointer position, when applicable.
    pub position: Option<tessera_primitives::Point>,
    /// Privacy-preserving operation class.
    pub kind: AgentInputKind,
    /// Active `wp_cursor_shape_device_v1` cursor shape of the Agent seat.
    pub cursor_shape: Option<u32>,
}

/// Visual class of a successfully applied Agent input operation.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentInputKind {
    PointerMove,
    Click {
        button: u32,
    },
    PointerButton {
        button: u32,
        state: tessera_primitives::input::ButtonState,
    },
    Scroll {
        dx: f32,
        dy: f32,
    },
    Keyboard {
        key_name: Option<String>,
    },
    Key {
        key_name: String,
        state: tessera_primitives::input::ButtonState,
    },
}

/// Edge space a chrome component reserves; tiled windows avoid it. Summed
/// across components by [`crate::Shell::reserved`] and subtracted from the tiling
/// work-area so tiles do not render under the dock or panels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reserved {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// Decoded icon texture handles shared with chrome components.
///
/// The textures are owned by the composition root's icon cache; an `IconSet`
/// is a map of borrowed raw `flux_image` pointers keyed by every lowercase id
/// an application may run as (StartupWMClass, desktop-id stem, icon name).
/// The owner must keep the textures alive until it has pushed a replacement
/// catalog via [`crate::Shell::set_app_catalog`]; components only dereference handles
/// from the most recently pushed set.
#[derive(Clone, Default)]
pub struct IconSet {
    map: HashMap<String, *mut c_void>,
    default_icon: Option<*mut c_void>,
}

impl IconSet {
    /// Wrap a raw handle map (`app_id` → borrowed `flux_image` pointer).
    pub fn from_raw(map: HashMap<String, *mut c_void>) -> IconSet {
        IconSet {
            map,
            default_icon: None,
        }
    }

    /// Wrap a raw handle map with a fallback default icon texture.
    pub fn from_raw_with_default(
        map: HashMap<String, *mut c_void>,
        default_icon: Option<*mut c_void>,
    ) -> IconSet {
        IconSet { map, default_icon }
    }

    /// The borrowed texture handle filed under `key`, if any.
    pub fn get(&self, key: &str) -> Option<*mut c_void> {
        self.map.get(key).copied()
    }

    /// The fallback default icon texture handle, if any.
    pub fn default_icon(&self) -> Option<*mut c_void> {
        self.default_icon
    }

    /// Look up an icon by key, falling back to the default icon when absent.
    pub fn get_or_default(&self, key: &str) -> Option<*mut c_void> {
        self.get(key).or(self.default_icon)
    }
}

/// One immutable snapshot of the host application catalog pushed to chrome.
#[derive(Clone, Default)]
pub struct AppCatalog {
    /// Every launchable entry: enumerated XDG applications plus
    /// compositor-owned built-ins.
    pub apps: Vec<Entry>,
    /// The user's pinned favorites, resolved against `apps` by the
    /// composition root from the `[dock] pinned` configuration (or
    /// auto-populated when unconfigured).
    pub pinned: Vec<Entry>,
    /// Decoded icons keyed by every id an entry might run as.
    pub icons: IconSet,
    /// The dock's configured screen edge from `[dock] position`. Carried on
    /// the catalog so the dock learns about live config edits through the
    /// same push that already carries its pinned list; other components
    /// ignore it.
    pub position: tessera_desktop::dock::DockPosition,
}

/// Cursor shapes chrome can request from the nested host. Values deliberately
/// mirror `wp_cursor_shape_device_v1.shape`, so the executable can pass them
/// through without maintaining a second translation table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum CursorShape {
    #[default]
    Default = 1,
    Pointer = 4,
    Crosshair = 3,
    Text = 9,
    NotAllowed = 15,
    ResizeEw = 26,
    ResizeNs = 27,
}

impl CursorShape {
    /// Map a Lens immediate-mode UI cursor hint to the corresponding Wayland cursor shape.
    pub fn from_hint(hint: lens::CursorHint) -> Option<Self> {
        match hint {
            lens::CursorHint::Default => None,
            lens::CursorHint::Pointer => Some(Self::Pointer),
            lens::CursorHint::Text => Some(Self::Text),
            lens::CursorHint::ResizeNs => Some(Self::ResizeNs),
            lens::CursorHint::ResizeEw => Some(Self::ResizeEw),
        }
    }
}

impl Reserved {
    /// Shrink `rect` by these margins, clamped so size never goes negative.
    pub fn inset(self, r: tessera_primitives::Rect) -> tessera_primitives::Rect {
        tessera_primitives::Rect {
            origin: tessera_primitives::Point {
                x: r.origin.x + self.left,
                y: r.origin.y + self.top,
            },
            size: tessera_primitives::Size {
                w: (r.size.w - self.left - self.right).max(0),
                h: (r.size.h - self.top - self.bottom).max(0),
            },
        }
    }
}

use lens::{Frame, Rect};

/// Re-export so callers can construct input snapshots without depending on
/// lens directly.
pub use lens::Input;

/// Parameters of one user-consent application pick, mapped from the IPC
/// request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct AppPickParams {
    /// Candidate desktop file ids, in the order the requester supplied.
    pub choices: Vec<String>,
    /// Human-readable context line (the file, URI, or content type the app
    /// is chosen for), shown under the title.
    pub subject: Option<String>,
    /// The previously used app id, pre-highlighted when still a candidate.
    pub last_choice: Option<String>,
}

/// Parameters of one user-consent secret prompt, mapped from the IPC
/// request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct SecretPromptParams {
    /// Prompt heading (e.g. "Unlock Keyring").
    pub title: String,
    /// Optional context line under the title.
    pub reason: Option<String>,
}

/// The dialog style: a plain yes/no consent, or the four-option
/// runtime-grant consent (ADR-0088).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmPickStyle {
    /// Cancel plus one affirmative button (portal consent flows).
    #[default]
    YesNo,
    /// Deny / Allow once / This session / Always (agent runtime grants).
    Grant,
}

/// The answer the user gave at the confirmation dialog. The yes/no style
/// only ever yields `Confirmed`/`Cancelled`; the grant style yields
/// `Cancelled` or one of the three persistence levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAnswer {
    /// Yes/no style: the affirmative button (or Enter).
    Confirmed,
    /// Either style: Deny/Cancel, `Escape`, or the compositor panic chord.
    Cancelled,
    /// Grant style: allow this one operation only; nothing is recorded.
    AllowOnce,
    /// Grant style: allow until the compositor exits.
    AllowSession,
    /// Grant style: allow and remember durably.
    AllowAlways,
}

/// Parameters of one user-consent confirmation, mapped from the IPC
/// request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct ConfirmPickParams {
    /// Dialog heading (e.g. "Share personal information?").
    pub title: String,
    /// Explanation of what is requested and by whom.
    pub body: String,
    /// Affirmative button label override ("Allow", "Share", …); the
    /// default is "OK". Only used by the yes/no style.
    pub accept_label: Option<String>,
    /// Dialog style; the default is the plain yes/no consent.
    pub style: ConfirmPickStyle,
}

/// Parameters of one capability-borrowing checklist, mapped from the agent
/// pairing request by the compositor runtime.
#[derive(Debug, Clone)]
pub struct CapabilityPickParams {
    /// Dialog heading (e.g. "Codex wants to borrow desktop capabilities").
    pub title: String,
    /// Look-alike installation warning (ADR-0088 TOFU continuity), shown as
    /// a highlighted row under the title when present.
    pub warning: Option<String>,
    /// One row per requested capability family, in display order.
    pub families: Vec<CapabilityFamily>,
}

/// One checkable capability family row; its members are the expandable
/// per-operation detail.
#[derive(Debug, Clone)]
pub struct CapabilityFamily {
    /// Stable machine key, unique within one checklist.
    pub key: String,
    /// Human-readable family description (e.g. "Control windows").
    pub label: String,
    /// The requested operations in this family, in display order.
    pub members: Vec<CapabilityGroup>,
}

/// One checkable operation row inside a family.
#[derive(Debug, Clone)]
pub struct CapabilityGroup {
    /// Stable machine key the runtime maps back to an operation family.
    pub key: String,
    /// Human-readable capability description (e.g. "Focus windows").
    pub label: String,
    /// High-risk operation: first use is confirmed again interactively.
    pub gated: bool,
    /// Initially checked.
    pub enabled: bool,
}

/// The user's answer: the checked operation keys on Allow, or `None` on
/// Deny, `Escape`, or the compositor panic chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityPickResult {
    pub approved: Option<Vec<String>>,
}

/// What a transient system OSD is reporting.
///
/// The kind decides the reading and the glyph, never the placement: every OSD
/// appears in the same place with the same lifetime, so a user learning one has
/// learned them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdKind {
    /// The battery crossed a level worth mentioning. `critical` upgrades the
    /// wording and the tone; it is not a request for confirmation.
    Battery,
    /// A wireless link came up, went down, or changed network.
    Wireless,
    /// A Bluetooth peripheral connected or disconnected.
    Bluetooth,
    /// Playback moved to another audio output (ADR-0215).
    Output,
    /// Output volume moved or was muted.
    Volume,
    /// Output brightness moved.
    Brightness,
}

impl OsdKind {
    /// Whether this kind's reading is a percentage (drives the gauge).
    #[must_use]
    pub fn is_level(self) -> bool {
        matches!(
            self,
            OsdKind::Battery | OsdKind::Volume | OsdKind::Brightness
        )
    }
}

/// One transient system OSD.
///
/// `level` is a percentage for a level kind and `None` otherwise. `critical`
/// is the tone escalation for a battery reading close to the end of the
/// session; it never becomes a barrier, because the barrier is
/// [`BatteryAlertParams`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsdParams {
    pub kind: OsdKind,
    /// Short headline, already localized by the caller.
    pub title: String,
    /// Optional second line (a network name, a device name).
    pub detail: Option<String>,
    /// Percentage for a level kind.
    pub level: Option<u8>,
    /// Escalated tone for a battery reading close to the end of the session.
    pub critical: bool,
}

/// A low-battery barrier: the one battery surface that stops the session.
///
/// Raised only for the lowest configured `[battery] warn_at` threshold, where
/// the machine is about to suspend. Softer thresholds go to the transient OSD
/// instead, so a working user is not interrupted to be told something a glance
/// could have told them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryAlertParams {
    /// Charge level shown in the panel, in percent.
    pub percent: u8,
    /// The lowest configured threshold fired: the alert uses the critical
    /// wording and the rejection-red fill.
    pub critical: bool,
}

/// The interaction a portal picker session asks for (ADR-0054).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// Drag out a screen region (the Print-key interaction).
    Region,
    /// Click one screen point (colour picking).
    Pixel,
    /// Click a window, or choose the whole output.
    Window,
    /// Click an output to pick it by connector (version 29, ADR-0128).
    Output,
}

/// Dock-specific pin/unpin action carried by the shared application menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinAction {
    Pin(String),
    Unpin(String),
}

/// One ordered window-management action emitted by compositor chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// Focus and raise a window; focusing a minimized window restores it.
    Focus(tessera_desktop::window::WindowId),
    /// Hide a window while keeping its client and buffers alive.
    Minimize(tessera_desktop::window::WindowId),
    /// Set or clear compositor-managed maximization.
    SetMaximized(tessera_desktop::window::WindowId, bool),
    /// Set or clear the compositor-internal always-on-top flag.
    SetAlwaysOnTop(tessera_desktop::window::WindowId, bool),
    /// Ask a client to close one of its toplevels gracefully.
    Close(tessera_desktop::window::WindowId),
}

/// The mirror guard's drag on a read-only mirror became a move request.
///
/// Position is presentation state, so the human may rearrange a mirror even
/// though content input, focus, resize, and close stay barred. `cursor` is
/// the physical pointer position when the drag threshold was crossed; the
/// compositor uses it as the grab origin because pointer motion is not
/// forwarded to the server while the guard captures the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MirrorMove {
    /// Mirror window to move.
    pub window: tessera_desktop::window::WindowId,
    /// Physical cursor position at drag-recognition time.
    pub cursor: (f32, f32),
}

/// Trusted Interaction Domain-management intent emitted by compositor-owned chrome.
///
/// The shell never mutates compositor authority directly. The main loop
/// translates these values into the same optimistic Interaction Domain transactions used
/// by IPC clients, preserving one validation and commit path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionDomainIntent {
    TransferWindow {
        window: tessera_desktop::window::WindowId,
        target: InteractionDomainId,
        retain_source_as_observer: bool,
        expected_revision: u64,
    },
}

/// Canonical panel event sink alias (ADR-0155).
pub type PanelEvents = ChromeEvents;

/// Interaction intents chrome components emit during a frame. The core
/// collects these and the main loop drains them into server window-management
/// actions or, for [`ChromeEvents::spawn`], into `tessera-launch`. Scalar intents
/// keep the latest value; application menus use an ordered queue for
/// multi-window actions.
#[derive(Debug, Default)]
pub struct ChromeEvents {
    /// The chrome requested the session to quit.
    pub quit: bool,
    /// A component changed its own presentation state in a way no host-owned
    /// snapshot describes — a dock tile's context menu opening or closing, a
    /// transient popup appearing. The host must rebuild the chrome scene and
    /// repaint the popup's footprint on this frame.
    ///
    /// Distinct from [`Chrome::anim_pending`]: that keeps the frame *clock*
    /// running for an easing value, while this is a one-shot structural change
    /// that produces no animation and no snapshot revision. Without it a popup
    /// that opens and closes without animating never reaches the output.
    pub scene_changed: bool,
    /// The chrome asked to lock the session immediately (the command panel's
    /// Lock now row). Drained by the main loop into the same lock path as the
    /// Super+L binding.
    pub lock: bool,
    /// A dock reading row asked to open the control center (ADR-0218).
    ///
    /// Deliberately **open-only**, unlike [`ChromeCommand::ToggleControlCenter`]:
    /// a reading is a shortcut into the surface that owns the reading, and a
    /// press that closed the panel the user is already looking at would be a
    /// toggle wearing a shortcut's clothes. The main loop answers it through
    /// [`crate::Shell::open_control_center`].
    pub open_control_center: bool,
    /// Window id a component asked to focus/activate.
    pub clicked: Option<tessera_desktop::window::WindowId>,
    /// Ordered window actions emitted by popup menus. A queue allows an
    /// application-level action such as "close all windows" to preserve one
    /// journal entry per affected toplevel.
    pub window_actions: Vec<WindowAction>,
    /// A desktop entry the chrome asked to launch (e.g. the launcher's
    /// clicked row). Drained into `tessera-launch-services` by the main loop; carrying the
    /// full [`Entry`] keeps `tessera-shell` free of any `tessera-launch-services` dependency
    /// (ADR-0022, ADR-0164).
    pub spawn: Option<Entry>,
    /// A trusted compositor-owned application to present. Built-ins share the
    /// launcher catalog with external apps but never pass through a shell
    /// command or process boundary.
    pub open_builtin: Option<BuiltInApplication>,
    /// A workspace the chrome asked to switch to (the workspace bar's clicked
    /// tile). Drained into `Server::switch_workspace_to` by the main loop
    /// (ADR-0025).
    pub switch_workspace: Option<tessera_desktop::workspace::WorkspaceId>,
    /// The chrome asked to toggle the Pivot system intent surface this frame
    /// (e.g. clicking the dock's Pivot tile). Drained by the main loop.
    pub toggle_pivot: bool,
    /// Text to copy to the system clipboard (e.g. Pivot calculation result).
    pub clipboard_copy: Option<String>,
    /// Natural-language prompt dispatched from Pivot to the Agent Broker.
    pub agent_prompt: Option<String>,
    /// Notification id the toast stack asked to dismiss. Drained through the
    /// same command/journal path as an IPC dismissal.
    pub dismissed_notification: Option<u64>,
    /// Window id the overview asked to focus this frame (a thumbnail click).
    /// Drained through the focus command/journal path; picking also closes
    /// the overview.
    pub overview_pick: Option<tessera_desktop::window::WindowId>,
    /// Window id clicked in the held-modifier switcher.
    pub window_switcher_pick: Option<tessera_desktop::window::WindowId>,
    /// A mirror-guard drag asked the compositor to move a read-only mirror
    /// window this frame.
    pub mirror_move: Option<MirrorMove>,
    /// The switcher was dismissed by clicking outside its cards.
    pub window_switcher_cancel: bool,
    /// Workspace id the overview's rail asked to switch to. Drained through
    /// the same command/journal path as `SwitchWorkspaceTo`.
    pub overview_switch: Option<tessera_desktop::workspace::WorkspaceId>,
    /// Region the screenshot selector asked to capture this frame, if any.
    pub screenshot_region: Option<tessera_primitives::Rect>,
    /// Point the pixel picker was clicked at this frame (ADR-0054), in
    /// compositor logical pixels.
    pub picked_point: Option<tessera_primitives::Point>,
    /// Window id the window picker was clicked on this frame (ADR-0054).
    pub picked_window: Option<tessera_desktop::window::WindowId>,
    /// The window-picker user chose the whole output instead of a window:
    /// Enter/Space, or a click on empty desktop (ADR-0054).
    pub pick_output: bool,
    /// Connector the output picker was clicked on this frame (version 29,
    /// ADR-0128). The main loop answers the waiting `PickTarget` request
    /// with it.
    pub picked_output: Option<String>,
    /// The user dismissed an IPC picker session without picking (Escape, or
    /// a confirm with no staged region). The main loop answers the waiting
    /// request with a cancellation.
    pub pick_cancelled: bool,
    /// The desktop file id the app picker confirmed this frame (the
    /// AppChooser portal's compositor side). The main loop answers the
    /// waiting `PickApp` IPC request with it.
    pub app_pick_confirmed: Option<String>,
    /// The user dismissed the app picker without confirming.
    pub app_pick_cancelled: bool,
    /// The secret value the user confirmed at the secret prompt this frame
    /// (the vault password unlock's compositor side). The main loop answers
    /// the waiting `PromptSecret` IPC request with it.
    pub secret_prompt_confirmed: Option<String>,
    /// The user dismissed the secret prompt without confirming.
    pub secret_prompt_cancelled: bool,
    /// The answer the user gave at the confirmation dialog this frame
    /// (portal consent flows, or the ADR-0088 runtime-grant consent). The
    /// main loop answers the waiting `PickConfirm` IPC request or grant
    /// request with it.
    pub confirm_pick_answered: Option<ConfirmAnswer>,
    /// The checklist answer the user gave at the capability-borrowing
    /// dialog this frame (ADR-0088 agent pairing). The main loop answers
    /// the waiting `PairAgent` IPC request with it.
    pub capability_pick_answered: Option<CapabilityPickResult>,
    /// Ordered host-system mutations requested by compositor-owned UI.
    pub system_actions: Vec<SystemAction>,
    /// Persistent-settings mutations requested by compositor-owned UI; the
    /// `Option` is the expected snapshot revision for optimistic concurrency.
    pub settings_actions: Vec<(Option<u64>, tessera_desktop::settings::SettingsAction)>,
    /// Ordered, idempotent pin mutations requested by application menus.
    /// Drained by the main loop, which updates `[dock] pinned` in the config
    /// and refreshes the dock catalog.
    pub dock_pin_actions: Vec<PinAction>,
    /// The complete pinned order the dock committed this frame when the user
    /// finished dragging a tile to a new slot (entry ids in dock order).
    /// Drained by the main loop into `DockStateStore`, like pin
    /// actions; the dock has already applied the order optimistically, and
    /// the resulting catalog push reconciles it.
    pub dock_reorder: Option<Vec<String>>,
    /// The screen edge the user dragged the dock to this frame. Drained by
    /// the main loop into `DockStateStore`; the dock has
    /// already switched edges optimistically.
    pub dock_position: Option<tessera_desktop::dock::DockPosition>,
    /// Ordered Interaction Domain lifecycle and authority mutations requested by trusted
    /// shell surfaces.
    pub interaction_domain_intents: Vec<InteractionDomainIntent>,
}

impl ChromeEvents {
    /// Activate one catalog entry through its declared target.
    pub fn activate_entry(&mut self, entry: Entry) {
        match entry.target {
            ApplicationTarget::External => self.spawn = Some(entry),
            ApplicationTarget::BuiltIn(app) => self.open_builtin = Some(app),
        }
    }
}

/// Discrete lifecycle request broadcast by the shell host.
///
/// Components match only the variants they own, keeping the base [`Chrome`]
/// trait independent of every built-in application's control surface.
#[derive(Debug)]
pub enum ChromeCommand<'a> {
    TogglePivot,
    ClosePivot,
    OpenBuiltIn(BuiltInApplication),
    ToggleOverview,
    CloseOverview,
    ToggleControlCenter,
    CloseControlCenter,
    StartWindowSwitcher,
    FinishWindowSwitcher,
    StartPick(PickerMode),
    CancelPick,
    StartAppPick(&'a AppPickParams),
    CancelAppPick,
    StartSecretPrompt(&'a SecretPromptParams),
    CancelSecretPrompt,
    StartConfirmPick(&'a ConfirmPickParams),
    CancelConfirmPick,
    StartCapabilityPick(&'a CapabilityPickParams),
    CancelCapabilityPick,
    StartBatteryAlert(&'a BatteryAlertParams),
    CancelBatteryAlert,
    /// Raise a transient system OSD. Repeated commands replace the current one
    /// rather than stacking: two volume readings in a row are one reading, not
    /// two panels.
    ShowOsd(&'a OsdParams),
    /// Force every active high-priority modal overlay to its safe-negative
    /// exit (the compositor panic chord, `Ctrl+Alt+Escape`). Consent prompts
    /// answer denied/cancelled through their normal [`ChromeEvents`] answer
    /// events — unlike the per-dialog `Cancel*` force-closes above, which
    /// emit no answer — so waiting IPC requests resolve exactly as if the
    /// user had pressed `Escape`. Non-modal components ignore it.
    DismissModal,
}

/// Borrowed host snapshot or presentation-policy update.
///
/// Components that retain an update clone only the variant they consume.
#[derive(Clone, Copy)]
pub enum ChromeUpdate<'a> {
    SystemStatus(&'a SystemStatus),
    ResourceStats(&'a ResourceStats),
    InteractionDomains(&'a InteractionDomainSnapshot),
    AgentActivity(&'a AgentActivity),
    AppCatalog(&'a AppCatalog),
    Windows(&'a [Window]),
    /// Every mapped toplevel across all workspaces (the global counterpart of
    /// [`ChromeUpdate::Windows`], which carries only the visible set). Only
    /// components whose strip is workspace-global — the dock — consume this.
    AllWindows(&'a [Window]),
    /// Device-pixel (HiDPI) scale of the output the chrome renders on, so a
    /// component can snap hairline geometry to the device pixel grid.
    Scale(f32),
    /// The server-resolved space use of the presentation domain the chrome
    /// draws on (ADR-0183). Components consume this verdict instead of
    /// inferring it from [`ChromeUpdate::Windows`]: a window that covers the
    /// output without declaring `xdg_toplevel.set_fullscreen` owns the
    /// presentation exactly like one that did, and only the server can resolve
    /// that (it owns surface geometry, declared opacity, and output layout).
    ///
    /// The verdict is desktop-global, because chrome is one canvas with no
    /// per-output layout; per-output ownership is ADR-0185.
    SpaceUse(tessera_desktop::window::SpaceUse),
    /// The shell-wide design snapshot for the resolved desktop color scheme.
    /// Components that paint from design tokens retain a copy instead of
    /// constructing one inline; seeded by [`crate::Shell::add`] and broadcast by
    /// [`crate::Shell::set_color_scheme`].
    Appearance(&'a tessera_design::Design),
    /// The persistent-settings snapshot, seeded by [`crate::Shell::add`] and
    /// broadcast by [`crate::Shell::set_settings`]. Only chrome hosting settings UI
    /// consumes it.
    Settings(&'a tessera_desktop::settings::SettingsSnapshot),
    ReducedMotion(bool),
    ModalReserved(Reserved),
    /// Whether the full-screen application launcher currently owns the
    /// output. Pushed every frame alongside [`ChromeUpdate::ModalReserved`]:
    /// the launcher blurs and dims the complete desktop, so persistent
    /// decorations that would float inside its field (the HUD chips) hide
    /// for its duration, while the dock below its work area stays.
    LauncherActive(bool),
}

/// Canonical panel component trait alias (ADR-0155).
pub use Chrome as Panel;

/// One piece of compositor chrome.
///
/// A component renders itself for one frame from the shared window and
/// workspace snapshots and the input, drawing through `frame` and pushing
/// any user intents into `out`. The core owns the lens context, the
/// snapshots, and the sink; the component owns only its own appearance and
/// state. Register implementations with [`crate::Shell::add`].
pub trait Chrome {
    /// Draw the component for this frame. Called inside the core's
    /// `Ui::frame` envelope, so `frame` is a live builder. `workspaces` is
    /// the live workspace/output snapshot (ADR-0025); components that don't
    /// care ignore it.
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    );

    /// Whether this component currently captures new key sequences (e.g. an
    /// open launcher). When any registered component returns true, the main
    /// loop routes each new press and its matching release through chrome
    /// instead of the focused client. This routing policy does not change the
    /// Wayland keyboard or text-input focus. Default `false`; override in
    /// components that capture text input.
    fn captures_keyboard(&self) -> bool {
        false
    }

    /// Handle one resolved key event while [`Chrome::captures_keyboard`] is
    /// true. Default no-op; override to consume typed input (the launcher's
    /// search box).
    fn key_char(&mut self, _kc: &tessera_primitives::input::KeyChar, _out: &mut ChromeEvents) {}

    /// Receive a discrete host lifecycle command.
    fn command(&mut self, _command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {}

    /// Receive a host-owned snapshot or presentation-policy update.
    fn update(&mut self, _update: ChromeUpdate<'_>) {}

    /// Whether this component is the Pivot system intent and action surface
    /// and is currently open or animating.
    fn pivot_active(&self) -> bool {
        false
    }

    /// Whether this component owns pointer input at the given output-space
    /// position. The main loop uses this before client routing so clicks on
    /// overlays never fall through to a window underneath them.
    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        false
    }

    /// Whether this component temporarily owns the chrome presentation band.
    /// While a modal component is active, the shell skips ordinary components
    /// so visually covered controls cannot still respond to pointer input.
    ///
    /// A modal component owns the session until it is dismissed, so it must
    /// always offer a way out: render an explicit dismissal affordance (a
    /// button), map `Escape` to its safe-negative answer in
    /// [`Chrome::key_char`], and honor [`ChromeCommand::DismissModal`] — the
    /// compositor panic chord, `Ctrl+Alt+Escape` — with that same answer.
    /// High-priority consent and alert dialogs must not dismiss on clicks
    /// outside the panel: an accidental click must never silently answer a
    /// security question.
    fn modal_active(&self) -> bool {
        false
    }

    /// Whether this component temporarily owns the complete chrome band.
    ///
    /// Full-output presentations such as the window switcher and command
    /// panel suppress every other component, including persistent HUD and
    /// Dock decorations, until their exit animation has finished.
    fn exclusive_presentation_active(&self) -> bool {
        self.window_switcher_active()
    }

    /// Whether this component is part of the persistent desktop decoration
    /// layer. Persistent decorations stay present above ordinary overlays such
    /// as Prism and the launcher. An exclusive presentation such as the window
    /// switcher may still suppress the whole decoration layer temporarily.
    fn persistent_decoration(&self) -> bool {
        false
    }

    /// Resting tile-icon rectangles for every running window, in output
    /// coordinates — the compositor's minimize-animation flight targets
    /// (ADR-0029). Only components with a window tile strip (the dock)
    /// contribute; the default is a no-op.
    fn minimize_targets(
        &self,
        _display: (f32, f32),
        _out: &mut Vec<(tessera_desktop::window::WindowId, tessera_primitives::Rect)>,
    ) {
    }

    /// Whether this component remains visible and interactive while another
    /// component is modal. Modal components opt in themselves; persistent
    /// decorations share the default opt-in through
    /// [`Chrome::persistent_decoration`].
    fn visible_during_modal(&self) -> bool {
        self.persistent_decoration()
    }

    /// Cursor shape to use while this component owns the pointer at `(x, y)`.
    /// Return `None` when the component only captures input and the cursor
    /// presentation should remain unchanged. The shell asks only after
    /// [`Chrome::captures_pointer`] returned true.
    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        None
    }

    /// Whether the component's control center is currently open.
    /// Default `false`.
    fn control_center_active(&self) -> bool {
        self.command_panel_active()
    }

    /// Whether the component's command panel is currently open.
    /// Default `false`.
    fn command_panel_active(&self) -> bool {
        false
    }

    /// Whether the component's overview mode is currently open. The main
    /// loop swaps the desktop scene for the overview thumbnail grid while
    /// this holds or the reveal animation is still running (see
    /// [`Chrome::overview_progress`]). Default `false`.
    fn overview_active(&self) -> bool {
        false
    }

    /// The component's overview reveal progress (0 = hidden, 1 = fully
    /// open). The compositor samples it at the start of the frame — before
    /// chrome render advances the animation — and feeds it to the thumbnail
    /// pass so windows fly in from their real positions instead of popping
    /// onto the grid; chrome geometry must therefore track the pre-advance
    /// value. Default `0`.
    fn overview_progress(&self) -> f32 {
        0.0
    }

    /// Advance and cache the switcher's shared live-preview layout.
    ///
    /// Only the switcher component returns a presentation. The shell calls
    /// this before the compositor paints client previews, then the component
    /// reuses the same snapshot during its chrome render.
    fn prepare_window_switcher(
        &mut self,
        _input: &Input,
        _display: tessera_primitives::Rect,
        _windows: &[Window],
        _order: &[tessera_desktop::window::WindowId],
        _selected: Option<tessera_desktop::window::WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        None
    }

    /// Return a live client-preview popover prepared by this component.
    /// Geometry is read after [`Chrome::prepare_frame`] and shared by the
    /// compositor preview pass, shell rendering, and pointer hit-testing.
    fn live_preview_presentation(&self) -> Option<LivePreviewPresentation> {
        None
    }

    /// Whether the Super+Tab preview strip is currently active.
    fn window_switcher_active(&self) -> bool {
        false
    }

    /// Test-only: the switcher's animation state, for end-to-end probes.
    #[cfg(test)]
    fn window_switcher_probe(
        &self,
    ) -> Option<(
        bool,
        bool,
        Option<tessera_desktop::window::WindowId>,
        f32,
        bool,
    )> {
        None
    }

    /// Whether the screenshot region selector is currently active. Default
    /// `false`; the screenshot selector overrides this.
    fn screenshot_active(&self) -> bool {
        false
    }

    /// Whether the app picker is currently open. Default `false`; the
    /// app-picker component overrides this.
    fn app_pick_active(&self) -> bool {
        false
    }

    /// Whether the secret prompt is currently open. Default `false`; the
    /// secret-prompt component overrides this.
    fn secret_prompt_active(&self) -> bool {
        false
    }

    /// Whether the confirmation dialog is currently open. Default `false`;
    /// the confirmation component overrides this.
    fn confirm_pick_active(&self) -> bool {
        false
    }

    /// Whether the capability checklist is currently open. Default `false`;
    /// the capability-prompt component overrides this.
    fn capability_pick_active(&self) -> bool {
        false
    }

    /// Whether a transient system OSD is currently visible. Default `false`;
    /// the OSD component overrides this.
    fn osd_active(&self) -> bool {
        false
    }

    /// Whether the low-battery alert is currently open. Default `false`; the
    /// battery-alert component overrides this.
    fn battery_alert_active(&self) -> bool {
        false
    }

    /// Advance animation clocks and query watchers whose results must be
    /// known before [`Chrome::damage_regions`] and the cover declarations are
    /// resolved for this frame (ADR-0171, ADR-0178).
    fn prepare_frame(
        &mut self,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
    }

    /// Whether this component's rendered surface completely occludes the
    /// underlying desktop wallpaper and client windows at full opacity this
    /// frame (e.g. an open Control Center once its opaque canvas reaches
    /// `alpha == 255`). Allows the compositor to skip the redundant base
    /// desktop pass.
    fn occludes_desktop(&self) -> bool {
        false
    }

    /// Edge space this component reserves; tiled windows avoid it (ADR-0024).
    /// Default none; overridden by chrome that should not be covered (the
    /// dock reserves the bottom edge). Summed by [`crate::Shell::reserved`].
    fn reserved(&self) -> Reserved {
        Reserved::default()
    }

    /// Whether this component has a multi-frame animation in flight (a dock
    /// spring still settling, a fade mid-transition, …). When any registered
    /// component returns true the main loop keeps ticking frames instead of
    /// blocking on the host event queue, so the animation can advance even
    /// when the pointer is still. Default `false`; override in components that
    /// run their own easing.
    fn anim_pending(&self) -> bool {
        false
    }

    /// Whether this component owes a frame whose purpose is **damage repair**
    /// rather than motion: a bounded retirement window for pixels it just
    /// removed (ADR-0227).
    ///
    /// This is deliberately separate from [`Chrome::anim_pending`]. An
    /// animation predicate describes a value that is still travelling and can
    /// legitimately keep the output at the panel rate; a retirement window is a
    /// fixed budget that must go false by itself. The host uses it to keep
    /// frames arriving and to include the component in
    /// [`Chrome::damage_regions`] without turning the repair into an
    /// unbounded animation.
    fn damage_pending(&self) -> bool {
        false
    }

    /// The output-logical **regions** this component's paint touches this frame,
    /// for hosts that convert a change into a partial-output repaint instead of a
    /// full composite.
    ///
    /// A list, not a rectangle: a component can touch unrelated areas at once
    /// (a strip at the top and a popup above it, a tile and the menu it owns), and
    /// collapsing them into one box asks the host to repaint everything between
    /// them. The compositor's damage pipeline is a list for the same reason.
    ///
    /// Contract:
    /// * Consulted while [`Chrome::anim_pending`] is true, and on any frame where
    ///   the component reported [`ChromeEvents::scene_changed`]. A quiescent
    ///   component's regions are never read.
    /// * The regions must cover every pixel the component may change between the
    ///   previous presented frame and the next — including the area vacated by a
    ///   moving element, plus any soft shadow/blur halo.
    /// * Returning `None` is always safe: the host falls back to a full-output
    ///   repaint. Third-party components should keep the `None` default unless
    ///   they can state their footprint.
    ///
    /// Damage reported this way has to be **re-reported** on every frame the
    /// element keeps moving, because each partial repaint consumes the previous
    /// frame's regions.
    fn damage_regions(&self, _windows: &[Window], _display: (f32, f32)) -> Option<Vec<Rect>> {
        None
    }

    /// The output-logical regions a **pointer move** to `(x, y)` can repaint,
    /// or `None` when this component cannot bound its own hover reaction.
    ///
    /// Only consulted for a component that answers
    /// [`Chrome::captures_pointer`] at that position: the question is "what may
    /// this component redraw because the pointer is now here", and a component
    /// that does not take the pointer has no answer to give.
    ///
    /// The default is [`None`] — a full-output repaint. That is the correct
    /// conservative answer for a component whose hover feedback lands on pixels
    /// it does not already localize, and it must stay the default: inventing a
    /// band here is exactly the guess this seam exists to remove. A component
    /// that already answers [`Chrome::damage_regions`] with a real footprint
    /// usually knows this one too, and the components that do answer keep the
    /// cost of a mouse movement proportional to the surface it moves over
    /// instead of to the output.
    fn pointer_damage_regions(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        None
    }

    /// Whether this component would draw visible pixels in the next frame.
    /// The default is conservative: third-party chrome blocks direct scanout
    /// until it explicitly proves that its inactive state is visually empty.
    fn requires_composition(&self) -> bool {
        true
    }

    /// The exact time until this component next needs a frame *without*
    /// animating — a timed state change (a toast leaving its window) that
    /// produces no queue event. Returning `Some(d)` lets the compositor
    /// schedule a single wake at that deadline instead of pumping the frame
    /// loop at the output refresh rate until the change happens. Components
    /// whose visuals only move while [`Chrome::anim_pending`] is true leave
    /// this `None`.
    fn animation_deadline_in(&self) -> Option<std::time::Duration> {
        None
    }

    /// Whether this component's paint is a **full-output opaque cover** whose
    /// own body establishes the frame's background this frame — the launcher
    /// sheet, the control center's canvas, a modal scrim.
    ///
    /// A cover has two consequences the compositor consumes instead of
    /// guessing:
    /// * the base desktop pass is elidable, because no pixel of it survives;
    /// * an exit fade has nothing beneath it to reveal, so the cover owns its
    ///   own fade-out through [`Chrome::cover_opacity`].
    ///
    /// Default `false`. A component that declares `true` must actually paint
    /// every output pixel at full reveal; declaring it while leaving gaps
    /// exposes stale framebuffer content underneath.
    fn paints_opaque_cover(&self) -> bool {
        false
    }

    /// Opacity of [`Chrome::paints_opaque_cover`] this frame, `[0, 1]`.
    ///
    /// Chrome that covers the whole output has to drain its cover while it
    /// leaves, or it holds the desktop hostage at full strength and then
    /// vanishes in a single frame. The compositor reads this to decide whether
    /// the desktop beneath the cover still has to be drawn. Meaningless when
    /// [`Chrome::paints_opaque_cover`] is `false`.
    fn cover_opacity(&self) -> f32 {
        0.0
    }
}

/// Exact compositor-owned output requirements for primary-plane policy.
///
/// This deliberately describes what would affect the *next rendered frame*,
/// rather than broad component state such as "the Dock exists" or "an
/// animation timer is running". A hidden/fully transparent component must not
/// prevent direct scanout, while any visible chrome pixel must.
///
/// There is one field, not two. A `live_scene_effect` flag used to sit beside
/// `visible_pixels` so chrome that sampled a compositor-owned blur could keep
/// the desktop pass alive. Chrome draws no material any more
/// ([ADR-0213](../../../docs/adr/0213-flat-chrome-and-removal-of-the-status-hud.md)),
/// so no component ever declared one: the flag was `false` by construction,
/// its scanout rejection unreachable, and its "reason" string undocumentable
/// truth. The blurred window shadow that motivated it is a scene pass Tessera
/// always records, not a condition chrome reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompositionRequirements {
    /// At least one eligible chrome component would draw visible pixels.
    pub visible_pixels: bool,
}

#[cfg(test)]
mod tests {

    /// A modal's veil is one painted fill covering the whole output. It has no
    /// material beneath it to blend into, so this pins the geometry the
    /// compositor elides the desktop pass on.
    #[test]
    fn modal_scrim_paints_the_whole_output_at_the_veil_tone() {
        for design in [
            tessera_design::Design::dark(),
            tessera_design::Design::light(),
        ] {
            let material = tessera_design::materials::modal_scrim(&design, 1.0);
            assert_eq!(material.bg, design.colors.modal_scrim);
            assert_eq!(material.radius, 0.0, "a full-output veil has no corners");
            assert_eq!(material.border_width, 0.0);
            assert_eq!(material.pad, 0.0);
            let veil = material.bg.components().3;
            assert!(veil > 0, "the veil carries a visible wash");
            assert!(
                material_alpha_at(&design, 0.5) < veil,
                "the veil follows the cover's fade"
            );
        }
    }

    /// Alpha of the veil at a given cover opacity, through the material's own
    /// premultiplied colour.
    fn material_alpha_at(design: &tessera_design::Design, opacity: f32) -> u8 {
        tessera_design::materials::modal_scrim(design, opacity)
            .bg
            .components()
            .3
    }
}
