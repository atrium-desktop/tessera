/*
 * material_observe.h — material filter accounting (ADR-0129, ADR-0134).
 *
 * Before this header, material filters were invisible to the ledger. Their host
 * storage comes from calloc/free, so nothing counted the filter itself — only
 * the frame-slot images and pipelines it holds, which are gfx objects and
 * therefore reported as `owner=gfx/core`. A leaked material filter was reported
 * as a gfx leak: the wrong library, the wrong operator, and no way to tell a
 * material leak from a renderer one. Misattribution is worse than silence.
 *
 * Kinds are per-material ("filter_mica", "filter_liquid_glass", …) rather than
 * one "filter" kind with a material field, because the ledger's kind is the
 * unit a leak lane maps to and the unit a reader acts on: "the Mica filter
 * leaked" is actionable, "a filter leaked" is not.
 *
 * Scope is DEVICE (the filter holds a retained device and GPU objects), the
 * lifetime is CONTEXT (a filter outlives frames and dies with the app's
 * material set).
 */

#ifndef TESSERA_MATERIAL_OBSERVE_H
#define TESSERA_MATERIAL_OBSERVE_H

#include <optics/gfx/observe.h>
#include <tessera/material/types.h>

#define TESSERA_MATERIAL_OBSERVE_OWNER "tessera/material"

/* The ledger row is now written by optics_gfx_device_alloc itself, with
 * TESSERA_MATERIAL_OBSERVE_OWNER / these kinds (ADR-0135). Counting here as well would
 * double every material allocation, which is exactly what happened when the
 * funnel was introduced — the lane caught it. The kind literals stay here
 * because they are the names both the funnel and the lane use. */

/* One literal per material so the guardrail's kind inventory and the call
 * sites cannot drift. */
#define TESSERA_MATERIAL_OBSERVE_KIND_MICA "filter_mica"
#define TESSERA_MATERIAL_OBSERVE_KIND_FROSTED "filter_frosted"
#define TESSERA_MATERIAL_OBSERVE_KIND_ACRYLIC "filter_acrylic"
#define TESSERA_MATERIAL_OBSERVE_KIND_LIQUID_GLASS "filter_liquid_glass"
#define TESSERA_MATERIAL_OBSERVE_KIND_BACKDROP_LAYER "filter_backdrop_layer"

#endif /* TESSERA_MATERIAL_OBSERVE_H */
