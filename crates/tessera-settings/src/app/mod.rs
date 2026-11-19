//! The Settings window: one `xdg_toplevel`, its seat input, the compositor
//! link, and the single-instance listener, all driven by one `poll` loop.

pub mod clipboard;
pub mod gpu;
pub mod input;
pub mod instance;
pub mod link;

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tessera_i18n::{Localizer, Message};
use tessera_settings::SettingsHost;
use tessera_wl_client::{
    AxisScroll, KeyEvent, Keysym, Modifiers, WakeReceiver, WakeSender, WakeSignal, XkbKeyboard,
    pipe_pair, run_event_loop, wake_channel,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_compositor::{self, WlCompositor},
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::{self, WlDataDeviceManager},
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_keyboard::{self, KeyState, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_pointer::{self, Axis, ButtonState, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, Capability, WlSeat},
        wl_surface::{self, WlSurface},
    },
};
use wayland_protocols::wp::{
    cursor_shape::v1::client::{
        wp_cursor_shape_device_v1::{self, Shape, WpCursorShapeDeviceV1},
        wp_cursor_shape_manager_v1::{self, WpCursorShapeManagerV1},
    },
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{
        wp_viewport::{self, WpViewport},
        wp_viewporter::{self, WpViewporter},
    },
};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::{self, ZxdgDecorationManagerV1},
    zxdg_toplevel_decoration_v1::{self, Mode, ZxdgToplevelDecorationV1},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

use gpu::{Graphics, RenderSurface};
use input::InputState;
use link::{Link, LinkEvent};

const DEFAULT_SIZE: (u32, u32) = (960, 680);
const MIN_SIZE: (u32, u32) = (420, 360);

pub struct Options {
    pub socket: PathBuf,
    pub target: Option<String>,
    pub instance: Option<instance::Listener>,
}

type OfferEntry = (WlDataOffer, Arc<Mutex<Vec<String>>>);

struct App {
    connection: Connection,
    qh: QueueHandle<App>,
    _compositor: WlCompositor,
    wl_surface: WlSurface,
    _wm_base: XdgWmBase,
    _xdg_surface: XdgSurface,
    _xdg_toplevel: XdgToplevel,
    _decoration: Option<ZxdgToplevelDecorationV1>,
    cursor_shapes: Option<WpCursorShapeManagerV1>,
    _viewporter: Option<WpViewporter>,
    _fractional: Option<WpFractionalScaleManagerV1>,
    viewport: Option<WpViewport>,
    _fractional_scale: Option<WpFractionalScaleV1>,
    scale_120: u32,
    logical_size: (u32, u32),
    pending_configure_size: Option<(u32, u32)>,
    graphics: Graphics,
    surface: Option<RenderSurface>,
    keyboard: Option<WlKeyboard>,
    xkb: XkbKeyboard,
    pointer: Option<WlPointer>,
    pointer_on_surface: bool,
    pointer_pos: (f64, f64),
    pointer_axis: (AxisScroll, AxisScroll),
    cursor_device: Option<WpCursorShapeDeviceV1>,
    last_pointer_serial: u32,
    input_serial: u32,
    cursor_shape: Option<Shape>,
    data_device_manager: Option<WlDataDeviceManager>,
    data_device: Option<WlDataDevice>,
    offers: HashMap<wayland_client::backend::ObjectId, OfferEntry>,
    selection_offer: Option<wayland_client::backend::ObjectId>,
    _copy_source: Option<WlDataSource>,
    copy_text: Option<String>,
    clipboard_host: Box<clipboard::Host>,
    clipboard_reads: WakeSender<clipboard::ReadFinished>,
    link_rx: WakeReceiver<LinkEvent>,
    target_rx: WakeReceiver<String>,
    clipboard_rx: WakeReceiver<clipboard::HostEvent>,
    read_rx: WakeReceiver<clipboard::ReadFinished>,
    input: InputState,
    host: SettingsHost,
    i18n: Localizer,
    link: Link,
    last_frame: Instant,
    dirty: bool,
    animating: bool,
    exit: bool,
    fatal: Option<String>,
}

