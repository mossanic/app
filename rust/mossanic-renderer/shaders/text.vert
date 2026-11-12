#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    vec2 screen_size;
} pc;

void main() {
    // Convert pixel coordinates to NDC (-1.0 to 1.0)
    vec2 ndc = (in_pos / pc.screen_size) * 2.0 - 1.0;
    
    // Flip Y axis for Vulkan
    ndc.y = -ndc.y;

    // Rotate 90 degrees to fix Android Surface orientation
    vec2 rotated = vec2(ndc.y, -ndc.x);
    
    // Flip both axes to correct the upside-down/mirrored text
    gl_Position = vec4(-rotated.x, -rotated.y, 0.0, 1.0);
    out_uv = in_uv;
}
