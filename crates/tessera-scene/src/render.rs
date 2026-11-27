//! Cameras, lights and built-in scene materials (ADR-0262).
//!
//! Carried from the retired `optics_gfx::scene` module. Passes are the
//! public [`optics_gfx::RenderPass`] (`Frame::begin_render_pass` /
//! `begin_image_render_pass`); samplers are [`optics_gfx::Sampler`].

use optics_gfx::{Device, Error, Format, Image, Sampler};

use crate::sys;

fn vec2(v: [f32; 2]) -> sys::optics_gfx_vec2 {
    sys::optics_gfx_vec2 { x: v[0], y: v[1] }
}

fn vec3(v: [f32; 3]) -> sys::optics_gfx_vec3 {
    sys::optics_gfx_vec3 {
        x: v[0],
        y: v[1],
        z: v[2],
    }
}

fn vec4(v: [f32; 4]) -> sys::optics_gfx_vec4 {
    sys::optics_gfx_vec4 {
        x: v[0],
        y: v[1],
        z: v[2],
        w: v[3],
    }
}

/// Perspective camera.
pub struct Camera {
    raw: sys::tessera_scene_camera,
}

impl Camera {
    /// Perspective projection (right-handed, depth 0..1 — Vulkan clip).
    pub fn perspective(fov_y_rad: f32, aspect: f32, z_near: f32, z_far: f32) -> Camera {
        let mut raw = sys::tessera_scene_camera::default();
        let opts = sys::tessera_scene_camera_perspective_opts {
            fov_y_rad,
            aspect,
            z_near,
            z_far,
        };
        // SAFETY: both pointers are valid for the duration of the call.
        unsafe { sys::tessera_scene_camera_perspective(&mut raw, &opts) };
        Camera { raw }
    }

    /// Point the camera from `eye` at `center` with the given `up` vector.
    pub fn look_at(&mut self, eye: [f32; 3], center: [f32; 3], up: [f32; 3]) {
        // SAFETY: self.raw is a valid camera value.
        unsafe {
            sys::tessera_scene_camera_look_at(&mut self.raw, vec3(eye), vec3(center), vec3(up))
        };
    }

    pub fn as_raw(&self) -> *const sys::tessera_scene_camera {
        &self.raw
    }
}

/// One world-space directional light for Phong materials.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLight {
    /// Direction the light travels (normalised by the renderer).
    pub direction: [f32; 3],
    /// Linear RGB intensity.
    pub color: [f32; 3],
    /// Ambient term: `base_color * ambient * color`.
    pub ambient: f32,
}

impl Default for SceneLight {
    fn default() -> Self {
        Self {
            direction: [-0.4, -0.8, -0.45],
            color: [1.0, 1.0, 1.0],
            ambient: 0.08,
        }
    }
}

impl SceneLight {
    pub fn as_raw(&self) -> sys::tessera_scene_light {
        sys::tessera_scene_light {
            direction: vec3(self.direction),
            color: vec3(self.color),
            ambient: self.ambient,
        }
    }
}

/// Built-in material pipeline kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialKind {
    Unlit,
    Phong,
}

/// Alpha interpretation and blending for a scene material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaterialAlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

/// Base-colour texture and UV transform borrowed during material creation.
#[derive(Clone, Copy)]
pub struct MaterialTexture<'a> {
    pub image: &'a Image,
    pub sampler: Option<&'a Sampler>,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub uv_rotation: f32,
}

