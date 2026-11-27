/* glTF animation sampling and VRMC_vrm_animation humanoid retargeting. */
#include "internal.h"

#include <float.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

static const char *const human_bone_names[SG_HUMAN_BONE_COUNT] = {
    "hips",
    "spine",
    "chest",
    "upperChest",
    "neck",
    "head",
    "leftEye",
    "rightEye",
    "jaw",
    "leftUpperLeg",
    "leftLowerLeg",
    "leftFoot",
    "leftToes",
    "rightUpperLeg",
    "rightLowerLeg",
    "rightFoot",
    "rightToes",
    "leftShoulder",
    "leftUpperArm",
    "leftLowerArm",
    "leftHand",
    "rightShoulder",
    "rightUpperArm",
    "rightLowerArm",
    "rightHand",
    "leftThumbMetacarpal",
    "leftThumbProximal",
    "leftThumbDistal",
    "leftIndexProximal",
    "leftIndexIntermediate",
    "leftIndexDistal",
    "leftMiddleProximal",
    "leftMiddleIntermediate",
    "leftMiddleDistal",
    "leftRingProximal",
    "leftRingIntermediate",
    "leftRingDistal",
    "leftLittleProximal",
    "leftLittleIntermediate",
    "leftLittleDistal",
    "rightThumbMetacarpal",
    "rightThumbProximal",
    "rightThumbDistal",
    "rightIndexProximal",
    "rightIndexIntermediate",
    "rightIndexDistal",
    "rightMiddleProximal",
    "rightMiddleIntermediate",
    "rightMiddleDistal",
    "rightRingProximal",
    "rightRingIntermediate",
    "rightRingDistal",
    "rightLittleProximal",
    "rightLittleIntermediate",
    "rightLittleDistal",
};

int sg_human_bone_index(const char *name, bool legacy_vrm0) {
    if (!name)
        return -1;
    /* VRM 0.x called the three thumb bones proximal/intermediate/distal;
     * VRM 1.0 renamed the first two metacarpal/proximal. */
    if (legacy_vrm0) {
        if (strcmp(name, "leftThumbProximal") == 0)
            return 25;
        if (strcmp(name, "leftThumbIntermediate") == 0)
            return 26;
        if (strcmp(name, "rightThumbProximal") == 0)
            return 40;
        if (strcmp(name, "rightThumbIntermediate") == 0)
            return 41;
    }
    for (int i = 0; i < SG_HUMAN_BONE_COUNT; ++i)
        if (strcmp(name, human_bone_names[i]) == 0)
            return i;
    return -1;
}

void sg_read_humanoid(const jv *root, const char *extension_name, bool legacy_vrm0,
                      int out_bones[SG_HUMAN_BONE_COUNT]) {
    const jv *extension = jv_obj_get(jv_obj_get(root, "extensions"), extension_name);
    const jv *bones = jv_obj_get(jv_obj_get(extension, "humanoid"), "humanBones");
    if (!bones)
        return;
    if (legacy_vrm0 && bones->kind == J_ARR) {
        for (size_t i = 0; i < bones->arr.count; ++i) {
            const jv *bone = jv_arr_at(bones, i);
            const jv *name = jv_obj_get(bone, "bone");
            int index =
                name && name->kind == J_STR ? sg_human_bone_index(name->str.data, true) : -1;
            int node = (int)jv_num(jv_obj_get(bone, "node"), -1);
            if (index >= 0)
                out_bones[index] = node;
        }
    } else if (bones->kind == J_OBJ) {
        for (int i = 0; i < SG_HUMAN_BONE_COUNT; ++i) {
            const jv *bone = jv_obj_get(bones, human_bone_names[i]);
            if (bone)
                out_bones[i] = (int)jv_num(jv_obj_get(bone, "node"), -1);
        }
    }
}

static optics_gfx_quat quat_inverse(optics_gfx_quat q) {
    q = optics_gfx_quat_normalize(q);
    return (optics_gfx_quat){-q.x, -q.y, -q.z, q.w};
}

