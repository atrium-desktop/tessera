//! The Settings window: one `xdg_toplevel`, its seat input, the compositor
//! link, and the single-instance listener, all driven by one calloop loop.

pub mod clipboard;
pub mod gpu;
pub mod input;
pub mod instance;
pub mod link;

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    data_device_manager::{
        DataDeviceManagerState, WritePipe,
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer},
        data_source::{CopyPasteSource, DataSourceHandler},
    },
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{EventLoop, channel},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager,
        },
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
};
use tessera_i18n::{Localizer, Message};
use tessera_settings::SettingsHost;
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{
        wl_data_device::WlDataDevice, wl_data_device_manager::DndAction,
        wl_data_source::WlDataSource, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
    },
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::{self, WpViewporter},
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

struct App {
    connection: Connection,
    registry_state: RegistryState,
    _compositor_state: CompositorState,
    output_state: OutputState,
    seat_state: SeatState,
    cursor_shapes: Option<CursorShapeManager>,
    _viewporter: Option<WpViewporter>,
    _fractional: Option<WpFractionalScaleManagerV1>,
    window: Window,
    viewport: Option<WpViewport>,
    _fractional_scale: Option<WpFractionalScaleV1>,
    /// Density in 120ths: the fractional hint, else integer scale × 120.
    scale_120: u32,
    logical_size: (u32, u32),
    graphics: Graphics,
    surface: Option<RenderSurface>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    last_pointer_serial: u32,
    /// Latest seat serial, used to claim the clipboard selection.
    input_serial: u32,
    cursor_shape: Option<Shape>,
    qh: QueueHandle<App>,
    data_device_manager: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,
    _copy_source: Option<CopyPasteSource>,
    copy_text: Option<String>,
    /// The boxed bridge whose address Lens stores in `lens_desc.clipboard`.
    clipboard_host: Box<clipboard::Host>,
    clipboard_reads: channel::Sender<clipboard::ReadFinished>,
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
    let (globals, event_queue) = registry_queue_init::<App>(&connection)?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;

    let (link_tx, link_rx) = channel::channel::<LinkEvent>();
    let link = Link::spawn(options.socket, link_tx)?;
    let (target_tx, target_rx) = channel::channel::<String>();
    let _instance_guard = match options.instance {
        Some(listener) => Some(listener.serve(target_tx)?),
        None => None,
    };
    let (clipboard_tx, clipboard_rx) = channel::channel::<clipboard::HostEvent>();
    let (read_tx, read_rx) = channel::channel::<clipboard::ReadFinished>();
    let clipboard_host = clipboard::Host::new(clipboard_tx);

