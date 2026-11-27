#ifndef TESSERA_MATERIAL_GLASS_DISPATCH_H
#define TESSERA_MATERIAL_GLASS_DISPATCH_H

/*
 * Shared glass-dispatch recording between the standalone liquid-glass
 * material (liquid_glass.c) and the layered backdrop compositor
 * (backdrop_layer.c). Both materials must evaluate a tessera_material_liquid_glass_group
 * with the identical reference recipe — the layer filter exists precisely so
 * a glass body in a layer stack samples the frosted sheet beneath it rather
 * than the sharp capture, and any divergence between the two dispatch
 * recorders would make a layered body render differently from a standalone
 * one.
 *
 * This header is material-internal (not installed) and holds no state: every
 * entry is a pure function of its arguments.
 */

#include <tessera/material/liquid_glass.h>

#include <optics/gfx/compute.h>
#include <optics/gfx/vulkan.h>

#include "material_frame.h"
#include "regions.h"

#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

/* Push-constant block — must match liquid_glass.comp exactly.
 *
 * Per-BODY state lives in the body table (material_frame.h), not here: the
 * block is a hard 160 bytes and this material previously consumed all of them
 * with one body's parameters, which is why cost used to scale linearly with
 * body count. What remains is genuinely per-DISPATCH: the sources, the output,
 * the region inside that output, the body list for this region, and the
 * dispatch-wide optical policy. That frees the room for region batches and
 * multi-region slabs without touching the budget. */
typedef struct liquid_glass_push {
    uint32_t input_handle;
    uint32_t blurred_handle;
    uint32_t sampler_handle;
    uint32_t output_handle;
    /* Output image extent and this dispatch's region inside it. */
    uint32_t width;
    uint32_t height;
    uint32_t origin_x;
    uint32_t origin_y;
    uint32_t region_width;
    uint32_t region_height;
    /* This slab's origin in capture space. The slab is sized to the material's
     * footprints, so output coordinate 0 is capture coordinate slab_origin:
     * capture = output + slab_origin. */
    float slab_origin_x;
    float slab_origin_y;
    float capture_width;
    float capture_height;
    /* Address of the body table (rows of tessera_material_glass_body_row) and of this
     * region's body-id list. Split lo/hi: gfx devices do not guarantee
     * shaderInt64. */
    uint32_t bodies_address_lo;
    uint32_t bodies_address_hi;
    uint32_t ids_address_lo;
    uint32_t ids_address_hi;
    uint32_t id_count;
    uint32_t reserved0;
    /* Dispatch-wide optical policy (tessera_material_glass_policy). */
    float refraction;
    float chromatic_aberration;
    float saturation;
    float brightness;
    float edge_width;
    float rim_light;
    float opacity;
    float size_reference;
    float size_scale_min;
    float tint_strength;
    float frost_strength;
    float policy_curvature;
    float contact_ao;
    float ambient_fresnel;
    float light_x;
    float light_y;
} liquid_glass_push;

static_assert(sizeof(liquid_glass_push) == 144,
              "liquid_glass_push exceeds the 160-byte push budget gfx guarantees");
static_assert(offsetof(liquid_glass_push, slab_origin_x) == 40,
              "liquid_glass_push capture offset no longer matches its shader block");
static_assert(offsetof(liquid_glass_push, refraction) == 80,
              "liquid_glass_push policy no longer matches its shader block");

/* One row of the body table — must match `GlassBody` in liquid_glass.comp.
 *
 * Per-body optical character, resolved against the dispatch policy on the host
 * so the shader does no inheritance logic. `*_strength` values are already the
 * effective (override-or-inherit) values, already clamped, already multiplied by
 * the dispatch-wide opacity for `opacity`. */
typedef struct tessera_material_glass_body_row {
    float shape0[4]; /* bounds x, y, w, h */
    float shape1[4];
    float focus_shape[4];
    float radius0;
    float radius1;
    float focus_radius;
    float blend_radius;
    float opacity;
    float shadow_alpha;
    float shadow_blur;
    float shadow_offset_y;
    float tint_strength;
    float frost_strength;
    float saturation;
    float curvature;
    float contact_ao;
    float ambient_fresnel;
    float size_reference;
    float size_scale_min;
    float light_x;
    float light_y;
    uint32_t tint_color; /* 0xRRGGBB (top byte free) */
    uint32_t shape_count;
    uint32_t has_focus; /* 1 when focus_strength > 0 */
    uint32_t reserved;
    /* Pad to TESSERA_MATERIAL_BODY_STRIDE. The shader's GlassBody pads identically: the row
     * stride is one number shared by both sides, and a mismatch reads the wrong
     * row. Asserted below on both the payload and the total. */
    uint32_t pad0[2];  /* payload 136 -> 144, matching 16-byte alignment of GlassBody (ADR-0156) */
    uint32_t pad1[28]; /* 112 more -> 256 == TESSERA_MATERIAL_BODY_STRIDE */
} tessera_material_glass_body_row;

