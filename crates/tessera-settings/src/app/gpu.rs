//! gfx/optics-ui rendering for the Settings window's single surface.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::ptr;

use ash::vk::{self, Handle};
use optics_ui::{CursorHint, Input, Ui};
use tessera_design::{Design, themes};
use tessera_i18n::Localizer;
use tessera_settings::SettingsHost;
use wayland_client::{Connection, Proxy, protocol::wl_surface};

use super::clipboard;

const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];
const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];

#[derive(Debug)]
pub enum RenderError {
    Vulkan,
    Gfx(optics_gfx::Error),
    Ui(optics_ui::Error),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan => f.write_str("Vulkan is unavailable"),
            Self::Gfx(err) => write!(f, "{err}"),
            Self::Ui(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gfx(err) => Some(err),
            Self::Ui(err) => Some(err),
            Self::Vulkan => None,
        }
    }
}

impl From<optics_gfx::Error> for RenderError {
    fn from(err: optics_gfx::Error) -> Self {
        Self::Gfx(err)
    }
}

impl From<optics_ui::Error> for RenderError {
    fn from(err: optics_ui::Error) -> Self {
        Self::Ui(err)
    }
}

/// What one rendered frame asks of the event loop.
pub struct FrameOutcome {
    /// An eased widget value is still moving; draw again next frame.
    pub animating: bool,
    pub cursor: CursorHint,
}

pub struct Graphics {
    device: optics_gfx::Device,
    ash: AshBridge,
}

impl Graphics {
    pub fn new(connection: &Connection) -> Result<Self, RenderError> {
        let device = optics_gfx::Device::new(true, &INSTANCE_EXTENSIONS, &DEVICE_EXTENSIONS, 2)?;
        let ash = AshBridge::new(connection, &device)?;
        Ok(Self { device, ash })
    }

    pub fn create_surface(
        &self,
        connection: &Connection,
        wl_surface: &wl_surface::WlSurface,
        logical_size: (u32, u32),
        scale: f32,
        clipboard_user: *mut c_void,
    ) -> Result<RenderSurface, RenderError> {
        let physical = physical_size(logical_size, scale);
        let vk_surface = self.ash.create_surface(connection, wl_surface)?;
        // SAFETY: `vk_surface` was created on this device's instance and lives
        // until `destroy_surface`, which drops the gfx surface first.
        let surface = match unsafe {
            optics_gfx::Surface::from_vk(
                &self.device,
                vk_surface.as_raw() as usize as *mut c_void,
                physical.0,
                physical.1,
                true,
            )
        } {
            Ok(surface) => surface,
            Err(error) => {
                self.ash.destroy_surface(vk_surface);
                return Err(error.into());
            }
        };
        let canvas = optics_gfx::Canvas::new(&surface)?;
        // SAFETY: the device outlives the Ui; both are owned by `Graphics`'
        // caller in drop order (surface before graphics). The clipboard
        // pointer is the boxed `clipboard::Host` owned by `App`; it outlives
        // this surface and is stable across resizes.
        let mut raw = ptr::null_mut();
        let desc = optics_ui::sys::optics_ui_desc {
            size: std::mem::size_of::<optics_ui::sys::optics_ui_desc>() as u32,
            device: self
                .device
                .as_raw()
                .cast::<optics_ui::sys::optics_gfx_device>(),
            theme: unsafe { optics_ui::sys::optics_ui_theme_dark() },
            clipboard: optics_ui::sys::optics_ui_clipboard {
                request_text: Some(clipboard::request_text),
                set_text: Some(clipboard::set_text),
                user: clipboard_user,
            },
            ..unsafe { std::mem::zeroed() }
        };
        let result = unsafe { optics_ui::sys::optics_ui_create(&desc, &mut raw) };
        if result != optics_ui::sys::optics_gfx_result::OPTICS_GFX_OK || raw.is_null() {
            return Err(RenderError::Ui(optics_ui::Error::Create(
                optics_ui::GfxDiagnostic::for_test(result),
            )));
        }
        let owned_ui = OwnedUi(raw);
        let mut ui = unsafe { Ui::borrow_raw(raw) };
        // Glyphs are host-registered per context since Optics v0.0.88.
        if let Err(error) = tessera_design::icons::install(&mut ui) {
            log::error!("settings: glyphs unavailable: {error}");
        }
        ui.set_scale(scale);
        Ok(RenderSurface {
            surface,
            canvas,
            ui,
            _ui: owned_ui,
            vk_surface,
            logical_size,
            scale,
            design: None,
        })
    }

    pub fn destroy_surface(&self, surface: RenderSurface) {
        self.device.wait_idle();
        let vk_surface = surface.vk_surface;
        drop(surface);
        self.ash.destroy_surface(vk_surface);
    }
}

/// Owns a raw Optics UI context created with the host clipboard descriptor.
/// [`Ui::borrow_raw`] does not release a borrowed context, so this guard is
/// the single owner and drops after `ui` in declaration order.
struct OwnedUi(*mut optics_ui::sys::optics_ui);

impl Drop for OwnedUi {
    fn drop(&mut self) {
        // SAFETY: `optics_ui_create` returned this context and this guard has not
        // released it; the borrowed `Ui` is dropped first.
        unsafe { optics_ui::sys::optics_ui_release(self.0) };
    }
}

