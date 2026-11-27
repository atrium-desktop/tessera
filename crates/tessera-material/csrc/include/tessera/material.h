/*
 * tessera/material.h — Tessera's backdrop materials (ADR-0262).
 *
 * Umbrella header. The materials run on the Optics gfx public effect runtime
 * (compute pipelines, compute-writable images, frame slots, the bindless
 * heap); Optics itself carries no named material since its ADR-0174.
 *
 *   - <tessera/material/frosted.h>       non-distorting frosted blur material
 *   - <tessera/material/liquid_glass.h>  convex-lens optical glass
 *   - <tessera/material/types.h>         common types and struct_type registry
 */

#ifndef TESSERA_MATERIAL_H
#define TESSERA_MATERIAL_H

#include <tessera/material/frosted.h>
#include <tessera/material/liquid_glass.h>
#include <tessera/material/types.h>

#endif /* TESSERA_MATERIAL_H */
