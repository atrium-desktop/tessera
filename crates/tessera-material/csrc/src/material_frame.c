/*
 * Machinery every material filter shares: the embedded shader table, lazy
 * pipeline creation, the slab-sized output image and the compute barriers.
 * See material_frame.h for the contracts.
 */

#include "material_frame.h"

#include <optics/gfx/compute.h>
#include <optics/gfx/profile.h>

#include <stdalign.h>
#include <stddef.h>

/* One embedded copy of each SPIR-V blob for the whole library. */
alignas(uint32_t) static const unsigned char liquid_glass_spv[] = {
#embed "liquid_glass.comp.spv"
};
alignas(uint32_t) static const unsigned char liquid_glass_16f_spv[] = {
#embed "liquid_glass_16f.comp.spv"
};
alignas(uint32_t) static const unsigned char frosted_spv[] = {
#embed "frosted.comp.spv"
};
alignas(uint32_t) static const unsigned char frosted_16f_spv[] = {
#embed "frosted_16f.comp.spv"
};
alignas(uint32_t) static const unsigned char storage_clear_spv[] = {
#embed "storage_clear.comp.spv"
};
alignas(uint32_t) static const unsigned char storage_clear_16f_spv[] = {
#embed "storage_clear_16f.comp.spv"
};
alignas(uint32_t) static const unsigned char backdrop_stats_spv[] = {
#embed "backdrop_stats.comp.spv"
};

typedef struct shader_blob {
    const unsigned char *bytes;
    size_t size;
} shader_blob;

#define BLOB(a) {.bytes = (a), .size = sizeof(a)}

/* [shader][storage class]: 0 = rgba8, 1 = rgba16f. A sampling-only shader
 * has one variant and repeats it in both columns. */
static const shader_blob shader_table[TESSERA_MATERIAL__SHADER_COUNT][2] = {
    [TESSERA_MATERIAL__SHADER_LIQUID_GLASS] = {BLOB(liquid_glass_spv), BLOB(liquid_glass_16f_spv)},
    [TESSERA_MATERIAL__SHADER_FROSTED] = {BLOB(frosted_spv), BLOB(frosted_16f_spv)},
    [TESSERA_MATERIAL__SHADER_STORAGE_CLEAR] = {BLOB(storage_clear_spv),
                                               BLOB(storage_clear_16f_spv)},
    [TESSERA_MATERIAL__SHADER_BACKDROP_STATS] = {BLOB(backdrop_stats_spv), BLOB(backdrop_stats_spv)},
};

#undef BLOB

optics_gfx_result tessera_material__pipeline_ensure(optics_gfx_device *device,
                                                   optics_gfx_compute_pipeline **pipeline,
                                                   const tessera_material__pipeline_opts *opts) {
    if (!device || !pipeline || !opts || opts->shader >= TESSERA_MATERIAL__SHADER_COUNT)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    if (*pipeline)
        return OPTICS_GFX_OK;
    OPTICS_GFX_PROFILE_ZONE(z_compile, "material.compile_pipeline");
    const shader_blob *blob = &shader_table[opts->shader][opts->is16f ? 1 : 0];
    optics_gfx_compute_pipeline_desc pdesc = OPTICS_GFX_COMPUTE_PIPELINE_DESC_INIT;
    pdesc.spirv = (const uint32_t *)blob->bytes;
    pdesc.spirv_word_count = blob->size / sizeof(uint32_t);
    pdesc.entry_point = "main";
    pdesc.push_constant_bytes = opts->push_constant_bytes;
    optics_gfx_result r = optics_gfx_compute_pipeline_create(device, &pdesc, pipeline);
    OPTICS_GFX_PROFILE_ZONE_END(z_compile);
    return r;
}

/* Push block of storage_clear.comp. */
typedef struct storage_clear_push {
    uint32_t output_handle;
    uint32_t width;
    uint32_t height;
    uint32_t origin_x;
    uint32_t origin_y;
    uint32_t region_width;
    uint32_t region_height;
} storage_clear_push;
static_assert(sizeof(storage_clear_push) == 28, "storage_clear_push no longer matches its shader");

uint32_t tessera_material__storage_clear_push_bytes(void) {
    return (uint32_t)sizeof(storage_clear_push);
}

void tessera_material__clear_regions(const tessera_material__clear_opts *opts) {
    if (!opts || !opts->pipeline || !opts->image || opts->cmd == VK_NULL_HANDLE)
        return;
    const uint32_t width = optics_gfx_image_width(opts->image);
    const uint32_t height = optics_gfx_image_height(opts->image);
    const tessera_material__rect whole = {.width = width, .height = height};
    const tessera_material__rect *regions = opts->regions ? opts->regions : &whole;
    const uint32_t count = opts->regions ? opts->region_count : 1u;
    const uint32_t handle = optics_gfx_image_bindless_storage_handle(opts->image);
    for (uint32_t i = 0; i < count; ++i) {
        storage_clear_push pc = {
            .output_handle = handle,
            .width = width,
            .height = height,
            .origin_x = regions[i].x,
            .origin_y = regions[i].y,
            .region_width = regions[i].width,
            .region_height = regions[i].height,
        };
        optics_gfx_compute_dispatch(
            &(optics_gfx_compute_dispatch_opts){.cmd = opts->cmd,
                                                .pipeline = opts->pipeline,
                                                .push_constants = &pc,
                                                .push_bytes = sizeof(pc),
                                                .group_x = tessera_material_groups(pc.region_width),
                                                .group_y = tessera_material_groups(pc.region_height),
                                                .group_z = 1u});
    }
}

