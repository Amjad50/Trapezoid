use super::GpuStat;

use glium::{
    buffer::{Buffer, BufferMode, BufferType},
    implement_vertex,
    index::PrimitiveType,
    program,
    texture::{ClientFormat, MipmapsOption, RawImage2d, Texture2d, UncompressedFloatFormat},
    uniform,
    uniforms::MagnifySamplerFilter,
    BlitTarget, IndexBuffer, Rect, Surface, VertexBuffer,
};

use std::borrow::Cow;
use std::convert::From;
use std::ops::Range;
use std::rc::Rc;

pub struct GlContext {
    context: Rc<glium::backend::Context>,
}

impl GlContext {
    pub fn new<F: glium::backend::Facade>(gl_facade: &F) -> Self {
        Self {
            context: gl_facade.get_context().clone(),
        }
    }
}

impl glium::backend::Facade for GlContext {
    #[inline]
    fn get_context(&self) -> &Rc<glium::backend::Context> {
        &self.context
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingVertex {
    position: [f32; 2],
    color: [f32; 3],
    tex_coord: [u8; 2],
}

impl DrawingVertex {
    #[inline]
    pub fn new_with_color(color: u32) -> Self {
        let mut s = Self::default();
        s.color_from_u32(color);
        s
    }

    #[inline]
    pub fn position_from_u32(&mut self, position: u32) {
        let x = position & 0x7ff;
        let sign_extend = 0xfffff800 * ((x >> 10) & 1);
        let x = (x | sign_extend) as i32;
        let y = (position >> 16) & 0x7ff;
        let sign_extend = 0xfffff800 * ((y >> 10) & 1);
        let y = (y | sign_extend) as i32;

        self.position = [x as f32, y as f32];
    }

    #[inline]
    pub fn color_from_u32(&mut self, color: u32) {
        let r = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = ((color >> 16) & 0xFF) as u8;

        self.color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
    }

    #[inline]
    pub fn tex_coord_from_u32(&mut self, tex_coord: u32) {
        self.tex_coord = [(tex_coord & 0xFF) as u8, ((tex_coord >> 8) & 0xFF) as u8];
    }
}

implement_vertex!(DrawingVertex, position, color, tex_coord);

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingTextureParams {
    clut_base: [u32; 2],
    tex_page_base: [u32; 2],
    // TODO: add support for transparent later
    // semi_transparecy_mode: u8,
    tex_page_color_mode: u8,
}

impl DrawingTextureParams {
    #[inline]
    pub fn tex_page_from_u32(&mut self, param: u32) {
        let param = param >> 16;
        let x = param & 0xF;
        let y = (param >> 4) & 1;

        self.tex_page_base = [x * 64, y * 256];
        //self.semi_transparecy_mode = ((param >> 5) & 3) as u8;
        self.tex_page_color_mode = ((param >> 7) & 3) as u8;
        // TODO: support disable later, in the bios, the textures have this bit set
        //self.texture_disable = (param >> 11) & 1 != 1;
    }

    #[inline]
    pub fn clut_from_u32(&mut self, param: u32) {
        let param = param >> 16;
        let x = param & 0x3F;
        let y = (param >> 6) & 0x1FF;
        self.clut_base = [x * 16, y];
    }
}

pub struct Vram {
    data: Buffer<[u16]>,
}

impl Vram {
    #[inline]
    fn new(gl_context: &GlContext) -> Self {
        let data = Buffer::empty_unsized(
            gl_context,
            BufferType::PixelUnpackBuffer,
            1024 * 512 * 2,
            BufferMode::Dynamic,
        )
        .unwrap();

        Self { data }
    }

    #[inline]
    fn write_at_position_from_drawing(&mut self, position: (u32, u32), data: (u8, u8, u8, u8)) {
        let data = ((data.3 & 1) as u16) << 15
            | ((data.2 >> 3) as u16) << 10
            | ((data.1 >> 3) as u16) << 5
            | (data.0 >> 3) as u16;

        self.write_at_position(position, data);
    }

    #[inline]
    fn write_at_position(&mut self, position: (u32, u32), data: u16) {
        let address = position.1 * 1024 + position.0;
        self.data.map_write().set(address as usize, data);
    }

    #[inline]
    fn read_at_position(&mut self, position: (u32, u32)) -> u16 {
        let address = position.1 * 1024 + position.0;
        self.data.map_read()[address as usize]
    }
}

pub struct GpuContext {
    pub(super) gpu_stat: GpuStat,
    pub(super) gpu_read: Option<u32>,
    pub(super) vram: Vram,

    pub(super) drawing_area_top_left: (u32, u32),
    pub(super) drawing_area_bottom_right: (u32, u32),
    pub(super) drawing_offset: (i32, i32),
    pub(super) texture_window_mask: (u32, u32),
    pub(super) texture_window_offset: (u32, u32),

    pub(super) vram_display_area_start: (u32, u32),
    pub(super) display_horizontal_range: (u32, u32),
    pub(super) display_vertical_range: (u32, u32),

    gl_context: GlContext,
    drawing_texture: Texture2d,
    /// Stores the VRAM content to be used for rendering in the shaders
    texture_buffer: Texture2d,
    /// Ranges in the VRAM which are not resident in the VRAM at the moment but in the
    /// [drawing_texture], so if any byte in this range is read/written to, then
    /// we need to retrieve it from the texture and not the VRAM array
    ranges_in_rendering: Vec<(Range<u32>, Range<u32>)>,
}

impl GpuContext {
    pub fn new(gl_context: GlContext) -> Self {
        let drawing_texture = Texture2d::with_mipmaps(
            &gl_context,
            RawImage2d {
                data: Cow::from(vec![0u16; 1024 * 512]),
                width: 1024,
                height: 512,
                format: ClientFormat::U5U5U5U1,
            },
            MipmapsOption::NoMipmap,
        )
        .unwrap();

        let texture_buffer = Texture2d::empty_with_format(
            &gl_context,
            UncompressedFloatFormat::U16,
            MipmapsOption::NoMipmap,
            1024,
            512,
        )
        .unwrap();

        Self {
            gpu_stat: Default::default(),
            gpu_read: Default::default(),
            vram: Vram::new(&gl_context),

            drawing_area_top_left: (0, 0),
            drawing_area_bottom_right: (0, 0),
            drawing_offset: (0, 0),
            texture_window_mask: (0, 0),
            texture_window_offset: (0, 0),

            vram_display_area_start: (0, 0),
            display_horizontal_range: (0, 0),
            display_vertical_range: (0, 0),
            gl_context,
            drawing_texture,
            texture_buffer,
            ranges_in_rendering: Vec::new(),
        }
    }
}

impl GpuContext {
    fn check_not_in_rendering(&self, position: (u32, u32)) {
        for range in &self.ranges_in_rendering {
            if range.0.contains(&position.0) && range.1.contains(&position.1) {
                println!("ranges= {:?}", self.ranges_in_rendering);
                println!("range found= {:?}, position={:?}", range, position);
                todo!();
            }
        }
    }

    fn add_drawing_range(&mut self, new_range: (Range<u32>, Range<u32>)) {
        fn range_overlap(r1: &(Range<u32>, Range<u32>), r2: &(Range<u32>, Range<u32>)) -> bool {
            // they are left/right to each other
            if r1.0.start >= r2.0.end || r2.0.start >= r1.0.end {
                return false;
            }

            // they are on top of one another
            if r1.1.start >= r2.1.end || r2.1.start >= r1.1.end {
                return false;
            }

            true
        }

        if !self.ranges_in_rendering.contains(&new_range) {
            let mut overlapped_ranges = Vec::new();
            self.ranges_in_rendering.retain(|range| {
                if range_overlap(&range, &new_range) {
                    overlapped_ranges.push(range.clone());
                    false
                } else {
                    true
                }
            });

            // return the parts that we deleted into the Vram buffer
            for range in overlapped_ranges {
                let width = range.0.end - range.0.start;
                let height = range.1.end - range.1.start;
                let tex = Texture2d::empty_with_mipmaps(
                    &self.gl_context,
                    MipmapsOption::NoMipmap,
                    width,
                    height,
                )
                .unwrap();
                self.drawing_texture.as_surface().blit_color(
                    &Rect {
                        left: range.0.start,
                        bottom: 512 - height + range.1.start,
                        width,
                        height,
                    },
                    &tex.as_surface(),
                    &BlitTarget {
                        left: 0,
                        bottom: 0,
                        width: width as i32,
                        height: height as i32,
                    },
                    MagnifySamplerFilter::Nearest,
                );

                let mut pixel_buffer = tex.read_to_pixel_buffer();
                let read_map = pixel_buffer.map_read();
                let mut i = 0;
                for y in range.1.into_iter() {
                    for x in range.0.clone() {
                        let data = read_map[i];
                        self.vram.write_at_position_from_drawing((x, y), data);
                        i += 1;
                    }
                }
            }

            self.ranges_in_rendering.push(new_range);

            println!("ranges now {:?}", self.ranges_in_rendering);
        }
    }

    pub fn write_vram(&mut self, position: (u32, u32), data: u16) {
        self.check_not_in_rendering(position);
        self.vram.write_at_position(position, data);
    }

    pub fn read_vram(&mut self, position: (u32, u32)) -> u16 {
        self.check_not_in_rendering(position);
        self.vram.read_at_position(position)
    }

    pub fn update_texture_buffer(&mut self) {
        self.texture_buffer
            .main_level()
            .raw_upload_from_pixel_buffer(self.vram.data.as_slice(), 0..1024, 0..512, 0..1);
    }

    pub fn draw_polygon(
        &mut self,
        vertices: &[DrawingVertex],
        texture_params: &DrawingTextureParams,
        textured: bool,
    ) {
        // TODO: if its textured, make sure the textures are not in rendering
        //  ranges and are updated in the texture buffer

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let drawing_range = (
            drawing_left..(drawing_right + 1),
            drawing_top..(drawing_bottom + 1),
        );

        self.add_drawing_range(drawing_range);

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom - drawing_top + 1;
        let width = drawing_right - drawing_left + 1;
        let bottom = 512 - height + top;

        let draw_params = glium::DrawParameters {
            viewport: Some(glium::Rect {
                left,
                bottom,
                width,
                height,
            }),
            ..Default::default()
        };

        let full_index_list = &[0u16, 1, 2, 1, 2, 3];
        let index_list = if vertices.len() == 4 {
            &full_index_list[..]
        } else {
            &full_index_list[..3]
        };

        let vertex_buffer = VertexBuffer::new(&self.gl_context, vertices).unwrap();
        let index_buffer =
            IndexBuffer::new(&self.gl_context, PrimitiveType::TrianglesList, index_list).unwrap();

        let program = program!(&self.gl_context,
            140 => {
                vertex: "
                    #version 140
                    in vec2 position;
                    in vec3 color;
                    in uvec2 tex_coord;

                    out vec3 v_color;
                    out vec2 v_tex_coord;

                    uniform ivec2 offset;

                    void main() {
                        /* Transform from 0-640 to 0-1 range. */
                        float posx = (position.x + offset.x) / 640 * 2 - 1;
                        /* Transform from 0-480 to 0-1 range. */
                        float posy = (position.y + offset.y) / 480 * (-2) + 1;

                        gl_Position = vec4(posx, posy, 0.0, 1.0);
                        v_color = color;
                        v_tex_coord = vec2(tex_coord);
                    }
                ",
                fragment: "
                    #version 140

                    in vec3 v_color;
                    in vec2 v_tex_coord;

                    out vec4 out_color;

                    uniform bool is_textured;
                    uniform sampler2D tex;
                    uniform uvec2 tex_page_base;
                    uniform uint tex_page_color_mode;
                    uniform uvec2 clut_base;

                    vec4 get_color_from_u16(uint color_texel) {
                        uint r = color_texel & 0x1Fu;
                        uint g = (color_texel >> 5) & 0x1Fu;
                        uint b = (color_texel >> 10) & 0x1Fu;
                        // TODO: use it for semi_transparency
                        uint a = (color_texel >> 15) & 1u;

                        return vec4(float(r) / 31.0, float(g) / 31.0, float(b) / 31.0, 0.0);
                    }

                    void main() {
                        // retrieve the interpolated value of `tex_coord`
                        uvec2 tex_coord = uvec2(round(v_tex_coord));

                        if (is_textured) {
                            // how many pixels in 16 bit
                            uint divider;
                            if (tex_page_color_mode == 0u) {
                                divider = 4u;
                            } else if (tex_page_color_mode == 1u) {
                                divider = 2u;
                            } else {
                                divider = 1u;
                            };

                            // offsetted position
                            uint x = tex_page_base.x + (tex_coord.x / divider);
                            uint y = tex_page_base.y + tex_coord.y;

                            uint color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);

                            // if we need clut, then compute it
                            if (tex_page_color_mode == 0u || tex_page_color_mode == 1u) {
                                uint mask = 0xFFFFu >> (16u - (16u / divider));
                                uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
                                uint clut_index = (color_value >> clut_index_shift) & mask;

                                x = clut_base.x + clut_index;
                                y = clut_base.y;
                                color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);
                            }

                            // if its all 0, then its transparent
                            if (color_value == 0u){
                                discard;
                            }

                            out_color = get_color_from_u16(color_value);
                        } else{
                            out_color = vec4(v_color, 0.0);
                        }       
                    }
                "
            },
        )
        .unwrap();

        let uniforms = uniform! {
            offset: self.drawing_offset,
            is_textured: textured,
            tex: self.texture_buffer.sampled(),
            tex_page_base: texture_params.tex_page_base,
            tex_page_color_mode: texture_params.tex_page_color_mode,
            clut_base: texture_params.clut_base,
        };

        let mut texture_target = self.drawing_texture.as_surface();
        texture_target
            .draw(
                &vertex_buffer,
                &index_buffer,
                &program,
                &uniforms,
                &draw_params,
            )
            .unwrap();
    }

    pub fn blit_to_front<S: glium::Surface>(&self, s: &S) {
        let (left, top) = self.vram_display_area_start;
        let width = self.gpu_stat.horizontal_resolution();
        let height = self.gpu_stat.vertical_resolution();
        let bottom = 512 - height - top;

        let (target_w, target_h) = s.get_dimensions();

        self.drawing_texture.as_surface().blit_color(
            &Rect {
                left,
                bottom,
                width,
                height,
            },
            s,
            &BlitTarget {
                left: 0,
                bottom: 0,
                width: target_w as i32,
                height: target_h as i32,
            },
            MagnifySamplerFilter::Nearest,
        );
    }
}
