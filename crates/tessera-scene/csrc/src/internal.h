/*
 * Internal types for scene: a minimal JSON value model used to
 * walk the glTF JSON chunk, and the loaded scene representation that owns
 * refcounted gfx mesh/material handles.
 */
#ifndef TESSERA_SCENE_INTERNAL_H
#define TESSERA_SCENE_INTERNAL_H

#include <optics/gfx/observe.h>
#include <tessera/scene.h>

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* ------------------------------------------------------------------ */
/*  JSON value tree                                                   */
/* ------------------------------------------------------------------ */

typedef enum : uint32_t { J_NULL, J_BOOL, J_NUM, J_STR, J_ARR, J_OBJ } jkind;

typedef struct jv {
    jkind kind;
    union {
        bool b;
        double num;
        struct {
            char *data;
            size_t len;
        } str; /* len excludes the NUL */
        struct {
            struct jv **items;
            size_t count;
        } arr;
        struct {
            char **keys;
            struct jv **vals;
            size_t count;
        } obj;
    };
} jv;

/* Parse `len` bytes of UTF-8 JSON into a tree. Returns NULL on syntax
 * error (the byte offset of the failure is written to *err_off when
 * non-NULL). The tree is heap-owned; free with jv_free. */
jv *jv_parse(const char *json, size_t len, size_t *err_off);

/* Recursively free a tree returned by jv_parse. NULL-safe. */
void jv_free(jv *v);

/* Accessors. Object lookup is linear (glTF objects are small). Return NULL
 * when absent or when `v` is the wrong kind. */
const jv *jv_obj_get(const jv *v, const char *key);
const jv *jv_arr_at(const jv *v, size_t i);
double jv_num(const jv *v, double fallback);
bool jv_bool(const jv *v, bool fallback);

/* VRM humanoid bone-slot count (shared by parse and scene). */
#define SG_HUMAN_BONE_COUNT 55

/* ------------------------------------------------------------------ */
/*  Loaded scene                                                      */
/* ------------------------------------------------------------------ */

/* One renderable primitive: a mesh built on the device plus the base colour
 * parsed from the glTF material (linear RGBA, default white). */
typedef struct tessera_scene_primitive {
    tessera_scene_mesh *mesh;
    optics_gfx_vec4 base_color; /* linear; parsed from pbrMetallicRoughness */
    int material_index;         /* -1 selects the installed fallback */
    optics_gfx_vec3 aabb_min;   /* local-space AABB of the mesh vertices */
    optics_gfx_vec3 aabb_max;
} tessera_scene_primitive;

typedef struct tessera_scene_skin {
    int *joints;
    optics_gfx_mat4 *inverse_bind;
    optics_gfx_mat4 *palette;
    uint32_t joint_count;
} tessera_scene_skin;

/* A glTF node. `parent` is -1 for roots. */
typedef struct tessera_scene_node {
    int parent;
    optics_gfx_vec3 rest_translation;
    optics_gfx_quat rest_rotation;
    optics_gfx_vec3 rest_scale;
    optics_gfx_vec3 translation;
    optics_gfx_quat rotation;
    optics_gfx_vec3 scale;
    optics_gfx_quat rest_world_rotation;
    optics_gfx_mat4 world; /* cached; recomputed each draw */
    int mesh_first;        /* -1 if this node has no mesh */
    int mesh_prim_count;
    int skin; /* -1 if this node is not skinned */
    char *name;
} tessera_scene_node;

typedef enum tessera_scene_animation_path : uint32_t {
    SG_ANIM_TRANSLATION,
    SG_ANIM_ROTATION,
    SG_ANIM_SCALE,
} tessera_scene_animation_path;

typedef enum tessera_scene_interpolation : uint32_t {
    SG_INTERP_LINEAR,
    SG_INTERP_STEP,
    SG_INTERP_CUBIC,
} tessera_scene_interpolation;

