#version 450
layout(location = 0) in vec2 out_uv;
layout(location = 0) out vec4 out_color;
layout(binding = 0) uniform sampler2D font_tex;

void main() {
    float alpha = texture(font_tex, out_uv).r;
    out_color = vec4(1.0, 1.0, 1.0, alpha);
}