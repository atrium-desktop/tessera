#ifndef TESSERA_MATERIAL_LIQUID_GLASS_REGIONS_H
#define TESSERA_MATERIAL_LIQUID_GLASS_REGIONS_H

#include "rect.h"

#include <tessera/material/liquid_glass.h>

#include <math.h>
#include <stdbool.h>
#include <stdint.h>

#define LIQUID_GLASS_MAX_GROUPS 64u
#define LIQUID_GLASS_MAX_CLEAR_REGIONS (LIQUID_GLASS_MAX_GROUPS * 2u)

/* Shape capacity per glass body: the shader's SDF slots. Single fact
 * source for regions.h validation, the public max_shapes() accessor,
 * and (via the assert below) the dispatch's explicit shape0/shape1
 * copies — raising the ceiling means raising it here and teaching the
 * shader, never re-validating count literals in N places. */
#define LIQUID_GLASS_MAX_SHAPES 2u

static inline bool liquid_glass_finite_rect(optics_gfx_rect rect) {
    return isfinite(rect.x) && isfinite(rect.y) && isfinite(rect.w) && isfinite(rect.h) &&
           rect.w > 0.0f && rect.h > 0.0f;
}

static inline bool liquid_glass_rect_contains(optics_gfx_rect outer, optics_gfx_rect inner) {
    double outer_right = (double)outer.x + outer.w;
    double outer_bottom = (double)outer.y + outer.h;
    double inner_right = (double)inner.x + inner.w;
    double inner_bottom = (double)inner.y + inner.h;
    return inner.x >= outer.x && inner.y >= outer.y && inner_right <= outer_right &&
           inner_bottom <= outer_bottom;
}

/* Validate one body's geometry and optical policy. A positive focus is an
 * interior field of a single body, never a second body or a smooth-union
 * participant, so its bounds must remain inside the primary shape. Override
 * fields must be finite; contact_ao, ambient_fresnel, and curvature are sentinel-driven
 * (<0 disables/inherits), so only values above their [0,1] range are rejected. */
static inline bool liquid_glass_group_is_valid(const tessera_material_liquid_glass_group *group) {
    if (!group || group->shape_count < 1u || group->shape_count > LIQUID_GLASS_MAX_SHAPES ||
        !group->shapes || !isfinite(group->blend_radius) || !isfinite(group->opacity) ||
        !isfinite(group->shadow_alpha) || !isfinite(group->shadow_blur) ||
        !isfinite(group->shadow_offset_y) || !isfinite(group->focus_strength) ||
        !isfinite(group->frost_strength) || !isfinite(group->tint_strength) ||
        !isfinite(group->saturation) || !isfinite(group->curvature) ||
        !isfinite(group->contact_ao) || !isfinite(group->ambient_fresnel))
        return false;
    if (group->curvature > 1.0f || group->contact_ao > 1.0f || group->ambient_fresnel > 1.0f)
        return false;
    for (uint32_t j = 0; j < group->shape_count; ++j) {
        if (!liquid_glass_finite_rect(group->shapes[j].bounds) ||
            !isfinite(group->shapes[j].corner_radius))
            return false;
    }
    if (group->focus_strength > 0.0f &&
        (group->shape_count != 1u || !liquid_glass_finite_rect(group->focus.bounds) ||
         !isfinite(group->focus.corner_radius) ||
         !liquid_glass_rect_contains(group->shapes[0].bounds, group->focus.bounds)))
        return false;
    return true;
}

/* Per-group override resolution: a negative group value inherits the
 * dispatch-wide desc value, a non-negative one is used verbatim. */
static inline float liquid_glass_group_or_desc(float group_value, float desc_value) {
    return group_value >= 0.0f ? group_value : desc_value;
}

/* Conservative physical-pixel footprint of one analytic body, including the
 * smooth-union bow, antialiasing, and drop-shadow falloff. The result is
 * clipped to the storage image and is therefore directly dispatchable. */
