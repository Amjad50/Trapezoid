use super::GpuStat;

use glium::{
    buffer::{Buffer, BufferMode, BufferType},
    implement_vertex,
    index::PrimitiveType,
    program,
    texture::{ClientFormat, MipmapsOption, RawImage2d, Texture2d, UncompressedFloatFormat},
    uniform,
    uniforms::MagnifySamplerFilter,
    Blend, BlendingFunction, BlitTarget, IndexBuffer, LinearBlendingFactor, Program, Rect, Surface,
    VertexBuffer,
};

use std::borrow::Cow;
use std::convert::From;
use std::ops::Range;
use std::rc::Rc;

/// helper to convert opengl colors into u16
#[inline]
fn gl_pixel_to_u16(pixel: &(u8, u8, u8, u8)) -> u16 {
    ((pixel.3 & 1) as u16) << 15
        | ((pixel.2 >> 3) as u16) << 10
        | ((pixel.1 >> 3) as u16) << 5
        | (pixel.0 >> 3) as u16
}

/// helper in getting the correct value for bottom for gl drawing/coordinate stuff
#[inline]
fn to_gl_bottom(top: u32, height: u32) -> u32 {
    512 - height - top
}

#[inline]
pub fn vertex_position_from_u32(position: u32) -> [f32; 2] {
    let x = position & 0x7ff;
    let sign_extend = 0xfffff800 * ((x >> 10) & 1);
    let x = (x | sign_extend) as i32;
    let y = (position >> 16) & 0x7ff;
    let sign_extend = 0xfffff800 * ((y >> 10) & 1);
    let y = (y | sign_extend) as i32;
    [x as f32, y as f32]
}

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
    pub fn position(&self) -> [f32; 2] {
        self.position
    }

    #[inline]
    pub fn set_position(&mut self, position: [f32; 2]) {
        self.position = position;
    }

    #[inline]
    pub fn tex_coord(&mut self) -> [u8; 2] {
        self.tex_coord
    }

    #[inline]
    pub fn set_tex_coord(&mut self, tex_coord: [u8; 2]) {
        self.tex_coord = tex_coord;
    }

    #[inline]
    pub fn new_with_color(color: u32) -> Self {
        let mut s = Self::default();
        s.color_from_u32(color);
        s
    }

    #[inline]
    pub fn position_from_u32(&mut self, position: u32) {
        self.position = vertex_position_from_u32(position);
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
    semi_transparency_mode: u8,
    tex_page_color_mode: u8,
    texture_disable: bool,
    texture_flip: (bool, bool),
}

impl DrawingTextureParams {
    /// Process tex page params, from the lower 16 bits, this is only used
    /// for when drawing rectangle, as the tex_page is take fron the gpu_stat
    /// and not from a parameter
    #[inline]
    pub fn tex_page_from_gpustat(&mut self, param: u32) {
        let x = param & 0xF;
        let y = (param >> 4) & 1;

        self.tex_page_base = [x * 64, y * 256];
        self.semi_transparency_mode = ((param >> 5) & 3) as u8;
        self.tex_page_color_mode = ((param >> 7) & 3) as u8;
        self.texture_disable = (param >> 11) & 1 == 1;
    }

    /// Process tex page params, from the higher 16 bits, which is found
    /// in tex page parameter in drawing stuff
    #[inline]
    pub fn tex_page_from_u32(&mut self, param: u32) {
        let param = param >> 16;
        self.tex_page_from_gpustat(param);
    }

    #[inline]
    pub fn clut_from_u32(&mut self, param: u32) {
        let param = param >> 16;
        let x = param & 0x3F;
        let y = (param >> 6) & 0x1FF;
        self.clut_base = [x * 16, y];
    }

