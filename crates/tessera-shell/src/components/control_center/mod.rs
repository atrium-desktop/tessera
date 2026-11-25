//! The Control Center: a sheet anchored to the right edge of the output.
//!
//! Two cards make up the sheet (see the `layout` module):
//!
//! - the **controls** card — the persona header with Settings, Lock, and Power
//!   actions; connectivity and preference tiles; the sound, brightness,
//!   microphone, and keyboard-backlight controls; the now-playing card; the
//!   work-mode switch; and the tray row. Wi-Fi, Bluetooth, Cast, and Sound
//!   drill into a detail page in place, with a Back affordance.
//! - the **notifications** card — the wall clock and date over the
//!   notification list, newest first, with dismiss-one and clear-all.
//!
//! The sheet floats over the live desktop: no full-output canvas, no scrim, no
//! backdrop capture. Its paint is bounded by its cards, so its cost follows its
//! area (ADR-0213) and its damage is the cards' footprint. It opens with
//! `Super+S`, a four-finger swipe down, or a press on the Dock's reading cells,
//! and closes on `Escape`, a press outside the cards, the same keybinding, or a
//! four-finger swipe up. While presented it owns the pointer and keyboard, so a
//! dismissing press never leaks into the window beneath it.
//!
//! Persistent settings are not edited here. They belong to the Settings
//! application (`tessera-settings`), which the header's gear opens; the sheet
//! only toggles the color scheme, because that is a daily control rather than
//! configuration.
//!
//! Scope (ADR-0115): the sheet is the display-and-control surface for
//! desktop-computer behavior — sound, displays, network and Bluetooth, power,
//! the session, notifications, the tray, and the user's persona. Compositor
//! mechanism (window-tree internals, IPC state, introspection) stays in the CLI
//! and MCP.
//!
//! The component owns presentation and interaction state only. Snapshots
//! arrive through the `Chrome` trait; intents leave through `ChromeEvents` and
//! the shared tray command channel.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::widgets::motion::{Spring, SpringParams};
use crate::widgets::tiered::TieredIndicator;
use optics_ui::{Align, Color, Frame, Input, LayoutOpts, Rect};
use tessera_avatar::persona::{Portrait, PortraitConfig, PortraitWatcher, Profile};
use tessera_design::tokens::TypeScale;
use tessera_design::{AvatarRole, ControlCenterColors, Design, SurfaceRole, materials, themes};
use tessera_desktop::notify::Notification;
use tessera_desktop::notify::NotificationQueue;
use tessera_desktop::power::PowerMode;
use tessera_desktop::settings::DesktopPreferences;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_primitives::input::KeyChar;

use crate::component::{
    Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, IconSet, Localizer, Message,
    PopupSide, Reserved, SystemAction, SystemStatus, ellipsize, place_popup_side, truncate,
};
pub(super) use crate::tray;
use crate::tray::{MenuNode, MenuState, TrayCommand, TrayHandle, TrayProjection};

mod layout;
mod mpris;
mod rendering;
mod tray_shelf;

use layout::{SheetLayout, sheet_layout};
use mpris::{MediaCommand, MediaHandle};
use rendering::*;

#[cfg(test)]
mod tests;

// ---- controls card rhythm -------------------------------------------------

/// The persona header row.
const HEADER_H: f32 = 48.0;
/// The session strip the power button discloses under the header.
const SESSION_STRIP_H: f32 = 40.0;
/// One tile row in the toggle grid.
const TILE_H: f32 = 58.0;
const TILE_GAP: f32 = 10.0;
/// One fader card: a 24px title row, an 8px gap, a 20px trough, and 10px of
/// padding above and below.
const FADER_H: f32 = 72.0;
/// The now-playing card.
const MEDIA_H: f32 = 92.0;
/// The work-mode segmented control.
const WORK_MODE_H: f32 = 40.0;
/// One tray cell: the hover plate / hit area around the icon. A pager chevron
/// is the same size, so turning a page never demands more precision than
/// pressing the icon beside it (ADR-0221).
const TRAY_CELL: f32 = 40.0;
/// The SNI glyph itself, centered in the cell.
const TRAY_ICON: f32 = 22.0;
/// Horizontal rhythm between tray slots (ADR-0221).
const TRAY_GAP: f32 = 10.0;
/// Inset between the tray capsule and its icons.
const TRAY_PAD: f32 = 8.0;
/// The tray row: one cell plus the capsule's inset.
const TRAY_ROW_H: f32 = TRAY_CELL + TRAY_PAD * 2.0;
/// A detail page never collapses below this, so its list has room to read.
const DETAIL_MIN_H: f32 = 520.0;
/// A header icon button.
const HEADER_BUTTON: f32 = 36.0;
/// Space a scrolling column leaves for its scrollbar.
const SCROLL_GUTTER: f32 = 12.0;

