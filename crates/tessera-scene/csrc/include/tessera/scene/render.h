/*
 * tessera/scene/render.h — 3D scene primitives (Tessera ADR-0262).
 *
 * Scope: camera, mesh, material, depth-tested renderer. Carried from the
 * Optics gfx scene core that Optics ADR-0174 retired, and rebuilt on the
 * Optics gfx public RHI: meshes are optics_gfx_buffer objects and materials
 * own optics_gfx_graphics_pipeline objects, so their destruction is deferred
 * by the gfx retire queue past every in-flight frame that used them.
 */

#ifndef TESSERA_SCENE_RENDER_H
#define TESSERA_SCENE_RENDER_H

#include <optics/gfx/core.h>
#include <optics/gfx/math.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(__GNUC__) || defined(__clang__)
#define TESSERA_SCENE_API __attribute__((visibility("default")))
#else
#define TESSERA_SCENE_API
#endif

/* Descriptor tags for this library's extension chains. Local to Tessera:
 * values are not shared with optics_gfx_struct_type. Append only. */
typedef enum tessera_scene_struct_type : uint32_t {
    TESSERA_SCENE_TYPE_UNKNOWN = 0,
    TESSERA_SCENE_TYPE_MESH_DESC = 1,
    TESSERA_SCENE_TYPE_MESH_SKIN_DESC = 2,
    TESSERA_SCENE_TYPE_MATERIAL_DESC = 3,
    TESSERA_SCENE_TYPE_MATERIAL_SURFACE_DESC = 4,
} tessera_scene_struct_type;

/* ------------------------------------------------------------------ */
/*  Camera (value type)                                               */
/* ------------------------------------------------------------------ */

typedef struct tessera_scene_camera {
    optics_gfx_mat4 view;
    optics_gfx_mat4 projection;
} tessera_scene_camera;

typedef struct tessera_scene_camera_perspective_opts {
    float fov_y_rad; /* vertical field of view, radians */
    float aspect;    /* width / height                  */
    float z_near;
    float z_far;
} tessera_scene_camera_perspective_opts;

TESSERA_SCENE_API void tessera_scene_camera_perspective(tessera_scene_camera *cam,
                                                  const tessera_scene_camera_perspective_opts *opts);

TESSERA_SCENE_API void tessera_scene_camera_look_at(tessera_scene_camera *cam, optics_gfx_vec3 eye,
                                              optics_gfx_vec3 center, optics_gfx_vec3 up);

/* ------------------------------------------------------------------ */
/*  Mesh (refcounted GPU buffers)                                     */
/* ------------------------------------------------------------------ */

typedef struct tessera_scene_mesh tessera_scene_mesh;

typedef struct tessera_scene_vertex {
    float position[3];
    float normal[3];
    float uv[2];
} tessera_scene_vertex;

typedef struct tessera_scene_mesh_desc {
    tessera_scene_struct_type type; /* TESSERA_SCENE_TYPE_MESH_DESC */
    const void *next;
    const tessera_scene_vertex *vertices;
    uint32_t vertex_count;
    const uint32_t *indices; /* may be NULL for non-indexed */
    uint32_t index_count;
} tessera_scene_mesh_desc;

#define TESSERA_SCENE_MESH_DESC_INIT {.type = TESSERA_SCENE_TYPE_MESH_DESC}

/* Optional `tessera_scene_mesh_desc.next` payload for GPU skinning. Joint indices are
 * local to the palette supplied at draw time; weights are normalised by the
 * content loader, but the shader defensively handles a zero-sum vertex. */
typedef struct tessera_scene_skin_vertex {
    uint16_t joints[4];
    float weights[4];
} tessera_scene_skin_vertex;

typedef struct tessera_scene_mesh_skin_desc {
    tessera_scene_struct_type type; /* TESSERA_SCENE_TYPE_MESH_SKIN_DESC */
    const void *next;
    const tessera_scene_skin_vertex *vertices; /* exactly mesh_desc.vertex_count */
} tessera_scene_mesh_skin_desc;

#define TESSERA_SCENE_MESH_SKIN_DESC_INIT {.type = TESSERA_SCENE_TYPE_MESH_SKIN_DESC}

OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result tessera_scene_mesh_create(
    optics_gfx_device *d, const tessera_scene_mesh_desc *desc, tessera_scene_mesh **out);
OPTICS_GFX_NODISCARD TESSERA_SCENE_API tessera_scene_mesh *tessera_scene_mesh_retain(tessera_scene_mesh *m);
/* Destruction is deferred: the mesh's buffers are optics_gfx_buffer objects,
 * which the gfx retire queue destroys only after the GPU passed every batch
 * that could reference them. */
TESSERA_SCENE_API void tessera_scene_mesh_release(tessera_scene_mesh *m);

/* ------------------------------------------------------------------ */
/*  Material (built-in pipelines; bindless-aware)                     */
/* ------------------------------------------------------------------ */

typedef struct tessera_scene_material tessera_scene_material;
typedef struct optics_gfx_sampler optics_gfx_sampler;

typedef enum tessera_scene_material_kind : uint32_t {
    TESSERA_SCENE_MATERIAL_UNLIT = 0,
    /* Blinn-Phong with one directional light per draw. Light data is
     * supplied at draw time via tessera_scene_draw_mesh_opts.light; draws made
     * through tessera_scene_draw_mesh use TESSERA_SCENE_LIGHT_DEFAULT. */
    TESSERA_SCENE_MATERIAL_PHONG = 1,
    /* 2 was the analytic vector-coverage material; Tessera does not carry
     * it. Reserved. */
    /* PBR will be added at the end when it ships. Append-only —
     * never repurpose existing values. */
} tessera_scene_material_kind;