pub fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<App>(&connection)?;
    let qh = event_queue.handle();
    let wake = WakeSignal::new()?;

    let (link_tx, link_rx) = wake_channel::<LinkEvent>(wake.clone());
    let link = Link::spawn(options.socket, link_tx)?;
    let (target_tx, target_rx) = wake_channel::<String>(wake.clone());
    let _instance_guard = match options.instance {
        Some(listener) => Some(listener.serve(target_tx)?),
        None => None,
    };
    let (clipboard_tx, clipboard_rx) = wake_channel::<clipboard::HostEvent>(wake.clone());
    let (read_tx, read_rx) = wake_channel::<clipboard::ReadFinished>(wake.clone());
    let clipboard_host = clipboard::Host::new(clipboard_tx);

    let i18n = Localizer::from_env();
    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let wm_base: XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let data_device_manager: Option<WlDataDeviceManager> = globals.bind(&qh, 1..=3, ()).ok();
    let _seat: Option<WlSeat> = globals.bind(&qh, 1..=9, ()).ok();
    let decoration_manager: Option<ZxdgDecorationManagerV1> = globals.bind(&qh, 1..=1, ()).ok();

    let wl_surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&wl_surface, &qh, ());
    let xdg_toplevel = xdg_surface.get_toplevel(&qh, ());
    xdg_toplevel.set_title(i18n.text(Message::Settings).to_string());
    xdg_toplevel.set_app_id(tessera_settings::APP_ID.to_string());
    xdg_toplevel.set_min_size(MIN_SIZE.0 as i32, MIN_SIZE.1 as i32);
    let decoration = decoration_manager.map(|mgr| {
        let dec = mgr.get_toplevel_decoration(&xdg_toplevel, &qh, ());
        dec.set_mode(Mode::ServerSide);
        dec
    });
    wl_surface.commit();

    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let fractional: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let (viewport, fractional_scale) = match (&viewporter, &fractional) {
        (Some(viewporter), Some(manager)) => (
            Some(viewporter.get_viewport(&wl_surface, &qh, ())),
            Some(manager.get_fractional_scale(&wl_surface, &qh, ())),
        ),
        _ => (None, None),
    };

    let host = SettingsHost::new(options.target.as_deref());
    if let Some(target) = &options.target
        && !host.selected().as_str().eq_ignore_ascii_case(target.trim())
    {
        log::warn!("settings: unknown page {target:?}; opening the default page");
    }

    let mut app = App {
        connection: connection.clone(),
        qh: qh.clone(),
        _compositor: compositor,
        wl_surface,
        _wm_base: wm_base,
        _xdg_surface: xdg_surface,
        _xdg_toplevel: xdg_toplevel,
        _decoration: decoration,
        cursor_shapes: globals.bind(&qh, 1..=1, ()).ok(),
        _viewporter: viewporter,
        _fractional: fractional,
        viewport,
        _fractional_scale: fractional_scale,
        scale_120: 120,
        logical_size: DEFAULT_SIZE,
        pending_configure_size: None,
        graphics: Graphics::new(&connection)?,
        surface: None,
        keyboard: None,
        xkb: XkbKeyboard::new(),
        pointer: None,
        pointer_on_surface: false,
        pointer_pos: (0.0, 0.0),
        pointer_axis: (AxisScroll::default(), AxisScroll::default()),
        cursor_device: None,
        last_pointer_serial: 0,
        input_serial: 0,
        cursor_shape: None,
        data_device_manager,
        data_device: None,
        offers: HashMap::new(),
        selection_offer: None,
        _copy_source: None,
        copy_text: None,
        clipboard_host,
        clipboard_reads: read_tx,
        link_rx,
        target_rx,
        clipboard_rx,
        read_rx,
        input: InputState::default(),
        host,
        i18n,
        link,
        last_frame: Instant::now(),
        dirty: true,
        animating: false,
        exit: false,
        fatal: None,
    };

    run_event_loop(
        &connection,
        &mut event_queue,
        &mut app,
        Some(&wake),
        |app| {
            app.drain_channels();
            app.flush_outbox();
            if app.dirty || app.animating || app.input.has_edges() {
                app.render();
            }
        },
        |app| app.exit,
        |app| app.animating.then_some(Duration::from_millis(16)),
    )?;

    if let Some(surface) = app.surface.take() {
        app.graphics.destroy_surface(surface);
    }
    match app.fatal.take() {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

impl App {
    fn scale(&self) -> f32 {
        self.scale_120 as f32 / 120.0
    }

    fn fail(&mut self, message: String) {
        log::error!("settings: {message}");
        self.fatal = Some(message);
        self.exit = true;
    }

    fn drain_channels(&mut self) {
        while let Ok(event) = self.link_rx.try_recv() {
            self.link_event(event);
        }
        while let Ok(target) = self.target_rx.try_recv() {
            if !target.is_empty() && !self.host.navigate(&target) {
                log::warn!("settings: unknown page {target:?}");
            }
            self.link.focus_self();
            self.dirty = true;
        }
        while let Ok(event) = self.clipboard_rx.try_recv() {
            self.clipboard_event(event);
        }
        while let Ok(done) = self.read_rx.try_recv() {
            self.clipboard_read(done);
        }
    }

    fn link_event(&mut self, event: LinkEvent) {
        match event {
            LinkEvent::Snapshot(snapshot) => self.host.apply_snapshot(snapshot),
            LinkEvent::Rejected(detail) => self.host.rejected(detail),
            LinkEvent::Linked { read_only } => {
                if let Some(reason) = &read_only {
                    log::warn!("settings: read-only: {reason}");
                }
                self.host.linked(read_only);
            }
            LinkEvent::Lost(reason) => {
                log::debug!("settings: compositor link lost: {reason}");
                self.host.connection_lost(reason);
            }
        }
        self.dirty = true;
    }

    fn flush_outbox(&mut self) {
        for (expected_revision, action) in self.host.take_outbox() {
            self.link.apply(expected_revision, action);
        }
    }

    fn clipboard_event(&mut self, event: clipboard::HostEvent) {
        match event {
            clipboard::HostEvent::Paste => self.paste_from_clipboard(),
            clipboard::HostEvent::Copy(text) => self.set_clipboard_text(text),
        }
    }

    fn set_clipboard_text(&mut self, text: String) {
        let (Some(manager), Some(device)) =
            (self.data_device_manager.as_ref(), self.data_device.as_ref())
        else {
            log::warn!("settings: this seat has no clipboard device");
            return;
        };
        if self.input_serial == 0 {
            log::warn!("settings: no seat serial yet; cannot own the clipboard");
            return;
        }
        let source = manager.create_data_source(&self.qh, ());
        for mime in clipboard::TEXT_MIME_TYPES {
            source.offer(mime.to_string());
        }
        device.set_selection(Some(&source), self.input_serial);
        self._copy_source = Some(source);
        self.copy_text = Some(text);
    }

    fn paste_from_clipboard(&mut self) {
        let Some(offer_id) = self.selection_offer.as_ref() else {
            log::debug!("settings: paste requested with no selection offer");
            return;
        };
        let Some((offer, mimes)) = self.offers.get(offer_id) else {
            log::debug!("settings: paste requested with no selection offer");
            return;
        };
        let mime = {
            let guard = mimes.lock().unwrap_or_else(|e| e.into_inner());
            clipboard::pick_text_mime(&guard)
        };
        let Some(mime) = mime else {
            log::warn!("settings: the clipboard offers no text MIME type");
            return;
        };
        log::debug!("settings: reading clipboard as {mime}");
        match pipe_pair() {
            Ok((read_fd, write_fd)) => {
                use std::os::fd::AsFd;
                offer.receive(mime, write_fd.as_fd());
                clipboard::spawn_read(read_fd, self.clipboard_reads.clone());
            }
            Err(error) => log::warn!("settings: could not read the clipboard: {error}"),
        }
    }

    fn clipboard_read(&mut self, done: clipboard::ReadFinished) {
        match done.0 {
            Some(text) => {
                log::debug!("settings: pasting {} clipboard bytes", text.len());
                if let Some(surface) = &mut self.surface {
                    surface.paste(&text);
                    self.dirty = true;
                }
            }
            None => log::debug!("settings: clipboard read produced no text"),
        }
    }

    fn reconfigure(&mut self) {
        let scale = self.scale();
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(self.logical_size.0 as i32, self.logical_size.1 as i32);
        } else {
            self.wl_surface
                .set_buffer_scale(self.scale_120.div_ceil(120) as i32);
        }
        let result = match &mut self.surface {
            Some(surface) => surface.resize(self.logical_size, scale),
            None => self
                .graphics
                .create_surface(
                    &self.connection,
                    &self.wl_surface,
                    self.logical_size,
                    scale,
                    self.clipboard_host.as_user_data(),
                )
                .map(|surface| {
                    self.surface = Some(surface);
                }),
        };
        if let Err(error) = result {
            self.fail(format!("render surface: {error}"));
        }
        self.dirty = true;
    }

    fn render(&mut self) {
        let Some(surface) = &mut self.surface else {
            return;
        };
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .clamp(0.0, 0.1);
        self.last_frame = now;
        let size = surface.logical_size();
        let input = self.input.take_frame((size.0 as f32, size.1 as f32), dt);
        match surface.render(&mut self.host, &self.i18n, &input) {
            Ok(outcome) => {
                self.animating = outcome.animating;
                self.dirty = false;
                self.set_cursor(cursor_shape(outcome.cursor));
            }
            Err(error) => self.fail(format!("render: {error}")),
        }
    }

    fn set_cursor(&mut self, shape: Shape) {
        if self.cursor_shape == Some(shape) {
            return;
        }
        if let Some(device) = &self.cursor_device {
            device.set_shape(self.last_pointer_serial, shape);
            self.cursor_shape = Some(shape);
        }
    }

    fn handle_key(&mut self, event: KeyEvent, pressed: bool, repeat: bool) {
        if pressed && self.input.ctrl() {
            match event.keysym {
                Keysym::f => {
                    self.host.focus_search();
                    self.dirty = true;
                    return;
                }
                Keysym::q | Keysym::w => {
                    self.exit = true;
                    return;
                }
                _ => {}
            }
        }
        if pressed && event.keysym == Keysym::Escape && self.host.clear_search() {
            self.dirty = true;
            return;
        }
        self.input.key(&event, pressed, repeat);
        self.dirty = true;
    }
}

