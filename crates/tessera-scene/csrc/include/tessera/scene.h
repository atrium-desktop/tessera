/*
 * tessera/scene.h — glTF 2.0 3D scene hierarchy, TRS transforms, and asset runtime (ADR-0016,
 * ADR-0106).
 *
 * Tessera's scene runtime (ADR-0262), carried from the Optics scene library
 * that Optics ADR-0174 retired: it parses glTF, builds mesh resources on the
 * Optics gfx public RHI, and records 3D scene draws through
 * tessera_scene_draw_mesh.
 *
 * Supported subset:
 *   - Binary glTF (.glb) with embedded buffer (the BIN chunk).
 *   - Indexed primitives with POSITION, NORMAL, optional TEXCOORD_0, and
 *     JOINTS_0/WEIGHTS_0 GPU skinning.
 *   - Single scene/node tree; mutable TRS poses compose into world matrices.
 *   - glTF animation sampling (STEP/LINEAR/CUBICSPLINE) and external VRM
 *     Animation 1.0 clips retargeted onto VRM 0.x or VRM 1.0 humanoid rigs.
 *   - Per-primitive materials may be installed by the host; callers may
 *     still override every primitive with one material at draw time.
 */

#ifndef TESSERA_SCENE_H
#define TESSERA_SCENE_H

#include <optics/gfx/core.h>  /* optics_gfx_device, optics_gfx_frame, optics_gfx_result        */
#include <optics/gfx/math.h>  /* optics_gfx_mat4, optics_gfx_vec3                         */
#include <tessera/scene/render.h> /* tessera_scene_camera, tessera_scene_material, tessera_scene_light*/

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ================================================================== */
/*  Scene                                                             */
/* ================================================================== */

/* Opaque loaded scene: a tree of nodes, each carrying an optional mesh
 * (one or more primitives), a local transform, and a cached world matrix. */
typedef struct tessera_scene tessera_scene;
typedef struct tessera_scene_animation tessera_scene_animation;

typedef struct tessera_scene_load_glb_opts {
    const void *bytes;
    size_t byte_count;
    tessera_scene **out;
} tessera_scene_load_glb_opts;

/* Parse a .glb (binary glTF 2.0) and build GPU resources on `device`. */
OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result
tessera_scene_load_glb(optics_gfx_device *device, const tessera_scene_load_glb_opts *opts);

/* Opaque parsed scene: everything a .glb contains, minus the device. */
typedef struct tessera_scene_data tessera_scene_data;

typedef struct tessera_scene_parse_glb_opts {
    const void *bytes;
    size_t byte_count;
    tessera_scene_data **out;
} tessera_scene_parse_glb_opts;

/* Parse a .glb into device-independent scene data (CPU parse only). */
OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result
tessera_scene_parse_glb(const tessera_scene_parse_glb_opts *opts);

typedef struct tessera_scene_data_build_opts {
    tessera_scene_data *data;
    tessera_scene **out;
} tessera_scene_data_build_opts;

/* Upload a parsed scene onto `device`: one tessera_scene_mesh per primitive. */
OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result
tessera_scene_data_build(optics_gfx_device *device, const tessera_scene_data_build_opts *opts);

/* Release a parsed scene data. */
TESSERA_SCENE_API void tessera_scene_data_free(tessera_scene_data *data);

/* Number of mesh primitives in the parsed scene. */
TESSERA_SCENE_API uint32_t tessera_scene_data_primitive_count(const tessera_scene_data *data);

typedef struct tessera_scene_data_bounds_opts {
    optics_gfx_vec3 *out_min;
    optics_gfx_vec3 *out_max;
} tessera_scene_data_bounds_opts;

/* World-space axis-aligned bounding box of parsed primitives. */
TESSERA_SCENE_API bool tessera_scene_data_bounds(const tessera_scene_data *data,
                                               const tessera_scene_data_bounds_opts *opts);

TESSERA_SCENE_API tessera_scene *tessera_scene_retain(tessera_scene *scene);
TESSERA_SCENE_API void tessera_scene_release(tessera_scene *scene);

typedef struct tessera_scene_set_materials_opts {
    tessera_scene_material *const *materials;
    uint32_t material_count;
    tessera_scene_material *fallback;
} tessera_scene_set_materials_opts;

/* Transactionally replace the scene-owned per-index material table. */
OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result
tessera_scene_set_materials(tessera_scene *scene, const tessera_scene_set_materials_opts *opts);

/* Number of mesh primitives in the scene. */
TESSERA_SCENE_API uint32_t tessera_scene_primitive_count(const tessera_scene *scene);

typedef struct tessera_scene_bounds_opts {
    optics_gfx_vec3 *out_min;
    optics_gfx_vec3 *out_max;
} tessera_scene_bounds_opts;

/* World-space axis-aligned bounding box of every primitive drawn by the scene. */
TESSERA_SCENE_API bool tessera_scene_bounds(const tessera_scene *scene,
                                          const tessera_scene_bounds_opts *opts);

typedef struct tessera_scene_humanoid_bone_position_opts {
    const char *bone_name;
    optics_gfx_vec3 *out_position;
} tessera_scene_humanoid_bone_position_opts;

/* Current model-space position of a VRM humanoid bone (e.g. "head" or "hips"). */
TESSERA_SCENE_API bool
tessera_scene_humanoid_bone_position(const tessera_scene *scene,
                                    const tessera_scene_humanoid_bone_position_opts *opts);

typedef struct tessera_scene_load_animation_glb_opts {
    const void *bytes;
    size_t byte_count;
    tessera_scene_animation **out;
} tessera_scene_load_animation_glb_opts;

/* Load animation from a binary glTF/VRMA file and bind its channels to `target`. */
OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result tessera_scene_load_animation_glb(
    const tessera_scene *target, const tessera_scene_load_animation_glb_opts *opts);
TESSERA_SCENE_API tessera_scene_animation *
tessera_scene_animation_retain(tessera_scene_animation *animation);
TESSERA_SCENE_API void tessera_scene_animation_release(tessera_scene_animation *animation);
TESSERA_SCENE_API float tessera_scene_animation_duration(const tessera_scene_animation *animation);
TESSERA_SCENE_API uint32_t
tessera_scene_animation_channel_count(const tessera_scene_animation *animation);

typedef struct tessera_scene_apply_animation_opts {
    const tessera_scene_animation *animation;
    float time_seconds;
    bool loop;
} tessera_scene_apply_animation_opts;

/* Reset to the model rest pose, then sample and apply the clip. */
OPTICS_GFX_NODISCARD TESSERA_SCENE_API optics_gfx_result
tessera_scene_apply_animation(tessera_scene *scene, const tessera_scene_apply_animation_opts *opts);
TESSERA_SCENE_API void tessera_scene_reset_pose(tessera_scene *scene);

/* ================================================================== */
/*  Draw                                                              */
/* ================================================================== */

typedef struct tessera_scene_draw_opts {
    const tessera_scene_camera *camera;
    const tessera_scene *scene;
    tessera_scene_material *material;
    const tessera_scene_light *light;
} tessera_scene_draw_opts;

/* Record mesh draw calls composed with each node's world matrix. */
TESSERA_SCENE_API void tessera_scene_draw(optics_gfx_frame *frame,
                                        const tessera_scene_draw_opts *opts);

#ifdef __cplusplus
}
#endif

#endif /* TESSERA_SCENE_H */
