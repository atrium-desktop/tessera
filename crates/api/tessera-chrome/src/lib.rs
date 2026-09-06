//! The Chrome contract for tessera (ADR-0021).
//!
//! This crate is the seam between the compositor's chrome host and every
//! chrome component surface. It contains exactly three things:
//!
//! 1. The [`Chrome`] component trait — what a surface must answer per frame.
//! 2. The host-owned types that flow across the seam: snapshot updates
//!    ([`ChromeUpdate`]), lifecycle commands ([`ChromeCommand`]), the
//!    interaction event sink ([`ChromeEvents`]), and the app catalog.
//! 3. Shared chrome declaration geometry: backdrop regions and the
//!    analytic liquid-glass graph, popup placement, preview-card geometry,
//!    and the shared application context menu.
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
pub use popup::{place_popup, place_popup_side, PopupSide, POPUP_GAP, POPUP_MARGIN};
pub use preview::{LivePreviewPresentation, PreviewCard, WindowSwitcherPresentation};
pub use text::{ellipsize, truncate};

pub use tessera_i18n::{Language, Localizer, Message};

pub use tessera_model::settings::SettingsAction;
pub use tessera_model::system::{ResourceStats, SystemAction, SystemStatus};

use std::collections::HashMap;
use std::os::raw::c_void;

/// Logical height of the HUD chips (the `tessera-hud` component).
/// Defined here, at the shell seam, so shell-resident chrome that must align
/// with the chips (the notification toast stack) can share the value without
/// depending on the component crate.
pub const HUD_HEIGHT: f32 = 32.0;

use tessera_model::app::{ApplicationTarget, BuiltInApplication, Entry};
use tessera_model::interaction_domain::{InteractionDomainId, InteractionDomainSnapshot};
use tessera_model::window::Window;
use tessera_model::workspace::WorkspaceSnapshot;

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
    pub window: tessera_model::window::WindowId,
    /// Applied compositor-global pointer position, when applicable.
    pub position: Option<tessera_model::Point>,
    /// Privacy-preserving operation class.
    pub kind: AgentInputKind,
}

/// Visual class of a successfully applied Agent input operation.
///
/// Keyboard feedback intentionally omits the key code so passwords and typed
/// content cannot leak through trusted chrome or screenshots of the desktop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentInputKind {
    PointerMove,
    Click { button: u32 },
    Scroll { dx: f32, dy: f32 },
    Keyboard,
}

/// Edge space a chrome component reserves; tiled windows avoid it. Summed
/// across components by [`Shell::reserved`] and subtracted from the tiling
/// work-area so tiles do not render under the dock or panels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reserved {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// A wash composited INTO the frosted backdrop, beneath every glass body.
///
/// This is the architectural home for a surface's scheme-adaptive veil: the
/// command panel's ink/pearl scrim, a modal's dim. A wash painted by chrome
/// sits ABOVE the analytic glass — it hides the glass's refraction and severs
/// the layer stack into "effects below, paint above". A wash declared here is
/// blended into the frost by the layered backdrop material, so glass bodies
/// refract the washed frost and the whole surface reads as one material.
/// `strength` is the blend factor into the frosted colour (`0.0` keeps the
/// plain frost).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackdropWash {
    /// Wash colour, sRGB bytes.
    pub tint: [u8; 3],
    /// Blend strength into the frosted backdrop, `[0, 1]`.
    pub strength: f32,
}

impl Default for BackdropWash {
    fn default() -> Self {
        Self {
            tint: [0xFF, 0xFF, 0xFF],
            strength: 0.0,
        }
    }
}

/// Convert a lens paint colour into a [`BackdropWash`].
///
/// A chrome-painted veil composites with straight alpha over whatever is
/// below it; the frost wash blends by strength instead. The mapping that
/// preserves the veil's visual weight is `strength = alpha`: a 50%-alpha
/// ink wash over the frost reads, to the glass lens above, exactly as the
/// painted veil read over the blur — but beneath the glass instead of over
/// it. `components()` returns premultiplied bytes, so RGB is unpremultiplied
/// against the source alpha here.
#[must_use]
pub fn backdrop_wash(color: lens::Color) -> BackdropWash {
    let (pr, pg, pb, a) = color.components();
    if a == 0 {
        return BackdropWash::default();
    }
    let un = |p: u8| {
        let v = (u32::from(p) * 255 + u32::from(a) / 2) / u32::from(a);
        u8::try_from(v.min(255)).unwrap_or(255)
    };
    BackdropWash {
        tint: [un(pr), un(pg), un(pb)],
        strength: f32::from(a) / 255.0,
    }
}

