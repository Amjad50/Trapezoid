use crossbeam::atomic::AtomicCell;
use crossbeam::channel::Sender;
use vulkano::buffer::{BufferUsage, CpuAccessibleBuffer};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer, PrimaryCommandBuffer,
    SubpassContents,
};
use vulkano::descriptor_set::PersistentDescriptorSet;
use vulkano::device::{Device, Queue};
use vulkano::format::{ClearValue, Format};
use vulkano::image::view::{ComponentMapping, ComponentSwizzle, ImageView};
use vulkano::image::{ImageDimensions, StorageImage};
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::vertex_input::BuffersDefinition;
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::{GraphicsPipeline, Pipeline, PipelineBindPoint};
use vulkano::render_pass::{Framebuffer, Subpass};
use vulkano::sampler::{Filter, MipmapMode, Sampler, SamplerAddressMode};
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
}

pub struct GpuContext {
    pub(super) gpu_stat: Arc<AtomicCell<GpuStat>>,
    pub(super) gpu_front_image_sender: Sender<Arc<StorageImage>>,
    gpu_read_sender: Sender<u32>,

    pub(super) allow_texture_disable: bool,
    pub(super) textured_rect_flip: (bool, bool),

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

    pub(super) device: Arc<Device>,
    queue: Arc<Queue>,
    render_image: Arc<StorageImage>,
    render_image_back_image: Arc<StorageImage>,

    render_image_framebuffer: Arc<Framebuffer>,
    polygon_pipeline: Arc<GraphicsPipeline>,
    line_pipeline: Arc<GraphicsPipeline>,
    // TODO: this buffer gives Gpu lock issues, so either we create
    //  buffer every time, we draw, or we create multiple buffers and loop through them
    _vertex_buffer: Arc<CpuAccessibleBuffer<[DrawingVertex]>>,

    front_blit: FrontBlit,

    gpu_future: Option<Box<dyn GpuFuture>>,

    command_builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
}