static optics_gfx_quat quat_from(const float *v) {
    return (optics_gfx_quat){v[0], v[1], v[2], v[3]};
}

static void quat_to(float *v, optics_gfx_quat q, bool normalize) {
    if (normalize)
        q = optics_gfx_quat_normalize(q);
    v[0] = q.x;
    v[1] = q.y;
    v[2] = q.z;
    v[3] = q.w;
}

static optics_gfx_quat retarget_rotation(const tessera_scene_node *source,
                                         const tessera_scene_node *target, optics_gfx_quat animated,
                                         bool normalize) {
    /* VRM Animation pose compatibility, via NormalizedLocalRotation:
     * N = Ws * inv(Ls) * A * inv(Ws)
     * B = Lt * inv(Wt) * N * Wt */
    optics_gfx_quat n = optics_gfx_quat_multiply(
        optics_gfx_quat_multiply(optics_gfx_quat_multiply(source->rest_world_rotation,
                                                          quat_inverse(source->rest_rotation)),
                                 animated),
        quat_inverse(source->rest_world_rotation));
    optics_gfx_quat result = optics_gfx_quat_multiply(
        optics_gfx_quat_multiply(
            optics_gfx_quat_multiply(target->rest_rotation,
                                     quat_inverse(target->rest_world_rotation)),
            n),
        target->rest_world_rotation);
    return normalize ? optics_gfx_quat_normalize(result) : result;
}

static int find_target_by_name(const tessera_scene *target, const char *name) {
    if (!name)
        return -1;
    for (uint32_t i = 0; i < target->node_count; ++i)
        if (target->nodes[i].name && strcmp(target->nodes[i].name, name) == 0)
            return (int)i;
    return -1;
}

static void free_nodes(tessera_scene_node *nodes, uint32_t count) {
    if (nodes)
        for (uint32_t i = 0; i < count; ++i)
            free(nodes[i].name);
    free(nodes);
}

static void free_animation(tessera_scene_animation *animation) {
    if (!animation)
        return;
    for (uint32_t i = 0; i < animation->channel_count; ++i) {
        free(animation->channels[i].times);
        free(animation->channels[i].values);
    }
    free(animation->channels);
    tessera_scene_release((tessera_scene *)animation->target);
    free(animation);
}

static bool copy_accessor_floats(const jv *root, const sg_glb *glb, int accessor,
                                 int expected_components, float **out, uint32_t *out_count) {
    int comp = 0, comps = 0;
    size_t count = 0, stride = 0;
    bool normalized = false;
    const uint8_t *data = sg_accessor_data(root, accessor, glb->bin, glb->bin_len, &comp, &comps,
                                           &count, &stride, &normalized);
    if (!data || comp != SG_CT_FLOAT || comps != expected_components || count == 0 ||
        count > UINT32_MAX || count > SIZE_MAX / ((size_t)comps * sizeof(float)))
        return false;
    float *values = malloc(count * (size_t)comps * sizeof(float));
    if (!values)
        return false;
    for (size_t i = 0; i < count; ++i)
        sg_read_floats(data + i * stride, comp, comps, false, values + i * (size_t)comps);
    *out = values;
    *out_count = (uint32_t)count;
    return true;
}

static int source_bone_for_node(const int bones[SG_HUMAN_BONE_COUNT], int node) {
    for (int i = 0; i < SG_HUMAN_BONE_COUNT; ++i)
        if (bones[i] == node)
            return i;
    return -1;
}