static inline bool
liquid_glass_group_dispatch_bounds(const tessera_material_liquid_glass_group *group,
                                   float shadow_reach, uint32_t image_width, uint32_t image_height,
                                   tessera_material__rect *out) {
    if (!group || !out || group->shape_count < 1u || group->shape_count > LIQUID_GLASS_MAX_SHAPES ||
        image_width == 0u || image_height == 0u)
        return false;

    float x0 = group->shapes[0].bounds.x;
    float y0 = group->shapes[0].bounds.y;
    float x1 = x0 + group->shapes[0].bounds.w;
    float y1 = y0 + group->shapes[0].bounds.h;
    if (group->shape_count == 2u) {
        optics_gfx_rect second = group->shapes[1].bounds;
        x0 = fminf(x0, second.x);
        y0 = fminf(y0, second.y);
        x1 = fmaxf(x1, second.x + second.w);
        y1 = fmaxf(y1, second.y + second.h);
    }

    float pad = fmaxf(fmaxf(group->blend_radius, 0.0f), fmaxf(shadow_reach, 0.0f)) + 2.0f;
    int64_t ix0 = (int64_t)floorf(x0 - pad);
    int64_t iy0 = (int64_t)floorf(y0 - pad);
    int64_t ix1 = (int64_t)ceilf(x1 + pad);
    int64_t iy1 = (int64_t)ceilf(y1 + pad);
    ix0 = ix0 < 0 ? 0 : ix0;
    iy0 = iy0 < 0 ? 0 : iy0;
    ix1 = ix1 > (int64_t)image_width ? (int64_t)image_width : ix1;
    iy1 = iy1 > (int64_t)image_height ? (int64_t)image_height : iy1;
    if (ix1 <= ix0 || iy1 <= iy0)
        return false;

    *out = (tessera_material__rect){
        .x = (uint32_t)ix0,
        .y = (uint32_t)iy0,
        .width = (uint32_t)(ix1 - ix0),
        .height = (uint32_t)(iy1 - iy0),
    };
    return true;
}

/* Primary body bounds clipped to the image — the reduction window for
 * per-region backdrop statistics. A body that does not intersect the
 * capture yields a zero region, which the stats shader reports as zeros. */
static inline tessera_material__rect
liquid_glass_group_stats_bounds(const tessera_material_liquid_glass_group *group,
                                uint32_t image_width, uint32_t image_height) {
    tessera_material__rect region = {0u, 0u, 0u, 0u};
    if (!group || group->shape_count < 1u || image_width == 0u || image_height == 0u)
        return region;
    optics_gfx_rect bounds = group->shapes[0].bounds;
    float x0 = fmaxf(bounds.x, 0.0f);
    float y0 = fmaxf(bounds.y, 0.0f);
    float x1 = fminf(bounds.x + bounds.w, (float)image_width);
    float y1 = fminf(bounds.y + bounds.h, (float)image_height);
    if (x1 <= x0 || y1 <= y0)
        return region;
    uint32_t ix0 = (uint32_t)floorf(x0);
    uint32_t iy0 = (uint32_t)floorf(y0);
    uint32_t ix1 = (uint32_t)ceilf(x1);
    uint32_t iy1 = (uint32_t)ceilf(y1);
    return (tessera_material__rect){ix0, iy0, ix1 - ix0, iy1 - iy0};
}

static inline bool liquid_glass_regions_overlap(tessera_material__rect a, tessera_material__rect b) {
    uint64_t ar = (uint64_t)a.x + a.width;
    uint64_t ab = (uint64_t)a.y + a.height;
    uint64_t br = (uint64_t)b.x + b.width;
    uint64_t bb = (uint64_t)b.y + b.height;
    return (uint64_t)a.x < br && (uint64_t)b.x < ar && (uint64_t)a.y < bb && (uint64_t)b.y < ab;
}

/* True when `region` shares at least one pixel with any of `earlier[0..count)`.
 *
 * Used to decide whether a material dispatch needs an ordering barrier against
 * the dispatches already recorded. Vulkan's image barriers cover a whole
 * subresource, so the only alternative to "always barrier" is "prove the two
 * dispatches write disjoint memory". This is that proof, computed exactly:
 * dispatch footprints are conservative pixel bounds on everything a group
 * writes (including its shadow falloff), so a negative answer is sound. */
static inline bool liquid_glass_region_overlaps_any(tessera_material__rect region,
                                                    const tessera_material__rect *earlier,
                                                    uint32_t count) {
    for (uint32_t i = 0; i < count; ++i) {
        if (liquid_glass_regions_overlap(region, earlier[i]))
            return true;
    }
    return false;
}

