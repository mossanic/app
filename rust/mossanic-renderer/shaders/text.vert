#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_foreground;
layout(location = 3) in vec4 in_background;
layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_foreground;
layout(location = 2) out vec4 out_background;

layout(push_constant) uniform PushConstants {
    mat4 proj;
} pc;

void main() {
    // Apply the projection matrix to convert pixel coords to NDC
    gl_Position = pc.proj * vec4(in_pos, 0.0, 1.0);
    out_uv = in_uv;
    out_foreground = in_foreground;
    out_background = in_background;
}