/// Optional surface state layered onto a built-in material.
#[derive(Clone, Copy)]
pub struct MaterialOptions<'a> {
    pub base_color_texture: Option<MaterialTexture<'a>>,
    pub alpha_mode: MaterialAlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl Default for MaterialOptions<'_> {
    fn default() -> Self {
        Self {
            base_color_texture: None,
            alpha_mode: MaterialAlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

/// Parameters for a built-in scene material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialDesc {
    pub kind: MaterialKind,
    pub base_color: [f32; 4],
    pub color_format: Format,
    pub depth_format: Format,
    pub shininess: f32,
    pub specular: f32,
}

/// Refcounted built-in scene material.
pub struct Material {
    raw: *mut sys::tessera_scene_material,
}

impl Material {
    pub fn new(device: &Device, desc: MaterialDesc) -> Result<Material, Error> {
        Self::new_impl(device, desc, None)
    }

    pub fn new_with_options(
        device: &Device,
        desc: MaterialDesc,
        options: MaterialOptions<'_>,
    ) -> Result<Material, Error> {
        Self::new_impl(device, desc, Some(options))
    }

    fn new_impl(
        device: &Device,
        desc: MaterialDesc,
        options: Option<MaterialOptions<'_>>,
    ) -> Result<Material, Error> {
        let kind = match desc.kind {
            MaterialKind::Unlit => sys::tessera_scene_material_kind::TESSERA_SCENE_MATERIAL_UNLIT,
            MaterialKind::Phong => sys::tessera_scene_material_kind::TESSERA_SCENE_MATERIAL_PHONG,
        };
        let raw_surface = options.map(|options| {
            let texture = options.base_color_texture;
            let alpha_mode = match options.alpha_mode {
                MaterialAlphaMode::Opaque => {
                    sys::tessera_scene_material_alpha_mode::TESSERA_SCENE_MATERIAL_ALPHA_OPAQUE
                }
                MaterialAlphaMode::Mask => {
                    sys::tessera_scene_material_alpha_mode::TESSERA_SCENE_MATERIAL_ALPHA_MASK
                }
                MaterialAlphaMode::Blend => {
                    sys::tessera_scene_material_alpha_mode::TESSERA_SCENE_MATERIAL_ALPHA_BLEND
                }
            };
            sys::tessera_scene_material_surface_desc {
                type_: sys::tessera_scene_struct_type::TESSERA_SCENE_TYPE_MATERIAL_SURFACE_DESC,
                next: std::ptr::null(),
                base_color_image: texture
                    .map_or(std::ptr::null_mut(), |value| value.image.as_raw()),
                base_color_sampler: texture
                    .and_then(|value| value.sampler)
                    .map_or(std::ptr::null_mut(), |sampler| sampler.as_raw().cast()),
                uv_offset: vec2(texture.map_or([0.0, 0.0], |value| value.uv_offset)),
                uv_scale: vec2(texture.map_or([1.0, 1.0], |value| value.uv_scale)),
                uv_rotation: texture.map_or(0.0, |value| value.uv_rotation),
                alpha_mode,
                alpha_cutoff: options.alpha_cutoff,
                double_sided: options.double_sided,
            }
        });
        let raw_desc = sys::tessera_scene_material_desc {
            type_: sys::tessera_scene_struct_type::TESSERA_SCENE_TYPE_MATERIAL_DESC,
            next: raw_surface.as_ref().map_or(std::ptr::null(), |value| {
                (value as *const sys::tessera_scene_material_surface_desc).cast()
            }),
            kind,
            base_color: vec4(desc.base_color),
            color_format: desc.color_format.as_raw(),
            depth_format: desc.depth_format.as_raw(),
            shininess: desc.shininess,
            specular: desc.specular,
        };
        let mut raw = std::ptr::null_mut();
        // SAFETY: raw_desc and its chained surface outlive the call; the
        // device pointer is live for the borrow of `device`.
        Error::check_raw(unsafe {
            sys::tessera_scene_material_create(device.as_raw(), &raw_desc, &mut raw)
        })?;
        Ok(Material { raw })
    }

    pub fn as_raw(&self) -> *mut sys::tessera_scene_material {
        self.raw
    }
}

impl Drop for Material {
    fn drop(&mut self) {
        // SAFETY: self.raw holds one owned reference; GPU objects retire
        // through the gfx retire queue after in-flight use completes.
        unsafe { sys::tessera_scene_material_release(self.raw) };
    }
}

// SAFETY: a material is immutable after creation and refcounted atomically.
unsafe impl Send for Material {}
unsafe impl Sync for Material {}
