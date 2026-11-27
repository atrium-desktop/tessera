/*
 * tessera/material/types.h — common types and macros for Tessera's
 * backdrop materials, built on the Optics gfx effect runtime (ADR-0262).
 */

#ifndef TESSERA_MATERIAL_TYPES_H
#define TESSERA_MATERIAL_TYPES_H

#include <optics/gfx/core.h>
#include <optics/gfx/math.h>
#include <stdint.h>

#if defined(_WIN32) && !defined(TESSERA_MATERIAL_STATIC)
#ifdef TESSERA_MATERIAL_BUILDING
#define TESSERA_MATERIAL_API __declspec(dllexport)
#else
#define TESSERA_MATERIAL_API __declspec(dllimport)
#endif
#elif defined(__GNUC__) || defined(__clang__)
#define TESSERA_MATERIAL_API __attribute__((visibility("default")))
#else
#define TESSERA_MATERIAL_API
#endif

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 202311L
#define TESSERA_MATERIAL_NODISCARD [[nodiscard]]
#elif defined(__GNUC__) || defined(__clang__)
#define TESSERA_MATERIAL_NODISCARD __attribute__((warn_unused_result))
#elif defined(_MSC_VER)
#define TESSERA_MATERIAL_NODISCARD _Check_return_
#else
#define TESSERA_MATERIAL_NODISCARD
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* material descriptors open with `tessera_material_struct_type type; const void *next;`
 * following the same extension-chain pattern as gfx. The registry is
 * material-local: values are not shared with optics_gfx_struct_type. */
typedef enum tessera_material_struct_type : uint32_t {
    TESSERA_MATERIAL_TYPE_UNKNOWN = 0,
    TESSERA_MATERIAL_TYPE_LIQUID_GLASS_DESC = 1,
    TESSERA_MATERIAL_TYPE_FROSTED_DESC = 2,
    /* 3..6 belonged to materials Tessera does not carry (acrylic, layered
     * backdrop, mica); they stay reserved. */
    /* Append only. Never repurpose. */
} tessera_material_struct_type;
/* Quality and accessibility degradation hint. */
typedef enum tessera_material_quality : uint32_t {
    TESSERA_MATERIAL_QUALITY_FULL = 0,
    TESSERA_MATERIAL_QUALITY_LOW_POWER = 1,
    TESSERA_MATERIAL_QUALITY_OPAQUE_FALLBACK = 2,
} tessera_material_quality;

/*
 * A material's output: a slab-sized image plus where it belongs.
 *
 * A material only paints inside its own footprints, so its output image is
 * sized to those footprints (quantised up a size ladder), NOT to the input
 * capture. That is what keeps a 48-px glass titlebar from allocating three 33 MiB
 * RGBA16F images on a 4K window, and it makes a window resize reuse the same
 * image instead of re-creating one per pixel step.
 *
 * The cost is that `image` no longer covers the input extent, so the caller must
 * place it rather than blit it edge-to-edge:
 *
 *     optics_gfx_canvas_draw_image(canvas, out.image,
 *                            (optics_gfx_rect){out.origin_x, out.origin_y,
 *                                        (float)optics_gfx_image_width(out.image),
 *                                        (float)optics_gfx_image_height(out.image)},
 *                            nullptr);
 *
 * The dimensions above are capture pixels; both are whole numbers. `image` is
 * borrowed: it stays valid until the same filter is applied again on the same
 * frame slot, the filter is released, or the required slab extent changes. Do not
 * release it.
 *
 * Everything outside the material's footprints inside the slab is transparent,
 * so drawing the whole slab is always correct — the extra area contributes
 * nothing.
 */
typedef struct tessera_material_output {
    optics_gfx_image *image;
    float origin_x; /* slab top-left, capture pixels */
    float origin_y;
    uint32_t width; /* slab extent, capture pixels (== the image's extent) */
    uint32_t height;
} tessera_material_output;

#ifdef __cplusplus
}
#endif

#endif /* TESSERA_MATERIAL_TYPES_H */