typedef struct tessera_scene_animation_channel {
    int target_node;
    tessera_scene_animation_path path;
    tessera_scene_interpolation interpolation;
    float *times;
    float *values;
    uint32_t key_count;
    uint8_t components;
} tessera_scene_animation_channel;

struct tessera_scene_animation {
    int32_t refcount;
    const tessera_scene *target;
    tessera_scene_animation_channel *channels;
    uint32_t channel_count;
    float duration;
};

struct tessera_scene {
    int32_t refcount;
    /* Retained device, so the scene can settle its own ledger row at release
     * (ADR-0134). Without it a leaked scene was reported as the gfx objects
     * it holds, pointing the reader at gfx instead of scene. */
    optics_gfx_device *device;
    tessera_scene_primitive *prims; /* flat list across all meshes */
    uint32_t prim_count;
    tessera_scene_node *nodes;
    uint32_t node_count;
    tessera_scene_skin *skins;
    uint32_t skin_count;
    tessera_scene_material **materials;
    uint32_t material_count;
    tessera_scene_material *fallback_material;
    int *roots; /* node indices */
    uint32_t root_count;
    int human_bones[SG_HUMAN_BONE_COUNT];
    uint32_t *world_order;
};

/* ------------------------------------------------------------------ */
/*  Parsed scene data (CPU-only, no device)                           */
/* ------------------------------------------------------------------ */

typedef struct sg_primitive_data {
    tessera_scene_vertex *vertices;
    uint32_t vertex_count;
    uint32_t *indices; /* NULL = non-indexed */
    uint32_t index_count;
    tessera_scene_skin_vertex *skin_vertices; /* NULL = static mesh */
    optics_gfx_vec3 aabb_min;
    optics_gfx_vec3 aabb_max;
    optics_gfx_vec4 base_color;
    int material_index;
} sg_primitive_data;

struct tessera_scene_data {
    sg_primitive_data *prims;
    uint32_t prim_count;
    tessera_scene_node *nodes;
    uint32_t node_count;
    tessera_scene_skin *skins;
    uint32_t skin_count;
    int *roots;
    uint32_t root_count;
    int human_bones[SG_HUMAN_BONE_COUNT];
};

typedef struct tessera_scene_data sg_scene_data;

void sg_data_free(sg_scene_data *data);
void sg_data_transfer(sg_scene_data *data, tessera_scene *sc);
optics_gfx_result sg_parse_glb(const void *bytes, size_t len, sg_scene_data *data);
optics_gfx_result sg_data_build(optics_gfx_device *dev, sg_scene_data *data, tessera_scene *sc);

typedef struct sg_glb {
    const uint8_t *json;
    size_t json_len;
    const uint8_t *bin;
    size_t bin_len;
} sg_glb;

#define SG_CT_BYTE 5120
#define SG_CT_UBYTE 5121
#define SG_CT_SHORT 5122
#define SG_CT_USHORT 5123
#define SG_CT_UINT 5125
#define SG_CT_FLOAT 5126

optics_gfx_result sg_glb_parse(const void *bytes, size_t len, sg_glb *out);
const uint8_t *sg_accessor_data(const jv *root, int idx, const uint8_t *bin, size_t bin_len,
                                int *comp, int *comps, size_t *count, size_t *stride,
                                bool *normalized);
void sg_read_floats(const uint8_t *ptr, int comp, int comps, bool normalized, float *out);
void sg_read_node(const jv *node_json, tessera_scene_node *node);
void sg_update_worlds(tessera_scene *scene);
void sg_update_rest_world_rotations(tessera_scene *scene);
void sg_update_skin_palettes(tessera_scene *scene);
int sg_human_bone_index(const char *name, bool legacy_vrm0);
void sg_read_humanoid(const jv *root, const char *extension_name, bool legacy_vrm0,
                      int out_bones[SG_HUMAN_BONE_COUNT]);

optics_gfx_result sg_parse_animation_glb(const tessera_scene *target, const void *bytes, size_t len,
                                         tessera_scene_animation **out);

#endif /* TESSERA_SCENE_INTERNAL_H */
