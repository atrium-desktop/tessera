//! Flux/Lens rendering for the Settings window's single surface.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::ptr;

use ash::vk::{self, Handle};
use lens::{CursorHint, Input, Ui};
use tessera_design::{Design, themes};
use tessera_i18n::Localizer;
use tessera_settings::SettingsHost;
use thiserror::Error;
use wayland_client::{Connection, Proxy, protocol::wl_surface};

use super::clipboard;

const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];
const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("Vulkan is unavailable")]
    Vulkan,
    #[error(transparent)]
    Flux(#[from] flux::Error),
    #[error(transparent)]
    Lens(#[from] lens::Error),
}

/// What one rendered frame asks of the event loop.
pub struct FrameOutcome {
    /// An eased widget value is still moving; draw again next frame.
    pub animating: bool,
    pub cursor: CursorHint,
}

pub struct Graphics {
    device: flux::Device,
    ash: AshBridge,
}

impl Graphics {
    pub fn new(connection: &Connection) -> Result<Self, RenderError> {
        let device = flux::Device::new(true, &INSTANCE_EXTENSIONS, &DEVICE_EXTENSIONS, 2)?;
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
        // until `destroy_surface`, which drops the flux surface first.
        let surface = match unsafe {
            flux::Surface::from_vk(
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
        let canvas = flux::Canvas::new(&surface)?;
        // SAFETY: the device outlives the Ui; both are owned by `Graphics`'
        // caller in drop order (surface before graphics). The clipboard
        // pointer is the boxed `clipboard::Host` owned by `App`; it outlives
        // this surface and is stable across resizes.
        let mut raw = ptr::null_mut();
        let desc = lens::sys::lens_desc {
            size: std::mem::size_of::<lens::sys::lens_desc>() as u32,
            device: self.device.as_raw().cast::<lens::sys::flux_device>(),
            theme: unsafe { lens::sys::lens_theme_dark() },
            clipboard: lens::sys::lens_clipboard {
                request_text: Some(clipboard::request_text),
                set_text: Some(clipboard::set_text),
                user: clipboard_user,
            },
            ..unsafe { std::mem::zeroed() }
        };
        let result = unsafe { lens::sys::lens_create(&desc, &mut raw) };
        if result != lens::sys::flux_result::FLUX_OK || raw.is_null() {
            return Err(RenderError::Lens(lens::Error::Create(
                lens::FluxDiagnostic::for_test(result),
            )));
        }
        let owned_lens = OwnedLens(raw);
        let mut ui = unsafe { Ui::borrow_raw(raw) };
        ui.set_scale(scale);
        Ok(RenderSurface {
            surface,
            canvas,
            ui,
            _lens: owned_lens,
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

/// Owns a raw Lens context created with the host clipboard descriptor.
/// [`Ui::borrow_raw`] does not release a borrowed context, so this guard is
/// the single owner and drops after `ui` in declaration order.
struct OwnedLens(*mut lens::sys::lens);

impl Drop for OwnedLens {
    fn drop(&mut self) {
        // SAFETY: `lens_create` returned this context and this guard has not
        // released it; the borrowed `Ui` is dropped first.
        unsafe { lens::sys::lens_release(self.0) };
    }
}

pub struct RenderSurface {
    surface: flux::Surface,
    canvas: flux::Canvas,
    ui: Ui,
    _lens: OwnedLens,
    vk_surface: vk::SurfaceKHR,
    logical_size: (u32, u32),
    scale: f32,
    /// The design the Lens base theme was last set from.
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

    /// Deliver clipboard text to the focused Lens text widget. The payload is
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
            flux::CanvasPassOptions {
                clear: Some(flux::rgba(
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
            snapshot.submit(session.as_raw().cast::<lens::sys::flux_canvas>())?;
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
    fn new(connection: &Connection, device: &flux::Device) -> Result<Self, RenderError> {
        // SAFETY: loading the system Vulkan loader has no preconditions.
        let entry = unsafe { ash::Entry::load() }.map_err(|_| RenderError::Vulkan)?;
        // SAFETY: flux created this instance and keeps it alive with the device.
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
        // SAFETY: callers destroy the flux surface built on it first.
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