/// A modal's full-display dim as a backdrop wash region.
///
/// Modals dim the whole output and float one glass panel over it. The dim
/// belongs beneath the panel's glass — blended into the frost — not painted
/// above it by chrome, where it would hide the panel's refraction and split
/// the layer stack. Return this region (plus the panel's own region) from
/// `Chrome::backdrop_regions` instead of calling a painted scrim placement
/// from `render`.
#[must_use]
pub fn modal_scrim_backdrop(display: (f32, f32), design: &tessera_design::Design) -> BackdropRegion {
    BackdropRegion {
        x: 0.0,
        y: 0.0,
        w: display.0,
        h: display.1,
        wash: Some(backdrop_wash(design.colors.modal_scrim)),
    }
}

/// Standard full-screen backdrop cover for immersive modal surfaces (Launchpad, Command Panel).
///
/// Immersive modals share an optical depth-of-field effect: a large-radius Gaussian blur
/// paired with an adaptive modal scrim wash, grounding foreground content without completely
/// severing visual continuity with the user's desktop workspace.
pub struct BackdropCover;

impl BackdropCover {
    /// Canonical backdrop blur sigma for full-screen immersive modals (16.0).
    pub const BLUR_SIGMA: f32 = 16.0;

    /// Full-screen frosted backdrop region with the canonical modal scrim wash.
    #[must_use]
    pub fn region(display: (f32, f32), design: &tessera_design::Design) -> BackdropRegion {
        BackdropRegion {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: display.1,
            wash: Some(backdrop_wash(design.colors.modal_scrim.with_alpha(126))),
        }
    }
}

/// Logical output-space rectangle whose already-composited desktop should be
/// sampled and blurred before chrome is drawn over it. Components declare
/// only the area occupied by their glass material; the executable shares one
/// desktop capture across every request. An optional [`BackdropWash`] blends
/// a veil into this region's frost — see its documentation for why a wash
/// belongs here and not in the chrome paint pass.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BackdropRegion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Optional wash baked into this region's frost, beneath the glass.
    pub wash: Option<BackdropWash>,
}

/// Stable identity of one offscreen backdrop layer.
///
/// `ROOT` is reserved for the compatibility layer synthesized from the
/// existing `Chrome::backdrop_*` methods. Components declaring additional
/// layers should derive their ids with [`backdrop_layer_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BackdropLayerId(pub u64);

impl BackdropLayerId {
    pub const ROOT: Self = Self(0);
}

/// Image sampled by one backdrop layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropLayerSource {
    /// The already-composited desktop before shell chrome.
    Scene,
    /// The resolved image produced by another declared layer.
    Layer(BackdropLayerId),
}

/// One declarative level in the offscreen backdrop graph.
///
/// A layer blurs its explicit source, applies its frost and glass material,
/// and produces a resolved image that later layers can sample. Dependencies
/// form a DAG; the compositor rejects duplicate ids, missing sources, and
/// cycles. Independent layers retain declaration order as their paint order.
/// A zero blur sigma remains an active explicit node and resolves the material
/// without adding a sampling radius.
#[derive(Debug, Clone, PartialEq)]
pub struct BackdropLayer {
    pub id: BackdropLayerId,
    pub source: BackdropLayerSource,
    pub blur_sigma: f32,
    pub frost: Vec<BackdropRegion>,
    pub glass: Vec<LiquidGlassRegion>,
}

