mod auth;
mod profile;
mod render;
mod style;

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use profile::Profile;
use render::{Graphics, LockRenderSurface};
use tessera_lock::{AuthResult, LockAction, LockState, PresentationMode};
use tessera_wl_client::{KeyEvent, Keysym, Modifiers, XkbKeyboard, run_event_loop};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
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
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::{self, ExtSessionLockManagerV1},
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{
        wp_viewport::{self, WpViewport},
        wp_viewporter::{self, WpViewporter},
    },
};

struct OutputSurface {
    output: WlOutput,
    wl_surface: WlSurface,
    lock_surface: ExtSessionLockSurfaceV1,
    render: Option<LockRenderSurface>,
    viewport: Option<WpViewport>,
    fractional_scale: Option<WpFractionalScaleV1>,
    logical_size: (u32, u32),
    /// Output density in 120ths (the `wp_fractional_scale_v1` wire unit): the
    /// compositor's fractional hint when it provides one, otherwise the
    /// integer `wl_output` scale × 120.
    scale_120: u32,
}

impl OutputSurface {
    fn scale_factor(&self) -> f32 {
        self.scale_120 as f32 / 120.0
    }
}

struct AppData {
    connection: Connection,
    compositor: WlCompositor,
    _lock_manager: ExtSessionLockManagerV1,
    session_lock: Option<ExtSessionLockV1>,
    is_locked: bool,
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    output_scales: HashMap<ObjectId, i32>,
    output_names: HashMap<u32, WlOutput>,
    outputs: Vec<OutputSurface>,
    keyboards: Vec<WlKeyboard>,
    xkb: XkbKeyboard,
    pointers: Vec<WlPointer>,
    pointer_positions: HashMap<u32, (WlSurface, (f64, f64))>,
    touches: Vec<WlTouch>,
    graphics: Graphics,
    profile: Profile,
    lock_state: LockState,
    auth_tx: Sender<AuthResult>,
    auth_rx: Receiver<AuthResult>,
    ready_fd: Option<OwnedFd>,
    modifiers: Modifiers,
    active_layout: u32,
    layout_names: Vec<String>,
    touch_points: HashMap<i32, (WlSurface, (f64, f64))>,
    visual_progress: f32,
    feedback_was_animating: bool,
    last_update: Instant,
    last_clock_minute: u64,
    dirty: bool,
    unlock_pending: bool,
    exit: bool,
    fatal: Option<String>,
}

