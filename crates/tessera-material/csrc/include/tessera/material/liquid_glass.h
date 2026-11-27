/*
 * material/liquid_glass.h — analytic liquid-glass material.
 *
 * material is the material library of the optics stack. A named material —
 * its shader, its curve shapes, and its parameter contract — lives here,
 * built on gfx's public effect runtime (compute-writable images, frame
 * slots, the bindless heap). gfx owns rendering mechanism, material owns
 * material identity, and the caller owns policy (ADR-0063; the material
 * model is ADR-0046, the focus field ADR-0050).
 *
 * The filter is a reusable frame-slot-safe compositor. `input` is the
 * sharp backdrop capture and `blurred_input` is normally the output of
 * optics_gfx_blur_filter_apply for the same capture. Both inputs and the
 * returned image have the same extent. The output is transparent outside
 * the exact analytic SDF plus the drop-shadow falloff, so drawing it over
 * the sharp desktop performs the complete glass composite without a
 * separate rectangular clip.
 *
 * Threading: filters are not thread-safe per device. Serialize calls per
 * device, as you would for any other gfx recording API.
 */

#ifndef TESSERA_MATERIAL_LIQUID_GLASS_H
#define TESSERA_MATERIAL_LIQUID_GLASS_H

#include <tessera/material/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* One rounded-rectangle volume in capture-image pixel coordinates. */
typedef struct tessera_material_liquid_glass_shape {
    optics_gfx_rect bounds;
    float corner_radius;
} tessera_material_liquid_glass_shape;

/* One glass body: an array of one or more rounded-rectangle shapes,
 * smoothly fused when more than one. The count supported by this build
 * is returned by tessera_material_liquid_glass_max_shapes(); passing more shapes
 * than that fails the apply call instead of being silently truncated.
 * (Today that ceiling is 2 — the dispatch pushes two SDFs; the pointer
 * form exists so raising it later is a shader change, not an ABI
 * break.)
 *
 * `shapes` is a borrowed array: the caller owns it and only reads it
 * during the tessera_material_liquid_glass_filter_apply call.
 *
 * Per-body optical character is caller policy, used verbatim:
 * - shadow_alpha / shadow_blur / shadow_offset_y: the drop shadow cast by
 *   the body's own SDF (alpha 0 disables it);
 * - tint_color: 0xRRGGBB multiplier on the adaptive body tint, for
 *   accent-tinted glass (0xFFFFFF keeps the neutral smoke/pearl pair);
 * - focus / focus_strength: one soft optical emphasis field inside a
 *   single-shape body. It changes clarity and directional light without
 *   creating or outlining another glass body. Focus bounds must remain
 *   inside the first shape. Focus and smooth union are mutually exclusive
 *   because both reuse the secondary-shape shader slot.
 *
 * The trailing fields override dispatch-wide desc policy per body and
 * carry physical contact and optical profile parameters; each is armed by a non-negative value and
 * disabled/inherited by a negative one:
 * - frost_strength / tint_strength / saturation: per-body override of the
 *   same desc knob (<0 inherits the desc value; >=0 is used verbatim and
 *   clamped like the desc value);
 * - curvature: [0, 1] continuous curvature blend factor: 0 = rounded rect, 1 = G2 superellipse
 * (p=4);
 * - contact_ao: [0, 1] sub-pixel contact ambient occlusion (CAO) at silhouette boundary;
 * - ambient_fresnel: [0, 1] 360-degree isotropic environmental Fresnel sheen.
 *
 * Build groups from TESSERA_MATERIAL_LIQUID_GLASS_GROUP_INIT: zero-init is NOT inherit
 * — a zeroed group pins frost/tint/saturation to explicit zeros instead of
 * inheriting the desc values. The INIT's .shapes needs a caller-owned
 * array; the common single-body pattern is:
 *
 *   const tessera_material_liquid_glass_shape shapes[] = {{.bounds = {…}, .corner_radius = …}};
 *   tessera_material_liquid_glass_group g = TESSERA_MATERIAL_LIQUID_GLASS_GROUP_INIT;
 *   g.shapes = shapes;
 *   g.shape_count = 1;
 */
typedef struct tessera_material_liquid_glass_group {
    const tessera_material_liquid_glass_shape *shapes; /* borrowed, len = shape_count */
    uint32_t shape_count; /* 1 .. tessera_material_liquid_glass_max_shapes() */
    float blend_radius;
    float opacity;
    float shadow_alpha;
    float shadow_blur;
    float shadow_offset_y;
    uint32_t tint_color;
    tessera_material_liquid_glass_shape focus;
    float focus_strength;
    float frost_strength;  /* <0 = inherit desc value */
    float tint_strength;   /* <0 = inherit desc value */
    float saturation;      /* <0 = inherit desc value */
    float curvature;       /* [0, 1] continuous curvature (squircle) blend factor:
                              0 = Euclidean rounded rect, 1 = G2 superellipse (p=4);
                              <0 = inherit desc value */
    float contact_ao;      /* [0, 1] sub-pixel contact ambient occlusion (CAO) at
                              silhouette boundary; <0 = inherit desc value */
    float ambient_fresnel; /* [0, 1] 360-degree isotropic environmental Fresnel sheen;
                              <0 = inherit desc value */
} tessera_material_liquid_glass_group;

