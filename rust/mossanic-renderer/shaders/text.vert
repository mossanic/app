#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    mat4 proj;
} pc;

void main() {
    // Apply the projection matrix to convert pixel coords to NDC
    gl_Position = pc.proj * vec4(in_pos, 0.0, 1.0);
    out_uv = in_uv;
}