// ---- notifications card rhythm --------------------------------------------

/// The clock block: hero time over the date.
const CLOCK_BLOCK_H: f32 = 76.0;
/// The "Notifications · Clear all" row.
const LIST_HEADER_H: f32 = 36.0;
/// One notification card: app line, summary, and a one-line body.
const NOTIFICATION_H: f32 = 72.0;
const NOTIFICATION_GAP: f32 = 8.0;
/// The empty state.
const EMPTY_LIST_H: f32 = 64.0;

/// The panel explicitly owns its VRM composition. Keeping the parameters here
/// lets another host choose a different crop without changing the VRM
/// renderer or the shared portrait source policy.
const AVATAR_CAMERA: tessera_avatar::persona::VrmCamera =
    tessera_avatar::persona::VrmCamera::new(28.0, 0.25, 0.48, 0.0);
/// Header avatar diameter.
const AVATAR_SIZE: f32 = 40.0;

// dbusmenu popover geometry. Placement follows the shared shell popup policy.
const MENU_WIDTH: f32 = 236.0;
const MENU_PAD: f32 = 7.0;
const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_HEADER_HEIGHT: f32 = 23.0;
const MENU_SEP_GAP: f32 = 4.0;
const MENU_SEPARATOR_HEIGHT: f32 = MENU_SEP_GAP * 2.0 + 1.0;

/// Reveal spring (ADR-0176, ADR-0178): critically damped, so the fade and the
/// slide approach their endpoint from one side only.
const REVEAL_SPRING: SpringParams = SpringParams::new(480.0, 1.0);
const REVEAL_SETTLE_VALUE_EPS: f32 = 0.004;
const REVEAL_SETTLE_VELOCITY_EPS: f32 = 0.04;
/// Frames the sheet keeps reporting its footprint after its reveal settles
/// (ADR-0227): every swapchain ring slot must repaint the pixels the cards
/// covered or uncovered. One more than `OPTICS_GFX_MAX_FRAMES_IN_FLIGHT` (3).
const RETIRE_FRAMES: u8 = 4;
/// Delay before a hovered icon button's tooltip starts to appear.
const TOOLTIP_DELAY: f32 = 0.45;

/// What the controls card shows below its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Main,
    Wifi,
    Bluetooth,
    Sound,
    Cast,
}

/// An icon-only control whose name is shown as a tooltip on hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tip {
    Settings,
    Lock,
    Power,
    DoNotDisturb,
    DarkMode,
    WorkMode(usize),
}

/// One SNI cell to draw this frame, distilled from the tray snapshot.
#[derive(Clone)]
pub(crate) struct SniCell {
    key: String,
    title: String,
    has_menu: bool,
    textured: bool,
}

/// The per-cell visuals for the tray row, distilled before the layout
/// closures so they borrow no `self` state.
struct TrayCellVisual {
    key: String,
    title: String,
    has_menu: bool,
    texture: Option<*mut optics_ui::sys::optics_gfx_image>,
    fallback: Option<*mut optics_ui::sys::optics_gfx_image>,
}

/// Render-side cache of the shared dbusmenu tree, tagged with the worker's
/// `menu_revision` so the tree is re-cloned only when the menu changed.
struct MenuSnapshotCache {
    revision: u64,
    menu: Option<Arc<MenuState>>,
}