static_assert(offsetof(tessera_material_glass_body_row, pad0) == 136,
              "tessera_material_glass_body_row payload changed; re-pad the shader's GlassBody");
static_assert(offsetof(tessera_material_glass_body_row, pad1) == 144,
              "the shader 16-aligns GlassBody's payload to 144; keep the C row in step");
static_assert(sizeof(tessera_material_glass_body_row) == TESSERA_MATERIAL_BODY_STRIDE,
              "tessera_material_glass_body_row must be exactly TESSERA_MATERIAL_BODY_STRIDE bytes: "
              "the shader's "
              "GlassBody array stride is its sizeof, so the two must agree");

/* Bounds of a body's footprint in capture pixels, in output space, clipped to
 * the output slab. */
static inline tessera_material__rect
tessera_material_glass_output_region(tessera_material__rect capture_region, uint32_t slab_x,
                                    uint32_t slab_y, uint32_t slab_w, uint32_t slab_h) {
    tessera_material__rect out = capture_region;
    /* Capture region is already clipped to the capture, and the slab covers every
     * footprint, so this only guards degenerate slabs. */
    if (out.x < slab_x)
        out.x = slab_x;
    if (out.y < slab_y)
        out.y = slab_y;
    if (out.x + out.width > slab_x + slab_w)
        out.width = slab_x + slab_w - out.x;
    if (out.y + out.height > slab_y + slab_h)
        out.height = slab_y + slab_h - out.y;
    return out;
}

#define TESSERA_MATERIAL_GLASS_WG 16u

/* IEEE-754 binary32 → binary16, round-to-nearest-even, for packed
 * 16-bit float push slots. Inputs are validated finite and clamped by caller. */
static inline uint16_t tessera_material_glass_f32_to_f16(float value) {
    uint32_t bits;
    memcpy(&bits, &value, sizeof(bits));
    uint32_t sign = (bits >> 16u) & 0x8000u;
    uint32_t exponent = (bits >> 23u) & 0xFFu;
    uint32_t mantissa = bits & 0x7FFFFFu;

    if (exponent == 0xFFu) /* inf/nan (validation rejects nan) → f16 inf */
        return (uint16_t)(sign | 0x7C00u);

    int half_exp = (int)exponent - 127 + 15;
    if (half_exp >= 0x1F) /* overflow → f16 inf */
        return (uint16_t)(sign | 0x7C00u);
    if (half_exp <= 0) {
        /* f16 subnormal or signed zero. */
        if (half_exp < -10)
            return (uint16_t)sign;
        mantissa |= 0x800000u;                      /* restore the hidden bit */
        uint32_t shift = (uint32_t)(14 - half_exp); /* 13..24 */
        uint32_t half = mantissa >> shift;
        uint32_t remainder = mantissa & ((1u << shift) - 1u);
        uint32_t halfway = 1u << (shift - 1u);
        if (remainder > halfway || (remainder == halfway && (half & 1u)))
            ++half; /* may carry into the exponent field — correct */
        return (uint16_t)(sign | half);
    }
    /* Normal f16: round the 13 dropped mantissa bits, nearest-even. */
    uint32_t half = sign | ((uint32_t)half_exp << 10u) | (mantissa >> 13u);
    uint32_t remainder = mantissa & 0x1FFFu;
    if (remainder > 0x1000u || (remainder == 0x1000u && (half & 1u)))
        ++half; /* mantissa overflow carries into the exponent — correct */
    return (uint16_t)half;
}

static inline void tessera_material_glass_copy_shape(float out[4],
                                                    tessera_material_liquid_glass_shape shape) {
    out[0] = shape.bounds.x;
    out[1] = shape.bounds.y;
    out[2] = shape.bounds.w;
    out[3] = shape.bounds.h;
}