optics_gfx_format tessera_material__output_format(const optics_gfx_image *input) {
    return optics_gfx_image_format(input) == OPTICS_GFX_FORMAT_RGBA16_SFLOAT
               ? OPTICS_GFX_FORMAT_RGBA16_SFLOAT
               : OPTICS_GFX_FORMAT_RGBA8_UNORM;
}

optics_gfx_result tessera_material__slab_ensure(optics_gfx_device *device,
                                               tessera_material__slab *slab,
                                               const tessera_material__slab_opts *opts,
                                               bool *out_recreated) {
    if (!device || !slab || !opts || !opts->input || !out_recreated)
        return OPTICS_GFX_ERROR_INVALID_ARGUMENT;
    *out_recreated = false;
    uint32_t width = 0u;
    uint32_t height = 0u;
    tessera_material_slab_for_footprint(opts->footprint_width, opts->footprint_height, &width,
                                       &height);
    optics_gfx_format format = tessera_material__output_format(opts->input);
    if (slab->image && slab->width == width && slab->height == height && slab->format == format)
        return OPTICS_GFX_OK;

    tessera_material__slab_release(slab);
    *out_recreated = true;
    optics_gfx_result r = optics_gfx_image_create(
        device,
        &(optics_gfx_image_desc){.type = OPTICS_GFX_TYPE_IMAGE_DESC,
                                 .width = width,
                                 .height = height,
                                 .format = format,
                                 .usage = OPTICS_GFX_IMAGE_USAGE_COMPUTE_WRITE},
        &slab->image);
    if (r != OPTICS_GFX_OK) {
        slab->image = nullptr;
        return r;
    }
    slab->width = width;
    slab->height = height;
    slab->format = format;
    return OPTICS_GFX_OK;
}

void tessera_material__slab_release(tessera_material__slab *slab) {
    if (!slab)
        return;
    if (slab->image)
        optics_gfx_image_release(slab->image);
    *slab = (tessera_material__slab){};
}

/* ------------------------------------------------------------------ */
/*  Barriers                                                          */
/* ------------------------------------------------------------------ */

/* Every material image stays in GENERAL; only the hazard changes. */
static void image_barrier(VkCommandBuffer cmd, VkImage image, VkPipelineStageFlags2 src_stage,
                          VkAccessFlags2 src_access, VkPipelineStageFlags2 dst_stage,
                          VkAccessFlags2 dst_access) {
    VkImageMemoryBarrier2 b = {
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER_2,
        .srcStageMask = src_stage,
        .srcAccessMask = src_access,
        .dstStageMask = dst_stage,
        .dstAccessMask = dst_access,
        .oldLayout = VK_IMAGE_LAYOUT_GENERAL,
        .newLayout = VK_IMAGE_LAYOUT_GENERAL,
        .image = image,
        .subresourceRange =
            {
                .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
                .levelCount = 1,
                .layerCount = 1,
            },
    };
    VkDependencyInfo di = {
        .sType = VK_STRUCTURE_TYPE_DEPENDENCY_INFO,
        .imageMemoryBarrierCount = 1,
        .pImageMemoryBarriers = &b,
    };
    vkCmdPipelineBarrier2(cmd, &di);
}

void tessera_material__barrier_compute_write_to_read(VkCommandBuffer cmd, VkImage image) {
    image_barrier(cmd, image, VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
                  VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT,
                  VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT | VK_PIPELINE_STAGE_2_FRAGMENT_SHADER_BIT,
                  VK_ACCESS_2_SHADER_SAMPLED_READ_BIT | VK_ACCESS_2_SHADER_STORAGE_READ_BIT);
}

void tessera_material__barrier_reuse_to_compute_write(VkCommandBuffer cmd, VkImage image) {
    /* The previous frame wrote the image in compute and read it in compute or
     * fragment. The write-after-read hazard needs only the stage dependency;
     * the write-after-write hazard needs the earlier write in the source
     * access scope. Read bits there would be no-ops. */
    image_barrier(cmd, image,
                  VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT | VK_PIPELINE_STAGE_2_FRAGMENT_SHADER_BIT,
                  VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT, VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
                  VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT);
}

void tessera_material__barrier_compute_write_to_read_write(VkCommandBuffer cmd, VkImage image) {
    image_barrier(cmd, image, VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
                  VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT, VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
                  VK_ACCESS_2_SHADER_STORAGE_READ_BIT | VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT);
}

void tessera_material__barrier_compute_write_to_sampled_and_storage(VkCommandBuffer cmd,
                                                                   VkImage image) {
    image_barrier(cmd, image, VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
                  VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT, VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
                  VK_ACCESS_2_SHADER_SAMPLED_READ_BIT | VK_ACCESS_2_SHADER_STORAGE_READ_BIT |
                      VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT);
}

void tessera_material__barrier_buffer_compute_write_to_compute(VkCommandBuffer cmd,
                                                              VkBuffer buffer) {
    VkBufferMemoryBarrier2 b = {
        .sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER_2,
        .srcStageMask = VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
        .srcAccessMask = VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT,
        .dstStageMask = VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
        .dstAccessMask = VK_ACCESS_2_SHADER_STORAGE_READ_BIT | VK_ACCESS_2_SHADER_STORAGE_WRITE_BIT,
        .buffer = buffer,
        .size = VK_WHOLE_SIZE,
    };
    VkDependencyInfo di = {
        .sType = VK_STRUCTURE_TYPE_DEPENDENCY_INFO,
        .bufferMemoryBarrierCount = 1,
        .pBufferMemoryBarriers = &b,
    };
    vkCmdPipelineBarrier2(cmd, &di);
}