pub struct ControlCenter {
    open: bool,
    /// Reveal spring (0 = hidden, 1 = presented) driving the cards' fade and
    /// slide. A spring, not an exponential follow, so convergence is bounded
    /// and the frame loop can prove when it has settled (ADR-0172).
    reveal: Spring,
    /// Frames left in the post-settle retirement window (ADR-0227).
    retire_frames: u8,
    page: Page,
    /// The session strip (Suspend · Restart · Power off) under the header.
    session_open: bool,
    /// The authoritative desktop preferences, so the dark-mode tile can flip
    /// the scheme without resetting the rest of the profile.
    preferences: Option<DesktopPreferences>,
    design: Design,
    reduced_motion: bool,
    prev_down: bool,
    status: SystemStatus,
    /// Local account behind the header, resolved once at construction.
    profile: Profile,
    /// Header portrait selected by the shared profile contract. Without
    /// configured content, initials render over a flat disc.
    avatar: Option<Portrait>,
    portrait_config: PortraitConfig,
    /// Non-owning view of the compositor device, used only on the render
    /// thread to build a complete hot-reload replacement.
    avatar_device: Option<optics_gfx::Device>,
    /// Filesystem observation is notification-only; decode and GPU work stay
    /// on the render thread.
    avatar_watcher: Option<PortraitWatcher>,
    avatar_warned: bool,
    icons: IconSet,
    notifications: Arc<Mutex<NotificationQueue>>,
    /// Notification list cache keyed by the queue's revision.
    notification_cache: Option<(u64, Arc<Vec<Notification>>)>,
    tray: Option<TrayProjection>,
    /// Session-bus MPRIS source. The worker owns all D-Bus calls.
    media: Option<MediaHandle>,
    /// SNI item key whose dbusmenu popover is showing.
    menu_open_for: Option<String>,
    /// Breadcrumb of submenu ids from root to the current view.
    menu_path: Vec<i32>,
    /// The tray cell rect that opened the popover.
    menu_owner: Rect,
    /// Suppresses the press that opened the menu from also closing it.
    menu_just_opened: bool,
    /// The tray row's current page (ADR-0221).
    tray_page: usize,
    /// Edge space the shell's persistent decorations reserve; the sheet never
    /// crosses it (ADR-0197).
    reserved: Reserved,
    /// The tray layout resolved by the last frame (ADR-0221).
    tray_shelf: Option<tray_shelf::TrayShelfLayout>,
    menu_cache: Option<MenuSnapshotCache>,
    work_mode_indicator: TieredIndicator,
    kbd_backlight_indicator: TieredIndicator,
    /// The tier a press-and-drag over the keyboard-backlight control previews.
    kbd_backlight_preview: Option<usize>,
    /// The icon control under the pointer and how long it has been there.
    tip: Option<(Tip, Rect)>,
    tip_dwell: f32,
    tip_reveal: f32,
    /// The action whose system-level confirmation is pending; the runtime owns
    /// the confirmation chrome.
    power_pending_confirm: Option<SystemAction>,
    frame_prepared: bool,
    /// The sheet resolved for the last frame's output.
    sheet: SheetLayout,
    display: (f32, f32),
    pub(super) wifi_input_ssid: Option<String>,
    pub(super) wifi_input_passphrase: String,
    pub(super) cast_scanning: bool,
    /// Test-only tray contents standing in for a session bus.
    #[cfg(test)]
    test_cells: Vec<SniCell>,
}