    #[inline]
    pub fn set_texture_flip(&mut self, flip: (bool, bool)) {
        self.texture_flip = flip;
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
    fn write_block(&mut self, block_range: &(Range<u32>, Range<u32>), block: &[u16]) {
        let (x_range, y_range) = block_range;
        let whole_size = x_range.len() * y_range.len();
        assert_eq!(block.len(), whole_size);

        let mut mapping = self.data.map_write();
        let mut block_iter = block.into_iter();

        for y in y_range.clone() {
            let mut current_pixel_pos = (y * 1024 + x_range.start) as usize;
            for _ in 0..x_range.len() {
                mapping.set(current_pixel_pos, *block_iter.next().unwrap());
                current_pixel_pos += 1;
            }
        }

        assert!(block_iter.next().is_none());
    }

    #[inline]
    fn read_block(&mut self, block_range: &(Range<u32>, Range<u32>), reverse: bool) -> Vec<u16> {
        let (x_range, y_range) = block_range;

        let row_size = x_range.len();
        let whole_size = row_size * y_range.len();
        let mut block = Vec::with_capacity(whole_size);

        let mapping = self.data.map_read();

        let y_range_iter: Box<dyn Iterator<Item = _>> = if reverse {
            Box::new(y_range.clone())
        } else {
            Box::new(y_range.clone().rev())
        };

        for y in y_range_iter {
            let row_start_addr = y * 1024 + x_range.start;
            block.extend_from_slice(
                &mapping[(row_start_addr as usize)..(row_start_addr as usize + row_size)],
            );
        }

        assert_eq!(block.len(), whole_size);

        block
    }
}

pub struct GpuContext {
    pub(super) gpu_stat: GpuStat,
    pub(super) allow_texture_disable: bool,
    pub(super) textured_rect_flip: (bool, bool),
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

    // These are only used for handleing GP1(0x10) command, so instead of creating
    // the values again from the individual parts, we just cache it
    pub(super) cached_gp0_e2: u32,
    pub(super) cached_gp0_e3: u32,
    pub(super) cached_gp0_e4: u32,
    pub(super) cached_gp0_e5: u32,

    gl_context: GlContext,
    program: Program,
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

        let program = program!(&gl_context,
            140 => {
                vertex: "
                    #version 140
                    in vec2 position;
                    in vec3 color;
                    in uvec2 tex_coord;

                    out vec3 v_color;
                    out vec2 v_tex_coord;

                    uniform ivec2 offset;
                    uniform uvec2 drawing_top_left;
                    uniform uvec2 drawing_size;

                    void main() {
                        float posx = (position.x + offset.x - drawing_top_left.x) / drawing_size.x * 2 - 1;
                        float posy = (position.y + offset.y - drawing_top_left.x) / drawing_size.y * (-2) + 1;

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

                    uniform bool texture_flip_x;
                    uniform bool texture_flip_y;
                    uniform bool is_textured;
                    uniform bool is_texture_blended;
                    uniform bool semi_transparent;
                    uniform uint semi_transparency_mode;
                    uniform sampler2D tex;
                    uniform uvec2 tex_page_base;
                    uniform uint tex_page_color_mode;
                    uniform uvec2 clut_base;

                    vec4 get_color_from_u16(uint color_texel) {
                        uint r = color_texel & 0x1Fu;
                        uint g = (color_texel >> 5) & 0x1Fu;
                        uint b = (color_texel >> 10) & 0x1Fu;
                        uint a = (color_texel >> 15) & 1u;

                        return vec4(float(r) / 31.0, float(g) / 31.0, float(b) / 31.0, float(a));
                    }

                    vec4 get_color_with_semi_transparency(vec3 color, float semi_transparency_param) {
                        float alpha;
                        if (semi_transparency_mode == 0u) {
                            if (semi_transparency_param == 1.0) {
                                alpha = 0.5;
                            } else {
                                alpha = 1.0;
                            }
                        } else if (semi_transparency_mode == 1u) {
                            alpha = semi_transparency_param;
                        } else if (semi_transparency_mode == 2u) {
                            alpha = semi_transparency_param;
                        } else {
                            // FIXME: inaccurate mode 3 semi transparency
                            //
                            // these numbers with the equation:
                            // (source * source_alpha + dest * (1 - source_alpha)
                            // Will result in the following cases:
                            // if semi=1:
                            //      s * 0.25 + d * 0.75
                            // if semi=0:
                            //      s * 1.0 + d * 0.0
                            //
                            // but we need
                            // if semi=1:
                            //      s * 0.25 + d * 1.00
                            // if semi=0:
                            //      s * 1.0 + d * 0.0
                            //
                            // Thus, this is not accurate, but temporary will keep
                            // it like this until we find a new solution
                            if (semi_transparency_param == 1.0) {
                                alpha = 0.25;
                            } else {
                                alpha = 1.0;
                            }
                        }

                        return vec4(color, alpha);
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
                            // FIXME: fix weird inconsistent types here (uint and int)
                            int x;
                            int y;
                            if (texture_flip_x) {
                                x = int(tex_page_base.x + (256u / divider)) - int(tex_coord.x / divider);
                            } else {
                                x = int(tex_page_base.x) + int(tex_coord.x / divider);
                            }
                            if (texture_flip_y) {
                                y = int(tex_page_base.y + 256u) - int(tex_coord.y);
                            } else {
                                y = int(tex_page_base.y) + int(tex_coord.y);
                            }

                            uint color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);

                            // if we need clut, then compute it
                            if (tex_page_color_mode == 0u || tex_page_color_mode == 1u) {
                                uint mask = 0xFFFFu >> (16u - (16u / divider));
                                uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
                                if (texture_flip_x) {
                                    clut_index_shift = 12u - clut_index_shift;
                                }
                                uint clut_index = (color_value >> clut_index_shift) & mask;

                                x = int(clut_base.x + clut_index);
                                y = int(clut_base.y);
                                color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);
                            }

                            // if its all 0, then its transparent
                            if (color_value == 0u){
                                discard;
                            }

                            vec4 color_with_alpha = get_color_from_u16(color_value);
                            vec3 color = vec3(color_with_alpha);

                            if (is_texture_blended) {
                                color *=  v_color * 2;
                            }
                            out_color = get_color_with_semi_transparency(color, color_with_alpha.a);
                        } else {
                            out_color = get_color_with_semi_transparency(v_color, float(semi_transparent));
                        }       
                    }
                "
            },
        )
        .unwrap();

        Self {
            gpu_stat: Default::default(),
            allow_texture_disable: false,
            textured_rect_flip: (false, false),
            gpu_read: Default::default(),
            vram: Vram::new(&gl_context),

            drawing_area_top_left: (0, 0),
            drawing_area_bottom_right: (0, 0),
            drawing_offset: (0, 0),
            texture_window_mask: (0, 0),
            texture_window_offset: (0, 0),

            cached_gp0_e2: 0,
            cached_gp0_e3: 0,
            cached_gp0_e4: 0,
            cached_gp0_e5: 0,

            vram_display_area_start: (0, 0),
            display_horizontal_range: (0, 0),
            display_vertical_range: (0, 0),
            gl_context,
            program,
            drawing_texture,
            texture_buffer,
            ranges_in_rendering: Vec::new(),
        }
    }
}

