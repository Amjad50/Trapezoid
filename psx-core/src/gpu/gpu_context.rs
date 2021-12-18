use vulkano::buffer::{BufferUsage, CpuAccessibleBuffer};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer, PrimaryCommandBuffer,
    SubpassContents,
};
use vulkano::descriptor_set::PersistentDescriptorSet;
use vulkano::device::{Device, Queue};
use vulkano::format::{ClearValue, Format};
use vulkano::image::view::ImageView;
use vulkano::image::{ImageAccess, ImageDimensions, StorageImage};
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::vertex_input::BuffersDefinition;
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::{GraphicsPipeline, Pipeline, PipelineBindPoint};
use vulkano::render_pass::{Framebuffer, Subpass};
use vulkano::sync::{self, GpuFuture};

use super::front_blit::FrontBlit;
use super::GpuStat;

use std::ops::Range;
use std::sync::Arc;

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "src/gpu/shaders/vertex.glsl"
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "src/gpu/shaders/fragment.glsl"
    }
}

/// helper to convert opengl colors into u16
#[inline]
fn gl_pixel_to_u16(pixel: &(u8, u8, u8, u8)) -> u16 {
    ((pixel.3 & 1) as u16) << 15
        | ((pixel.2 >> 3) as u16) << 10
        | ((pixel.1 >> 3) as u16) << 5
        | (pixel.0 >> 3) as u16
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

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingVertex {
    position: [f32; 2],
    color: [f32; 3],
    tex_coord: [u32; 2],
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
    pub fn tex_coord(&mut self) -> [u32; 2] {
        self.tex_coord
    }

    #[inline]
    pub fn set_tex_coord(&mut self, tex_coord: [u32; 2]) {
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
        self.tex_coord = [(tex_coord & 0xFF), ((tex_coord >> 8) & 0xFF)];
    }
}

vulkano::impl_vertex!(DrawingVertex, position, color, tex_coord);

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

    #[inline]
    pub fn texture_width(&self) -> u32 {
        // 0 => 64
        // 1 => 128
        // 1 << 256
        1 << (6 + self.tex_page_color_mode)
    }

    #[inline]
    pub fn does_need_clut(&self) -> bool {
        self.tex_page_color_mode == 0 || self.tex_page_color_mode == 1
    }

    #[inline]
    pub fn clut_width(&self) -> u32 {
        // 0 => 16
        // 1 => 256
        1 << ((self.tex_page_color_mode + 1) * 4)
    }
}

pub struct GpuContext {
    pub(super) gpu_stat: GpuStat,
    pub(super) allow_texture_disable: bool,
    pub(super) textured_rect_flip: (bool, bool),
    pub(super) gpu_read: Option<u32>,

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

    device: Arc<Device>,
    queue: Arc<Queue>,
    render_image: Arc<StorageImage>,
    texture_buffer: Arc<CpuAccessibleBuffer<[u16]>>,
    clut_buffer: Arc<CpuAccessibleBuffer<[u16]>>,

    render_image_framebuffer: Arc<Framebuffer>,
    pipeline: Arc<GraphicsPipeline>,
    // TODO: this buffer gives Gpu lock issues, so either we create
    //  buffer every time, we draw, or we create multiple buffers and loop through them
    _vertex_buffer: Arc<CpuAccessibleBuffer<[DrawingVertex]>>,

    front_blit: FrontBlit,

    gpu_future: Option<Box<dyn GpuFuture>>,
    // /// Stores the VRAM content to be used for rendering in the shaders
    // texture_buffer: Texture2d,
}