pub struct RenderSurface {
    surface: optics_gfx::Surface,
    canvas: optics_gfx::Canvas,
    ui: Ui,
    _ui: OwnedUi,
    vk_surface: vk::SurfaceKHR,
    logical_size: (u32, u32),
    scale: f32,
    /// The design the optics-ui base theme was last set from.
    design: Option<Design>,
}

impl RenderSurface {
    pub fn logical_size(&self) -> (u32, u32) {
        self.logical_size
    }

    pub fn resize(&mut self, logical_size: (u32, u32), scale: f32) -> Result<(), RenderError> {
        let physical = physical_size(logical_size, scale);
        if self.surface.size() != physical {
            self.surface.resize(physical.0, physical.1)?;
        }
        self.logical_size = logical_size;
        self.scale = scale;
        self.ui.set_scale(scale);
        Ok(())
    }

    /// Deliver clipboard text to the focused optics-ui text widget. The payload is
    /// consumed by the next frame's build.
    pub fn paste(&mut self, text: &str) {
        self.ui.paste(text);
    }

    pub fn render(
        &mut self,
        host: &mut SettingsHost,
        i18n: &Localizer,
        input: &Input,
    ) -> Result<FrameOutcome, RenderError> {
        let design = *host.design();
        if self.design != Some(design) {
            self.ui.set_theme(themes::application_base(&design));
            self.design = Some(design);
        }
        self.ui.set_text_scale(host.text_scale());
        self.ui.set_reduced_motion(host.reduced_motion());

        let mut frame = self.surface.begin_frame()?;
        let background = design.colors.application_surface;
        let session = self.canvas.begin_session(
            &mut frame,
            optics_gfx::CanvasPassOptions {
                clear: Some(optics_gfx::rgba(
                    background.r(),
                    background.g(),
                    background.b(),
                    255,
                )),
                ..Default::default()
            },
        )?;
        let size = (self.logical_size.0 as f32, self.logical_size.1 as f32);
        self.ui.frame(input, |f| host.render(f, size, i18n));
        let snapshot = self.ui.snapshot()?;
        // SAFETY: the canvas session is live until `end` below.
        unsafe {
            snapshot.submit(session.as_raw().cast::<optics_ui::sys::optics_gfx_canvas>())?;
        }
        self.ui.activate(&snapshot)?;
        session.end()?;
        frame.submit()?.present()?;
        Ok(FrameOutcome {
            animating: self.ui.anim_pending() || self.ui.caret_flip_pending(),
            cursor: self.ui.cursor_hint(),
        })
    }
}

/// Buffer extent for a logical size at a (possibly fractional) scale.
fn physical_size(logical_size: (u32, u32), scale: f32) -> (u32, u32) {
    let scale = scale.max(0.01);
    (
        (logical_size.0.max(1) as f32 * scale).round().max(1.0) as u32,
        (logical_size.1.max(1) as f32 * scale).round().max(1.0) as u32,
    )
}

struct AshBridge {
    _entry: ash::Entry,
    _instance: ash::Instance,
    wayland: ash::khr::wayland_surface::Instance,
    surface: ash::khr::surface::Instance,
}

impl AshBridge {
    fn new(connection: &Connection, device: &optics_gfx::Device) -> Result<Self, RenderError> {
        // SAFETY: loading the system Vulkan loader has no preconditions.
        let entry = unsafe { ash::Entry::load() }.map_err(|_| RenderError::Vulkan)?;
        // SAFETY: gfx created this instance and keeps it alive with the device.
        let instance = unsafe {
            ash::Instance::load(
                entry.static_fn(),
                vk::Instance::from_raw(device.vk_instance() as usize as u64),
            )
        };
        let wayland = ash::khr::wayland_surface::Instance::new(&entry, &instance);
        let surface = ash::khr::surface::Instance::new(&entry, &instance);
        if connection.backend().display_ptr().is_null() {
            return Err(RenderError::Vulkan);
        }
        Ok(Self {
            _entry: entry,
            _instance: instance,
            wayland,
            surface,
        })
    }

    fn create_surface(
        &self,
        connection: &Connection,
        surface: &wl_surface::WlSurface,
    ) -> Result<vk::SurfaceKHR, RenderError> {
        let display = connection.backend().display_ptr();
        let proxy = surface.id().as_ptr();
        if display.is_null() || proxy.is_null() {
            return Err(RenderError::Vulkan);
        }
        let info = vk::WaylandSurfaceCreateInfoKHR::default()
            .display(display.cast())
            .surface(proxy.cast());
        // SAFETY: display and surface are live proxies of this connection.
        unsafe { self.wayland.create_wayland_surface(&info, None) }.map_err(|_| RenderError::Vulkan)
    }

    fn destroy_surface(&self, surface: vk::SurfaceKHR) {
        // SAFETY: callers destroy the gfx surface built on it first.
        unsafe {
            self.surface.destroy_surface(surface, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::physical_size;

    #[test]
    fn fractional_scales_round_to_whole_device_pixels() {
        assert_eq!(physical_size((960, 640), 1.25), (1200, 800));
        assert_eq!(physical_size((101, 33), 1.5), (152, 50));
        assert_eq!(physical_size((0, 0), 2.0), (2, 2));
    }
}