impl GpuContext {
    /// Drawing commands that use textures will update gpustat
    fn update_gpu_stat_from_texture_params(&mut self, texture_params: &DrawingTextureParams) {
        let x = (texture_params.tex_page_base[0] / 64) & 0xF;
        let y = (texture_params.tex_page_base[1] / 256) & 1;
        self.gpu_stat.bits &= !0x81FF;
        self.gpu_stat.bits |= x;
        self.gpu_stat.bits |= y << 4;
        self.gpu_stat.bits |= (texture_params.semi_transparency_mode as u32) << 5;
        self.gpu_stat.bits |= (texture_params.tex_page_color_mode as u32) << 7;
        self.gpu_stat.bits |= (texture_params.texture_disable as u32) << 15;
    }

    fn move_from_rendering_to_vram(&mut self, range: &(Range<u32>, Range<u32>)) {
        let width = range.0.end - range.0.start;
        let height = range.1.end - range.1.start;
        let tex =
            Texture2d::empty_with_mipmaps(&self.gl_context, MipmapsOption::NoMipmap, width, height)
                .unwrap();
        self.drawing_texture.as_surface().blit_color(
            &Rect {
                left: range.0.start,
                bottom: to_gl_bottom(range.1.start, height),
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

        let pixels: Vec<_> = tex
            .read::<Vec<_>>()
            .iter()
            .rev()
            .flatten()
            .map(gl_pixel_to_u16)
            .collect();
        self.vram.write_block(range, &pixels);
        self.update_texture_buffer();
    }

    fn move_from_vram_to_rendering(&mut self, range: &(Range<u32>, Range<u32>)) {
        let (x_range, y_range) = range;
        let width = x_range.len() as u32;
        let height = y_range.len() as u32;

        let block = self.vram.read_block(&range, true);

        self.drawing_texture.write(
            Rect {
                left: x_range.start,
                bottom: to_gl_bottom(y_range.start, height),
                width,
                height,
            },
            RawImage2d {
                data: Cow::Borrowed(block.as_ref()),
                width,
                height,
                format: ClientFormat::U1U5U5U5Reversed,
            },
        );
    }

    /// check if a whole block in rendering
    fn is_block_in_rendering(&self, block_range: &(Range<u32>, Range<u32>)) -> bool {
        let positions = [
            (block_range.0.start, block_range.1.start),
            (block_range.0.end - 1, block_range.1.end - 1),
        ];

        for range in &self.ranges_in_rendering {
            let contain_start =
                range.0.contains(&positions[0].0) && range.1.contains(&positions[0].1);
            let contain_end =
                range.0.contains(&positions[1].0) && range.1.contains(&positions[1].1);

            // we use or and then assert, to make sure both ends are in the rendering.
            //  if only one of them are in the rendering, then this is a problem.
            //
            //  TODO: fix block half present in rendering.
            if contain_start || contain_end {
                assert!(contain_start && contain_end);

                return true;
            }
        }

        false
    }

    fn add_to_rendering_range(&mut self, new_range: (Range<u32>, Range<u32>)) {
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
                self.move_from_rendering_to_vram(&range);
            }
            self.move_from_vram_to_rendering(&new_range);

            self.ranges_in_rendering.push(new_range);

            println!("ranges now {:?}", self.ranges_in_rendering);
        }
    }

