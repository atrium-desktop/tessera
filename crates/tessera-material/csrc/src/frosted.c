/*
 * Classic non-distorting frosted glass material implementation.
 */

#include "material_frame.h"
#include "material_observe.h"
#include <tessera/material/frosted.h>

#include <optics/gfx/compute.h>
#include <optics/gfx/profile.h>
#include <optics/gfx/vulkan.h>

#include <math.h>
#include <stdalign.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

#define FROSTED_MAX_GROUPS 64u

typedef struct frosted_push {
    uint32_t input_handle;
    uint32_t blurred_handle;
    uint32_t sampler_handle;
    uint32_t output_handle;
    uint32_t width;
    uint32_t height;
    uint32_t origin_x;
    uint32_t origin_y;
    float bounds[4];
    float corner_radius;
    float opacity;
    float saturation;
    float tint_strength;
    float noise_intensity;
    float shadow_alpha;
    float shadow_blur;
    float shadow_offset_y;
    uint32_t tint_color;
    uint32_t group_width;
    uint32_t group_height;
    float slab_origin_x;
    float slab_origin_y;
    float capture_width;
    float capture_height;
} frosted_push;

static_assert(sizeof(frosted_push) == 108, "frosted_push size mismatch");

typedef struct frosted_filter_slot {
    tessera_material__slab slab; /* the output, on the slab ladder (material_frame.h) */
    bool initialized;
    uint32_t slab_x;
    uint32_t slab_y;
    uint32_t capture_width;
    uint32_t capture_height;
    uint32_t previous_count;
    tessera_material__rect previous[FROSTED_MAX_GROUPS];
} frosted_filter_slot;

struct tessera_material_frosted_filter {
    atomic_uint ref_count;
    optics_gfx_device *device;
    optics_gfx_compute_pipeline *frosted_pipelines[2];
    optics_gfx_compute_pipeline *clear_pipelines[2];
    frosted_filter_slot slots[OPTICS_GFX_MAX_FRAMES_IN_FLIGHT];
};