static void retarget_channel_values(tessera_scene_animation_channel *channel,
                                    const tessera_scene_node *source,
                                    const tessera_scene_node *target, bool humanoid, bool hips,
                                    float translation_scale) {
    uint32_t samples =
        channel->interpolation == SG_INTERP_CUBIC ? channel->key_count * 3u : channel->key_count;
    if (channel->path == SG_ANIM_ROTATION) {
        for (uint32_t i = 0; i < samples; ++i) {
            bool is_value = channel->interpolation != SG_INTERP_CUBIC || i % 3u == 1u;
            optics_gfx_quat q = quat_from(channel->values + (size_t)i * 4u);
            if (humanoid)
                q = retarget_rotation(source, target, q, is_value);
            quat_to(channel->values + (size_t)i * 4u, q, is_value);
        }
    } else if (channel->path == SG_ANIM_TRANSLATION && humanoid && hips) {
        for (uint32_t i = 0; i < samples; ++i) {
            float *v = channel->values + (size_t)i * 3u;
            bool is_value = channel->interpolation != SG_INTERP_CUBIC || i % 3u == 1u;
            if (is_value) {
                v[0] = target->rest_translation.x +
                       (v[0] - source->rest_translation.x) * translation_scale;
                v[1] = target->rest_translation.y +
                       (v[1] - source->rest_translation.y) * translation_scale;
                v[2] = target->rest_translation.z +
                       (v[2] - source->rest_translation.z) * translation_scale;
            } else {
                v[0] *= translation_scale;
                v[1] *= translation_scale;
                v[2] *= translation_scale;
            }
        }
    }
}