    fn get_semi_transparency_blending_params(&self, semi_transparecy_mode: u8) -> Blend {
        let color_func = match semi_transparecy_mode & 3 {
            0 => BlendingFunction::Addition {
                source: LinearBlendingFactor::SourceAlpha,
                destination: LinearBlendingFactor::OneMinusSourceAlpha,
            },
            1 => BlendingFunction::Addition {
                source: LinearBlendingFactor::One,
                destination: LinearBlendingFactor::SourceAlpha,
            },
            2 => BlendingFunction::ReverseSubtraction {
                source: LinearBlendingFactor::One,
                destination: LinearBlendingFactor::SourceAlpha,
            },
            3 => BlendingFunction::Addition {
                source: LinearBlendingFactor::SourceAlpha,
                destination: LinearBlendingFactor::OneMinusSourceAlpha,
            },
            _ => unreachable!(),
        };

        Blend {
            color: color_func,
            // TODO: handle alpha so that it takes the mask value
            alpha: BlendingFunction::AlwaysReplace,
            constant_value: (1.0, 1.0, 1.0, 1.0),
        }
    }
}

impl GpuContext {
    pub fn write_vram_block(&mut self, block_range: (Range<u32>, Range<u32>), block: &[u16]) {
        // cannot write outside range
        assert!(block_range.0.end <= 1024);
        assert!(block_range.1.end <= 512);

        let whole_size = block_range.0.len() * block_range.1.len();
        assert_eq!(block.len(), whole_size);

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;
        let drawing_range = (
            drawing_left..(drawing_right + 1),
            drawing_top..(drawing_bottom + 1),
        );

        // add the current drawing area to rendering range
        //
        // if we are copying a block into a rendering range, then just blit
        // directly into it
        self.add_to_rendering_range(drawing_range);

        if self.is_block_in_rendering(&block_range) {
            let (x_range, y_range) = block_range;
            let width = x_range.len() as u32;
            let height = y_range.len() as u32;

            // reverse on y axis
            let block: Vec<_> = block
                .chunks(width as usize)
                .rev()
                .flat_map(|row| row.iter())
                .cloned()
                .collect();

            self.drawing_texture.write(
                Rect {
                    left: x_range.start,
                    bottom: to_gl_bottom(y_range.start, height),
                    width,
                    height,
                },
                RawImage2d {
                    data: Cow::Borrowed(block.as_ref()),
                    width,
                    height,
                    format: ClientFormat::U1U5U5U5Reversed,
                },
            );
        } else {
            self.vram.write_block(&block_range, block);
            self.update_texture_buffer();
        }
    }

    pub fn read_vram_block(&mut self, block_range: &(Range<u32>, Range<u32>)) -> Vec<u16> {
        // cannot read outside range
        assert!(block_range.0.end <= 1024);
        assert!(block_range.1.end <= 512);

        if self.is_block_in_rendering(&block_range) {
            let (x_range, y_range) = block_range;
            let x_size = x_range.len() as u32;
            let y_size = y_range.len() as u32;

            let tex = Texture2d::empty_with_mipmaps(
                &self.gl_context,
                MipmapsOption::NoMipmap,
                x_size,
                y_size,
            )
            .unwrap();
            self.drawing_texture.as_surface().blit_color(
                &Rect {
                    left: x_range.start,
                    bottom: to_gl_bottom(y_range.start, y_size),
                    width: x_size,
                    height: y_size,
                },
                &tex.as_surface(),
                &BlitTarget {
                    left: 0,
                    bottom: 0,
                    width: x_size as i32,
                    height: y_size as i32,
                },
                MagnifySamplerFilter::Nearest,
            );

            let pixel_buffer: Vec<_> = tex.read();

            let block = pixel_buffer
                .iter()
                // reverse, as its from bottom to top
                .rev()
                .flatten()
                .map(|pixel| gl_pixel_to_u16(pixel))
                .collect::<Vec<_>>();

            block
        } else {
            self.vram.read_block(block_range, false)
        }
    }