optics_gfx_result tessera_material_frosted_filter_create(optics_gfx_device *device,
                                                        tessera_material_frosted_filter **out) {
    if (!device || !out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *out = nullptr;
    /* Caller's allocator, attributed to material (ADR-0135). */
    tessera_material_frosted_filter *filter = nullptr;
    if (optics_gfx_device_alloc(device,
                                &(optics_gfx_device_alloc_opts){
                                    .type = OPTICS_GFX_TYPE_DEVICE_ALLOC_OPTS,
                                    .bytes = sizeof(*filter),
                                    .owner = TESSERA_MATERIAL_OBSERVE_OWNER,
                                    .kind = TESSERA_MATERIAL_OBSERVE_KIND_FROSTED,
                                    .scope = OPTICS_GFX_SCOPE_DEVICE,
                                },
                                (void **)&filter) != OPTICS_GFX_OK)
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    atomic_init(&filter->ref_count, 1u);
    filter->device = optics_gfx_device_retain(device);
    *out = filter;
    return OPTICS_GFX_OK;
}

tessera_material_frosted_filter *
tessera_material_frosted_filter_retain(tessera_material_frosted_filter *filter) {
    if (filter)
        atomic_fetch_add_explicit(&filter->ref_count, 1u, memory_order_relaxed);
    return filter;
}

void tessera_material_frosted_filter_release(tessera_material_frosted_filter *filter) {
    if (!filter)
        return;
    if (atomic_fetch_sub_explicit(&filter->ref_count, 1u, memory_order_acq_rel) != 1u)
        return;
    optics_gfx_device *device = filter->device;
    for (uint32_t i = 0; i < OPTICS_GFX_MAX_FRAMES_IN_FLIGHT; ++i) {
        tessera_material__slab_release(&filter->slots[i].slab);
    }
    for (int i = 0; i < 2; ++i) {
        if (filter->frosted_pipelines[i])
            optics_gfx_compute_pipeline_release(filter->frosted_pipelines[i]);
        if (filter->clear_pipelines[i])
            optics_gfx_compute_pipeline_release(filter->clear_pipelines[i]);
    }
    optics_gfx_device_free(device, &(optics_gfx_device_free_opts){
                                       .type = OPTICS_GFX_TYPE_DEVICE_FREE_OPTS,
                                       .ptr = filter,
                                       .owner = TESSERA_MATERIAL_OBSERVE_OWNER,
                                       .kind = TESSERA_MATERIAL_OBSERVE_KIND_FROSTED,
                                       .scope = OPTICS_GFX_SCOPE_DEVICE,
                                   });
    optics_gfx_device_release(device);
}

static optics_gfx_result frosted_ensure_slot(tessera_material_frosted_filter *filter, uint32_t index,
                                             const optics_gfx_image *input, uint32_t footprint_w,
                                             uint32_t footprint_h) {
    frosted_filter_slot *slot = &filter->slots[index];
    bool recreated = false;
    optics_gfx_result r = tessera_material__slab_ensure(
        filter->device, &slot->slab,
        &(tessera_material__slab_opts){
            .input = input, .footprint_width = footprint_w, .footprint_height = footprint_h},
        &recreated);
    if (recreated) /* a new image starts with no previous regions to clear */
        *slot = (frosted_filter_slot){.slab = slot->slab};
    return r;
}

static optics_gfx_result frosted_ensure_pipelines(tessera_material_frosted_filter *filter,
                                                  bool is16f, bool need_clear, bool need_frosted) {
    const int cls = is16f ? 1 : 0;
    optics_gfx_result r = OPTICS_GFX_OK;
    if (need_clear)
        r = tessera_material__pipeline_ensure(
            filter->device, &filter->clear_pipelines[cls],
            &(tessera_material__pipeline_opts){
                .shader = TESSERA_MATERIAL__SHADER_STORAGE_CLEAR,
                .is16f = is16f,
                .push_constant_bytes = tessera_material__storage_clear_push_bytes(),
            });
    if (r == OPTICS_GFX_OK && need_frosted)
        r = tessera_material__pipeline_ensure(filter->device, &filter->frosted_pipelines[cls],
                                             &(tessera_material__pipeline_opts){
                                                 .shader = TESSERA_MATERIAL__SHADER_FROSTED,
                                                 .is16f = is16f,
                                                 .push_constant_bytes = sizeof(frosted_push),
                                             });
    return r;
}

optics_gfx_result
tessera_material_frosted_filter_warmup(tessera_material_frosted_filter *filter,
                                      const tessera_material_frosted_warmup_opts *opts) {
    if (!filter)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;

    bool is16f = false;
    if (opts) {
        if (opts->format == OPTICS_GFX_FORMAT_RGBA16_SFLOAT)
            is16f = true;
    }

    return frosted_ensure_pipelines(filter, is16f, true, true);
}

static bool frosted_group_dispatch_bounds(const tessera_material_frosted_group *group,
                                          uint32_t image_width, uint32_t image_height,
                                          tessera_material__rect *out) {
    if (!group || !out || image_width == 0 || image_height == 0)
        return false;
    float pad = fmaxf(group->shadow_blur * 2.0f, 2.0f);
    int64_t x0 = (int64_t)floorf(group->shape.bounds.x - pad);
    int64_t y0 = (int64_t)floorf(group->shape.bounds.y - pad);
    int64_t x1 = (int64_t)ceilf(group->shape.bounds.x + group->shape.bounds.w + pad);
    int64_t y1 = (int64_t)ceilf(group->shape.bounds.y + group->shape.bounds.h + pad +
                                fmaxf(group->shadow_offset_y, 0.0f));

    x0 = x0 < 0 ? 0 : x0;
    y0 = y0 < 0 ? 0 : y0;
    x1 = x1 > (int64_t)image_width ? (int64_t)image_width : x1;
    y1 = y1 > (int64_t)image_height ? (int64_t)image_height : y1;
    if (x1 <= x0 || y1 <= y0)
        return false;

    out->x = (uint32_t)x0;
    out->y = (uint32_t)y0;
    out->width = (uint32_t)(x1 - x0);
    out->height = (uint32_t)(y1 - y0);
    return true;
}

/* Everything one frosted submission needs to replay its dispatches later
 * (ADR-0147). Copied into the frame's plan arena, so the descriptor and group
 * array may live on the caller's stack and die when apply returns. */
typedef struct frosted_plan {
    optics_gfx_device *device;
    struct tessera_material_frosted_filter *filter;
    optics_gfx_image *input;
    optics_gfx_image *blurred;
    optics_gfx_image *output;
    uint32_t slot_index;
    bool is16f;
    uint32_t group_count;
    float opacity;
    float saturation;
    float tint_strength;
    float noise_intensity;
    /* Precomputed dispatch geometry, so the replay does no bounds work and
     * cannot disagree with the frames already recorded. */
    uint32_t slab_x;
    uint32_t slab_y;
    uint32_t current_count;
    tessera_material__rect current[FROSTED_MAX_GROUPS];
    tessera_material__rect previous[FROSTED_MAX_GROUPS];
    uint32_t previous_count;
    bool clear_full;
    bool slab_moved;
    tessera_material_frosted_group groups[FROSTED_MAX_GROUPS];
} frosted_plan;

static void frosted_plan_record(void *raw, VkCommandBuffer cmd) {
    const frosted_plan *p = raw;
    if (!p || cmd == VK_NULL_HANDLE || !p->filter)
        return;
    tessera_material_frosted_filter *filter = p->filter;
    const int cls = p->is16f ? 1 : 0;
    VkImage out_vk = optics_gfx_image_vk_image(p->output);

    tessera_material__barrier_reuse_to_compute_write(cmd, out_vk);

    /* Full clear on first use or when the slab origin moved; otherwise only
     * what the previous submission drew and what this one will draw. */
    optics_gfx_compute_pipeline *clear = filter->clear_pipelines[cls];
    if (p->clear_full) {
        tessera_material__clear_regions(
            &(tessera_material__clear_opts){.cmd = cmd, .pipeline = clear, .image = p->output});
        if (clear)
            tessera_material__barrier_compute_write_to_read_write(cmd, out_vk);
    } else if (clear) {
        tessera_material__clear_regions(
            &(tessera_material__clear_opts){.cmd = cmd,
                                           .pipeline = clear,
                                           .image = p->output,
                                           .regions = p->previous,
                                           .region_count = p->previous_count});
        tessera_material__clear_regions(
            &(tessera_material__clear_opts){.cmd = cmd,
                                           .pipeline = clear,
                                           .image = p->output,
                                           .regions = p->current,
                                           .region_count = p->current_count});
        /* Order the clears before the material dispatches; with nothing to
         * dispatch, the closing write-to-read barrier orders them instead. */
        if (p->current_count > 0u)
            tessera_material__barrier_compute_write_to_read_write(cmd, out_vk);
    }

    if (p->group_count > 0 && filter->frosted_pipelines[cls]) {
        const uint32_t out_w = (uint32_t)optics_gfx_image_width(p->output);
        const uint32_t out_h = (uint32_t)optics_gfx_image_height(p->output);
        for (uint32_t i = 0; i < p->current_count; ++i) {
            const tessera_material_frosted_group *g = &p->groups[i];
            const tessera_material__rect *reg = &p->current[i];
            frosted_push pc = {
                .input_handle = optics_gfx_image_bindless_handle(p->input),
                .blurred_handle = optics_gfx_image_bindless_handle(p->blurred),
                .sampler_handle = optics_gfx_device_default_sampler_handle(p->device),
                .output_handle = optics_gfx_image_bindless_storage_handle(p->output),
                .width = out_w,
                .height = out_h,
                .origin_x = reg->x,
                .origin_y = reg->y,
                .bounds = {g->shape.bounds.x, g->shape.bounds.y, g->shape.bounds.w,
                           g->shape.bounds.h},
                .corner_radius = g->shape.corner_radius,
                .opacity = g->opacity * p->opacity,
                .saturation = g->saturation >= 0.0f ? g->saturation : p->saturation,
                .tint_strength = g->tint_strength >= 0.0f ? g->tint_strength : p->tint_strength,
                .noise_intensity =
                    g->noise_intensity >= 0.0f ? g->noise_intensity : p->noise_intensity,
                .shadow_alpha = g->shadow_alpha,
                .shadow_blur = g->shadow_blur,
                .shadow_offset_y = g->shadow_offset_y,
                .tint_color = g->tint_color,
                .group_width = reg->width,
                .group_height = reg->height,
                .slab_origin_x = (float)p->slab_x,
                .slab_origin_y = (float)p->slab_y,
                .capture_width = (float)optics_gfx_image_width(p->input),
                .capture_height = (float)optics_gfx_image_height(p->input),
            };
            uint32_t gx = tessera_material_groups(reg->width);
            uint32_t gy = tessera_material_groups(reg->height);
            optics_gfx_compute_dispatch(
                &(optics_gfx_compute_dispatch_opts){.cmd = cmd,
                                                    .pipeline = filter->frosted_pipelines[cls],
                                                    .push_constants = &pc,
                                                    .push_bytes = sizeof(pc),
                                                    .group_x = gx,
                                                    .group_y = gy,
                                                    .group_z = 1});
            if (i + 1 < p->current_count)
                tessera_material__barrier_compute_write_to_read_write(cmd, out_vk);
        }
    }

    tessera_material__barrier_compute_write_to_read(cmd, out_vk);
}

optics_gfx_result
tessera_material_frosted_filter_apply(tessera_material_frosted_filter *filter,
                                     const tessera_material_frosted_filter_apply_opts *opts) {
    if (!filter || !opts || !opts->frame || !opts->desc || !opts->out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    optics_gfx_frame *frame = opts->frame;
    const tessera_material_frosted_desc *desc = opts->desc;
    tessera_material_output *out = opts->out;
    if (desc->type != TESSERA_MATERIAL_TYPE_FROSTED_DESC)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (!desc->input || !desc->blurred_input)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (desc->group_count > FROSTED_MAX_GROUPS)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (desc->group_count > 0 && !desc->groups)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;

    *out = (tessera_material_output){};
    if (optics_gfx_frame_get_state(frame) != OPTICS_GFX_FRAME_STATE_RECORDING ||
        optics_gfx_frame_has_active_pass(frame))
        return OPTICS_GFX_ERROR_INVALID_STATE;
    if (optics_gfx_image_bindless_handle(desc->input) == OPTICS_GFX_BINDLESS_INVALID ||
        optics_gfx_image_bindless_handle(desc->blurred_input) == OPTICS_GFX_BINDLESS_INVALID)
        return OPTICS_GFX_ERROR_UNSUPPORTED;

    /* ADR-0147: the blurred input is normally a deferred optics_gfx_blur_filter
     * output; materialize it so its dispatches precede ours. */
    optics_gfx_result mat = optics_gfx_frame_materialize_image(frame, desc->blurred_input);
    if (mat != OPTICS_GFX_OK)
        return mat;

    const uint32_t capture_width = optics_gfx_image_width(desc->input);
    const uint32_t capture_height = optics_gfx_image_height(desc->input);

    OPTICS_GFX_PROFILE_ZONE(z_frosted, "material.frosted.apply");

    optics_gfx_result r = OPTICS_GFX_OK;
    frosted_filter_slot *slot = &filter->slots[optics_gfx_frame_index(frame)];
    frosted_plan plan = {
        .device = filter->device,
        .filter = filter,
        .input = desc->input,
        .blurred = desc->blurred_input,
        .slot_index = optics_gfx_frame_index(frame),
        .is16f = slot->slab.format == OPTICS_GFX_FORMAT_RGBA16_SFLOAT,
        .group_count = desc->group_count,
        .opacity = desc->opacity,
        .saturation = desc->saturation,
        .tint_strength = desc->tint_strength,
        .noise_intensity = desc->noise_intensity,
    };

    uint32_t footprint_x0 = UINT32_MAX, footprint_y0 = UINT32_MAX;
    uint32_t footprint_x1 = 0u, footprint_y1 = 0u;
    for (uint32_t i = 0; i < desc->group_count; ++i) {
        tessera_material__rect reg;
        if (!frosted_group_dispatch_bounds(&desc->groups[i], capture_width, capture_height, &reg))
            continue;
        plan.groups[plan.current_count] = desc->groups[i];
        plan.current[plan.current_count++] = reg;
        if (reg.x < footprint_x0)
            footprint_x0 = reg.x;
        if (reg.y < footprint_y0)
            footprint_y0 = reg.y;
        if (reg.x + reg.width > footprint_x1)
            footprint_x1 = reg.x + reg.width;
        if (reg.y + reg.height > footprint_y1)
            footprint_y1 = reg.y + reg.height;
    }

    uint32_t footprint_w = 1u;
    uint32_t footprint_h = 1u;
    if (plan.current_count > 0u) {
        footprint_w = footprint_x1 - footprint_x0;
        footprint_h = footprint_y1 - footprint_y0;
    }

    r = frosted_ensure_slot(filter, plan.slot_index, desc->input, footprint_w, footprint_h);
    if (r != OPTICS_GFX_OK)
        return r;
    slot = &filter->slots[plan.slot_index];
    /* The slot image is only known after ensure_slot; the snapshot must carry it
     * because the replay runs long after this frame returns. */
    plan.output = slot->slab.image;
    plan.is16f = slot->slab.format == OPTICS_GFX_FORMAT_RGBA16_SFLOAT;

    const uint32_t slab_x = plan.current_count > 0u ? footprint_x0 : 0u;
    const uint32_t slab_y = plan.current_count > 0u ? footprint_y0 : 0u;
    plan.slab_x = slab_x;
    plan.slab_y = slab_y;
    plan.slab_moved = slot->initialized && (slot->slab_x != slab_x || slot->slab_y != slab_y);
    plan.clear_full = !slot->initialized || plan.slab_moved;
    plan.previous_count = slot->initialized && !plan.slab_moved ? slot->previous_count : 0u;
    if (plan.previous_count > 0u)
        memcpy(plan.previous, slot->previous, plan.previous_count * sizeof(tessera_material__rect));

    /* Regions are expressed relative to the slab origin, which is what the
     * shaders add back. Doing it here means the replay needs no geometry code. */
    for (uint32_t i = 0; i < plan.current_count; ++i) {
        plan.current[i].x -= slab_x;
        plan.current[i].y -= slab_y;
    }
    for (uint32_t i = 0; i < plan.previous_count; ++i) {
        plan.previous[i].x -= slab_x;
        plan.previous[i].y -= slab_y;
    }

    r = frosted_ensure_pipelines(filter, plan.is16f, true, desc->group_count > 0);
    if (r != OPTICS_GFX_OK)
        return r;

    slot->slab_x = slab_x;
    slot->slab_y = slab_y;
    slot->capture_width = capture_width;
    slot->capture_height = capture_height;
    slot->initialized = true;
    slot->previous_count = plan.current_count;
    memcpy(slot->previous, plan.current, plan.current_count * sizeof(tessera_material__rect));

    *out = (tessera_material_output){
        .image = slot->slab.image,
        .origin_x = (float)slot->slab_x,
        .origin_y = (float)slot->slab_y,
        .width = slot->slab.width,
        .height = slot->slab.height,
    };

    /* Defer the dispatches (ADR-0147). The handle above is valid now; the work
     * is replayed only if something requires the output. A plan that cannot take
     * the pass falls back to recording here, which is always correct — pruning is
     * an optimisation, never a correctness dependency. */
    size_t plan_bytes = offsetof(frosted_plan, groups) +
                        sizeof(tessera_material_frosted_group) * (size_t)plan.group_count;
    tessera_material_deferred_pass pass = TESSERA_MATERIAL_DEFERRED_PASS_INIT;
    pass.output = slot->slab.image;
    pass.label = "material.frosted";
    pass.inputs[0] = desc->input;
    pass.inputs[1] = desc->blurred_input;
    pass.input_count = 2;

    /* gfx copies `plan` into its own arena, so this stack snapshot is safe. */
    optics_gfx_deferred_token *token = nullptr;
    r = tessera_material_deferred_register(frame, &pass, frosted_plan_record, &plan, plan_bytes,
                                          &token);
    if (r != OPTICS_GFX_OK) {
        /* Arena or plan exhausted: record eagerly so the frame is still right.
         * Pruning is an optimisation, never a correctness dependency. */
        frosted_plan_record(&plan, optics_gfx_frame_vk_command_buffer(frame));
        token = nullptr;
    }

    /* The host draws this slab in a canvas pass, so it is the terminal consumer
     * and the pass must run. A downstream pass would instead name the token. */
    (void)tessera_material_deferred_require(frame, token);

    OPTICS_GFX_PROFILE_ZONE_END(z_frosted);
    return OPTICS_GFX_OK;
}