    let i18n = Localizer::from_env();
    let compositor_state = CompositorState::bind(&globals, &qh)?;
    let xdg_shell = XdgShell::bind(&globals, &qh)?;
    let data_device_manager = DataDeviceManagerState::bind(&globals, &qh).ok();
    let wl_surface = compositor_state.create_surface(&qh);
    let window = xdg_shell.create_window(wl_surface, WindowDecorations::RequestServer, &qh);
    window.set_title(i18n.text(Message::Settings));
    window.set_app_id(tessera_settings::APP_ID);
    window.set_min_size(Some(MIN_SIZE));
    window.commit();

    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let fractional: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let (viewport, fractional_scale) = match (&viewporter, &fractional) {
        (Some(viewporter), Some(manager)) => (
            Some(viewporter.get_viewport(window.wl_surface(), &qh, ())),
            Some(manager.get_fractional_scale(window.wl_surface(), &qh, ())),
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
        registry_state: RegistryState::new(&globals),
        _compositor_state: compositor_state,
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        cursor_shapes: CursorShapeManager::bind(&globals, &qh).ok(),
        _viewporter: viewporter,
        _fractional: fractional,
        window,
        viewport,
        _fractional_scale: fractional_scale,
        scale_120: 120,
        logical_size: DEFAULT_SIZE,
        graphics: Graphics::new(&connection)?,
        surface: None,
        keyboard: None,
        pointer: None,
        cursor_device: None,
        last_pointer_serial: 0,
        input_serial: 0,
        cursor_shape: None,
        qh: qh.clone(),
        data_device_manager,
        data_device: None,
        _copy_source: None,
        copy_text: None,
        clipboard_host,
        clipboard_reads: read_tx,
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

    let handle = event_loop.handle();
    WaylandSource::new(connection, event_queue).insert(handle.clone())?;
    handle.insert_source(link_rx, |event, _, app: &mut App| {
        if let channel::Event::Msg(event) = event {
            app.link_event(event);
        }
    })?;
    handle.insert_source(target_rx, |event, _, app: &mut App| {
        if let channel::Event::Msg(target) = event {
            if !target.is_empty() && !app.host.navigate(&target) {
                log::warn!("settings: unknown page {target:?}");
            }
            // A later invocation means "show me this page": switch the page
            // and raise the existing window. The command connection owns the
            // scoped focus request so the render loop never blocks on IPC.
            app.link.focus_self();
            app.dirty = true;
        }
    })?;
    handle.insert_source(clipboard_rx, |event, _, app: &mut App| {
        if let channel::Event::Msg(event) = event {
            app.clipboard_event(event);
        }
    })?;
    handle.insert_source(read_rx, |event, _, app: &mut App| {
        if let channel::Event::Msg(done) = event {
            app.clipboard_read(done);
        }
    })?;

    while !app.exit {
        let timeout = if app.animating {
            Some(Duration::from_millis(16))
        } else {
            None
        };
        event_loop.dispatch(timeout, &mut app)?;
        app.flush_outbox();
        if app.dirty || app.animating || app.input.has_edges() {
            app.render();
        }
    }
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
        let source = manager.create_copy_paste_source(&self.qh, clipboard::TEXT_MIME_TYPES);
        source.set_selection(device, self.input_serial);
        self._copy_source = Some(source);
        self.copy_text = Some(text);
    }

    fn paste_from_clipboard(&mut self) {
        let Some(device) = self.data_device.as_ref() else {
            log::warn!("settings: this seat has no clipboard device");
            return;
        };
        let Some(offer) = device.data().selection_offer() else {
            // An empty selection is not an error: Ctrl+V simply does nothing.
            log::debug!("settings: paste requested with no selection offer");
            return;
        };
        let Some(mime) = offer.with_mime_types(clipboard::pick_text_mime) else {
            log::warn!("settings: the clipboard offers no text MIME type");
            return;
        };
        log::debug!("settings: reading clipboard as {mime}");
        match offer.receive(mime) {
            Ok(pipe) => clipboard::spawn_read(pipe, self.clipboard_reads.clone()),
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

    /// Apply a new logical size or density to the buffer and viewport.
    fn reconfigure(&mut self, qh: &QueueHandle<Self>) {
        let scale = self.scale();
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(self.logical_size.0 as i32, self.logical_size.1 as i32);
        } else {
            self.window
                .wl_surface()
                .set_buffer_scale(self.scale_120.div_ceil(120) as i32);
        }
        let result = match &mut self.surface {
            Some(surface) => surface.resize(self.logical_size, scale),
            None => self
                .graphics
                .create_surface(
                    &self.connection,
                    self.window.wl_surface(),
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
        let _ = qh;
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

fn cursor_shape(hint: lens::CursorHint) -> Shape {
    match hint {
        lens::CursorHint::Default => Shape::Default,
        lens::CursorHint::Pointer => Shape::Pointer,
        lens::CursorHint::Text => Shape::Text,
        lens::CursorHint::ResizeNs => Shape::NsResize,
        lens::CursorHint::ResizeEw => Shape::EwResize,
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        self.logical_size = (
            width
                .map_or(self.logical_size.0, |w| w.get())
                .max(MIN_SIZE.0),
            height
                .map_or(self.logical_size.1, |h| h.get())
                .max(MIN_SIZE.1),
        );
        self.reconfigure(qh);
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // The fractional hint, when present, wins over the integer factor.
        if self.viewport.is_none() {
            let scale_120 = factor.max(1) as u32 * 120;
            if scale_120 != self.scale_120 {
                self.scale_120 = scale_120;
                self.reconfigure(qh);
            }
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.data_device.is_none() {
            self.data_device = self
                .data_device_manager
                .as_ref()
                .map(|manager| manager.get_data_device(&self.qh, &seat));
        }
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                match self.seat_state.get_keyboard(qh, &seat, None) {
                    Ok(keyboard) => self.keyboard = Some(keyboard),
                    Err(error) => log::warn!("settings: keyboard unavailable: {error}"),
                }
            }
            Capability::Pointer if self.pointer.is_none() => {
                match self.seat_state.get_pointer(qh, &seat) {
                    Ok(pointer) => {
                        self.cursor_device = self
                            .cursor_shapes
                            .as_ref()
                            .map(|manager| manager.get_shape_device(&pointer, qh));
                        self.pointer = Some(pointer);
                    }
                    Err(error) => log::warn!("settings: pointer unavailable: {error}"),
                }
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                if let Some(keyboard) = self.keyboard.take() {
                    keyboard.release();
                }
            }
            Capability::Pointer => {
                if let Some(device) = self.cursor_device.take() {
                    device.destroy();
                }
                if let Some(pointer) = self.pointer.take() {
                    pointer.release();
                }
                self.input.pointer_left();
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        serial: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.input_serial = serial;
        self.dirty = true;
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.input.modifiers(Modifiers::default());
        self.dirty = true;
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event, true, false);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event, true, true);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event, false, false);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.input.modifiers(modifiers);
    }
}

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &wl_surface::WlSurface,
    ) {
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}

    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &WlDataSource,
        _: String,
        mut fd: WritePipe,
    ) {
        // A queued send from a source we already replaced must not leak the
        // newer selection into the older consumer.
        if !self
            ._copy_source
            .as_ref()
            .is_some_and(|current| current.inner() == source)
        {
            return;
        }
        if let Some(text) = &self.copy_text
            && let Err(error) = fd.write_all(text.as_bytes())
        {
            log::warn!("settings: clipboard write failed: {error}");
        }
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        // Cancel events can trail the replacement that caused them. Only the
        // current source may clear the owner.
        if self
            ._copy_source
            .as_ref()
            .is_some_and(|current| current.inner() == source)
        {
            self._copy_source = None;
            self.copy_text = None;
        }
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.last_pointer_serial = serial;
                    // The compositor resets the cursor on enter.
                    self.cursor_shape = None;
                    self.input.pointer_moved(event.position);
                }
                PointerEventKind::Leave { .. } => self.input.pointer_left(),
                PointerEventKind::Motion { .. } => self.input.pointer_moved(event.position),
                PointerEventKind::Press { button, serial, .. } => {
                    self.last_pointer_serial = serial;
                    self.input_serial = serial;
                    self.input.pointer_moved(event.position);
                    self.input.button(button, true);
                }
                PointerEventKind::Release { button, serial, .. } => {
                    self.last_pointer_serial = serial;
                    self.input.button(button, false);
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => self.input.axis(horizontal, vertical),
            }
            self.dirty = true;
        }
    }
}

impl wayland_client::Dispatch<WpViewporter, ()> for App {
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

impl wayland_client::Dispatch<WpViewport, ()> for App {
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

impl wayland_client::Dispatch<WpFractionalScaleManagerV1, ()> for App {
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

impl wayland_client::Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let scale_120 = scale.max(1);
            if scale_120 != app.scale_120 {
                app.scale_120 = scale_120;
                if app.surface.is_some() {
                    app.reconfigure(qh);
                }
            }
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