/* Neutral baseline for designated-initializer use: a single visible body
 * with no shadow and the neutral tint, so omitted fields can never turn
 * the glass black, and with every override/adaptive field in its
 * inherit/disabled (<0) state. Override the fields a body actually needs.
 * `shapes`/`shape_count` are NOT initialized — see the doc comment above
 * for the caller-owned-array pattern. */
#define TESSERA_MATERIAL_LIQUID_GLASS_GROUP_INIT                                                    \
    {.opacity = 1.0f,                                                                              \
     .tint_color = 0xFFFFFFu,                                                                      \
     .frost_strength = -1.0f,                                                                      \
     .tint_strength = -1.0f,                                                                       \
     .saturation = -1.0f,                                                                          \
     .curvature = -1.0f,                                                                           \
     .contact_ao = -1.0f,                                                                          \
     .ambient_fresnel = -1.0f}

/* Number of shapes this build supports per glass body (the dispatch
 * pushes one SDF per shape; the shader fuses them pairwise). Callers
 * that want to be forward-compatible size their arrays from this. */
TESSERA_MATERIAL_API uint32_t tessera_material_liquid_glass_max_shapes(void);

/* Dispatch-wide caller policy. Distances are capture-image pixels.
 * refraction controls the lens offset, chromatic_aberration separates the
 * RGB samples along the surface normal, edge_width controls the curved rim
 * thickness, and rim_light scales the whole rim lighting set (key line,
 * sheen, fresnel, shadow side, trough) — one knob for overall rim energy.
 * saturation and brightness are multipliers; opacity is additionally
 * multiplied by each group's opacity. Drop shadows are configured per
 * group — see tessera_material_liquid_glass_group.
 *
 * Rim band and lensing scale down for small bodies: size_reference is the
 * body size (small side, px) at which effects render at full strength —
 * 0 disables the scaling so every body uses the raw parameters — and
 * size_scale_min floors the factor. tint_strength and frost_strength
 * multiply the adaptive body tint and the scattering layer (1.0 = the
 * reference recipe), letting callers dial a body between clearer and
 * frostier without forking the material; saturation, tint_strength and
 * frost_strength are the dispatch-wide defaults that a group may override
 * per body (see tessera_material_liquid_glass_group). group_count may be zero (and
 * groups NULL) to clear any footprints retained by this frame slot after
 * all bodies disappear; otherwise it is capped at 64. */
typedef struct tessera_material_liquid_glass_desc {
    tessera_material_struct_type type; /* TESSERA_MATERIAL_TYPE_LIQUID_GLASS_DESC */
    const void *next;
    optics_gfx_image *input;
    optics_gfx_image *blurred_input;
    const tessera_material_liquid_glass_group *groups;
    uint32_t group_count;
    float refraction;
    float chromatic_aberration;
    float saturation;
    float brightness;
    float edge_width;
    float rim_light;
    optics_gfx_point light_direction;
    float opacity;
    float size_reference;
    float size_scale_min;
    float tint_strength;
    float frost_strength;
    float curvature;  /* [0, 1] continuous curvature (squircle) blend factor: 0 = rounded rect, 1 =
                         G2 superellipse */
    float contact_ao; /* [0, 1] Contact ambient occlusion default (default: 0.35) */
    float
        ambient_fresnel; /* [0, 1] Isotropic environmental Fresnel sheen default (default: 0.50) */
} tessera_material_liquid_glass_desc;

#define TESSERA_MATERIAL_LIQUID_GLASS_DESC_INIT                                                     \
    {.type = TESSERA_MATERIAL_TYPE_LIQUID_GLASS_DESC,                                               \
     .refraction = 8.0f,                                                                           \
     .chromatic_aberration = 1.25f,                                                                \
     .saturation = 1.08f,                                                                          \
     .brightness = 1.02f,                                                                          \
     .edge_width = 18.0f,                                                                          \
     .rim_light = 0.55f,                                                                           \
     .light_direction = {-0.45f, -0.89f},                                                          \
     .opacity = 1.0f,                                                                              \
     .size_reference = 72.0f,                                                                      \
     .size_scale_min = 0.15f,                                                                      \
     .tint_strength = 1.0f,                                                                        \
     .frost_strength = 1.0f,                                                                       \
     .curvature = 0.0f,                                                                            \
     .contact_ao = 0.35f,                                                                          \
     .ambient_fresnel = 0.50f}