/* Dispatch-wide glass policy shared by both materials: the desc knobs the
 * reference recipe consumes. The standalone desc and the layered desc both
 * flatten into this before any group is recorded, so a body's evaluation
 * cannot depend on which filter drew it. */
typedef struct tessera_material_glass_policy {
    float refraction;
    float chromatic_aberration;
    float saturation;
    float brightness;
    float edge_width;
    float rim_light;
    float light_x;
    float light_y;
    float opacity;
    float size_reference;
    float size_scale_min;
    float tint_strength;
    float frost_strength;
    float curvature;
    float contact_ao;
    float ambient_fresnel;
} tessera_material_glass_policy;

/* Flatten one caller group into a body-table row. Every inherit/override and
 * every clamp happens HERE, on the host, so the shader reads effective values and
 * carries no inheritance logic (which was a per-pixel branch before). */
static inline tessera_material_glass_body_row
tessera_material_glass_make_row(const tessera_material_liquid_glass_group *group,
                               const tessera_material_glass_policy *policy) {
    float saturation = liquid_glass_group_or_desc(group->saturation, policy->saturation);
    float tint_strength = liquid_glass_group_or_desc(group->tint_strength, policy->tint_strength);
    float frost_strength =
        liquid_glass_group_or_desc(group->frost_strength, policy->frost_strength);
    float size_reference = fmaxf(policy->size_reference, 0.0f);
    float size_scale_min = fminf(fmaxf(policy->size_scale_min, 0.0f), 1.0f);
    float focus_strength = fminf(fmaxf(group->focus_strength, 0.0f), 1.0f);
    float curvature = liquid_glass_group_or_desc(group->curvature, policy->curvature);
    curvature = fminf(fmaxf(curvature, 0.0f), 1.0f);
    float contact_ao = liquid_glass_group_or_desc(group->contact_ao, policy->contact_ao);
    if (contact_ao < 0.0f)
        contact_ao = 0.35f;
    contact_ao = fminf(fmaxf(contact_ao, 0.0f), 1.0f);
    float ambient_fresnel =
        liquid_glass_group_or_desc(group->ambient_fresnel, policy->ambient_fresnel);
    if (ambient_fresnel < 0.0f)
        ambient_fresnel = 0.50f;
    ambient_fresnel = fminf(fmaxf(ambient_fresnel, 0.0f), 1.0f);

    /* Capacity guard: the row carries exactly two SDF slots (shape0/shape1). This
     * must never fire in practice — validation clamps shape_count to
     * LIQUID_GLASS_MAX_SHAPES — but a capacity raise in regions.h without a
     * matching row raise must fail the build, not silently truncate geometry. */
    _Static_assert(LIQUID_GLASS_MAX_SHAPES <= 2u,
                   "the body row carries two SDF slots; raise the row before raising "
                   "LIQUID_GLASS_MAX_SHAPES");

    tessera_material_glass_body_row row = {};
    tessera_material_glass_copy_shape(row.shape0, group->shapes[0]);
    if (group->shape_count == 2u)
        tessera_material_glass_copy_shape(row.shape1, group->shapes[1]);
    if (focus_strength > 0.0f) {
        tessera_material_glass_copy_shape(row.focus_shape, group->focus);
        row.has_focus = 1u;
    }
    row.radius0 = fmaxf(group->shapes[0].corner_radius, 0.0f);
    row.radius1 = (group->shape_count == 2u ? fmaxf(group->shapes[1].corner_radius, 0.0f) : 0.0f);
    row.focus_radius = fmaxf(group->focus.corner_radius, 0.0f);
    row.blend_radius = fmaxf(group->blend_radius, 0.0f);
    row.opacity = fminf(fmaxf(group->opacity * policy->opacity, 0.0f), 1.0f);
    row.shadow_alpha = fminf(fmaxf(group->shadow_alpha, 0.0f), 1.0f);
    row.shadow_blur = fmaxf(group->shadow_blur, 0.0f);
    row.shadow_offset_y = group->shadow_offset_y;
    row.tint_strength = fmaxf(tint_strength, 0.0f);
    row.frost_strength = fmaxf(frost_strength, 0.0f);
    row.saturation = fmaxf(saturation, 0.0f);
    row.curvature = curvature;
    row.contact_ao = contact_ao;
    row.ambient_fresnel = ambient_fresnel;
    row.size_reference = size_reference;
    row.size_scale_min = size_scale_min;
    row.light_x = policy->light_x;
    row.light_y = policy->light_y;
    row.tint_color = group->tint_color & 0x00FFFFFFu;
    row.shape_count = group->shape_count;
    return row;
}