impl BackdropLayer {
    #[must_use]
    pub fn new(id: BackdropLayerId, source: BackdropLayerSource, blur_sigma: f32) -> Self {
        Self {
            id,
            source,
            blur_sigma: blur_sigma.max(0.0),
            frost: Vec::new(),
            glass: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_frost(mut self, frost: Vec<BackdropRegion>) -> Self {
        self.frost = frost;
        self
    }

    #[must_use]
    pub fn with_glass(mut self, glass: Vec<LiquidGlassRegion>) -> Self {
        self.glass = glass;
        self
    }
}

impl From<tessera_model::Rect> for BackdropRegion {
    fn from(rect: tessera_model::Rect) -> Self {
        Self {
            x: rect.origin.x as f32,
            y: rect.origin.y as f32,
            w: rect.size.w as f32,
            h: rect.size.h as f32,
            wash: None,
        }
    }
}

impl From<lens::Rect> for BackdropRegion {
    fn from(rect: lens::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            wash: None,
        }
    }
}

/// One analytic liquid-glass body backed by a [`BackdropRegion`] capture.
/// Radius and opacity participate in the same SDF composite as refraction,
/// blur and edge lighting, so rounded corners and visibility cannot diverge.
/// The drop shadow is cast by the body's own SDF; the component configures
/// it in logical pixels and `shadow_alpha` 0 disables it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidGlassRegion {
    pub bounds: BackdropRegion,
    pub corner_radius: f32,
    pub opacity: f32,
    pub shadow_alpha: f32,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
    /// Material-strength multipliers from the role's
    /// [`tessera_design::GlassStyle`] (1.0 = the shared reference recipe).
    pub frost_strength: f32,
    pub tint_strength: f32,
    pub saturation: f32,
    /// Role-pinned plate polarity: 0 pins the whole body to the smoke plate,
    /// 1 to pearl; negative keeps the shader's per-pixel adaptive polarity.
    pub plate_polarity: f32,
    /// Stable cross-frame identity. A component that opts into backdrop
    /// adaptation declares a unique non-zero id (see
    /// [`liquid_glass_region_id`]); the compositor keys temporal smoothing
    /// on it and writes back `adaptation`. 0 = anonymous (the shader's
    /// legacy per-pixel adaptive tint).
    pub id: u64,
    /// Compositor-fed smoothed backdrop statistics for this body. Components
    /// always declare `None`; the adaptation pass fills it for identified
    /// regions before the regions are mapped to prism groups.
    pub adaptation: Option<LiquidGlassAdaptation>,
    /// Optional optical emphasis inside this body. This is one field in the
    /// parent's material, not a nested glass body.
    pub focus: Option<LiquidGlassFocus>,
    /// Optional capture footprint declared when the body's `bounds` animate.
    /// The compositor keys its backdrop capture on exact region geometry, so
    /// a body whose shape morphs every frame (a revealing panel, a
    /// spring-driven strip) would otherwise invalidate the capture and
    /// re-render the whole scene per frame. When set, the footprint — a
    /// stable envelope containing every animated shape — covers capture and
    /// blur instead of `bounds`, and the component must not also declare a
    /// matching `BackdropRegion`. The footprint carries no visible material:
    /// the glass body drawn is still `bounds`.
    pub capture_bounds: Option<BackdropRegion>,
}

/// Smoothed per-region backdrop statistics: `plate_luminance` is the
/// region-mean backdrop luminance (the compositor uses it to scale the
/// role's tint strength — calm dark backdrops recover translucency);
/// `backdrop_energy` is the high-frequency energy the shader's tint boost
/// consumes. Both are 0..1 and temporally smoothed by the compositor.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiquidGlassAdaptation {
    pub plate_luminance: f32,
    pub backdrop_energy: f32,
}