impl GpuContext {
    pub fn new(device: Arc<Device>, queue: Arc<Queue>) -> Self {
        let render_image = StorageImage::new(
            device.clone(),
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::A1R5G5B5_UNORM_PACK16,
            [queue.family()],
        )
        .unwrap();

        let texture_buffer = CpuAccessibleBuffer::from_iter(
            device.clone(),
            BufferUsage::all(),
            false,
            (0..256 * 256).map(|_| 0),
        )
        .unwrap();
        let clut_buffer = CpuAccessibleBuffer::from_iter(
            device.clone(),
            BufferUsage::all(),
            false,
            (0..256).map(|_| 0),
        )
        .unwrap();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                device.clone(),
                queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        builder
            .clear_color_image(
                render_image.clone(),
                ClearValue::Float([0.0, 0.0, 0.0, 0.0]),
            )
            .unwrap();
        // add command to clear the render image, and keep the future
        // for stacking later
        let command_buffer = builder.build().unwrap();
        let image_clear_future = command_buffer.execute(queue.clone()).unwrap();

        let vs = vs::load(device.clone()).unwrap();
        let fs = fs::load(device.clone()).unwrap();

        let render_pass = vulkano::single_pass_renderpass!(
            device.clone(),
            attachments: {
                color: {
                    load: Load,
                    store: Store,
                    format: Format::A1R5G5B5_UNORM_PACK16,
                    samples: 1,
                }
            },
            pass: {
                color: [color],
                depth_stencil: {}
            }
        )
        .unwrap();

        let pipeline = GraphicsPipeline::start()
            .vertex_input_state(BuffersDefinition::new().vertex::<DrawingVertex>())
            .vertex_shader(vs.entry_point("main").unwrap(), ())
            .input_assembly_state(
                InputAssemblyState::new().topology(PrimitiveTopology::TriangleStrip),
            )
            .viewport_state(ViewportState::viewport_dynamic_scissor_irrelevant())
            .fragment_shader(fs.entry_point("main").unwrap(), ())
            .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
            .build(device.clone())
            .unwrap();

        let render_image_framebuffer = Framebuffer::start(render_pass.clone())
            .add(ImageView::new(render_image.clone()).unwrap())
            .unwrap()
            .build()
            .unwrap();

        let vertex_buffer = CpuAccessibleBuffer::from_iter(
            device.clone(),
            BufferUsage::all(),
            false,
            [DrawingVertex::default(); 4].iter().cloned(),
        )
        .unwrap();

        let (texture_blit, texture_blit_future) =
            FrontBlit::new(device.clone(), queue.clone(), render_image.clone());

        let gpu_future = Some(image_clear_future.join(texture_blit_future).boxed());

        //let program = program!(&gl_context,
        //    140 => {
        //        vertex: "
        //            #version 140
        //            in vec2 position;
        //            in vec3 color;
        //            in uvec2 tex_coord;

        //            out vec3 v_color;
        //            out vec2 v_tex_coord;

        //            uniform ivec2 offset;
        //            uniform uvec2 drawing_top_left;
        //            uniform uvec2 drawing_size;

        //            void main() {
        //                float posx = (position.x + offset.x - drawing_top_left.x) / drawing_size.x * 2 - 1;
        //                float posy = (position.y + offset.y - drawing_top_left.x) / drawing_size.y * (-2) + 1;

        //                gl_Position = vec4(posx, posy, 0.0, 1.0);
        //                v_color = color;
        //                v_tex_coord = vec2(tex_coord);
        //            }
        //        ",
        //        fragment: "
        //            #version 140

        //            in vec3 v_color;
        //            in vec2 v_tex_coord;

        //            out vec4 out_color;

        //            uniform bool texture_flip_x;
        //            uniform bool texture_flip_y;
        //            uniform bool is_textured;
        //            uniform bool is_texture_blended;
        //            uniform bool semi_transparent;
        //            uniform uint semi_transparency_mode;
        //            uniform sampler2D tex;
        //            uniform uvec2 tex_page_base;
        //            uniform uint tex_page_color_mode;
        //            uniform uvec2 clut_base;

        //            vec4 get_color_from_u16(uint color_texel) {
        //                uint r = color_texel & 0x1Fu;
        //                uint g = (color_texel >> 5) & 0x1Fu;
        //                uint b = (color_texel >> 10) & 0x1Fu;
        //                uint a = (color_texel >> 15) & 1u;

        //                return vec4(float(r) / 31.0, float(g) / 31.0, float(b) / 31.0, float(a));
        //            }

        //            vec4 get_color_with_semi_transparency(vec3 color, float semi_transparency_param) {
        //                float alpha;
        //                if (semi_transparency_mode == 0u) {
        //                    if (semi_transparency_param == 1.0) {
        //                        alpha = 0.5;
        //                    } else {
        //                        alpha = 1.0;
        //                    }
        //                } else if (semi_transparency_mode == 1u) {
        //                    alpha = semi_transparency_param;
        //                } else if (semi_transparency_mode == 2u) {
        //                    alpha = semi_transparency_param;
        //                } else {
        //                    // FIXME: inaccurate mode 3 semi transparency
        //                    //
        //                    // these numbers with the equation:
        //                    // (source * source_alpha + dest * (1 - source_alpha)
        //                    // Will result in the following cases:
        //                    // if semi=1:
        //                    //      s * 0.25 + d * 0.75
        //                    // if semi=0:
        //                    //      s * 1.0 + d * 0.0
        //                    //
        //                    // but we need
        //                    // if semi=1:
        //                    //      s * 0.25 + d * 1.00
        //                    // if semi=0:
        //                    //      s * 1.0 + d * 0.0
        //                    //
        //                    // Thus, this is not accurate, but temporary will keep
        //                    // it like this until we find a new solution
        //                    if (semi_transparency_param == 1.0) {
        //                        alpha = 0.25;
        //                    } else {
        //                        alpha = 1.0;
        //                    }
        //                }

        //                return vec4(color, alpha);
        //            }

        //            void main() {
        //                // retrieve the interpolated value of `tex_coord`
        //                uvec2 tex_coord = uvec2(round(v_tex_coord));

        //                if (is_textured) {
        //                    // how many pixels in 16 bit
        //                    uint divider;
        //                    if (tex_page_color_mode == 0u) {
        //                        divider = 4u;
        //                    } else if (tex_page_color_mode == 1u) {
        //                        divider = 2u;
        //                    } else {
        //                        divider = 1u;
        //                    };

        //                    // offsetted position
        //                    // FIXME: fix weird inconsistent types here (uint and int)
        //                    int x;
        //                    int y;
        //                    if (texture_flip_x) {
        //                        x = int(tex_page_base.x + (256u / divider)) - int(tex_coord.x / divider);
        //                    } else {
        //                        x = int(tex_page_base.x) + int(tex_coord.x / divider);
        //                    }
        //                    if (texture_flip_y) {
        //                        y = int(tex_page_base.y + 256u) - int(tex_coord.y);
        //                    } else {
        //                        y = int(tex_page_base.y) + int(tex_coord.y);
        //                    }

        //                    uint color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);

        //                    // if we need clut, then compute it
        //                    if (tex_page_color_mode == 0u || tex_page_color_mode == 1u) {
        //                        uint mask = 0xFFFFu >> (16u - (16u / divider));
        //                        uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
        //                        if (texture_flip_x) {
        //                            clut_index_shift = 12u - clut_index_shift;
        //                        }
        //                        uint clut_index = (color_value >> clut_index_shift) & mask;

        //                        x = int(clut_base.x + clut_index);
        //                        y = int(clut_base.y);
        //                        color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);
        //                    }

        //                    // if its all 0, then its transparent
        //                    if (color_value == 0u){
        //                        discard;
        //                    }

        //                    vec4 color_with_alpha = get_color_from_u16(color_value);
        //                    vec3 color = vec3(color_with_alpha);

        //                    if (is_texture_blended) {
        //                        color *=  v_color * 2;
        //                    }
        //                    out_color = get_color_with_semi_transparency(color, color_with_alpha.a);
        //                } else {
        //                    out_color = get_color_with_semi_transparency(v_color, float(semi_transparent));
        //                }
        //            }
        //        "
        //    },
        //)
        //.unwrap();

        Self {
            gpu_stat: Default::default(),
            allow_texture_disable: false,
            textured_rect_flip: (false, false),
            gpu_read: Default::default(),

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
            device,
            queue,
            render_image,
            render_image_framebuffer,

            texture_buffer,
            clut_buffer,

            pipeline,

            _vertex_buffer: vertex_buffer,

            front_blit: texture_blit,

            gpu_future,
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

    fn get_semi_transparency_blending_params(&self, semi_transparecy_mode: u8) -> () {
        //let color_func = match semi_transparecy_mode & 3 {
        //    0 => BlendingFunction::Addition {
        //        source: LinearBlendingFactor::SourceAlpha,
        //        destination: LinearBlendingFactor::OneMinusSourceAlpha,
        //    },
        //    1 => BlendingFunction::Addition {
        //        source: LinearBlendingFactor::One,
        //        destination: LinearBlendingFactor::SourceAlpha,
        //    },
        //    2 => BlendingFunction::ReverseSubtraction {
        //        source: LinearBlendingFactor::One,
        //        destination: LinearBlendingFactor::SourceAlpha,
        //    },
        //    3 => BlendingFunction::Addition {
        //        source: LinearBlendingFactor::SourceAlpha,
        //        destination: LinearBlendingFactor::OneMinusSourceAlpha,
        //    },
        //    _ => unreachable!(),
        //};

        //Blend {
        //    color: color_func,
        //    // TODO: handle alpha so that it takes the mask value
        //    alpha: BlendingFunction::AlwaysReplace,
        //    constant_value: (1.0, 1.0, 1.0, 1.0),
        //}
    }
}

impl GpuContext {
    pub fn write_vram_block(&mut self, block_range: (Range<u32>, Range<u32>), block: &[u16]) {
        self.gpu_future.as_mut().unwrap().cleanup_finished();

        // TODO: check for out-of-bound writes here

        let left = block_range.0.start;
        let top = block_range.1.start;
        let width = block_range.0.len() as u32;
        let height = block_range.1.len() as u32;

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        let buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::transfer_source(),
            false,
            block.iter().cloned(),
        )
        .unwrap();

        builder
            .copy_buffer_to_image_dimensions(
                buffer,
                self.render_image.clone(),
                [left, top, 0],
                [width, height, 1],
                0,
                1,
                0,
            )
            .unwrap();

        let command_buffer = builder.build().unwrap();

        self.gpu_future = Some(
            self.gpu_future
                .take()
                .unwrap()
                .then_execute(self.queue.clone(), command_buffer)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .boxed(),
        );
    }

    pub fn read_vram_block(&mut self, block_range: &(Range<u32>, Range<u32>)) -> Vec<u16> {
        // TODO: check for out-of-bound reads here

        let left = block_range.0.start;
        let top = block_range.1.start;
        let width = block_range.0.len() as u32;
        let height = block_range.1.len() as u32;

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        let buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::transfer_destination(),
            false,
            (0..width * height).map(|_| 0u16),
        )
        .unwrap();

        builder
            .copy_image_to_buffer_dimensions(
                self.render_image.clone(),
                buffer.clone(),
                [left, top, 0],
                [width, height, 1],
                0,
                1,
                0,
            )
            .unwrap();

        let command_buffer = builder.build().unwrap();

        self.gpu_future
            .take()
            .unwrap()
            .then_execute(self.queue.clone(), command_buffer)
            .unwrap()
            .then_signal_fence_and_flush()
            .unwrap()
            .wait(None)
            .unwrap();
        self.gpu_future = Some(sync::now(self.device.clone()).boxed());

        let buffer_read = buffer.read().unwrap();

        buffer_read.to_vec()
    }

