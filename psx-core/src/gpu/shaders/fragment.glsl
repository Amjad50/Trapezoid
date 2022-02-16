#version 450

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec2 v_tex_coord;

layout(location = 0) out vec4 f_color;

layout(push_constant) uniform PushConstantData {
    ivec2 offset;
    uvec2 drawing_top_left;
    uvec2 drawing_size;

    bool semi_transparent;
    uint semi_transparency_mode;

    bool dither_enabled;

    bool is_textured;
    uvec2 tex_page_base;
    uvec2 clut_base;
    bool is_texture_blended;
    uint tex_page_color_mode;
    bvec2 texture_flip;
} pc;

layout(set = 0, binding = 0) uniform sampler2D back_tex;


const vec2 SCREEN_DIM = vec2(1024, 512);

const float dither_table[16] = {
    -4.0/255.0,  +0.0/255.0,  -3.0/255.0,  +1.0/255.0,   //\dither offsets for first two scanlines
    +2.0/255.0,  -2.0/255.0,  +3.0/255.0,  -1.0/255.0,   ///
    -3.0/255.0,  +1.0/255.0,  -4.0/255.0,  +0.0/255.0,   //\dither offsets for next two scanlines
    +3.0/255.0,  -1.0/255.0,  +2.0/255.0,  -2.0/255.0    ///(same as above, but shifted two pixels horizontally)
};

// this gets the back value from the texture and does manual blending
// since we can't acheive this blending using Vulkan's alphaBlending ops
vec3 get_color_with_semi_transparency_for_mode_3(vec3 color, bool semi_transparency_param) {
    if (!semi_transparency_param) {
        return color;
    }

    vec3 back_color = vec3(texture(back_tex, gl_FragCoord.xy / SCREEN_DIM));

    return (1.0 * back_color) + (0.25 * color);
}

vec4 get_color_with_semi_transparency(vec3 color, bool semi_transparency_param) {
    float alpha = 0.0;

    // since this is mostly the most common case, we'll do it first
    if (pc.semi_transparency_mode == 3u) {
        // alpha here doesn't matter since it won't be written to the framebuffer anyway (disabled by the blend)
        return vec4(get_color_with_semi_transparency_for_mode_3(color, semi_transparency_param), 0.0);
    }
    if (pc.semi_transparency_mode == 0u) {
        if (semi_transparency_param) {
            alpha = 0.5;
        } else {
            alpha = 1.0;
        }
    } else if (pc.semi_transparency_mode == 1u) {
        alpha = float(semi_transparency_param);
    } else { // pc.semi_transparency_mode == 2u
        alpha = float(semi_transparency_param);
    }

    // transparency will be handled by alpha blending
    return vec4(color, alpha);
}

vec4 fetch_color_from_texture(uvec2 coord) {
    return texture(back_tex, vec2(coord) / SCREEN_DIM, 0);
}

uint u16_from_color_with_alpha(vec4 raw_color_value) {
    uint color_value = 0;
    color_value |= uint(raw_color_value.r * 0x1Fu) << 0;
    color_value |= uint(raw_color_value.g * 0x1Fu) << 5;
    color_value |= uint(raw_color_value.b * 0x1Fu) << 10;
    color_value |= uint(raw_color_value.a * 0x1u) << 15;
    return color_value;
}

void main() {
    vec3 t_color;
    vec4 out_color;

    if (pc.dither_enabled) {
        uint x = uint(gl_FragCoord.x) % 4;
        uint y = uint(gl_FragCoord.y) % 4;

        float change = dither_table[y * 4 + x];
        t_color = v_color + change;
    } else {
        t_color = v_color;
    }

    if (pc.is_textured) {
        uvec2 tex_coord = uvec2(round(v_tex_coord));

        // how many pixels in 16 bit
        // 0 => 4
        // 1 => 2
        // 2 => 1
        // 3 => 1
        uint divider = 1 << (2 - pc.tex_page_color_mode);
        if (pc.tex_page_color_mode == 3) {
            divider = 1;
        }

        uint x = tex_coord.x / divider;
        uint y = tex_coord.y;

        // texture flips
        if (pc.texture_flip.x) {
            x = (255u / divider) - x;
        }
        if (pc.texture_flip.y) {
            y = 255u - y;
        }

        vec4 color_value = fetch_color_from_texture(pc.tex_page_base + uvec2(x, y));

        // if we need clut, then compute it
        if (pc.tex_page_color_mode == 0u || pc.tex_page_color_mode == 1u) {
            uint color_u16 = u16_from_color_with_alpha(color_value);

            uint mask = 0xFFFFu >> (16u - (16u / divider));
            uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
            uint clut_index = (color_u16 >> clut_index_shift) & mask;

            color_value = fetch_color_from_texture(pc.clut_base + uvec2(clut_index, 0));
        }

        // if its all 0, then its transparent
        if (color_value == vec4(0.0)) {
            discard;
        }

        vec3 color = color_value.rgb;

        if (pc.is_texture_blended) {
            color *= t_color * 2;
        }
        out_color = get_color_with_semi_transparency(color, pc.semi_transparent && color_value.a == 1.0);
    } else {
        out_color = get_color_with_semi_transparency(t_color, pc.semi_transparent);
    }
    // swizzle the colors
    f_color = out_color.bgra;
}