impl ControlCenter {
    /// Construct the Control Center. The gfx device is borrowed (non-owning,
    /// like [`crate::Shell::new`]) to upload tray pixmaps and the portrait; the
    /// caller keeps it alive past the panel. The tray handle comes from the
    /// composition root's single `crate::tray::spawn()`.
    pub fn new(
        device: &optics_gfx::Device,
        tray: Option<TrayHandle>,
        notifications: Arc<Mutex<NotificationQueue>>,
    ) -> ControlCenter {
        let tray = tray.map(|handle| {
            // SAFETY: the composition root declares its gfx device before the
            // shell and drops it after; the panel touches it on the render
            // thread only.
            let device = unsafe { optics_gfx::Device::borrow_raw(device.as_raw()) };
            TrayProjection::new(device, handle)
        });
        let portrait_config = PortraitConfig::current();
        let avatar = match Portrait::load_transactional(device, &portrait_config, AVATAR_CAMERA) {
            Ok(loaded) => loaded,
            Err(error) => {
                log::warn!("control-center: avatar load failed, using initials: {error}");
                None
            }
        };
        // SAFETY: as above.
        let avatar_device = unsafe { optics_gfx::Device::borrow_raw(device.as_raw()) };
        let avatar_watcher = match PortraitWatcher::new(&portrait_config) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                log::warn!("control-center: avatar hot reload disabled: {error}");
                None
            }
        };
        let mut panel = Self::bare(notifications);
        panel.avatar = avatar;
        panel.portrait_config = portrait_config;
        panel.avatar_device = Some(avatar_device);
        panel.avatar_watcher = avatar_watcher;
        panel.tray = tray;
        panel.media = mpris::spawn();
        panel
    }

    fn bare(notifications: Arc<Mutex<NotificationQueue>>) -> ControlCenter {
        ControlCenter {
            open: false,
            reveal: Spring::new(0.0),
            retire_frames: 0,
            page: Page::Main,
            session_open: false,
            preferences: None,
            design: Design::dark(),
            reduced_motion: false,
            prev_down: false,
            status: SystemStatus::default(),
            profile: Profile::current().unwrap_or_else(|_| Profile::fallback()),
            avatar: None,
            portrait_config: PortraitConfig::new(Vec::new()),
            avatar_device: None,
            avatar_watcher: None,
            avatar_warned: false,
            icons: IconSet::default(),
            notifications,
            notification_cache: None,
            tray: None,
            media: None,
            menu_open_for: None,
            menu_path: Vec::new(),
            menu_owner: Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            menu_just_opened: false,
            tray_page: 0,
            reserved: Reserved::default(),
            tray_shelf: None,
            menu_cache: None,
            work_mode_indicator: TieredIndicator::at(0),
            kbd_backlight_indicator: TieredIndicator::default(),
            kbd_backlight_preview: None,
            tip: None,
            tip_dwell: 0.0,
            tip_reveal: 0.0,
            power_pending_confirm: None,
            frame_prepared: false,
            sheet: sheet_layout((0.0, 0.0), Reserved::default(), 1.0, 1.0),
            display: (0.0, 0.0),
            wifi_input_ssid: None,
            wifi_input_passphrase: String::new(),
            cast_scanning: false,
            #[cfg(test)]
            test_cells: Vec::new(),
        }
    }

    /// Test constructor without a GPU device, tray, or bus.
    #[cfg(test)]
    pub(crate) fn without_sources() -> ControlCenter {
        Self::bare(Arc::new(Mutex::new(NotificationQueue::new(3_600_000))))
    }

    /// Visible or mid reveal/close: owns the input and the modal layer.
    fn presented(&self) -> bool {
        self.open || self.reveal.value > 0.005
    }

    /// Presented, or still repairing the pixels it last covered.
    fn active(&self) -> bool {
        self.presented() || self.retire_frames > 0
    }

    fn reveal(&self) -> f32 {
        self.reveal.value.clamp(0.0, 1.0)
    }

    fn panel_colors(&self) -> ControlCenterColors {
        // In the shell's material, not the bare palette: the sheet's cards are
        // chrome bodies and switch with the Dock (ADR-0252).
        self.design.control_center_colors()
    }

    /// Reach of the cards' elevation halo beyond their bodies.
    fn halo_reach(&self) -> f32 {
        let style = self.design.surfaces.for_role(SurfaceRole::FloatingPanel);
        style.shadow_blur + style.shadow_offset_y.abs()
    }

    fn open_sheet(&mut self) {
        self.open = true;
        self.page = Page::Main;
        if let Some(name) = self
            .avatar
            .as_mut()
            .and_then(|avatar| avatar.play_random_action().map(str::to_owned))
        {
            log::debug!("control-center: playing avatar action {name:?}");
        }
    }

    /// Close the sheet. Detail pages, the session strip, the tray popover, and
    /// hover latches all go with it, so a reopened sheet starts at rest and a
    /// latch can never keep a closed sheet animating.
    fn close(&mut self, out: &mut ChromeEvents) {
        self.open = false;
        self.show_page(Page::Main, out);
        self.session_open = false;
        self.power_pending_confirm = None;
        self.tray_page = 0;
        self.kbd_backlight_preview = None;
        self.tip = None;
        self.tip_dwell = 0.0;
        self.tip_reveal = 0.0;
        if let Some(key) = self.menu_open_for.take() {
            self.menu_path.clear();
            self.menu_just_opened = false;
            self.send_tray_command(TrayCommand::CloseMenu { key });
        }
    }

    /// Switch the controls card's page, ending the side effects of the page
    /// being left: Cast discovery stops and a half-typed Wi-Fi passphrase is
    /// dropped. Entering a page starts its scan.
    pub(crate) fn show_page(&mut self, page: Page, out: &mut ChromeEvents) {
        if self.page == page {
            return;
        }
        match self.page {
            Page::Cast => {
                self.cast_scanning = false;
                out.system_actions.push(SystemAction::StopCastDiscovery);
            }
            Page::Wifi => {
                self.wifi_input_ssid = None;
                self.wifi_input_passphrase.clear();
            }
            Page::Main | Page::Bluetooth | Page::Sound => {}
        }
        match page {
            Page::Wifi => out.system_actions.push(SystemAction::ScanWifi),
            Page::Bluetooth => out.system_actions.push(SystemAction::ScanBluetooth),
            Page::Cast => {
                self.cast_scanning = true;
                out.system_actions.push(SystemAction::ScanCast);
            }
            Page::Main | Page::Sound => {}
        }
        self.page = page;
        self.session_open = false;
    }

    /// Advance clocks and the portrait watcher before damage is queried.
    fn prepare_frame(&mut self, input: &Input) {
        let raw = input.as_raw();
        let dt = raw.dt_seconds.max(0.0);
        self.advance(dt);
        self.reload_avatar_if_ready();
        // Advance the animated VRM portrait during the pre-pass, before the
        // output canvas session opens, so offscreen submissions never
        // interleave with output render passes.
        if self.open
            && self.avatar.as_ref().is_some_and(Portrait::is_animated)
            && let Some(avatar) = &mut self.avatar
            && let Err(error) = avatar.advance(dt)
            && !self.avatar_warned
        {
            log::warn!("control-center: avatar advance failed: {error}");
            self.avatar_warned = true;
        }
        self.frame_prepared = true;
    }

    fn avatar_reload_pending(&self) -> bool {
        self.avatar_watcher
            .as_ref()
            .is_some_and(PortraitWatcher::needs_poll)
    }

    /// Rebuild the portrait on the render thread and publish only a complete
    /// replacement; a failed decode keeps the current one and retries later.
    fn reload_avatar_if_ready(&mut self) {
        let ready = self
            .avatar_watcher
            .as_mut()
            .is_some_and(PortraitWatcher::poll);
        if !ready {
            return;
        }
        if let Some(watcher) = &mut self.avatar_watcher
            && let Err(error) = watcher.refresh()
        {
            log::warn!("control-center: could not refresh avatar watches: {error}");
        }
        let Some(device) = &self.avatar_device else {
            return;
        };
        let previous_motion = self
            .avatar
            .as_ref()
            .and_then(Portrait::current_motion)
            .map(str::to_owned);
        match Portrait::load_transactional(device, &self.portrait_config, AVATAR_CAMERA) {
            Ok(Some(mut replacement)) => {
                let restored = previous_motion
                    .as_deref()
                    .is_some_and(|name| replacement.play_motion(name));
                if !restored && self.open {
                    replacement.play_random_action();
                }
                self.avatar = Some(replacement);
                self.avatar_warned = false;
                log::info!("control-center: avatar hot reloaded");
            }
            Ok(None) => {
                self.avatar = None;
                self.avatar_warned = false;
                log::info!("control-center: avatar removed, using initials");
            }
            Err(error) => {
                log::warn!("control-center: avatar hot reload failed, keeping current: {error}");
                if let Some(watcher) = &mut self.avatar_watcher {
                    watcher.retry();
                }
            }
        }
    }

    fn reveal_target(&self) -> f32 {
        if self.open { 1.0 } else { 0.0 }
    }

    fn reveal_settled(&self) -> bool {
        self.reveal.settled(
            self.reveal_target(),
            REVEAL_SETTLE_VALUE_EPS,
            REVEAL_SETTLE_VELOCITY_EPS,
        )
    }

    fn work_mode_index(&self) -> usize {
        PowerMode::ALL
            .iter()
            .position(|mode| *mode == self.status.power_mode)
            .unwrap_or(0)
    }

    fn kbd_backlight_index(&self) -> usize {
        self.kbd_backlight_preview
            .unwrap_or_else(|| self.status.kbd_brightness_tier_index())
    }

    fn advance(&mut self, dt: f32) {
        let reduced = self.reduced_motion;
        let was_moving = !self.reveal_settled();
        self.reveal
            .advance_substep(self.reveal_target(), REVEAL_SPRING, dt, 1.0 / 60.0, reduced);
        if !self.reveal_settled() {
            self.retire_frames = 0;
        } else if was_moving {
            self.reveal.snap_to(self.reveal_target());
            self.retire_frames = RETIRE_FRAMES;
        } else if self.retire_frames > 0 {
            self.retire_frames -= 1;
        }
        self.work_mode_indicator
            .advance(self.work_mode_index(), dt, reduced);
        if self.status.kbd_brightness.is_some() {
            self.kbd_backlight_indicator
                .advance(self.kbd_backlight_index(), dt, reduced);
        }
        // A tooltip appears after a short dwell and leaves at once.
        if self.tip.is_some() {
            self.tip_dwell += dt;
        } else {
            self.tip_dwell = 0.0;
        }
        let tip_target = if self.tip.is_some() && self.tip_dwell >= TOOLTIP_DELAY {
            1.0
        } else {
            0.0
        };
        self.tip_reveal =
            crate::widgets::motion::approach(self.tip_reveal, tip_target, 18.0, dt, reduced);
        if (self.tip_reveal - tip_target).abs() < 0.01 {
            self.tip_reveal = tip_target;
        }
    }

    /// Whether a micro-interaction (segment indicator, backlight indicator,
    /// tooltip dwell or fade) is still moving.
    fn interaction_anim_pending(&self) -> bool {
        self.work_mode_indicator
            .anim_pending(self.work_mode_index())
            || (self.status.kbd_brightness.is_some()
                && self
                    .kbd_backlight_indicator
                    .anim_pending(self.kbd_backlight_index()))
            || (self.tip.is_some() && self.tip_reveal < 1.0)
            || (self.tip.is_none() && self.tip_reveal > 0.0)
    }

    fn avatar_animating(&self) -> bool {
        (self.open && self.avatar.as_ref().is_some_and(Portrait::is_animated))
            || self.avatar_reload_pending()
    }

    // ---- content-driven geometry --------------------------------------

    fn media_visible(&self) -> bool {
        self.media
            .as_ref()
            .is_some_and(|media| media.snapshot().available)
    }

    fn microphone_visible(&self) -> bool {
        self.status.microphone_volume.is_some() || !self.status.audio_inputs.is_empty()
    }

    /// The controls card's height when nothing constrains it.
    fn controls_natural_height(&mut self) -> f32 {
        let mut sections: Vec<f32> = vec![HEADER_H];
        if self.session_open {
            sections.push(SESSION_STRIP_H);
        }
        if self.page != Page::Main {
            let body: f32 = sections.iter().sum::<f32>()
                + layout::SECTION_GAP * sections.len() as f32
                + layout::CARD_PAD * 2.0;
            return body.max(DETAIL_MIN_H);
        }
        sections.push(TILE_H * 2.0 + TILE_GAP);
        let faders = [
            self.status.volume.is_some(),
            self.status.brightness.is_some(),
            self.microphone_visible(),
            self.status.kbd_brightness.is_some(),
        ]
        .into_iter()
        .filter(|visible| *visible)
        .count();
        if faders > 0 {
            sections.push(FADER_H * faders as f32 + TILE_GAP * (faders - 1) as f32);
        }
        if self.media_visible() {
            sections.push(MEDIA_H);
        }
        sections.push(WORK_MODE_H);
        if !self.sni_cells().is_empty() {
            sections.push(TRAY_ROW_H);
        }
        sections.iter().sum::<f32>()
            + layout::SECTION_GAP * (sections.len() - 1) as f32
            + layout::CARD_PAD * 2.0
    }

    /// The notifications card's height when nothing constrains it.
    fn notifications_natural_height(&mut self) -> f32 {
        let count = self.notification_snapshot().len();
        let list = if count == 0 {
            EMPTY_LIST_H
        } else {
            NOTIFICATION_H * count as f32 + NOTIFICATION_GAP * (count - 1) as f32
        };
        CLOCK_BLOCK_H
            + layout::SECTION_GAP
            + LIST_HEADER_H
            + NOTIFICATION_GAP
            + list
            + layout::CARD_PAD * 2.0
    }

    /// Resolve and cache the sheet for `display`.
    fn resolve_sheet(&mut self, display: (f32, f32)) -> SheetLayout {
        let controls = self.controls_natural_height();
        let notifications = self.notifications_natural_height();
        self.display = display;
        self.sheet = sheet_layout(display, self.reserved, controls, notifications);
        self.sheet
    }

    // ---- shared sources ------------------------------------------------

    /// Clone the notification queue, memoized on its revision.
    fn notification_snapshot(&mut self) -> Arc<Vec<Notification>> {
        let queue = self.notifications.lock().unwrap_or_else(|e| e.into_inner());
        let revision = queue.revision();
        match &self.notification_cache {
            Some((cached, list)) if *cached == revision => Arc::clone(list),
            _ => {
                let list = Arc::new(queue.snapshot());
                self.notification_cache = Some((revision, Arc::clone(&list)));
                list
            }
        }
    }

    fn send_tray_command(&self, command: TrayCommand) {
        if let Some(tray) = &self.tray {
            // The worker may be gone (bus disconnected); clicks just drop.
            let _ = tray.handle().send(command);
        }
    }

    fn themed_icon(&self, name: &str) -> Option<*mut c_void> {
        self.icons.get(&format!("tessera-hud:{name}"))
    }

    /// Read the shared menu snapshot, memoized on the worker's
    /// `menu_revision`. A contended lock serves the cached menu rather than
    /// blocking the frame.
    fn menu_snapshot(&mut self) -> Option<Arc<MenuState>> {
        let tray = self.tray.as_ref()?;
        if let Ok(snapshot) = tray.handle().snapshot().try_lock() {
            let stale = self
                .menu_cache
                .as_ref()
                .is_none_or(|cache| cache.revision != snapshot.menu_revision);
            if stale {
                self.menu_cache = Some(MenuSnapshotCache {
                    revision: snapshot.menu_revision,
                    menu: snapshot.menu.clone().map(Arc::new),
                });
            }
        }
        self.menu_cache.as_ref()?.menu.clone()
    }

    /// Project the SNI tray for this frame.
    fn sni_cells(&mut self) -> Vec<SniCell> {
        #[cfg(test)]
        if !self.test_cells.is_empty() {
            return self.test_cells.clone();
        }
        self.tray
            .as_mut()
            .map(TrayProjection::cells)
            .unwrap_or_default()
            .into_iter()
            .map(|cell| SniCell {
                key: cell.key,
                title: cell.title,
                has_menu: cell.has_menu,
                textured: cell.textured,
            })
            .collect()
    }

    fn close_menu(&mut self, key: String) {
        self.menu_open_for = None;
        self.menu_path.clear();
        self.menu_just_opened = false;
        self.send_tray_command(TrayCommand::CloseMenu { key });
    }

    /// Bounds of the tray's dbusmenu popover, if one is open.
    fn open_popover_bounds(&mut self) -> Option<Rect> {
        let key = self.menu_open_for.clone()?;
        let menu = self.menu_snapshot().filter(|menu| menu.key == key)?;
        tray::visible_children(&menu.root, &self.menu_path)
            .map(|visible| menu_bounds(self.menu_owner, visible, self.display))
    }
}

