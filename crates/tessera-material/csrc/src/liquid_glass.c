/*
 * Analytic liquid-glass material — the convex-lens model of ADR-0046 with
 * the focus field of ADR-0050.
 *
 * material is a separate library from gfx: this file is implemented entirely
 * on gfx's public API (compute pipelines, compute-writable images,
 * frame-slot queries, the bindless heap, raw Vulkan handle accessors).
 * gfx owns the effect runtime; the material — its shader, the regions
 * policy below, and this dispatch machinery — lives here (ADR-0063).
 *
 * Layout:
 *   - shader bytecode (#embed or generated headers, same idiom as gfx)
 *   - push-constant blocks matching the shaders
 *   - barrier helpers (raw Vulkan over optics_gfx_image_vk_image)
 *   - the frame-slot filter: per-slot output, footprint clearing, dispatch
 */

#include "material_observe.h"
#include <optics/gfx/profile.h>
#include <tessera/material/liquid_glass.h>

#include <optics/gfx/compute.h>
#include <optics/gfx/vulkan.h>

#include "glass_dispatch.h"
#include "regions.h"

uint32_t tessera_material_liquid_glass_max_shapes(void) {
    return LIQUID_GLASS_MAX_SHAPES;
}

#include <math.h>
#include <stdalign.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

/* Push-constant block — must match backdrop_stats.comp exactly. The stats
 * buffer's 64-bit device address rides as two u32s because gfx devices do
 * not guarantee shaderInt64. */
typedef struct backdrop_stats_push {
    uint32_t input_handle;
    uint32_t blurred_handle;
    uint32_t sampler_handle;
    uint32_t width;
    uint32_t height;
    uint32_t rect_x;
    uint32_t rect_y;
    uint32_t rect_width;
    uint32_t rect_height;
    uint32_t group_index;
    uint32_t stats_address_lo;
    uint32_t stats_address_hi;
} backdrop_stats_push;

/* gfx requires every device to accept 160 push-constant bytes, so the
 * material can rely on that budget on any gfx device. */
static_assert(sizeof(backdrop_stats_push) == 48,
              "backdrop_stats_push no longer matches its shader block");

/* Per-slot stats buffer: one tessera_material_backdrop_stat per group. */
#define TESSERA_MATERIAL_STATS_BYTES (LIQUID_GLASS_MAX_GROUPS * 2u * sizeof(float))

/* The material output container follows the input (ADR-0069): 8-bit SDR
 * inputs produce the historic RGBA8 output; 16F working-space inputs a
 * 16F output, so the material composites in linear light. */
typedef struct liquid_glass_filter_slot {
    tessera_material__slab slab; /* the output, on the slab ladder (material_frame.h) */
    bool initialized;
    /* Where this slab sits in capture space. */
    uint32_t slab_x;
    uint32_t slab_y;
    uint32_t capture_width;
    uint32_t capture_height;
    uint32_t previous_count;
    tessera_material__rect previous[LIQUID_GLASS_MAX_GROUPS];
    /* Per-slot backdrop statistics: one HOST_VISIBLE storage buffer written
     * by the stats dispatch and read back OPTICS_GFX_MAX_FRAMES_IN_FLIGHT frames
     * later, once begin_frame has waited this slot's fence. */
    optics_gfx_buffer *stats;
    bool stats_submitted;
    uint32_t stats_group_count;
} liquid_glass_filter_slot;

struct tessera_material_liquid_glass_filter {
    atomic_uint ref_count;
    optics_gfx_device *device; /* retained; slot images hold weak device refs */
    /* Indexed by storage class: [0] rgba8, [1] rgba16f (ADR-0069). */
    optics_gfx_compute_pipeline *glass_pipelines[2]; /* lazily built on first apply */
    optics_gfx_compute_pipeline *clear_pipelines[2];
    optics_gfx_compute_pipeline *stats_pipeline;
    /* Per-body parameters and per-region body id lists, both host-visible and
     * addressed by device address. Not per slot: a filter slot is owned by one
     * frame at a time (its fence has signalled before the slot is reused), and
     * the table is fully written before the submission that reads it. */
    tessera_material_body_table bodies;
    tessera_material_body_ids ids;
    liquid_glass_filter_slot slots[OPTICS_GFX_MAX_FRAMES_IN_FLIGHT];
};

/* ------------------------------------------------------------------ */
/*  Barrier helpers (raw Vulkan over optics_gfx_image_vk_image)             */
/* ------------------------------------------------------------------ */