    pub fn fill_color(&mut self, top_left: (u32, u32), size: (u32, u32), color: (u8, u8, u8)) {
        self.gpu_future.as_mut().unwrap().cleanup_finished();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        let mut width = size.0;
        let mut height = size.1;
        // TODO: I'm not sure if we should support wrapping, but for now
        //       we do not, since we would need to do extra clean draws
        if top_left.0 + width > 1024 {
            width = 1024 - top_left.0;
        }
        if top_left.1 + height > 512 {
            height = 512 - top_left.1;
        }

        // TODO: check that the gl color encoding works well with vulkano
        let u16_color = gl_pixel_to_u16(&(color.0, color.1, color.2, 0));

        // TODO: for now vulkano does not support clear rect, but later change this

        // Creates buffer of the desired color, then clear it
        let buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::transfer_source(),
            false,
            (0..size.0 * size.1).map(|_| u16_color),
        )
        .unwrap();

        builder
            .copy_buffer_to_image_dimensions(
                buffer,
                self.render_image.clone(),
                [top_left.0, top_left.1, 0],
                [width, height, 1],
                0,
                1,
                0,
            )
            .unwrap();

        let command_buffer = builder.build().unwrap();

        self.gpu_future = Some(
            self.gpu_future
                .take()
                .unwrap()
                .then_execute(self.queue.clone(), command_buffer)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .boxed(),
        );
    }

    pub fn draw_polygon(
        &mut self,
        vertices: &[DrawingVertex],
        texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        _semi_transparent: bool,
    ) {
        self.gpu_future.as_mut().unwrap().cleanup_finished();

        let vertex_buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::all(),
            false,
            vertices.into_iter().cloned(),
        )
        .unwrap();

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom - drawing_top + 1;
        let width = drawing_right - drawing_left + 1;

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        let mut texture_width = 0;
        if textured {
            texture_width = texture_params.texture_width();
            let tex_base_x = texture_params.tex_page_base[0];
            if tex_base_x + texture_width > 1024 {
                // make sure that no tex_coords are not out of bounds
                texture_width = 1024 - tex_base_x;
                for &DrawingVertex {
                    tex_coord: [x, _], ..
                } in vertices
                {
                    assert!(
                        x <= texture_width,
                        "Using out-of-bound tex_coord in out-of-bound texture base_x={}, modified_width={}, coord_x={}",
                        tex_base_x, texture_width, x
                    );
                }
            }
            builder
                .copy_image_to_buffer_dimensions(
                    self.render_image.clone(),
                    self.texture_buffer.clone(),
                    [
                        texture_params.tex_page_base[0],
                        texture_params.tex_page_base[1],
                        0,
                    ],
                    [texture_width, 256, 1],
                    0,
                    1,
                    0,
                )
                .unwrap();
            if texture_params.does_need_clut() {
                // TODO: if the clut is out of bound, then we only copy the
                //       until bound, and leave the rest, this will result in
                //       clut information from the previous buffer, hopefully
                //       they are not used. Not sure if we need wrapping here or what.
                let mut clut_width = texture_params.clut_width();
                let clut_base_x = texture_params.clut_base[0];

                if clut_base_x + clut_width > 1024 {
                    clut_width = 1024 - clut_base_x;
                }

                builder
                    .copy_image_to_buffer_dimensions(
                        self.render_image.clone(),
                        self.clut_buffer.clone(),
                        [clut_base_x, texture_params.clut_base[1], 0],
                        [clut_width, 1, 1],
                        0,
                        1,
                        0,
                    )
                    .unwrap();
            }
        };

        let push_constants = fs::ty::PushConstantData {
            offset: [self.drawing_offset.0, self.drawing_offset.1],
            drawing_top_left: [left, top],
            drawing_size: [width, height],

            is_textured: textured as u32,
            texture_width,
            is_texture_blended: texture_blending as u32,
            tex_page_color_mode: texture_params.tex_page_color_mode as u32,
            texture_flip: [
                texture_params.texture_flip.0 as u32,
                texture_params.texture_flip.1 as u32,
            ],
        };

        let layout = self
            .pipeline
            .layout()
            .descriptor_set_layouts()
            .get(0)
            .unwrap();
        let mut set_builder = PersistentDescriptorSet::start(layout.clone());

        set_builder
            .add_buffer(self.texture_buffer.clone())
            .unwrap()
            .add_buffer(self.clut_buffer.clone())
            .unwrap();

        let set = set_builder.build().unwrap();

        builder
            .begin_render_pass(
                self.render_image_framebuffer.clone(),
                SubpassContents::Inline,
                [ClearValue::None],
            )
            .unwrap()
            .set_viewport(
                0,
                [Viewport {
                    origin: [left as f32, top as f32],
                    dimensions: [width as f32, height as f32],
                    depth_range: 0.0..1.0,
                }],
            )
            .bind_pipeline_graphics(self.pipeline.clone())
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.pipeline.layout().clone(),
                0,
                set.clone(),
            )
            .push_constants(self.pipeline.layout().clone(), 0, push_constants)
            .bind_vertex_buffers(0, vertex_buffer.clone())
            .draw(vertices.len() as u32, 1, 0, 0)
            .unwrap()
            .end_render_pass()
            .unwrap();

        let command_buffer = builder.build().unwrap();

        self.gpu_future = Some(
            self.gpu_future
                .take()
                .unwrap()
                .then_execute(self.queue.clone(), command_buffer)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .boxed(),
        );

        //if textured {
        //    if !self.allow_texture_disable {
        //        texture_params.texture_disable = false;
        //    }
        //    self.update_gpu_stat_from_texture_params(&texture_params);

        //    // if the texure we can using is inside `rendering`, bring it back
        //    // to `vram` and `texture_buffer`
        //    //
        //    // 0 => 64,
        //    // 1 => 128,
        //    // 2 => 256,
        //    let row_size = 64 * (1 << texture_params.tex_page_color_mode);
        //    let texture_block = (
        //        texture_params.tex_page_base[0]..texture_params.tex_page_base[0] + row_size,
        //        texture_params.tex_page_base[1]..texture_params.tex_page_base[1] + 256,
        //    );
        //    if self.is_block_in_rendering(&texture_block) {
        //        self.move_from_rendering_to_vram(&texture_block);
        //    }
        //}

        //// TODO: if its textured, make sure the textures are not in rendering
        ////  ranges and are updated in the texture buffer

        //let (drawing_left, drawing_top) = self.drawing_area_top_left;
        //let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        //let drawing_range = (
        //    drawing_left..(drawing_right + 1),
        //    drawing_top..(drawing_bottom + 1),
        //);

        //self.add_to_rendering_range(drawing_range);

        //let left = drawing_left;
        //let top = drawing_top;
        //let height = drawing_bottom - drawing_top + 1;
        //let width = drawing_right - drawing_left + 1;
        //let bottom = to_gl_bottom(top, height);

        //let semi_transparency_mode = if textured {
        //    texture_params.semi_transparency_mode
        //} else {
        //    self.gpu_stat.semi_transparency_mode()
        //};
        //let blend = self.get_semi_transparency_blending_params(semi_transparency_mode);

        //let draw_params = glium::DrawParameters {
        //    viewport: Some(glium::Rect {
        //        left,
        //        bottom,
        //        width,
        //        height,
        //    }),
        //    blend,
        //    color_mask: (true, true, true, false),
        //    ..Default::default()
        //};

        //let full_index_list = &[0u16, 1, 2, 1, 2, 3];
        //let index_list = if vertices.len() == 4 {
        //    &full_index_list[..]
        //} else {
        //    &full_index_list[..3]
        //};

        //let vertex_buffer = VertexBuffer::new(&self.gl_context, vertices).unwrap();
        //let index_buffer =
        //    IndexBuffer::new(&self.gl_context, PrimitiveType::TrianglesList, index_list).unwrap();

        //let uniforms = uniform! {
        //    offset: self.drawing_offset,
        //    texture_flip_x: texture_params.texture_flip.0,
        //    texture_flip_y: texture_params.texture_flip.1,
        //    is_textured: textured,
        //    is_texture_blended: texture_blending,
        //    semi_transparency_mode: semi_transparency_mode,
        //    semi_transparent: semi_transparent,
        //    tex: self.texture_buffer.sampled(),
        //    tex_page_base: texture_params.tex_page_base,
        //    tex_page_color_mode: texture_params.tex_page_color_mode,
        //    clut_base: texture_params.clut_base,
        //    drawing_top_left: [left, top],
        //    drawing_size: [width, height],
        //};

        //let mut texture_target = self.drawing_texture.as_surface();
        //texture_target
        //    .draw(
        //        &vertex_buffer,
        //        &index_buffer,
        //        &self.program,
        //        &uniforms,
        //        &draw_params,
        //    )
        //    .unwrap();
    }

    pub fn blit_to_front<D, IF>(&mut self, dest_image: Arc<D>, full_vram: bool, in_future: IF)
    where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        let (topleft, size) = if full_vram {
            ([0; 2], [1024, 512])
        } else {
            (
                [
                    self.vram_display_area_start.0,
                    self.vram_display_area_start.1,
                ],
                [
                    self.gpu_stat.horizontal_resolution(),
                    self.gpu_stat.vertical_resolution(),
                ],
            )
        };

        // TODO: try to remove the `wait` from here
        self.front_blit
            .blit(
                dest_image,
                topleft,
                size,
                self.gpu_future.take().unwrap().join(in_future),
            )
            .then_signal_fence_and_flush()
            .unwrap()
            .wait(None)
            .unwrap();

        // reset future since we are waiting
        self.gpu_future = Some(sync::now(self.device.clone()).boxed());

        //let (left, top) = self.vram_display_area_start;
        //let width = self.gpu_stat.horizontal_resolution();
        //let height = self.gpu_stat.vertical_resolution();
        //let bottom = to_gl_bottom(top, height);

        //let src_rect = if full_vram {
        //    Rect {
        //        left: 0,
        //        bottom: 0,
        //        width: 1024,
        //        height: 512,
        //    }
        //} else {
        //    Rect {
        //        left,
        //        bottom,
        //        width,
        //        height,
        //    }
        //};

        //let (target_w, target_h) = s.get_dimensions();

        //self.drawing_texture.as_surface().blit_color(
        //    &src_rect,
        //    s,
        //    &BlitTarget {
        //        left: 0,
        //        bottom: 0,
        //        width: target_w as i32,
        //        height: target_h as i32,
        //    },
        //    MagnifySamplerFilter::Nearest,
        //);
    }
}
