use super::GpuStat;

use glium::{
    implement_vertex,
    index::PrimitiveType,
    program,
    texture::{ClientFormat, MipmapsOption, RawImage2d, Texture2d},
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
    fn get_context(&self) -> &Rc<glium::backend::Context> {
        &self.context
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingVertex {
    position: [f32; 2],
    color: [f32; 3],
}

impl DrawingVertex {
    pub fn new_with_color(color: u32) -> Self {
        let mut s = Self::default();
        s.color_from_u32(color);
        s
    }

    pub fn position_from_u32(&mut self, position: u32) {
        let x = position & 0x7ff;
        let sign_extend = 0xfffff800 * ((x >> 10) & 1);
        let x = (x | sign_extend) as i32;
        let y = (position >> 16) & 0x7ff;
        let sign_extend = 0xfffff800 * ((y >> 10) & 1);
        let y = (y | sign_extend) as i32;

        self.position = [x as f32, y as f32];
    }

    pub fn color_from_u32(&mut self, color: u32) {
        let r = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = ((color >> 16) & 0xFF) as u8;

        self.color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
    }
}

implement_vertex!(DrawingVertex, position, color);

pub struct Vram {
    data: Box<[u16; 1024 * 512]>,
}

impl Default for Vram {
    fn default() -> Self {
        Self {
            data: Box::new([0; 1024 * 512]),
        }
    }
}

impl Vram {
    fn write_at_position_from_drawing(&mut self, position: (u32, u32), data: (u8, u8, u8, u8)) {
        let address = position.1 * 1024 + position.0;
        let data = ((data.3 & 1) as u16) << 15
            | ((data.2 >> 3) as u16) << 10
            | ((data.1 >> 3) as u16) << 5
            | (data.0 >> 3) as u16;
        self.data[address as usize] = data;
    }

    fn write_at_position(&mut self, position: (u32, u32), data: u16) {
        let address = position.1 * 1024 + position.0;
        self.data[address as usize] = data;
    }

    fn read_at_position(&self, position: (u32, u32)) -> u16 {
        let address = position.1 * 1024 + position.0;
        self.data[address as usize]
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

        Self {
            gpu_stat: Default::default(),
            gpu_read: Default::default(),
            vram: Default::default(),

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

    pub fn read_vram(&self, position: (u32, u32)) -> u16 {
        self.check_not_in_rendering(position);
        self.vram.read_at_position(position)
    }

    pub fn draw_polygon(&mut self, vertices: &[DrawingVertex]) {
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
                    out vec3 vColor;

                    uniform ivec2 offset;

                    void main() {
                        /* Transform from 0-640 to 0-1 range. */
                        float posx = (position.x + offset.x) / 640 * 2 - 1;
                        /* Transform from 0-480 to 0-1 range. */
                        float posy = (position.y + offset.y) / 480 * (-2) + 1;

                        gl_Position = vec4(posx, posy, 0.0, 1.0);
                        vColor = color;
                    }
                ",
                fragment: "
                    #version 140

                    in vec3 vColor;
                    out vec4 f_color;

                    void main() {
                        f_color = vec4(vColor, 1.0);
                    }
                "
            },
        )
        .unwrap();

        let uniforms = uniform! {
            offset: self.drawing_offset,
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
