// storage_clear.comp.slang
// Region-limited transparent clear for persistent material outputs (ADR-0069, ADR-0156).

[[vk::binding(1, 0)]]
#ifdef TESSERA_MATERIAL_STORAGE_RGBA16F
[[vk::image_format("rgba16f")]]
#else
[[vk::image_format("rgba8")]]
#endif
RWTexture2D<float4> u_storage[];

struct StorageClearPush {
    uint output_handle;
    uint width;
    uint height;
    uint origin_x;
    uint origin_y;
    uint region_width;
    uint region_height;
};
[[vk::push_constant]] StorageClearPush pc;

[numthreads(16, 16, 1)]
void main(uint3 dispatchThreadID : SV_DispatchThreadID)
{
    uint2 local = dispatchThreadID.xy;
    if (local.x >= pc.region_width || local.y >= pc.region_height) return;
    uint2 pixel = local + uint2(pc.origin_x, pc.origin_y);
    if (pixel.x >= pc.width || pixel.y >= pc.height) return;
    u_storage[NonUniformResourceIndex(pc.output_handle & 0x0FFFFFFFu)][pixel] = float4(0.0);
}
