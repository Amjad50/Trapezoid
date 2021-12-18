#version 450

layout(location = 0) in vec2 position;
layout(location = 1) in vec3 color;
layout(location = 2) in uvec2 tex_coord;

layout(location = 0) out vec3 v_color;
layout(location = 1) out vec2 v_tex_coord;

layout(push_constant) uniform PushConstantData {
    ivec2 offset;
    uvec2 drawing_top_left;
    uvec2 drawing_size;
} pc;

void main() {
    vec2 pos = ((position + pc.offset - pc.drawing_top_left) / pc.drawing_size) * 2 - 1;

    gl_Position = vec4(pos, 0.0, 1.0);
    v_color = color;
    v_tex_coord = vec2(tex_coord);
}