/* Record one BATCHED glass dispatch: one region, its body ids, the dispatch-wide
 * policy.
 *
 * Replaces the old one-body-per-dispatch recorder. The two things that were
 * per-body before — the parameter block and the barrier between bodies — are
 * gone: the parameters live in the body table and the bodies in a region are
 * evaluated in one dispatch, so their source-over order is the shader's loop
 * order and needs no image barrier. Regions are disjoint, so no dispatch observes
 * another's writes and the whole N-1 barrier chain disappears.
 *
 * `copy` describes what the lens samples (the sharp capture or the frost sheet)
 * and where those texels sit in capture space; `slab` is the output image, whose
 * extent is the slab's, not the capture's. */
typedef struct tessera_material_glass_copy {
    const optics_gfx_image *sharp;
    const optics_gfx_image *blurred;
    float capture_width;
    float capture_height;
    /* This slab's origin in capture space (capture = output + origin). */
    float slab_origin_x;
    float slab_origin_y;
} tessera_material_glass_copy;

static inline void tessera_material_glass_record_batch(
    VkCommandBuffer command, optics_gfx_compute_pipeline *pipeline, optics_gfx_device *device,
    const tessera_material_glass_copy *copy, optics_gfx_image *output, uint32_t slab_width,
    uint32_t slab_height, const tessera_material_region_batch *batch,
    const tessera_material_glass_policy *policy, uint64_t bodies_address, uint64_t ids_address) {
    liquid_glass_push push = {
        .input_handle = optics_gfx_image_bindless_handle(copy->sharp),
        .blurred_handle = optics_gfx_image_bindless_handle(copy->blurred),
        .sampler_handle = optics_gfx_device_default_sampler_handle(device),
        .output_handle = optics_gfx_image_bindless_storage_handle(output),
        .width = slab_width,
        .height = slab_height,
        .origin_x = batch->x,
        .origin_y = batch->y,
        .region_width = batch->width,
        .region_height = batch->height,
        .slab_origin_x = copy->slab_origin_x,
        .slab_origin_y = copy->slab_origin_y,
        .capture_width = copy->capture_width,
        .capture_height = copy->capture_height,
        .bodies_address_lo = tessera_material_address_lo(bodies_address),
        .bodies_address_hi = tessera_material_address_hi(bodies_address),
        .ids_address_lo = tessera_material_address_lo(ids_address),
        .ids_address_hi = tessera_material_address_hi(ids_address),
        .id_count = batch->id_count,
        .refraction = fmaxf(policy->refraction, 0.0f),
        .chromatic_aberration = fmaxf(policy->chromatic_aberration, 0.0f),
        .saturation = fmaxf(policy->saturation, 0.0f),
        .brightness = fmaxf(policy->brightness, 0.0f),
        .edge_width = fmaxf(policy->edge_width, 1.0f),
        .rim_light = fmaxf(policy->rim_light, 0.0f),
        .opacity = fminf(fmaxf(policy->opacity, 0.0f), 1.0f),
        .size_reference = fmaxf(policy->size_reference, 0.0f),
        .size_scale_min = fminf(fmaxf(policy->size_scale_min, 0.0f), 1.0f),
        .tint_strength = policy->tint_strength,
        .frost_strength = policy->frost_strength,
        .policy_curvature = policy->curvature,
        .contact_ao = policy->contact_ao,
        .ambient_fresnel = policy->ambient_fresnel,
        .light_x = policy->light_x,
        .light_y = policy->light_y,
    };
    uint32_t gx = (batch->width + TESSERA_MATERIAL_GLASS_WG - 1u) / TESSERA_MATERIAL_GLASS_WG;
    uint32_t gy = (batch->height + TESSERA_MATERIAL_GLASS_WG - 1u) / TESSERA_MATERIAL_GLASS_WG;
    optics_gfx_compute_dispatch(&(optics_gfx_compute_dispatch_opts){.cmd = command,
                                                                    .pipeline = pipeline,
                                                                    .push_constants = &push,
                                                                    .push_bytes = sizeof(push),
                                                                    .group_x = gx,
                                                                    .group_y = gy,
                                                                    .group_z = 1u});
}

#endif /* TESSERA_MATERIAL_GLASS_DISPATCH_H */
