#version 450

layout(location = 0)  in vec2  position;
layout(location = 1)  in vec3  color;
layout(location = 2)  in ivec2 tex_coord;

layout(location = 3)  in uvec2 clut_base;
layout(location = 4)  in uvec2 tex_page_base;
layout(location = 5)  in uint  semi_transparency_mode;
layout(location = 6)  in uint  tex_page_color_mode;
layout(location = 7)  in uint  bool_flags;


layout(location = 0)  out vec3  v_color;
layout(location = 1)  out vec2  v_tex_coord;

layout(location = 2)  flat out uvec2 v_clut_base;
layout(location = 3)  flat out uvec2 v_tex_page_base;
layout(location = 4)  flat out uint  v_semi_transparency_mode;
layout(location = 5)  flat out uint  v_tex_page_color_mode;
layout(location = 6)  flat out uint  v_bool_flags;

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

    v_clut_base              = clut_base;
    v_tex_page_base          = tex_page_base;
    v_semi_transparency_mode = semi_transparency_mode;
    v_tex_page_color_mode    = tex_page_color_mode;
    v_bool_flags             = bool_flags;
}