/// Derive a stable liquid-glass region identity from a component's layer id
/// (FNV-1a). Layer ids are already unique static strings across the product,
/// so the hash is collision-safe for this use.
#[must_use]
pub fn liquid_glass_region_id(layer_id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in layer_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Derive a stable, non-root offscreen-layer identity from a component's
/// globally unique layer id.
#[must_use]
pub fn backdrop_layer_id(layer_id: &str) -> BackdropLayerId {
    let id = liquid_glass_region_id(layer_id);
    BackdropLayerId(if id == 0 { 1 } else { id })
}

impl Default for LiquidGlassRegion {
    /// The reference-recipe body: unit material strengths, legacy per-pixel
    /// polarity, anonymous. `from_role` is the product constructor; this
    /// exists for tests and incremental struct updates.
    fn default() -> Self {
        Self {
            bounds: BackdropRegion::default(),
            corner_radius: 0.0,
            opacity: 0.0,
            shadow_alpha: 0.0,
            shadow_blur: 0.0,
            shadow_offset_y: 0.0,
            frost_strength: 1.0,
            tint_strength: 1.0,
            saturation: 1.0,
            plate_polarity: -1.0,
            id: 0,
            adaptation: None,
            focus: None,
            capture_bounds: None,
        }
    }
}

impl LiquidGlassRegion {
    /// Construct a body from one semantic product role.
    #[must_use]
    pub fn from_role(
        design: &tessera_design::Design,
        role: tessera_design::GlassRole,
        bounds: BackdropRegion,
        corner_radius: f32,
        opacity: f32,
    ) -> Self {
        let style = design.glass.for_role(role);
        Self {
            bounds,
            corner_radius,
            opacity: opacity.clamp(0.0, 1.0),
            shadow_alpha: style.shadow_alpha,
            shadow_blur: style.shadow_blur,
            shadow_offset_y: style.shadow_offset_y,
            frost_strength: style.frost_strength,
            tint_strength: style.tint_strength,
            saturation: style.saturation,
            plate_polarity: style.plate_polarity,
            id: 0,
            adaptation: None,
            focus: None,
            capture_bounds: None,
        }
    }

    /// Declare a stable capture footprint containing every shape this body's
    /// animated `bounds` can take. See the field's documentation.
    #[must_use]
    pub fn with_capture_bounds(mut self, bounds: BackdropRegion) -> Self {
        self.capture_bounds = Some(bounds);
        self
    }

    /// Opt this body into backdrop adaptation under a stable identity.
    #[must_use]
    pub fn with_id(mut self, id: u64) -> Self {
        self.id = id;
        self
    }

    /// Attach the parent's single optical interaction focus.
    #[must_use]
    pub fn with_focus(mut self, focus: Option<LiquidGlassFocus>) -> Self {
        self.focus = focus;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiquidGlassFocus {
    pub bounds: BackdropRegion,
    pub corner_radius: f32,
    pub strength: f32,
}

/// Decoded icon texture handles shared with chrome components.
///
/// The textures are owned by the composition root's icon cache; an `IconSet`
/// is a map of borrowed raw `flux_image` pointers keyed by every lowercase id
/// an application may run as (StartupWMClass, desktop-id stem, icon name).
/// The owner must keep the textures alive until it has pushed a replacement
/// catalog via [`Shell::set_app_catalog`]; components only dereference handles
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
    pub position: tessera_model::dock::DockPosition,
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
}

impl Reserved {
    /// Shrink `rect` by these margins, clamped so size never goes negative.
    pub fn inset(self, r: tessera_model::Rect) -> tessera_model::Rect {
        tessera_model::Rect {
            origin: tessera_model::Point {
                x: r.origin.x + self.left,
                y: r.origin.y + self.top,
            },
            size: tessera_model::Size {
                w: (r.size.w - self.left - self.right).max(0),
                h: (r.size.h - self.top - self.bottom).max(0),
            },
        }
    }
}

use lens::Frame;

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

/// Parameters of one low-battery alert, produced by the compositor runtime
/// when a configured threshold fires.
#[derive(Debug, Clone, Copy)]
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
    Focus(tessera_model::window::WindowId),
    /// Hide a window while keeping its client and buffers alive.
    Minimize(tessera_model::window::WindowId),
    /// Set or clear compositor-managed maximization.
    SetMaximized(tessera_model::window::WindowId, bool),
    /// Set or clear the compositor-internal always-on-top flag.
    SetAlwaysOnTop(tessera_model::window::WindowId, bool),
    /// Ask a client to close one of its toplevels gracefully.
    Close(tessera_model::window::WindowId),
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
    pub window: tessera_model::window::WindowId,
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
        window: tessera_model::window::WindowId,
        target: InteractionDomainId,
        retain_source_as_observer: bool,
        expected_revision: u64,
    },
}

