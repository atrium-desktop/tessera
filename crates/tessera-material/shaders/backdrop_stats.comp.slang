// backdrop_stats.comp.slang
// Per-region backdrop statistics for the liquid-glass material (ADR-0065, ADR-0156).

[[vk::binding(0, 0)]] Texture2D u_sampled[];
[[vk::binding(2, 0)]] SamplerState u_samplers[];

struct BackdropStatsPush {
    uint input_handle;
    uint blurred_handle;
    uint sampler_handle;
    uint width;
    uint height;
    uint rect_x;
    uint rect_y;
    uint rect_width;
    uint rect_height;
    uint group_index;
    uint stats_address_lo;
    uint stats_address_hi;
};
[[vk::push_constant]] BackdropStatsPush pc;

groupshared float partial_luminance[256];
groupshared float partial_energy[256];
groupshared uint partial_count[256];

float luminance_rec709(float3 colour)
{
    return dot(colour, float3(0.2126, 0.7152, 0.0722));
}

float3 sample_rgb(uint handle, float2 uv)
{
    return u_sampled[NonUniformResourceIndex(handle & 0x0FFFFFFFu)].SampleLevel(
        u_samplers[NonUniformResourceIndex(pc.sampler_handle & 0x0FFFFFFFu)],
        clamp(uv, float2(0.0), float2(1.0)),
        0.0
    ).rgb;
}

[numthreads(16, 16, 1)]
void main(uint3 groupThreadID : SV_GroupThreadID)
{
    uint pixel_count = pc.rect_width * pc.rect_height;
    uint stride = max(uint(ceil(sqrt(float(pixel_count) / 65536.0))), 1u);

    float2 inv_extent = 1.0 / float2(pc.width, pc.height);
    uint rect_end_x = pc.rect_x + pc.rect_width;
    uint rect_end_y = pc.rect_y + pc.rect_height;
    float luminance_sum = 0.0;
    float energy_sum = 0.0;
    uint count = 0u;

    for (uint y = pc.rect_y + groupThreadID.y * stride; y < rect_end_y; y += 16u * stride) {
        for (uint x = pc.rect_x + groupThreadID.x * stride; x < rect_end_x; x += 16u * stride) {
            float2 uv = (float2(x, y) + 0.5) * inv_extent;
            float blurred_lum = luminance_rec709(sample_rgb(pc.blurred_handle, uv));
            float sharp_lum = luminance_rec709(sample_rgb(pc.input_handle, uv));
            luminance_sum += blurred_lum;
            energy_sum += abs(sharp_lum - blurred_lum);
            ++count;
        }
    }

    uint lane = groupThreadID.y * 16u + groupThreadID.x;
    partial_luminance[lane] = luminance_sum;
    partial_energy[lane] = energy_sum;
    partial_count[lane] = count;
    GroupMemoryBarrierWithGroupSync();

    for (uint offset = 128u; offset > 0u; offset >>= 1u) {
        if (lane < offset) {
            partial_luminance[lane] += partial_luminance[lane + offset];
            partial_energy[lane] += partial_energy[lane + offset];
            partial_count[lane] += partial_count[lane + offset];
        }
        GroupMemoryBarrierWithGroupSync();
    }

    if (lane == 0u) {
        float2 result = partial_count[0] > 0u
                            ? float2(partial_luminance[0], partial_energy[0]) / float(partial_count[0])
                            : float2(0.0);
        uint64_t addr = (uint64_t(pc.stats_address_hi) << 32) | uint64_t(pc.stats_address_lo);
        float2* stats = (float2*)addr;
        stats[pc.group_index] = result;
    }
}
