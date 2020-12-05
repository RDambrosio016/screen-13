#version 450

layout(push_constant) uniform PushConstants {
    layout(offset = 0) mat4 world_view_proj;
}
push_constants;

layout(location = 0) in vec3 position_in;
layout(location = 1) in vec3 normal_in;
layout(location = 2) in vec2 texcoord_in;

layout(location = 0) out flat vec3 normal_out;
layout(location = 1) out vec2 texcoord_out;

void main() {
    normal_out = normal_in;
    texcoord_out = texcoord_in;

    gl_Position = push_constants.world_view_proj * vec4(position_in, 1);
}