fn cursor_shape(hint: optics_lens::CursorHint) -> Shape {
    match hint {
        optics_lens::CursorHint::Default => Shape::Default,
        optics_lens::CursorHint::Pointer => Shape::Pointer,
        optics_lens::CursorHint::Text => Shape::Text,
        optics_lens::CursorHint::ResizeNs => Shape::NsResize,
        optics_lens::CursorHint::ResizeEw => Shape::EwResize,
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
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

impl Dispatch<WlCompositor, ()> for App {
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

impl Dispatch<WlSurface, ()> for App {
    fn event(
        app: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event
            && app.viewport.is_none()
        {
            let scale_120 = factor.max(1) as u32 * 120;
            if scale_120 != app.scale_120 {
                app.scale_120 = scale_120;
                if app.surface.is_some() {
                    app.reconfigure();
                }
            }
        }
    }
}

impl Dispatch<WlOutput, ()> for App {
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

impl Dispatch<XdgWmBase, ()> for App {
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

impl Dispatch<XdgSurface, ()> for App {
    fn event(
        app: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            if let Some(size) = app.pending_configure_size.take() {
                app.logical_size = size;
            }
            app.reconfigure();
        }
    }
}

impl Dispatch<XdgToplevel, ()> for App {
    fn event(
        app: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                let w = if width > 0 {
                    (width as u32).max(MIN_SIZE.0)
                } else {
                    app.logical_size.0.max(MIN_SIZE.0)
                };
                let h = if height > 0 {
                    (height as u32).max(MIN_SIZE.1)
                } else {
                    app.logical_size.1.max(MIN_SIZE.1)
                };
                app.pending_configure_size = Some((w, h));
            }
            xdg_toplevel::Event::Close => {
                app.exit = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZxdgDecorationManagerV1,
        _: zxdg_decoration_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        _: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for App {
    fn event(
        app: &mut Self,
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
            if app.data_device.is_none()
                && let Some(mgr) = &app.data_device_manager
            {
                app.data_device = Some(mgr.get_data_device(seat, qh, ()));
            }
            if caps.contains(Capability::Keyboard) && app.keyboard.is_none() {
                app.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(Capability::Pointer) && app.pointer.is_none() {
                let pointer = seat.get_pointer(qh, ());
                app.cursor_device = app
                    .cursor_shapes
                    .as_ref()
                    .map(|mgr| mgr.get_pointer(&pointer, qh, ()));
                app.pointer = Some(pointer);
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(
        app: &mut Self,
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
                app.xkb.load_keymap(fd.as_fd(), size);
            }
            wl_keyboard::Event::Enter { serial, .. } => {
                app.input_serial = serial;
                app.dirty = true;
            }
            wl_keyboard::Event::Leave { .. } => {
                app.input.modifiers(Modifiers::default());
                app.dirty = true;
            }
            wl_keyboard::Event::Key {
                time,
                key,
                state: WEnum::Value(state),
                ..
            } => {
                let pressed = state == KeyState::Pressed;
                let ev = app.xkb.key_event(key, time);
                app.handle_key(ev, pressed, false);
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                app.xkb
                    .update_modifiers(mods_depressed, mods_latched, mods_locked, group);
                app.input.modifiers(app.xkb.modifiers);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                if surface == app.wl_surface {
                    app.pointer_on_surface = true;
                    app.last_pointer_serial = serial;
                    app.cursor_shape = None;
                    app.pointer_pos = (surface_x, surface_y);
                    app.input.pointer_moved(app.pointer_pos);
                    app.dirty = true;
                }
            }
            wl_pointer::Event::Leave { surface, .. } => {
                if surface == app.wl_surface {
                    app.pointer_on_surface = false;
                    app.input.pointer_left();
                    app.dirty = true;
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if app.pointer_on_surface {
                    app.pointer_pos = (surface_x, surface_y);
                    app.input.pointer_moved(app.pointer_pos);
                    app.dirty = true;
                }
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state: WEnum::Value(state),
                ..
            } => {
                if app.pointer_on_surface {
                    app.last_pointer_serial = serial;
                    let pressed = state == ButtonState::Pressed;
                    if pressed {
                        app.input_serial = serial;
                        app.input.pointer_moved(app.pointer_pos);
                    }
                    app.input.button(button, pressed);
                    app.dirty = true;
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                if app.pointer_on_surface {
                    match axis {
                        Axis::HorizontalScroll => app.pointer_axis.0.absolute += value,
                        Axis::VerticalScroll => app.pointer_axis.1.absolute += value,
                        _ => {}
                    }
                }
            }
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                if app.pointer_on_surface {
                    match axis {
                        Axis::HorizontalScroll => app.pointer_axis.0.discrete += discrete,
                        Axis::VerticalScroll => app.pointer_axis.1.discrete += discrete,
                        _ => {}
                    }
                }
            }
            wl_pointer::Event::Frame
                if app.pointer_on_surface
                    && (app.pointer_axis.0 != AxisScroll::default()
                        || app.pointer_axis.1 != AxisScroll::default()) =>
            {
                let (h, v) = std::mem::take(&mut app.pointer_axis);
                app.input.axis(h, v);
                app.dirty = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: wl_data_device_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for App {
    fn event(
        app: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                let mimes = Arc::new(Mutex::new(Vec::new()));
                app.offers.insert(id.id(), (id, mimes));
            }
            wl_data_device::Event::Selection { id } => {
                app.selection_offer = id.map(|o| o.id());
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(App, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Arc::new(Mutex::new(Vec::new()))),
    ]);
}

impl Dispatch<WlDataOffer, Arc<Mutex<Vec<String>>>> for App {
    fn event(
        app: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        mimes: &Arc<Mutex<Vec<String>>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            if let Some((_, stored)) = app.offers.get(&offer.id()) {
                stored
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(mime_type);
            } else {
                mimes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(mime_type);
            }
        }
    }
}

impl Dispatch<WlDataSource, ()> for App {
    fn event(
        app: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                if !app
                    ._copy_source
                    .as_ref()
                    .is_some_and(|current| current == source)
                {
                    return;
                }
                if let Some(text) = &app.copy_text {
                    let mut file = File::from(fd);
                    if let Err(error) = file.write_all(text.as_bytes()) {
                        log::warn!("settings: clipboard write failed: {error}");
                    }
                }
            }
            wl_data_source::Event::Cancelled
                if app
                    ._copy_source
                    .as_ref()
                    .is_some_and(|current| current == source) =>
            {
                app._copy_source = None;
                app.copy_text = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<WpCursorShapeManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeManagerV1,
        _: wp_cursor_shape_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpCursorShapeDeviceV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeDeviceV1,
        _: wp_cursor_shape_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewporter, ()> for App {
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

impl Dispatch<WpViewport, ()> for App {
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

impl Dispatch<WpFractionalScaleManagerV1, ()> for App {
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

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let scale_120 = scale.max(1);
            if scale_120 != app.scale_120 {
                app.scale_120 = scale_120;
                if app.surface.is_some() {
                    app.reconfigure();
                }
            }
        }
    }
}
