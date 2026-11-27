/*
 * scene_phong.vert.slang — Phong scene vertex shader in Slang.
 */

#include "scene_color.slang"

struct PushConstants {
    float4x4     mvp;
    PhongParams* params;
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
    [[vk::location(0)]] float3 v_world_pos : POSITION0;
    [[vk::location(1)]] float3 v_world_nrm : NORMAL0;
    [[vk::location(2)]] float2 v_uv        : TEXCOORD0;
};

[shader("vertex")]
VertexOutput main(VertexInput input)
{
    VertexOutput output;
    output.sv_position = mul(pc.mvp, float4(input.a_position, 1.0f));
    output.v_world_pos = mul(pc.params.world, float4(input.a_position, 1.0f)).xyz;
    float3x3 nrm = float3x3(pc.params.nrm0.xyz, pc.params.nrm1.xyz, pc.params.nrm2.xyz);
    output.v_world_nrm = mul(nrm, input.a_normal);
    output.v_uv = input.a_uv;
    return output;
}
