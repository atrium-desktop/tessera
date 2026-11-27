/*
 * scene_phong_skin.vert.slang — Phong skinning vertex shader in Slang.
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
VertexOutput main(VertexInput input, uint vid : SV_VulkanVertexID)
{
    VertexOutput output;
    float4x4 skin = scene_skin_matrix(pc.params.skin, pc.params.palette, pc.params.joint_count, vid);

    float4 skinned_position = mul(skin, float4(input.a_position, 1.0f));
    float3x3 skin3 = float3x3(skin[0].xyz, skin[1].xyz, skin[2].xyz);
    float3 skinned_normal = mul(skin3, input.a_normal);

    output.sv_position = mul(pc.mvp, skinned_position);
    output.v_world_pos = mul(pc.params.world, skinned_position).xyz;
    float3x3 nrm = float3x3(pc.params.nrm0.xyz, pc.params.nrm1.xyz, pc.params.nrm2.xyz);
    output.v_world_nrm = mul(nrm, skinned_normal);
    output.v_uv = input.a_uv;
    return output;
}