static inline tessera_material__rect liquid_glass_region_union(tessera_material__rect a,
                                                              tessera_material__rect b) {
    uint32_t x0 = a.x < b.x ? a.x : b.x;
    uint32_t y0 = a.y < b.y ? a.y : b.y;
    uint64_t ar = (uint64_t)a.x + a.width;
    uint64_t ab = (uint64_t)a.y + a.height;
    uint64_t br = (uint64_t)b.x + b.width;
    uint64_t bb = (uint64_t)b.y + b.height;
    uint64_t x1 = ar > br ? ar : br;
    uint64_t y1 = ab > bb ? ab : bb;
    return (tessera_material__rect){
        .x = x0,
        .y = y0,
        .width = (uint32_t)(x1 - x0),
        .height = (uint32_t)(y1 - y0),
    };
}

static inline bool liquid_glass_add_clear_region(tessera_material__rect region,
                                                 tessera_material__rect *out, uint32_t capacity,
                                                 uint32_t *count) {
    /* Re-start after every union: the enlarged rectangle can now overlap an
     * earlier entry that did not overlap the original input. This computes a
     * transitive merge while leaving truly disjoint HUD/Dock regions apart. */
    for (uint32_t i = 0; i < *count;) {
        if (!liquid_glass_regions_overlap(region, out[i])) {
            ++i;
            continue;
        }
        region = liquid_glass_region_union(region, out[i]);
        out[i] = out[--(*count)];
        i = 0;
    }
    if (*count >= capacity)
        return false;
    out[(*count)++] = region;
    return true;
}

/* True when `region` is a non-degenerate rectangle fully inside the image. */
static inline bool liquid_glass_region_is_dispatchable(tessera_material__rect region,
                                                       uint32_t image_width,
                                                       uint32_t image_height) {
    return region.width != 0u && region.height != 0u && region.x < image_width &&
           region.y < image_height && region.width <= image_width - region.x &&
           region.height <= image_height - region.y;
}

/* Build the clear work for one frame slot. A newly allocated slot is cleared
 * exactly once over its complete extent. Reused slots clear previous AND
 * current footprints: previous removes pixels left by moved/shrunk/disappeared
 * groups, while current makes every overlapping group start from transparent.
 * All clears happen before any liquid dispatch, so a later group's clear can
 * never erase an earlier group's freshly written pixels.
 *
 * Overflow policy: `capacity` is the number of disjoint rectangles the caller
 * can dispatch, NOT a limit on how many footprints the material may carry.
 * Because `liquid_glass_add_clear_region` merges transitively, the coalesced
 * set can never exceed the input count, so there is always a valid answer that
 * fits — but the coalesced set can exceed `capacity` when the footprints are
 * pairwise disjoint (a 16-plate + 64-body layer at rest has 80 distinct
 * footprints, while a 64-body animation needs up to 128). Collapsing to the
 * full image in that case clears strictly more than required (correct, just
 * more expensive) and keeps the call succeeding: a legal descriptor must never
 * be rejected, and a rejected slot would fail on every later frame too, since
 * `slot->previous` stays at the state that caused the overflow. */
static inline bool
liquid_glass_build_clear_regions(bool initialized, uint32_t image_width, uint32_t image_height,
                                 const tessera_material__rect *previous, uint32_t previous_count,
                                 const tessera_material__rect *current, uint32_t current_count,
                                 tessera_material__rect *out, uint32_t capacity,
                                 uint32_t *out_count) {
    if (!out || !out_count || image_width == 0u || image_height == 0u || capacity == 0u ||
        (previous_count > 0u && !previous) || (current_count > 0u && !current))
        return false;

    *out_count = 0u;
    const tessera_material__rect whole = (tessera_material__rect){0u, 0u, image_width, image_height};
    if (!initialized) {
        out[0] = whole;
        *out_count = 1u;
        return true;
    }

    const tessera_material__rect *lists[2] = {previous, current};
    uint32_t counts[2] = {previous_count, current_count};
    for (uint32_t list = 0; list < 2u; ++list) {
        for (uint32_t i = 0; i < counts[list]; ++i) {
            tessera_material__rect region = lists[list][i];
            if (!liquid_glass_region_is_dispatchable(region, image_width, image_height))
                return false;
            if (!liquid_glass_add_clear_region(region, out, capacity, out_count)) {
                /* Coalesced set outgrew the dispatch budget: fall back to one
                 * full-image clear, which is a superset of every footprint. */
                out[0] = whole;
                *out_count = 1u;
                return true;
            }
        }
    }
    return true;
}

#endif /* TESSERA_MATERIAL_LIQUID_GLASS_REGIONS_H */