fn main() {
    tessera_env::init("info");
    if let Err(error) = run() {
        log::error!("lock: {error}");
        eprintln!("tessera-lock: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let ready_fd = parse_args()?;
    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
    let connection = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<AppData>(&connection)?;
    let qh = event_queue.handle();
    let (auth_tx, auth_rx) = mpsc::channel();
    let now = Instant::now();

    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let lock_manager: ExtSessionLockManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let session_lock = lock_manager.lock(&qh, ());
    let _seat: Option<WlSeat> = globals.bind(&qh, 1..=9, ()).ok();

    let mut initial_outputs = Vec::new();
    let mut output_names = HashMap::new();
    globals.contents().with_list(|list| {
        for global in list {
            if global.interface == "wl_output" {
                let version = global.version.min(4);
                let out: WlOutput = globals.registry().bind(global.name, version, &qh, ());
                output_names.insert(global.name, out.clone());
                initial_outputs.push(out);
            }
        }
    });

    let fractional_scale_manager: Option<WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    if fractional_scale_manager.is_none() || viewporter.is_none() {
        log::info!("lock: fractional scaling unavailable; using integer buffer scale");
    }
    let mut app = AppData {
        connection: connection.clone(),
        compositor,
        _lock_manager: lock_manager,
        session_lock: Some(session_lock),
        is_locked: false,
        fractional_scale_manager,
        viewporter,
        output_scales: HashMap::new(),
        output_names,
        outputs: Vec::new(),
        keyboards: Vec::new(),
        xkb: XkbKeyboard::new(),
        pointers: Vec::new(),
        pointer_positions: HashMap::new(),
        touches: Vec::new(),
        graphics: Graphics::new(&connection)?,
        profile: Profile::current()?,
        lock_state: LockState::new(now),
        auth_tx,
        auth_rx,
        ready_fd,
        modifiers: Modifiers::default(),
        active_layout: 0,
        layout_names: Vec::new(),
        touch_points: HashMap::new(),
        visual_progress: 1.0,
        feedback_was_animating: false,
        last_update: now,
        last_clock_minute: clock_minute(),
        dirty: true,
        unlock_pending: false,
        exit: false,
        fatal: None,
    };

    for output in initial_outputs {
        app.ensure_output(output, &qh);
    }

    run_event_loop(
        &connection,
        &mut event_queue,
        &mut app,
        None,
        |app| {
            app.advance();
            if app.dirty {
                app.render_all();
            }
            app.maybe_unlock();
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

impl AppData {
    fn ensure_output(&mut self, output: WlOutput, qh: &QueueHandle<Self>) {
        if self.outputs.iter().any(|entry| entry.output == output) {
            return;
        }
        let Some(lock) = self.session_lock.clone() else {
            return;
        };
        let surface = self.compositor.create_surface(qh, ());
        let lock_surface = lock.get_lock_surface(&surface, &output, qh, surface.clone());
        let integer_scale = self
            .output_scales
            .get(&output.id())
            .copied()
            .unwrap_or(1)
            .max(1);
        let (viewport, fractional_scale) = match (&self.viewporter, &self.fractional_scale_manager)
        {
            (Some(viewporter), Some(manager)) => {
                let viewport = viewporter.get_viewport(&surface, qh, ());
                let fractional = manager.get_fractional_scale(&surface, qh, surface.clone());
                (Some(viewport), Some(fractional))
            }
            _ => {
                surface.set_buffer_scale(integer_scale);
                (None, None)
            }
        };
        self.outputs.push(OutputSurface {
            output,
            wl_surface: surface,
            lock_surface,
            render: None,
            viewport,
            fractional_scale,
            logical_size: (1, 1),
            scale_120: integer_scale as u32 * 120,
        });
        self.dirty = true;
    }

    fn remove_output(&mut self, output: &WlOutput) {
        let Some(index) = self
            .outputs
            .iter()
            .position(|entry| &entry.output == output)
        else {
            return;
        };
        let mut removed = self.outputs.remove(index);
        if let Some(render) = removed.render.take() {
            self.graphics.destroy_surface(render);
        }
        if let Some(fractional) = removed.fractional_scale.take() {
            fractional.destroy();
        }
        if let Some(viewport) = removed.viewport.take() {
            viewport.destroy();
        }
        removed.lock_surface.destroy();
        removed.wl_surface.destroy();
    }

    fn update_output(&mut self, output: &WlOutput) {
        let scale = self
            .output_scales
            .get(&output.id())
            .copied()
            .unwrap_or(1)
            .max(1);
        if let Some(entry) = self
            .outputs
            .iter_mut()
            .find(|entry| &entry.output == output)
            && entry.fractional_scale.is_none()
            && entry.scale_120 != scale as u32 * 120
        {
            entry.scale_120 = scale as u32 * 120;
            entry.wl_surface.set_buffer_scale(scale);
            let factor = entry.scale_factor();
            if let Some(render) = &mut entry.render
                && let Err(error) = render.resize(entry.logical_size, factor)
            {
                self.fail(error.to_string());
                return;
            }
            self.dirty = true;
        }
    }

    fn configure_surface(
        &mut self,
        connection: &Connection,
        surface: &WlSurface,
        size: (u32, u32),
    ) {
        let Some(entry) = self
            .outputs
            .iter_mut()
            .find(|entry| &entry.wl_surface == surface)
        else {
            self.fail("received configure for an unknown lock surface".into());
            return;
        };
        entry.logical_size = (size.0.max(1), size.1.max(1));
        if let Some(viewport) = &entry.viewport {
            viewport.set_destination(entry.logical_size.0 as i32, entry.logical_size.1 as i32);
        }
        let scale = entry.scale_factor();
        let result = if let Some(render) = &mut entry.render {
            render.resize(entry.logical_size, scale)
        } else {
            self.graphics
                .create_surface(connection, &entry.wl_surface, entry.logical_size, scale)
                .map(|render| {
                    entry.render = Some(render);
                })
        };
        if let Err(error) = result {
            self.fail(error.to_string());
        } else {
            self.dirty = true;
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let now = Instant::now();
        let action = match event.keysym {
            Keysym::Return | Keysym::KP_Enter if self.securely_presented() => {
                self.lock_state.submit(now)
            }
            Keysym::Return | Keysym::KP_Enter => {
                self.lock_state.reveal(now);
                LockAction::None
            }
            Keysym::BackSpace | Keysym::Delete | Keysym::KP_Delete => {
                self.lock_state.backspace(now);
                LockAction::None
            }
            Keysym::Escape => {
                self.lock_state.clear(now);
                LockAction::None
            }
            Keysym::u | Keysym::U if self.modifiers.ctrl => {
                self.lock_state.clear(now);
                LockAction::None
            }
            _ => {
                if let Some(text) = event.utf8 {
                    self.lock_state.type_text(&text, now);
                } else {
                    self.lock_state.reveal(now);
                }
                LockAction::None
            }
        };
        self.handle_action(action);
        self.dirty = true;
    }

    fn handle_action(&mut self, action: LockAction) {
        match action {
            LockAction::None => {}
            LockAction::Authenticate(secret) => auth::authenticate_async(
                self.profile.username.clone(),
                secret,
                self.auth_tx.clone(),
            ),
            LockAction::Unlock => self.unlock_pending = true,
        }
    }

    fn pointer_activity(
        &mut self,
        _surface: &wl_surface::WlSurface,
        _position: (f64, f64),
        _submit: bool,
    ) {
        let now = Instant::now();
        if self.lock_state.reveal(now) {
            self.dirty = true;
        }
    }

    fn securely_presented(&self) -> bool {
        self.is_locked
    }

    fn advance(&mut self) {
        let now = Instant::now();
        if self.graphics.reload_avatar_if_ready() {
            self.dirty = true;
        }
        while let Ok(result) = self.auth_rx.try_recv() {
            let action = self.lock_state.authentication_finished(result, now);
            self.handle_action(action);
            self.dirty = true;
        }
        let (state_changed, action) = self.lock_state.tick(now);
        if state_changed {
            self.dirty = true;
        }
        self.handle_action(action);
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
            // Authentication results arrive through a polled std channel.
            // Keep dispatch latency bounded even when reduced motion turns
            // the visible validation sweep into a static treatment.
            || self.lock_state.validation_pending()
            || self.graphics.composition_animates(&self.lock_state)
            || self
                .graphics
                .feedback_animation_active(&self.lock_state, Instant::now())
    }

    fn render_all(&mut self) {
        self.dirty = false;
        let now = Instant::now();
        for entry in &mut self.outputs {
            let Some(render) = &mut entry.render else {
                continue;
            };
            if let Err(error) = self.graphics.render(
                render,
                &self.lock_state,
                &self.profile,
                self.visual_progress,
                now,
            ) {
                self.fail(error.to_string());
                break;
            }
        }
    }

    fn maybe_unlock(&mut self) {
        if !self.unlock_pending || !self.is_locked {
            return;
        }
        if let Some(lock) = self.session_lock.take() {
            log::info!("lock: authentication succeeded; unlocking session");
            lock.unlock_and_destroy();
            let _ = self.connection.flush();
            self.exit = true;
        }
    }

    fn signal_ready(&mut self) {
        let Some(fd) = self.ready_fd.take() else {
            return;
        };
        let mut file = File::from(fd);
        if let Err(error) = file.write_all(b"\n") {
            log::warn!("lock: failed to signal ready fd: {error}");
        }
    }

    fn fail(&mut self, message: String) {
        log::error!("lock: {message}");
        self.fatal.get_or_insert(message);
        self.exit = true;
    }
}

impl Drop for AppData {
    fn drop(&mut self) {
        let outputs = std::mem::take(&mut self.outputs);
        for mut output in outputs {
            if let Some(render) = output.render.take() {
                self.graphics.destroy_surface(render);
            }
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for AppData {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                let output: WlOutput = registry.bind(name, version.min(4), qh, ());
                state.output_names.insert(name, output.clone());
                state.ensure_output(output, qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.output_names.remove(&name) {
                    state.remove_output(&output);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCompositor, ()> for AppData {
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

impl Dispatch<WlSurface, ()> for AppData {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event
            && let Some(entry) = state
                .outputs
                .iter_mut()
                .find(|entry| &entry.wl_surface == surface)
            && entry.fractional_scale.is_none()
        {
            entry.scale_120 = factor.max(1) as u32 * 120;
            entry.wl_surface.set_buffer_scale(factor.max(1));
            let scale = entry.scale_factor();
            if let Some(render) = &mut entry.render
                && let Err(error) = render.resize(entry.logical_size, scale)
            {
                state.fail(error.to_string());
            }
            state.dirty = true;
        }
    }
}

impl Dispatch<WlOutput, ()> for AppData {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Scale { factor } => {
                state.output_scales.insert(output.id(), factor);
            }
            wl_output::Event::Done => {
                state.update_output(output);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &ExtSessionLockManagerV1,
        _: ext_session_lock_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockV1, ()> for AppData {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => {
                log::info!("lock: compositor confirmed secure presentation on all outputs");
                state.is_locked = true;
                state.signal_ready();
                state.dirty = true;
            }
            ext_session_lock_v1::Event::Finished if !state.exit => {
                state.fail("compositor denied or terminated the session lock".into());
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, WlSurface> for AppData {
    fn event(
        state: &mut Self,
        lock_surface: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        wl_surface: &WlSurface,
        connection: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            lock_surface.ack_configure(serial);
            state.configure_surface(connection, wl_surface, (width, height));
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: wp_fractional_scale_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewporter, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: wp_viewporter::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, WlSurface> for AppData {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let scale_120 = scale.max(1);
            let Some(entry) = state
                .outputs
                .iter_mut()
                .find(|entry| &entry.wl_surface == surface)
            else {
                return;
            };
            if entry.scale_120 == scale_120 {
                return;
            }
            entry.scale_120 = scale_120;
            let scale = entry.scale_factor();
            if let Some(render) = &mut entry.render
                && let Err(error) = render.resize(entry.logical_size, scale)
            {
                state.fail(error.to_string());
                return;
            }
            state.dirty = true;
        }
    }
}

impl Dispatch<WlSeat, ()> for AppData {
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

impl Dispatch<WlKeyboard, ()> for AppData {
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
            wl_keyboard::Event::Enter { .. } => {
                if state.lock_state.reveal(Instant::now()) {
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

impl Dispatch<WlPointer, ()> for AppData {
    fn event(
        state: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let pointer_id = pointer.id().protocol_id();
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                let pos = (surface_x, surface_y);
                state
                    .pointer_positions
                    .insert(pointer_id, (surface.clone(), pos));
                state.pointer_activity(&surface, pos, false);
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer_positions.remove(&pointer_id);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let pos = (surface_x, surface_y);
                if let Some((surface, prev)) = state.pointer_positions.get_mut(&pointer_id) {
                    let changed = *prev != pos;
                    *prev = pos;
                    let surface = surface.clone();
                    if changed {
                        state.pointer_activity(&surface, pos, false);
                    }
                }
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                if let Some((surface, pos)) = state.pointer_positions.get(&pointer_id).cloned() {
                    state.pointer_activity(&surface, pos, button == 0x110);
                }
            }
            wl_pointer::Event::Axis { value, .. } if value != 0.0 => {
                if let Some((surface, pos)) = state.pointer_positions.get(&pointer_id).cloned() {
                    state.pointer_activity(&surface, pos, false);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlTouch, ()> for AppData {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down {
                surface, id, x, y, ..
            } => {
                let pos = (x, y);
                state.touch_points.insert(id, (surface.clone(), pos));
                state.pointer_activity(&surface, pos, true);
            }
            wl_touch::Event::Up { id, .. } => {
                state.touch_points.remove(&id);
            }
            wl_touch::Event::Motion { id, x, y, .. } => {
                let pos = (x, y);
                let surface = state.touch_points.get(&id).map(|(s, _)| s.clone());
                if let Some(surface) = surface {
                    state.touch_points.insert(id, (surface.clone(), pos));
                    state.pointer_activity(&surface, pos, false);
                }
            }
            wl_touch::Event::Cancel => {
                state.touch_points.clear();
            }
            _ => {}
        }
    }
}

fn parse_args() -> Result<Option<OwnedFd>, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut ready_fd = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: tessera-lock [--ready-fd FD]\n\n\
                     Locks the current Tessera Wayland session. The ready fd is\n\
                     signalled only after every output has presented securely."
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("tessera-lock {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--ready-fd" => {
                let value = args.next().ok_or("--ready-fd requires a descriptor")?;
                ready_fd = Some(owned_fd(&value)?);
            }
            value if value.starts_with("--ready-fd=") => {
                ready_fd = Some(owned_fd(&value["--ready-fd=".len()..])?);
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    Ok(ready_fd)
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

fn clock_minute() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60
}