optics_gfx_result sg_parse_animation_glb(const tessera_scene *target, const void *bytes, size_t len,
                                         tessera_scene_animation **out) {
    if (!target || !bytes || !out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *out = nullptr;
    sg_glb glb;
    optics_gfx_result result = sg_glb_parse(bytes, len, &glb);
    if (result != OPTICS_GFX_OK)
        return result;

    /* Declared up front and NULL/zero-initialised so the single `fail`
     * block at the bottom can release them from any exit (the repo's
     * standard single-exit discipline). */
    optics_gfx_result r = OPTICS_GFX_OK;
    jv *root = nullptr;
    tessera_scene_node *source_nodes = nullptr;
    uint32_t source_node_count = 0;
    tessera_scene_animation *animation = nullptr;
    /* The retarget source scratch scene. Declared here so early `goto fail`
     * exits (before it is populated) can still release its cached world
     * order; zero-init leaves `world_order` NULL, which `free` accepts. */
    tessera_scene source_scene = {};

    size_t error_offset = 0;
    root = jv_parse((const char *)glb.json, glb.json_len, &error_offset);
    if (!root || root->kind != J_OBJ) {
        r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
        goto fail;
    }

    const jv *nodes_json = jv_obj_get(root, "nodes");
    if (nodes_json && nodes_json->kind == J_ARR && nodes_json->arr.count > UINT32_MAX) {
        r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
        goto fail;
    }
    source_node_count =
        nodes_json && nodes_json->kind == J_ARR ? (uint32_t)nodes_json->arr.count : 0;
    source_nodes = source_node_count ? calloc(source_node_count, sizeof(*source_nodes)) : nullptr;
    if (source_node_count && !source_nodes) {
        r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        goto fail;
    }
    for (uint32_t i = 0; i < source_node_count; ++i)
        sg_read_node(jv_arr_at(nodes_json, i), &source_nodes[i]);
    for (uint32_t i = 0; i < source_node_count; ++i) {
        const jv *children = jv_obj_get(jv_arr_at(nodes_json, i), "children");
        if (!children || children->kind != J_ARR)
            continue;
        for (size_t j = 0; j < children->arr.count; ++j) {
            int child = (int)jv_num(jv_arr_at(children, j), -1);
            if (child >= 0 && (uint32_t)child < source_node_count)
                source_nodes[child].parent = (int)i;
        }
    }
    source_scene = (tessera_scene){.nodes = source_nodes, .node_count = source_node_count};
    sg_update_worlds(&source_scene);
    sg_update_rest_world_rotations(&source_scene);
    int source_bones[SG_HUMAN_BONE_COUNT];
    for (int i = 0; i < SG_HUMAN_BONE_COUNT; ++i)
        source_bones[i] = -1;
    const jv *extensions = jv_obj_get(root, "extensions");
    bool vrma = jv_obj_get(extensions, "VRMC_vrm_animation") != nullptr;
    if (vrma)
        sg_read_humanoid(root, "VRMC_vrm_animation", false, source_bones);

    const jv *animations = jv_obj_get(root, "animations");
    const jv *animation_json = jv_arr_at(animations, 0);
    const jv *channels_json = jv_obj_get(animation_json, "channels");
    const jv *samplers_json = jv_obj_get(animation_json, "samplers");
    if (!animation_json || !channels_json || channels_json->kind != J_ARR || !samplers_json ||
        samplers_json->kind != J_ARR) {
        r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
        goto fail;
    }
    if (channels_json->arr.count > UINT32_MAX) {
        r = OPTICS_GFX_ERROR_INVALID_ARGUMENT;
        goto fail;
    }

    animation = calloc(1, sizeof(*animation));
    if (!animation) {
        r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        goto fail;
    }
    animation->channels = calloc(channels_json->arr.count, sizeof(*animation->channels));
    if (channels_json->arr.count && !animation->channels) {
        r = OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        goto fail;
    }
    animation->refcount = 1;
    /* A clip is permanently bound to this scene's node indices. Holding a
     * reference makes the standalone C handle safe regardless of release
     * order (the Rust wrapper likewise needs no self-referential lifetime). */
    animation->target = tessera_scene_retain((tessera_scene *)target);

    for (size_t ci = 0; ci < channels_json->arr.count; ++ci) {
        const jv *channel_json = jv_arr_at(channels_json, ci);
        const jv *target_json = jv_obj_get(channel_json, "target");
        int source_node = (int)jv_num(jv_obj_get(target_json, "node"), -1);
        const jv *path_json = jv_obj_get(target_json, "path");
        if (source_node < 0 || (uint32_t)source_node >= source_node_count || !path_json ||
            path_json->kind != J_STR)
            continue;
        tessera_scene_animation_path path;
        uint8_t components;
        if (strcmp(path_json->str.data, "translation") == 0) {
            path = SG_ANIM_TRANSLATION;
            components = 3;
        } else if (strcmp(path_json->str.data, "rotation") == 0) {
            path = SG_ANIM_ROTATION;
            components = 4;
        } else if (strcmp(path_json->str.data, "scale") == 0) {
            path = SG_ANIM_SCALE;
            components = 3;
        } else {
            continue; /* weights/morph animation is outside this renderer */
        }

        int bone = source_bone_for_node(source_bones, source_node);
        if (vrma &&
            (bone < 0 || path == SG_ANIM_SCALE || (path == SG_ANIM_TRANSLATION && bone != 0)))
            continue;
        int target_node = bone >= 0 ? target->human_bones[bone]
                                    : find_target_by_name(target, source_nodes[source_node].name);
        if (target_node < 0 || (uint32_t)target_node >= target->node_count)
            continue;

        int sampler_index = (int)jv_num(jv_obj_get(channel_json, "sampler"), -1);
        const jv *sampler = jv_arr_at(samplers_json, (size_t)sampler_index);
        if (!sampler)
            continue;
        tessera_scene_interpolation interpolation = SG_INTERP_LINEAR;
        const jv *interpolation_json = jv_obj_get(sampler, "interpolation");
        if (interpolation_json && interpolation_json->kind == J_STR) {
            if (strcmp(interpolation_json->str.data, "STEP") == 0)
                interpolation = SG_INTERP_STEP;
            else if (strcmp(interpolation_json->str.data, "CUBICSPLINE") == 0)
                interpolation = SG_INTERP_CUBIC;
            else if (strcmp(interpolation_json->str.data, "LINEAR") != 0)
                continue;
        }

        tessera_scene_animation_channel channel = {
            .target_node = target_node,
            .path = path,
            .interpolation = interpolation,
            .components = components,
        };
        uint32_t output_count = 0;
        if (!copy_accessor_floats(root, &glb, (int)jv_num(jv_obj_get(sampler, "input"), -1), 1,
                                  &channel.times, &channel.key_count) ||
            !copy_accessor_floats(root, &glb, (int)jv_num(jv_obj_get(sampler, "output"), -1),
                                  components, &channel.values, &output_count)) {
            free(channel.times);
            free(channel.values);
            continue;
        }
        if (interpolation == SG_INTERP_CUBIC && channel.key_count > UINT32_MAX / 3u) {
            free(channel.times);
            free(channel.values);
            continue;
        }
        uint32_t expected =
            interpolation == SG_INTERP_CUBIC ? channel.key_count * 3u : channel.key_count;
        if (output_count != expected) {
            free(channel.times);
            free(channel.values);
            continue;
        }
        bool valid_times = true;
        for (uint32_t i = 0; i < channel.key_count; ++i) {
            if (!isfinite(channel.times[i]) || channel.times[i] < 0.0f ||
                (i > 0 && channel.times[i] < channel.times[i - 1])) {
                valid_times = false;
                break;
            }
        }
        if (!valid_times) {
            free(channel.times);
            free(channel.values);
            continue;
        }
        float scale = 1.0f;
        if (bone == 0) {
            float source_hips_y = fabsf(source_nodes[source_node].world.m[13]);
            float target_hips_y = fabsf(target->nodes[target_node].world.m[13]);
            if (source_hips_y > FLT_EPSILON && target_hips_y > FLT_EPSILON)
                scale = target_hips_y / source_hips_y;
        }
        retarget_channel_values(&channel, &source_nodes[source_node], &target->nodes[target_node],
                                bone >= 0, bone == 0, scale);
        float last = channel.times[channel.key_count - 1u];
        if (last > animation->duration)
            animation->duration = last;
        animation->channels[animation->channel_count++] = channel;
    }

    if (animation->channel_count == 0) {
        r = OPTICS_GFX_ERROR_UNSUPPORTED;
        goto fail;
    }
    /* Success: the source-scene scratch and the JSON tree are no longer
     * needed; ownership of the animation passes to the caller. */
    free(source_scene.world_order);
    free_nodes(source_nodes, source_node_count);
    jv_free(root);
    *out = animation;
    return OPTICS_GFX_OK;

fail:
    free_animation(animation);
    free(source_scene.world_order);
    free_nodes(source_nodes, source_node_count);
    jv_free(root);
    return r;
}

static void sample_channel(const tessera_scene_animation_channel *channel, float time, float *out) {
    uint32_t last = channel->key_count - 1u;
    uint32_t value_stride = channel->interpolation == SG_INTERP_CUBIC ? 3u : 1u;
    if (time <= channel->times[0] || channel->key_count == 1u) {
        uint32_t index = channel->interpolation == SG_INTERP_CUBIC ? 1u : 0u;
        memcpy(out, channel->values + (size_t)index * channel->components,
               channel->components * sizeof(float));
        return;
    }
    if (time >= channel->times[last]) {
        uint32_t index =
            last * value_stride + (channel->interpolation == SG_INTERP_CUBIC ? 1u : 0u);
        memcpy(out, channel->values + (size_t)index * channel->components,
               channel->components * sizeof(float));
        return;
    }
    uint32_t lo = 0, hi = last;
    while (lo + 1u < hi) {
        uint32_t mid = lo + (hi - lo) / 2u;
        if (channel->times[mid] <= time)
            lo = mid;
        else
            hi = mid;
    }
    float dt = channel->times[hi] - channel->times[lo];
    float u = dt > FLT_EPSILON ? (time - channel->times[lo]) / dt : 0.0f;
    if (channel->interpolation == SG_INTERP_STEP) {
        memcpy(out, channel->values + (size_t)lo * channel->components,
               channel->components * sizeof(float));
    } else if (channel->interpolation == SG_INTERP_CUBIC) {
        float u2 = u * u, u3 = u2 * u;
        float h00 = 2.0f * u3 - 3.0f * u2 + 1.0f;
        float h10 = u3 - 2.0f * u2 + u;
        float h01 = -2.0f * u3 + 3.0f * u2;
        float h11 = u3 - u2;
        const float *p0 = channel->values + (size_t)(lo * 3u + 1u) * channel->components;
        const float *m0 = channel->values + (size_t)(lo * 3u + 2u) * channel->components;
        const float *p1 = channel->values + (size_t)(hi * 3u + 1u) * channel->components;
        const float *m1 = channel->values + (size_t)(hi * 3u) * channel->components;
        for (uint8_t i = 0; i < channel->components; ++i)
            out[i] = h00 * p0[i] + h10 * dt * m0[i] + h01 * p1[i] + h11 * dt * m1[i];
        if (channel->path == SG_ANIM_ROTATION)
            quat_to(out, quat_from(out), true);
    } else if (channel->path == SG_ANIM_ROTATION) {
        optics_gfx_quat a = quat_from(channel->values + (size_t)lo * 4u);
        optics_gfx_quat b = quat_from(channel->values + (size_t)hi * 4u);
        quat_to(out, optics_gfx_quat_slerp(a, b, u), true);
    } else {
        const float *a = channel->values + (size_t)lo * channel->components;
        const float *b = channel->values + (size_t)hi * channel->components;
        for (uint8_t i = 0; i < channel->components; ++i)
            out[i] = a[i] + (b[i] - a[i]) * u;
    }
}

void tessera_scene_reset_pose(tessera_scene *scene) {
    if (!scene)
        return;
    for (uint32_t i = 0; i < scene->node_count; ++i) {
        scene->nodes[i].translation = scene->nodes[i].rest_translation;
        scene->nodes[i].rotation = scene->nodes[i].rest_rotation;
        scene->nodes[i].scale = scene->nodes[i].rest_scale;
    }
    sg_update_worlds(scene);
    sg_update_skin_palettes(scene);
}

optics_gfx_result tessera_scene_apply_animation(tessera_scene *scene,
                                               const tessera_scene_apply_animation_opts *opts) {
    if (!scene || !opts || !opts->animation || opts->animation->target != scene ||
        !isfinite(opts->time_seconds))
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    const tessera_scene_animation *animation = opts->animation;
    float time_seconds = opts->time_seconds;
    bool loop = opts->loop;
    tessera_scene_reset_pose(scene);
    float time = fmaxf(time_seconds, 0.0f);
    if (animation->duration > FLT_EPSILON)
        time = loop ? fmodf(time, animation->duration) : fminf(time, animation->duration);
    for (uint32_t i = 0; i < animation->channel_count; ++i) {
        const tessera_scene_animation_channel *channel = &animation->channels[i];
        if (channel->target_node < 0 || (uint32_t)channel->target_node >= scene->node_count)
            continue;
        float value[4] = {0, 0, 0, 1};
        sample_channel(channel, time, value);
        tessera_scene_node *node = &scene->nodes[channel->target_node];
        if (channel->path == SG_ANIM_TRANSLATION)
            node->translation = optics_gfx_vec3_make(value[0], value[1], value[2]);
        else if (channel->path == SG_ANIM_ROTATION)
            node->rotation = optics_gfx_quat_normalize(quat_from(value));
        else
            node->scale = optics_gfx_vec3_make(value[0], value[1], value[2]);
    }
    sg_update_worlds(scene);
    sg_update_skin_palettes(scene);
    return OPTICS_GFX_OK;
}

optics_gfx_result
tessera_scene_load_animation_glb(const tessera_scene *target,
                                const tessera_scene_load_animation_glb_opts *opts) {
    if (!opts || !opts->bytes || !opts->out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    return sg_parse_animation_glb(target, opts->bytes, opts->byte_count, opts->out);
}

tessera_scene_animation *tessera_scene_animation_retain(tessera_scene_animation *animation) {
    if (animation)
        animation->refcount++;
    return animation;
}

void tessera_scene_animation_release(tessera_scene_animation *animation) {
    if (animation && --animation->refcount == 0)
        free_animation(animation);
}

float tessera_scene_animation_duration(const tessera_scene_animation *animation) {
    return animation ? animation->duration : 0.0f;
}

uint32_t tessera_scene_animation_channel_count(const tessera_scene_animation *animation) {
    return animation ? animation->channel_count : 0;
}
