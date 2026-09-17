//! Display-engine resource ownership and lifecycle.
//!
//! A session borrows the engine's resources for its event loop. It cannot move
//! them out or outlive the engine. The engine controls teardown order, including
//! the nested host's Vulkan surface and the device that created it.

use tessera_platform::Backend;
use tessera_platform::host::Host;
use tessera_render::Renderer;
use tessera_wayland::Server;

pub struct Engine {
    // Declaration order is teardown order. The canvas and WSI surface must
    // disappear before the host destroys its wl_display/VkSurfaceKHR; the
    // device and VkInstance remain alive until every dependent resource drops.
    canvas: flux::Canvas,
    surface: flux::Surface,
    renderer: Renderer,
    server: Server,
    host: Host,
    device: flux::Device,
}

/// Exclusive, disjoint resource borrows for the session's frame loop.
/// These are borrowed capabilities, not ownership transfers or re-exports.
pub struct EngineSession<'a> {
    pub canvas: &'a mut flux::Canvas,
    pub surface: &'a mut flux::Surface,
    pub renderer: &'a mut Renderer,
    pub server: &'a mut Server,
    pub host: &'a mut Host,
    pub device: &'a mut flux::Device,
}

impl Engine {
    /// Initialize the rendering target and Wayland server against one host.
    /// Product configuration, process environment, and UI are supplied by the
    /// session after construction; the engine never resolves them itself.
    pub fn new(host: Host) -> Result<Self, Box<dyn std::error::Error>> {
        let device = host.create_device()?;
        // This binding must follow `device`: failure paths also drop the host
        // before the instance backing it.
        let mut host = host;
        let surface = host.create_surface(&device)?;
        if let Err(error) = surface.prepare_readback() {
            log::warn!(
                "capture: could not preallocate readback staging: {error}{}",
                tessera_render::presentation::flux_last_error_detail()
            );
        }
        let canvas = flux::Canvas::new(&surface)?;
        host.set_buffer_scale();
        let main_device = host.dmabuf_feedback_device(&device);
        if host.name() == "drm" && main_device.is_none() {
            log::warn!("drm: linux-dmabuf v4 feedback disabled: main DRM device is unknown");
        }
        let server = Server::new_with_dmabuf_feedback(
            flux::dmabuf_supported(&device),
            flux::dmabuf_sync_supported(&device),
            tessera_render::formats_with_modifiers(&device),
            main_device,
            host.dmabuf_scanout_formats(),
            host.dmabuf_scanout_device(),
        )?;
        Ok(Self {
            canvas,
            surface,
            renderer: Renderer::new(),
            server,
            host,
            device,
        })
    }

    pub fn session(&mut self) -> EngineSession<'_> {
        EngineSession {
            canvas: &mut self.canvas,
            surface: &mut self.surface,
            renderer: &mut self.renderer,
            server: &mut self.server,
            host: &mut self.host,
            device: &mut self.device,
        }
    }
}