mod presentation;

impl Chrome for ControlCenter {
    fn hot_zone_capability(&self) -> Option<tessera_desktop::hot_zone::HotZoneCapability> {
        Some(tessera_desktop::hot_zone::HotZoneTarget::ControlCenter.capability())
    }
    fn prepare_frame(
        &mut self,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
        self.prepare_frame(input);
    }

    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.frame_prepared {
            self.prepare_frame(input);
        }
        self.frame_prepared = false;
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        self.prev_down = down;
        if !self.presented() {
            return;
        }
        let sheet = self.resolve_sheet(display);

        // A press outside both cards and any open popover dismisses the sheet.
        let on_popover = self
            .open_popover_bounds()
            .is_some_and(|rect| contains(rect, cursor.0, cursor.1));
        if pressed && self.open && !sheet.contains(cursor.0, cursor.1) && !on_popover {
            self.close(out);
        }
        let pressed = pressed && self.open;

        let reveal = ease_out_cubic(self.reveal());
        let slide = (1.0 - reveal) * layout::REVEAL_SLIDE;
        let shift = |rect: Rect| Rect {
            x: rect.x + slide,
            ..rect
        };

        // optics-ui stamps every built node with the context opacity, so one switch
        // fades a card's text, icons, images, and controls together.
        self.tip = None;
        f.set_opacity(reveal);
        self.render_controls_card(f, shift(sheet.controls), cursor, i18n, out);
        if sheet.notifications_visible() {
            self.render_notifications_card(f, shift(sheet.notifications), i18n, out);
        }
        if self.menu_open_for.is_some()
            && let Some(menu) = self.menu_snapshot()
            && Some(&menu.key) == self.menu_open_for.as_ref()
        {
            self.render_tray_menu(f, &menu, display, cursor, pressed);
        }
        self.render_tip(f, i18n);
        f.set_opacity(1.0);
    }

    fn captures_keyboard(&self) -> bool {
        self.presented()
    }

    fn key_char(&mut self, kc: &KeyChar, out: &mut ChromeEvents) {
        if !self.open {
            return;
        }
        if self.wifi_key_char(kc, out) {
            return;
        }
        if kc.keysym != tessera_primitives::input::XKB_KEY_Escape {
            return;
        }
        // Escape peels the innermost layer first: the tray popover, the
        // session strip, a detail page, then the sheet itself.
        if let Some(key) = self.menu_open_for.clone() {
            self.close_menu(key);
        } else if self.session_open {
            self.session_open = false;
        } else if self.page != Page::Main {
            self.show_page(Page::Main, out);
        } else {
            self.close(out);
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, out: &mut ChromeEvents) {
        match command {
            ChromeCommand::ToggleControlCenter => {
                if self.open {
                    self.close(out);
                } else {
                    self.open_sheet();
                }
            }
            ChromeCommand::CloseControlCenter | ChromeCommand::DismissModal if self.open => {
                self.close(out);
            }
            _ => {}
        }
    }

    fn control_center_active(&self) -> bool {
        self.presented()
    }

    /// While presented the sheet owns the whole pointer: a press outside the
    /// cards dismisses it and must not also reach the window beneath.
    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.presented()
    }

    fn pointer_damage_regions(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        // Hover feedback lands on the cards and on an open popover; a pointer
        // over the desktop changes nothing the sheet paints.
        self.footprint()
    }

    fn modal_active(&self) -> bool {
        self.presented()
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        (self.presented() && self.sheet.contains(x, y)).then_some(CursorShape::Pointer)
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::AppCatalog(catalog) => self.icons = catalog.icons.clone(),
            ChromeUpdate::SystemStatus(status) => self.status = status.clone(),
            ChromeUpdate::Appearance(design) => self.design = *design,
            ChromeUpdate::Settings(snapshot) => {
                self.preferences = Some(snapshot.preferences.clone());
            }
            ChromeUpdate::SpaceUse(space_use) => {
                // A client that takes the whole output takes it from the sheet.
                if space_use.client_owns_output() && self.open {
                    self.close(&mut ChromeEvents::default());
                }
            }
            ChromeUpdate::ModalReserved(reserved) => self.reserved = reserved,
            ChromeUpdate::ReducedMotion(reduced) => {
                self.reduced_motion = reduced;
                if reduced && (self.reveal.value - self.reveal_target()).abs() > 0.0 {
                    self.reveal.snap_to(self.reveal_target());
                    self.retire_frames = RETIRE_FRAMES;
                }
            }
            _ => {}
        }
    }

    fn anim_pending(&self) -> bool {
        !self.reveal_settled()
            || self.avatar_animating()
            || (self.presented() && self.interaction_anim_pending())
    }

    fn damage_pending(&self) -> bool {
        self.retire_frames > 0
    }

    fn damage_regions(&self, _windows: &[Window], _display: (f32, f32)) -> Option<Vec<Rect>> {
        if !self.active() {
            return None;
        }
        self.footprint()
    }

    fn requires_composition(&self) -> bool {
        self.active()
    }
}

impl ControlCenter {
    /// Every pixel the sheet may change: both cards with their halo and reveal
    /// slide, plus the tray popover.
    fn footprint(&self) -> Option<Vec<Rect>> {
        let mut rects = self.sheet.footprint(self.halo_reach());
        if self.menu_open_for.is_some()
            && let Some(key) = self.menu_open_for.as_deref()
            && let Some(cache) = &self.menu_cache
            && let Some(menu) = cache.menu.as_ref().filter(|menu| menu.key == key)
            && let Some(visible) = tray::visible_children(&menu.root, &self.menu_path)
        {
            rects.push(menu_bounds(self.menu_owner, visible, self.display));
        }
        Some(rects)
    }
}
