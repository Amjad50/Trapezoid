#version 450

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec2 v_tex_coord;

layout(location = 2)  flat in uvec4 v_tex_info;
layout(location = 3)  flat in uvec4 v_tex_window;
layout(location = 4)  flat in uvec3 v_extra_draw_state;

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

    uint semi_transparency_mode = v_extra_draw_state.x;

    // since this is mostly the most common case, we'll do it first
    if (semi_transparency_mode == 3u) {
        // alpha here doesn't matter since it won't be written to the framebuffer anyway (disabled by the blend)
        return vec4(get_color_with_semi_transparency_for_mode_3(color, semi_transparency_param), 0.0);
    }
    if (semi_transparency_mode == 0u) {
        if (semi_transparency_param) {
            alpha = 0.5;
        } else {
            alpha = 1.0;
        }
    } else if (semi_transparency_mode == 1u) {
        alpha = float(semi_transparency_param);
    } else { // semi_transparency_mode == 2u
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

    uint bool_flags = v_extra_draw_state.z;
    bool semi_transparent = (bool_flags & 0x1u) != 0;
    bool dither_enabled = (bool_flags & 0x2u) != 0;
    bool is_textured = (bool_flags & 0x4u) != 0;
    bool is_texture_blended = (bool_flags & 0x8u) != 0;

    if (dither_enabled) {
        uint x = uint(gl_FragCoord.x) % 4;
        uint y = uint(gl_FragCoord.y) % 4;

        float change = dither_table[y * 4 + x];
        t_color = v_color + change;
    } else {
        t_color = v_color;
    }

    if (is_textured) {
        uvec2 clut_base = v_tex_info.xy;
        uvec2 tex_page_base = v_tex_info.zw;

        uint tex_page_color_mode = v_extra_draw_state.y;

        uvec2 tex_window_mask = v_tex_window.xy & 0x1Fu;
        uvec2 tex_window_offset = v_tex_window.zw & 0x1Fu;

        // how many pixels in 16 bit
        // 0 => 4
        // 1 => 2
        // 2 => 1
        // 3 => 1
        uint divider = 1 << (2 - tex_page_color_mode);
        if (tex_page_color_mode == 3) {
            divider = 1;
        }

        // texture flip and texture repeat support
        // flipped textures, will have decrement in number
        // and might flip to negative as well, we can handle that by mod
        uvec2 norm_coord = uvec2(mod(v_tex_coord, 256));

        // apply texture window
        // Texcoord = (Texcoord AND (NOT (Mask*8))) OR ((Offset AND Mask)*8)
        norm_coord = (norm_coord & (~(tex_window_mask * 8))) | ((tex_window_offset & tex_window_mask) * 8);

        float x = norm_coord.x / divider;
        float y = norm_coord.y;

        vec4 color_value = fetch_color_from_texture_float(vec2(tex_page_base) + vec2(x, y));

        // if we need clut, then compute it
        if (tex_page_color_mode == 0u || tex_page_color_mode == 1u) {
            uint color_u16 = u16_from_color_with_alpha(color_value);

            uint mask = 0xFFFFu >> (16u - (16u / divider));
            uint clut_index_shift = (uint(norm_coord.x) % divider) * (16u / divider);
            uint clut_index = (color_u16 >> clut_index_shift) & mask;

            color_value = fetch_color_from_texture(clut_base + uvec2(clut_index, 0));
        }

        // if its all 0, then its transparent
        if (color_value == vec4(0)) {
            discard;
        }

        vec3 color = color_value.rgb;

        if (is_texture_blended) {
            color *= t_color * 2;
        }
        out_color = get_color_with_semi_transparency(color, semi_transparent && color_value.a == 1.0);
    } else {
        out_color = get_color_with_semi_transparency(t_color, semi_transparent);
    }
    // swizzle the colors
    f_color = out_color.bgra;
}
