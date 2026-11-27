/*
 * scene_unlit.frag.slang — unlit scene fragment shader.
 */

#include "scene_color.slang"

struct PushConstants {
    float4x4     mvp;
    UnlitParams* params;
};

[[vk::push_constant]]
PushConstants pc;

struct FragmentInput {
    [[vk::location(0)]] float2 v_uv : TEXCOORD0;
};

[shader("fragment")]
float4 main(FragmentInput input) : SV_Target
{
    float4 base_col = scene_surface_color(pc.params.base_color, pc.params.uv_scale_offset,
                                          pc.params.uv_rotation_alpha_cutoff,
                                          pc.params.texture_info, pc.params.color_flags,
                                          input.v_uv);
    if (pc.params.texture_info.z == SCENE_ALPHA_BLEND)
        base_col.rgb *= base_col.a;
    return base_col;
}
