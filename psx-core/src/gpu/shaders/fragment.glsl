#version 450

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec2 v_tex_coord;

layout(location = 2)  flat in uvec2 v_clut_base;
layout(location = 3)  flat in uvec2 v_tex_page_base;
layout(location = 4)  flat in uint  v_semi_transparency_mode;
layout(location = 5)  flat in uint  v_tex_page_color_mode;
layout(location = 6)  flat in uint  v_semi_transparent;
layout(location = 7)  flat in uint  v_dither_enabled;
layout(location = 8)  flat in uint  v_is_textured;
layout(location = 9) flat in uint  v_is_texture_blended;

layout(location = 0) out vec4 f_color;

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
    if (v_semi_transparency_mode == 3u) {
        // alpha here doesn't matter since it won't be written to the framebuffer anyway (disabled by the blend)
        return vec4(get_color_with_semi_transparency_for_mode_3(color, semi_transparency_param), 0.0);
    }
    if (v_semi_transparency_mode == 0u) {
        if (semi_transparency_param) {
            alpha = 0.5;
        } else {
            alpha = 1.0;
        }
    } else if (v_semi_transparency_mode == 1u) {
        alpha = float(semi_transparency_param);
    } else { // v_semi_transparency_mode == 2u
        alpha = float(semi_transparency_param);
    }

    // transparency will be handled by alpha blending
    return vec4(color, alpha);
}

vec4 fetch_color_from_texture_float(vec2 coord) {
    return texture(back_tex, coord / SCREEN_DIM, 0);
}

vec4 fetch_color_from_texture(uvec2 coord) {
    coord.x = coord.x & 1023u;
    coord.y = coord.y & 511u;
    return texelFetch(back_tex, ivec2(coord), 0);
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

    if (v_dither_enabled == 1) {
        uint x = uint(gl_FragCoord.x) % 4;
        uint y = uint(gl_FragCoord.y) % 4;

        float change = dither_table[y * 4 + x];
        t_color = v_color + change;
    } else {
        t_color = v_color;
    }

    if (v_is_textured == 1) {
        // how many pixels in 16 bit
        // 0 => 4
        // 1 => 2
        // 2 => 1
        // 3 => 1
        uint divider = 1 << (2 - v_tex_page_color_mode);
        if (v_tex_page_color_mode == 3) {
            divider = 1;
        }

        // texture flip and texture repeat support
        // flipped textures, will have decrement in number
        // and might flip to negative as well, we can handle that by mod
        vec2 norm_coord = mod(v_tex_coord, 256);

        float x = norm_coord.x / divider;
        float y = norm_coord.y;

        vec4 color_value = fetch_color_from_texture_float(vec2(v_tex_page_base) + vec2(x, y));

        // if we need clut, then compute it
        if (v_tex_page_color_mode == 0u || v_tex_page_color_mode == 1u) {
            uint color_u16 = u16_from_color_with_alpha(color_value);

            uint mask = 0xFFFFu >> (16u - (16u / divider));
            uint clut_index_shift = (uint(norm_coord.x) % divider) * (16u / divider);
            uint clut_index = (color_u16 >> clut_index_shift) & mask;

            color_value = fetch_color_from_texture(v_clut_base + uvec2(clut_index, 0));
        }

        // if its all 0, then its transparent
        if (color_value == vec4(0)) {
            discard;
        }

        vec3 color = color_value.rgb;

        if (v_is_texture_blended == 1) {
            color *= t_color * 2;
        }
        out_color = get_color_with_semi_transparency(color, v_semi_transparent == 1 && color_value.a == 1.0);
    } else {
        out_color = get_color_with_semi_transparency(t_color, v_semi_transparent == 1);
    }
    // swizzle the colors
    f_color = out_color.bgra;
}