typedef struct tessera_material_liquid_glass_filter tessera_material_liquid_glass_filter;

/* Options descriptor for tessera_material_liquid_glass_filter_warmup (ADR-0141). */
typedef struct tessera_material_liquid_glass_warmup_opts {
    optics_gfx_format format; /* Target image format (default 0 -> OPTICS_GFX_FORMAT_RGBA8_UNORM) */
    bool stats;               /* Pre-warm stats reduction pipeline */
} tessera_material_liquid_glass_warmup_opts;

TESSERA_MATERIAL_NODISCARD TESSERA_MATERIAL_API optics_gfx_result
tessera_material_liquid_glass_filter_create(optics_gfx_device *device,
                                           tessera_material_liquid_glass_filter **out);
/* Pre-warm compute pipelines ahead of time to eliminate cold-start JIT hitches (ADR-0141). */
TESSERA_MATERIAL_NODISCARD TESSERA_MATERIAL_API optics_gfx_result
tessera_material_liquid_glass_filter_warmup(tessera_material_liquid_glass_filter *filter,
                                           const tessera_material_liquid_glass_warmup_opts *opts);
TESSERA_MATERIAL_NODISCARD TESSERA_MATERIAL_API tessera_material_liquid_glass_filter *
tessera_material_liquid_glass_filter_retain(tessera_material_liquid_glass_filter *filter);
/* Release only after every submission that references the filter's outputs
 * has completed (its frame-slot fence has signalled, or after
 * optics_gfx_device_wait_idle): release destroys the filter's compute pipelines
 * inline. */
TESSERA_MATERIAL_API void
tessera_material_liquid_glass_filter_release(tessera_material_liquid_glass_filter *filter);
/* Paint every body into the material's output slab.
 *
 * Requires a recording frame with no active pass
 * (optics_gfx_frame_get_state(frame) == OPTICS_GFX_FRAME_STATE_RECORDING and
 * !optics_gfx_frame_has_active_pass(frame)).
 *
 * The output is sized to the union of the bodies' footprints, NOT to the input
 * capture, and `out->origin_x/origin_y` say where it belongs in capture pixels
 * (see tessera_material_output). Draw it at that placement; everything inside the
 * slab but outside the bodies is transparent, so drawing the whole slab is
 * always correct.
 *
 * Bodies are evaluated in caller order, and one dispatch covers each coalesced
 * region, so cost tracks coverage rather than body count.
 *
 * The returned image is borrowed from the filter and remains valid until the
 * same slot is applied again, the filter is released, or the required slab
 * extent changes. Do not release it. */
typedef struct tessera_material_liquid_glass_filter_apply_opts {
    optics_gfx_frame *frame;
    const tessera_material_liquid_glass_desc *desc;
    tessera_material_output *out;
} tessera_material_liquid_glass_filter_apply_opts;

TESSERA_MATERIAL_NODISCARD TESSERA_MATERIAL_API optics_gfx_result
tessera_material_liquid_glass_filter_apply(
    tessera_material_liquid_glass_filter *filter,
    const tessera_material_liquid_glass_filter_apply_opts *opts);

/* Per-group backdrop statistics, reduced on the GPU by
 * tessera_material_liquid_glass_filter_apply: the mean Rec.709 luminance of the
 * blurred backdrop over the group's primary body, and the mean
 * |sharp − blurred| luminance (high-frequency energy). What a caller does
 * with the numbers — mapping them onto plate_luminance / backdrop_energy or
 * the strength overrides, temporal smoothing — is caller policy; material only
 * measures. */
typedef struct tessera_material_backdrop_stat {
    float mean_luminance;
    float high_freq_energy;
} tessera_material_backdrop_stat;

/* Reads the stats this frame slot last submitted (OPTICS_GFX_MAX_FRAMES_IN_FLIGHT
 * frames ago): begin_frame has waited that slot's fence, so the mapped
 * buffer is stable while `frame` records. Call before this frame's apply on
 * the same slot — the numbers always describe the slot's previous
 * submission. out_count receives the number of group stats copied
 * (min of max_groups and the group count that submission carried). Returns
 * OPTICS_GFX_ERROR_INVALID_STATE when the slot has never been applied with
 * stats. */
typedef struct tessera_material_liquid_glass_filter_stats_opts {
    optics_gfx_frame *frame;
    tessera_material_backdrop_stat *out;
    uint32_t max_groups;
    uint32_t *out_count;
} tessera_material_liquid_glass_filter_stats_opts;

TESSERA_MATERIAL_NODISCARD TESSERA_MATERIAL_API optics_gfx_result
tessera_material_liquid_glass_filter_stats(
    tessera_material_liquid_glass_filter *filter,
    const tessera_material_liquid_glass_filter_stats_opts *opts);

#ifdef __cplusplus
}
#endif

#endif /* TESSERA_MATERIAL_LIQUID_GLASS_H */
