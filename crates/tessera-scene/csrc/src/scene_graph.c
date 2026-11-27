/*
 * scene — public API implementation.
 *
 * The scene owns refcounted tessera_scene_mesh handles and a node tree with cached
 * world matrices. Draw records one tessera_scene_draw_mesh(_lit) per primitive,
 * composed with the owning node's world matrix, into the caller's active
 * pass. The scene never touches the canvas or scene module internals — it is
 * a pure consumer of the tessera_scene_draw_mesh primitive (ADR-0016).
 */
#include "internal.h"

#include <optics/gfx/math.h>
#include <tessera/scene/render.h>

#include <float.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

TESSERA_SCENE_API optics_gfx_result tessera_scene_load_glb(optics_gfx_device *device,
                                                         const tessera_scene_load_glb_opts *opts) {
    if (!device || !opts || !opts->bytes || !opts->out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *opts->out = nullptr;
    const void *glb_bytes = opts->bytes;
    size_t byte_count = opts->byte_count;
    tessera_scene **out = opts->out;

    /* Two-stage pipeline: parse (device-independent, testable, fuzzer-
     * reachable) then build (the only stage touching the device). */
    sg_scene_data data;
    memset(&data, 0, sizeof(data));
    optics_gfx_result r = sg_parse_glb(glb_bytes, byte_count, &data);
    if (r != OPTICS_GFX_OK)
        return r; /* sg_parse_glb releases everything it allocated on failure */

    tessera_scene *sc = calloc(1, sizeof(*sc));
    if (!sc) {
        sg_data_free(&data);
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    }
    sc->refcount = 1;
    r = sg_data_build(device, &data, sc);
    /* sg_data_build consumed `data` either way; on failure it released
     * whatever it uploaded and left `sc` safe to free. */
    if (r != OPTICS_GFX_OK) {
        tessera_scene_release(sc);
        return r;
    }
    *out = sc;
    return OPTICS_GFX_OK;
}

/* ---- Parsed-data lifecycle (the parse/build seam, ADR-0016) -------- */

/* ---- Parsed-data lifecycle (the parse/build seam, ADR-0016) -------- */

/* The internal header completes the same struct tag the public header
 * declares (classic C opaque pattern), so the public handle and the
 * internal storage are one type — no downcasts, no layout drift. */

TESSERA_SCENE_API optics_gfx_result tessera_scene_parse_glb(const tessera_scene_parse_glb_opts *opts) {
    if (!opts || !opts->bytes || !opts->out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *opts->out = nullptr;
    sg_scene_data *data = calloc(1, sizeof(*data));
    if (!data)
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    optics_gfx_result r = sg_parse_glb(opts->bytes, opts->byte_count, data);
    if (r != OPTICS_GFX_OK) {
        /* sg_parse_glb releases everything it allocated on failure. */
        free(data);
        return r;
    }
    *opts->out = (tessera_scene_data *)(void *)data;
    return OPTICS_GFX_OK;
}

void sg_data_free(sg_scene_data *data) {
    if (!data)
        return;
    for (uint32_t i = 0; i < data->prim_count; ++i) {
        free(data->prims[i].vertices);
        free(data->prims[i].indices);
        free(data->prims[i].skin_vertices);
    }
    free(data->prims);
    if (data->nodes)
        for (uint32_t i = 0; i < data->node_count; ++i)
            free(data->nodes[i].name);
    free(data->nodes);
    if (data->skins)
        for (uint32_t i = 0; i < data->skin_count; ++i) {
            free(data->skins[i].joints);
            free(data->skins[i].inverse_bind);
            free(data->skins[i].palette);
        }
    free(data->skins);
    free(data->roots);
    memset(data, 0, sizeof(*data));
}

TESSERA_SCENE_API void tessera_scene_data_free(tessera_scene_data *data) {
    sg_data_free(data);
    free(data);
}

TESSERA_SCENE_API uint32_t tessera_scene_data_primitive_count(const tessera_scene_data *data) {
    return data ? data->prim_count : 0;
}

TESSERA_SCENE_API bool tessera_scene_data_bounds(const tessera_scene_data *data,
                                               const tessera_scene_data_bounds_opts *opts) {
    if (!opts || !opts->out_min || !opts->out_max || !data)
        return false;
    optics_gfx_vec3 *out_min = opts->out_min;
    optics_gfx_vec3 *out_max = opts->out_max;
    optics_gfx_vec3 wmin = {FLT_MAX, FLT_MAX, FLT_MAX};
    optics_gfx_vec3 wmax = {-FLT_MAX, -FLT_MAX, -FLT_MAX};
    bool any = false;
    for (uint32_t k = 0; k < data->prim_count; ++k) {
        if (data->prims[k].vertices) {
            wmin.x = fminf(wmin.x, data->prims[k].aabb_min.x);
            wmin.y = fminf(wmin.y, data->prims[k].aabb_min.y);
            wmin.z = fminf(wmin.z, data->prims[k].aabb_min.z);
            wmax.x = fmaxf(wmax.x, data->prims[k].aabb_max.x);
            wmax.y = fmaxf(wmax.y, data->prims[k].aabb_max.y);
            wmax.z = fmaxf(wmax.z, data->prims[k].aabb_max.z);
            any = true;
        }
    }
    if (!any)
        return false;
    *out_min = wmin;
    *out_max = wmax;
    return true;
}

void sg_data_transfer(sg_scene_data *data, tessera_scene *sc) {
    /* sc->prims is the build stage's business (it owns the upload);
     * this moves only the parse-owned node/skin/root state. */
    sc->nodes = data->nodes;
    data->nodes = nullptr;
    sc->node_count = data->node_count;
    data->node_count = 0;
    sc->skins = data->skins;
    data->skins = nullptr;
    sc->skin_count = data->skin_count;
    data->skin_count = 0;
    sc->roots = data->roots;
    data->roots = nullptr;
    sc->root_count = data->root_count;
    data->root_count = 0;
    for (int i = 0; i < SG_HUMAN_BONE_COUNT; ++i)
        sc->human_bones[i] = data->human_bones[i];
}

optics_gfx_result sg_data_build(optics_gfx_device *dev, sg_scene_data *data, tessera_scene *sc) {
    if (!dev || !data || !sc)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;

    /* Upload every parsed primitive. On failure the already-uploaded
     * meshes are released here; the node/skin transfers below have not
     * happened yet (they are only committed on full success), so the
     * caller's zeroed `data` and `sc` stay consistent for either path. */
    tessera_scene_primitive *prims =
        data->prim_count ? calloc(data->prim_count, sizeof(*prims)) : nullptr;
    if (data->prim_count && !prims) {
        sg_data_free(data);
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    }
    for (uint32_t i = 0; i < data->prim_count; ++i) {
        const sg_primitive_data *pd = &data->prims[i];
        tessera_scene_mesh_skin_desc sd = {
            .type = TESSERA_SCENE_TYPE_MESH_SKIN_DESC,
            .vertices = pd->skin_vertices,
        };
        tessera_scene_mesh_desc md = {
            .type = TESSERA_SCENE_TYPE_MESH_DESC,
            .next = pd->skin_vertices ? &sd : nullptr,
            .vertices = pd->vertices,
            .vertex_count = pd->vertex_count,
            .indices = pd->indices,
            .index_count = pd->index_count,
        };
        optics_gfx_result r = tessera_scene_mesh_create(dev, &md, &prims[i].mesh);
        if (r != OPTICS_GFX_OK) {
            for (uint32_t k = 0; k < i; ++k)
                tessera_scene_mesh_release(prims[k].mesh);
            free(prims);
            sg_data_free(data);
            return r;
        }
        prims[i].base_color = pd->base_color;
        prims[i].material_index = pd->material_index;
        prims[i].aabb_min = pd->aabb_min;
        prims[i].aabb_max = pd->aabb_max;
        /* The mesh owns copies on the GPU; the parsed buffers are
         * consumed here, one primitive at a time (the array free below
         * then sees zeroed slots and frees nothing twice). */
        free(pd->vertices);
        free(pd->indices);
        free(pd->skin_vertices);
        data->prims[i] = (sg_primitive_data){};
    }

    /* All meshes live: transfer everything into the scene and finish the
     * derived CPU state (world matrices, rest rotations, skin palettes).
     * The parsed primitive array itself is consumed here — its buffers
     * were freed per-primitive above, the copies now live in sc->prims. */
    sc->prims = prims;
    sc->prim_count = data->prim_count;
    free(data->prims);
    data->prims = nullptr;
    data->prim_count = 0;
    sg_data_transfer(data, sc);
    sg_data_free(data); /* nodes/skins/roots transferred; nothing left */
    sg_update_worlds(sc);
    sg_update_rest_world_rotations(sc);
    sg_update_skin_palettes(sc);
    return OPTICS_GFX_OK;
}

TESSERA_SCENE_API optics_gfx_result
tessera_scene_data_build(optics_gfx_device *device, const tessera_scene_data_build_opts *opts) {
    if (!device || !opts || !opts->data || !opts->out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *opts->out = nullptr;
    tessera_scene_data *data = opts->data;
    tessera_scene **out = opts->out;
    tessera_scene *sc = calloc(1, sizeof(*sc));
    if (!sc) {
        sg_data_free(data);
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    }
    sc->refcount = 1;
    sc->device = optics_gfx_device_retain(device);
    optics_gfx_result r = sg_data_build(device, data, sc);
    /* sg_data_build consumed `data` either way; on failure it released
     * whatever it uploaded and left `sc` safe to free. */
    if (r != OPTICS_GFX_OK) {
        tessera_scene_release(sc);
        return r;
    }
    *out = sc;
    /* Context-scope row (ADR-0134): a scene outlives frames and dies with the
     * app's asset set. */
    OPTICS_GFX_OBSERVE_ACQUIRE(sc->device, OPTICS_GFX_SCOPE_CONTEXT, OPTICS_GFX_LIFETIME_CONTEXT,
                               "scene", "tessera/scene", 0, OPTICS_GFX_SITE_NONE);
    return OPTICS_GFX_OK;
}

TESSERA_SCENE_API tessera_scene *tessera_scene_retain(tessera_scene *scene) {
    if (scene)
        scene->refcount++;
    return scene;
}

TESSERA_SCENE_API void tessera_scene_release(tessera_scene *scene) {
    if (!scene)
        return;
    if (--scene->refcount > 0)
        return;
    for (uint32_t i = 0; i < scene->prim_count; ++i)
        if (scene->prims[i].mesh)
            tessera_scene_mesh_release(scene->prims[i].mesh);
    for (uint32_t i = 0; i < scene->node_count; ++i)
        free(scene->nodes[i].name);
    for (uint32_t i = 0; i < scene->skin_count; ++i) {
        free(scene->skins[i].joints);
        free(scene->skins[i].inverse_bind);
        free(scene->skins[i].palette);
    }
    for (uint32_t i = 0; i < scene->material_count; ++i)
        tessera_scene_material_release(scene->materials[i]);
    tessera_scene_material_release(scene->fallback_material);
    OPTICS_GFX_OBSERVE_RELEASE(scene->device, OPTICS_GFX_SCOPE_CONTEXT, "scene", "tessera/scene", 0);
    if (scene->device)
        optics_gfx_device_release(scene->device);
    free(scene->materials);
    free(scene->prims);
    free(scene->nodes);
    free(scene->skins);
    free(scene->roots);
    free(scene->world_order);
    free(scene);
}

TESSERA_SCENE_API optics_gfx_result
tessera_scene_set_materials(tessera_scene *scene, const tessera_scene_set_materials_opts *opts) {
    if (!scene || !opts || (opts->material_count > 0 && !opts->materials))
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    tessera_scene_material *const *materials = opts->materials;
    uint32_t material_count = opts->material_count;
    tessera_scene_material *fallback = opts->fallback;
    tessera_scene_material **replacement = nullptr;
    if (material_count > 0) {
        /* Pointer array: the element is a tessera_scene_material *, not a struct. */
        replacement =
            calloc(material_count, sizeof(*replacement)); // NOLINT(bugprone-sizeof-expression)
        if (!replacement)
            return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        for (uint32_t i = 0; i < material_count; ++i)
            replacement[i] = tessera_scene_material_retain(materials[i]);
    }
    tessera_scene_material *replacement_fallback = tessera_scene_material_retain(fallback);

    tessera_scene_material **old = scene->materials;
    uint32_t old_count = scene->material_count;
    tessera_scene_material *old_fallback = scene->fallback_material;
    scene->materials = replacement;
    scene->material_count = material_count;
    scene->fallback_material = replacement_fallback;

    for (uint32_t i = 0; i < old_count; ++i)
        tessera_scene_material_release(old[i]);
    tessera_scene_material_release(old_fallback);
    free(old);
    return OPTICS_GFX_OK;
}

TESSERA_SCENE_API uint32_t tessera_scene_primitive_count(const tessera_scene *scene) {
    return scene ? scene->prim_count : 0;
}

TESSERA_SCENE_API bool
tessera_scene_humanoid_bone_position(const tessera_scene *scene,
                                    const tessera_scene_humanoid_bone_position_opts *opts) {
    if (!scene || !opts || !opts->bone_name || !opts->out_position)
        return false;
    const char *bone_name = opts->bone_name;
    optics_gfx_vec3 *out_position = opts->out_position;
    int bone = sg_human_bone_index(bone_name, false);
    if (bone < 0)
        return false;
    int node = scene->human_bones[bone];
    if (node < 0 || (uint32_t)node >= scene->node_count)
        return false;
    *out_position =
        optics_gfx_vec3_make(scene->nodes[node].world.m[12], scene->nodes[node].world.m[13],
                             scene->nodes[node].world.m[14]);
    return true;
}

static optics_gfx_mat4 node_local(const tessera_scene_node *node) {
    optics_gfx_mat4 t =
        optics_gfx_mat4_translate(node->translation.x, node->translation.y, node->translation.z);
    optics_gfx_mat4 r = optics_gfx_mat4_rotation_quat(node->rotation);
    optics_gfx_mat4 s = optics_gfx_mat4_scale(node->scale.x, node->scale.y, node->scale.z);
    return optics_gfx_mat4_multiply(t, optics_gfx_mat4_multiply(r, s));
}

/* Build the node index order in which every node appears after its parent.
 *
 * glTF node arrays are not required to be topologically sorted, and a node's
 * parent link never changes after parse/build (`sg_read_node`/glb.c set it;
 * nothing mutates it afterwards). The previous implementation iterated the
 * whole node array `node_count` times to converge such a tree — O(n^2) matrix
 * multiplies per animated frame, which dominated the frame budget for a
 * skinned VRM avatar. Instead, compute a parents-before-children order once
 * (a depth-first walk from every root; roots are derived from the parent
 * links, not `scene->roots`, because the animation retargeter's scratch scene
 * has parent links but no root list) and fold each node's world from its
 * parent's in one linear pass.
 *
 * The walk is iterative so a pathological deep chain cannot overflow the C
 * stack, and a malformed parent cycle is broken rather than followed (a node
 * whose parent is not yet placed is treated as a root, and any node the walk
 * never reaches is appended so the linear pass still visits it). Returns NULL
 * on allocation failure or an empty scene. */
static uint32_t *sg_build_world_order(const tessera_scene *scene) {
    uint32_t n = scene->node_count;
    if (n == 0)
        return nullptr;
    uint32_t *order = malloc((size_t)n * sizeof(*order));
    bool *placed = calloc(n, sizeof(*placed));
    /* head[i] is the first child of i; next[i] the following sibling; both
     * UINT32_MAX-terminated, so the walk needs no per-node child array. */
    uint32_t *head = malloc((size_t)n * sizeof(*head));
    uint32_t *next = malloc((size_t)n * sizeof(*next));
    uint32_t *stack = malloc((size_t)n * sizeof(*stack));
    if (!order || !placed || !head || !next || !stack) {
        free(order);
        free(placed);
        free(head);
        free(next);
        free(stack);
        return nullptr;
    }
    for (uint32_t i = 0; i < n; ++i) {
        head[i] = UINT32_MAX;
        next[i] = UINT32_MAX;
    }
    for (uint32_t i = 0; i < n; ++i) {
        int parent = scene->nodes[i].parent;
        /* Skip self/out-of-range parents: such a node folds as a root below,
         * so it must not enter a sibling list that a cycle could trap. */
        if (parent >= 0 && (uint32_t)parent < n && (uint32_t)parent != i) {
            next[i] = head[parent];
            head[parent] = i;
        }
    }

    uint32_t count = 0;
    for (uint32_t root = 0; root < n; ++root) {
        int parent = scene->nodes[root].parent;
        bool is_root = parent < 0 || (uint32_t)parent >= n || (uint32_t)parent == root;
        if (!is_root || placed[root])
            continue;
        uint32_t top = 0;
        stack[top++] = root;
        while (top > 0) {
            uint32_t node = stack[--top];
            if (placed[node])
                continue;
            placed[node] = true;
            order[count++] = node;
            for (uint32_t c = head[node]; c != UINT32_MAX; c = next[c])
                if (!placed[c])
                    stack[top++] = c;
        }
    }
    /* A node in an unrooted cycle keeps index order so it is still visited. */
    for (uint32_t i = 0; i < n; ++i)
        if (!placed[i])
            order[count++] = i;

    free(placed);
    free(head);
    free(next);
    free(stack);
    return order;
}

void sg_update_worlds(tessera_scene *scene) {
    if (!scene)
        return;
    if (scene->node_count > 0 && !scene->world_order)
        scene->world_order = sg_build_world_order(scene);
    if (!scene->world_order) {
        /* Allocation failure (or an empty scene): fall back to the array-order
         * convergence loop so world matrices stay correct, just slower. */
        for (uint32_t i = 0; i < scene->node_count; ++i) {
            tessera_scene_node *node = &scene->nodes[i];
            node->world = node->parent < 0 ? node_local(node) : optics_gfx_mat4_identity();
        }
        for (uint32_t pass = 0; pass < scene->node_count; ++pass)
            for (uint32_t i = 0; i < scene->node_count; ++i) {
                tessera_scene_node *node = &scene->nodes[i];
                if (node->parent >= 0 && (uint32_t)node->parent < scene->node_count)
                    node->world = optics_gfx_mat4_multiply(scene->nodes[node->parent].world,
                                                           node_local(node));
            }
        return;
    }
    for (uint32_t k = 0; k < scene->node_count; ++k) {
        tessera_scene_node *node = &scene->nodes[scene->world_order[k]];
        optics_gfx_mat4 local = node_local(node);
        int parent = node->parent;
        /* A missing/out-of-range/self parent folds as a root (matches the
         * order builder and keeps a malformed file from indexing out of
         * bounds). */
        node->world = (parent < 0 || (uint32_t)parent >= scene->node_count ||
                       (uint32_t)parent == scene->world_order[k])
                          ? local
                          : optics_gfx_mat4_multiply(scene->nodes[parent].world, local);
    }
}

void sg_update_rest_world_rotations(tessera_scene *scene) {
    if (!scene)
        return;
    for (uint32_t i = 0; i < scene->node_count; ++i)
        scene->nodes[i].rest_world_rotation = scene->nodes[i].rest_rotation;
    for (uint32_t pass = 0; pass < scene->node_count; ++pass)
        for (uint32_t i = 0; i < scene->node_count; ++i) {
            tessera_scene_node *node = &scene->nodes[i];
            if (node->parent >= 0 && (uint32_t)node->parent < scene->node_count)
                node->rest_world_rotation = optics_gfx_quat_normalize(optics_gfx_quat_multiply(
                    scene->nodes[node->parent].rest_world_rotation, node->rest_rotation));
        }
}

void sg_update_skin_palettes(tessera_scene *scene) {
    if (!scene)
        return;
    for (uint32_t si = 0; si < scene->skin_count; ++si) {
        tessera_scene_skin *skin = &scene->skins[si];
        for (uint32_t ji = 0; ji < skin->joint_count; ++ji) {
            int node = skin->joints[ji];
            skin->palette[ji] =
                node >= 0 && (uint32_t)node < scene->node_count
                    ? optics_gfx_mat4_multiply(scene->nodes[node].world, skin->inverse_bind[ji])
                    : optics_gfx_mat4_identity();
        }
    }
}

/* Expand [*wmin, *wmax] to contain the 8 corners of [bmin, bmax] after
 * transformation by world matrix m. */
static void expand_world_aabb(optics_gfx_mat4 m, optics_gfx_vec3 bmin, optics_gfx_vec3 bmax,
                              optics_gfx_vec3 *wmin, optics_gfx_vec3 *wmax) {
    const float cx[2] = {bmin.x, bmax.x};
    const float cy[2] = {bmin.y, bmax.y};
    const float cz[2] = {bmin.z, bmax.z};
    for (int i = 0; i < 2; ++i)
        for (int j = 0; j < 2; ++j)
            for (int k = 0; k < 2; ++k) {
                optics_gfx_vec4 wp = optics_gfx_mat4_transform_vec4(
                    m, optics_gfx_vec4_make(cx[i], cy[j], cz[k], 1.0f));
                if (wp.x < wmin->x)
                    wmin->x = wp.x;
                if (wp.y < wmin->y)
                    wmin->y = wp.y;
                if (wp.z < wmin->z)
                    wmin->z = wp.z;
                if (wp.x > wmax->x)
                    wmax->x = wp.x;
                if (wp.y > wmax->y)
                    wmax->y = wp.y;
                if (wp.z > wmax->z)
                    wmax->z = wp.z;
            }
}

TESSERA_SCENE_API bool tessera_scene_bounds(const tessera_scene *scene,
                                          const tessera_scene_bounds_opts *opts) {
    if (!scene || !opts || !opts->out_min || !opts->out_max)
        return false;
    optics_gfx_vec3 *out_min = opts->out_min;
    optics_gfx_vec3 *out_max = opts->out_max;
    optics_gfx_vec3 wmin = {1e30f, 1e30f, 1e30f};
    optics_gfx_vec3 wmax = {-1e30f, -1e30f, -1e30f};
    bool any = false;

    /* Match tessera_scene_draw's visibility rule: when the scene has a node
     * tree, primitives are drawn via their owning node's world matrix;
     * a node-less scene draws all primitives with an identity world. */
    if (scene->node_count > 0) {
        for (uint32_t ni = 0; ni < scene->node_count; ++ni) {
            const tessera_scene_node *n = &scene->nodes[ni];
            if (n->mesh_first < 0 || n->mesh_prim_count <= 0)
                continue;
            for (int k = n->mesh_first; k < n->mesh_first + n->mesh_prim_count; ++k) {
                if ((uint32_t)k >= scene->prim_count)
                    break;
                const tessera_scene_primitive *p = &scene->prims[k];
                expand_world_aabb(n->world, p->aabb_min, p->aabb_max, &wmin, &wmax);
                any = true;
            }
        }
    } else {
        optics_gfx_mat4 id = optics_gfx_mat4_identity();
        for (uint32_t k = 0; k < scene->prim_count; ++k) {
            expand_world_aabb(id, scene->prims[k].aabb_min, scene->prims[k].aabb_max, &wmin, &wmax);
            any = true;
        }
    }
    if (!any)
        return false;
    *out_min = wmin;
    *out_max = wmax;
    return true;
}

static tessera_scene_material *primitive_material(const tessera_scene *scene,
                                               const tessera_scene_primitive *primitive,
                                               tessera_scene_material *override) {
    if (override)
        return override;
    int index = primitive->material_index;
    if (index >= 0 && (uint32_t)index < scene->material_count && scene->materials[index])
        return scene->materials[index];
    return scene->fallback_material;
}

static void draw_primitive(optics_gfx_frame *frame, const tessera_scene_camera *cam,
                           optics_gfx_mat4 world, const tessera_scene_primitive *primitive,
                           tessera_scene_material *material, const tessera_scene_light *light,
                           const tessera_scene_skin *skin) {
    if (!primitive->mesh || !material)
        return;
    if (skin && skin->joint_count > 0) {
        /* Skin palettes already produce model/world-space positions, so the
         * draw world is identity (world * inverse(world) form). */
        tessera_scene_draw_mesh(frame, &(tessera_scene_draw_mesh_opts){
                                              .camera = cam,
                                              .world = optics_gfx_mat4_identity(),
                                              .mesh = primitive->mesh,
                                              .material = material,
                                              .light = light,
                                              .joint_matrices = skin->palette,
                                              .joint_count = skin->joint_count,
                                          });
    } else {
        tessera_scene_draw_mesh(frame, &(tessera_scene_draw_mesh_opts){
                                              .camera = cam,
                                              .world = world,
                                              .mesh = primitive->mesh,
                                              .material = material,
                                              .light = light,
                                          });
    }
}

TESSERA_SCENE_API void tessera_scene_draw(optics_gfx_frame *frame,
                                        const tessera_scene_draw_opts *opts) {
    if (!frame || !opts || !opts->camera || !opts->scene)
        return;
    const tessera_scene_camera *cam = opts->camera;
    const tessera_scene *scene = opts->scene;

    /* glTF alpha semantics require all depth-writing OPAQUE/MASK draws before
     * depth-reading, non-writing BLEND draws. Source order remains stable
     * within each phase for layered avatar materials. */
    for (int blend_phase = 0; blend_phase < 2; ++blend_phase) {
        for (uint32_t ni = 0; ni < scene->node_count; ++ni) {
            const tessera_scene_node *n = &scene->nodes[ni];
            if (n->mesh_first < 0 || n->mesh_prim_count <= 0)
                continue;
            for (int k = n->mesh_first; k < n->mesh_first + n->mesh_prim_count; ++k) {
                if ((uint32_t)k >= scene->prim_count)
                    break;
                const tessera_scene_primitive *p = &scene->prims[k];
                tessera_scene_material *material = primitive_material(scene, p, opts->material);
                bool blend = material && tessera_scene_material_get_alpha_mode(material) ==
                                             TESSERA_SCENE_MATERIAL_ALPHA_BLEND;
                if (!material || blend != (blend_phase != 0))
                    continue;
                const tessera_scene_skin *skin =
                    n->skin >= 0 && (uint32_t)n->skin < scene->skin_count ? &scene->skins[n->skin]
                                                                          : nullptr;
                draw_primitive(frame, cam, n->world, p, material, opts->light, skin);
            }
        }

        /* A scene with no nodes (mesh-only file) still has primitives. */
        if (scene->node_count == 0) {
            optics_gfx_mat4 identity = optics_gfx_mat4_identity();
            for (uint32_t k = 0; k < scene->prim_count; ++k) {
                const tessera_scene_primitive *p = &scene->prims[k];
                tessera_scene_material *material = primitive_material(scene, p, opts->material);
                bool blend = material && tessera_scene_material_get_alpha_mode(material) ==
                                             TESSERA_SCENE_MATERIAL_ALPHA_BLEND;
                if (material && blend == (blend_phase != 0))
                    draw_primitive(frame, cam, identity, p, material, opts->light, nullptr);
            }
        }
    }
}
