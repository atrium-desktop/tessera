/*
 * scene_unlit_skin.vert.slang — Unlit skinning vertex shader in Slang.
 */

#include "scene_color.slang"

struct PushConstants {
    float4x4     mvp;
    UnlitParams* params;
};

[[vk::push_constant]]
PushConstants pc;

struct VertexInput {
    [[vk::location(0)]] float3 a_position : POSITION0;
    [[vk::location(1)]] float3 a_normal   : NORMAL0;
    [[vk::location(2)]] float2 a_uv       : TEXCOORD0;
};

struct VertexOutput {
    float4 sv_position : SV_Position;
    [[vk::location(0)]] float2 v_uv : TEXCOORD0;
};

[shader("vertex")]
VertexOutput main(VertexInput input, uint vid : SV_VulkanVertexID)
{
    VertexOutput output;
    float4x4 skin = scene_skin_matrix(pc.params.skin, pc.params.palette, pc.params.joint_count, vid);

    output.sv_position = mul(pc.mvp, mul(skin, float4(input.a_position, 1.0f)));
    output.v_uv = input.a_uv;
    return output;
}
