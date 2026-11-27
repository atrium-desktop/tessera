#ifndef TESSERA_MATERIAL_RECT_H
#define TESSERA_MATERIAL_RECT_H

#include <stdint.h>

/* An integer pixel rectangle inside a material image or capture: dispatch
 * regions, clear regions and slab footprints. */
typedef struct tessera_material__rect {
    uint32_t x;
    uint32_t y;
    uint32_t width;
    uint32_t height;
} tessera_material__rect;

#endif /* TESSERA_MATERIAL_RECT_H */
