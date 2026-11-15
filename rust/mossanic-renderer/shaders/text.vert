#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    vec2 screen_size;
} pc;

void main() {
    // Convert pixel coords to Normalized Device Coordinates (NDC)
    float x = (in_pos.x / pc.screen_size.x) * 2.0 - 1.0;
    float y = (in_pos.y / pc.screen_size.y) * 2.0 - 1.0;
    
    gl_Position = vec4(x, y, 0.0, 1.0);
    out_uv = in_uv;
}
