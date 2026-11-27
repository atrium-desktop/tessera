// liquid_glass.comp.slang
// Analytic liquid-glass compositor — convex-lens model (ADR-0063, ADR-0148, ADR-0156).

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

struct GlassBody {
    float4 shape0;
    float4 shape1;
    float4 focus_shape;
    float radius0;
    float radius1;
    float focus_radius;
    float blend_radius;
    float opacity;
    float shadow_alpha;
    float shadow_blur;
    float shadow_offset_y;
    float tint_strength;
    float frost_strength;
    float saturation;
    float curvature;
    float contact_ao;
    float ambient_fresnel;
    float size_reference;
    float size_scale_min;
    float light_x;
    float light_y;
    uint tint_color;
    uint shape_count;
    uint has_focus;
    uint reserved;
    /* Pad to TESSERA_MATERIAL_BODY_STRIDE (256 bytes). */
    uint pad0[2]; /* payload 136 -> 144: 16-align */
    float4 pad1[7]; /* 112 more -> 256 = TESSERA_MATERIAL_BODY_STRIDE */
};

struct LiquidGlassPush {
    uint input_handle;
    uint blurred_handle;
    uint sampler_handle;
    uint output_handle;
    uint width;          /* output slab extent */
    uint height;
    uint origin_x;       /* this dispatch's region inside the slab */
    uint origin_y;
    uint region_width;
    uint region_height;
    float slab_origin_x; /* this slab's origin in capture space */
    float slab_origin_y;
    float capture_width;
    float capture_height;
    uint bodies_address_lo; /* body table, rows of GlassBody */
    uint bodies_address_hi;
    uint ids_address_lo;    /* this region's body ids, in source-over order */
    uint ids_address_hi;
    uint id_count;
    uint reserved0;
    /* Dispatch-wide optical policy. */
    float refraction;
    float chromatic_aberration;
    float saturation;
    float brightness;
    float edge_width;
    float rim_light;
    float opacity;
    float size_reference;
    float size_scale_min;
    float tint_strength;
    float frost_strength;
    float policy_curvature;
    float contact_ao;
    float ambient_fresnel;
    float light_x;
    float light_y;
};
[[vk::push_constant]] LiquidGlassPush pc;

GlassBody body_at(uint index)
{
    uint64_t addr = (uint64_t(pc.bodies_address_hi) << 32) | uint64_t(pc.bodies_address_lo);
    GlassBody* bodies = (GlassBody*)addr;
    return bodies[index];
}