/* Emit a compute-write → compute-read barrier on `image`, no layout
 * transition (image stays in GENERAL). Makes the output sampleable by any
 * later shader stage. */
/* A reusable frame-slot image may have been sampled by the slot's previous
 * submission. begin_frame waited that slot's fence; this barrier expresses
 * the device-side read/write dependency before the next compute overwrite. */
/* Groups source-over the persistent storage image. Make an earlier group's
 * write visible to both imageLoad and imageStore in the next group; a
 * write-only dependency lets a later shadow replace the body below it
 * instead of compositing over it. */
/* Make a stats-buffer compute write visible to the next submission's
 * compute read/write on the same buffer. Host visibility needs no barrier
 * here: the slot's frame fence plus the HOST_COHERENT mapping cover the
 * CPU readback path. */
/* ------------------------------------------------------------------ */
/*  Filter lifecycle                                                  */
/* ------------------------------------------------------------------ */

optics_gfx_result
tessera_material_liquid_glass_filter_create(optics_gfx_device *device,
                                           tessera_material_liquid_glass_filter **out) {
    if (!device || !out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *out = nullptr;
    /* Caller's allocator, attributed to material (ADR-0135). */
    tessera_material_liquid_glass_filter *filter = nullptr;
    if (optics_gfx_device_alloc(device,
                                &(optics_gfx_device_alloc_opts){
                                    .type = OPTICS_GFX_TYPE_DEVICE_ALLOC_OPTS,
                                    .bytes = sizeof(*filter),
                                    .owner = TESSERA_MATERIAL_OBSERVE_OWNER,
                                    .kind = TESSERA_MATERIAL_OBSERVE_KIND_LIQUID_GLASS,
                                    .scope = OPTICS_GFX_SCOPE_DEVICE,
                                },
                                (void **)&filter) != OPTICS_GFX_OK)
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
    atomic_init(&filter->ref_count, 1u);
    filter->device = optics_gfx_device_retain(device);
    *out = filter;
    return OPTICS_GFX_OK;
}

tessera_material_liquid_glass_filter *
tessera_material_liquid_glass_filter_retain(tessera_material_liquid_glass_filter *filter) {
    if (filter)
        atomic_fetch_add_explicit(&filter->ref_count, 1u, memory_order_relaxed);
    return filter;
}

void tessera_material_liquid_glass_filter_release(tessera_material_liquid_glass_filter *filter) {
    if (!filter)
        return;
    if (atomic_fetch_sub_explicit(&filter->ref_count, 1u, memory_order_acq_rel) != 1u)
        return;
    optics_gfx_device *device = filter->device;
    for (uint32_t i = 0; i < OPTICS_GFX_MAX_FRAMES_IN_FLIGHT; ++i) {
        tessera_material__slab_release(&filter->slots[i].slab);
        if (filter->slots[i].stats)
            optics_gfx_buffer_release(filter->slots[i].stats);
    }
    /* Pipelines are destroyed inline by gfx; the header's release
     * contract (all referencing GPU work completed) covers this. */
    for (int i = 0; i < 2; ++i) {
        if (filter->glass_pipelines[i])
            optics_gfx_compute_pipeline_release(filter->glass_pipelines[i]);
        if (filter->clear_pipelines[i])
            optics_gfx_compute_pipeline_release(filter->clear_pipelines[i]);
    }
    if (filter->stats_pipeline)
        optics_gfx_compute_pipeline_release(filter->stats_pipeline);
    tessera_material_body_table_release(&filter->bodies);
    tessera_material_body_ids_release(&filter->ids);
    optics_gfx_device_free(device, &(optics_gfx_device_free_opts){
                                       .type = OPTICS_GFX_TYPE_DEVICE_FREE_OPTS,
                                       .ptr = filter,
                                       .owner = TESSERA_MATERIAL_OBSERVE_OWNER,
                                       .kind = TESSERA_MATERIAL_OBSERVE_KIND_LIQUID_GLASS,
                                       .scope = OPTICS_GFX_SCOPE_DEVICE,
                                   });
    optics_gfx_device_release(device);
}

static optics_gfx_result
liquid_glass_ensure_stats_buffer(tessera_material_liquid_glass_filter *filter,
                                 liquid_glass_filter_slot *slot) {
    if (slot->stats)
        return OPTICS_GFX_OK;
    optics_gfx_buffer_desc bdesc = OPTICS_GFX_BUFFER_DESC_INIT;
    bdesc.size = TESSERA_MATERIAL_STATS_BYTES;
    bdesc.usage = OPTICS_GFX_BUFFER_USAGE_STORAGE;
    /* HOST_VISIBLE is HOST_COHERENT in gfx: the shader's writes are
     * visible to the CPU after the slot's fence, no flush/invalidate. */
    bdesc.location = OPTICS_GFX_BUFFER_HOST_VISIBLE;
    bdesc.device_address = true; /* stats shader writes via buffer reference */
    return optics_gfx_buffer_create(filter->device, &bdesc, &slot->stats);
}

/* Ensure the slot's slab can hold `footprint`: an image whose extent is the
 * footprint quantised up the size ladder, never the capture. Re-allocation only
 * happens when the quantised extent actually changes, so a resize sweep that
 * walks a few pixels per frame reuses the image instead of churning one per
 * step. */
static optics_gfx_result liquid_glass_ensure_slot(tessera_material_liquid_glass_filter *filter,
                                                  uint32_t index, const optics_gfx_image *input,
                                                  uint32_t footprint_w, uint32_t footprint_h,
                                                  bool need_stats) {
    liquid_glass_filter_slot *slot = &filter->slots[index];
    bool recreated = false;
    optics_gfx_result r = tessera_material__slab_ensure(
        filter->device, &slot->slab,
        &(tessera_material__slab_opts){
            .input = input, .footprint_width = footprint_w, .footprint_height = footprint_h},
        &recreated);
    if (recreated) {
        /* The previous regions and the stats buffer described the old image. */
        if (slot->stats)
            optics_gfx_buffer_release(slot->stats);
        *slot = (liquid_glass_filter_slot){.slab = slot->slab};
    }
    if (r != OPTICS_GFX_OK)
        return r;
    return need_stats ? liquid_glass_ensure_stats_buffer(filter, slot) : OPTICS_GFX_OK;
}

static optics_gfx_result liquid_glass_ensure_pipelines(tessera_material_liquid_glass_filter *filter,
                                                       bool is16f, bool need_clear, bool need_glass,
                                                       bool need_stats) {
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
    if (r == OPTICS_GFX_OK && need_glass)
        r = tessera_material__pipeline_ensure(filter->device, &filter->glass_pipelines[cls],
                                             &(tessera_material__pipeline_opts){
                                                 .shader = TESSERA_MATERIAL__SHADER_LIQUID_GLASS,
                                                 .is16f = is16f,
                                                 .push_constant_bytes = sizeof(liquid_glass_push),
                                             });
    /* The stats shader only samples, so one pipeline serves both classes. */
    if (r == OPTICS_GFX_OK && need_stats)
        r = tessera_material__pipeline_ensure(filter->device, &filter->stats_pipeline,
                                             &(tessera_material__pipeline_opts){
                                                 .shader = TESSERA_MATERIAL__SHADER_BACKDROP_STATS,
                                                 .push_constant_bytes = sizeof(backdrop_stats_push),
                                             });
    return r;
}

optics_gfx_result
tessera_material_liquid_glass_filter_warmup(tessera_material_liquid_glass_filter *filter,
                                           const tessera_material_liquid_glass_warmup_opts *opts) {
    if (!filter)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;

    bool is16f = false;
    bool stats = true;
    if (opts) {
        if (opts->format == OPTICS_GFX_FORMAT_RGBA16_SFLOAT)
            is16f = true;
        stats = opts->stats;
    }

    /* Pre-allocate body table and id list storage buffers to eliminate driver host-visible buffer
     * allocation hitch (ADR-0141) */
    void *dummy_row = tessera_material_body_table_row(filter->device, &filter->bodies, 1u, 0u);
    uint32_t *dummy_ids = tessera_material_body_ids_slot(filter->device, &filter->ids, 1u);
    if (!dummy_row || !dummy_ids)
        return OPTICS_GFX_ERROR_OUT_OF_MEMORY;

    return liquid_glass_ensure_pipelines(filter, is16f, true, true, stats);
}

/* ------------------------------------------------------------------ */
/*  Apply                                                             */
/* ------------------------------------------------------------------ */

static bool valid_liquid_glass_desc(const tessera_material_liquid_glass_desc *desc) {
    if (desc->type != TESSERA_MATERIAL_TYPE_LIQUID_GLASS_DESC || !desc->input ||
        !desc->blurred_input || desc->group_count > LIQUID_GLASS_MAX_GROUPS ||
        (desc->group_count > 0u && !desc->groups))
        return false;
    if (!isfinite(desc->refraction) || !isfinite(desc->chromatic_aberration) ||
        !isfinite(desc->saturation) || !isfinite(desc->brightness) || !isfinite(desc->edge_width) ||
        !isfinite(desc->rim_light) || !isfinite(desc->light_direction.x) ||
        !isfinite(desc->light_direction.y) || !isfinite(desc->opacity) ||
        !isfinite(desc->size_reference) || !isfinite(desc->size_scale_min) ||
        !isfinite(desc->tint_strength) || !isfinite(desc->frost_strength) ||
        !isfinite(desc->curvature))
        return false;
    for (uint32_t i = 0; i < desc->group_count; ++i) {
        if (!liquid_glass_group_is_valid(&desc->groups[i]))
            return false;
    }
    return true;
}

/* Everything one liquid-glass submission needs to replay its dispatches later
 * (ADR-0147). Body rows and id lists are NOT here: they upload into host-visible
 * buffers that outlive the call, so the replay reads them through the addresses
 * captured below. Only the dispatch geometry and the policy travel in the plan,
 * which is what keeps the arena copy small. */
typedef struct glass_plan {
    optics_gfx_device *device;
    struct tessera_material_liquid_glass_filter *filter;
    optics_gfx_image *input;
    optics_gfx_image *blurred;
    optics_gfx_image *output;
    uint32_t output_width;
    uint32_t output_height;
    uint32_t capture_width;
    uint32_t capture_height;
    bool is16f;

    tessera_material_glass_policy policy;
    tessera_material_glass_copy copy;

    uint64_t bodies_address;
    uint64_t ids_address;

    tessera_material__rect clear_regions[TESSERA_MATERIAL_MAX_BATCHES];
    uint32_t clear_count;

    tessera_material_region_batch batches[TESSERA_MATERIAL_MAX_BATCHES];
    uint32_t batch_count;

    /* Per-group statistic windows, in capture space. */
    tessera_material__rect stats_bounds[LIQUID_GLASS_MAX_GROUPS];
    uint32_t stats_group_count;
    uint64_t stats_address;
    VkBuffer stats_buffer;
} glass_plan;

static void glass_plan_record(void *raw, VkCommandBuffer command) {
    const glass_plan *p = raw;
    if (!p || command == VK_NULL_HANDLE || !p->filter)
        return;
    tessera_material_liquid_glass_filter *filter = p->filter;
    VkImage output_image = optics_gfx_image_vk_image(p->output);
    bool dispatched = false;

    if (p->clear_count > 0u) {
        tessera_material__barrier_reuse_to_compute_write(command, output_image);
        tessera_material__clear_regions(
            &(tessera_material__clear_opts){.cmd = command,
                                           .pipeline = filter->clear_pipelines[p->is16f ? 1 : 0],
                                           .image = p->output,
                                           .regions = p->clear_regions,
                                           .region_count = p->clear_count});
        if (p->batch_count > 0u)
            tessera_material__barrier_compute_write_to_read_write(command, output_image);
    }

    if (p->stats_group_count > 0u) {
        for (uint32_t i = 0; i < p->stats_group_count; ++i) {
            const tessera_material__rect *rect = &p->stats_bounds[i];
            backdrop_stats_push spush = {
                .input_handle = optics_gfx_image_bindless_handle(p->input),
                .blurred_handle = optics_gfx_image_bindless_handle(p->blurred),
                .sampler_handle = optics_gfx_device_default_sampler_handle(p->device),
                .width = p->capture_width,
                .height = p->capture_height,
                .rect_x = rect->x,
                .rect_y = rect->y,
                .rect_width = rect->width,
                .rect_height = rect->height,
                .group_index = i,
                .stats_address_lo = tessera_material_address_lo(p->stats_address),
                .stats_address_hi = tessera_material_address_hi(p->stats_address),
            };
            optics_gfx_compute_dispatch(
                &(optics_gfx_compute_dispatch_opts){.cmd = command,
                                                    .pipeline = filter->stats_pipeline,
                                                    .push_constants = &spush,
                                                    .push_bytes = sizeof(spush),
                                                    .group_x = 1u,
                                                    .group_y = 1u,
                                                    .group_z = 1u});
        }
        if (p->stats_buffer != VK_NULL_HANDLE)
            tessera_material__barrier_buffer_compute_write_to_compute(command, p->stats_buffer);
    }

    for (uint32_t r = 0; r < p->batch_count; ++r) {
        const tessera_material_region_batch *b = &p->batches[r];
        if (!liquid_glass_region_is_dispatchable(
                (tessera_material__rect){b->x, b->y, b->width, b->height}, p->output_width,
                p->output_height))
            continue;
        tessera_material_glass_record_batch(command, filter->glass_pipelines[p->is16f ? 1 : 0],
                                           p->device, &p->copy, p->output, p->output_width,
                                           p->output_height, b, &p->policy, p->bodies_address,
                                           p->ids_address + b->ids_first * sizeof(uint32_t));
        dispatched = true;
    }

    /* Make the persistent output visible to the Canvas sample that follows,
     * including a clear-only submission after every body disappeared. */
    if (p->clear_count > 0u || dispatched)
        tessera_material__barrier_compute_write_to_read(command, output_image);
}

optics_gfx_result tessera_material_liquid_glass_filter_apply(
    tessera_material_liquid_glass_filter *filter,
    const tessera_material_liquid_glass_filter_apply_opts *opts) {
    if (!filter || !opts || !opts->frame || !opts->desc || !opts->out)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    optics_gfx_frame *frame = opts->frame;
    const tessera_material_liquid_glass_desc *desc = opts->desc;
    tessera_material_output *out = opts->out;
    *out = (tessera_material_output){};
    if (!valid_liquid_glass_desc(desc))
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (optics_gfx_frame_get_state(frame) != OPTICS_GFX_FRAME_STATE_RECORDING ||
        optics_gfx_frame_has_active_pass(frame))
        return OPTICS_GFX_ERROR_INVALID_STATE;
    optics_gfx_image *input = desc->input;
    optics_gfx_image *blurred = desc->blurred_input;
    /* ADR-0147: `blurred` is normally a deferred optics_gfx_blur_filter output.
     * Materialize it before this dispatch is recorded, so the blur's
     * dispatches precede the material's in the command stream. */
    optics_gfx_result mat = optics_gfx_frame_materialize_image(frame, blurred);
    if (mat != OPTICS_GFX_OK)
        return mat;
    if (optics_gfx_frame_device(frame) != filter->device ||
        optics_gfx_image_device(input) != filter->device ||
        optics_gfx_image_device(blurred) != filter->device)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    /* ADR-0069: 8-bit SDR inputs keep the historic RGBA8 path (blurred
     * input is the gfx blur filter's RGBA8 output); 16F working-space
     * inputs run the material in 16F end to end, and the blurred input
     * must be the matching 16F blur output. */
    const bool is16f = optics_gfx_image_format(input) == OPTICS_GFX_FORMAT_RGBA16_SFLOAT;
    const optics_gfx_format want_blur =
        is16f ? OPTICS_GFX_FORMAT_RGBA16_SFLOAT : OPTICS_GFX_FORMAT_RGBA8_UNORM;
    if (optics_gfx_image_width(input) != optics_gfx_image_width(blurred) ||
        optics_gfx_image_height(input) != optics_gfx_image_height(blurred) ||
        (optics_gfx_image_format(input) != OPTICS_GFX_FORMAT_RGBA8_UNORM &&
         optics_gfx_image_format(input) != OPTICS_GFX_FORMAT_BGRA8_UNORM && !is16f) ||
        optics_gfx_image_format(blurred) != want_blur ||
        optics_gfx_image_bindless_handle(input) == OPTICS_GFX_BINDLESS_INVALID ||
        optics_gfx_image_bindless_handle(blurred) == OPTICS_GFX_BINDLESS_INVALID)
        return OPTICS_GFX_ERROR_UNSUPPORTED;
    uint32_t index = optics_gfx_frame_index(frame);
    if (index >= OPTICS_GFX_MAX_FRAMES_IN_FLIGHT)
        return OPTICS_GFX_ERROR_OUT_OF_RANGE;

    OPTICS_GFX_PROFILE_ZONE(z_glass, "material.liquid_glass_apply");

    const uint32_t capture_width = optics_gfx_image_width(input);
    const uint32_t capture_height = optics_gfx_image_height(input);
    liquid_glass_filter_slot *slot = &filter->slots[index];

    /* 1. Footprints, in capture space. */
    tessera_material__rect current[LIQUID_GLASS_MAX_GROUPS];
    uint32_t current_body_ids[LIQUID_GLASS_MAX_GROUPS];
    uint32_t current_count = 0u;
    uint32_t footprint_x0 = UINT32_MAX, footprint_y0 = UINT32_MAX;
    uint32_t footprint_x1 = 0u, footprint_y1 = 0u;
    for (uint32_t i = 0; i < desc->group_count; ++i) {
        const tessera_material_liquid_glass_group *group = &desc->groups[i];
        /* The body's shadow reaches its downward offset plus the falloff. */
        float shadow_reach = group->shadow_alpha > 0.0f ? fmaxf(group->shadow_offset_y, 0.0f) +
                                                              2.0f * fmaxf(group->shadow_blur, 0.0f)
                                                        : 0.0f;
        tessera_material__rect region;
        if (!liquid_glass_group_dispatch_bounds(group, shadow_reach, capture_width, capture_height,
                                                &region))
            continue;
        current[current_count] = region;
        current_body_ids[current_count] = i;
        ++current_count;
        if (region.x < footprint_x0)
            footprint_x0 = region.x;
        if (region.y < footprint_y0)
            footprint_y0 = region.y;
        if (region.x + region.width > footprint_x1)
            footprint_x1 = region.x + region.width;
        if (region.y + region.height > footprint_y1)
            footprint_y1 = region.y + region.height;
    }

    /* 2. Slab extent from those footprints. An empty submission keeps a minimal
     * slab: it still has to exist to be returned and cleared. */
    uint32_t footprint_w = 1u;
    uint32_t footprint_h = 1u;
    if (current_count > 0u) {
        footprint_w = footprint_x1 - footprint_x0;
        footprint_h = footprint_y1 - footprint_y0;
    }
    OPTICS_GFX_PROFILE_ZONE(z_slot, "material.liquid_glass.ensure_slot");
    optics_gfx_result result = liquid_glass_ensure_slot(filter, index, input, footprint_w,
                                                        footprint_h, desc->group_count > 0u);
    OPTICS_GFX_PROFILE_ZONE_END(z_slot);
    if (result != OPTICS_GFX_OK)
        return result;

    /* The slab covers every footprint by construction, so its origin IS the
     * footprint origin and the capture-to-slab offset is its negation. Regions
     * are stored in slab space from here on, including the ones kept for the
     * next submission's clear. */
    const uint32_t slab_x = current_count > 0u ? footprint_x0 : 0u;
    const uint32_t slab_y = current_count > 0u ? footprint_y0 : 0u;
    for (uint32_t i = 0; i < current_count; ++i) {
        current[i].x -= slab_x;
        current[i].y -= slab_y;
    }
    slot->slab_x = slab_x;
    slot->slab_y = slab_y;
    slot->capture_width = capture_width;
    slot->capture_height = capture_height;

    /* 3. Clear previous + current footprints (slab space) to transparent. */
    tessera_material__rect clear_regions[LIQUID_GLASS_MAX_CLEAR_REGIONS];
    uint32_t clear_count = 0u;
    if (!liquid_glass_build_clear_regions(slot->initialized, slot->slab.width, slot->slab.height,
                                          slot->previous, slot->previous_count, current,
                                          current_count, clear_regions,
                                          LIQUID_GLASS_MAX_CLEAR_REGIONS, &clear_count))
        return OPTICS_GFX_ERROR_INVALID_STATE;

    /**
     * 4. Coalesce into disjoint dispatch regions, and for each region record the
     * bodies that reach it, in caller order.
     *
     * Coalescing is what makes batching correct as well as cheap: two bodies that
     * share a pixel necessarily coalesce into the same rectangle, so their
     * source-over order becomes id order inside one dispatch. Disjoint regions
     * never write each other's pixels, so no barrier is needed between them —
     * which removes the per-body barrier chain that used to make cost linear in
     * body count.
     */
    tessera_material_region_batch batches[TESSERA_MATERIAL_MAX_BATCHES];
    uint32_t batch_ids[TESSERA_MATERIAL_MAX_BODIES];
    uint32_t batch_ids_used = 0u;
    tessera_material__rect merged[TESSERA_MATERIAL_MAX_BATCHES];
    uint32_t merged_count = 0u;
    for (uint32_t i = 0; i < current_count; ++i) {
        if (!liquid_glass_add_clear_region(current[i], merged, TESSERA_MATERIAL_MAX_BATCHES,
                                           &merged_count)) {
            /* Cannot coalesce into the budget: one region covering the slab. Every
             * body lands in it, which is correct (the shader intersects each body
             * with its own SDF anyway) and is still a single dispatch. */
            merged[0] = (tessera_material__rect){0u, 0u, slot->slab.width, slot->slab.height};
            merged_count = 1u;
            break;
        }
    }
    /* Each body's footprint belongs to exactly one merged region: merging is
     * transitive over overlap, so a footprint touching two regions would have
     * fused them. That makes the assignment total, the id array bounded by the
     * body count (no per-region capacity guessing), and the per-region id runs
     * trivially contiguous. */
    uint32_t body_owner[LIQUID_GLASS_MAX_GROUPS];
    for (uint32_t i = 0; i < current_count; ++i) {
        body_owner[i] = UINT32_MAX;
        for (uint32_t r = 0; r < merged_count; ++r) {
            if (liquid_glass_regions_overlap(current[i], merged[r])) {
                body_owner[i] = r;
                break;
            }
        }
    }
    uint32_t batch_count = 0u;
    for (uint32_t r = 0; r < merged_count; ++r) {
        uint32_t first = batch_ids_used;
        /* Caller order inside the region = the source-over order the material
         * requires, and it is exactly the order the shader's loop walks. */
        for (uint32_t i = 0; i < current_count; ++i) {
            if (body_owner[i] == r)
                batch_ids[batch_ids_used++] = current_body_ids[i];
        }
        batches[batch_count++] = (tessera_material_region_batch){
            .x = merged[r].x,
            .y = merged[r].y,
            .width = merged[r].width,
            .height = merged[r].height,
            .ids_first = first,
            .id_count = batch_ids_used - first,
        };
    }

    OPTICS_GFX_PROFILE_ZONE(z_pipe, "material.liquid_glass.ensure_pipelines");
    result = liquid_glass_ensure_pipelines(filter, is16f, clear_count > 0u, batch_count > 0u,
                                           desc->group_count > 0u);
    OPTICS_GFX_PROFILE_ZONE_END(z_pipe);
    if (result != OPTICS_GFX_OK)
        return result;

    const tessera_material_glass_policy policy = {
        .refraction = desc->refraction,
        .chromatic_aberration = desc->chromatic_aberration,
        .saturation = desc->saturation,
        .brightness = desc->brightness,
        .edge_width = desc->edge_width,
        .rim_light = desc->rim_light,
        .light_x = desc->light_direction.x,
        .light_y = desc->light_direction.y,
        .opacity = desc->opacity,
        .size_reference = desc->size_reference,
        .size_scale_min = desc->size_scale_min,
        .tint_strength = desc->tint_strength,
        .frost_strength = desc->frost_strength,
        .curvature = desc->curvature,
        .contact_ao = desc->contact_ao,
        .ambient_fresnel = desc->ambient_fresnel,
    };

    /* Upload the body rows and id lists now: they live in host-visible buffers
     * that outlive this call, so a deferred replay reads them through the
     * addresses captured below rather than through the arena. */
    bool have_batches = batch_count > 0u;
    if (have_batches) {
        uint32_t rows = desc->group_count;
        for (uint32_t i = 0; i < rows; ++i) {
            void *row = tessera_material_body_table_row(filter->device, &filter->bodies, rows, i);
            if (!row)
                return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
            *(tessera_material_glass_body_row *)row =
                tessera_material_glass_make_row(&desc->groups[i], &policy);
        }
        uint32_t *ids = tessera_material_body_ids_slot(filter->device, &filter->ids, batch_ids_used);
        if (!ids)
            return OPTICS_GFX_ERROR_OUT_OF_MEMORY;
        for (uint32_t i = 0; i < batch_ids_used; ++i)
            ids[i] = batch_ids[i];
    }

    glass_plan plan = {
        .device = filter->device,
        .filter = filter,
        .input = input,
        .blurred = blurred,
        .output = slot->slab.image,
        .output_width = slot->slab.width,
        .output_height = slot->slab.height,
        .capture_width = capture_width,
        .capture_height = capture_height,
        .is16f = is16f,
        .policy = policy,
        .copy =
            {
                .sharp = input,
                .blurred = blurred,
                .capture_width = (float)capture_width,
                .capture_height = (float)capture_height,
                .slab_origin_x = (float)slab_x,
                .slab_origin_y = (float)slab_y,
            },
        .bodies_address = filter->bodies.address,
        .ids_address = filter->ids.address,
        .clear_count = clear_count,
        .batch_count = batch_count,
        .stats_group_count = desc->group_count,
        .stats_address = slot->stats ? optics_gfx_buffer_device_address(slot->stats) : 0u,
        .stats_buffer = slot->stats ? optics_gfx_buffer_vk_buffer(slot->stats) : VK_NULL_HANDLE,
    };
    /* The slab was not necessarily initialised before this call, so the reuse
     * barrier is decided here rather than in the replay. */
    bool needed_reuse_barrier = slot->initialized;
    (void)needed_reuse_barrier;
    memcpy(plan.clear_regions, clear_regions, clear_count * sizeof(clear_regions[0]));
    memcpy(plan.batches, batches, batch_count * sizeof(batches[0]));
    for (uint32_t i = 0; i < desc->group_count; ++i)
        plan.stats_bounds[i] =
            liquid_glass_group_stats_bounds(&desc->groups[i], capture_width, capture_height);

    if (current_count > 0u)
        memcpy(slot->previous, current, current_count * sizeof(current[0]));
    slot->previous_count = current_count;
    slot->initialized = true;
    slot->stats_submitted = true;
    /* A zero-group apply submits no stats; the frame-lagged read then
     * honestly reports zero groups for this slot's last submission. */
    slot->stats_group_count = desc->group_count;
    *out = (tessera_material_output){
        .image = slot->slab.image,
        .origin_x = (float)slab_x,
        .origin_y = (float)slab_y,
        .width = slot->slab.width,
        .height = slot->slab.height,
    };

    /* Defer the dispatches (ADR-0147): the handle above is valid now, the work
     * replays only if something requires the output. Recording eagerly on a
     * registration failure keeps correctness independent of pruning. */
    tessera_material_deferred_pass pass = TESSERA_MATERIAL_DEFERRED_PASS_INIT;
    pass.output = slot->slab.image;
    pass.label = "material.liquid_glass";
    pass.inputs[0] = input;
    pass.inputs[1] = blurred;
    pass.input_count = 2;
    optics_gfx_deferred_token *token = nullptr;
    optics_gfx_result reg = tessera_material_deferred_register(frame, &pass, glass_plan_record,
                                                              &plan, sizeof(plan), &token);
    if (reg != OPTICS_GFX_OK) {
        glass_plan_record(&plan, optics_gfx_frame_vk_command_buffer(frame));
        token = nullptr;
    }
    /* The host draws this slab in a canvas pass, so it is the terminal consumer
     * and the pass must run. A downstream pass would name the token instead. */
    (void)tessera_material_deferred_require(frame, token);

    if (current_count > 0u)
        memcpy(slot->previous, current, current_count * sizeof(current[0]));
    slot->previous_count = current_count;
    slot->initialized = true;
    slot->stats_submitted = true;
    /* A zero-group apply submits no stats; the frame-lagged read then
     * honestly reports zero groups for this slot's last submission. */
    slot->stats_group_count = desc->group_count;
    *out = (tessera_material_output){
        .image = slot->slab.image,
        .origin_x = (float)slab_x,
        .origin_y = (float)slab_y,
        .width = slot->slab.width,
        .height = slot->slab.height,
    };
    OPTICS_GFX_PROFILE_ZONE_END(z_glass);
    return OPTICS_GFX_OK;
}

optics_gfx_result tessera_material_liquid_glass_filter_stats(
    tessera_material_liquid_glass_filter *filter,
    const tessera_material_liquid_glass_filter_stats_opts *opts) {
    if (!filter || !opts || !opts->frame || !opts->out_count ||
        (opts->max_groups > 0u && !opts->out))
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    optics_gfx_frame *frame = opts->frame;
    tessera_material_backdrop_stat *out = opts->out;
    uint32_t max_groups = opts->max_groups;
    uint32_t *out_count = opts->out_count;
    *out_count = 0u;
    if (optics_gfx_frame_device(frame) != filter->device)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    /* The mapped buffer is stable only while this frame records: its
     * begin_frame waited the slot's fence. */
    if (optics_gfx_frame_get_state(frame) != OPTICS_GFX_FRAME_STATE_RECORDING)
        return OPTICS_GFX_ERROR_INVALID_STATE;
    uint32_t index = optics_gfx_frame_index(frame);
    if (index >= OPTICS_GFX_MAX_FRAMES_IN_FLIGHT)
        return OPTICS_GFX_ERROR_OUT_OF_RANGE;
    const liquid_glass_filter_slot *slot = &filter->slots[index];
    if (!slot->stats_submitted || !slot->stats)
        return OPTICS_GFX_ERROR_INVALID_STATE;
    uint32_t count = slot->stats_group_count < max_groups ? slot->stats_group_count : max_groups;
    if (count > 0u)
        memcpy(out, optics_gfx_buffer_mapped(slot->stats), count * sizeof(*out));
    *out_count = count;
    return OPTICS_GFX_OK;
}