    pub fn fill_color(&mut self, top_left: (u32, u32), size: (u32, u32), color: (u8, u8, u8)) {
        let mut x_range = (top_left.0)..(top_left.0 + size.0);
        let mut y_range = (top_left.1)..(top_left.1 + size.1);

        if x_range.end >= 1024 {
            self.fill_color((0, top_left.1), (x_range.end - 1024 + 1, size.1), color);
            x_range.end = 1023;
        }
        if y_range.end >= 512 {
            self.fill_color((top_left.0, 0), (size.0, y_range.end - 512 + 1), color);
            y_range.end = 511;
        }

        let block_range = (x_range, y_range);

        if self.is_block_in_rendering(&block_range) {
            self.drawing_texture.as_surface().clear(
                Some(&Rect {
                    left: top_left.0,
                    bottom: to_gl_bottom(top_left.1, size.1),
                    width: size.0,
                    height: size.1,
                }),
                Some((
                    color.0 as f32 / 255.0,
                    color.1 as f32 / 255.0,
                    color.2 as f32 / 255.0,
                    0.0,
                )),
                false,
                None,
                None,
            );
        } else {
            let color = gl_pixel_to_u16(&(color.0, color.1, color.2, 0));
            let size = block_range.0.len() * block_range.1.len();

            self.vram.write_block(&block_range, &vec![color; size]);
            self.update_texture_buffer();
        }
    }

    pub fn update_texture_buffer(&mut self) {
        self.texture_buffer
            .main_level()
            .raw_upload_from_pixel_buffer(self.vram.data.as_slice(), 0..1024, 0..512, 0..1);
    }

    pub fn draw_polygon(
        &mut self,
        vertices: &[DrawingVertex],
        mut texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
    ) {
        if textured {
            if !self.allow_texture_disable {
                texture_params.texture_disable = false;
            }
            self.update_gpu_stat_from_texture_params(&texture_params);

            // if the texure we can using is inside `rendering`, bring it back
            // to `vram` and `texture_buffer`
            //
            // 0 => 64,
            // 1 => 128,
            // 2 => 256,
            let row_size = 64 * (1 << texture_params.tex_page_color_mode);
            let texture_block = (
                texture_params.tex_page_base[0]..texture_params.tex_page_base[0] + row_size,
                texture_params.tex_page_base[1]..texture_params.tex_page_base[1] + 256,
            );
            if self.is_block_in_rendering(&texture_block) {
                self.move_from_rendering_to_vram(&texture_block);
            }
        }

        // TODO: if its textured, make sure the textures are not in rendering
        //  ranges and are updated in the texture buffer

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let drawing_range = (
            drawing_left..(drawing_right + 1),
            drawing_top..(drawing_bottom + 1),
        );

        self.add_to_rendering_range(drawing_range);

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom - drawing_top + 1;
        let width = drawing_right - drawing_left + 1;
        let bottom = to_gl_bottom(top, height);

        let semi_transparency_mode = if textured {
            texture_params.semi_transparency_mode
        } else {
            self.gpu_stat.semi_transparency_mode()
        };
        let blend = self.get_semi_transparency_blending_params(semi_transparency_mode);

        let draw_params = glium::DrawParameters {
            viewport: Some(glium::Rect {
                left,
                bottom,
                width,
                height,
            }),
            blend,
            color_mask: (true, true, true, false),
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

        let uniforms = uniform! {
            offset: self.drawing_offset,
            texture_flip_x: texture_params.texture_flip.0,
            texture_flip_y: texture_params.texture_flip.1,
            is_textured: textured,
            is_texture_blended: texture_blending,
            semi_transparency_mode: semi_transparency_mode,
            semi_transparent: semi_transparent,
            tex: self.texture_buffer.sampled(),
            tex_page_base: texture_params.tex_page_base,
            tex_page_color_mode: texture_params.tex_page_color_mode,
            clut_base: texture_params.clut_base,
            drawing_top_left: [left, top],
            drawing_size: [width, height],
        };

        let mut texture_target = self.drawing_texture.as_surface();
        texture_target
            .draw(
                &vertex_buffer,
                &index_buffer,
                &self.program,
                &uniforms,
                &draw_params,
            )
            .unwrap();
    }

    pub fn blit_to_front<S: glium::Surface>(&self, s: &S) {
        let (left, top) = self.vram_display_area_start;
        let width = self.gpu_stat.horizontal_resolution();
        let height = self.gpu_stat.vertical_resolution();
        let bottom = to_gl_bottom(top, height);

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