impl GpuContext {
    pub(super) fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        gpu_stat: Arc<AtomicCell<GpuStat>>,
        gpu_read_sender: Sender<u32>,
        gpu_front_image_sender: Sender<Arc<StorageImage>>,
    ) -> Self {
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

        let render_image_back_image = StorageImage::new(
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

        let polygon_pipeline = GraphicsPipeline::start()
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

        let line_pipeline = GraphicsPipeline::start()
            .vertex_input_state(BuffersDefinition::new().vertex::<DrawingVertex>())
            .vertex_shader(vs.entry_point("main").unwrap(), ())
            .input_assembly_state(InputAssemblyState::new().topology(PrimitiveTopology::LineStrip))
            .viewport_state(ViewportState::viewport_dynamic_scissor_irrelevant())
            .fragment_shader(fs.entry_point("main").unwrap(), ())
            .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
            .build(device.clone())
            .unwrap();

        let render_image_framebuffer = Framebuffer::start(render_pass)
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

        let command_buffer = AutoCommandBufferBuilder::primary(
            device.clone(),
            queue.family(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .unwrap();

        Self {
            gpu_stat,
            gpu_read_sender,
            gpu_front_image_sender,

            allow_texture_disable: false,
            textured_rect_flip: (false, false),

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

            render_image_back_image,

            polygon_pipeline,
            line_pipeline,

            _vertex_buffer: vertex_buffer,

            front_blit: texture_blit,

            gpu_future,

            command_builder: command_buffer,
        }
    }

    pub(super) fn read_gpu_stat(&self) -> GpuStat {
        self.gpu_stat.load()
    }

    pub(super) fn write_gpu_stat(&self, stat: GpuStat) {
        self.gpu_stat.store(stat);
    }

    pub(super) fn send_to_gpu_read(&self, value: u32) {
        self.gpu_read_sender.send(value).unwrap();
    }
}

impl GpuContext {
    /// Drawing commands that use textures will update gpustat
    fn update_gpu_stat_from_texture_params(&mut self, texture_params: &DrawingTextureParams) {
        let x = (texture_params.tex_page_base[0] / 64) & 0xF;
        let y = (texture_params.tex_page_base[1] / 256) & 1;
        self.gpu_stat
            .fetch_update(|mut s| {
                s.bits &= !0x81FF;
                s.bits |= x;
                s.bits |= y << 4;
                s.bits |= (texture_params.semi_transparency_mode as u32) << 5;
                s.bits |= (texture_params.tex_page_color_mode as u32) << 7;
                s.bits |= (texture_params.texture_disable as u32) << 15;
                Some(s)
            })
            .unwrap();
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

        let buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::transfer_source(),
            false,
            block.iter().cloned(),
        )
        .unwrap();

        self.command_builder
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
    }

    pub fn read_vram_block(&mut self, block_range: &(Range<u32>, Range<u32>)) -> Vec<u16> {
        // TODO: check for out-of-bound reads here

        self.flush_command_builder();

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

        self.command_builder
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
    }

    fn new_command_buffer_builder(&mut self) -> AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> {
        let mut builder = AutoCommandBufferBuilder::primary(
            self.device.clone(),
            self.queue.family(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .unwrap();

        // copy to the back buffer
        builder
            .copy_image(
                self.render_image.clone(),
                [0, 0, 0],
                0,
                0,
                self.render_image_back_image.clone(),
                [0, 0, 0],
                0,
                0,
                [1024, 512, 1],
                1,
            )
            .unwrap();

        let sampler = Sampler::new(
            self.device.clone(),
            Filter::Nearest,
            Filter::Nearest,
            MipmapMode::Nearest,
            SamplerAddressMode::Repeat,
            SamplerAddressMode::Repeat,
            SamplerAddressMode::Repeat,
            0.0,
            1.0,
            0.0,
            0.0,
        )
        .unwrap();

        // even though, we are using the layout from the `polygon_pipeline`
        // it still works without issues with `polyline_pipeline` since its the
        // same layout taken from the same shader.
        let layout = self
            .polygon_pipeline
            .layout()
            .descriptor_set_layouts()
            .get(0)
            .unwrap();
        let mut set_builder = PersistentDescriptorSet::start(layout.clone());

        set_builder
            .add_sampled_image(
                ImageView::start(self.render_image_back_image.clone())
                    .with_component_mapping(ComponentMapping {
                        r: ComponentSwizzle::Blue,
                        b: ComponentSwizzle::Red,
                        ..Default::default()
                    })
                    .build()
                    .unwrap(),
                sampler,
            )
            .unwrap();

        let set = set_builder.build().unwrap();

        builder.bind_descriptor_sets(
            PipelineBindPoint::Graphics,
            self.polygon_pipeline.layout().clone(),
            0,
            set,
        );

        builder
    }

    fn flush_command_builder(&mut self) {
        let new_builder = self.new_command_buffer_builder();
        let command_buffer_builder = std::mem::replace(&mut self.command_builder, new_builder);

        let command_buffer = command_buffer_builder.build().unwrap();

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
        mut texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
    ) {
        self.gpu_future.as_mut().unwrap().cleanup_finished();

        let gpu_stat = self.read_gpu_stat();

        let vertex_buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::all(),
            false,
            vertices.iter().cloned(),
        )
        .unwrap();

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom - drawing_top + 1;
        let width = drawing_right - drawing_left + 1;

        if textured || semi_transparent {
            self.flush_command_builder();
        }

        if textured {
            if !self.allow_texture_disable {
                texture_params.texture_disable = false;
            }
            self.update_gpu_stat_from_texture_params(&texture_params);
        };

        let semi_transparency_mode = if textured {
            texture_params.semi_transparency_mode
        } else {
            gpu_stat.semi_transparency_mode()
        };

        let push_constants = fs::ty::PushConstantData {
            offset: [self.drawing_offset.0, self.drawing_offset.1],
            drawing_top_left: [left, top],
            drawing_size: [width, height],

            semi_transparent: semi_transparent as u32,
            semi_transparency_mode: semi_transparency_mode as u32,

            dither_enabled: gpu_stat.dither_enabled() as u32,

            is_textured: textured as u32,
            clut_base: texture_params.clut_base,
            tex_page_base: texture_params.tex_page_base,
            is_texture_blended: texture_blending as u32,
            tex_page_color_mode: texture_params.tex_page_color_mode as u32,
            texture_flip: [
                texture_params.texture_flip.0 as u32,
                texture_params.texture_flip.1 as u32,
            ],
        };

        self.command_builder
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
            .bind_pipeline_graphics(self.polygon_pipeline.clone())
            .push_constants(self.polygon_pipeline.layout().clone(), 0, push_constants)
            .bind_vertex_buffers(0, vertex_buffer)
            .draw(vertices.len() as u32, 1, 0, 0)
            .unwrap()
            .end_render_pass()
            .unwrap();
    }

    pub fn draw_polyline(&mut self, vertices: &[DrawingVertex], semi_transparent: bool) {
        self.gpu_future.as_mut().unwrap().cleanup_finished();
        let gpu_stat = self.read_gpu_stat();

        let vertex_buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::all(),
            false,
            vertices.iter().cloned(),
        )
        .unwrap();

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom - drawing_top + 1;
        let width = drawing_right - drawing_left + 1;

        if semi_transparent {
            self.flush_command_builder();
        }

        let semi_transparency_mode = gpu_stat.semi_transparency_mode();

        let push_constants = fs::ty::PushConstantData {
            offset: [self.drawing_offset.0, self.drawing_offset.1],
            drawing_top_left: [left, top],
            drawing_size: [width, height],

            semi_transparent: semi_transparent as u32,
            semi_transparency_mode: semi_transparency_mode as u32,

            dither_enabled: gpu_stat.dither_enabled() as u32,

            is_textured: false as u32,
            tex_page_base: [0; 2],
            clut_base: [0; 2],
            is_texture_blended: false as u32,
            tex_page_color_mode: 0,
            texture_flip: [0, 0],
        };

        self.command_builder
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
            .bind_pipeline_graphics(self.line_pipeline.clone())
            .push_constants(self.line_pipeline.layout().clone(), 0, push_constants)
            .bind_vertex_buffers(0, vertex_buffer)
            .draw(vertices.len() as u32, 1, 0, 0)
            .unwrap()
            .end_render_pass()
            .unwrap();
    }

    pub fn blit_to_front(&mut self, full_vram: bool) {
        let gpu_stat = self.read_gpu_stat();
        self.flush_command_builder();

        let (topleft, size) = if full_vram {
            ([0; 2], [1024, 512])
        } else {
            (
                [
                    self.vram_display_area_start.0,
                    self.vram_display_area_start.1,
                ],
                [
                    gpu_stat.horizontal_resolution(),
                    gpu_stat.vertical_resolution(),
                ],
            )
        };

        let front_image = StorageImage::new(
            self.device.clone(),
            ImageDimensions::Dim2d {
                width: size[0],
                height: size[1],
                array_layers: 1,
            },
            Format::B8G8R8A8_UNORM,
            Some(self.queue.family()),
        )
        .unwrap();

        // TODO: try to remove the `wait` from here
        self.front_blit
            .blit(
                front_image.clone(),
                topleft,
                size,
                self.gpu_future.take().unwrap(),
            )
            .then_signal_fence_and_flush()
            .unwrap()
            .wait(None)
            .unwrap();

        // send the front buffer
        self.gpu_front_image_sender.send(front_image).unwrap();

        // reset future since we are waiting
        self.gpu_future = Some(sync::now(self.device.clone()).boxed());
    }
}
