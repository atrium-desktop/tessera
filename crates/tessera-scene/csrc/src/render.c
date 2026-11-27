/*
 * render.c — camera, mesh, material and the depth-tested mesh draw
 * (Tessera ADR-0262).
 *
 * Carried from the Optics gfx scene core (retired by Optics ADR-0174) and
 * rebuilt on the Optics gfx public RHI only:
 *
 *   - Meshes are optics_gfx_buffer objects (GPU-local, uploaded at create).
 *   - Materials own two optics_gfx_graphics_pipeline objects (static and
 *     skinned) bound through optics_gfx_graphics_pipeline_bind.
 *   - Per-draw parameters live in the frame transient ring and reach the
 *     shaders by buffer device address, as before.
 *
 * Every GPU object here is therefore destroyed by the gfx retire queue only
 * after the GPU passed each batch that could reference it, so releasing a
 * mesh or material is safe while frames that drew it are still in flight.
 *
 * Skinning reads joints and weights by device address from the mesh's skin
 * buffer (indexed by gl_VertexIndex) rather than from a second vertex
 * binding, because the public pipeline descriptor carries one binding.
 *
 * The depth attachment is owned by the caller (peer-defined attachments,
 * Optics ADR-0001): tessera_scene_draw_mesh assumes the frame's current pass
 * has a depth attachment matching the material's depth_format.
 */
#include <optics/gfx/vulkan.h>
#include <tessera/scene/render.h>

#include <math.h>
#include <stdalign.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

/* Shader bytecode embedded via native C23 #embed (precompiled under spv/). */
alignas(uint32_t) static const unsigned char scene_unlit_vert_spv[] = {
#embed "scene_unlit.vert.spv"
};
alignas(uint32_t) static const unsigned char scene_unlit_frag_spv[] = {
#embed "scene_unlit.frag.spv"
};
alignas(uint32_t) static const unsigned char scene_phong_vert_spv[] = {
#embed "scene_phong.vert.spv"
};
alignas(uint32_t) static const unsigned char scene_phong_frag_spv[] = {
#embed "scene_phong.frag.spv"
};
alignas(uint32_t) static const unsigned char scene_unlit_skin_vert_spv[] = {
#embed "scene_unlit_skin.vert.spv"
};
alignas(uint32_t) static const unsigned char scene_phong_skin_vert_spv[] = {
#embed "scene_phong_skin.vert.spv"
};

/* ================================================================== */
/*  Camera                                                            */
/* ================================================================== */

void tessera_scene_camera_perspective(tessera_scene_camera *cam,
                                      const tessera_scene_camera_perspective_opts *opts) {
    if (!cam || !opts)
        return;
    cam->view = optics_gfx_mat4_identity();
    cam->projection =
        optics_gfx_mat4_perspective(opts->fov_y_rad, opts->aspect, opts->z_near, opts->z_far);
}

void tessera_scene_camera_look_at(tessera_scene_camera *cam, optics_gfx_vec3 eye,
                                  optics_gfx_vec3 center, optics_gfx_vec3 up) {
    if (!cam)
        return;
    cam->view = optics_gfx_mat4_look_at(eye, center, up);
}

/* ================================================================== */
/*  Mesh                                                              */
/* ================================================================== */

static_assert(sizeof(tessera_scene_skin_vertex) == 24,
              "scene_skin_matrix reads 6 words per skin vertex");

struct tessera_scene_mesh {
    atomic_uint ref_count;
    optics_gfx_buffer *vertices;
    uint32_t vertex_count;
    optics_gfx_buffer *indices; /* nullptr if non-indexed */
    uint32_t index_count;
    optics_gfx_buffer *skin; /* nullptr for a static mesh; read by device address */
};

static void mesh_free(tessera_scene_mesh *m) {
    optics_gfx_buffer_release(m->skin);
    optics_gfx_buffer_release(m->indices);
    optics_gfx_buffer_release(m->vertices);
    free(m);
}

