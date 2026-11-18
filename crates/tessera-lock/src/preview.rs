//! Development-only xdg-shell host for the production lock-screen renderer.

use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tessera_lock::{AuthResult, LockAction, LockScreenStyle, LockState, PresentationMode, Secret};
use tessera_wl_client::{KeyEvent, Keysym, Modifiers, XkbKeyboard, run_event_loop};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_compositor::{self, WlCompositor},
        wl_keyboard::{self, KeyState, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_pointer::{self, ButtonState, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, Capability, WlSeat},
        wl_surface::{self, WlSurface},
        wl_touch::{self, WlTouch},
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

use crate::profile::Profile;
use crate::render::{Graphics, GraphicsOptions, LockRenderSurface};

const DEFAULT_SIZE: (u32, u32) = (1280, 800);
const MAX_DIMENSION: u32 = 16_384;
const SIMULATED_AUTH_DELAY: Duration = Duration::from_millis(520);
const APP_ID: &str = "dev.tessera.LockPreview";
const TITLE: &str = "Tessera Lock Preview";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InitialState {
    #[default]
    Ready,
    Typing,
    Checking,
    Rejected,
    Unavailable,
    Ambient,
}

impl InitialState {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ready" => Ok(Self::Ready),
            "typing" => Ok(Self::Typing),
            "checking" => Ok(Self::Checking),
            "rejected" => Ok(Self::Rejected),
            "unavailable" => Ok(Self::Unavailable),
            "ambient" => Ok(Self::Ambient),
            _ => Err(format!(
                "invalid preview state {value:?}; expected ready, typing, checking, rejected, unavailable, or ambient"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SimulatedResult {
    #[default]
    Accepted,
    Rejected,
    Unavailable,
}

impl SimulatedResult {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(format!(
                "invalid simulated result {value:?}; expected accepted, rejected, or unavailable"
            )),
        }
    }
}

#[derive(Debug)]
struct Options {
    state: InitialState,
    result: SimulatedResult,
    password: Option<Secret>,
    size: (u32, u32),
    style: Option<LockScreenStyle>,
    background: Option<PathBuf>,
    ready_fd: Option<OwnedFd>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            state: InitialState::Ready,
            result: SimulatedResult::Accepted,
            password: None,
            size: DEFAULT_SIZE,
            style: None,
            background: None,
            ready_fd: None,
        }
    }
}

