#version 450

layout(location = 0) in vec2 position;
layout(location = 1) in vec3 color;
layout(location = 2) in uvec2 tex_coord;

layout(location = 0) out vec3 v_color;
layout(location = 1) out vec2 v_tex_coord;

void main() {
    float posx = (position.x) / 640.0 * 2 - 1;
    float posy = (position.y) / 480.0 * 2 - 1;

    gl_Position = vec4(posx, posy, 0.0, 1.0);
    v_color = color;
    v_tex_coord = vec2(tex_coord);
}