/// Interaction intents chrome components emit during a frame. The core
/// collects these and the main loop drains them into server window-management
/// actions or, for [`ChromeEvents::spawn`], into `tessera-launch`. Scalar intents
/// keep the latest value; application menus use an ordered queue for
/// multi-window actions.
#[derive(Debug, Default)]
pub struct ChromeEvents {
    /// The chrome requested the session to quit.
    pub quit: bool,
    /// The chrome asked to lock the session immediately (the command panel's
    /// Lock now row). Drained by the main loop into the same lock path as the
    /// Super+L binding.
    pub lock: bool,
    /// Window id a component asked to focus/activate.
    pub clicked: Option<tessera_model::window::WindowId>,
    /// Ordered window actions emitted by popup menus. A queue allows an
    /// application-level action such as "close all windows" to preserve one
    /// journal entry per affected toplevel.
    pub window_actions: Vec<WindowAction>,
    /// A desktop entry the chrome asked to launch (e.g. the launcher's
    /// clicked row). Drained into `tessera-launch` by the main loop; carrying the
    /// full [`Entry`] keeps `tessera-shell` free of any `tessera-apps` dependency
    /// (ADR-0022).
    pub spawn: Option<Entry>,
    /// A trusted compositor-owned application to present. Built-ins share the
    /// launcher catalog with external apps but never pass through a shell
    /// command or process boundary.
    pub open_builtin: Option<BuiltInApplication>,
    /// A workspace the chrome asked to switch to (the workspace bar's clicked
    /// tile). Drained into `Server::switch_workspace_to` by the main loop
    /// (ADR-0025).
    pub switch_workspace: Option<tessera_model::workspace::WorkspaceId>,
    /// The chrome asked to toggle the launcher this frame (the dock's
    /// Launchpad tile). Drained by the main loop, which calls [`Shell::toggle`]
    /// — the same path as the Super+A hotkey — so the launcher flips open or
    /// closed.
    pub toggle_launcher: bool,
    /// Notification id the toast stack asked to dismiss. Drained through the
    /// same command/journal path as an IPC dismissal.
    pub dismissed_notification: Option<u64>,
    /// Window id the overview asked to focus this frame (a thumbnail click).
    /// Drained through the focus command/journal path; picking also closes
    /// the overview.
    pub overview_pick: Option<tessera_model::window::WindowId>,
    /// Window id clicked in the held-modifier switcher.
    pub window_switcher_pick: Option<tessera_model::window::WindowId>,
    /// A mirror-guard drag asked the compositor to move a read-only mirror
    /// window this frame.
    pub mirror_move: Option<MirrorMove>,
    /// The switcher was dismissed by clicking outside its cards.
    pub window_switcher_cancel: bool,
    /// Workspace id the overview's rail asked to switch to. Drained through
    /// the same command/journal path as `SwitchWorkspaceTo`.
    pub overview_switch: Option<tessera_model::workspace::WorkspaceId>,
    /// Region the screenshot selector asked to capture this frame, if any.
    pub screenshot_region: Option<tessera_model::Rect>,
    /// Point the pixel picker was clicked at this frame (ADR-0054), in
    /// compositor logical pixels.
    pub picked_point: Option<tessera_model::Point>,
    /// Window id the window picker was clicked on this frame (ADR-0054).
    pub picked_window: Option<tessera_model::window::WindowId>,
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
    pub settings_actions: Vec<(Option<u64>, tessera_model::settings::SettingsAction)>,
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
    pub dock_position: Option<tessera_model::dock::DockPosition>,
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
    ToggleLauncher,
    CloseLauncher,
    TogglePrism,
    ClosePrism,
    OpenBuiltIn(BuiltInApplication),
    ToggleOverview,
    CloseOverview,
    ToggleCommandPanel,
    CloseCommandPanel,
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
    /// The shell-wide design snapshot for the resolved desktop color scheme.
    /// Components that paint from design tokens retain a copy instead of
    /// constructing one inline; seeded by [`Shell::add`] and broadcast by
    /// [`Shell::set_color_scheme`].
    Appearance(&'a tessera_design::Design),
    /// The persistent-settings snapshot, seeded by [`Shell::add`] and
    /// broadcast by [`Shell::set_settings`]. Only chrome hosting settings UI
    /// consumes it.
    Settings(&'a tessera_model::settings::SettingsSnapshot),
    ReducedMotion(bool),
    ModalReserved(Reserved),
    /// Whether the full-screen application launcher currently owns the
    /// output. Pushed every frame alongside [`ChromeUpdate::ModalReserved`]:
    /// the launcher blurs and dims the complete desktop, so persistent
    /// decorations that would float inside its field (the HUD chips) hide
    /// for its duration, while the dock below its work area stays.
    LauncherActive(bool),
}

/// One piece of compositor chrome.
///
/// A component renders itself for one frame from the shared window and
/// workspace snapshots and the input, drawing through `frame` and pushing
/// any user intents into `out`. The core owns the lens context, the
/// snapshots, and the sink; the component owns only its own appearance and
/// state. Register implementations with [`Shell::add`].
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
    fn key_char(&mut self, _kc: &tessera_model::input::KeyChar, _out: &mut ChromeEvents) {}

    /// Receive a discrete host lifecycle command.
    fn command(&mut self, _command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {}

    /// Receive a host-owned snapshot or presentation-policy update.
    fn update(&mut self, _update: ChromeUpdate<'_>) {}

    /// Whether this component owns the application launcher state.
    fn launcher_active(&self) -> bool {
        false
    }

    /// Whether this component owns an open Prism surface.
    fn prism_active(&self) -> bool {
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
        _out: &mut Vec<(tessera_model::window::WindowId, tessera_model::Rect)>,
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
        _display: tessera_model::Rect,
        _windows: &[Window],
        _order: &[tessera_model::window::WindowId],
        _selected: Option<tessera_model::window::WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        None
    }

    /// Return a live client-preview popover prepared by this component.
    /// Geometry is read after [`Chrome::prepare_backdrop`] and shared by the
    /// compositor preview pass, analytic glass declarations, shell rendering,
    /// and pointer hit-testing.
    fn live_preview_presentation(&self) -> Option<LivePreviewPresentation> {
        None
    }

    /// Whether the Super+Tab preview strip is currently active.
    fn window_switcher_active(&self) -> bool {
        false
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

    /// Whether the low-battery alert is currently open. Default `false`; the
    /// battery-alert component overrides this.
    fn battery_alert_active(&self) -> bool {
        false
    }

    /// Prepare geometry/visibility that the backdrop capture must consume in
    /// the same frame as [`Chrome::render`]. Components with cursor-driven
    /// glass animations use this prepass so their SDF opacity never trails
    /// foreground content by one frame.
    fn prepare_backdrop(
        &mut self,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
    }

    /// Edge space this component reserves; tiled windows avoid it (ADR-0024).
    /// Default none; overridden by chrome that should not be covered (the
    /// dock reserves the bottom edge). Summed by [`Shell::reserved`].
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

    /// The output-logical rectangle this component's in-flight animation can
    /// touch **this frame**, for hosts that convert animation ticks into
    /// partial-output repaints instead of a full composite.
    ///
    /// Contract:
    /// * Only consulted while [`Chrome::anim_pending`] returns true; a
    ///   static component's region is never read.
    /// * The rectangle must cover every pixel the animation may change
    ///   between the previous presented frame and the next one — including
    ///   the area vacated by a moving element (union of the previous and
    ///   current position), plus any soft shadow/blur halo.
    /// * Returning `None` is always safe: the host falls back to a
    ///   full-output repaint for this frame. Third-party components should
    ///   keep the `None` default unless they can state their footprint.
    ///
    /// Damage reported this way also has to be **re-reported** every animated
    /// frame while the element keeps moving, because each partial repaint
    /// consumes the previous frame's rectangle.
    fn damage_region(
        &self,
        _windows: &[Window],
        _display: (f32, f32),
    ) -> Option<tessera_model::Rect> {
        None
    }

    /// Whether this component would draw visible pixels in the next frame.
    /// The default is conservative: third-party chrome blocks direct scanout
    /// until it explicitly proves that its inactive state is visually empty.
    fn requires_composition(&self) -> bool {
        true
    }

    /// Blur width requested for the desktop behind compositor
    /// chrome, in logical pixels. The host takes the maximum across
    /// components and applies one shared backdrop capture before rendering
    /// chrome. A zero value disables the capture path.
    fn backdrop_blur_sigma(&self) -> f32 {
        0.0
    }

    /// Regions covered by this component's glass material, in logical output
    /// coordinates. An empty list means the component contributes no
    /// rectangular frost; components that want a full-screen backdrop (the
    /// launcher, the command panel) must declare the full-screen rect
    /// explicitly. Analytic glass bodies need no entry here — they carry
    /// their own capture footprint via `LiquidGlassRegion::capture_bounds`.
    fn backdrop_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        Vec::new()
    }

    /// Subset of [`Chrome::backdrop_regions`] that should use the analytic
    /// thick-glass compositor instead of a rectangular frosted-blur clip.
    fn liquid_glass_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        Vec::new()
    }

    /// Additional offscreen layers declared by this component.
    ///
    /// The host already synthesizes [`BackdropLayerId::ROOT`] from the
    /// compatibility blur/frost/glass methods above. An additional layer may
    /// sample the scene, that root, or another additional layer by id. Keep
    /// the default empty unless the material intentionally needs cumulative
    /// composition such as glass sampling an earlier glass result.
    fn backdrop_layers(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropLayer> {
        Vec::new()
    }
}

/// Exact compositor-owned output requirements for primary-plane policy.
///
/// This deliberately describes what would affect the *next rendered frame*,
/// rather than broad component state such as "the Dock exists" or "an
/// animation timer is running".  A hidden/fully transparent component must
/// not prevent direct scanout, while any visible chrome pixel or live
/// backdrop sample must.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompositionRequirements {
    /// At least one eligible chrome component would draw visible pixels.
    pub visible_pixels: bool,
    /// Visible chrome samples the client scene through a backdrop effect.
    pub live_backdrop_effect: bool,
}

