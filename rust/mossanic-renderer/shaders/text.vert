#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    vec2 screen_size;
} pc;

void main() {
    // Convert pixel coordinates (0..width, 0..height) to NDC (-1.0..1.0)
    vec2 ndc = (in_pos / pc.screen_size) * 2.0 - 1.0;
    
    // In Vulkan, Y goes down. So we flip it.
    ndc.y = -ndc.y;

    // Apply rotation to fix the Android surface orientation (90 degrees clockwise)
    vec2 rotated = vec2(-ndc.y, ndc.x);
    
    gl_Position = vec4(-rotated.x, -rotated.y, 0.0, 1.0);
    out_uv = in_uv;
}
