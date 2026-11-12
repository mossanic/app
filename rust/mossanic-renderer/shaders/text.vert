#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    vec2 screen_size;
} pc;

void main() {
    // Convert pixel coords (0,0 top-left) to Vulkan NDC (-1,-1 top-left)
    vec2 ndc = (in_pos / pc.screen_size) * 2.0 - 1.0;
    
    // Flip both axes for correct orientation
    gl_Position = vec4(-ndc.x, -ndc.y, 0.0, 1.0);
    out_uv = in_uv;
}
