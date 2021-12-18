#version 450

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec2 v_tex_coord;

layout(location = 0) out vec4 f_color;

layout(push_constant) uniform PushConstantData {
    ivec2 offset;
    uvec2 drawing_top_left;
    uvec2 drawing_size;

    bool is_textured;
    uint texture_width;
    bool is_texture_blended;
    uint tex_page_color_mode;
    bvec2 texture_flip;
} pc;

layout(set = 0, binding = 0) buffer TextureData {
    uint data[];
} texture;
layout(set = 0, binding = 1) buffer ClutData {
    uint data[];
} clut;

vec4 get_color_from_u16(uint color_texel) {
    uint r = color_texel & 0x1Fu;
    uint g = (color_texel >> 5) & 0x1Fu;
    uint b = (color_texel >> 10) & 0x1Fu;
    uint a = (color_texel >> 15) & 1u;

    return vec4(float(r) / 31.0, float(g) / 31.0, float(b) / 31.0, float(a));
}

void main() {
    if (pc.is_textured) {
        uvec2 tex_coord = uvec2(round(v_tex_coord));

        // how many pixels in 16 bit
        // 0 => 4
        // 1 => 2
        // 2 => 1
        uint divider = 1 << (2 - pc.tex_page_color_mode);
        uint texture_width = 1 << (6 + pc.tex_page_color_mode);

        uint x = tex_coord.x / divider;
        uint y = tex_coord.y;

        // texture flips
        if (pc.texture_flip.x) {
            x = (255u / divider) - x;
        }
        if (pc.texture_flip.y) {
            y = 255u - y;
        }

        // since this is u32 datatype, we need to manually extract
        // the u16 data
        uint color_value = texture.data[((y * texture_width) + x ) / 2];
        if (x % 2 == 0) {
            color_value = color_value & 0xFFFF;
        } else {
            color_value = color_value >> 16;
        }

        // if we need clut, then compute it
        if (pc.tex_page_color_mode == 0u || pc.tex_page_color_mode == 1u) {
            uint mask = 0xFFFFu >> (16u - (16u / divider));
            uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
            uint clut_index = (color_value >> clut_index_shift) & mask;

            x = int(clut_index);
            // since this is u32 datatype, we need to manually extract
            // the u16 data
            color_value = clut.data[x / 2];
            if (x % 2 == 0) {
                color_value = color_value & 0xFFFF;
            } else {
                color_value = color_value >> 16;
            }
        }

        // if its all 0, then its transparent
        if (color_value == 0u){
            discard;
        }

        vec4 color_with_alpha = get_color_from_u16(color_value);
        vec3 color = vec3(color_with_alpha);

        if (pc.is_texture_blended) {
            color *=  v_color * 2;
        }
        f_color = vec4(color.b, color.g, color.r, color_with_alpha.a);
    } else {
        f_color = vec4(v_color.b, v_color.g, v_color.r, 0.0);
    }
}
