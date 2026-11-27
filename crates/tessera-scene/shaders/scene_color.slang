/*
 * scene_color.slang — shared scene shader declarations (Tessera ADR-0262).
 *
 * Carried from the retired Optics gfx scene core. The per-draw parameter
 * blocks below are field-for-field mirrors of the std430 structs in
 * csrc/src/render.c; change both together.
 */

#ifndef SCENE_COLOR_SLANG
#define SCENE_COLOR_SLANG

[[vk::binding(0, 0)]] Texture2D u_textures[];
[[vk::binding(2, 0)]] SamplerState u_samplers[];

struct UnlitParams {
    float4       base_color;
    float4       uv_scale_offset;
    float4       uv_rotation_alpha_cutoff;
    uint4        texture_info;      /* image, sampler, alpha mode, textured */
    uint32_t     color_flags;
    uint32_t     _color_pad[3];
    float4x4*    palette;
    uint32_t*    skin;              /* 6 words per vertex: joints (4×u16), weights (4×f32) */
    uint32_t     joint_count;
    uint32_t     _pad[3];
};

struct PhongParams {
    float4x4     world;
    float4       nrm0;
    float4       nrm1;
    float4       nrm2;
    float4       base_color;
    float4       uv_scale_offset;
    float4       uv_rotation_alpha_cutoff;
    uint4        texture_info;
    uint32_t     color_flags;
    uint32_t     _color_pad[3];
    float4       light_dir_shininess;
    float4       light_color_ambient;
    float4       eye_specular;
    float4x4*    palette;
    uint32_t*    skin;
    uint32_t     joint_count;
    uint32_t     _pad[3];
};

/* color_flags: DECODE converts sampled 8-bit texels from sRGB into the linear
 * working space when the material renders into a 16F target (ADR-0069). */
static const uint32_t SCENE_COLOR_DECODE = 0x1u;

/* texture_info.z */
static const uint32_t SCENE_ALPHA_OPAQUE = 0u;
static const uint32_t SCENE_ALPHA_MASK   = 1u;
static const uint32_t SCENE_ALPHA_BLEND  = 2u;

float scene_srgb_decode(float e) {
    if (e <= 0.0f)
        return 0.0f;
    if (e >= 1.0f)
        return 1.0f;
    return e <= 0.04045f ? e / 12.92f : pow((e + 0.055f) / 1.055f, 2.4f);
}

float3 scene_srgb_decode3(float3 e) {
    return float3(scene_srgb_decode(e.r), scene_srgb_decode(e.g), scene_srgb_decode(e.b));
}

/* Base colour after texturing and the glTF alpha mode. Blended materials
 * come back premultiplied: the pipeline blends PREMUL source-over. */
float4 scene_surface_color(float4 base_color, float4 uv_scale_offset, float4 uv_rot_cutoff,
                           uint4 texture_info, uint32_t color_flags, float2 v_uv) {
    float2 scaled = v_uv * uv_scale_offset.xy;
    float c = uv_rot_cutoff.x;
    float s = uv_rot_cutoff.y;
    float2 uv = float2(c * scaled.x - s * scaled.y, s * scaled.x + c * scaled.y) +
                uv_scale_offset.zw;

    float4 base_col = base_color;
    if (texture_info.w != 0u) {
        uint32_t ih = texture_info.x & 0x0FFFFFFFu;
        uint32_t sh = texture_info.y & 0x0FFFFFFFu;
        float4 texel = u_textures[NonUniformResourceIndex(ih)].Sample(
            u_samplers[NonUniformResourceIndex(sh)], uv);
        if ((color_flags & SCENE_COLOR_DECODE) != 0u)
            texel.rgb = scene_srgb_decode3(texel.rgb);
        base_col *= texel;
    }

    uint32_t alpha_mode = texture_info.z;
    if (alpha_mode == SCENE_ALPHA_MASK) {
        if (base_col.a < uv_rot_cutoff.z)
            discard;
        base_col.a = 1.0f;
    } else if (alpha_mode == SCENE_ALPHA_OPAQUE) {
        base_col.a = 1.0f;
    }
    return base_col;
}

/* Linear-blend skinning matrix for vertex `vid`, read from the mesh's skin
 * buffer by device address. */
float4x4 scene_skin_matrix(uint32_t* skin, float4x4* palette, uint32_t joint_count, uint vid) {
    uint base = vid * 6u;
    uint j01 = skin[base + 0u];
    uint j23 = skin[base + 1u];
    uint4 joints = uint4(j01 & 0xFFFFu, j01 >> 16, j23 & 0xFFFFu, j23 >> 16);
    float4 weights = float4(asfloat(skin[base + 2u]), asfloat(skin[base + 3u]),
                            asfloat(skin[base + 4u]), asfloat(skin[base + 5u]));
    weights = max(weights, float4(0.0f, 0.0f, 0.0f, 0.0f));
    float total = dot(weights, float4(1.0f, 1.0f, 1.0f, 1.0f));
    weights = total > 0.0f ? weights / total : float4(1.0f, 0.0f, 0.0f, 0.0f);
    joints = min(joints, uint4(max(joint_count, 1u) - 1u));
    return weights.x * palette[joints.x] + weights.y * palette[joints.y] +
           weights.z * palette[joints.z] + weights.w * palette[joints.w];
}

#endif /* SCENE_COLOR_SLANG */
