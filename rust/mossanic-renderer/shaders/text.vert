#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    vec2 screen_size;
} pc;

void main() {
    // Convert pixel coords (0..width, 0..height) to NDC (-1.0 to 1.0)
    vec2 ndc = (in_pos / pc.screen_size) * 2.0 - 1.0;
    
    // Flip Y for Vulkan
    ndc.y = -ndc.y;

    // Rotate 90 degrees (flip sign of y if text appears wrong direction)
    vec2 rotated = vec2(ndc.y, -ndc.x);
    
    gl_Position = vec4(rotated, 0.0, 1.0);
    out_uv = in_uv;
}