optics_gfx_result tessera_scene_mesh_create(optics_gfx_device *d, const tessera_scene_mesh_desc *desc,
                                            tessera_scene_mesh **out) {
    if (!d || !desc || !out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *out = nullptr;
    if (desc->type != TESSERA_SCENE_TYPE_MESH_DESC || !desc->vertices || desc->vertex_count == 0)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    const tessera_scene_mesh_skin_desc *skin = desc->next;
    if (skin && (skin->type != TESSERA_SCENE_TYPE_MESH_SKIN_DESC || skin->next || !skin->vertices))
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    static_assert(SIZE_MAX / sizeof(optics_gfx_mat4) >= UINT32_MAX,
                  "u32 element counts must not overflow size_t byte counts");

    tessera_scene_mesh *m = calloc(1, sizeof(*m));
    if (!m)
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    atomic_init(&m->ref_count, 1u);
    m->vertex_count = desc->vertex_count;
    m->index_count = desc->indices ? desc->index_count : 0;

    optics_gfx_buffer_desc vb = OPTICS_GFX_BUFFER_DESC_INIT;
    vb.size = (size_t)desc->vertex_count * sizeof(tessera_scene_vertex);
    vb.usage = OPTICS_GFX_BUFFER_USAGE_VERTEX;
    vb.location = OPTICS_GFX_BUFFER_GPU_LOCAL;
    vb.initial_data = desc->vertices;
    optics_gfx_result r = optics_gfx_buffer_create(d, &vb, &m->vertices);
    if (r == OPTICS_GFX_OK && m->index_count > 0) {
        optics_gfx_buffer_desc ib = OPTICS_GFX_BUFFER_DESC_INIT;
        ib.size = (size_t)m->index_count * sizeof(uint32_t);
        ib.usage = OPTICS_GFX_BUFFER_USAGE_INDEX;
        ib.location = OPTICS_GFX_BUFFER_GPU_LOCAL;
        ib.initial_data = desc->indices;
        r = optics_gfx_buffer_create(d, &ib, &m->indices);
    }
    if (r == OPTICS_GFX_OK && skin) {
        optics_gfx_buffer_desc sb = OPTICS_GFX_BUFFER_DESC_INIT;
        sb.size = (size_t)desc->vertex_count * sizeof(tessera_scene_skin_vertex);
        sb.usage = OPTICS_GFX_BUFFER_USAGE_STORAGE;
        sb.location = OPTICS_GFX_BUFFER_GPU_LOCAL;
        sb.device_address = true;
        sb.initial_data = skin->vertices;
        r = optics_gfx_buffer_create(d, &sb, &m->skin);
    }
    if (r != OPTICS_GFX_OK) {
        mesh_free(m);
        return r;
    }
    *out = m;
    return OPTICS_GFX_OK;
}

tessera_scene_mesh *tessera_scene_mesh_retain(tessera_scene_mesh *m) {
    if (m)
        atomic_fetch_add_explicit(&m->ref_count, 1u, memory_order_relaxed);
    return m;
}

void tessera_scene_mesh_release(tessera_scene_mesh *m) {
    if (!m)
        return;
    if (atomic_fetch_sub_explicit(&m->ref_count, 1u, memory_order_acq_rel) != 1u)
        return;
    mesh_free(m);
}

/* ================================================================== */
/*  Material                                                          */
/* ================================================================== */

/* Default Blinn-Phong specular exponent when desc.shininess <= 0. */
#define SCENE_PHONG_DEFAULT_SHININESS 32.0f

struct tessera_scene_material {
    atomic_uint ref_count;
    optics_gfx_device *device;
    tessera_scene_material_kind kind;
    optics_gfx_vec4 base_color;
    optics_gfx_image *base_color_image;
    optics_gfx_sampler *base_color_sampler;
    optics_gfx_vec2 uv_offset;
    optics_gfx_vec2 uv_scale;
    float uv_rotation;
    float alpha_cutoff;
    tessera_scene_material_alpha_mode alpha_mode;
    bool double_sided;
    optics_gfx_bindless_handle image_handle;
    optics_gfx_bindless_handle sampler_handle;
    float shininess; /* PHONG only */
    float specular;  /* PHONG only */
    /* The material renders into the linear working space (its declared
     * color_format is 16F), so 8-bit base-color texels are decoded from
     * sRGB at the edge (ADR-0069). 8-bit targets keep the raw path. */
    bool renders_working_space;
    optics_gfx_graphics_pipeline *pipeline;
    optics_gfx_graphics_pipeline *skinned_pipeline;
};

/* All material variants keep the push block small and place per-draw state
 * in the frame transient ring. */
typedef struct scene_params_push {
    float mvp[16];
    uint64_t params_address;
} scene_params_push;

static_assert(sizeof(scene_params_push) <= 128, "scene_params_push exceeds the push budget");

/* Shared surface block, std430. Mirrors the leading fields of UnlitParams
 * and the material fields of PhongParams in shaders/scene_color.slang. */
typedef struct scene_surface_params {
    float base_color[4];
    float uv_scale_offset[4];          /* scale.xy, offset.xy */
    float uv_rotation_alpha_cutoff[4]; /* cos, sin, cutoff, unused */
    uint32_t texture_info[4];          /* image, sampler, alpha mode, textured */
    uint32_t color_flags;              /* SCENE_COLOR_* */
    uint32_t _color_pad[3];
} scene_surface_params;

/* scene_surface_params.color_flags: DECODE converts sampled 8-bit texels
 * from sRGB into the linear working space (16F target). */
#define SCENE_COLOR_DECODE 0x1u

/* The skinning tail both parameter blocks end with. */
typedef struct scene_skin_params {
    uint64_t joint_palette_address;
    uint64_t skin_address;
    uint32_t joint_count;
    uint32_t _pad[3];
} scene_skin_params;

typedef struct scene_unlit_params {
    scene_surface_params surface;
    scene_skin_params skin;
} scene_unlit_params;

/* Per-draw lighting block, std430. Field-for-field mirror of PhongParams. */
typedef struct scene_phong_params {
    float world[16];
    float nrm0[4]; /* normal matrix columns:            */
    float nrm1[4]; /*   transpose(inverse(mat3(world))) */
    float nrm2[4];
    scene_surface_params surface;
    float light_dir_shininess[4]; /* xyz = travel direction, w = exponent */
    float light_color_ambient[4]; /* rgb = light colour, w = ambient      */
    float eye_specular[4];        /* xyz = world eye pos, w = strength    */
    scene_skin_params skin;
} scene_phong_params;

static_assert(offsetof(scene_unlit_params, skin) == 80, "UnlitParams layout drifted");
static_assert(sizeof(scene_unlit_params) == 112, "UnlitParams layout drifted");
static_assert(offsetof(scene_phong_params, skin) == 240, "PhongParams layout drifted");
static_assert(sizeof(scene_phong_params) == 272, "PhongParams layout drifted");

static optics_gfx_result create_material_pipeline(tessera_scene_material *mat,
                                                  const tessera_scene_material_desc *desc,
                                                  bool skinned,
                                                  optics_gfx_graphics_pipeline **out) {
    static const optics_gfx_vertex_attribute attributes[3] = {
        {.location = 0,
         .format = OPTICS_GFX_FORMAT_RGB32_SFLOAT,
         .offset = offsetof(tessera_scene_vertex, position)},
        {.location = 1,
         .format = OPTICS_GFX_FORMAT_RGB32_SFLOAT,
         .offset = offsetof(tessera_scene_vertex, normal)},
        {.location = 2,
         .format = OPTICS_GFX_FORMAT_RG32_SFLOAT,
         .offset = offsetof(tessera_scene_vertex, uv)},
    };
    static const optics_gfx_vertex_binding binding = {
        .stride = sizeof(tessera_scene_vertex),
        .attribute_count = 3,
        .attributes = attributes,
    };

    const unsigned char *vs;
    size_t vs_len;
    const unsigned char *fs;
    size_t fs_len;
    if (mat->kind == TESSERA_SCENE_MATERIAL_PHONG) {
        vs = skinned ? scene_phong_skin_vert_spv : scene_phong_vert_spv;
        vs_len = skinned ? sizeof(scene_phong_skin_vert_spv) : sizeof(scene_phong_vert_spv);
        fs = scene_phong_frag_spv;
        fs_len = sizeof(scene_phong_frag_spv);
    } else {
        vs = skinned ? scene_unlit_skin_vert_spv : scene_unlit_vert_spv;
        vs_len = skinned ? sizeof(scene_unlit_skin_vert_spv) : sizeof(scene_unlit_vert_spv);
        fs = scene_unlit_frag_spv;
        fs_len = sizeof(scene_unlit_frag_spv);
    }

    const bool blend = mat->alpha_mode == TESSERA_SCENE_MATERIAL_ALPHA_BLEND;
    optics_gfx_graphics_pipeline_desc pd = OPTICS_GFX_GRAPHICS_PIPELINE_DESC_INIT;
    pd.vertex_spirv = (const uint32_t *)vs;
    pd.vertex_spirv_word_count = vs_len / sizeof(uint32_t);
    pd.fragment_spirv = (const uint32_t *)fs;
    pd.fragment_spirv_word_count = fs_len / sizeof(uint32_t);
    pd.vertex_binding = &binding;
    pd.topology = OPTICS_GFX_TOPOLOGY_TRIANGLE_LIST;
    pd.cull = mat->double_sided ? OPTICS_GFX_CULL_NONE : OPTICS_GFX_CULL_BACK;
    /* Blended surfaces are premultiplied by the fragment shaders. */
    pd.blend = blend ? OPTICS_GFX_BLEND_PRESET_PREMUL : OPTICS_GFX_BLEND_PRESET_NONE;
    pd.depth = desc->depth_format == OPTICS_GFX_FORMAT_UNDEFINED ? OPTICS_GFX_DEPTH_NONE
               : blend                                          ? OPTICS_GFX_DEPTH_TEST_READONLY
                                                                : OPTICS_GFX_DEPTH_TEST_AND_WRITE;
    pd.color_format = desc->color_format;
    pd.depth_format = desc->depth_format;
    pd.push_constant_bytes = sizeof(scene_params_push);
    return optics_gfx_graphics_pipeline_create(mat->device, &pd, out);
}

static void material_free(tessera_scene_material *m) {
    optics_gfx_graphics_pipeline_release(m->skinned_pipeline);
    optics_gfx_graphics_pipeline_release(m->pipeline);
    optics_gfx_sampler_release(m->base_color_sampler);
    optics_gfx_image_release(m->base_color_image);
    optics_gfx_device_release(m->device);
    free(m);
}

optics_gfx_result tessera_scene_material_create(optics_gfx_device *d,
                                                const tessera_scene_material_desc *desc,
                                                tessera_scene_material **out) {
    if (!d || !desc || !out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *out = nullptr;
    if (desc->type != TESSERA_SCENE_TYPE_MATERIAL_DESC)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (desc->kind != TESSERA_SCENE_MATERIAL_UNLIT && desc->kind != TESSERA_SCENE_MATERIAL_PHONG)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    const tessera_scene_material_surface_desc *surface = desc->next;
    if (surface && (surface->type != TESSERA_SCENE_TYPE_MATERIAL_SURFACE_DESC || surface->next ||
                    (surface->alpha_mode != TESSERA_SCENE_MATERIAL_ALPHA_OPAQUE &&
                     surface->alpha_mode != TESSERA_SCENE_MATERIAL_ALPHA_MASK &&
                     surface->alpha_mode != TESSERA_SCENE_MATERIAL_ALPHA_BLEND)))
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (desc->color_format == OPTICS_GFX_FORMAT_UNDEFINED)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;

    tessera_scene_material *m = calloc(1, sizeof(*m));
    if (!m)
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    atomic_init(&m->ref_count, 1u);
    m->device = optics_gfx_device_retain(d);
    m->kind = desc->kind;
    m->base_color = desc->base_color;
    m->uv_scale = optics_gfx_vec2_make(1.0f, 1.0f);
    m->alpha_cutoff = 0.5f;
    m->alpha_mode = TESSERA_SCENE_MATERIAL_ALPHA_OPAQUE;
    m->image_handle = OPTICS_GFX_BINDLESS_INVALID;
    m->sampler_handle = OPTICS_GFX_BINDLESS_INVALID;
    if (surface) {
        m->base_color_image = optics_gfx_image_retain(surface->base_color_image);
        m->base_color_sampler =
            m->base_color_image ? optics_gfx_sampler_retain(surface->base_color_sampler) : nullptr;
        m->uv_offset = surface->uv_offset;
        m->uv_scale = surface->uv_scale;
        m->uv_rotation = surface->uv_rotation;
        m->alpha_cutoff = surface->alpha_cutoff;
        m->alpha_mode = surface->alpha_mode;
        m->double_sided = surface->double_sided;
        if (m->base_color_image) {
            m->image_handle = optics_gfx_image_bindless_handle(m->base_color_image);
            m->sampler_handle = m->base_color_sampler
                                    ? optics_gfx_sampler_bindless_handle(m->base_color_sampler)
                                    : optics_gfx_device_default_sampler_handle(d);
            if (m->image_handle == OPTICS_GFX_BINDLESS_INVALID ||
                m->sampler_handle == OPTICS_GFX_BINDLESS_INVALID) {
                material_free(m);
                return OPTICS_GFX_ERROR_BACKEND_FAILURE;
            }
        }
    }
    m->shininess = desc->shininess > 0.0f ? desc->shininess : SCENE_PHONG_DEFAULT_SHININESS;
    m->specular = desc->specular;
    m->renders_working_space = desc->color_format == OPTICS_GFX_FORMAT_RGBA16_SFLOAT;

    optics_gfx_result r = create_material_pipeline(m, desc, false, &m->pipeline);
    if (r == OPTICS_GFX_OK)
        r = create_material_pipeline(m, desc, true, &m->skinned_pipeline);
    if (r != OPTICS_GFX_OK) {
        material_free(m);
        return r;
    }
    *out = m;
    return OPTICS_GFX_OK;
}

tessera_scene_material *tessera_scene_material_retain(tessera_scene_material *m) {
    if (m)
        atomic_fetch_add_explicit(&m->ref_count, 1u, memory_order_relaxed);
    return m;
}

void tessera_scene_material_release(tessera_scene_material *m) {
    if (!m)
        return;
    if (atomic_fetch_sub_explicit(&m->ref_count, 1u, memory_order_acq_rel) != 1u)
        return;
    material_free(m);
}

tessera_scene_material_alpha_mode
tessera_scene_material_get_alpha_mode(const tessera_scene_material *m) {
    return m ? m->alpha_mode : TESSERA_SCENE_MATERIAL_ALPHA_OPAQUE;
}

/* ================================================================== */
/*  Draw                                                              */
/* ================================================================== */

static void fill_surface_params(scene_surface_params *p, const tessera_scene_material *material) {
    memset(p, 0, sizeof(*p));
    p->base_color[0] = material->base_color.x;
    p->base_color[1] = material->base_color.y;
    p->base_color[2] = material->base_color.z;
    p->base_color[3] = material->base_color.w;
    p->uv_scale_offset[0] = material->uv_scale.x;
    p->uv_scale_offset[1] = material->uv_scale.y;
    p->uv_scale_offset[2] = material->uv_offset.x;
    p->uv_scale_offset[3] = material->uv_offset.y;
    p->uv_rotation_alpha_cutoff[0] = cosf(material->uv_rotation);
    p->uv_rotation_alpha_cutoff[1] = sinf(material->uv_rotation);
    p->uv_rotation_alpha_cutoff[2] = material->alpha_cutoff;
    p->texture_info[0] = material->image_handle;
    p->texture_info[1] = material->sampler_handle;
    p->texture_info[2] = (uint32_t)material->alpha_mode;
    p->texture_info[3] = material->base_color_image ? 1u : 0u;
    /* ADR-0069: rendering into the linear working space (16F target)
     * decodes untagged 8-bit UNORM texels as sRGB. 16F sources are already
     * linear and *_SRGB sources are decoded by the sampler hardware: both
     * stay raw. Legacy 8-bit targets keep the historical raw path. */
    const optics_gfx_image *img = material->base_color_image;
    if (material->renders_working_space && img) {
        const optics_gfx_format format = optics_gfx_image_format(img);
        if (format == OPTICS_GFX_FORMAT_RGBA8_UNORM || format == OPTICS_GFX_FORMAT_BGRA8_UNORM ||
            format == OPTICS_GFX_FORMAT_RGB10A2_UNORM)
            p->color_flags = SCENE_COLOR_DECODE;
    }
}

/* Fill the per-draw phong parameter block. The normal matrix is the
 * transpose of the inverse of world's upper-left 3×3 (handles non-uniform
 * scale); the eye position is the translation column of the inverted view
 * matrix. */
static void fill_phong_params(scene_phong_params *p, const optics_gfx_mat4 *view_inv,
                              optics_gfx_mat4 world, const tessera_scene_material *material,
                              const tessera_scene_light *light) {
    static const tessera_scene_light default_light = TESSERA_SCENE_LIGHT_DEFAULT;
    if (!light)
        light = &default_light;

    memcpy(p->world, world.m, sizeof(p->world));

    /* Column i of transpose(inverse(world)) is row i of inverse(world);
     * optics_gfx_mat4 is column-major, so row i reads m[i], m[4+i], m[8+i]. */
    optics_gfx_mat4 inv = optics_gfx_mat4_invert(world);
    for (int i = 0; i < 3; ++i) {
        float *col = i == 0 ? p->nrm0 : i == 1 ? p->nrm1 : p->nrm2;
        col[0] = inv.m[i];
        col[1] = inv.m[4 + i];
        col[2] = inv.m[8 + i];
        col[3] = 0.0f;
    }

    fill_surface_params(&p->surface, material);

    optics_gfx_vec3 dir = optics_gfx_vec3_normalize(light->direction);
    if (optics_gfx_vec3_length(dir) == 0.0f)
        dir = optics_gfx_vec3_normalize(default_light.direction);
    p->light_dir_shininess[0] = dir.x;
    p->light_dir_shininess[1] = dir.y;
    p->light_dir_shininess[2] = dir.z;
    p->light_dir_shininess[3] = material->shininess;

    p->light_color_ambient[0] = light->color.x;
    p->light_color_ambient[1] = light->color.y;
    p->light_color_ambient[2] = light->color.z;
    p->light_color_ambient[3] = light->ambient;

    p->eye_specular[0] = view_inv->m[12];
    p->eye_specular[1] = view_inv->m[13];
    p->eye_specular[2] = view_inv->m[14];
    p->eye_specular[3] = material->specular;
}

void tessera_scene_draw_mesh(optics_gfx_frame *f, const tessera_scene_draw_mesh_opts *opts) {
    if (!f || !opts || !opts->camera || !opts->mesh || !opts->material)
        return;
    const tessera_scene_camera *cam = opts->camera;
    tessera_scene_mesh *mesh = opts->mesh;
    tessera_scene_material *material = opts->material;

    VkCommandBuffer cmd = optics_gfx_frame_vk_command_buffer(f);
    if (!cmd)
        return;

    /* MVP = projection * view * world (column-major; optics_gfx_mat4_multiply
     * mirrors math notation a*b). */
    optics_gfx_mat4 view_proj = optics_gfx_mat4_multiply(cam->projection, cam->view);
    optics_gfx_mat4 mvp = optics_gfx_mat4_multiply(view_proj, opts->world);
    const bool skinned = mesh->skin && opts->joint_matrices && opts->joint_count > 0;

    scene_skin_params skin = {};
    if (skinned) {
        size_t palette_bytes = (size_t)opts->joint_count * sizeof(optics_gfx_mat4);
        optics_gfx_transient palette;
        if (optics_gfx_frame_alloc_transient(
                f, &(optics_gfx_frame_alloc_transient_opts){
                       .bytes = palette_bytes, .alignment = 16, .out = &palette}) != OPTICS_GFX_OK)
            return;
        memcpy(palette.cpu, opts->joint_matrices, palette_bytes);
        skin.joint_palette_address = palette.gpu_address;
        skin.skin_address = optics_gfx_buffer_device_address(mesh->skin);
        skin.joint_count = opts->joint_count;
        if (skin.skin_address == 0)
            return;
    }

    optics_gfx_transient slice;
    const size_t params_bytes = material->kind == TESSERA_SCENE_MATERIAL_PHONG
                                    ? sizeof(scene_phong_params)
                                    : sizeof(scene_unlit_params);
    if (optics_gfx_frame_alloc_transient(f, &(optics_gfx_frame_alloc_transient_opts){
                                                .bytes = params_bytes,
                                                .alignment = 16,
                                                .out = &slice}) != OPTICS_GFX_OK)
        return; /* draw dropped; alloc_transient set the error */
    if (material->kind == TESSERA_SCENE_MATERIAL_PHONG) {
        scene_phong_params *params = slice.cpu;
        optics_gfx_mat4 view_inv = optics_gfx_mat4_invert(cam->view);
        fill_phong_params(params, &view_inv, opts->world, material, opts->light);
        params->skin = skin;
    } else {
        scene_unlit_params *params = slice.cpu;
        fill_surface_params(&params->surface, material);
        params->skin = skin;
    }

    scene_params_push pc = {};
    memcpy(pc.mvp, mvp.m, sizeof(pc.mvp));
    pc.params_address = slice.gpu_address;
    optics_gfx_graphics_pipeline_bind(
        f, &(optics_gfx_graphics_pipeline_bind_opts){
               .pipeline = skinned ? material->skinned_pipeline : material->pipeline,
               .push_constants = &pc,
               .push_bytes = sizeof(pc),
           });

    VkBuffer vertex_buffer = optics_gfx_buffer_vk_buffer(mesh->vertices);
    VkDeviceSize offset = 0;
    vkCmdBindVertexBuffers(cmd, 0, 1, &vertex_buffer, &offset);
    if (mesh->index_count > 0 && mesh->indices) {
        vkCmdBindIndexBuffer(cmd, optics_gfx_buffer_vk_buffer(mesh->indices), 0,
                             VK_INDEX_TYPE_UINT32);
        vkCmdDrawIndexed(cmd, mesh->index_count, 1, 0, 0, 0);
    } else {
        vkCmdDraw(cmd, mesh->vertex_count, 1, 0, 0);
    }
}