typedef enum tessera_scene_material_alpha_mode : uint32_t {
    TESSERA_SCENE_MATERIAL_ALPHA_OPAQUE = 0,
    TESSERA_SCENE_MATERIAL_ALPHA_MASK = 1,
    TESSERA_SCENE_MATERIAL_ALPHA_BLEND = 2,
} tessera_scene_material_alpha_mode;

/* Optional `tessera_scene_material_desc.next` payload for a sampled base-colour
 * texture and glTF-compatible surface state. The material retains image and
 * sampler. A NULL sampler selects gfx's linear clamp-to-edge sampler.
 * UV transform order is scale, counter-clockwise rotation, then offset. */
typedef struct tessera_scene_material_surface_desc {
    tessera_scene_struct_type type; /* TESSERA_SCENE_TYPE_MATERIAL_SURFACE_DESC */
    const void *next;
    optics_gfx_image *base_color_image;     /* optional */
    optics_gfx_sampler *base_color_sampler; /* optional; ignored without image */
    optics_gfx_vec2 uv_offset;
    optics_gfx_vec2 uv_scale;
    float uv_rotation;
    tessera_scene_material_alpha_mode alpha_mode;
    float alpha_cutoff; /* MASK only; glTF default is 0.5 */
    bool double_sided;
} tessera_scene_material_surface_desc;

#define TESSERA_SCENE_MATERIAL_SURFACE_DESC_INIT                                                      \
    {.type = TESSERA_SCENE_TYPE_MATERIAL_SURFACE_DESC, .uv_scale = {1.0f, 1.0f}, .alpha_cutoff = 0.5f}

typedef struct tessera_scene_material_desc {
    tessera_scene_struct_type type; /* TESSERA_SCENE_TYPE_MATERIAL_DESC */
    const void *next;
    tessera_scene_material_kind kind;
    optics_gfx_vec4 base_color;

    /* Render-target formats the pipeline must be compatible with.
     * Translate via optics_gfx_format_from_vk(optics_gfx_surface_vk_format(...))
     * for the colour target; pass the depth format you'll attach in
     * optics_gfx_pass_desc.depth, or OPTICS_GFX_FORMAT_UNDEFINED for no depth. */
    optics_gfx_format color_format;
    optics_gfx_format depth_format;

    /* TESSERA_SCENE_MATERIAL_PHONG only; ignored by UNLIT.
     * shininess: Blinn-Phong specular exponent. <= 0 selects the
     *            library default (32).
     * specular:  specular strength in [0, 1]. 0 (the zero-init value)
     *            disables the highlight entirely. */
    float shininess;
    float specular;
} tessera_scene_material_desc;

#define TESSERA_SCENE_MATERIAL_DESC_INIT {.type = TESSERA_SCENE_TYPE_MATERIAL_DESC}

OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result tessera_scene_material_create(
    optics_gfx_device *d, const tessera_scene_material_desc *desc, tessera_scene_material **out);
OPTICS_GFX_NODISCARD TESSERA_SCENE_API tessera_scene_material *
tessera_scene_material_retain(tessera_scene_material *m);
/* Drop one owned reference. GPU objects retire after submitted use completes.
 * Keep resources alive through submission; raw command buffers follow the
 * same rule. No caller-side device-idle wait is required after submission. */
TESSERA_SCENE_API void tessera_scene_material_release(tessera_scene_material *m);

TESSERA_SCENE_API tessera_scene_material_alpha_mode
tessera_scene_material_get_alpha_mode(const tessera_scene_material *m);

/* ------------------------------------------------------------------ */
/*  Light (value type)                                                */
/* ------------------------------------------------------------------ */

/* One directional light, world space. `direction` is the direction
 * the light travels (sunlight vector, surface-bound); it is
 * normalised internally, and a zero vector falls back to the default
 * light's direction. `ambient` scales `color` for the unlit term. */
typedef struct tessera_scene_light {
    optics_gfx_vec3 direction;
    optics_gfx_vec3 color; /* linear RGB intensity */
    float ambient;         /* ambient = base_color * ambient * color */
} tessera_scene_light;

#define TESSERA_SCENE_LIGHT_DEFAULT                                                             \
    {.direction = {-0.4f, -0.8f, -0.45f}, .color = {1.0f, 1.0f, 1.0f}, .ambient = 0.08f}

/* ------------------------------------------------------------------ */
/*  Scene draw (records into the current pass)                        */
/* ------------------------------------------------------------------ */

/* Records a single mesh+material draw into the frame's currently
 * active pass. Must be called between optics_gfx_frame_begin_pass and
 * optics_gfx_frame_end_pass on the same frame, and the active pass's
 * attachments must match the material's color_format and depth_format
 * exactly. No-op if required arguments are NULL. PHONG materials use
 * `light` if non-NULL, or TESSERA_SCENE_LIGHT_DEFAULT when NULL; UNLIT
 * materials ignore it. When `joint_matrices` is non-NULL and
 * `joint_count > 0` on a skinned mesh, GPU skinning is enabled. */
typedef struct tessera_scene_draw_mesh_opts {
    const tessera_scene_camera *camera;
    optics_gfx_mat4 world;
    tessera_scene_mesh *mesh;
    tessera_scene_material *material;
    const tessera_scene_light *light;
    const optics_gfx_mat4 *joint_matrices;
    uint32_t joint_count;
} tessera_scene_draw_mesh_opts;

TESSERA_SCENE_API void tessera_scene_draw_mesh(optics_gfx_frame *f,
                                               const tessera_scene_draw_mesh_opts *opts);

#ifdef __cplusplus
}
#endif

#endif /* TESSERA_SCENE_RENDER_H */