enum ParsedCommand {
    Run(Options),
    Help,
    Version,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = match parse_args(std::env::args().skip(1))? {
        ParsedCommand::Run(options) => options,
        ParsedCommand::Help => {
            print_help();
            return Ok(());
        }
        ParsedCommand::Version => {
            println!("tessera-lock-preview {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    };

    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
    let connection = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<PreviewApp>(&connection)?;
    let qh = event_queue.handle();

    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let wm_base: XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let _seat: Option<WlSeat> = globals.bind(&qh, 1..=9, ()).ok();
    let wl_surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&wl_surface, &qh, ());
    let xdg_toplevel = xdg_surface.get_toplevel(&qh, ());
    xdg_toplevel.set_title(TITLE.to_string());
    xdg_toplevel.set_app_id(APP_ID.to_string());
    xdg_toplevel.set_min_size(options.size.0 as i32, options.size.1 as i32);
    xdg_toplevel.set_max_size(options.size.0 as i32, options.size.1 as i32);
    wl_surface.commit();

    let now = Instant::now();
    let (lock_state, visual_progress, advance_deadlines) = initial_lock_state(options.state, now);
    let profile = Profile::current()?;
    log::debug!(
        "lock preview: rendering the current profile for {:?}",
        profile.username
    );
    let graphics = if options.style.is_none() && options.background.is_none() {
        Graphics::new(&connection)?
    } else {
        Graphics::new_with_options(
            &connection,
            GraphicsOptions {
                style: options.style,
                background: options.background,
            },
        )?
    };
    let mut app = PreviewApp {
        connection: connection.clone(),
        _compositor: compositor,
        wl_surface,
        _wm_base: wm_base,
        _xdg_surface: xdg_surface,
        _xdg_toplevel: xdg_toplevel,
        keyboards: Vec::new(),
        xkb: XkbKeyboard::new(),
        pointers: Vec::new(),
        pointer_pos: (0.0, 0.0),
        touches: Vec::new(),
        graphics,
        render: None,
        profile,
        lock_state,
        simulated_result: options.result,
        expected_password: options.password,
        pending_auth: None,
        logical_size: options.size,
        pending_configure_size: None,
        scale: 1,
        modifiers: Modifiers::default(),
        active_layout: 0,
        layout_names: Vec::new(),
        visual_progress,
        feedback_was_animating: false,
        advance_deadlines,
        last_update: now,
        last_clock_minute: clock_minute(),
        ready_fd: options.ready_fd,
        ready_signaled: false,
        dirty: false,
        exit: false,
        fatal: None,
    };

    run_event_loop(
        &connection,
        &mut event_queue,
        &mut app,
        None,
        |app| {
            app.advance();
            if app.dirty {
                app.render();
            }
        },
        |app| app.exit,
        |app| {
            Some(if app.dirty || app.is_animating() {
                Duration::from_millis(16)
            } else {
                Duration::from_secs(1)
            })
        },
    )?;
    if let Some(error) = app.fatal.take() {
        return Err(error.into());
    }
    Ok(())
}

struct PreviewApp {
    connection: Connection,
    _compositor: WlCompositor,
    wl_surface: WlSurface,
    _wm_base: XdgWmBase,
    _xdg_surface: XdgSurface,
    _xdg_toplevel: XdgToplevel,
    keyboards: Vec<WlKeyboard>,
    xkb: XkbKeyboard,
    pointers: Vec<WlPointer>,
    pointer_pos: (f64, f64),
    touches: Vec<WlTouch>,
    graphics: Graphics,
    render: Option<LockRenderSurface>,
    profile: Profile,
    lock_state: LockState,
    simulated_result: SimulatedResult,
    expected_password: Option<Secret>,
    pending_auth: Option<PendingAuth>,
    logical_size: (u32, u32),
    pending_configure_size: Option<(u32, u32)>,
    scale: i32,
    modifiers: Modifiers,
    active_layout: u32,
    layout_names: Vec<String>,
    visual_progress: f32,
    feedback_was_animating: bool,
    advance_deadlines: bool,
    last_update: Instant,
    last_clock_minute: u64,
    ready_fd: Option<OwnedFd>,
    ready_signaled: bool,
    dirty: bool,
    exit: bool,
    fatal: Option<String>,
}

struct PendingAuth {
    result: AuthResult,
    complete_at: Instant,
}

impl PreviewApp {
    fn configure(&mut self) {
        let size = self
            .pending_configure_size
            .take()
            .unwrap_or(self.logical_size);
        self.logical_size = (size.0.max(1), size.1.max(1));
        let connection = &self.connection;
        let result = if let Some(render) = &mut self.render {
            render.resize(self.logical_size, self.scale as f32)
        } else {
            self.graphics
                .create_surface(
                    connection,
                    &self.wl_surface,
                    self.logical_size,
                    self.scale as f32,
                )
                .map(|render| self.render = Some(render))
        };
        if let Err(error) = result {
            self.fail(error.to_string());
        } else {
            self.dirty = true;
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let now = Instant::now();
        self.advance_deadlines = true;
        match event.keysym {
            Keysym::Escape => self.exit = true,
            Keysym::Return | Keysym::KP_Enter => {
                let action = self.lock_state.submit(now);
                self.handle_action(action, now);
            }
            Keysym::BackSpace | Keysym::Delete | Keysym::KP_Delete => {
                self.lock_state.backspace(now);
            }
            Keysym::u | Keysym::U if self.modifiers.ctrl => {
                self.lock_state.clear(now);
            }
            _ => {
                if let Some(text) = event.utf8 {
                    self.lock_state.type_text(&text, now);
                } else {
                    self.lock_state.reveal(now);
                }
            }
        }
        self.dirty = true;
    }

    fn handle_action(&mut self, action: LockAction, now: Instant) {
        let LockAction::Authenticate(secret) = action else {
            return;
        };
        let result = simulated_auth_result(
            self.simulated_result,
            &secret,
            self.expected_password.as_ref(),
        );
        self.pending_auth = Some(PendingAuth {
            result,
            complete_at: now + SIMULATED_AUTH_DELAY,
        });
    }

    fn pointer_activity(&mut self, _position: (f64, f64), _submit: bool) {
        let now = Instant::now();
        if self.lock_state.reveal(now) {
            self.dirty = true;
        }
    }

    fn advance(&mut self) {
        let now = Instant::now();
        if self.graphics.reload_avatar_if_ready() {
            self.dirty = true;
        }
        let (state_changed, action) = if self.advance_deadlines {
            self.lock_state.tick(now)
        } else {
            (false, LockAction::None)
        };
        if state_changed {
            self.dirty = true;
        }
        self.handle_action(action, now);
        if self
            .pending_auth
            .as_ref()
            .is_some_and(|pending| now >= pending.complete_at)
        {
            let pending = self.pending_auth.take().expect("pending auth checked");
            if matches!(
                self.lock_state.authentication_finished(pending.result, now),
                LockAction::Unlock
            ) {
                log::info!("lock preview: simulated authentication accepted; closing preview");
                self.exit = true;
            }
            self.dirty = true;
        }
        let feedback_animating = self
            .graphics
            .feedback_animation_active(&self.lock_state, now);
        if feedback_animating || self.feedback_was_animating {
            self.dirty = true;
        }
        self.feedback_was_animating = feedback_animating;

        let target = if self.lock_state.presentation() == PresentationMode::Engaged {
            1.0
        } else {
            0.0
        };
        let elapsed = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;
        let avatar_visible = self.lock_state.presentation() == PresentationMode::Engaged
            || self.visual_progress > 0.02;
        if avatar_visible {
            match self.graphics.advance_avatar(elapsed) {
                Ok(true) => self.dirty = true,
                Ok(false) => {}
                Err(error) => {
                    self.fail(error.to_string());
                    return;
                }
            }
        }
        if (self.visual_progress - target).abs() > 0.001 {
            let step = elapsed / 0.24;
            if target > self.visual_progress {
                self.visual_progress = (self.visual_progress + step).min(target);
            } else {
                self.visual_progress = (self.visual_progress - step).max(target);
            }
            self.dirty = true;
        }
        let minute = clock_minute();
        if minute != self.last_clock_minute {
            self.last_clock_minute = minute;
            self.dirty = true;
        }
    }

    fn is_animating(&self) -> bool {
        let target = if self.lock_state.presentation() == PresentationMode::Engaged {
            1.0
        } else {
            0.0
        };
        let avatar_visible = self.lock_state.presentation() == PresentationMode::Engaged
            || self.visual_progress > 0.02;
        (self.visual_progress - target).abs() > 0.001
            || (avatar_visible && self.graphics.avatar_is_animated())
            || self.graphics.avatar_reload_pending()
            || self.lock_state.validation_pending()
            || self.graphics.composition_animates(&self.lock_state)
            || self
                .graphics
                .feedback_animation_active(&self.lock_state, Instant::now())
    }

    fn render(&mut self) {
        self.dirty = false;
        let Some(render) = &mut self.render else {
            return;
        };
        if let Err(error) = self.graphics.render(
            render,
            &self.lock_state,
            &self.profile,
            self.visual_progress,
            Instant::now(),
        ) {
            self.fail(error.to_string());
            return;
        }
        self.signal_ready();
    }

    fn signal_ready(&mut self) {
        if self.ready_signaled {
            return;
        }
        self.ready_signaled = true;
        if let Some(fd) = self.ready_fd.take() {
            let mut file = File::from(fd);
            if let Err(error) = file.write_all(b"\n") {
                log::warn!("lock preview: failed to signal ready fd: {error}");
            }
        }
        log::info!("lock preview: first frame presented");
    }

    fn fail(&mut self, message: String) {
        log::error!("lock preview: {message}");
        self.fatal.get_or_insert(message);
        self.exit = true;
    }
}

impl Drop for PreviewApp {
    fn drop(&mut self) {
        if let Some(render) = self.render.take() {
            self.graphics.destroy_surface(render);
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for PreviewApp {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCompositor, ()> for PreviewApp {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event
            && surface == &state.wl_surface
        {
            state.scale = factor.max(1);
            state.wl_surface.set_buffer_scale(state.scale);
            if let Some(render) = &mut state.render
                && let Err(error) = render.resize(state.logical_size, state.scale as f32)
            {
                state.fail(error.to_string());
                return;
            }
            state.dirty = true;
        }
    }
}

impl Dispatch<WlOutput, ()> for PreviewApp {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for PreviewApp {
    fn event(
        _: &mut Self,
        wm: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.configure();
        }
    }
}

impl Dispatch<XdgToplevel, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                let w = if width > 0 {
                    width as u32
                } else {
                    state.logical_size.0
                };
                let h = if height > 0 {
                    height as u32
                } else {
                    state.logical_size.1
                };
                state.pending_configure_size = Some((w, h));
            }
            xdg_toplevel::Event::Close => {
                state.exit = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(Capability::Keyboard) && state.keyboards.is_empty() {
                state.keyboards.push(seat.get_keyboard(qh, ()));
            }
            if caps.contains(Capability::Pointer) && state.pointers.is_empty() {
                state.pointers.push(seat.get_pointer(qh, ()));
            }
            if caps.contains(Capability::Touch) && state.touches.is_empty() {
                state.touches.push(seat.get_touch(qh, ()));
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                use std::os::fd::AsFd;
                state.xkb.load_keymap(fd.as_fd(), size);
                state.layout_names = state.xkb.layout_names.clone();
                let layout = state
                    .layout_names
                    .get(state.active_layout as usize)
                    .cloned();
                state
                    .lock_state
                    .set_keyboard_status(state.modifiers.caps_lock, layout);
                state.dirty = true;
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                if surface == state.wl_surface && state.lock_state.reveal(Instant::now()) {
                    state.dirty = true;
                }
            }
            wl_keyboard::Event::Key {
                time,
                key,
                state: WEnum::Value(KeyState::Pressed),
                ..
            } => {
                let ev = state.xkb.key_event(key, time);
                state.handle_key(ev);
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state
                    .xkb
                    .update_modifiers(mods_depressed, mods_latched, mods_locked, group);
                state.modifiers = state.xkb.modifiers;
                state.active_layout = group;
                let layout = state.layout_names.get(group as usize).cloned();
                state
                    .lock_state
                    .set_keyboard_status(state.modifiers.caps_lock, layout);
                state.dirty = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } if surface == state.wl_surface => {
                state.pointer_pos = (surface_x, surface_y);
                state.pointer_activity(state.pointer_pos, false);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_pos = (surface_x, surface_y);
                state.pointer_activity(state.pointer_pos, false);
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                state.pointer_activity(state.pointer_pos, button == 0x110);
            }
            wl_pointer::Event::Axis { value, .. } if value != 0.0 => {
                state.pointer_activity(state.pointer_pos, false);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlTouch, ()> for PreviewApp {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down { surface, x, y, .. } if surface == state.wl_surface => {
                state.pointer_activity((x, y), true);
            }
            wl_touch::Event::Motion { x, y, .. } => {
                state.pointer_activity((x, y), false);
            }
            _ => {}
        }
    }
}

fn initial_lock_state(state: InitialState, now: Instant) -> (LockState, f32, bool) {
    let mut lock = LockState::new(now);
    match state {
        InitialState::Ready => (lock, 1.0, true),
        InitialState::Typing => {
            lock.type_text("preview", now);
            (lock, 1.0, false)
        }
        InitialState::Checking => {
            lock.type_text("preview", now);
            let _ = lock.submit(now);
            (lock, 1.0, false)
        }
        InitialState::Rejected => {
            lock.type_text("preview", now);
            let _ = lock.submit(now);
            let _ = lock.authentication_finished(
                AuthResult::Rejected {
                    message: localized(
                        "Incorrect password · Please wait before trying again",
                        "密码错误 · 请稍后重试",
                    ),
                },
                now,
            );
            (lock, 1.0, false)
        }
        InitialState::Unavailable => {
            lock.type_text("preview", now);
            let _ = lock.submit(now);
            let _ = lock.authentication_finished(
                AuthResult::Unavailable {
                    message: localized("Authentication unavailable", "认证服务不可用"),
                },
                now,
            );
            (lock, 1.0, false)
        }
        InitialState::Ambient => {
            let _ = lock.tick(now + Duration::from_secs(60));
            (lock, 0.0, false)
        }
    }
}

fn simulated_auth_result(
    result: SimulatedResult,
    submitted: &Secret,
    expected: Option<&Secret>,
) -> AuthResult {
    if let Some(expected) = expected {
        return if submitted.content_eq(expected) {
            AuthResult::Accepted
        } else {
            rejected_result()
        };
    }
    match result {
        SimulatedResult::Accepted => AuthResult::Accepted,
        SimulatedResult::Rejected => rejected_result(),
        SimulatedResult::Unavailable => AuthResult::Unavailable {
            message: localized("Authentication unavailable", "认证服务不可用"),
        },
    }
}

fn rejected_result() -> AuthResult {
    AuthResult::Rejected {
        message: localized(
            "Incorrect password · Please wait before trying again",
            "密码错误 · 请稍后重试",
        ),
    }
}

fn localized(en: &str, zh: &str) -> String {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if locale.starts_with("zh") {
        zh.to_owned()
    } else {
        en.to_owned()
    }
}

fn parse_args(
    args: impl IntoIterator<Item = String>,
) -> Result<ParsedCommand, Box<dyn std::error::Error>> {
    let mut options = Options::default();
    let mut result_explicit = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(ParsedCommand::Help),
            "--version" | "-V" => return Ok(ParsedCommand::Version),
            "--state" => {
                let value = args.next().ok_or("--state requires a value")?;
                options.state = InitialState::parse(&value)?;
            }
            "--result" => {
                let value = args.next().ok_or("--result requires a value")?;
                options.result = SimulatedResult::parse(&value)?;
                result_explicit = true;
            }
            "--password" => {
                let value = args.next().ok_or("--password requires a value")?;
                options.password = Some(parse_preview_password(&value)?);
            }
            "--size" => {
                let value = args.next().ok_or("--size requires WIDTHxHEIGHT")?;
                options.size = parse_size(&value)?;
            }
            "--composition" | "--style" => {
                let value = args.next().ok_or("--composition requires a value")?;
                options.style = Some(parse_composition(&value)?);
            }
            "--background" => {
                let value = args.next().ok_or("--background requires a path")?;
                options.background = Some(PathBuf::from(value));
            }
            "--ready-fd" => {
                let value = args.next().ok_or("--ready-fd requires a descriptor")?;
                options.ready_fd = Some(owned_fd(&value)?);
            }
            value if value.starts_with("--state=") => {
                options.state = InitialState::parse(&value["--state=".len()..])?;
            }
            value if value.starts_with("--result=") => {
                options.result = SimulatedResult::parse(&value["--result=".len()..])?;
                result_explicit = true;
            }
            value if value.starts_with("--password=") => {
                options.password = Some(parse_preview_password(&value["--password=".len()..])?);
            }
            value if value.starts_with("--size=") => {
                options.size = parse_size(&value["--size=".len()..])?;
            }
            value if value.starts_with("--composition=") => {
                options.style = Some(parse_composition(&value["--composition=".len()..])?);
            }
            value if value.starts_with("--style=") => {
                options.style = Some(parse_composition(&value["--style=".len()..])?);
            }
            value if value.starts_with("--background=") => {
                options.background = Some(PathBuf::from(&value["--background=".len()..]));
            }
            value if value.starts_with("--ready-fd=") => {
                options.ready_fd = Some(owned_fd(&value["--ready-fd=".len()..])?);
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    if options.password.is_some() && result_explicit {
        return Err("--password cannot be combined with --result".into());
    }
    Ok(ParsedCommand::Run(options))
}

fn parse_preview_password(value: &str) -> Result<Secret, String> {
    if value.is_empty() {
        return Err("preview password must not be empty".into());
    }
    let mut password = Secret::default();
    if !password.push_str(value) {
        return Err(format!(
            "preview password must not exceed {} UTF-8 bytes",
            Secret::MAX_BYTES
        ));
    }
    Ok(password)
}

fn parse_composition(value: &str) -> Result<LockScreenStyle, String> {
    match value {
        "centered" => Ok(LockScreenStyle::Centered),
        "cinematic" => Ok(LockScreenStyle::Cinematic),
        "bsod" => Ok(LockScreenStyle::Bsod),
        _ => Err(format!(
            "invalid lock-screen composition {value:?}; expected centered, cinematic, or bsod"
        )),
    }
}

fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let separator = value
        .find(['x', 'X'])
        .ok_or_else(|| format!("invalid preview size {value:?}; expected WIDTHxHEIGHT"))?;
    let width = value[..separator]
        .parse::<u32>()
        .map_err(|_| format!("invalid preview width in {value:?}"))?;
    let height = value[separator + 1..]
        .parse::<u32>()
        .map_err(|_| format!("invalid preview height in {value:?}"))?;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!(
            "preview size must be between 1x1 and {MAX_DIMENSION}x{MAX_DIMENSION}"
        ));
    }
    Ok((width, height))
}

fn owned_fd(value: &str) -> Result<OwnedFd, Box<dyn std::error::Error>> {
    let fd: i32 = value.parse()?;
    if fd < 0 {
        return Err("ready fd must be non-negative".into());
    }
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn print_help() {
    print!(
        "{}",
        concat!(
            "Usage: tessera-lock-preview [OPTIONS]\n\n",
            "Preview the production lock-screen renderer in an ordinary Wayland window.\n\n",
            "Options:\n",
            "  --state STATE       Initial state: ready, typing, checking, rejected,\n",
            "                      unavailable, or ambient [default: ready]\n",
            "  --result RESULT     Submission result: accepted, rejected, or unavailable\n",
            "                      [default: accepted]\n",
            "  --password TEXT     Accept only this development-only fake password\n",
            "                      (mutually exclusive with --result)\n",
            "  --size WIDTHxHEIGHT Fixed logical window size [default: 1280x800]\n",
            "  --composition NAME  UI composition: centered, cinematic, or bsod\n",
            "  --style NAME        Compatibility alias for --composition\n",
            "  --background PATH   Independent lock-screen image override\n",
            "  --ready-fd FD       Signal FD after the first frame is presented\n",
            "  -h, --help          Print help\n",
            "  -V, --version       Print version\n\n",
            "Escape closes the preview. No session lock or PAM authentication is used.\n",
            "Never pass a real account password: command arguments may be visible.\n",
        )
    );
}

fn clock_minute() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scoped_preview_options() {
        let ParsedCommand::Run(options) = parse_args([
            "--state=rejected".to_owned(),
            "--result".to_owned(),
            "unavailable".to_owned(),
            "--size".to_owned(),
            "1440x900".to_owned(),
            "--composition=cinematic".to_owned(),
            "--background".to_owned(),
            "lock.png".to_owned(),
        ])
        .unwrap() else {
            panic!("expected run options");
        };
        assert_eq!(options.state, InitialState::Rejected);
        assert_eq!(options.result, SimulatedResult::Unavailable);
        assert_eq!(options.size, (1440, 900));
        assert_eq!(options.style, Some(LockScreenStyle::Cinematic));
        assert_eq!(options.background, Some(PathBuf::from("lock.png")));
    }

    #[test]
    fn rejects_unknown_state_and_unsafe_dimensions() {
        assert!(InitialState::parse("accepted").is_err());
        assert!(parse_size("0x800").is_err());
        assert!(parse_size("20000x800").is_err());
        assert!(parse_size("wide").is_err());
        assert!(parse_composition("glass").is_err());
        assert_eq!(parse_composition("bsod"), Ok(LockScreenStyle::Bsod));
        let ParsedCommand::Run(alias) = parse_args(["--style=centered".to_owned()]).unwrap() else {
            panic!("expected run options");
        };
        assert_eq!(alias.style, Some(LockScreenStyle::Centered));
    }

    #[test]
    fn parses_fake_password_and_rejects_ambiguous_result_policy() {
        let ParsedCommand::Run(options) = parse_args(["--password=0000".to_owned()]).unwrap()
        else {
            panic!("expected run options");
        };
        let expected = parse_preview_password("0000").unwrap();
        assert!(
            options
                .password
                .as_ref()
                .is_some_and(|password| password.content_eq(&expected))
        );

        assert!(parse_args(["--password=".to_owned()]).is_err());
        assert!(
            parse_args(["--password=0000".to_owned(), "--result=rejected".to_owned(),]).is_err()
        );
    }

    #[test]
    fn fake_password_accepts_only_an_exact_match() {
        let expected = parse_preview_password("0000").unwrap();
        let matching = parse_preview_password("0000").unwrap();
        let wrong = parse_preview_password("0001").unwrap();

        assert!(matches!(
            simulated_auth_result(SimulatedResult::Accepted, &matching, Some(&expected)),
            AuthResult::Accepted
        ));
        assert!(matches!(
            simulated_auth_result(SimulatedResult::Accepted, &wrong, Some(&expected)),
            AuthResult::Rejected { .. }
        ));
    }

    #[test]
    fn constructs_each_visual_state_through_the_product_state_machine() {
        let now = Instant::now();
        let (ready, _, _) = initial_lock_state(InitialState::Ready, now);
        assert_eq!(ready.password_len(), 0);
        assert!(!ready.checking());

        let (typing, _, _) = initial_lock_state(InitialState::Typing, now);
        assert!(typing.password_len() > 0);

        let (checking, _, _) = initial_lock_state(InitialState::Checking, now);
        assert!(checking.checking());
        assert_eq!(checking.password_len(), "preview".chars().count());

        let (rejected, _, _) = initial_lock_state(InitialState::Rejected, now);
        assert!(rejected.message().is_some());

        let (unavailable, _, _) = initial_lock_state(InitialState::Unavailable, now);
        assert!(unavailable.message().is_some());

        let (ambient, progress, _) = initial_lock_state(InitialState::Ambient, now);
        assert_eq!(ambient.presentation(), PresentationMode::Ambient);
        assert_eq!(progress, 0.0);
    }
}
