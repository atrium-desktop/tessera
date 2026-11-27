/*
 * scene_phong.frag.slang — Phong scene fragment shader in Slang.
 */

#include "scene_color.slang"

struct PushConstants {
    float4x4     mvp;
    PhongParams* params;
};

[[vk::push_constant]]
PushConstants pc;

struct FragmentInput {
    [[vk::location(0)]] float3 v_world_pos : POSITION0;
    [[vk::location(1)]] float3 v_world_nrm : NORMAL0;
    [[vk::location(2)]] float2 v_uv        : TEXCOORD0;
    bool is_front_face : SV_IsFrontFace;
};

[shader("fragment")]
float4 main(FragmentInput input) : SV_Target
{
    float4 base_col = scene_surface_color(pc.params.base_color, pc.params.uv_scale_offset,
                                          pc.params.uv_rotation_alpha_cutoff,
                                          pc.params.texture_info, pc.params.color_flags,
                                          input.v_uv);

    float3 N = normalize(input.v_world_nrm);
    if (!input.is_front_face)
        N = -N;
    float3 L = normalize(-pc.params.light_dir_shininess.xyz);
    float ndotl = max(dot(N, L), 0.0f);
    float3 V = normalize(pc.params.eye_specular.xyz - input.v_world_pos);
    float3 H = normalize(L + V);
    float spec = ndotl > 0.0f
        ? pow(max(dot(N, H), 0.0f), pc.params.light_dir_shininess.w)
        : 0.0f;

    float3 diffuse = base_col.rgb * pc.params.light_color_ambient.xyz * ndotl;
    float3 ambient = base_col.rgb * pc.params.light_color_ambient.w;
    float3 specular = pc.params.light_color_ambient.xyz * spec * pc.params.eye_specular.w;
    float3 lit = diffuse + ambient + specular;
    if (pc.params.texture_info.z == SCENE_ALPHA_BLEND)
        lit *= base_col.a;
    return float4(lit, base_col.a);
}