uint body_id(uint index)
{
    uint64_t addr = (uint64_t(pc.ids_address_hi) << 32) | uint64_t(pc.ids_address_lo);
    uint* ids = (uint*)addr;
    return ids[index];
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

float2 squircle_centre(float4 bounds)
{
    float2 half_extent = max(bounds.zw * 0.5, float2(0.001));
    return bounds.xy + half_extent;
}

float2 squircle_half(float4 bounds)
{
    return max(bounds.zw * 0.5, float2(0.001));
}

float squircle_sdf(float2 p, float4 bounds, float requested_radius, float curvature)
{
    return optics_gfx_sdf_squircle(p - squircle_centre(bounds), squircle_half(bounds), requested_radius,
                             curvature);
}

float smooth_union(float a, float b, float radius)
{
    float h;
    return optics_gfx_smooth_union(float2(a, b), radius, h);
}

float glass_sdf(float2 p, GlassBody b)
{
    float cur = b.curvature;
    float d = squircle_sdf(p, b.shape0, b.radius0, cur);
    if (b.shape_count > 1u) {
        d = smooth_union(d, squircle_sdf(p, b.shape1, b.radius1, cur), b.blend_radius);
    }
    return d;
}

float2 glass_normal(float2 p, GlassBody b, float aa)
{
    float cur = b.curvature;
    float3 s0 = optics_gfx_sdf_squircle_sample(p - squircle_centre(b.shape0), squircle_half(b.shape0),
                                        b.radius0, cur, aa);
    if (b.shape_count <= 1u) return s0.yz;

    float3 s1 = optics_gfx_sdf_squircle_sample(p - squircle_centre(b.shape1), squircle_half(b.shape1),
                                        b.radius1, cur, aa);
    return optics_gfx_smooth_union_gradient(s0.yz, s1.yz, float2(s0.x, s1.x), b.blend_radius);
}

float focus_field(float2 p, GlassBody b)
{
    if (b.has_focus == 0u) return 0.0;
    float d = squircle_sdf(p, b.focus_shape, b.focus_radius, b.curvature);
    float small_side = min(b.focus_shape.z, b.focus_shape.w);
    float feather = clamp(small_side * 0.28, 14.0, 36.0);
    return 1.0 - smoothstep(-feather, feather, d);
}

float3 adjust_saturation(float3 colour, float saturation)
{
    float luminance = dot(colour, float3(0.2126, 0.7152, 0.0722));
    return lerp(float3(luminance), colour, saturation);
}

void shade_body(uint2 pixel, float2 p, float2 uv, GlassBody b)
{
    float d = glass_sdf(p, b);
    float focus = focus_field(p, b);
    float coverage = 1.0 - smoothstep(-0.75, 0.75, d);

    float body_min = min(b.shape0.z, b.shape0.w);
    float2 size_ref_min = float2(b.size_reference, b.size_scale_min);
    float size_scale = 1.0;
    if (size_ref_min.x > 0.0) {
        size_scale = clamp(body_min / size_ref_min.x, size_ref_min.y, 1.0);
    }

    float shadow_blur = b.shadow_blur;
    float shadow_offset_y = b.shadow_offset_y;
    float shadow = 0.0;
    if (b.shadow_alpha > 0.0) {
        float sd = glass_sdf(p - float2(0.0, shadow_offset_y), b);
        shadow = (1.0 - smoothstep(0.0, shadow_blur * 2.0, max(sd, 0.0))) * b.shadow_alpha;
    }
    shadow *= clamp(b.opacity, 0.0, 1.0);
    if (coverage <= 0.0) {
        if (shadow <= 0.002) return;
        store_source_over(pixel, float4(0.0, 0.0, 0.0, shadow));
        return;
    }

    float max_edge = max(body_min * 0.35, 1.0);
    float edge = clamp(pc.edge_width * size_scale, 1.0, max_edge);
    float u = clamp(1.0 + d / edge, 0.0, 1.0);
    float lens = min(u * rsqrt(1.0 - u * u + 0.02), 3.0) * (1.0 / 3.0);

    float2 rim_normal = float2(0.0, 1.0);
    if (u > 0.0) {
        rim_normal = glass_normal(p, b, 1.0);
    }

    float2 inv_extent = 1.0 / max(float2(pc.capture_width, pc.capture_height), float2(1.0));
    float2 normal_uv = rim_normal * inv_extent;

    float refraction = pc.refraction * size_scale;
    float chromatic = pc.chromatic_aberration * size_scale;
    float bend_g = refraction * lens;
    float bend_r = (refraction + chromatic) * lens;
    float bend_b = max(refraction - chromatic, 0.0) * lens;
    float3 refracted;
    if (lens > 0.0001) {
        refracted.r = sample_image(pc.input_handle, uv - normal_uv * bend_r).r;
        refracted.g = sample_image(pc.input_handle, uv - normal_uv * bend_g).g;
        refracted.b = sample_image(pc.input_handle, uv - normal_uv * bend_b).b;
    } else {
        refracted = sample_image(pc.input_handle, uv).rgb;
    }
    float3 frost = sample_image(pc.blurred_handle, uv - normal_uv * bend_g * 0.5).rgb;

    float2 strengths = float2(b.tint_strength, b.frost_strength);
    float tint_strength = strengths.x;
    float frost_strength = strengths.y;
    float interior_frost = frost_strength <= 1.0
                               ? 0.08 * frost_strength
                               : clamp(0.08 + (frost_strength - 1.0) * 0.20, 0.0, 1.0);
    float frost_amount =
        clamp(interior_frost + 0.20 * u * u * min(frost_strength, 1.0), 0.0, 1.0);
    frost_amount *= 1.0 - 0.35 * focus;
    float3 colour = lerp(refracted, frost, frost_amount);
    colour = adjust_saturation(colour, max(b.saturation, 0.0));

    float2 optics = float2(b.contact_ao, b.ambient_fresnel);
    float contact_ao = optics.x;
    float ambient_fresnel = optics.y;

    float backdrop_luma = dot(frost, float3(0.2126, 0.7152, 0.0722));
    float dark_ratio = 1.0 - smoothstep(0.15, 0.65, backdrop_luma);
    float tint_profile = (0.65 + 0.35 * (1.0 - u)) * tint_strength * (1.0 - 0.25 * focus);

    float3 tint_rgb = float3((b.tint_color >> 16) & 0xFFu, (b.tint_color >> 8) & 0xFFu,
                             b.tint_color & 0xFFu) / 255.0;

    float3 refl_floor = float3(1.0) * tint_rgb * (0.10 * dark_ratio * tint_profile);

    float trans_atten = lerp(0.24, 0.08, dark_ratio) * tint_profile;
    float3 trans_color = colour * lerp(float3(0.65), tint_rgb, 0.35);
    colour = lerp(colour, trans_color, trans_atten) + refl_floor;
    colour *= max(pc.brightness, 0.0);

    if (focus > 0.0) {
        float2 focus_uv = clamp((p - b.focus_shape.xy) / max(b.focus_shape.zw, float2(1.0)), float2(0.0), float2(1.0));
        float focus_key = clamp(1.05 - dot(focus_uv, float2(0.35, 0.65)), 0.0, 1.0);
        float focus_luminance = dot(colour, float3(0.2126, 0.7152, 0.0722));
        colour = lerp(float3(focus_luminance), colour, 1.0 + 0.08 * focus);
        colour *= 1.0 + 0.018 * focus * (0.40 + 0.60 * focus_key);
    }

    float2 key_dir = normalize(float2(b.light_x, b.light_y) + float2(1e-6));

    if (u > 0.0) {
        float top_facing = max(-rim_normal.y, 0.0);
        float key_align = max(dot(rim_normal, key_dir), 0.0);
        float top_rim = (top_facing * sqrt(sqrt(top_facing))) *
                        (0.65 + 0.35 * (key_align * sqrt(sqrt(key_align))));

        float bot_facing = max(rim_normal.y, 0.0);
        float away_align = max(dot(rim_normal, -key_dir), 0.0);
        float bot_rim = (bot_facing * sqrt(sqrt(bot_facing))) *
                        (0.55 + 0.35 * (away_align * sqrt(sqrt(away_align))));

        float line = exp(-(d + 0.65) * (d + 0.65) / 2.00);
        float tir_line = exp(-(d + 0.80) * (d + 0.80) / 2.00);
        float ambient_rim = (0.28 + 0.32 * max(ambient_fresnel, 0.0)) * line;

        float total_spec = ambient_rim + top_rim * line + bot_rim * tir_line;

        float3 spec_tint = lerp(float3(1.0), max(frost, float3(0.12)) * 1.35, 0.28);
        colour += spec_tint * (pc.rim_light * total_spec);

        float contact_alpha = clamp(contact_ao, 0.0, 1.0);
        float contact_absorb = contact_alpha * 0.45 * exp(-d * d / 1.20);
        colour *= 1.0 - contact_absorb;
    }

#ifndef TESSERA_MATERIAL_STORAGE_RGBA16F
    float dither = frac(sin(dot(p, float2(12.9898, 78.233))) * 43758.5453) - 0.5;
    colour += dither * (1.5 / 255.0);
#endif

    float alpha = coverage * clamp(b.opacity, 0.0, 1.0);
    alpha = alpha + shadow * (1.0 - alpha);
    store_source_over(pixel, float4(colour * alpha, alpha));
}

[numthreads(16, 16, 1)]
void main(uint3 dispatchThreadID : SV_DispatchThreadID)
{
    uint2 local = dispatchThreadID.xy;
    if (local.x >= pc.region_width || local.y >= pc.region_height) return;
    uint2 pixel = local + uint2(pc.origin_x, pc.origin_y);
    if (pixel.x >= pc.width || pixel.y >= pc.height) return;

    float2 p = float2(pixel) + float2(0.5) + float2(pc.slab_origin_x, pc.slab_origin_y);
    float2 capture_extent = float2(pc.capture_width, pc.capture_height);
    float2 uv = p / max(capture_extent, float2(1.0));

    for (uint slot = 0u; slot < pc.id_count; ++slot) {
        GlassBody b = body_at(body_id(slot));
        shade_body(pixel, p, uv, b);
    }
}
