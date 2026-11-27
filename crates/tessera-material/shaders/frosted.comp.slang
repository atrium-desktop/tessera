// frosted.comp.slang
// Classic non-distorting frosted glass material shader (ADR-0063, ADR-0156).

#include "optics/gfx/shaders/optics_gfx_sdf.slang"

[[vk::binding(0, 0)]] Texture2D u_sampled[];
[[vk::binding(1, 0)]]
#ifdef TESSERA_MATERIAL_STORAGE_RGBA16F
[[vk::image_format("rgba16f")]]
#else
[[vk::image_format("rgba8")]]
#endif
RWTexture2D<float4> u_storage[];
[[vk::binding(2, 0)]] SamplerState u_samplers[];

struct FrostedPush {
    uint input_handle;
    uint blurred_handle;
    uint sampler_handle;
    uint output_handle;
    uint width;
    uint height;
    uint origin_x;
    uint origin_y;
    float4 bounds;
    float corner_radius;
    float opacity;
    float saturation;
    float tint_strength;
    float noise_intensity;
    float shadow_alpha;
    float shadow_blur;
    float shadow_offset_y;
    uint tint_color;
    uint group_width;
    uint group_height;
    float slab_origin_x;
    float slab_origin_y;
    float capture_width;
    float capture_height;
};
[[vk::push_constant]] FrostedPush pc;

float3 unpack_rgb(uint c)
{
    return float3(
        float((c >> 16u) & 0xFFu) / 255.0,
        float((c >> 8u) & 0xFFu) / 255.0,
        float(c & 0xFFu) / 255.0
    );
}

float prng_dither(float2 uv)
{
    float n = frac(sin(dot(uv, float2(12.9898, 78.233))) * 43758.5453);
    return (n - 0.5);
}

float4 sample_image(uint handle, float2 uv)
{
    return u_sampled[NonUniformResourceIndex(handle & 0x0FFFFFFFu)].SampleLevel(
        u_samplers[NonUniformResourceIndex(pc.sampler_handle & 0x0FFFFFFFu)],
        clamp(uv, float2(0.0), float2(1.0)),
        0.0
    );
}

void store_source_over(uint2 pixel, float4 source)
{
    uint handle = NonUniformResourceIndex(pc.output_handle & 0x0FFFFFFFu);
    float4 destination = u_storage[handle][pixel];
    u_storage[handle][pixel] = source + destination * (1.0 - source.a);
}

float rounded_rect_sdf(float2 p, float4 bounds, float requested_radius)
{
    float2 half_extent = max(bounds.zw * 0.5, float2(0.001));
    float2 centre = bounds.xy + half_extent;
    return optics_gfx_sdf_round_rect(p - centre, half_extent, requested_radius);
}

[numthreads(16, 16, 1)]
void main(uint3 dispatchThreadID : SV_DispatchThreadID)
{
    uint2 local = dispatchThreadID.xy;
    if (local.x >= pc.group_width || local.y >= pc.group_height) return;

    uint2 pixel = local + uint2(pc.origin_x, pc.origin_y);
    if (pixel.x >= pc.width || pixel.y >= pc.height) return;

    float2 p = float2(pixel) + float2(0.5) + float2(pc.slab_origin_x, pc.slab_origin_y);
    float2 uv = p / max(float2(pc.capture_width, pc.capture_height), float2(1.0));

    float d = rounded_rect_sdf(p, pc.bounds, pc.corner_radius);

    // Drop shadow computation: solid inside shadow footprint (sd <= 0), Gaussian falloff outside (sd > 0)
    float shadow_cov = 0.0;
    if (pc.shadow_alpha > 0.001 && pc.shadow_blur > 0.0) {
        float2 sp = p - float2(0.0, pc.shadow_offset_y);
        float sd = rounded_rect_sdf(sp, pc.bounds, pc.corner_radius);
        float sigma = max(pc.shadow_blur * 0.5, 0.5);
        shadow_cov = pc.shadow_alpha * (sd <= 0.0 ? 1.0 : exp(-0.5 * (sd * sd) / (sigma * sigma)));
    }

    if (d > 1.5 && shadow_cov <= 0.001) return;

    // Body coverage with 1px smooth antialiasing
    float body_cov = clamp(0.5 - d, 0.0, 1.0) * pc.opacity;

    // Sample blurred backdrop
    float4 blurred = sample_image(pc.blurred_handle, uv);

    // Boost saturation
    float lum = dot(blurred.rgb, float3(0.2126, 0.7152, 0.0722));
    float3 sat_rgb = lerp(float3(lum), blurred.rgb, pc.saturation);

    // Blend tint
    float3 tint = unpack_rgb(pc.tint_color);
    float3 tinted = lerp(sat_rgb, tint, clamp(pc.tint_strength, 0.0, 1.0));

    // Dither against banding
    float grain = prng_dither(float2(pixel)) * pc.noise_intensity;
    float3 body_rgb = clamp(tinted + float3(grain), 0.0, 1.0);

    // Combine body and drop shadow
    float4 body_out = float4(body_rgb * body_cov, body_cov);
    float4 shadow_out = float4(float3(0.0), shadow_cov * (1.0 - body_cov));

    float4 final_color = body_out + shadow_out;
    if (final_color.a > 0.001) {
        store_source_over(pixel, final_color);
    }
}
